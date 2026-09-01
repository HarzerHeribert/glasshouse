//! The per-session raw store — map line 1984: preserve every reduced
//! result's original bytes locally, addressable by a stable reference the
//! session can later expand (line 1985's `show` reads this back
//! byte-identically).
//!
//! Content-addressed, write-once files under the project's own state
//! directory — never a database migration (design-decisions.md's Phase 57
//! ruling: MVP needs write-once blobs, not a query surface). A `gh-tool://`
//! id embeds a hash of the session id, so two sessions' stores never
//! collide even when run concurrently, and `read` never has to search more
//! than one directory to answer.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The reference scheme's prefix — the spec's own shape.
pub const REFERENCE_PREFIX: &str = "gh-tool://";

/// One preserved raw tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawEntry {
    pub session_id: String,
    pub tool_use_id: String,
    pub tool: String,
    pub timestamp_unix: i64,
    pub content: String,
    pub original_token_estimate: u64,
}

/// A raw store rooted at one directory — normally
/// `Runtime::state_dir().join("context-firewall")`, but a plain path here
/// so tests can point it at a temp directory without a full
/// [`crate::Runtime`].
pub struct RawStore {
    root: PathBuf,
}

impl RawStore {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Write `entry`, returning its stable `gh-tool://<id>` reference.
    ///
    /// Write-once and content-addressed: writing the same content again for
    /// the same session reaches the same id and touches no bytes on disk
    /// the second time.
    pub fn write(&self, entry: &RawEntry) -> io::Result<String> {
        let session_key = session_key(&entry.session_id);
        let content_key = content_key(&entry.content);
        let dir = self.root.join(&session_key);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{content_key}.json"));
        if !path.exists() {
            let json = serde_json::to_vec(entry)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            write_atomically(&dir, &path, &json)?;
        }
        Ok(format!("{REFERENCE_PREFIX}{session_key}-{content_key}"))
    }

    /// Read back an entry by its `gh-tool://<id>` reference (the bare id
    /// works too). `Ok(None)` for an id this store never wrote, or one that
    /// is not shaped like one of ours.
    pub fn read(&self, reference: &str) -> io::Result<Option<RawEntry>> {
        let id = reference
            .strip_prefix(REFERENCE_PREFIX)
            .unwrap_or(reference);
        let Some((session_key, content_key)) = id.split_once('-') else {
            return Ok(None);
        };
        if !is_hex(session_key) || !is_hex(content_key) {
            return Ok(None);
        }
        let path = self
            .root
            .join(session_key)
            .join(format!("{content_key}.json"));
        match std::fs::read(&path) {
            Ok(bytes) => {
                let entry: RawEntry = serde_json::from_slice(&bytes)
                    .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
                Ok(Some(entry))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }
}

/// Write via a temp file in the same directory, then rename into place, so
/// a reader never observes a partially written entry and two writers
/// racing on the same content never corrupt each other's bytes.
fn write_atomically(dir: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut suffix = [0u8; 8];
    getrandom::fill(&mut suffix).map_err(io::Error::other)?;
    let tmp = dir.join(format!(
        "{}.tmp-{}-{}",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("entry.json"),
        std::process::id(),
        hex::encode(suffix)
    ));
    std::fs::write(&tmp, bytes)?;
    // A concurrent writer for the exact same content may have already won
    // the race and created `target`; that is the write-once contract
    // working as intended, not an error.
    match std::fs::rename(&tmp, target) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            if target.exists() { Ok(()) } else { Err(err) }
        }
    }
}

fn session_key(session_id: &str) -> String {
    let digest = Sha256::digest(session_id.as_bytes());
    hex::encode(&digest[..8])
}

fn content_key(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    hex::encode(&digest[..16])
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(session: &str, content: &str) -> RawEntry {
        RawEntry {
            session_id: session.to_string(),
            tool_use_id: "tu-1".to_string(),
            tool: "Grep".to_string(),
            timestamp_unix: 1_700_000_000,
            content: content.to_string(),
            original_token_estimate: 42,
        }
    }

    #[test]
    fn a_write_round_trips_byte_identically() {
        let dir = tempfile::tempdir().unwrap();
        let store = RawStore::open(dir.path());
        let original = entry("session-a", "the exact original bytes\nwith two lines\n");
        let reference = store.write(&original).unwrap();
        assert!(reference.starts_with(REFERENCE_PREFIX));
        let read_back = store.read(&reference).unwrap().expect("entry must exist");
        assert_eq!(read_back.content, original.content);
        assert_eq!(read_back, original);
    }

    #[test]
    fn an_unknown_id_reads_as_none_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = RawStore::open(dir.path());
        let result = store.read("gh-tool://0000000000000000-00000000000000000000000000000000");
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn a_malformed_reference_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = RawStore::open(dir.path());
        assert_eq!(store.read("not-a-reference-at-all").unwrap(), None);
        assert_eq!(store.read("gh-tool://").unwrap(), None);
    }

    #[test]
    fn two_concurrent_sessions_never_collide() {
        let dir = tempfile::tempdir().unwrap();
        let store = RawStore::open(dir.path());
        let a = store.write(&entry("session-a", "same content")).unwrap();
        let b = store.write(&entry("session-b", "same content")).unwrap();
        assert_ne!(a, b, "distinct sessions must never share a reference");
        assert_eq!(store.read(&a).unwrap().unwrap().session_id, "session-a");
        assert_eq!(store.read(&b).unwrap().unwrap().session_id, "session-b");
    }

    #[test]
    fn identical_content_in_one_session_is_content_addressed() {
        let dir = tempfile::tempdir().unwrap();
        let store = RawStore::open(dir.path());
        let first = store.write(&entry("session-a", "dup")).unwrap();
        let second = store.write(&entry("session-a", "dup")).unwrap();
        assert_eq!(first, second);
    }
}
