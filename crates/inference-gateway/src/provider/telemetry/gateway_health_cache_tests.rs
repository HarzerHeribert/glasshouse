use super::*;

/// The read side, alone: a cache with nothing written for a provider
/// answers with an empty list rather than an error or a fabricated
/// healthy default — capability map line 1324's "never invent a reading"
/// half, at the type that owns the read.
#[test]
fn a_provider_with_no_stored_health_reads_as_an_empty_list() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayHealthCache::at(dir.path());
    assert_eq!(cache.load("anyrouter"), Vec::new());
    assert_eq!(cache.load_all(), Vec::new());
}

/// The write-then-read round trip, including that a cooldown survives it
/// as the absolute unix second it was given — never re-derived from a
/// process-local clock on the way back out.
#[test]
fn a_stored_reading_round_trips_including_a_cooldown_deadline() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayHealthCache::at(dir.path());
    let entries = vec![GatewayHealthReading {
        credential_label: "anyrouter/ANYROUTER_API_KEY".to_owned(),
        model: "anyrouter/free-model".to_owned(),
        consecutive_failures: 2,
        cooling_down_until_unix: Some(1_787_800_600),
        cooldown_cause: Some(crate::routing::free::CooldownCause::Declared),
        credential_rejected: false,
    }];
    cache.store("anyrouter", &entries, 1_787_800_000);

    assert_eq!(cache.load("anyrouter"), entries);
    assert_eq!(cache.load("groq"), Vec::new());
    assert_eq!(cache.load_all(), vec![("anyrouter".to_owned(), entries)]);
}

/// [`GatewayHealthCache::store`]'s own guard: an empty slice must not
/// clobber a real reading a previous exchange left on disk, mirroring
/// [`GatewayQuotaCache::store`]'s identical guard and its own test of it.
#[test]
fn an_empty_reading_does_not_clear_a_previous_one() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayHealthCache::at(dir.path());
    let entries = vec![GatewayHealthReading {
        credential_label: "anyrouter/ANYROUTER_API_KEY".to_owned(),
        model: "anyrouter/free-model".to_owned(),
        consecutive_failures: 0,
        cooling_down_until_unix: None,
        cooldown_cause: None,
        credential_rejected: false,
    }];
    cache.store("anyrouter", &entries, 1_787_800_000);
    cache.store("anyrouter", &[], 1_787_800_100);
    assert_eq!(cache.load("anyrouter"), entries);
}

/// A truncated file must read as "nothing observed", never as an error
/// that would fail `glasshouse resources` and never as a fabricated
/// healthy reading.
#[test]
fn a_truncated_health_cache_file_reads_as_an_empty_list() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayHealthCache::at(dir.path());
    cache.store(
        "anyrouter",
        &[GatewayHealthReading {
            credential_label: "anyrouter/ANYROUTER_API_KEY".to_owned(),
            model: "anyrouter/free-model".to_owned(),
            consecutive_failures: 1,
            cooling_down_until_unix: None,
            cooldown_cause: None,
            credential_rejected: false,
        }],
        1_787_800_000,
    );
    let path = dir.path().join(format!(
        "{}.json",
        crate::provider::cache::file_stem("anyrouter")
    ));
    let bytes = std::fs::read(&path).expect("the store above wrote a file");
    std::fs::write(&path, &bytes[..bytes.len() / 2]).expect("truncated");

    assert_eq!(cache.load("anyrouter"), Vec::new());
    assert_eq!(cache.load_all(), Vec::new());
}

/// A `.writing` temporary left behind by a crashed writer — the shape a
/// second producer in a separate process (`main.rs`'s support-work
/// dispatch, alongside the gateway) can now leave — must never surface as
/// a second reading for a provider that already has a real one on disk.
#[test]
fn a_leftover_writing_temporary_is_never_returned_as_a_reading() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayHealthCache::at(dir.path());
    let real_entries = vec![GatewayHealthReading {
        credential_label: "anyrouter/ANYROUTER_API_KEY".to_owned(),
        model: "anyrouter/free-model".to_owned(),
        consecutive_failures: 1,
        cooling_down_until_unix: None,
        cooldown_cause: None,
        credential_rejected: false,
    }];
    cache.store("anyrouter", &real_entries, 1_787_800_000);

    let stem = crate::provider::cache::file_stem("anyrouter");
    let real_path = dir.path().join(format!("{stem}.json"));
    let bytes = std::fs::read(&real_path).expect("the store above wrote a file");
    // A valid, fully-written temporary — the shape a crash right after
    // `std::fs::write` but before the rename leaves behind.
    let temporary_path = dir.path().join(format!("{stem}.4242-7.writing"));
    std::fs::write(&temporary_path, &bytes).expect("planted temporary");

    assert_eq!(
        cache.load_all(),
        vec![("anyrouter".to_owned(), real_entries.clone())],
        "load_all must return only the real reading, not the planted temporary"
    );
    assert_eq!(
        cache.load_all_dated(),
        vec![("anyrouter".to_owned(), 1_787_800_000, real_entries)],
        "load_all_dated must return only the real reading, not the planted temporary"
    );
}

/// A format version this build does not recognise must read as nothing —
/// [`GatewayQuotaCache`]'s own
/// `a_reading_stored_by_a_future_incompatible_format_is_ignored_rather_than_misread`
/// twin.
#[test]
fn a_reading_stored_by_a_future_format_version_is_ignored() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayHealthCache::at(dir.path());
    cache.store(
        "anyrouter",
        &[GatewayHealthReading {
            credential_label: "anyrouter/ANYROUTER_API_KEY".to_owned(),
            model: "anyrouter/free-model".to_owned(),
            consecutive_failures: 1,
            cooling_down_until_unix: None,
            cooldown_cause: None,
            credential_rejected: false,
        }],
        1_787_800_000,
    );
    let path = dir.path().join(format!(
        "{}.json",
        crate::provider::cache::file_stem("anyrouter")
    ));
    let bytes = std::fs::read(&path).expect("the store above wrote a file");
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
    value["version"] = serde_json::json!(99);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).expect("overwritten");

    assert_eq!(cache.load("anyrouter"), Vec::new());
}

/// [`GatewayHealthReading::is_available`] — the read-side twin of
/// [`crate::routing::free::ResourceHealth::is_available`], over a value
/// that has already crossed the process boundary rather than the
/// in-memory type itself.
#[test]
fn a_reading_is_available_exactly_when_not_rejected_and_not_still_cooling() {
    let healthy = GatewayHealthReading {
        credential_label: "anyrouter/ANYROUTER_API_KEY".to_owned(),
        model: "anyrouter/free-model".to_owned(),
        consecutive_failures: 0,
        cooling_down_until_unix: None,
        cooldown_cause: None,
        credential_rejected: false,
    };
    assert!(healthy.is_available(1_787_800_000));

    let cooling = GatewayHealthReading {
        cooling_down_until_unix: Some(1_787_800_600),
        ..healthy.clone()
    };
    assert!(!cooling.is_available(1_787_800_000));
    assert!(
        cooling.is_available(1_787_800_600),
        "a cooldown that has just elapsed reads as available again"
    );

    let rejected = GatewayHealthReading {
        credential_rejected: true,
        ..healthy
    };
    assert!(!rejected.is_available(1_787_800_000));
}
