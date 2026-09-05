//! One immutable profile compiled from `.claude/settings.json`'s
//! `permissions`, and the pre-call check that answers every path question
//! from it — map line 2455, specification
//! `docs/product/pane/sandbox-grants.md`.
//!
//! The invariant the whole module exists for: **a profile is built once and
//! can never be widened afterwards.** That is enforced by the type rather
//! than by a comment — [`Profile`] has no public field, no setter, no method
//! taking a mutable receiver and no shared-mutable interior, so there is no
//! expression a later module could write that adds a grant. It also holds no
//! handle to the document it was compiled from, which is why re-reading
//! `.claude/settings.json` mid-session — the widening path §1.5 names, since
//! `.claude/` lives inside the writable project root — is not something a
//! caller can accidentally do.
//!
//! Two questions, answered in that order and never conflated (§2):
//! [`Profile::admits_command`] asks whether a command line may be attempted,
//! and [`Profile::check`] asks what any process may touch. A `Bash` pattern
//! answers the first and contributes nothing to the second.

use crate::contract::ProjectConfig;
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

/// Which half of a file grant a call needs. An editing tool asks both, in
/// whichever order it performs them; there is no combined variant, because a
/// single answer would hide which half was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

impl Access {
    pub fn as_str(self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Write => "write",
        }
    }

    /// The refusal sentence for a path no grant covers, §5.
    fn only_root_sentence(self) -> &'static str {
        match self {
            Access::Read => "no grant covers this path; the project root is the only readable root",
            Access::Write => {
                "no grant covers this path; the project root is the only writable root"
            }
        }
    }
}

/// A refusal, as a value.
///
/// It is returned, never raised as a question: nothing in this module reads
/// from a terminal and nothing here widens anything (§1.4). `rule`
/// names the *deciding* rule — the `deny` entry that matched, the
/// never-grantable entry that matched, or the absence of any `allow` — so a
/// person reading a transcript can fix the settings file without
/// re-deriving the profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionDenied {
    pub tool: String,
    /// The path asked for, as it was resolved. For an argv refusal this is
    /// the command line, because that is what was refused.
    pub path: String,
    pub rule: String,
}

impl fmt::Display for PermissionDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PermissionDenied: {}(\"{}\")\n  rule: {}\n  tool: {}",
            self.tool, self.path, self.rule, self.tool
        )
    }
}

impl std::error::Error for PermissionDenied {}

/// One filesystem rule, keeping the pattern as written so a refusal can quote
/// it back.
#[derive(Debug, Clone)]
struct PathRule {
    written: String,
    glob: Vec<String>,
    read: bool,
    write: bool,
}

/// One entry of §4's never-grantable set, expressed as a resolved subtree.
#[derive(Debug, Clone)]
struct NeverRule {
    prefix: PathBuf,
    /// `true` for `.claude/**`, which is never writable but stays readable —
    /// `settings.json` is read before the sandbox is entered (§1.5).
    write_only: bool,
    rule: String,
}

/// The compiled profile.
///
/// Every field is private and every method takes a shared receiver, which is
/// the mechanism behind §1.1: there is no way to widen this after
/// [`Profile::compile`] returns.
#[derive(Debug, Clone)]
pub struct Profile {
    root: PathBuf,
    home: Option<PathBuf>,
    allow: Vec<PathRule>,
    deny: Vec<PathRule>,
    never: Vec<NeverRule>,
    command_allow: Vec<String>,
    command_deny: Vec<String>,
    mcp_allow: BTreeSet<String>,
    mcp_deny: BTreeSet<String>,
    diagnostics: Vec<String>,
}

/// The five `$HOME` directories §4.3 names, refusable by no pattern at all.
const NEVER_GRANTABLE_HOME: [&str; 5] = [".claude", ".codex", ".ssh", ".aws", ".config"];

