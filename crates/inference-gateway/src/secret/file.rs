//! The gateway's own credential file: `<data dir>/credentials.toml`.
//!
//! The invariant: **a credential stored through the gateway is readable by
//! every later build of the gateway.** The native store cannot promise that
//! for an ad-hoc-signed binary. Measured 2026-09-11: a Keychain item filed
//! by one build carries a `partition_id` ACL naming that build's code hash,
//! and the next installed build is refused — silently, because
//! `native::silence_authorization_dialogs` turns the *Allow* dialog into an
//! error. A file the gateway owns, mode `0600`, has no such binding, and it
//! is the protection class a shell profile already gives the same key.
//! History: design-decisions.md, "The inference gateway is its own crate and
//! process", the 2026-09-11 addendum.
//!
//! Flat TOML, variable name to value, so a [`SecretRef::Environment`]
//! resolves against it by the same name it would use anywhere else. Read on
//! every resolution and never cached: a store answers for *now*, and the
//! file is a few lines long.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::{Secret, SecretRef, SecretStore};

/// [`SecretStore::describe`] for this store on its own.
pub const LABEL: &str = "the gateway's credential file";

/// The file, by path. Holds no value: every read opens the file.
#[derive(Debug, Clone)]
pub struct FileSecretStore {
    path: PathBuf,
}

/// Why a write did not happen. Every variant names the path and never a
/// value; the one carrying toml's message carries `message()`, not the
/// source excerpt that would echo a line of the file back.
#[derive(Debug, thiserror::Error)]
pub enum FileStoreError {
    #[error(
        "{path} does not parse as a credential file ({reason}); fix or remove it before storing \
         into it"
    )]
    Unparsable { path: PathBuf, reason: String },
    #[error("could not write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl FileSecretStore {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The variable names filed here. Names only.
    pub fn variables(&self) -> Vec<String> {
        self.read()
            .map(|entries| entries.into_keys().collect())
            .unwrap_or_default()
    }

    /// Put `value` under `var`, replacing whatever was there.
    ///
    /// Written whole to a sibling temporary file created `0600` and renamed
    /// into place, so a reader never sees a half-written file and the mode
    /// is right before the first byte of a value lands.
    pub fn store(&self, var: &str, value: &str) -> Result<(), FileStoreError> {
        let mut entries = self.read()?;
        entries.insert(var.to_owned(), value.to_owned());
        self.write(&entries)
    }

    /// Remove `var`. `Ok(false)` is absent, not an error.
    pub fn remove(&self, var: &str) -> Result<bool, FileStoreError> {
        let mut entries = self.read()?;
        if entries.remove(var).is_none() {
            return Ok(false);
        }
        self.write(&entries)?;
        Ok(true)
    }

    /// Every entry. A missing file is empty; a file that will not parse is
    /// the one error a read has, and it is what stops [`Self::store`] from
    /// overwriting a file the user wrote by hand.
    fn read(&self) -> Result<BTreeMap<String, String>, FileStoreError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(source) => {
                return Err(FileStoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|error: toml::de::Error| FileStoreError::Unparsable {
            path: self.path.clone(),
            reason: error.message().to_owned(),
        })
    }

    fn write(&self, entries: &BTreeMap<String, String>) -> Result<(), FileStoreError> {
        let io = |source| FileStoreError::Io {
            path: self.path.clone(),
            source,
        };
        let text = toml::to_string(entries).map_err(|error| FileStoreError::Unparsable {
            path: self.path.clone(),
            reason: error.to_string(),
        })?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let temporary = self.path.with_extension("toml.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            // `mode` above applies only to a file this call creates; a
            // leftover temporary keeps whatever it had.
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(io)?;
        }
        file.write_all(text.as_bytes()).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        std::fs::rename(&temporary, &self.path).map_err(io)
    }
}

impl SecretStore for FileSecretStore {
    fn resolve(&self, reference: &SecretRef) -> Option<Secret> {
        let SecretRef::Environment { var } = reference else {
            return None;
        };
        self.read().ok()?.remove(var).map(Secret)
    }

    fn is_present(&self, reference: &SecretRef) -> bool {
        let SecretRef::Environment { var } = reference else {
            return false;
        };
        self.read()
            .ok()
            .is_some_and(|entries| entries.contains_key(var))
    }

    fn describe(&self) -> &'static str {
        LABEL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, FileSecretStore) {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let store = FileSecretStore::at(dir.path().join("nested").join("credentials.toml"));
        (dir, store)
    }

    fn reference(var: &str) -> SecretRef {
        SecretRef::Environment {
            var: var.to_owned(),
        }
    }

    /// The whole contract: a value stored under a variable name resolves by
    /// that name, only that name, and removal makes it absent again.
    #[test]
    fn a_stored_value_resolves_by_its_variable_name_until_removed() {
        let (_dir, store) = store();
        assert!(!store.is_present(&reference("A_KEY")));
        assert_eq!(store.variables(), Vec::<String>::new());

        store
            .store("A_KEY", "value-a")
            .expect("the first write creates the file");
        store
            .store("B_KEY", "value-b")
            .expect("a second write keeps the first");
        assert_eq!(
            store.resolve(&reference("A_KEY")).unwrap().expose(),
            "value-a"
        );
        assert_eq!(
            store.resolve(&reference("B_KEY")).unwrap().expose(),
            "value-b"
        );
        assert!(store.resolve(&reference("C_KEY")).is_none());
        assert!(
            store
                .resolve(&SecretRef::OsCredential {
                    service: "x".into(),
                    account: "A_KEY".into()
                })
                .is_none(),
            "only an environment-style reference is answered from the file"
        );
        assert_eq!(
            store.variables(),
            vec!["A_KEY".to_owned(), "B_KEY".to_owned()]
        );

        assert!(store.remove("A_KEY").expect("removal writes"));
        assert!(!store.remove("A_KEY").expect("a second removal is absent"));
        assert!(store.resolve(&reference("A_KEY")).is_none());
        assert_eq!(
            store.resolve(&reference("B_KEY")).unwrap().expose(),
            "value-b"
        );
    }

    /// The file is created readable by its owner alone, and the temporary it
    /// was written through does not survive the rename.
    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only_and_leaves_no_temporary() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_dir, store) = store();
        store.store("A_KEY", "value-a").expect("written");
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "mode was {mode:o}");
        assert!(!store.path().with_extension("toml.tmp").exists());
    }

    /// A file the gateway cannot read is never overwritten: the refusal
    /// names the path and toml's message, not the offending line.
    #[test]
    fn an_unparsable_file_refuses_a_write_without_echoing_it() {
        let (_dir, store) = store();
        std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        std::fs::write(store.path(), "SECRET_LINE = \"not-closed\n").unwrap();
        let error = store.store("A_KEY", "value-a").expect_err("refused");
        let text = error.to_string();
        assert!(text.contains("does not parse"), "{text}");
        assert!(
            !text.contains("not-closed"),
            "the file's content is not echoed: {text}"
        );
        assert!(store.resolve(&reference("A_KEY")).is_none());
        assert_eq!(
            std::fs::read_to_string(store.path()).unwrap(),
            "SECRET_LINE = \"not-closed\n",
            "the user's file is left as it was"
        );
    }
}
