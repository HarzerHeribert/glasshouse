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
    /// Map line 2005's shadow-comparison record: the deterministic ladder's
    /// own forwarded-size estimate, recorded at write time — before the
    /// optional semantic stage ever runs, so it reflects exactly what this
    /// entry's own bytes went through. The semantic stage's own additional
    /// savings are the evidence ledger's reducer-call rows (map line
    /// 1987's second half); this field never doubles that count and never
    /// touches a ledger token column. `None` only for an entry a build
    /// before this package wrote — read back and shown honestly as
    /// "no recorded estimate", never backfilled with a guess.
    #[serde(default)]
    pub forwarded_token_estimate: Option<u64>,
    /// The deterministic ladder's own retained/total candidate counts for
    /// this entry, beside [`Self::forwarded_token_estimate`] — map line
    /// 2005's "record what was dropped and whether it mattered". Same
    /// backward-compatibility rule: `None` for an entry written before this
    /// field existed.
    #[serde(default)]
    pub retained_candidates: Option<usize>,
    #[serde(default)]
    pub total_candidates: Option<usize>,
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

    /// Every entry currently stored, across every session — map line 2006's
    /// aggregate-savings reader walks this rather than adding a query
    /// surface to the store's own files (no migration, no database table:
    /// design-decisions.md's Phase 57 ruling stands). Fail-soft in both
    /// directions: a store that has never been written to answers with an
    /// empty list rather than a "not found" error, and one unreadable
    /// session directory or entry (partial write, corrupt file) is skipped
    /// rather than failing the whole scan — the same posture
    /// [`Self::read`] already takes for one entry, extended to a walk of
    /// all of them.
    pub fn all_entries(&self) -> io::Result<Vec<RawEntry>> {
        let mut out = Vec::new();
        let session_dirs = match std::fs::read_dir(&self.root) {
            Ok(dirs) => dirs,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(out),
            Err(err) => return Err(err),
        };
        for session_dir in session_dirs.flatten() {
            let path = session_dir.path();
            if !path.is_dir() {
                continue;
            }
            let Ok(files) = std::fs::read_dir(&path) else {
                continue;
            };
            for file in files.flatten() {
                let file_path = file.path();
                if file_path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(bytes) = std::fs::read(&file_path)
                    && let Ok(entry) = serde_json::from_slice::<RawEntry>(&bytes)
                {
                    out.push(entry);
                }
            }
        }
        Ok(out)
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
            forwarded_token_estimate: Some(10),
            retained_candidates: Some(1),
            total_candidates: Some(3),
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

    #[test]
    fn all_entries_is_empty_for_a_store_never_written_to() {
        let dir = tempfile::tempdir().unwrap();
        let store = RawStore::open(dir.path().join("never-created"));
        assert_eq!(store.all_entries().unwrap(), Vec::new());
    }

    #[test]
    fn all_entries_walks_every_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = RawStore::open(dir.path());
        store.write(&entry("session-a", "content a")).unwrap();
        store.write(&entry("session-a", "content a2")).unwrap();
        store.write(&entry("session-b", "content b")).unwrap();

        let mut entries = store.all_entries().unwrap();
        assert_eq!(entries.len(), 3);
        entries.sort_by(|a, b| a.content.cmp(&b.content));
        assert_eq!(entries[0].content, "content a");
        assert_eq!(entries[1].content, "content a2");
        assert_eq!(entries[2].content, "content b");
    }

    /// An entry written by a build before map line 2005's fields existed
    /// still reads back — `None`, never a deserialization failure or a
    /// fabricated `Some(0)`.
    #[test]
    fn an_entry_without_the_2005_fields_still_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = RawStore::open(dir.path());
        let legacy = serde_json::json!({
            "session_id": "session-legacy",
            "tool_use_id": "tu-1",
            "tool": "Grep",
            "timestamp_unix": 1_700_000_000i64,
            "content": "old bytes\n",
            "original_token_estimate": 5,
        });
        let session_dir = dir.path().join(session_key("session-legacy"));
        std::fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join(format!("{}.json", content_key("old bytes\n")));
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

        let reference = format!(
            "{REFERENCE_PREFIX}{}-{}",
            session_key("session-legacy"),
            content_key("old bytes\n")
        );
        let read_back = store.read(&reference).unwrap().expect("must still read");
        assert_eq!(read_back.content, "old bytes\n");
        assert_eq!(read_back.forwarded_token_estimate, None);
        assert_eq!(read_back.retained_candidates, None);
        assert_eq!(read_back.total_candidates, None);

        let all = store.all_entries().unwrap();
        assert_eq!(all.len(), 1);
    }
}