/// Command names that re-enter the sandbox launcher or attach a debugger,
/// §4.6. Matched on every word of a command line, stripped of its directory
/// part, so `sh -c "sandbox-exec …"` is the same refusal as `sandbox-exec`.
const NEVER_GRANTABLE_COMMANDS: [&str; 12] = [
    "sandbox-exec",
    "bwrap",
    "bubblewrap",
    "lldb",
    "gdb",
    "strace",
    "ltrace",
    "dtrace",
    "dtruss",
    "windbg",
    "x64dbg",
    "vsjitdebugger.exe",
];

impl Profile {
    /// Compiles the profile a project's loaded configuration implies.
    ///
    /// This is the propagation path in one line: `project::load` fills
    /// `ProjectConfig::settings` with the document's exact bytes, and nothing
    /// between there and here repairs them.
    pub fn from_project(config: &ProjectConfig) -> Self {
        Self::compile(&config.root, config.settings.as_deref())
    }

    /// Compiles the profile for `root` from `settings`, the verbatim text of
    /// `.claude/settings.json`.
    ///
    /// A document that cannot be understood grants nothing rather than
    /// everything: it is untrusted input from a directory invariant 3 makes
    /// writable, so a parse failure, an unknown pattern kind and an unknown
    /// `permissions` key each add a diagnostic and no rule.
    pub fn compile(root: impl AsRef<Path>, settings: Option<&str>) -> Self {
        let root = resolve(root.as_ref(), None, None);
        let home = home_dir().map(|home| resolve(&home, None, None));
        let mut profile = Self {
            never: never_rules(&root, home.as_deref()),
            root,
            home,
            allow: Vec::new(),
            deny: Vec::new(),
            command_allow: Vec::new(),
            command_deny: Vec::new(),
            mcp_allow: BTreeSet::new(),
            mcp_deny: BTreeSet::new(),
            diagnostics: Vec::new(),
        };
        let Some(text) = settings else {
            return profile;
        };
        let Ok(document) = serde_json::from_str::<serde_json::Value>(text) else {
            profile
                .diagnostics
                .push("`.claude/settings.json` is not valid JSON; it grants nothing".to_string());
            return profile;
        };
        let Some(permissions) = document.get("permissions") else {
            return profile;
        };
        let Some(permissions) = permissions.as_object() else {
            profile
                .diagnostics
                .push("`permissions` is not an object; it grants nothing".to_string());
            return profile;
        };
        for (key, value) in permissions {
            match key.as_str() {
                "allow" | "deny" => {}
                "ask" => {
                    profile.diagnostics.push(
                        "`permissions.ask` grants nothing: a refusal is a value that is returned, never a question put to a user"
                            .to_string(),
                    );
                    continue;
                }
                other => {
                    profile.diagnostics.push(format!(
                        "`permissions.{other}` is not a pattern list and grants nothing"
                    ));
                    continue;
                }
            }
            let denying = key == "deny";
            let Some(entries) = value.as_array() else {
                profile.diagnostics.push(format!(
                    "`permissions.{key}` is not an array; it grants nothing"
                ));
                continue;
            };
            for entry in entries {
                let Some(pattern) = entry.as_str() else {
                    profile.diagnostics.push(format!(
                        "a non-string entry in `permissions.{key}` grants nothing"
                    ));
                    continue;
                };
                register(&mut profile, pattern, denying);
            }
        }
        profile
    }
}

