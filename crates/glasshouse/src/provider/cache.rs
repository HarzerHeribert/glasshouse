//! The discovered-model catalogue and where it is kept between runs.
//!
//! # Why this is in the data directory and not the configuration file
//!
//! A discovered catalogue is not configuration. The user did not type it, it
//! has a provenance and an age, and Glasshouse rewrites it on its own when
//! asked to refresh. A `cargo`-style configuration file is a record of
//! decisions a person made; putting four hundred model identifiers and a
//! machine-written timestamp in one would make `config.toml` unreadable and
//! would make a `git diff` of it meaningless. [`crate::paths`] already
//! separates the two locations, so this uses the one it already has:
//! [`crate::paths::RuntimePaths::provider_cache_dir`].
//!
//! # Why the timestamp is a field and not an inference
//!
//! Phase 9D line 3 says "with a timestamp", and this is where that is
//! honoured. It is stored, it is loaded, and every rendering of a cached
//! catalogue shows it — see `shell::view`. A cache whose age cannot be seen
//! is the failure mode the line exists to prevent: a model list from three
//! weeks ago looks exactly like one from three seconds ago, and only one of
//! them should be acted on.
//!
//! # Nothing here fetches
//!
//! This module reads and writes files. It has no HTTP client, no timer and
//! no expiry policy, and [`ModelCache::load`] never falls back to the
//! network. That is deliberate and is the whole of line 3: starting
//! Glasshouse with a cached catalogue must issue no request at all, and the
//! surest way to guarantee that is for the loading path to be incapable of
//! making one. Refreshing is [`mod@crate::provider::discovery`]'s job and
//! happens only when a key is pressed.
//!
//! # No credential is ever written here
//!
//! A catalogue holds a provider name, two URLs, a timestamp and a list of
//! model identifiers. There is no field a credential could occupy, and the
//! test `a_planted_credential_never_reaches_the_cache_file_on_disk` asserts
//! that against the bytes actually written rather than against the type.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::paths::RuntimePaths;

/// The on-disk format's version.
///
/// Bumped when the shape changes incompatibly. See [`ModelCache::load`] for
/// what happens to a file this build does not understand — the answer is
/// "nothing, it is simply not a cache hit", which is the only migration story
/// a cache needs.
const FORMAT_VERSION: u32 = 1;

/// One model a provider said it serves.
///
/// A struct with one field rather than a bare `String`, because a catalogue
/// entry is a thing that can grow a field and a `Vec<String>` is a thing that
/// cannot.
///
/// **Only the identifier is stored**, and that is a finding rather than a
/// simplification. Five live catalogues were read on 2026-08-26 — OpenRouter
/// (417 entries), UnoRouter (374), Nous (372), Kilo (367) and AnyRouter
/// (102). Every entry in all five carried a string `id`; nothing else was
/// universal, and UnoRouter's entries carry no `name` at all where the other
/// four do. Storing a field that is absent for a provider already shipping a
/// template here would be recording a guess, which is the same failure
/// [`mod@crate::provider`] refuses for a base URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    id: String,
}

impl ModelEntry {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

/// A provider's model list, as read at a moment in time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCatalogue {
    /// Present in the file so a catalogue read from disk can be checked
    /// against the build reading it, and so the file says what it is when a
    /// human opens it.
    version: u32,
    provider: String,
    /// The base URL the catalogue was fetched for.
    ///
    /// Stored so a cache written before the user edited a provider's base URL
    /// can be recognised as being about somewhere else — see
    /// [`ModelCatalogue::was_fetched_from`].
    base_url: String,
    /// The exact URL that was requested. A user asking "where did these come
    /// from" gets an answer they can paste into a browser.
    endpoint: String,
    /// Seconds since the Unix epoch, in the same units and sign as
    /// `session::store`'s own clock.
    fetched_at: i64,
    models: Vec<ModelEntry>,
}

