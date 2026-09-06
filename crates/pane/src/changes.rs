//! Bounded, local observation of project files before and after a Pane cell.
//!
//! Nothing produced here belongs in the model conversation. A snapshot is
//! deliberately best-effort: unreadable or over-limit coverage is named, and
//! an incomplete later scan never turns an unseen path into a deletion.

use crate::sandbox::profile::{Access, Profile};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_FILES: usize = 20_000;
const MAX_DEPTH: usize = 64;
const MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

const EXCLUDED: &[&str] = &[
    ".git",
    ".glasshouse",
    ".pane",
    "node_modules",
    "target",
    ".cache",
    ".next",
    ".venv",
    "__pycache__",
];

#[derive(Clone, Debug)]
struct FileState {
    len: u64,
    digest: Option<[u8; 32]>,
    bytes: Option<Vec<u8>>,
}

/// A point-in-time, bounded view of readable regular files below the project root.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    files: BTreeMap<PathBuf, FileState>,
    complete: bool,
    notes: Vec<String>,
}

impl Snapshot {
    /// Captures readable project files without following symlinks.
    pub fn capture(profile: &Profile) -> Self {
        Self::capture_with_limits(profile, MAX_FILES, MAX_TOTAL_BYTES, MAX_FILE_BYTES)
    }

    fn capture_with_limits(
        profile: &Profile,
        max_files: usize,
        max_total_bytes: u64,
        max_file_bytes: u64,
    ) -> Self {
        let mut snapshot = Self {
            complete: true,
            ..Self::default()
        };
        let mut remaining = max_total_bytes;
        let mut visited = 0usize;
        snapshot.visit(
            profile,
            profile.root(),
            0,
            (max_files, max_file_bytes),
            &mut remaining,
            &mut visited,
        );
        snapshot
    }

    fn visit(
        &mut self,
        profile: &Profile,
        directory: &Path,
        depth: usize,
        (max_files, max_file_bytes): (usize, u64),
        remaining: &mut u64,
        visited: &mut usize,
    ) {
        if depth > MAX_DEPTH {
            self.incomplete("directory-depth limit reached");
            return;
        }
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(_) => {
                self.incomplete("a directory could not be read");
                return;
            }
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if *visited >= max_files {
                self.incomplete("path-count limit reached");
                break;
            }
            *visited += 1;
            let relative = match path.strip_prefix(profile.root()) {
                Ok(path) => path.to_path_buf(),
                Err(_) => continue,
            };
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => {
                    self.incomplete("a path changed while it was scanned");
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                if relative
                    .components()
                    .next_back()
                    .and_then(|part| part.as_os_str().to_str())
                    .is_some_and(|name| EXCLUDED.contains(&name))
                {
                    continue;
                }
                // Walking names is local bookkeeping; only opening a file is
                // gated. Otherwise an allowed `Read(src/**)` could be hidden
                // behind a root directory that was not itself granted.
                self.visit(
                    profile,
                    &path,
                    depth + 1,
                    (max_files, max_file_bytes),
                    remaining,
                    visited,
                );
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let Ok(resolved) = profile.check("read", Access::Read, &path) else {
                continue;
            };
            let Ok(resolved_metadata) = fs::metadata(&resolved) else {
                self.incomplete("a path changed while it was scanned");
                continue;
            };
            if !resolved_metadata.is_file() {
                continue;
            }
            let len = resolved_metadata.len();
            if len > max_file_bytes || len > *remaining {
                self.files.insert(
                    relative,
                    FileState {
                        len,
                        digest: None,
                        bytes: None,
                    },
                );
                self.incomplete("one or more files exceeded the byte limit");
                continue;
            }
            let limit = len.min(max_file_bytes).min(*remaining);
            let read = fs::File::open(&resolved).and_then(|file| {
                let mut bytes = Vec::with_capacity(limit as usize);
                file.take(limit + 1).read_to_end(&mut bytes)?;
                Ok(bytes)
            });
            match read {
                Ok(bytes) if bytes.len() as u64 <= limit => {
                    if fs::metadata(&resolved)
                        .ok()
                        .map(|now| (now.len(), now.modified().ok()))
                        != Some((len, resolved_metadata.modified().ok()))
                    {
                        self.incomplete("a file changed while it was scanned");
                        continue;
                    }
                    *remaining = remaining.saturating_sub(bytes.len() as u64);
                    let digest: [u8; 32] = Sha256::digest(&bytes).into();
                    self.files.insert(
                        relative,
                        FileState {
                            len,
                            digest: Some(digest),
                            bytes: Some(bytes),
                        },
                    );
                }
                Ok(_) => self.incomplete("one or more files exceeded the byte limit"),
                Err(_) => self.incomplete("a file could not be read"),
            }
        }
    }