/// Maps one written pattern to its kind and records it, per §2's table.
///
/// A free function rather than a method, and deliberately: [`Profile`] has
/// no method at all that takes a mutable receiver, so the only place a rule
/// can be added is inside [`Profile::compile`], before the value exists for
/// anyone else to hold.
fn register(profile: &mut Profile, pattern: &str, denying: bool) {
    let (name, argument) = split_pattern(pattern);
    let (read, write) = match name {
        "Read" => (true, false),
        "Write" => (false, true),
        "Edit" => (true, true),
        "Bash" => {
            // Argv admission, and nothing in the filesystem profile. A
            // bare `Bash` admits every command line; the profile is
            // unchanged either way.
            let admitted = argument.unwrap_or("*").to_string();
            if denying {
                profile.command_deny.push(admitted);
            } else {
                profile.command_allow.push(admitted);
            }
            return;
        }
        "WebFetch" | "WebSearch" => {
            // Network is never granted (§4.1). It is not registered and
            // not carried as a disabled rule: a network-needing tool is
            // absent, not present and failing.
            return;
        }
        other if other.starts_with("mcp__") => {
            if denying {
                profile.mcp_deny.insert(other.to_string());
            } else {
                profile.mcp_allow.insert(other.to_string());
            }
            return;
        }
        other => {
            profile.diagnostics.push(format!(
                "`{other}` is not a pattern kind this profile understands; `{pattern}` grants nothing"
            ));
            return;
        }
    };
    let Some(argument) = argument.map(str::trim).filter(|arg| !arg.is_empty()) else {
        profile.diagnostics.push(format!(
            "`{pattern}` names no path; a bare `{name}` grants nothing"
        ));
        return;
    };
    let glob = resolve_pattern(&profile.root, profile.home.as_deref(), argument);
    if glob.is_empty() {
        profile.diagnostics.push(format!(
            "`{pattern}` resolves to no path at all; it grants nothing"
        ));
        return;
    }
    let rule = PathRule {
        written: pattern.to_string(),
        glob,
        read,
        write,
    };
    if denying {
        // A `deny` entry refuses the paths it matches outright, whichever
        // verb spells it: §1.2 and §4.5 state the refusal path-wide, and
        // refusing more than was asked is the safe direction for a
        // document that is itself untrusted input.
        profile.deny.push(rule);
    } else {
        profile.allow.push(rule);
    }
}

impl Profile {
    /// The project root, resolved.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// What the document said that this profile did not act on. Empty for a
    /// document every entry of which mapped to a rule.
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// How many filesystem rules the document produced. A `Bash` or
    /// `WebFetch` pattern leaves this untouched, which is what makes §2's
    /// separation observable rather than asserted.
    pub fn rule_count(&self) -> usize {
        self.allow.len() + self.deny.len()
    }

    /// How many command-line patterns were admitted.
    pub fn command_pattern_count(&self) -> usize {
        self.command_allow.len()
    }

    /// How many MCP tools were admitted.
    pub fn mcp_tool_count(&self) -> usize {
        self.mcp_allow.len()
    }

    /// Whether this profile grants any network reach. Always `false`: no
    /// `permissions` pattern names a host, a port or a protocol, so a network
    /// grant would have to be invented, and an invented capability is the one
    /// thing an allow-list must never produce (§4.1).
    pub fn grants_network(&self) -> bool {
        false
    }

    /// Whether an MCP tool is registered. A tool named in `deny` is not, and
    /// a network-needing tool never is.
    pub fn admits_mcp_tool(&self, name: &str) -> bool {
        if name.eq_ignore_ascii_case("webfetch") || name.eq_ignore_ascii_case("websearch") {
            return false;
        }
        !self.mcp_deny.contains(name) && self.mcp_allow.contains(name)
    }

    /// The first question of §2: may this command line be attempted at all?
    ///
    /// Answering `Ok` grants no file access whatsoever — the process it
    /// spawns gets exactly the grants the `Read`/`Write`/`Edit` patterns
    /// produced, which [`Profile::check`] is what answers.
    pub fn admits_command(&self, command_line: &str) -> Result<(), PermissionDenied> {
        let denied = |rule: String| -> Result<(), PermissionDenied> {
            Err(PermissionDenied {
                tool: "Bash".to_string(),
                path: command_line.to_string(),
                rule,
            })
        };
        if let Some(name) = escaping_command(command_line) {
            return denied(format!(
                "`{name}` re-enters the sandbox launcher or attaches a debugger and is never grantable by any pattern (sandbox-grants.md §4.6)"
            ));
        }
        for pattern in &self.command_deny {
            if match_segment(pattern, command_line, true) {
                return denied(format!("`Bash({pattern})` in permissions.deny"));
            }
        }
        if self
            .command_allow
            .iter()
            .any(|pattern| match_segment(pattern, command_line, false))
        {
            return Ok(());
        }
        denied("no `Bash` pattern in permissions.allow admits this command line".to_string())
    }

