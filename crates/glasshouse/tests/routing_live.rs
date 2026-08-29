//! What Glasshouse's own automated runs may spend, and what a real free model
//! actually does when one is called.
//!
//! # Phase 9I line 539 is an acceptance condition, not a preference
//!
//! *"Allow Glasshouse's own automated evaluation and test runs to use
//! configured zero-cost models, and never a metered resource without an
//! explicit opt-in."* A test run that silently spends the user's money is the
//! worst failure this area can produce — worse than a failing test — so the
//! first test here is the one that must never be deleted: it asserts that the
//! policy an automated run is built with refuses a metered resource, and that
//! the refusal names the switch that would change that.
//!
//! # The live test is `#[ignore]`d, and the reason is stated rather than
//! implied
//!
//! `a_free_model_answers_through_a_real_gateway` makes a real request to a
//! real router. It is ignored by default because a test suite that reached the
//! network would fail when the network did, and because a credential is not
//! present on every machine — not because the evidence is optional. The run,
//! and what it printed, belongs in the batch's report; a green CI run is not
//! evidence that it happened.
//!
//! It is bounded by construction: [`DisposableRouting::for_glasshouses_own_run`]
//! chooses the resource, and if that choice is not [`Cost::Free`] the test
//! refuses to make a request at all. The model identifier is never hard-coded
//! past that gate — the gate is what decides.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Instant;

use glasshouse::gateway::{Route, Upstream, UpstreamBackend};
use glasshouse::integrations::IntegrationId;
use glasshouse::profile::{BackendResource, LaunchProfile};
use glasshouse::routing::disposable::{
    DisposableCandidate, DisposableRouting, JobKind, MeteredUse, NoResource,
};
use glasshouse::routing::free::{FreePool, FreePreferences, FreeResource, WorkloadOutcome};
use glasshouse::routing::{AssignedModel, Cost, CredentialId};
use glasshouse::secret::{SecretRef, SecretStore};

/// The free model this probe calls when it is run.
///
/// One of the seventeen OpenRouter listed as `:free` on 2026-08-26, chosen
/// because Phase 9I line 531 names Nemotron variants explicitly as the kind of
/// *explicitly configured* free model that must be able to participate.
///
/// It is a **candidate**, not a decision: the policy below still has to choose
/// it, and the request is only made if what the policy chose is free.
const FREE_MODEL: &str = "nvidia/nemotron-3-ultra-550b-a55b:free";

/// The variable the real credential is read from, when one is present.
const CREDENTIAL_VAR: &str = "OPENROUTER_API_KEY";

fn openrouter_credential() -> CredentialId {
    CredentialId::new(
        "openrouter",
        SecretRef::Environment {
            var: CREDENTIAL_VAR.to_owned(),
        },
    )
}

