//! Version-bound, exact text replacement for an already existing file.

use crate::sandbox::profile::{Access, Profile};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditResult {
    pub path: PathBuf,
    pub before_sha256: String,
    pub after_sha256: String,
    pub changed_lines: ChangedLines,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangedLines {
    pub start: usize,
    pub before: usize,
    pub after: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditError {
    pub kind: String,
    pub message: String,
}

impl EditError {
    fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
}
impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for EditError {}
impl From<&str> for EditError {
    fn from(message: &str) -> Self {
        Self::new("edit_refused", message)
    }
}
impl From<String> for EditError {
    fn from(message: String) -> Self {
        Self::new("edit_refused", message)
    }
}

/// Replace the one exact occurrence of `expected` when the current file has
/// `expected_sha256`. The replacement is installed by a same-directory rename.
pub fn apply(
    profile: &Profile,
    path: &Path,
    expected_sha256: &str,
    expected: &str,
    replacement: &str,
) -> Result<EditResult, EditError> {
    if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("expected SHA-256 must be 64 hexadecimal characters".into());
    }
    if expected.is_empty() {
        return Err("exact match must be nonempty".into());
    }
    if expected == replacement {
        return Err("replacement would make no change".into());
    }

    let readable = profile
        .check("edit", Access::Read, path)
        .map_err(|error| EditError::new("permission_denied", error.to_string()))?;
    let writable = profile
        .check("edit", Access::Write, path)
        .map_err(|error| EditError::new("permission_denied", error.to_string()))?;
    if readable != writable {
        return Err("read and write checks resolved to different files".into());
    }
    let metadata =
        fs::metadata(&readable).map_err(|error| format!("could not inspect file: {error}"))?;
    if !metadata.is_file() {
        return Err("edit target is not a regular file".into());
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!("edit target exceeds {MAX_FILE_BYTES} bytes").into());
    }
    let before_bytes =
        fs::read(&readable).map_err(|error| format!("could not read file: {error}"))?;
    if before_bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("edit target exceeds {MAX_FILE_BYTES} bytes").into());
    }
    let before = std::str::from_utf8(&before_bytes).map_err(|_| "edit target is not UTF-8")?;
    let before_hash = sha256(&before_bytes);
    if !before_hash.eq_ignore_ascii_case(expected_sha256) {
        return Err(EditError::new(
            "stale_hash",
            "The source version changed; refresh context before editing.",
        ));
    }
    let mut matches = before.match_indices(expected);
    let Some((offset, _)) = matches.next() else {
        return Err(EditError::new("missing_match", "exact match was not found"));
    };
    if matches.next().is_some() {
        return Err(EditError::new(
            "ambiguous_match",
            "exact match is ambiguous",
        ));
    }

    let mut after = String::with_capacity(before.len() - expected.len() + replacement.len());
    after.push_str(&before[..offset]);
    after.push_str(replacement);
    after.push_str(&before[offset + expected.len()..]);
    if after.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("edited file exceeds {MAX_FILE_BYTES} bytes").into());
    }
    let changed_lines = ChangedLines {
        start: before[..offset]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1,
        before: line_count(expected),
        after: line_count(replacement),
    };
    let after_hash = sha256(after.as_bytes());
    let parent = readable
        .parent()
        .ok_or("edit target has no parent directory")?;
    let name = readable.file_name().ok_or("edit target has no file name")?;
    let temp = parent.join(format!(
        ".{}.pane-edit-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let write_result = (|| -> Result<(), EditError> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| format!("could not create temporary edit file: {error}"))?;
        output
            .set_permissions(metadata.permissions())
            .map_err(|error| format!("could not preserve file permissions: {error}"))?;
        output
            .write_all(after.as_bytes())
            .and_then(|()| output.sync_all())
            .map_err(|error| format!("could not write temporary edit file: {error}"))?;
        let latest =
            fs::read(&readable).map_err(|error| format!("could not recheck file: {error}"))?;
        if sha256(&latest) != before_hash {
            return Err("file changed while the edit was being prepared".into());
        }
        install(&temp, &readable)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result?;
    Ok(EditResult {
        path: readable,
        before_sha256: before_hash,
        after_sha256: after_hash,
        changed_lines,
    })
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn line_count(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count() + usize::from(!text.is_empty())
}

#[cfg(not(target_os = "windows"))]
fn install(temp: &Path, target: &Path) -> Result<(), EditError> {
    fs::rename(temp, target)
        .map_err(|error| EditError::new("io", format!("could not install edit: {error}")))
}

#[cfg(target_os = "windows")]
fn install(temp: &Path, target: &Path) -> Result<(), EditError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let mut from: Vec<u16> = temp.as_os_str().encode_wide().collect();
    let mut to: Vec<u16> = target.as_os_str().encode_wide().collect();
    from.push(0);
    to.push(0);
    // SAFETY: both buffers are NUL-terminated and live for the duration of
    // the call. The flags request the replacement semantics Unix rename has.
    let ok = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        return Err(EditError::new(
            "io",
            format!(
                "could not install edit: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    Ok(())
}