    /// The second question of §2: may `tool` touch `path` for `access`?
    ///
    /// Decided in the only order that keeps §1.2 true — never-grantable
    /// first, then `deny`, then `allow` — so no `allow`, however exact, can
    /// reach past either. `path` is resolved before matching: `~` expands, a
    /// relative path resolves against the project root, and symlinks are
    /// resolved as far as the path exists, because two spellings of one path
    /// are how a containment check comes to disagree with itself.
    pub fn check(&self, tool: &str, access: Access, path: &Path) -> Result<(), PermissionDenied> {
        let resolved = resolve(path, Some(&self.root), self.home.as_deref());
        let denied = |rule: String| -> Result<(), PermissionDenied> {
            Err(PermissionDenied {
                tool: tool.to_string(),
                path: display(&resolved),
                rule,
            })
        };
        for never in &self.never {
            if (!never.write_only || access == Access::Write) && resolved.starts_with(&never.prefix)
            {
                return denied(never.rule.clone());
            }
        }
        let candidate = components(&resolved);
        for rule in &self.deny {
            if covers(&rule.glob, &candidate, true) {
                return denied(format!("`{}` in permissions.deny", rule.written));
            }
        }
        if resolved.starts_with(&self.root) {
            return Ok(());
        }
        let granted = self.allow.iter().any(|rule| {
            let wanted = match access {
                Access::Read => rule.read,
                Access::Write => rule.write,
            };
            wanted && covers(&rule.glob, &candidate, false)
        });
        if granted {
            return Ok(());
        }
        denied(access.only_root_sentence().to_string())
    }
}

/// Builds §4's never-grantable set for one project root.
///
/// An entry whose subtree contains the project root, or lies inside it, is
/// skipped: a project checked out under one of these directories would
/// otherwise refuse every path in itself. `.claude/**` inside the root is the
/// deliberate exception, and it is write-only.
fn never_rules(root: &Path, home: Option<&Path>) -> Vec<NeverRule> {
    let mut rules = vec![NeverRule {
        prefix: root.join(".claude"),
        write_only: true,
        rule: "`.claude/**` is never writable: a program that could edit it could widen the profile it was derived from (sandbox-grants.md §1.5)".to_string(),
    }];
    let mut push = |prefix: PathBuf, rule: String| {
        let prefix = resolve(&prefix, None, None);
        if root.starts_with(&prefix) || prefix.starts_with(root) {
            return;
        }
        rules.push(NeverRule {
            prefix,
            write_only: false,
            rule,
        });
    };
    let keyring = "the OS keyring or credential store is never grantable by any pattern (sandbox-grants.md §4.2)";
    if let Some(home) = home {
        for name in NEVER_GRANTABLE_HOME {
            push(
                home.join(name),
                format!("`~/{name}` is never grantable by any pattern (sandbox-grants.md §4.3)"),
            );
        }
        for path in [
            home.join("Library").join("Keychains"),
            home.join(".local").join("share").join("keyrings"),
            home.join(".gnupg"),
        ] {
            push(path, keyring.to_string());
        }
    }
    push(PathBuf::from("/Library/Keychains"), keyring.to_string());
    for state in glasshouse_state_dirs(home) {
        push(
            state,
            "Glasshouse's own state and data directories, and every database in them, are never grantable by any pattern (sandbox-grants.md §4.4)".to_string(),
        );
    }
    rules
}

