//! Deterministic, bounded source context assembled without subprocesses.
use crate::sandbox::profile::{Access, Profile};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, File},
    io::Read,
    path::Path,
};

const SOURCE_CAP: u64 = 1_048_576;
const SMALL: usize = 16_384;
const DEF_CAP: usize = 24_000;
const RENDER_CAP: usize = 30_000;
const SUPPORT_CAP: usize = 18;
const VISIT_CAP: usize = 2_048;
const SCAN_CAP: u64 = 131_072;
const SKIP: &[&str] = &[
    ".git",
    ".pane",
    ".worktrees",
    "target",
    "node_modules",
    "vendor",
    "build",
    "dist",
    ".venv",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRole {
    CompleteFile,
    TargetDefinition,
    Import,
    NearbyDefinition,
    Caller,
    Test,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceExcerpt {
    pub role: ContextRole,
    pub path: String,
    pub range: LineRange,
    pub text: String,
    pub complete: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceContext {
    pub path: String,
    pub sha256: String,
    pub language: String,
    pub symbol: Option<String>,
    pub target: SourceExcerpt,
    pub supporting: Vec<SourceExcerpt>,
    pub complete: bool,
    pub omissions: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextError(pub String);
impl fmt::Display for ContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ContextError {}

impl SourceContext {
    pub fn render(&self) -> String {
        let mut o = format!(
            "## Source context\npath: {}\nlanguage: {}\nsymbol: {}\nversion: {}\ncomplete: {}\n",
            self.path,
            self.language,
            self.symbol.as_deref().unwrap_or("(whole file)"),
            &self.sha256[..12],
            self.complete
        );
        render_one(&mut o, &self.target);
        for e in &self.supporting {
            render_one(&mut o, e)
        }
        for x in &self.omissions {
            o.push_str(&format!("omission: {x}\n"))
        }
        o
    }
}

pub fn pack(
    profile: &Profile,
    path: &Path,
    symbol: Option<&str>,
) -> Result<SourceContext, ContextError> {
    let path = profile
        .check("source context", Access::Read, path)
        .map_err(|e| ContextError(format!("refused: {}", e.rule)))?;
    let text = read(&path, SOURCE_CAP)?;
    let lines: Vec<&str> = text.lines().collect();
    let lang = Lang::of(&path);
    let rel = relative(profile, &path);
    let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
    let mut omissions = vec![];
    let inferred = (text.len() > SMALL && symbol.is_none())
        .then(|| infer_incomplete_symbol(&lines, lang))
        .flatten();
    let selected_symbol = symbol.or(inferred.as_deref());
    if symbol.is_none() && inferred.is_some() {
        omissions.push("target symbol inferred from the file's unique incomplete marker".into());
    }
    let (role, range, complete) = if text.len() <= SMALL {
        (ContextRole::CompleteFile, (0, lines.len()), true)
    } else if let Some(name) = selected_symbol {
        if let Some(r) = definition(&lines, name, lang) {
            (ContextRole::TargetDefinition, r, true)
        } else {
            let i = lines
                .iter()
                .position(|l| has_ident(l, name))
                .ok_or_else(|| {
                    ContextError(format!("symbol `{name}` was not found; nothing packed"))
                })?;
            omissions.push(
                "complete definition boundary unavailable; target is a language-agnostic window"
                    .into(),
            );
            (
                ContextRole::TargetDefinition,
                (i.saturating_sub(20), (i + 21).min(lines.len())),
                false,
            )
        }
    } else {
        let n = bounded_prefix(&lines, 500, DEF_CAP);
        if n < lines.len() {
            omissions.push(format!(
                "target is an incomplete first {n} of {} lines; supply `symbol` for a complete editing target",
                lines.len()
            ))
        }
        (ContextRole::CompleteFile, (0, n), n == lines.len())
    };
    let body = slice(&lines, range);
    if body.len() > DEF_CAP {
        return Err(ContextError(format!(
            "target definition exceeds the {DEF_CAP} byte cap; nothing packed"
        )));
    }
    let target = make(role, rel.clone(), range, body, complete);
    let mut supporting = vec![];
    if text.len() > SMALL {
        supporting.extend(imports(&lines, &rel, lang));
        if complete {
            supporting.extend(nearby(&lines, &rel, range, lang))
        }
    }
    if let Some(name) = selected_symbol {
        let (mut refs, notes) = references(profile, &path, name);
        supporting.append(&mut refs);
        omissions.extend(notes)
    }
    if supporting.len() > SUPPORT_CAP {
        omissions.push(format!(
            "{} lower-ranked supporting excerpts omitted at the {SUPPORT_CAP}-excerpt cap",
            supporting.len() - SUPPORT_CAP
        ));
        supporting.truncate(SUPPORT_CAP);
    }
    let mut result = SourceContext {
        path: rel,
        sha256: hash,
        language: lang.name().into(),
        symbol: selected_symbol.map(str::to_owned),
        target,
        supporting,
        complete,
        omissions,
    };
    while result.render().len() > RENDER_CAP && !result.supporting.is_empty() {
        result.supporting.pop();
        result
            .omissions
            .push("one lower-ranked supporting excerpt omitted to fit the delivery cap".into());
    }
    if result.render().len() > RENDER_CAP {
        return Err(ContextError(format!(
            "context exceeds the {RENDER_CAP} byte render cap; nothing packed"
        )));
    }
    Ok(result)
}

/// Returns the one definition a broad source read can safely promote to an
/// editing context. Ordinary files and ambiguous TODOs stay ordinary reads.
pub fn infer_incomplete_target(
    profile: &Profile,
    path: &Path,
) -> Result<Option<String>, ContextError> {
    let path = profile
        .check("source context", Access::Read, path)
        .map_err(|error| ContextError(format!("refused: {}", error.rule)))?;
    let text = read(&path, SOURCE_CAP)?;
    if text.len() <= SMALL {
        return Ok(None);
    }
    let lines: Vec<&str> = text.lines().collect();
    Ok(infer_incomplete_symbol(&lines, Lang::of(&path)))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Py,
    Rs,
    Other,
}
impl Lang {
    fn of(p: &Path) -> Self {
        match p.extension().and_then(|x| x.to_str()) {
            Some("py") => Self::Py,
            Some("rs") => Self::Rs,
            _ => Self::Other,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Py => "python",
            Self::Rs => "rust",
            Self::Other => "other",
        }
    }
}
fn read(p: &Path, cap: u64) -> Result<String, ContextError> {
    let m = fs::metadata(p)
        .map_err(|e| ContextError(format!("could not inspect {}: {e}", p.display())))?;
    if !m.is_file() {
        return Err(ContextError(format!(
            "{} is not a regular file",
            p.display()
        )));
    }
    if m.len() > cap {
        return Err(ContextError(format!(
            "{} exceeds the {cap} byte source cap",
            p.display()
        )));
    }
    let mut b = vec![];
    File::open(p)
        .and_then(|f| f.take(cap + 1).read_to_end(&mut b))
        .map_err(|e| ContextError(format!("could not read {}: {e}", p.display())))?;
    if b.len() as u64 > cap {
        return Err(ContextError(format!(
            "{} grew beyond the {cap} byte source cap",
            p.display()
        )));
    }
    String::from_utf8(b).map_err(|_| ContextError(format!("{} is not UTF-8 source", p.display())))
}
fn definition(l: &[&str], n: &str, g: Lang) -> Option<(usize, usize)> {
    match g {
        Lang::Py => py_def(l, n),
        Lang::Rs => rs_def(l, n),
        _ => None,
    }
}
fn infer_incomplete_symbol(lines: &[&str], lang: Lang) -> Option<String> {
    let markers: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            ["NotImplementedError", "TODO", "todo!", "unimplemented!"]
                .iter()
                .any(|marker| line.contains(marker))
        })
        .map(|(index, _)| index)
        .collect();
    if markers.len() != 1 {
        return None;
    }
    let marker = markers[0];
    (0..=marker).rev().find_map(|index| {
        let candidate = name(lines[index], lang)?;
        let range = definition(lines, candidate, lang)?;
        (marker >= range.0 && marker < range.1).then(|| candidate.to_string())
    })
}
fn py_decl(l: &str, n: &str) -> bool {
    let t = l.trim_start();
    [
        format!("def {n}"),
        format!("async def {n}"),
        format!("class {n}"),
    ]
    .iter()
    .any(|p| {
        t.strip_prefix(p)
            .is_some_and(|r| r.starts_with('(') || r.starts_with(':'))
    })
}
fn py_def(l: &[&str], n: &str) -> Option<(usize, usize)> {
    let i = l.iter().position(|x| py_decl(x, n))?;
    let ind = indent(l[i]);
    let mut s = i;
    while s > 0 && l[s - 1].trim_start().starts_with('@') {
        s -= 1
    }
    let e = (i + 1..l.len())
        .find(|&j| {
            let t = l[j].trim();
            !t.is_empty() && indent(l[j]) <= ind
        })
        .unwrap_or(l.len());
    Some((s, e))
}
fn rs_decl(l: &str, n: &str) -> bool {
    let code = l.split("//").next().unwrap_or("");
    ["fn", "struct", "enum", "trait", "type", "union"]
        .iter()
        .any(|k| {
            let q = format!("{k} {n}");
            code.match_indices(&q)
                .any(|(i, _)| boundary(code, i, q.len()))
        })
}
fn rs_def(l: &[&str], n: &str) -> Option<(usize, usize)> {
    let i = l.iter().position(|x| rs_decl(x, n))?;
    let mut s = i;
    while s > 0 {
        let t = l[s - 1].trim_start();
        if t.starts_with("#[") || t.starts_with("///") {
            s -= 1
        } else {
            break;
        }
    }
    let mut lex = Lexer::default();
    let mut d: isize = 0;
    let mut opened = false;
    for (j, x) in l.iter().enumerate().skip(i) {
        let (a, b, semi) = lex.scan(x);
        d += a as isize - b as isize;
        opened |= a > 0;
        if (opened && d == 0) || (!opened && semi) {
            return Some((s, j + 1));
        }
    }
    None
}
#[derive(Default)]
struct Lexer {
    block: usize,
    raw: Option<usize>,
    string: bool,
    character: bool,
    escape: bool,
}
impl Lexer {
    fn scan(&mut self, l: &str) -> (usize, usize, bool) {
        let b = l.as_bytes();
        let (mut i, mut a, mut z, mut semi) = (0, 0, 0, false);
        while i < b.len() {
            if let Some(hashes) = self.raw {
                if b[i] == b'"'
                    && b.get(i + 1..i + 1 + hashes)
                        .is_some_and(|tail| tail.iter().all(|&c| c == b'#'))
                {
                    self.raw = None;
                    i += hashes + 1;
                } else {
                    i += 1;
                }
                continue;
            }
            if self.block > 0 {
                if b.get(i..i + 2) == Some(b"/*") {
                    self.block += 1;
                    i += 2
                } else if b.get(i..i + 2) == Some(b"*/") {
                    self.block -= 1;
                    i += 2
                } else {
                    i += 1
                }
                continue;
            }
            if self.string || self.character {
                if self.escape {
                    self.escape = false;
                    i += 1;
                    continue;
                }
                if b[i] == b'\\' {
                    self.escape = true;
                    i += 1;
                    continue;
                }
                if (self.string && b[i] == b'"') || (self.character && b[i] == b'\'') {
                    self.string = false;
                    self.character = false
                }
                i += 1;
                continue;
            }
            if b.get(i..i + 2) == Some(b"//") {
                break;
            }
            if b.get(i..i + 2) == Some(b"/*") {
                self.block = 1;
                i += 2;
                continue;
            }
            if b[i] == b'r' {
                let hashes = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
                if b.get(i + hashes + 1) == Some(&b'"') {
                    self.raw = Some(hashes);
                    i += hashes + 2;
                    continue;
                }
            }
            match b[i] {
                b'"' => self.string = true,
                // Lifetimes have no closing quote. Only a short, closed
                // literal may hide braces from the definition scanner.
                b'\''
                    if b[i + 1..]
                        .iter()
                        .position(|&c| c == b'\'')
                        .is_some_and(|distance| distance <= 5) =>
                {
                    self.character = true
                }
                b'{' => a += 1,
                b'}' => z += 1,
                b';' => semi = true,
                _ => {}
            }
            i += 1
        }
        (a, z, semi)
    }
}
fn imports(l: &[&str], p: &str, g: Lang) -> Vec<SourceExcerpt> {
    l.iter()
        .enumerate()
        .filter(|(_, x)| {
            let t = x.trim_start();
            match g {
                Lang::Py => t.starts_with("import ") || t.starts_with("from "),
                Lang::Rs => {
                    t.starts_with("use ") || t.starts_with("pub use ") || t.starts_with("mod ")
                }
                _ => false,
            }
        })
        .take(6)
        // This deliberately does not claim a one-line excerpt is a complete
        // multi-line import declaration.
        .map(|(i, x)| {
            make(
                ContextRole::Import,
                p.into(),
                (i, i + 1),
                (*x).into(),
                false,
            )
        })
        .collect()
}
fn name(l: &str, g: Lang) -> Option<&str> {
    let t = l.trim_start();
    let ks: &[&str] = match g {
        Lang::Py => &["def ", "async def ", "class "],
        Lang::Rs => &[
            "pub fn ",
            "fn ",
            "pub struct ",
            "struct ",
            "pub enum ",
            "enum ",
            "trait ",
            "type ",
        ],
        _ => &[],
    };
    ks.iter().find_map(|k| {
        t.strip_prefix(k)
            .and_then(|r| {
                r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .next()
            })
            .filter(|x| !x.is_empty())
    })
}
fn nearby(l: &[&str], p: &str, target: (usize, usize), g: Lang) -> Vec<SourceExcerpt> {
    let mut v = vec![];
    for i in target.0.saturating_sub(120)..(target.1 + 120).min(l.len()) {
        if i >= target.0 && i < target.1 {
            continue;
        }
        if let Some(n) = name(l[i], g)
            && let Some(r) = definition(l, n, g)
            && (r.1 <= target.0 || r.0 >= target.1)
        {
            v.push((i.abs_diff(target.0), r))
        }
    }
    v.sort();
    v.dedup_by_key(|x| x.1);
    v.into_iter()
        .take(4)
        .map(|(_, r)| {
            make(
                ContextRole::NearbyDefinition,
                p.into(),
                r,
                slice(l, r),
                false,
            )
        })
        .collect()
}
fn references(profile: &Profile, target: &Path, symbol: &str) -> (Vec<SourceExcerpt>, Vec<String>) {
    let mut stack = vec![(profile.root().to_path_buf(), 0)];
    let (mut files, mut visited, mut refused, mut cutoff) = (vec![], 0, 0, false);
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        let mut es: Vec<_> = rd.flatten().collect();
        es.sort_by_key(|e| e.file_name());
        for e in es {
            if visited == VISIT_CAP {
                cutoff = true;
                break;
            }
            visited += 1;
            let p = e.path();
            if profile.check("source context", Access::Read, &p).is_err() {
                refused += 1;
                continue;
            }
            let Ok(k) = e.file_type() else { continue };
            let n = e.file_name();
            let n = n.to_string_lossy();
            if k.is_dir() && !k.is_symlink() && depth < 5 && !SKIP.contains(&n.as_ref()) {
                stack.push((p, depth + 1))
            } else if k.is_file() && source(&p) {
                files.push(p)
            }
        }
        if cutoff {
            break;
        }
        stack.sort_by(|a, b| b.0.cmp(&a.0))
    }
    files.sort();
    let mut hits = vec![];
    for p in files {
        let Ok(t) = read(&p, SCAN_CAP) else { continue };
        let l: Vec<&str> = t.lines().collect();
        let Some(i) = l
            .iter()
            .position(|x| has_ident(x, symbol) && (p != target || !def_line(x, symbol)))
        else {
            continue;
        };
        let rel = relative(profile, &p);
        let test = is_test(&rel);
        let score = usize::from(test) * 4
            + usize::from(l[i].contains(&format!("{symbol}("))) * 2
            + usize::from(p.extension() == target.extension());
        let r = (i.saturating_sub(2), (i + 3).min(l.len()));
        hits.push((
            std::cmp::Reverse(score),
            rel.clone(),
            i,
            make(
                if test {
                    ContextRole::Test
                } else {
                    ContextRole::Caller
                },
                rel,
                r,
                slice(&l, r),
                false,
            ),
        ))
    }
    hits.sort_by(|a, b| (&a.0, &a.1, a.2).cmp(&(&b.0, &b.1, b.2)));
    let mut notes = vec![format!(
        "reference index visited {visited} entries; {refused} were refused"
    )];
    if cutoff {
        notes.push(format!(
            "reference traversal stopped at the {VISIT_CAP}-entry cap"
        ))
    }
    if hits.len() > 12 {
        notes.push(format!(
            "{} lower-ranked references omitted",
            hits.len() - 12
        ))
    }
    (hits.into_iter().take(12).map(|x| x.3).collect(), notes)
}
fn make(
    role: ContextRole,
    path: String,
    r: (usize, usize),
    text: String,
    complete: bool,
) -> SourceExcerpt {
    SourceExcerpt {
        role,
        path,
        range: LineRange {
            start: r.0 + 1,
            end: r.1,
        },
        text,
        complete,
    }
}
fn slice(l: &[&str], r: (usize, usize)) -> String {
    l[r.0..r.1].join("\n")
}
fn bounded_prefix(lines: &[&str], line_cap: usize, byte_cap: usize) -> usize {
    let mut bytes = 0;
    lines
        .iter()
        .take(line_cap)
        .take_while(|line| {
            let extra = line.len() + usize::from(bytes > 0);
            if bytes + extra > byte_cap {
                false
            } else {
                bytes += extra;
                true
            }
        })
        .count()
}
fn indent(l: &str) -> usize {
    l.len() - l.trim_start().len()
}
fn has_ident(l: &str, n: &str) -> bool {
    l.match_indices(n).any(|(i, _)| boundary(l, i, n.len()))
}
fn boundary(l: &str, i: usize, n: usize) -> bool {
    l[..i].chars().next_back().is_none_or(|c| !ident(c))
        && l[i + n..].chars().next().is_none_or(|c| !ident(c))
}
fn ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
fn def_line(l: &str, n: &str) -> bool {
    py_decl(l, n) || rs_decl(l, n)
}
fn source(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|x| x.to_str()),
        Some("rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go" | "java" | "c" | "h" | "cpp" | "cc")
    )
}
fn is_test(p: &str) -> bool {
    p.split('/').any(|x| x == "tests" || x == "test") || p.contains("_test.") || p.contains("test_")
}
fn relative(profile: &Profile, p: &Path) -> String {
    p.strip_prefix(profile.root())
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}
fn render_one(o: &mut String, e: &SourceExcerpt) {
    o.push_str(&format!(
        "\n### {:?}: {}:{}-{} [{}]\n",
        e.role,
        e.path,
        e.range.start,
        e.range.end,
        if e.complete { "complete" } else { "excerpt" }
    ));
    for (i, l) in e.text.lines().enumerate() {
        o.push_str(&format!("{:>5} | {l}\n", e.range.start + i))
    }
}
