use super::*;

const NOW: i64 = 1_787_900_000;
const LABEL: &str = "free-runner/FREE_RUNNER_API_KEY";

fn cache() -> (tempfile::TempDir, DispatchReservationCache) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = DispatchReservationCache::at(dir.path().join("dispatch-reservations"));
    (dir, cache)
}

/// Nothing claimed is nothing reserved, and an absent directory is not an
/// error — [`GatewayQuotaCache`]'s own fail-soft contract.
#[test]
fn a_cache_nothing_has_claimed_reserves_nothing() {
    let (_dir, cache) = cache();
    assert_eq!(cache.reserved(LABEL, NOW), 0);
    assert_eq!(cache.live(NOW), Vec::new());
    assert!(!cache.root().exists(), "a read must not create the cache");
}

/// The mutual exclusion, at the type that owns it: a pool of one serves
/// one claim, and the second dispatcher is told so rather than being
/// handed the same request.
#[test]
fn a_pool_of_one_request_is_claimed_once() {
    let (_dir, cache) = cache();
    let first = cache
        .claim(LABEL, "a-free-model", 1, NOW)
        .expect("the only request must be claimable");
    assert_eq!(cache.reserved(LABEL, NOW), 1);
    assert!(
        cache.claim(LABEL, "a-free-model", 1, NOW).is_none(),
        "a request already spoken for must not be handed out twice"
    );

    first.release();
    assert_eq!(cache.reserved(LABEL, NOW), 0);
    assert!(
        cache.claim(LABEL, "a-free-model", 1, NOW).is_some(),
        "a released request is back in the pool"
    );
}

/// A pool of two serves two, which is what makes the refusal above a
/// statement about the remainder rather than about claiming at all.
#[test]
fn a_pool_of_two_requests_serves_two_dispatches_and_no_more() {
    let (_dir, cache) = cache();
    assert!(cache.claim(LABEL, "a-free-model", 2, NOW).is_some());
    assert!(cache.claim(LABEL, "another-model", 2, NOW).is_some());
    assert_eq!(
        cache.reserved(LABEL, NOW),
        2,
        "two models behind one credential draw down one pool"
    );
    assert!(cache.claim(LABEL, "a-free-model", 2, NOW).is_none());
}

/// Capability map line 1367's *never blocks a pool for ever*: a row a
/// killed process left behind stops counting at its deadline, and the
/// slot is taken over rather than left occupied.
#[test]
fn an_expired_row_stops_counting_and_its_slot_is_taken_over() {
    let (_dir, cache) = cache();
    cache
        .plant(
            0,
            &DispatchReservation {
                credential_label: LABEL.to_owned(),
                model: "a-free-model".to_owned(),
                requests: 1,
                process_id: 999_999,
                reserved_at_unix: NOW - 600,
                expires_at_unix: NOW - 60,
            },
        )
        .expect("the row must be plantable");

    assert_eq!(
        cache.reserved(LABEL, NOW),
        0,
        "a deadline that has passed reserves nothing"
    );
    let lease = cache
        .claim(LABEL, "a-free-model", 1, NOW)
        .expect("the dead row's slot must be claimable");
    assert_eq!(cache.reserved(LABEL, NOW), 1);
    assert_eq!(
        cache.live(NOW).len(),
        1,
        "the takeover replaces the dead row rather than adding to it"
    );
    lease.release();
}

/// The row carries two names and a model, and the reader gets them back.
#[test]
fn a_claim_records_the_label_and_the_model_and_expires_on_its_own_schedule() {
    let (_dir, cache) = cache();
    let _lease = cache
        .claim(LABEL, "a-free-model", 4, NOW)
        .expect("a pool of four serves this claim");

    let live = cache.live(NOW);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].credential_label, LABEL);
    assert_eq!(live[0].model, "a-free-model");
    assert_eq!(live[0].requests, 1);
    assert_eq!(live[0].process_id, std::process::id());
    assert_eq!(
        live[0].expires_at_unix,
        NOW + DISPATCH_RESERVATION_LEASE.as_secs() as i64
    );
    assert_eq!(
        cache.reserved(LABEL, NOW + DISPATCH_RESERVATION_LEASE.as_secs() as i64),
        0,
        "the lease is what bounds the row, and it is absolute"
    );
}

/// Two credentials are two pools — line 538's per-credential quota state,
/// applied to what is in flight rather than to what was spent.
#[test]
fn one_credentials_reservation_says_nothing_about_another() {
    let (_dir, cache) = cache();
    let _lease = cache
        .claim(LABEL, "a-free-model", 1, NOW)
        .expect("a pool of one serves the first claim");
    assert_eq!(cache.reserved("other-runner/OTHER_KEY", NOW), 0);
    assert!(
        cache
            .claim("other-runner/OTHER_KEY", "a-free-model", 1, NOW)
            .is_some(),
        "one credential being spoken for must not take another out of service"
    );
}

/// A claim caught between its exclusive create and its description still
/// holds the request — the state a reader can genuinely observe, and the
/// one where treating "unreadable" as "free" would double-spend.
#[test]
fn a_slot_claimed_but_not_yet_described_still_holds_its_request() {
    let (_dir, cache) = cache();
    std::fs::create_dir_all(cache.root()).unwrap();
    let path = cache.path_for(LABEL, 0);
    std::fs::write(&path, b"").unwrap();

    assert_eq!(
        cache.reserved(LABEL, now_unix_for_test()),
        1,
        "an empty slot file is a claim in progress, not a free request"
    );
    assert!(
        cache
            .claim(LABEL, "a-free-model", 1, now_unix_for_test())
            .is_none()
    );
}

/// The same file, once its own age is past the lease: a claim nobody
/// could describe expires on the same schedule as one that was.
#[test]
fn a_slot_that_was_never_described_expires_like_any_other() {
    let (_dir, cache) = cache();
    std::fs::create_dir_all(cache.root()).unwrap();
    std::fs::write(cache.path_for(LABEL, 0), b"").unwrap();

    let later = now_unix_for_test() + DISPATCH_RESERVATION_LEASE.as_secs() as i64 + 1;
    assert_eq!(cache.reserved(LABEL, later), 0);
    assert!(cache.claim(LABEL, "a-free-model", 1, later).is_some());
}

/// `written_at` is read from the filesystem, so the two tests above need
/// the real clock rather than the fixed second the others use.
fn now_unix_for_test() -> i64 {
    crate::provider::cache::now_unix_seconds()
}

/// The credential label is a provider and a variable name, and the file
/// on disk carries exactly that and no value — the invariant line 1367
/// inherits from every other cache in this module.
#[test]
fn no_credential_value_can_reach_the_row() {
    let (_dir, cache) = cache();
    let lease = cache
        .claim(LABEL, "a-free-model", 1, NOW)
        .expect("a pool of one serves this claim");
    let text = std::fs::read_to_string(lease.path()).expect("the row must be readable");
    assert!(text.contains(LABEL));
    assert!(text.contains("a-free-model"));
    assert!(
        !text.contains("sk-"),
        "a reservation carries names, never a credential value: {text}"
    );
}