/// Every shape Glasshouse's own state and data directories take, on every
/// platform, computed without a `#[cfg]` so a profile compiled on one host
/// still refuses the paths another host would use.
fn glasshouse_state_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for key in [
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        if let Some(value) = std::env::var_os(key)
            && !value.is_empty()
        {
            roots.push(PathBuf::from(value));
        }
    }
    if let Some(home) = home {
        roots.push(home.join(".local").join("share"));
        roots.push(home.join(".local").join("state"));
        roots.push(home.join(".cache"));
        roots.push(home.join("Library").join("Application Support"));
        roots.push(home.join("Library").join("Caches"));
        roots.push(home.join("AppData").join("Roaming"));
        roots.push(home.join("AppData").join("Local"));
    }
    let mut dirs: Vec<PathBuf> = roots
        .into_iter()
        .map(|root| root.join("glasshouse"))
        .collect();
    if let Some(home) = home {
        dirs.push(home.join(".glasshouse"));
    }
    for key in ["GLASSHOUSE_STATE_DIR", "GLASSHOUSE_DATA_DIR"] {
        if let Some(value) = std::env::var_os(key)
            && !value.is_empty()
        {
            dirs.push(PathBuf::from(value));
        }
    }
    dirs
}

/// The word of `command_line` naming one of §4.6's escapes, if any.
///
/// Every word is examined rather than the first, because `sh -c "bwrap …"`
/// and `sudo lldb` are the spellings a first-word check walks straight past;
/// a command line that merely mentions one of these names is refused too,
/// which is the direction a never-grantable set must err in.
fn escaping_command(command_line: &str) -> Option<&'static str> {
    command_line.split_whitespace().find_map(|word| {
        let base = word
            .trim_matches(['"', '\'', '`', '(', ')', ';', '&', '|'])
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(word);
        NEVER_GRANTABLE_COMMANDS
            .iter()
            .copied()
            .find(|name| base.eq_ignore_ascii_case(name))
    })
}

/// `$HOME`, read from the environment rather than from a platform helper so
/// this stays one code path on every host.
fn home_dir() -> Option<PathBuf> {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(key)
            && !value.is_empty()
        {
            return Some(PathBuf::from(value));
        }
    }
    None
}

/// Resolves a candidate path the way every comparison in this module needs
/// it: `~` expands, a relative path resolves against `root`, `.` and `..` are
/// removed, and symlinks are resolved for as much of the path as exists.
///
/// The tail that does not exist yet — a file about to be created — is
/// appended to the resolved prefix, so a write check decides on the same
/// spelling a later read of that file would.
fn resolve(path: &Path, root: Option<&Path>, home: Option<&Path>) -> PathBuf {
    let mut expanded = expand_tilde(path, home);
    if !expanded.is_absolute()
        && let Some(root) = root
    {
        expanded = root.join(expanded);
    }
    let lexical = normalize(&expanded);
    let mut tail = Vec::new();
    let mut prefix = lexical;
    while !prefix.exists() {
        let Some(name) = prefix.file_name().map(|name| name.to_os_string()) else {
            break;
        };
        tail.push(name);
        if !prefix.pop() {
            break;
        }
    }
    let mut resolved = std::fs::canonicalize(&prefix).unwrap_or(prefix);
    for name in tail.iter().rev() {
        resolved.push(name);
    }
    resolved
}

fn expand_tilde(path: &Path, home: Option<&Path>) -> PathBuf {
    let text = path.to_string_lossy();
    let Some(rest) = text.strip_prefix('~') else {
        return path.to_path_buf();
    };
    if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\')) {
        return path.to_path_buf();
    }
    let Some(home) = home.map(PathBuf::from).or_else(home_dir) else {
        return path.to_path_buf();
    };
    let rest = rest.trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        home
    } else {
        home.join(rest)
    }
}