    fn incomplete(&mut self, note: &str) {
        self.complete = false;
        if !self.notes.iter().any(|held| held == note) {
            self.notes.push(note.to_string());
        }
    }

    /// Renders observed changes. `None` means no change was observed within the capture limits.
    pub fn diff(&self, after: &Self) -> Option<String> {
        let mut out = String::new();
        for (path, current) in &after.files {
            match self.files.get(path) {
                None if self.complete => render_addition(&mut out, path, current),
                Some(previous) if changed(previous, current) => {
                    render_change(&mut out, path, previous, current)
                }
                _ => {}
            }
        }
        if after.complete {
            for (path, previous) in &self.files {
                if !after.files.contains_key(path) {
                    render_deletion(&mut out, path, previous);
                }
            }
        }
        let mut notes = self.notes.clone();
        for note in &after.notes {
            if !notes.contains(note) {
                notes.push(note.clone());
            }
        }
        if !out.is_empty() && !notes.is_empty() {
            out.push_str("\n[change capture incomplete: ");
            out.push_str(&notes.join("; "));
            out.push_str("]\n");
        }
        if out.is_empty() {
            return None;
        }
        if out.len() > MAX_OUTPUT_BYTES {
            let mut end = MAX_OUTPUT_BYTES;
            while !out.is_char_boundary(end) {
                end -= 1;
            }
            out.truncate(end);
            out.push_str("\n[change output truncated]\n");
        }
        Some(out)
    }
}

fn changed(before: &FileState, after: &FileState) -> bool {
    before.len != after.len || matches!((before.digest, after.digest), (Some(a), Some(b)) if a != b)
}

