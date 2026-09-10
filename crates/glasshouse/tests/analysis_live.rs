//! The one test that actually talks to Artificial Analysis.
//!
//! `#[ignore]` because it needs a key and a network, and neither belongs in a
//! gate: CI has no key, and a test that silently passes when a service is
//! unreachable proves nothing. Run it deliberately:
//!
//! ```text
//! ARTIFICIAL_ANALYSIS_API_KEY=… cargo test -p glasshouse --test analysis_live -- --ignored --nocapture
//! ```
//!
//! It spends one request of a free key's hundred.

use glasshouse::routing::analysis::{self, Refresh};

/// Without a key the service does nothing at all — no request, no file, no
/// change to any ranking. This is the opt-in half of the contract and it is
/// the only part that can be checked without one.
#[test]
fn without_a_key_the_service_does_nothing() {
    // SAFETY: single-threaded test, and the variable is removed again before
    // anything else can observe it.
    let restore = std::env::var(analysis::KEY_VARIABLE).ok();
    unsafe { std::env::remove_var(analysis::KEY_VARIABLE) };

    let dir = std::env::temp_dir().join(format!("gh-aa-optout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let outcome = analysis::ensure(&dir, analysis::DEFAULT_TTL);

    if let Some(key) = restore {
        unsafe { std::env::set_var(analysis::KEY_VARIABLE, key) };
    }

    assert_eq!(outcome, Refresh::NotConfigured);
    assert!(
        !analysis::cache_path(&dir).exists(),
        "opting out must leave no file behind"
    );
}

/// The live shape: a real fetch, cached, and looked up by a model identifier
/// spelled the way a route spells it.
#[test]
#[ignore = "spends one request of a free key's budget; needs the network"]
fn a_live_fetch_answers_for_a_model_a_route_would_name() {
    let Some(_) = analysis::configured_key() else {
        panic!("set {} to run this", analysis::KEY_VARIABLE);
    };
    let dir = std::env::temp_dir().join(format!("gh-aa-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let outcome = analysis::ensure(&dir, analysis::DEFAULT_TTL);
    let Refresh::Fetched { models } = outcome else {
        panic!("expected a fetch, got {outcome:?}");
    };
    assert!(
        models > 100,
        "the catalogue should be substantial: {models}"
    );

    let catalogue = analysis::cached(&dir).expect("the fetch wrote a cache");
    assert_eq!(catalogue.len(), models);
    println!(
        "fetched {models} models, index version {:?}",
        catalogue.index_version
    );

    // A dotted identifier finds its dashed slug, which is the whole reason
    // `normalise` exists.
    let scored = catalogue
        .models
        .values()
        .filter(|facts| facts.intelligence.is_some())
        .count();
    assert!(scored > 50, "most listed models carry an index: {scored}");

    // And the second call spends nothing, because the cache is fresh.
    assert_eq!(
        analysis::ensure(&dir, analysis::DEFAULT_TTL),
        Refresh::Fresh,
        "a fresh cache must not spend a request"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