/// Removes `.` and `..` textually. Run before the physical resolution above,
/// which then re-resolves whatever of the result exists.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Splits a path into `/`-separated components, with `\` treated as a
/// separator too so one matcher serves every host.
fn components(path: &Path) -> Vec<String> {
    display(path)
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether a written pattern names an absolute path, a `~`-rooted one, or a
/// Windows drive — the three that are not resolved against the project root.
fn is_rooted(pattern: &str) -> bool {
    pattern.starts_with('/')
        || pattern == "~"
        || pattern.starts_with("~/")
        || pattern
            .as_bytes()
            .get(1)
            .is_some_and(|byte| *byte == b':' && pattern.as_bytes()[0].is_ascii_alphabetic())
}

/// Resolves a written pattern's literal prefix and keeps its glob tail.
///
/// The prefix is resolved so a pattern and a candidate spelled differently —
/// `/tmp/…` against `/private/tmp/…` — cannot disagree; the tail is left
/// alone because a glob names no single path to resolve. A pattern that
/// names no root is anchored at the project root before either step, so a
/// project-relative glob cannot match its way out of the project.
fn resolve_pattern(root: &Path, home: Option<&Path>, pattern: &str) -> Vec<String> {
    let normalized = pattern.replace('\\', "/");
    let anchored = if is_rooted(&normalized) {
        normalized
    } else {
        format!("{}/{normalized}", display(root).replace('\\', "/"))
    };
    let mut literal = Vec::new();
    let mut rest = Vec::new();
    for part in anchored.split('/') {
        if rest.is_empty() && !part.contains(['*', '?']) {
            literal.push(part.to_string());
        } else {
            rest.push(part.to_string());
        }
    }
    let literal_path = literal.join("/");
    let mut out = if literal_path.is_empty() {
        Vec::new()
    } else {
        components(&resolve(Path::new(&literal_path), Some(root), home))
    };
    out.extend(rest.into_iter().filter(|part| !part.is_empty()));
    out
}

/// Splits `Name(argument)` into its parts; a bare `Name` has no argument.
fn split_pattern(pattern: &str) -> (&str, Option<&str>) {
    let pattern = pattern.trim();
    let Some(open) = pattern.find('(') else {
        return (pattern, None);
    };
    if !pattern.ends_with(')') {
        return (pattern, None);
    }
    (
        &pattern[..open],
        Some(&pattern[open + 1..pattern.len() - 1]),
    )
}

/// Whether `glob` matches `candidate` or names one of its ancestors.
///
/// Naming an ancestor is a match because a pattern that names a directory
/// covers its subtree — the `(subpath …)` term §3's seatbelt shape uses, and
/// the "realpath closure of the glob" §2's table names.
fn covers(glob: &[String], candidate: &[String], fold: bool) -> bool {
    (0..=candidate.len()).any(|end| match_components(glob, &candidate[..end], fold))
}

fn match_components(glob: &[String], candidate: &[String], fold: bool) -> bool {
    let Some((head, tail)) = glob.split_first() else {
        return candidate.is_empty();
    };
    if head == "**" {
        return (0..=candidate.len()).any(|skip| match_components(tail, &candidate[skip..], fold));
    }
    let Some((first, rest)) = candidate.split_first() else {
        return false;
    };
    match_segment(head, first, fold) && match_components(tail, rest, fold)
}

/// Matches one component, where `*` and `?` do not cross a separator.
///
/// `fold` is the case-sensitivity decision, and it is deliberate rather than
/// the host's: **a `deny` pattern matches case-insensitively on every
/// platform and an `allow` pattern matches case-sensitively on every
/// platform.** A case-insensitive filesystem would otherwise let
/// `SECRET.ENV` walk past a `deny` written as `secret.env`, and folding
/// `allow` instead would let the same trick reach a path its author never
/// spelled. Both errors are made in the refusing direction, and the answer is
/// the same on macOS, Linux and Windows rather than three answers.
fn match_segment(pattern: &str, text: &str, fold: bool) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    match_chars(&pattern, &text, fold)
}

fn match_chars(pattern: &[char], text: &[char], fold: bool) -> bool {
    let Some((head, tail)) = pattern.split_first() else {
        return text.is_empty();
    };
    match head {
        '*' => (0..=text.len()).any(|skip| match_chars(tail, &text[skip..], fold)),
        '?' => !text.is_empty() && match_chars(tail, &text[1..], fold),
        expected => match text.split_first() {
            Some((actual, rest)) if same(*expected, *actual, fold) => match_chars(tail, rest, fold),
            _ => false,
        },
    }
}

fn same(expected: char, actual: char, fold: bool) -> bool {
    if expected == actual {
        return true;
    }
    fold && expected.to_lowercase().eq(actual.to_lowercase())
}