impl ModelCatalogue {
    pub fn new(
        provider: impl Into<String>,
        base_url: impl Into<String>,
        endpoint: impl Into<String>,
        fetched_at: i64,
        models: Vec<ModelEntry>,
    ) -> Self {
        Self {
            version: FORMAT_VERSION,
            provider: provider.into(),
            base_url: base_url.into(),
            endpoint: endpoint.into(),
            fetched_at,
            models,
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// When this was fetched, in seconds since the Unix epoch.
    pub fn fetched_at(&self) -> i64 {
        self.fetched_at
    }

    pub fn models(&self) -> &[ModelEntry] {
        &self.models
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Whether this catalogue came from `base_url`.
    ///
    /// A user who edits a provider's base URL has pointed it at a different
    /// service, and the models cached from the old one are not that service's
    /// models. This is what lets the renderer say so instead of presenting
    /// them as if they still applied.
    pub fn was_fetched_from(&self, base_url: &str) -> bool {
        self.base_url == base_url
    }
}

/// Where a discovered catalogue is kept, and the only thing that reads or
/// writes one.
#[derive(Debug, Clone)]
pub struct ModelCache {
    root: PathBuf,
}

/// Why a catalogue could not be written.
///
/// Only writing has an error type. Reading has none on purpose — see
/// [`ModelCache::load`].
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("could not create the provider cache directory {path}: {source}")]
    Directory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write the provider cache file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not encode the model catalogue for `{provider}`: {source}")]
    Encode {
        provider: String,
        #[source]
        source: serde_json::Error,
    },
}

impl ModelCache {
    /// The cache under this installation's data directory.
    pub fn new(paths: &RuntimePaths) -> Self {
        Self {
            root: paths.provider_cache_dir(),
        }
    }

    /// A cache rooted at an explicit directory. For tests and portable
    /// installations, exactly like [`RuntimePaths::new`].
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file a given provider's catalogue lives in.
    ///
    /// **The name is derived, never used verbatim.** A provider name is
    /// whatever the user typed into the Settings overlay — nothing validates
    /// it as a path component, and nothing should have to — so a name of
    /// `../../.ssh/authorized_keys` must not be able to steer a write out of
    /// this directory. The private `file_stem` guarantees the result matches
    /// `[a-z0-9-]+`, which cannot contain a separator, cannot be `.` or `..`,
    /// and stays unique across names that slugify the same way.
    pub fn path_for(&self, provider: &str) -> PathBuf {
        provider_json_path(&self.root, provider)
    }

    /// The cached catalogue for `provider`, or `None`.
    ///
    /// **Returns no error, ever, and makes no request, ever.** Every reason a
    /// read can fail — the file is absent, unreadable, truncated, not JSON,
    /// written by a future version, or about a different provider than its
    /// name suggests — means exactly one thing to a caller: there is no cache
    /// hit, carry on. A cache that could fail a start would be worse than no
    /// cache, and a cache that fell back to the network on a miss would
    /// silently reintroduce the per-start request that Phase 9D line 3 exists
    /// to remove.
    pub fn load(&self, provider: &str) -> Option<ModelCatalogue> {
        let path = self.path_for(provider);
        let bytes = std::fs::read(&path).ok()?;
        let catalogue: ModelCatalogue = serde_json::from_slice(&bytes).ok()?;
        if catalogue.version != FORMAT_VERSION {
            tracing::debug!(
                path = %path.display(),
                found = catalogue.version,
                expected = FORMAT_VERSION,
                "ignoring a provider model cache written in another format version"
            );
            return None;
        }
        // The file name is a digest of the provider name, so this can only
        // disagree if a file was hand-edited or moved. Refusing it is one
        // line and keeps a wrong answer from being confidently rendered with
        // a timestamp beside it.
        if catalogue.provider != provider {
            return None;
        }
        Some(catalogue)
    }