fn label(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn text(state: &FileState) -> Option<&str> {
    let bytes = state.bytes.as_deref()?;
    if bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

fn render_addition(out: &mut String, path: &Path, state: &FileState) {
    let name = label(path);
    match text(state) {
        Some(body) => render_hunk(out, "/dev/null", &format!("b/{name}"), "", body),
        None if state.bytes.is_none() => out.push_str(&format!(
            "Content unavailable for added file: {name} ({} bytes)\n",
            state.len
        )),
        None => out.push_str(&format!(
            "Binary file added: {name} ({} bytes)\n",
            state.len
        )),
    }
}

fn render_deletion(out: &mut String, path: &Path, state: &FileState) {
    let name = label(path);
    match text(state) {
        Some(body) => render_hunk(out, &format!("a/{name}"), "/dev/null", body, ""),
        None if state.bytes.is_none() => out.push_str(&format!(
            "Content unavailable for deleted file: {name} ({} bytes)\n",
            state.len
        )),
        None => out.push_str(&format!(
            "Binary file deleted: {name} ({} bytes)\n",
            state.len
        )),
    }
}

fn render_change(out: &mut String, path: &Path, before: &FileState, after: &FileState) {
    let name = label(path);
    match (text(before), text(after)) {
        (Some(old), Some(new)) => {
            render_hunk(out, &format!("a/{name}"), &format!("b/{name}"), old, new)
        }
        _ if before.bytes.is_none() || after.bytes.is_none() => out.push_str(&format!(
            "Content unavailable for changed file: {name} ({} -> {} bytes)\n",
            before.len, after.len
        )),
        _ => out.push_str(&format!(
            "Binary file changed: {name} ({} -> {} bytes)\n",
            before.len, after.len
        )),
    }
}

fn render_hunk(out: &mut String, old_name: &str, new_name: &str, old: &str, new: &str) {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    if old_lines == new_lines && old != new {
        out.push_str(&format!("--- {old_name}\n+++ {new_name}\n"));
        out.push_str("@@ end-of-file newline @@\n");
        out.push_str(if old.ends_with('\n') == new.ends_with('\n') {
            "Line endings changed (CRLF/LF)\n"
        } else if old.ends_with('\n') {
            "-newline present\n+no newline\n"
        } else {
            "-no newline\n+newline present\n"
        });
        return;
    }
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old_end = old_lines.len().saturating_sub(suffix);
    let new_end = new_lines.len().saturating_sub(suffix);
    out.push_str(&format!("--- {old_name}\n+++ {new_name}\n"));
    out.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        prefix + 1,
        old_end.saturating_sub(prefix),
        prefix + 1,
        new_end.saturating_sub(prefix)
    ));
    for line in &old_lines[prefix..old_end] {
        out.push('-');
        out.push_str(line);
        out.push('\n');
    }
    for line in &new_lines[prefix..new_end] {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture(name: &str) -> (PathBuf, Profile) {
        let root = std::env::temp_dir().join(format!(
            "pane-changes-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let profile =
            Profile::compile(&root, Some(r#"{"permissions":{"allow":["Read","Write"]}}"#));
        (root, profile)
    }

    #[test]
    fn reports_only_changes_after_the_baseline() {
        let (root, profile) = fixture("baseline");
        fs::write(root.join("already-dirty.txt"), "held\n").unwrap();
        fs::write(root.join("changed.txt"), "before\n").unwrap();
        fs::write(root.join("deleted.txt"), "gone\n").unwrap();
        let before = Snapshot::capture(&profile);
        fs::write(root.join("changed.txt"), "after\n").unwrap();
        fs::write(root.join("added.txt"), "new\n").unwrap();
        fs::remove_file(root.join("deleted.txt")).unwrap();
        let rendered = before.diff(&Snapshot::capture(&profile)).unwrap();
        assert!(rendered.contains("+++ b/added.txt"), "{rendered}");
        assert!(
            rendered.contains("-before") && rendered.contains("+after"),
            "{rendered}"
        );
        assert!(rendered.contains("--- a/deleted.txt"), "{rendered}");
        assert!(!rendered.contains("already-dirty"), "{rendered}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exclusions_and_symlinks_are_not_captured() {
        let (root, profile) = fixture("excluded");
        for directory in [".git", ".glasshouse", ".pane", "node_modules", "target"] {
            fs::create_dir_all(root.join(directory)).unwrap();
            fs::write(root.join(directory).join("noise"), "before").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", root.join("escape")).unwrap();
        let before = Snapshot::capture(&profile);
        for directory in [".git", ".glasshouse", ".pane", "node_modules", "target"] {
            fs::write(root.join(directory).join("noise"), "after").unwrap();
        }
        assert!(before.diff(&Snapshot::capture(&profile)).is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn limits_are_explicit_and_do_not_invent_deletions() {
        let (root, profile) = fixture("limits");
        fs::write(root.join("a.txt"), "a").unwrap();
        fs::write(root.join("b.txt"), "b").unwrap();
        let before = Snapshot::capture_with_limits(&profile, 10, 10, 10);
        fs::remove_file(root.join("a.txt")).unwrap();
        fs::write(root.join("0-added.txt"), "new").unwrap();
        let after = Snapshot::capture_with_limits(&profile, 1, 10, 10);
        let rendered = before.diff(&after).unwrap();
        assert!(rendered.contains("capture incomplete"), "{rendered}");
        assert!(rendered.contains("0-added.txt"), "{rendered}");
        assert!(!rendered.contains("deleted"), "{rendered}");
        assert!(!rendered.contains("--- a/a.txt"), "{rendered}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_incomplete_baseline_does_not_invent_additions() {
        let (root, profile) = fixture("incomplete-before");
        fs::write(root.join("a.txt"), "a").unwrap();
        fs::write(root.join("b.txt"), "b").unwrap();
        let before = Snapshot::capture_with_limits(&profile, 1, 10, 10);
        assert!(before.diff(&Snapshot::capture(&profile)).is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_changes_are_summarized() {
        let (root, profile) = fixture("binary");
        fs::write(root.join("blob.bin"), [0, 1, 2]).unwrap();
        let before = Snapshot::capture(&profile);
        fs::write(root.join("blob.bin"), [0, 1, 3]).unwrap();
        let rendered = before.diff(&Snapshot::capture(&profile)).unwrap();
        assert!(
            rendered.contains("Binary file changed: blob.bin"),
            "{rendered}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_end_of_file_newline_change_is_visible() {
        let (root, profile) = fixture("newline");
        fs::write(root.join("line.txt"), "same").unwrap();
        let before = Snapshot::capture(&profile);
        fs::write(root.join("line.txt"), "same\n").unwrap();
        let rendered = before.diff(&Snapshot::capture(&profile)).unwrap();
        assert!(rendered.contains("end-of-file newline"), "{rendered}");
        fs::remove_dir_all(root).unwrap();
    }
}
