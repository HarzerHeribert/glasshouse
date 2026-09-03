use super::*;

fn pick(provider: &str, model: &str, chosen_at_unix: i64) -> RetainedPick {
    RetainedPick {
        provider: provider.to_owned(),
        model: model.to_owned(),
        chosen_at_unix,
    }
}

#[test]
fn a_stored_pick_comes_back_unchanged() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = RoutingStickyCache::at(dir.path().join("routing-sticky.json"));
    let written = pick("groq", "kimi-k2", 1_800_000_000);
    cache.store(&written);
    assert_eq!(cache.load(), Some(written));
}

/// Acceptance test 5 (part one): a project that has never stored a pick
/// is a miss, never an error — [`GatewayQuotaCache::load`]'s own
/// contract, mirrored here.
#[test]
fn a_project_with_no_persisted_pick_is_a_miss_rather_than_an_error() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = RoutingStickyCache::at(dir.path().join("routing-sticky.json"));
    assert!(cache.load().is_none());
}

#[test]
fn storing_again_replaces_the_previous_pick() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = RoutingStickyCache::at(dir.path().join("routing-sticky.json"));
    cache.store(&pick("groq", "kimi-k2", 1_800_000_000));
    cache.store(&pick("nous", "hermes-4", 1_800_000_060));
    assert_eq!(cache.load(), Some(pick("nous", "hermes-4", 1_800_000_060)));
}

/// A file written by a future format version is a miss, not a misread —
/// [`GatewayQuotaCache`]'s own contract, mirrored here.
#[test]
fn a_future_format_version_is_ignored_rather_than_misread() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("routing-sticky.json");
    let cache = RoutingStickyCache::at(&path);
    cache.store(&pick("groq", "kimi-k2", 1_800_000_000));
    let bytes = std::fs::read(&path).expect("written above");
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
    value["version"] = serde_json::json!(99);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).expect("overwritten");
    assert!(cache.load().is_none());
}

/// Acceptance test 5 (part two): a corrupted or partially written file is
/// a miss, not a panic — the same crash-mid-write case
/// [`crate::provider::cache::ModelCache::store`]'s own doc names.
#[test]
fn a_truncated_file_is_a_miss_rather_than_a_panic() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("routing-sticky.json");
    std::fs::write(&path, b"{\"version\": 1, \"provider\": \"gro").unwrap();
    assert!(RoutingStickyCache::at(&path).load().is_none());
}

/// A missing parent directory must not make a first write fail — nothing
/// creates `project_state_dir` before the first sticky pick is stored.
#[test]
fn the_first_write_creates_its_own_parent_directory() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("nested").join("routing-sticky.json");
    let cache = RoutingStickyCache::at(&path);
    cache.store(&pick("groq", "kimi-k2", 1_800_000_000));
    assert_eq!(cache.load(), Some(pick("groq", "kimi-k2", 1_800_000_000)));
}