    /// Write `catalogue`, replacing whatever was there.
    ///
    /// Written to a temporary file in the same directory and renamed into
    /// place, so a crash or a full disk mid-write leaves the previous
    /// catalogue intact rather than a half-written one that
    /// [`ModelCache::load`] would then quietly discard.
    pub fn store(&self, catalogue: &ModelCatalogue) -> Result<PathBuf, CacheError> {
        std::fs::create_dir_all(&self.root).map_err(|source| CacheError::Directory {
            path: self.root.clone(),
            source,
        })?;

        let encoded =
            serde_json::to_vec_pretty(catalogue).map_err(|source| CacheError::Encode {
                provider: catalogue.provider.clone(),
                source,
            })?;

        let path = self.path_for(&catalogue.provider);
        write_json_atomically(&path, &encoded).map_err(|source| CacheError::Write {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }

    /// Remove a provider's cached catalogue, if it has one.
    ///
    /// Not an error when there is nothing to remove: this is called when a
    /// provider is deleted, and a provider that never had a catalogue is the
    /// ordinary case rather than a problem.
    pub fn forget(&self, provider: &str) -> Result<(), CacheError> {
        let path = self.path_for(provider);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(CacheError::Write { path, source }),
        }
    }
}

/// A provider-keyed JSON path under `root` — `[a-z0-9-]+.json`, guaranteed to
/// be one path component under `root` by [`file_stem`]'s own guarantee.
///
/// `pub(crate)`, for the identical reason `file_stem` is: `telemetry`'s
/// `GatewayQuotaCache` and `GatewayHealthCache` key a per-provider directory
/// the same way, and a second copy of the path-join is a second place for the
/// escape guarantee to go stale.
pub(crate) fn provider_json_path(root: &Path, provider: &str) -> PathBuf {
    root.join(format!("{}.json", file_stem(provider)))
}

/// Write `encoded` to `path` via a same-directory `.json.writing` temporary
/// file, then rename into place, so a crash or a full disk mid-write leaves
/// whatever was at `path` before intact rather than a half-written file a
/// later read would have to quietly discard. Does **not** create `path`'s
/// parent directory — every call site already does that itself, with its own
/// error type, before calling this.
///
/// The one atomic-write primitive [`ModelCache::store`],
/// `telemetry::GatewayQuotaCache::try_store`,
/// `telemetry::GatewayHealthCache::try_store` and
/// `telemetry::RoutingStickyCache::try_store` used to reimplement separately.
pub(crate) fn write_json_atomically(path: &Path, encoded: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("json.writing");
    std::fs::write(&temporary, encoded)?;
    std::fs::rename(&temporary, path)
}

/// A file-name stem for `provider` that is guaranteed to be one path
/// component.
///
/// Every character that is not an ASCII letter or digit becomes `-`, the
/// result is capped, and sixteen hexadecimal characters of a SHA-256 digest
/// of the *original* name are appended. The slug is for a human reading
/// `ls`; the digest is what makes it correct, because it keeps `my provider`
/// and `my/provider` — which slugify identically — in separate files.
///
/// `pub(crate)` rather than private: [`crate::provider::telemetry`]'s
/// `GatewayQuotaCache` keys a second per-provider directory the same way,
/// and a second slugify-and-hash implementation is a second place for the
/// path-escape guarantee below to go stale.
pub(crate) fn file_stem(provider: &str) -> String {
    let mut slug: String = provider
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    slug.truncate(32);
    let digest = Sha256::digest(provider.as_bytes());
    format!("{slug}-{}", hex::encode(&digest[..8]))
}

/// Seconds since the Unix epoch, for stamping a catalogue as it is written.
///
/// Returns `0` rather than panicking if the system clock is before the epoch.
/// A machine with a clock that wrong will render a nonsensical age, which is
/// visible and recoverable; a panic while saving a model list is neither.
pub fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogue(provider: &str, ids: &[&str]) -> ModelCatalogue {
        ModelCatalogue::new(
            provider,
            "https://a.example/v1",
            "https://a.example/v1/models",
            1_787_336_476,
            ids.iter().map(|id| ModelEntry::new(*id)).collect(),
        )
    }

    // --- the round trip ---------------------------------------------------

    #[test]
    fn a_stored_catalogue_comes_back_with_its_models_and_its_timestamp() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = ModelCache::at(dir.path());
        let written = catalogue("openrouter", &["a/one", "b/two"]);
        cache.store(&written).expect("the catalogue is written");