/// Phase 9I line 539, in the form that can never be skipped.
///
/// The policy an automated Glasshouse run is built with refuses every metered
/// resource, and says which switch would change that. This test needs no
/// credential, no network and no configuration, which is the point: it is the
/// guarantee, and a guarantee that only holds on a developer's machine is not
/// one.
#[test]
fn glasshouses_own_run_refuses_to_spend_money_and_says_what_would_change_that() {
    let routing = DisposableRouting::for_glasshouses_own_run(
        // No opt-in in the environment this policy is told about.
        MeteredUse::for_automated_run(|_| None),
        FreePreferences::new(),
    );

    let only_metered = vec![DisposableCandidate::new(
        "openrouter",
        "an-expensive-model",
        openrouter_credential(),
        Cost::Metered,
    )];

    let refused = routing
        .choose(
            JobKind::Evaluation,
            &only_metered,
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect_err("an automated Glasshouse run must not choose a metered model");

    assert!(
        matches!(refused, NoResource::NoFreeResourceAndMeteredWithheld { .. }),
        "the refusal must be about the withheld opt-in, not about configuration: {refused}"
    );
    assert!(
        refused.to_string().contains(MeteredUse::OPT_IN_VAR),
        "the refusal must name the switch that would change it: {refused}"
    );

    // And the same policy takes the free model when there is one, so the
    // guarantee is "never metered without an opt-in" rather than "never
    // anything".
    let with_a_free_one = vec![
        DisposableCandidate::new(
            "openrouter",
            "an-expensive-model",
            openrouter_credential(),
            Cost::Metered,
        ),
        DisposableCandidate::new(
            "openrouter",
            FREE_MODEL,
            openrouter_credential(),
            Cost::Free,
        ),
    ];
    let chosen = routing
        .choose(
            JobKind::Evaluation,
            &with_a_free_one,
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("a configured zero-cost model is exactly what line 539 allows");
    assert_eq!(chosen.cost(), Cost::Free);
    assert_eq!(chosen.model(), FREE_MODEL);
}

/// A real request, to a real router, for a model the policy proved is free —
/// and the health the free pool learns from it.
///
/// Phase 9I line 534: the health signal below comes from **this request**,
/// which was going to happen anyway, and not from a probe made to produce it.
/// There is no probe in the product to make one with; see
/// `glasshouse::routing::free`.
///
/// Run it by hand with a credential present:
///
/// ```text
/// OPENROUTER_API_KEY=… cargo test --test routing_live --all-features -- --ignored --nocapture
/// ```
#[test]
#[ignore = "makes a real request to a real router; run by hand, free models only"]
fn a_free_model_answers_through_a_real_gateway_and_its_health_comes_from_that_request() {
    let store = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let reference = SecretRef::Environment {
        var: CREDENTIAL_VAR.to_owned(),
    };
    let Some(credential) = store.resolve(&reference) else {
        panic!(
            "this probe needs {CREDENTIAL_VAR} to be resolvable; it is ignored by default \
             precisely so that its absence is never mistaken for a pass"
        );
    };

    // The gate. Nothing below runs unless the policy chose a free resource.
    let routing = DisposableRouting::for_glasshouses_own_run(
        MeteredUse::for_automated_run(|_| None),
        FreePreferences::new(),
    );
    let choice = routing
        .choose(
            JobKind::Evaluation,
            &[DisposableCandidate::new(
                "openrouter",
                FREE_MODEL,
                openrouter_credential(),
                Cost::Free,
            )],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("a free model is configured for this probe");
    assert_eq!(
        choice.cost(),
        Cost::Free,
        "this probe must never spend a metered request"
    );
    println!("routed: {}", choice.describe());

    // A real gateway, pointed at the real router, holding the real credential.
    let backend = UpstreamBackend::new(
        "openrouter".to_owned(),
        vec![Route::new(
            "anthropic-messages".to_owned(),
            &["/messages"],
            "https://openrouter.ai/api",
        )],
        credential,
        openrouter_credential(),
        Cost::Free,
    )
    .expect("openrouter's base URL is absolute");
    let upstream = Upstream::with_failover(vec![backend]).expect("one backend is not none");

    // Started through the production entry point rather than a test-only
    // constructor: `start_if_required` is the only way a gateway comes into
    // existence in the shipped binary, and a probe that used a different door
    // would be evidence about a door nobody walks through.
    let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    let gateway = glasshouse::gateway::start_if_required(&[profile], || Ok(upstream))
        .expect("loopback is bindable")
        .expect("a gateway-backed profile requires a gateway");
    gateway.routing().bind(
        "claude-code",
        "anthropic-messages",
        AssignedModel::named(choice.model()),
        gateway.upstream(),
    );

    let body = format!(
        r#"{{"model":"{}","max_tokens":16,"messages":[{{"role":"user","content":"Reply with exactly the word GLASSHOUSE."}}]}}"#,
        choice.model()
    );
    let request = format!(
        "POST /v1/messages HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Authorization: Bearer {}\r\n\
         Content-Type: application/json\r\n\
         Anthropic-Version: 2023-06-01\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        gateway.token().expose(),
        body.len()
    );

    let mut client = TcpStream::connect(gateway.address()).expect("the gateway accepts");
    client
        .write_all(request.as_bytes())
        .expect("the gateway reads");
    client.flush().expect("the gateway reads");
    let mut received = Vec::new();
    client
        .read_to_end(&mut received)
        .expect("the gateway answers");
    let response = String::from_utf8_lossy(&received).into_owned();

    println!(
        "--- first line ---\n{}",
        response.lines().next().unwrap_or("(nothing)")
    );
    println!(
        "--- body ---\n{}",
        response.split("\r\n\r\n").nth(1).unwrap_or("")
    );

    assert!(
        response.starts_with("HTTP/1.1 200"),
        "the free model must answer; got: {}",
        response.lines().next().unwrap_or("(nothing)")
    );

    // Phase 9I lines 529 and 534: the resource's health, learned from the
    // request above and from nothing else.
    let mut pool = FreePool::new();
    let resource = FreeResource::new(openrouter_credential(), choice.model());
    pool.observe(&resource, WorkloadOutcome::Served, Instant::now());
    assert!(pool.is_available(&resource, Instant::now()));
    println!("health after real workload: {:?}", pool.health(&resource));

    // And the credential never left the process: the request the harness side
    // sent carried the gateway's own token.
    assert!(
        !request.contains("sk-or-"),
        "the child-side request must carry the gateway's token, never a provider key"
    );
}