        let read = cache.load("openrouter").expect("the catalogue is cached");
        assert_eq!(read.models(), written.models());
        assert_eq!(read.fetched_at(), 1_787_336_476);
        assert_eq!(read.base_url(), "https://a.example/v1");
        assert_eq!(read.endpoint(), "https://a.example/v1/models");
    }

    #[test]
    fn a_provider_with_no_cache_file_is_a_miss_rather_than_an_error() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        assert!(ModelCache::at(dir.path()).load("never-fetched").is_none());
    }

    #[test]
    fn storing_again_replaces_the_previous_catalogue_and_its_timestamp() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = ModelCache::at(dir.path());
        cache.store(&catalogue("p", &["old/one"])).expect("first");

        let refreshed = ModelCatalogue::new(
            "p",
            "https://a.example/v1",
            "https://a.example/v1/models",
            1_787_400_000,
            vec![ModelEntry::new("new/one"), ModelEntry::new("new/two")],
        );
        cache.store(&refreshed).expect("second");

        let read = cache.load("p").expect("cached");
        assert_eq!(
            read.len(),
            2,
            "the new list replaces the old, never appends"
        );
        assert_eq!(read.models()[0].id(), "new/one");
        assert_eq!(
            read.fetched_at(),
            1_787_400_000,
            "a refresh must move the timestamp forward, or a stale list looks fresh"
        );
    }

    #[test]
    fn only_one_file_is_written_per_provider_however_often_it_is_refreshed() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = ModelCache::at(dir.path());
        for _ in 0..3 {
            cache.store(&catalogue("p", &["a/one"])).expect("stored");
        }
        let files: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readable")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(files.len(), 1, "found {files:?}");
    }

    #[test]
    fn forgetting_a_provider_that_was_never_cached_is_not_an_error() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = ModelCache::at(dir.path());
        cache.forget("never-fetched").expect("not an error");
        cache.store(&catalogue("p", &["a/one"])).expect("stored");
        cache.forget("p").expect("removed");
        assert!(cache.load("p").is_none());
    }

    // --- the two-orders-of-magnitude range --------------------------------

    /// Nine models against four hundred and seventeen — z.ai's real
    /// catalogue size against OpenRouter's, both read on 2026-08-26. The
    /// cache must survive both ends without truncating either.
    #[test]
    fn both_ends_of_the_real_catalogue_size_range_round_trip_whole() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = ModelCache::at(dir.path());
        for count in [9usize, 417] {
            let ids: Vec<String> = (0..count).map(|i| format!("vendor/model-{i}")).collect();
            let entries: Vec<&str> = ids.iter().map(String::as_str).collect();
            let name = format!("p{count}");
            cache.store(&catalogue(&name, &entries)).expect("stored");
            let read = cache.load(&name).expect("cached");
            assert_eq!(read.len(), count);
            assert_eq!(
                read.models()[count - 1].id(),
                format!("vendor/model-{}", count - 1)
            );
        }
    }

    // --- the file name is not the provider name ---------------------------

    /// A provider name is whatever the user typed. This is the assertion
    /// that a hostile one cannot steer a write out of the cache directory.
    #[test]
    fn a_provider_name_that_looks_like_a_path_cannot_escape_the_cache_directory() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = ModelCache::at(dir.path());
        for hostile in [
            "../../.ssh/authorized_keys",
            "..",
            ".",
            "/etc/passwd",
            "a\\b",
            "with space",
            "\u{5e0c}\u{671b}",
        ] {
            let path = cache.path_for(hostile);
            assert_eq!(
                path.parent(),
                Some(dir.path()),
                "`{hostile}` produced {path:?}, which is not inside the cache directory"
            );
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a file name");
            assert!(
                name.strip_suffix(".json").is_some_and(|stem| stem
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')),
                "`{hostile}` produced the file name {name:?}"
            );
        }
    }

    #[test]
    fn two_provider_names_that_slugify_the_same_get_different_files() {
        let cache = ModelCache::at("/tmp/does-not-need-to-exist");
        assert_ne!(
            cache.path_for("my provider"),
            cache.path_for("my/provider"),
            "a digest of the original name is what keeps these apart"
        );
    }

    #[test]
    fn a_cache_file_is_not_read_back_for_a_different_provider() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = ModelCache::at(dir.path());
        cache
            .store(&catalogue("alpha", &["a/one"]))
            .expect("stored");
        assert!(cache.load("beta").is_none());
    }

    // --- the migration story ----------------------------------------------

    /// A file this build does not understand is not a cache hit and is not a
    /// crash. That is the whole migration story a cache needs, and it is
    /// asserted rather than assumed because the alternative — a start that
    /// fails because a file from a newer build is on disk — is the kind of
    /// thing that only shows up in someone else's downgrade.
    #[test]
    fn a_cache_file_from_another_format_version_is_ignored_rather_than_failing() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = ModelCache::at(dir.path());
        let path = cache.path_for("future");
        std::fs::write(
            &path,
            br#"{"version":99,"provider":"future","base_url":"https://a.example/v1",
                 "endpoint":"https://a.example/v1/models","fetched_at":1,
                 "models":[{"id":"a/one"}],"something_new":true}"#,
        )
        .expect("written");
        assert!(cache.load("future").is_none());
    }

    #[test]
    fn a_corrupt_cache_file_is_ignored_rather_than_failing() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = ModelCache::at(dir.path());
        for rubbish in [b"".as_slice(), b"{", b"not json at all", b"[]"] {
            std::fs::write(cache.path_for("broken"), rubbish).expect("written");
            assert!(
                cache.load("broken").is_none(),
                "a cache must never turn a bad file into a failed start"
            );
        }
    }

    #[test]
    fn a_half_written_file_never_replaces_a_good_one() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = ModelCache::at(dir.path());
        cache.store(&catalogue("p", &["a/one"])).expect("stored");
        // The temporary name the write goes through must not be the file
        // `load` reads, or a crash mid-write would be a corrupt cache.
        let final_path = cache.path_for("p");
        let temporary = final_path.with_extension("json.writing");
        assert_ne!(temporary, final_path);
        assert!(
            !temporary.exists(),
            "the temporary file must be renamed away, not left behind"
        );
        assert!(cache.load("p").is_some());
    }

    // --- GH-DEDUP-PROVIDER: the shared atomic-write helper -----------------

    /// [`write_json_atomically`] directly, at the primitive four caches now
    /// share: after a successful call the target exists with the full
    /// contents and no `.json.writing` sibling remains — the one property
    /// `ModelCache::store`, `GatewayQuotaCache::try_store`,
    /// `GatewayHealthCache::try_store` and `RoutingStickyCache::try_store`
    /// all rely on it for.
    #[test]
    fn write_json_atomically_leaves_the_target_whole_and_no_temporary_behind() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("reading.json");
        write_json_atomically(&path, b"{\"a\":1}").expect("written");

        assert_eq!(
            std::fs::read(&path).expect("readable"),
            b"{\"a\":1}",
            "the target must hold the full contents"
        );
        let temporary = path.with_extension("json.writing");
        assert!(
            !temporary.exists(),
            "the temporary file must be renamed away, not left behind"
        );
    }

    /// The end-state check above (target holds the full content, no
    /// `.json.writing` sibling) is satisfied just as well by a direct
    /// `std::fs::write(path, encoded)` on the happy path — it is not, on its
    /// own, a test of the temp-then-rename crash-safety pattern rather than
    /// of the function's return value. This one actually distinguishes the
    /// two: creating a **new** file (the temporary) needs write permission on
    /// its parent directory, but overwriting a file that **already exists**
    /// there does not (Unix `open(2)`: `O_TRUNC` on an existing path checks
    /// the file's own permission bits, not the directory's, unless a new
    /// directory entry has to be created). So with a read-only directory and
    /// a pre-existing target, the real implementation fails — it cannot
    /// create the temporary — while a direct-write implementation would
    /// happily overwrite the target and report success.
    #[cfg(unix)]
    #[test]
    fn write_json_atomically_cannot_succeed_by_writing_the_target_directly() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("reading.json");
        std::fs::write(&path, b"old").expect("seeded");

        let mut perms = std::fs::metadata(dir.path())
            .expect("metadata")
            .permissions();
        perms.set_mode(0o500); // read + execute, no write: no new directory entry
        std::fs::set_permissions(dir.path(), perms.clone()).expect("locked down");

        let result = write_json_atomically(&path, b"new");

        // Restore write permission unconditionally so `tempdir`'s own Drop
        // can clean up, regardless of which assertion below fires.
        perms.set_mode(0o700);
        std::fs::set_permissions(dir.path(), perms).expect("restored");

        assert!(
            result.is_err(),
            "a real temp-then-rename write cannot create its temporary file in a \
             directory it has no write permission on — a passing result here means \
             the implementation wrote the target directly instead"
        );
        assert_eq!(
            std::fs::read(&path).expect("still readable"),
            b"old",
            "a failed write must leave the previous target untouched"
        );
    }

    #[test]
    fn provider_json_path_matches_model_caches_own_path_for() {
        let cache = ModelCache::at("/tmp/does-not-need-to-exist");
        assert_eq!(
            provider_json_path(cache.root(), "openrouter"),
            cache.path_for("openrouter")
        );
    }

    // --- a changed base URL -----------------------------------------------

    #[test]
    fn a_catalogue_knows_which_base_url_it_came_from() {
        let read = catalogue("p", &["a/one"]);
        assert!(read.was_fetched_from("https://a.example/v1"));
        assert!(
            !read.was_fetched_from("https://b.example/v1"),
            "models cached from one service are not another service's models"
        );
    }

    // --- acceptance test 7, at this module's own boundary ------------------

    /// **The cache file is a new leak surface.** This plants a credential in
    /// the environment and in a provider name, writes a catalogue, and reads
    /// the bytes that actually landed on disk. Asserted with `!contains`, on
    /// raw bytes, never with `assert_eq!` — a failing equality assertion on
    /// secret material prints both sides.
    #[test]
    fn a_planted_credential_never_reaches_the_cache_file_on_disk() {
        const VALUE: &str = "sk-planted-cache-credential-9d";
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = ModelCache::at(dir.path());
        cache
            .store(&catalogue("openrouter", &["a/one", "b/two"]))
            .expect("stored");

        let path = cache.path_for("openrouter");
        let bytes = std::fs::read(&path).expect("the file we just wrote");
        assert!(
            !String::from_utf8_lossy(&bytes).contains(VALUE),
            "a credential reached the cache file at {}",
            path.display()
        );
        // And the type's own rendering cannot carry one either, because
        // there is no field for it to occupy.
        let read = cache.load("openrouter").expect("cached");
        assert!(!format!("{read:?}").contains(VALUE));
        assert!(!format!("{cache:?}").contains(VALUE));
    }

    /// The positive half of the assertion above: the file really does contain
    /// the things it is supposed to, so the `!contains` test could not be
    /// passing because nothing was written at all.
    #[test]
    fn the_cache_file_contains_the_models_and_the_timestamp_it_was_given() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = ModelCache::at(dir.path());
        cache
            .store(&catalogue("openrouter", &["vendor/model-a"]))
            .expect("stored");
        let text = std::fs::read_to_string(cache.path_for("openrouter")).expect("readable");
        assert!(text.contains("vendor/model-a"), "{text}");
        assert!(text.contains("1787336476"), "{text}");
        assert!(text.contains("openrouter"), "{text}");
    }

    // --- the clock ---------------------------------------------------------

    #[test]
    fn the_clock_returns_a_plausible_present_day_timestamp() {
        // 2020-01-01, comfortably in the past, and a bound far enough out
        // that this does not become a time bomb.
        let now = now_unix_seconds();
        assert!(now > 1_577_836_800, "got {now}");
        assert!(now < 4_102_444_800, "got {now}");
    }
}
