use super::*;
use std::sync::{Arc, Mutex};

use glasshouse::Runtime;
use glasshouse::checkpoint::ProjectCheckpoints;
use glasshouse::checkpoint::git::GitPosition;
use glasshouse::cli::CheckpointCommand;
use glasshouse::config::response::ResponseRequest;
use glasshouse::events::EventLog;
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionRuntime};

/// Phase 9J line 576's §35 proof for `main.rs`'s own two lines: what a
/// user actually configured reaches `GatewayPairing`, not
/// `glasshouse::profile::GatewayPairing::default`'s `"strong"`. If
/// `resolved_gateway_pairing` (or either of its two call sites) went back
/// to constructing `GatewayPairing::default()` instead of resolving
/// `effective.native_pairing_preference()`, this would still read
/// `"strong"` and fail.
#[test]
fn resolved_gateway_pairing_reflects_the_users_configured_preference() {
    let mut user = UserConfig::default();
    user.pairing_mut()
        .set_native_pairing_preference(Some(glasshouse::config::pairing::PairingPreference::Off));
    let effective = EffectiveConfig::new(&user, None);

    let pairing = crate::commands::launch::resolved_gateway_pairing(&effective);

    assert_eq!(
        pairing.preference_slug, "off",
        "the user configured `off`; a default-valued `GatewayPairing` would read `strong` \
         instead"
    );
}

/// The out-of-the-box answer, for a user who has never configured this —
/// matches `EffectiveConfig::native_pairing_preference`'s own documented
/// default, and `GatewayPairing::default`'s.
#[test]
fn resolved_gateway_pairing_defaults_to_strong_when_nothing_is_configured() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);

    let pairing = crate::commands::launch::resolved_gateway_pairing(&effective);

    assert_eq!(pairing.preference_slug, "strong");
}

/// Hold the runtime's lock for `held`, signalling once it is definitely
/// taken so a test never races its own fixture.
fn hold_lock_for(
    live: &Arc<Mutex<SessionRuntime>>,
    held: std::time::Duration,
) -> std::thread::JoinHandle<()> {
    let live = Arc::clone(live);
    let (taken, is_taken) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let guard = live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        taken.send(()).expect("the test is still waiting");
        std::thread::sleep(held);
        drop(guard);
    });
    is_taken.recv().expect("the holder thread took the lock");
    holder
}

/// The regression for the orphan race, made deterministic.
///
/// The end-to-end version of this lives in `pty_smoke` and is
/// probabilistic — it caught the defect once in a hundred runs under
/// load, and once on macOS CI. This one holds the lock on purpose, so it
/// fails every time rather than one time in a hundred.
#[test]
fn a_forced_exit_cleanup_waits_out_a_briefly_held_lock() {
    let live = Arc::new(Mutex::new(SessionRuntime::new()));
    let holder = hold_lock_for(&live, std::time::Duration::from_millis(100));

    let reached = crate::commands::resume::close_before_forced_exit(
        &live,
        &SessionId::new("headless"),
        std::time::Duration::from_secs(5),
    );

    assert!(
        reached,
        "the cleanup gave up while the lock was merely busy, which is how a \
         real harness gets orphaned"
    );
    holder.join().expect("holder thread");
}

/// The other direction, and the reason the bound exists at all: a lock
/// that is never released must not keep the process from exiting.
#[test]
fn a_forced_exit_cleanup_gives_up_rather_than_hanging() {
    let live = Arc::new(Mutex::new(SessionRuntime::new()));
    let holder = hold_lock_for(&live, std::time::Duration::from_secs(3));

    let started = std::time::Instant::now();
    let reached = crate::commands::resume::close_before_forced_exit(
        &live,
        &SessionId::new("headless"),
        std::time::Duration::from_millis(50),
    );
    let waited = started.elapsed();

    assert!(
        !reached,
        "nothing could have reached a lock held throughout"
    );
    assert!(
        waited < std::time::Duration::from_secs(2),
        "the bound was not honoured: waited {waited:?}, which on the real \
         forced-exit path is a process that will not die"
    );
    holder.join().expect("holder thread");
}

/// What the code did before the bound existed, kept as a test so the
/// defect cannot quietly return: a single attempt against a busy lock
/// simply loses, and a lost attempt is a permanently orphaned harness.
#[test]
fn a_single_attempt_loses_the_race_that_the_bound_wins() {
    let live = Arc::new(Mutex::new(SessionRuntime::new()));
    let holder = hold_lock_for(&live, std::time::Duration::from_millis(200));

    let one_shot = crate::commands::resume::close_before_forced_exit(
        &live,
        &SessionId::new("headless"),
        std::time::Duration::ZERO,
    );

    assert!(
        !one_shot,
        "if a zero bound now succeeds, the retry loop stopped being what \
         makes this safe and this test is no longer measuring anything"
    );
    holder.join().expect("holder thread");
}

/// Every gateway this binary starts is handed the routing evidence
/// ledger — Phase 33A's wiring, which no behavioural test can reach.
///
/// **Why a source scan rather than a real assertion.** Both call sites are
/// inside `launch_session` and `resolve_resume_overlay`, and reaching
/// either needs a launch profile that actually requires a gateway plus a
/// real harness process. The integrator removed the ledger from both sites
/// and the entire suite stayed green — so the wiring was, to the tests,
/// invisible. That is the gap this closes, and it is the same reason
/// `a_single_attempt_loses_the_race_that_the_bound_wins` above exists:
/// keep a defect from quietly returning when nothing else would notice.
///
/// It deliberately proves *structure*, not behaviour, and the evidence
/// ledger says so — Phase 33A's boxes do not close on this test. What it
/// prevents is a future edit dropping the ledger back to `None` with
/// nothing to object.
///
/// Scans by `str::lines` via `production_code` (§14): `include_str!` reads
/// the file as checked out, and a CRLF checkout would otherwise make a
/// multi-line search silently find nothing.
#[test]
fn every_gateway_the_binary_starts_is_given_the_evidence_ledger() {
    let code = all_production_code();

    // Both doors, counted together. `start_if_required_with_degrade_sink`
    // is `start_if_required_with_telemetry` plus map line 1735's sink and
    // is what both sites call today; counting only the older name would
    // have made this test pass with *zero* gateways found, which is the
    // §68 shape — a filter that matches nothing looks exactly like a pass.
    let starts = code.matches("start_if_required_with_telemetry(").count()
        + code.matches("start_if_required_with_degrade_sink(").count();
    // Counts the *gated* form on purpose. An ungated `evidence_ledger(runtime)`
    // is the exact shape that hung six Windows tests for 37 minutes, so this
    // test must not accept it back.
    let ledgers = code
        .matches("evidence_ledger(runtime, std::slice::from_ref(&launch_profile))")
        .count();
    assert_eq!(
        starts, 2,
        "this binary should start a gateway at exactly two sites (launch and \
         resume); if that changed, this test needs to change with it"
    );
    assert_eq!(
        ledgers, starts,
        "a gateway is started somewhere without being handed the routing \
         evidence ledger: Phase 33A records nothing for that path, and no \
         behavioural test in this crate would notice"
    );
    assert!(
        !code.contains("start_if_required_with_quota_cache("),
        "a call site still uses the pre-Phase-33A entry point, which cannot \
         carry an evidence ledger at all"
    );
}

/// Map line 1735's structural half: every gateway this binary starts is
/// also given somewhere to report a failed upstream.
///
/// The same standing and the same limits as the evidence-ledger scan
/// above — it proves *presence*, not behaviour, and line 1735 does not
/// close on it. `gateway_degrade::the_shipped_binary_records_a_gateway_\
/// failure_against_the_session_it_launched` is what closes the line; this
/// exists so that a future edit dropping one of the two sites back to
/// `None` has something to object, since only one of the two paths has a
/// behavioural test.
#[test]
fn every_gateway_the_binary_starts_is_given_somewhere_to_report_a_failure() {
    let code = all_production_code();

    let starts = code.matches("start_if_required_with_degrade_sink(").count();
    // The argument each start is actually given. `launch_session` hands
    // its relay's sink in directly; the resume path builds the relay in
    // `resume_session` and passes it down, so the argument at the start
    // itself is the forwarded parameter.
    let sinks = code.matches("Some(degrade_relay.sink()),").count()
        + code.matches("Some(degrade_sink),").count();
    assert_eq!(
        starts, 2,
        "this binary should start a gateway at exactly two sites (launch and \
         resume); if that changed, this test needs to change with it"
    );
    assert_eq!(
        sinks, starts,
        "a gateway is started somewhere without a degrade sink, so its \
         upstream failing would be recorded nowhere — which is the state \
         map line 1735 was refused in"
    );
    assert_eq!(
        code.matches("DegradeRelay::new()").count(),
        starts,
        "each gateway start needs its own relay: two paths sharing one \
         would report a failure against the other's session"
    );
    assert_eq!(
        code.matches("degrade_relay.install(").count(),
        starts,
        "a relay is built and never installed: its sink would hold every \
         failure and write none of them"
    );
}

/// This file's own source, with its `#[cfg(test)]` block (and `//`
/// comments) stripped — the same idiom as
/// `harness::resolving_a_launch_profile_touches_no_files`'s
/// `production_code` helper, used here to prove structure rather than to
/// forbid a name.
fn production_code(source: &str) -> String {
    source
        .split("#[cfg(test)]")
        .next()
        .expect("split always yields at least one part")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every file `main.rs` was decomposed into (Phase 59, objective 2b) —
/// the source scans below used to read `main.rs` alone because every
/// subcommand's implementation lived there; the move relocated the code
/// these scans look for without changing it, so the scans now have to
/// cover `commands/*.rs` too or they would silently find nothing. Add a
/// new file here whenever one is added under `commands/`.
const PRODUCTION_SOURCE_FILES: &[&str] = &[
    include_str!("main.rs"),
    include_str!("commands/status.rs"),
    include_str!("commands/entitlements.rs"),
    include_str!("commands/gateway.rs"),
    include_str!("commands/setup.rs"),
    include_str!("commands/response.rs"),
    include_str!("commands/resources.rs"),
    include_str!("commands/cost.rs"),
    include_str!("commands/context_firewall.rs"),
    include_str!("commands/credentials.rs"),
    include_str!("commands/sessions.rs"),
    include_str!("commands/memory.rs"),
    include_str!("commands/memory_extraction.rs"),
    include_str!("commands/checkpoint.rs"),
    include_str!("commands/hook.rs"),
    include_str!("commands/shim.rs"),
    include_str!("commands/assumptions.rs"),
    include_str!("commands/launch.rs"),
    include_str!("commands/resume.rs"),
    include_str!("commands/shared.rs"),
    include_str!("commands/gateway_forward.rs"),
    include_str!("commands/migrate_gateway_state.rs"),
];

/// [`production_code`], applied to every file in [`PRODUCTION_SOURCE_FILES`]
/// and joined — the whole binary crate's production source as one string,
/// each file's own `#[cfg(test)]` tail stripped first so a later file's
/// tests cannot leak into an earlier file's stripped output.
fn all_production_code() -> String {
    PRODUCTION_SOURCE_FILES
        .iter()
        .map(|source| production_code(source))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `glasshouse run` exists only so a generated shim has a stable name to
/// `exec` into (see `glasshouse::shim`'s module doc); Phase 9B's
/// guarantee is that it behaves exactly like `glasshouse launch`. The
/// guarantee is structural, not merely observed: `run` and `launch`
/// match together in one arm in `run()` above and call `launch_session`
/// from there, so there is exactly one call site in production code for
/// this test to find — a second one would mean the two commands had
/// drifted onto separate paths.
#[test]
fn glasshouse_run_and_glasshouse_launch_take_the_same_path() {
    let code = all_production_code();
    // `return ...launch_session(` matches only an actual call, never the
    // `fn launch_session(` definition line itself. Phase 59 moved the
    // dispatch arm's call site into `main.rs` behind a qualified path,
    // so the call now reads `return crate::commands::launch::launch_session(`.
    let call_sites = code
        .matches("return crate::commands::launch::launch_session(")
        .count();
    assert_eq!(
        call_sites, 1,
        "`glasshouse run` and `glasshouse launch` must dispatch through exactly one call \
         to `launch_session` so they cannot diverge; found {call_sites} call sites"
    );
}

fn fixture_with_enabled_claude_code(tmp: &std::path::Path) -> Runtime {
    let root = tmp.join("project");
    std::fs::create_dir_all(root.join(".git")).unwrap();

    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        tmp.join("data").to_str().unwrap(),
        "--config-dir",
        tmp.join("config").to_str().unwrap(),
    ])
    .unwrap();
    let runtime = glasshouse::bootstrap(&cli, &root).unwrap();

    let decoy = tmp.join("fake-claude");
    std::fs::write(&decoy, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&decoy).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&decoy, perms).unwrap();
    }

    let mut user = UserConfig::load(runtime.paths()).unwrap();
    user.integrations_mut()
        .entry(glasshouse::integrations::IntegrationId::ClaudeCode)
        .set_enabled(true)
        .set_executable(Some(decoy));
    user.save(runtime.paths()).unwrap();

    runtime
}

/// `GH-LAUNCH-BRIEFING`'s test (e): the delivery ladder's third rung —
/// no adapter additive mechanism and no session runtime to fall back to
/// (this launch is not headless). Every adapter this build ships except
/// Claude Code declares no additive mechanism (`response.rs`'s own
/// `an_adapter_that_declares_nothing_says_so_rather_than_inventing_a_mechanism`),
/// so Codex stands in for "the harness whose adapter declares none".
///
/// A unit test on `brief_launch_session` itself rather than a
/// shipped-binary test, per the packet's own escape hatch: reaching rung
/// three through the real binary needs an *embedded* (non-headless)
/// launch, and `session::attach` refuses to run at all without a real
/// terminal on both ends — which a `cargo test` process never has. That
/// makes the harness never spawn, so there is no argv to read back and
/// nothing to assert `"not briefed"` against other than a vacuous
/// absence (§17). This test asserts the ladder's own decision directly
/// instead.
#[test]
fn rung_three_fires_with_no_additive_mechanism_and_no_session_runtime() {
    use glasshouse::integrations::IntegrationId;
    use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};

    let codex = glasshouse::harness::adapter_for(IntegrationId::Codex).unwrap();
    assert!(
        codex.additive_response_injection().is_none(),
        "this test is vacuous unless Codex declares no additive mechanism"
    );

    let tmp = tempfile::tempdir().unwrap();
    let runtime = fixture_with_enabled_claude_code(tmp.path());
    let project = ProjectMemory::open(&runtime).unwrap();
    project
        .store()
        .record(
            NewMemory::new(MemoryKind::Constraint, "Some current binding memory.")
                .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .unwrap();
    drop(project);

    let mut response_application =
        glasshouse::harness::response::Application::none("no response profile is under test here");
    let session = SessionId::new("rung-three-test-session");
    let briefing = crate::commands::launch::brief_launch_session(
        &runtime,
        &session,
        codex,
        false, // headless: false, so there is no session runtime to fall back to
        false, // no_memory
        true,  // inject_at_launch
        None,
        &mut response_application,
    );

    match briefing {
        crate::commands::launch::LaunchBriefing::NotBriefed(reason) => {
            assert!(
                reason.contains("no mechanism"),
                "the reason must name why: {reason}"
            );
        }
        other => panic!(
            "expected rung three (`NotBriefed`) with no additive mechanism and headless \
             false; got {other:?}"
        ),
    }
    assert!(
        response_application.args().is_empty(),
        "rung three must never touch the response application's arguments"
    );
}

#[test]
fn a_refused_profile_starts_no_process_and_records_no_session() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = fixture_with_enabled_claude_code(tmp.path());

    // A provider-backed profile is always refused in Phase 9A (Phase
    // 9C/9D supply the provider configuration it would need).
    let mut user = UserConfig::load(runtime.paths()).unwrap();
    let mut profile =
        glasshouse::config::ProfileConfig::new(glasshouse::integrations::IntegrationId::ClaudeCode);
    profile.set_backend(glasshouse::config::ProfileBackend::DirectProvider {
        provider: "openrouter".to_owned(),
    });
    user.profiles_mut().set("gateway", profile);
    user.save(runtime.paths()).unwrap();

    let status = crate::commands::launch::launch_session(
        &runtime,
        Some("claude-code"),
        crate::commands::launch::LaunchDestination {
            profile: Some("gateway"),
            ..crate::commands::launch::LaunchDestination::default()
        },
        &ResponseRequest::default(),
        false,
        false,
        crate::commands::launch::ExternalPresentation::Embedded,
        &[],
        None,
    )
    .unwrap();
    assert_eq!(status, ExitCode::FAILURE);

    let sessions = glasshouse::session::ProjectSessions::open(&runtime).unwrap();
    assert!(
        sessions.store().list().unwrap().is_empty(),
        "a refused profile must record no session"
    );
}

#[test]
fn an_unacknowledged_bypass_also_starts_no_process_and_records_no_session() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = fixture_with_enabled_claude_code(tmp.path());

    let mut user = UserConfig::load(runtime.paths()).unwrap();
    let mut profile =
        glasshouse::config::ProfileConfig::new(glasshouse::integrations::IntegrationId::ClaudeCode);
    profile.set_approval(glasshouse::config::ProfileApproval::Bypass);
    user.profiles_mut().set("yolo", profile);
    user.save(runtime.paths()).unwrap();

    let status = crate::commands::launch::launch_session(
        &runtime,
        Some("claude-code"),
        crate::commands::launch::LaunchDestination {
            profile: Some("yolo"),
            ..crate::commands::launch::LaunchDestination::default()
        },
        &ResponseRequest::default(),
        false,
        false,
        crate::commands::launch::ExternalPresentation::Embedded,
        &[],
        None,
    )
    .unwrap();
    assert_eq!(status, ExitCode::FAILURE);

    let sessions = glasshouse::session::ProjectSessions::open(&runtime).unwrap();
    assert!(sessions.store().list().unwrap().is_empty());
}

#[test]
fn a_native_profile_launch_records_its_profile_name_and_backend() {
    // Not a full launch (that needs a real PTY-attachable harness); this
    // exercises everything `launch_session` does up to and including the
    // session record, by stopping the resolved profile one step short of
    // `HarnessLaunch` and checking what would have been recorded.
    let tmp = tempfile::tempdir().unwrap();
    let runtime = fixture_with_enabled_claude_code(tmp.path());
    let user = UserConfig::load(runtime.paths()).unwrap();
    let project = config::load_project_config(runtime.project()).unwrap();
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let selection = glasshouse::session::select::select(Some("claude-code"), effective).unwrap();

    let resolved = effective
        .launch_profile(glasshouse::profile::NATIVE_PROFILE_NAME, selection.id())
        .unwrap()
        .value;
    assert_eq!(resolved.name, "native");
    assert_eq!(resolved.backend.slug(), "native");

    let secrets = glasshouse::secret::EnvironmentSecretStore::new();
    let overlay = glasshouse::profile::resolve(
        &resolved,
        &glasshouse::profile::Resolution {
            adapter: selection.adapter(),
            acknowledged_bypass: false,
            provider: None,
            secrets: &secrets,
        },
    )
    .unwrap();
    assert!(crate::commands::resume::mechanism_summary(&overlay).contains("automatic review"));
}

// --- the hook handler never reads its payload -------------------------

/// Every field a Codex hook payload can carry, per
/// `docs/product/design-decisions.md`'s "Codex lifecycle hooks" section:
/// the six every event carries, plus `SessionStart`'s `source`,
/// `UserPromptSubmit`'s `turn_id`/`prompt`, and `Stop`'s
/// `stop_hook_active`/`last_assistant_message`. `prompt` and
/// `last_assistant_message` are the conversation itself.
const HOOK_PAYLOAD_FIELDS: &[&str] = &[
    "session_id",
    "transcript_path",
    "hook_event_name",
    "permission_mode",
    "source",
    "turn_id",
    "prompt",
    "stop_hook_active",
    "last_assistant_message",
];

/// The hook handler's own source, isolated from the rest of this file. A
/// whole-file scan would trip on legitimate, unrelated code — this
/// module's own `native_session_id` and `cwd` locals are not the Codex
/// payload fields of the same or similar name — so this extracts just
/// the one function the design decision is actually about.
///
/// **`report_hook_with`, not `report_hook`.** The two were one function
/// until extraction needed a model seam; `report_hook` is now a
/// two-line wrapper and scanning *it* would pass trivially, which is
/// this test's own stated failure mode — a scan over the wrong span
/// passing for the wrong reason. The anchor assertion below is what
/// caught the split when it happened.
fn hook_handler_source() -> String {
    let full = all_production_code();
    let start = full
        .find("fn report_hook_with(")
        .expect("report_hook_with must exist in this file");
    let after_start = &full[start..];
    // `"\n}"` rather than `"\n}\n"`: on Windows this file is checked out
    // with CRLF endings, so the closing brace reads `\r\n}\r\n` and a
    // pattern demanding `\n` on both sides never matches. Windows CI caught
    // exactly that. Matching only the newline *before* the brace works on
    // both, and a brace at column zero can only be this function's own.
    let end = after_start
        .find("\n}")
        .expect("report_hook must have a top-level closing brace");
    let body = &after_start[..end];
    // The slice must be the real function, not an empty or truncated one.
    // A scan over the wrong span passes for the wrong reason, which this
    // project has been caught by before — a `skip_while` that found a
    // harness *list* where an adapter *block* was meant. Anchor on
    // something the handler provably contains.
    assert!(
        body.contains("std::io::sink()"),
        "hook_handler_source() did not capture the real `report_hook` body; \
         the payload scan below would be checking nothing"
    );
    body.to_owned()
}

/// Strip `//` line comments, so a doc comment that merely *mentions* a
/// forbidden name (as this file's own comments now do) cannot fail the
/// scan below.
fn strip_comments(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_hook_command_never_reads_its_payload() {
    let source = strip_comments(&hook_handler_source());

    for forbidden in ["serde_json", "from_str", "from_reader"] {
        assert!(
            !source.contains(forbidden),
            "the hook handler names `{forbidden}`, so it might parse the payload it must \
             only drain and discard"
        );
    }
    for field in HOOK_PAYLOAD_FIELDS {
        assert!(
            !source.contains(field),
            "the hook handler names the payload field `{field}`, which must never be read, \
             logged, or stored"
        );
    }
}

#[test]
fn the_payload_scan_would_catch_a_violation() {
    // The guard above is only worth having if it can fail.
    let violating = "fn report_hook(runtime: &Runtime, session: &str, event: &str) {\n    \
                      let payload: serde_json::Value = serde_json::from_str(\"{}\").unwrap();\n}\n";
    assert!(strip_comments(violating).contains("serde_json"));
    assert!(strip_comments(violating).contains("from_str"));

    let reading_a_field = "fn report_hook(runtime: &Runtime, session: &str, event: &str) {\n    \
                            tracing::debug!(prompt = \"x\");\n}\n";
    assert!(strip_comments(reading_a_field).contains("prompt"));
}

/// An outcome that recorded nothing, which every case below varies one
/// field of.
///
/// A struct literal rather than a constructor because
/// `ExtractionOutcome::empty` is private to its own module and needs a
/// `SessionChunk` this decision has nothing to do with. One literal, so a
/// field added to the outcome breaks one place.
fn recorded_nothing() -> glasshouse::memory::ExtractionOutcome {
    glasshouse::memory::ExtractionOutcome {
        trigger: glasshouse::memory::ExtractionTrigger::BeforeCompaction,
        model: "a test model".to_owned(),
        session_id: "s".to_owned(),
        commit: None,
        recorded: Vec::new(),
        lowered: Vec::new(),
        speculative: 0,
        duplicates: 0,
        rejected: Vec::new(),
        activity_dropped: 0,
        activity_truncated: 0,
        redactions: 0,
        paths_dropped: 0,
        failure: None,
        call: None,
    }
}

/// A model that could not be asked is a memory that should exist and does
/// not, and [`lost_extraction_notice`] says so naming both the trigger and
/// the reason.
///
/// The reason is [`glasshouse::memory::ModelError`]'s own `Display`, which
/// is a fixed phrase by construction — this line reaches a person's
/// terminal, and a provider error body can echo the prompt that was sent.
#[test]
fn a_failed_extraction_is_reported_with_its_trigger_and_its_reason() {
    let mut outcome = recorded_nothing();
    outcome.failure = Some(glasshouse::memory::extract::ExtractionFailure::Model(
        glasshouse::memory::ModelError::Refused,
    ));

    let notice = crate::commands::memory_extraction::lost_extraction_notice(
        "before_compaction",
        Some(&outcome),
    )
    .expect("a failed extraction is a lost memory");
    assert!(
        notice.contains("before_compaction"),
        "the notice must name which boundary lost its memory: {notice}"
    );
    assert!(
        notice.contains("the extraction model declined the request"),
        "the notice must say why: {notice}"
    );
}

/// Map line 488's consequence for the extraction hook, said out loud: a
/// configured provider whose credential resolves from neither the hook's
/// environment nor the native store is named — provider and variable — on
/// the one stderr line the hook has, with the store instruction, and a
/// provider whose credential does resolve produces no notice and no value
/// anywhere. Built through the production seams (`withheld_provider_credentials`,
/// `noting_withheld_credentials`, `lost_extraction_notice`) against the
/// environment store alone, so no keychain and no network is touched.
#[test]
fn a_withheld_provider_credential_is_named_by_the_extraction_notice_and_never_its_value() {
    use glasshouse::config::{EffectiveConfig, UserConfig};

    const VAR: &str = "GLASSHOUSE_TEST_WITHHELD_KEY";
    const SET_VAR: &str = "GLASSHOUSE_TEST_WITHHELD_SET_KEY";
    const VALUE: &str = "hunter2-placeholder-never-a-real-key";
    assert!(
        std::env::var_os(VAR).is_none(),
        "test setup: {VAR} is already set"
    );

    let user: UserConfig = toml::from_str(&format!(
        "version = 1\n\n[providers.withheld]\ntemplate = \"openrouter\"\ncredential_env = \
         [\"{VAR}\"]\n"
    ))
    .expect("user config");
    let effective = EffectiveConfig::new(&user, None);
    let secrets = glasshouse::secret::EnvironmentSecretStore::new();

    let withheld =
        crate::commands::shared::withheld_provider_credentials(&effective, &secrets, None);
    assert_eq!(
        withheld.len(),
        1,
        "one configured provider, one withheld credential"
    );
    assert_eq!(withheld[0].provider, "withheld");
    assert_eq!(withheld[0].vars, [VAR.to_owned()]);

    let model = crate::commands::memory_extraction::noting_withheld_credentials(
        Box::new(crate::commands::memory_extraction::NoExtractionModel),
        &withheld,
    );
    let mut outcome = recorded_nothing();
    outcome.model = model.describe();
    outcome.failure = Some(glasshouse::memory::extract::ExtractionFailure::Model(
        glasshouse::memory::ModelError::Unavailable,
    ));
    let notice = crate::commands::memory_extraction::lost_extraction_notice("stop", Some(&outcome))
        .expect("an unavailable model is a lost memory");
    assert!(
        notice.contains(VAR),
        "the notice must name the variable: {notice}"
    );
    assert!(
        notice.contains("provider `withheld`"),
        "the notice must name the provider: {notice}"
    );
    assert!(
        notice.contains("map line 488") && notice.contains("hooks it runs"),
        "the notice must say what Glasshouse withholds and from whom: {notice}"
    );
    assert!(
        notice.contains(&glasshouse::integrations::store_credential_instruction(VAR)),
        "the notice must give the store instruction: {notice}"
    );

    // A provider whose credential resolves — through either of its two
    // variables — is not withheld, so nothing is said and no value is read.
    let user: UserConfig = toml::from_str(&format!(
        "version = 1\n\n[providers.resolving]\ntemplate = \"openrouter\"\ncredential_env = \
         [\"{VAR}\", \"{SET_VAR}\"]\n"
    ))
    .expect("user config");
    let effective = EffectiveConfig::new(&user, None);
    // SAFETY: a name unique to this test, removed again before any assertion
    // can panic, so no other test observes it.
    unsafe {
        std::env::set_var(SET_VAR, VALUE);
    }
    let withheld =
        crate::commands::shared::withheld_provider_credentials(&effective, &secrets, None);
    unsafe {
        std::env::remove_var(SET_VAR);
    }
    assert!(
        withheld.is_empty(),
        "a resolving credential is not withheld"
    );
    assert!(crate::commands::shared::withheld_credential_notice(&withheld).is_none());
    let untouched = crate::commands::memory_extraction::noting_withheld_credentials(
        Box::new(crate::commands::memory_extraction::NoExtractionModel),
        &withheld,
    );
    assert!(!untouched.describe().contains(VALUE));
    assert!(
        !notice.contains(VALUE),
        "value leaked (withheld from this message)"
    );
}

/// The context-firewall hook's half of the same notice: the configured
/// reducer's provider — named directly or through an entitlement — is
/// reported when its credential is withheld; a `local:` reducer and an
/// absent reducer say nothing. Through `reducer_credential_notice`, the
/// hook's own seam, against the environment store alone.
#[test]
fn the_firewall_reducer_notice_names_the_withheld_variable_and_never_a_value() {
    use glasshouse::config::UserConfig;

    const VAR: &str = "GLASSHOUSE_TEST_REDUCER_WITHHELD_KEY";
    assert!(
        std::env::var_os(VAR).is_none(),
        "test setup: {VAR} is already set"
    );
    let secrets = glasshouse::secret::EnvironmentSecretStore::new();
    let providers = format!(
        "[providers.reducer-provider]\ntemplate = \"openrouter\"\ncredential_env = [\"{VAR}\"]\n"
    );

    // The account a reducer may be named by is the gateway's since the
    // 2026-09-11 ruling; the provider it is behind is still Glasshouse's.
    let gateway = glasshouse::config::GatewayCatalogue::from_toml(
        "[accounts.support]\nprovider = \"reducer-provider\"\n",
    )
    .expect("the gateway's catalogue parses");
    let empty_gateway = glasshouse::config::GatewayCatalogue::default();

    let direct: UserConfig = toml::from_str(&format!(
        "version = 1\n\n{providers}\n[context_firewall]\nreducer = \"reducer-provider\"\n"
    ))
    .expect("user config");
    let notice = crate::commands::context_firewall::reducer_credential_notice(
        &direct,
        None,
        &empty_gateway,
        &secrets,
    )
    .expect("a withheld reducer credential is reported");
    assert!(
        notice.contains(VAR),
        "the notice must name the variable: {notice}"
    );
    assert!(
        notice.contains("provider `reducer-provider`") && notice.contains("map line 488"),
        "the notice must name the provider and the rule: {notice}"
    );
    assert!(
        notice.contains(&glasshouse::integrations::store_credential_instruction(VAR)),
        "the notice must give the store instruction: {notice}"
    );

    let via_entitlement: UserConfig = toml::from_str(&format!(
        "version = 1\n\n{providers}\n[context_firewall]\nreducer = \"support\"\n"
    ))
    .expect("user config");
    let notice = crate::commands::context_firewall::reducer_credential_notice(
        &via_entitlement,
        None,
        &gateway,
        &secrets,
    )
    .expect("an account's provider is resolved to its credential");
    assert!(notice.contains(VAR), "{notice}");

    let local: UserConfig = toml::from_str(&format!(
        "version = 1\n\n{providers}\n[context_firewall]\nreducer = \"local:summarise\"\n"
    ))
    .expect("user config");
    assert!(
        crate::commands::context_firewall::reducer_credential_notice(
            &local,
            None,
            &empty_gateway,
            &secrets
        )
        .is_none(),
        "a local tool has no credential to withhold"
    );
    let none: UserConfig =
        toml::from_str(&format!("version = 1\n\n{providers}")).expect("user config");
    assert!(
        crate::commands::context_firewall::reducer_credential_notice(
            &none,
            None,
            &empty_gateway,
            &secrets
        )
        .is_none(),
        "no reducer, no notice"
    );
}

/// A session with no activity has no memory to have lost, and a warning
/// here would fire on every compaction of a session that had not done
/// anything yet.
///
/// This is the assertion that keeps the notice worth reading: a warning
/// that cries wolf is indistinguishable from one that matters, and the way
/// this one stops being read is by appearing when nothing is wrong.
#[test]
fn a_compaction_with_no_session_activity_is_not_reported_as_a_loss() {
    let mut outcome = recorded_nothing();
    outcome.failure = Some(glasshouse::memory::extract::ExtractionFailure::NothingToExtract);

    assert_eq!(
        crate::commands::memory_extraction::lost_extraction_notice(
            "before_compaction",
            Some(&outcome)
        ),
        None,
        "nothing was extracted because there was nothing to extract; that is not a loss"
    );
}

/// [`run_extraction`] answers [`None`] for its preparation failures and
/// for [`EXTRACTION_BOUND`] expiring, and all of those are losses — a
/// boundary went by and nothing was written.
#[test]
fn an_extraction_that_never_produced_an_outcome_is_reported_as_a_loss() {
    let notice =
        crate::commands::memory_extraction::lost_extraction_notice("before_compaction", None)
            .expect("no outcome at all is a lost memory");
    assert!(
        notice.contains("before_compaction") && notice.contains("recorded nothing"),
        "{notice}"
    );
    assert!(
        notice.contains(
            &crate::commands::memory_extraction::EXTRACTION_BOUND
                .as_secs()
                .to_string()
        ),
        "the notice must name the bound it may have been cut off at: {notice}"
    );
}

/// Dogfooding 2026-09-06, finding 4:
/// [`glasshouse::evaluation::record_memory_extraction`]'s three shapes —
/// ran with no failure, ran with a failure, and no outcome at all (both of
/// its reasons) — with the counts and the failure phrase asserted.
///
/// `rejected` carries a planted credential-shaped value and planted
/// memory-body text: the writer counts rejections (`.len()`), it never
/// renders one, so neither string may reach the row it writes.
#[test]
fn the_extraction_observation_writer_counts_never_renders_a_rejection() {
    use glasshouse::evaluation::{EvaluationKind, EvaluationObservations, ExtractionObservation};

    const PLANTED_CREDENTIAL: &str = "sk-planted-test-credential-4e21";
    const PLANTED_BODY: &str = "the staging database password is hunter2-not-real";

    let fixture = CliFixture::new();

    let mut ran = recorded_nothing();
    ran.model = "test/writer".to_owned();
    ran.recorded = vec![
        glasshouse::memory::MemoryId::new("m1"),
        glasshouse::memory::MemoryId::new("m2"),
    ];
    ran.duplicates = 3;
    ran.speculative = 1;
    ran.rejected = vec![glasshouse::memory::extract::Rejection::Store(format!(
        "{PLANTED_BODY}: credential {PLANTED_CREDENTIAL}"
    ))];
    glasshouse::evaluation::record_memory_extraction(
        &fixture.runtime,
        "sess-ran",
        "task_completed",
        ExtractionObservation::Ran(&ran),
        250,
        glasshouse::evaluation::now_unix(),
    );

    let mut failed = recorded_nothing();
    failed.model = "test/writer".to_owned();
    failed.failure = Some(glasshouse::memory::extract::ExtractionFailure::NothingToExtract);
    glasshouse::evaluation::record_memory_extraction(
        &fixture.runtime,
        "sess-failed",
        "before_compaction",
        ExtractionObservation::Ran(&failed),
        5,
        glasshouse::evaluation::now_unix(),
    );

    glasshouse::evaluation::record_memory_extraction(
        &fixture.runtime,
        "sess-none-bound",
        "git_commit",
        ExtractionObservation::NoOutcome {
            bound_expired: true,
        },
        5000,
        glasshouse::evaluation::now_unix(),
    );
    glasshouse::evaluation::record_memory_extraction(
        &fixture.runtime,
        "sess-none-prep",
        "manual",
        ExtractionObservation::NoOutcome {
            bound_expired: false,
        },
        2,
        glasshouse::evaluation::now_unix(),
    );

    let ledger = EvaluationObservations::open(&fixture.runtime).unwrap();
    let rows = ledger.recent(10).unwrap();
    let extraction_rows: Vec<_> = rows
        .iter()
        .filter(|row| row.kind == EvaluationKind::MemoryExtractionObserved)
        .collect();
    assert_eq!(extraction_rows.len(), 4, "{extraction_rows:?}");

    let ran_row = extraction_rows
        .iter()
        .find(|row| row.session_id.as_deref() == Some("sess-ran"))
        .expect("the ran case must record a row");
    assert_eq!(ran_row.subject.as_deref(), Some("task_completed"));
    let detail = ran_row.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("test/writer"), "{detail}");
    assert!(
        detail.contains("recorded 2, lowered 0, speculative 1, duplicates 3, rejected 1; 250 ms"),
        "{detail}"
    );
    assert!(
        !detail.contains(PLANTED_CREDENTIAL),
        "a rejection's rendered message must never reach the ledger: {detail}"
    );
    assert!(
        !detail.contains(PLANTED_BODY),
        "a rejection's rendered message must never reach the ledger: {detail}"
    );

    let failed_row = extraction_rows
        .iter()
        .find(|row| row.session_id.as_deref() == Some("sess-failed"))
        .expect("the failed case must record a row");
    assert_eq!(failed_row.subject.as_deref(), Some("before_compaction"));
    let detail = failed_row.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("test/writer"), "{detail}");
    assert!(
        detail.contains("no session activity to extract from"),
        "{detail}"
    );
    assert!(detail.contains("5 ms"), "{detail}");

    let none_bound_row = extraction_rows
        .iter()
        .find(|row| row.session_id.as_deref() == Some("sess-none-bound"))
        .expect("the bound-expired case must record a row");
    assert_eq!(none_bound_row.subject.as_deref(), Some("git_commit"));
    let detail = none_bound_row.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("no outcome"), "{detail}");
    assert!(detail.contains("the bound expired"), "{detail}");
    assert!(detail.contains("5000 ms"), "{detail}");

    let none_prep_row = extraction_rows
        .iter()
        .find(|row| row.session_id.as_deref() == Some("sess-none-prep"))
        .expect("the preparation-failed case must record a row");
    assert_eq!(none_prep_row.subject.as_deref(), Some("manual"));
    let detail = none_prep_row.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("no outcome"), "{detail}");
    assert!(detail.contains("preparation failed"), "{detail}");
    assert!(detail.contains("2 ms"), "{detail}");
}

/// A run that stored something and rejected something else lost nothing a
/// person needs to act on, and neither did one that found only
/// duplicates.
///
/// The discriminating half of the case below it: rejections are reported
/// only when *nothing* survived them.
#[test]
fn a_run_that_stored_a_memory_is_silent_even_when_it_also_rejected_one() {
    let mut outcome = recorded_nothing();
    outcome
        .recorded
        .push(glasshouse::memory::MemoryId::new("m1"));
    outcome
        .rejected
        .push(glasshouse::memory::extract::Rejection::Store(
            "a rejected one".to_owned(),
        ));
    assert_eq!(
        crate::commands::memory_extraction::lost_extraction_notice(
            "task_completed",
            Some(&outcome)
        ),
        None
    );

    let mut duplicates_only = recorded_nothing();
    duplicates_only.duplicates = 2;
    assert_eq!(
        crate::commands::memory_extraction::lost_extraction_notice(
            "task_completed",
            Some(&duplicates_only)
        ),
        None,
        "a duplicate is the duplicate check working, not a memory lost"
    );
}

/// And the case that is a loss: the model answered, and nothing it
/// proposed survived the contract.
#[test]
fn a_run_whose_every_memory_was_rejected_is_reported_as_a_loss() {
    let mut outcome = recorded_nothing();
    outcome
        .rejected
        .push(glasshouse::memory::extract::Rejection::Store(
            "the store refused it".to_owned(),
        ));

    let notice = crate::commands::memory_extraction::lost_extraction_notice(
        "before_compaction",
        Some(&outcome),
    )
    .expect("a run that stored none of what it proposed lost every one of them");
    assert!(notice.contains("rejected"), "{notice}");
}

/// The mechanism that stops the payload drain being an open-ended wait —
/// see [`PAYLOAD_DRAIN_BOUND`].
///
/// The drain itself cannot be driven from in here: it reads the *process's*
/// standard input, and a test that redirected that would redirect it for
/// every other test in this binary. What is testable is the bound, and the
/// bound is the whole of the fix. The end-to-end observation is recorded
/// where it was made: with this process's input held open, the six tests
/// that call `report_hook_with` block for ever, on this tree and on its
/// base commit alike.
///
/// Asserts *both* halves. Waiting is not evidence on its own — a version
/// that reported `false` without ever running the work would satisfy the
/// first assertion and be useless, which is what the second one is for.
#[test]
fn work_that_never_finishes_is_abandoned_at_its_bound() {
    let bound = std::time::Duration::from_millis(200);
    let started = std::time::Instant::now();
    let finished = crate::commands::hook::abandon_after(bound, || {
        std::thread::sleep(std::time::Duration::from_secs(30));
    });
    let waited = started.elapsed();

    assert!(
        !finished,
        "work that sleeps for thirty seconds cannot have finished inside a {bound:?} bound"
    );
    assert!(
        waited < std::time::Duration::from_secs(5),
        "the caller waited {waited:?} on a {bound:?} bound, so the bound is not what ended \
         the wait"
    );
    assert!(
        waited >= bound,
        "waiting {waited:?} means the bound was not what ended the wait either"
    );
}

/// The other half: work that does finish inside its bound is waited for
/// and reported as finished, so the bound never becomes an excuse to skip
/// the drain a live harness is in the middle of.
#[test]
fn work_that_finishes_inside_its_bound_is_waited_for() {
    let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&done);
    assert!(
        crate::commands::hook::abandon_after(std::time::Duration::from_secs(30), move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }),
        "work that returns immediately must be reported as finished"
    );
    assert_eq!(
        done.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the work must actually have run"
    );
}

/// The listing's ages, including the case a review flagged: a timestamp in
/// the future. `saturating_sub` saturates at `i64::MIN`, not at zero, so
/// the value really can be negative and the first arm has to absorb it.
#[test]
fn ages_read_sensibly_including_a_clock_that_moved_backwards() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs() as i64;

    assert_eq!(crate::commands::shared::format_age(now), "just now");
    assert_eq!(crate::commands::shared::format_age(now - 30), "just now");
    assert_eq!(crate::commands::shared::format_age(now - 120), "2m ago");
    assert_eq!(crate::commands::shared::format_age(now - 7_200), "2h ago");
    assert_eq!(
        crate::commands::shared::format_age(now - 3 * 86_400),
        "3d ago"
    );

    // A future timestamp must not print a negative age.
    let ahead = crate::commands::shared::format_age(now + 10_000);
    assert_eq!(
        ahead, "just now",
        "a future timestamp must not read as an age"
    );
    assert!(!ahead.contains('-'), "no negative ages: {ahead}");

    // Extremes must not panic or overflow. A row holding a nonsense
    // timestamp cannot come from Glasshouse's own writes — `system_clock`
    // never returns a negative — so the honest contract is only that the
    // output stays finite and non-negative. `i64::MIN` yields an absurdly
    // large age, which is the right kind of wrong: visibly broken rather
    // than plausibly incorrect.
    for extreme in [i64::MIN, i64::MAX, 0] {
        let text = crate::commands::shared::format_age(extreme);
        assert!(!text.is_empty() && !text.contains('-'), "bad age: {text}");
    }
    assert_eq!(
        crate::commands::shared::format_age(i64::MAX),
        "just now",
        "the far future reads as now"
    );
}

/// The header and every row go through `session_row`, so their columns
/// cannot drift apart. Checked here rather than trusted.
#[test]
fn listing_columns_line_up_between_the_header_and_a_row() {
    let header = crate::commands::sessions::session_row(
        "SESSION",
        "NAME",
        "PURPOSE",
        "HARNESS",
        "PROFILE",
        "STATE",
        "ROLE",
        "PRESENTED",
        "LAST",
        crate::commands::sessions::PRESENTED_WIDTH,
    );
    let row = crate::commands::sessions::session_row(
        "abc123",
        "the auth probe",
        "auth",
        "claude-code",
        "native",
        "resumable",
        "orchestrator",
        "embedded",
        "2h ago",
        crate::commands::sessions::PRESENTED_WIDTH,
    );

    let starts = |line: &str| -> Vec<usize> {
        let mut out = vec![0];
        let bytes = line.as_bytes();
        for i in 1..bytes.len() {
            if bytes[i] != b' ' && bytes[i - 1] == b' ' && i >= 2 && bytes[i - 2] == b' ' {
                out.push(i);
            }
        }
        out
    };
    assert_eq!(
        starts(&header),
        starts(&row),
        "columns must start at the same offsets:\n{header}\n{row}"
    );
}

// ---------------------------------------------------------------------
// Phase 21 / 21A — the command surfaces, which is where these
// capabilities become true of a program a person can run rather than of
// a Rust API nothing calls.
// ---------------------------------------------------------------------

/// A bootstrapped project, with its temp directories kept alive.
struct CliFixture {
    _workspace: tempfile::TempDir,
    _data: tempfile::TempDir,
    runtime: Runtime,
}

impl CliFixture {
    fn new() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
        let data = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, workspace.path()).unwrap();
        Self {
            _workspace: workspace,
            _data: data,
            runtime,
        }
    }
}

/// Map line 1139's safety property, at the one branch a black-box test
/// cannot reach.
///
/// `tests/file_aware_memory.rs` proves the hook's response is identical
/// across every recording outcome a real invocation can reach — recorded,
/// no session, not a writing tool, outside the root. It cannot reach a
/// *failed append*, because this binary's own bootstrap opens the project
/// database before any subcommand runs and refuses to start if it cannot,
/// so a database made unwritable never gets as far as the hook.
///
/// In process the branch is reachable, and this is what it must do:
/// return, having logged, propagating nothing. The type system carries
/// the rest — `record_file_touches` returns `()`, so the response written
/// afterwards cannot read anything from it.
///
/// Unix only, and the reason is the injection rather than the behaviour:
/// making a write fail means taking write permission away, and Windows'
/// ACLs do not honour a mode bit. The code under test has no `#[cfg]`.
#[cfg(unix)]
#[test]
fn record_file_touches_never_propagates_a_failure() {
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = CliFixture::new();
    let root = fixture.runtime.project().root().to_path_buf();
    let event = glasshouse::firewall::adapter::PostToolUseEvent {
        tool_name: "Edit".to_owned(),
        tool_input: serde_json::json!({ "file_path": root.join("a.rs") }),
        tool_response: serde_json::json!({ "type": "text", "text": "done" }),
        tool_use_id: "tu".to_owned(),
        session_id: "cc".to_owned(),
    };

    // First, the healthy case, so a later `is_empty` cannot pass because
    // the event was never recordable in the first place.
    crate::commands::context_firewall::record_file_touches(&fixture.runtime, Some("s-1"), &event);
    let log = glasshouse::events::EventLog::open(&fixture.runtime).unwrap();
    assert_eq!(
        log.all().unwrap().len(),
        1,
        "the healthy case must record, or the failure below proves nothing"
    );
    drop(log);

    let state = fixture.runtime.state_dir().to_path_buf();
    let original = std::fs::metadata(&state).unwrap().permissions();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o555)).unwrap();
    let database = fixture.runtime.database_path();
    let mut readonly = std::fs::metadata(&database).unwrap().permissions();
    readonly.set_mode(0o444);
    std::fs::set_permissions(&database, readonly).unwrap();

    // The whole assertion: this returns. It does not panic, and there is
    // no error for a caller to have to handle.
    crate::commands::context_firewall::record_file_touches(&fixture.runtime, Some("s-1"), &event);

    std::fs::set_permissions(&state, original).unwrap();
    let mut writable = std::fs::metadata(&database).unwrap().permissions();
    writable.set_mode(0o644);
    std::fs::set_permissions(&database, writable).unwrap();
    let log = glasshouse::events::EventLog::open(&fixture.runtime).unwrap();
    assert_eq!(
        log.all().unwrap().len(),
        1,
        "the second call really must have failed to write, or this proves nothing"
    );
}

/// Phase 21A's fixed architectural requirement, at the only surface a
/// person reaches: retrieval preserves the authority distinction rather
/// than flattening every remembered statement into equally authoritative
/// text.
///
/// Drives all seven classes rather than a sample, from
/// `MemoryAuthority::ALL`, so an eighth class fails here rather than
/// being quietly unprintable.
#[test]
fn a_memory_search_names_the_authority_class_of_every_result() {
    use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};

    let fixture = CliFixture::new();
    let project = ProjectMemory::open(&fixture.runtime).unwrap();
    let store = project.store();

    for authority in MemoryAuthority::ALL {
        store
            .record(
                NewMemory::new(
                    MemoryKind::Finding,
                    format!("The kestrel deploy is {}.", authority.as_str()),
                )
                .with_authority(Some(*authority)),
            )
            .unwrap();
    }
    // An unclassified memory says so. It must not borrow a neighbour's
    // class, and it must not be indistinguishable from a classified one.
    store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "The kestrel deploy was never classified.",
        ))
        .unwrap();

    let report =
        crate::commands::memory::memory_report(&fixture.runtime, "kestrel", false, 20).unwrap();

    for authority in MemoryAuthority::ALL {
        assert!(
            report.contains(authority.as_str()),
            "`{}` is missing from a search that returned it:\n{report}",
            authority.as_str()
        );
    }
    assert!(
        report.contains("unclassified"),
        "an unclassified memory must say so:\n{report}"
    );
}

/// Phase 21A — a person can promote and demote explicitly, and only a
/// person can reach `invariant` at all.
#[test]
fn a_person_can_promote_a_memory_and_demote_it_again() {
    use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};

    let fixture = CliFixture::new();
    let project = ProjectMemory::open(&fixture.runtime).unwrap();
    let id = project
        .store()
        .record(NewMemory::new(
            MemoryKind::Decision,
            "Sessions are keyed by project, not by directory.",
        ))
        .unwrap()
        .id;

    let promoted =
        crate::commands::memory::memory_promote(&fixture.runtime, id.as_str(), "invariant")
            .unwrap();
    assert!(promoted.contains("invariant"), "{promoted}");
    assert_eq!(
        project.store().get(&id).unwrap().unwrap().authority,
        Some(glasshouse::memory::MemoryAuthority::Invariant)
    );

    // Demotion is never refused: 21A's concern is memories becoming
    // binding without anyone deciding they should.
    let demoted =
        crate::commands::memory::memory_promote(&fixture.runtime, id.as_str(), "preference")
            .unwrap();
    assert!(demoted.contains("preference"), "{demoted}");

    let cleared =
        crate::commands::memory::memory_promote(&fixture.runtime, id.as_str(), "unclassified")
            .unwrap();
    assert!(cleared.contains("unclassified"), "{cleared}");
    assert_eq!(project.store().get(&id).unwrap().unwrap().authority, None);

    // A class that does not exist is refused by name rather than
    // silently storing nothing.
    let refused = crate::commands::memory::memory_promote(
        &fixture.runtime,
        id.as_str(),
        "extremely-important",
    );
    assert!(refused.is_err());
}

/// Phase 21F lines 937/938, acceptance test 5: a challenged memory is
/// not returned as settled, and its reason is recorded and readable.
/// Enters through `memory_challenge` and `memory_report`, exactly what
/// `glasshouse memory challenge` and `glasshouse memory search` run.
#[test]
fn a_challenged_memory_drops_out_of_current_search_and_names_why() {
    use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};

    let fixture = CliFixture::new();
    let project = ProjectMemory::open(&fixture.runtime).unwrap();
    let id = project
        .store()
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "The egret worker retries indefinitely.",
            )
            .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap()
        .id;

    const BODY: &str = "retries indefinitely";

    let before =
        crate::commands::memory::memory_report(&fixture.runtime, "egret", false, 10).unwrap();
    assert!(before.contains(BODY), "{before}");

    let challenged = crate::commands::memory::memory_challenge(
        &fixture.runtime,
        id.as_str(),
        "production_incident",
    )
    .unwrap();
    assert!(challenged.contains("needs_review"), "{challenged}");
    assert!(challenged.contains("production_incident"), "{challenged}");

    // No longer returned as current, settled knowledge.
    let after =
        crate::commands::memory::memory_report(&fixture.runtime, "egret", false, 10).unwrap();
    assert!(
        !after.contains(BODY),
        "a challenged memory must not appear in a default search:\n{after}"
    );

    // Still reachable as history, with the reason recorded and readable.
    let history =
        crate::commands::memory::memory_report(&fixture.runtime, "egret", true, 10).unwrap();
    assert!(history.contains(BODY), "{history}");
    assert!(history.contains("needs_review"), "{history}");
    assert!(
        history.contains("production_incident"),
        "the challenge reason must be readable in the history report:\n{history}"
    );

    // A reason that is not one of the six is refused, and nothing is
    // written.
    let refused = crate::commands::memory::memory_challenge(&fixture.runtime, id.as_str(), "vibes");
    assert!(refused.is_err());
}

/// Phase 21G line 949, acceptance test 1 — the round trip the binary
/// currently promises and cannot deliver: challenging a memory moves it
/// to `needs-review` and out of every default search, and until this
/// batch nothing could move it back. Enters through `memory_challenge`,
/// `memory_report` and `memory_revalidate`, exactly what `glasshouse
/// memory challenge`, `glasshouse memory search` and `glasshouse memory
/// revalidate` run.
#[test]
fn a_challenged_memory_is_reaffirmed_back_into_default_search_with_a_fresh_validation() {
    use glasshouse::memory::{MemoryAuthority, MemoryKind, MemoryStatus, NewMemory, ProjectMemory};

    let fixture = CliFixture::new();
    let project = ProjectMemory::open(&fixture.runtime).unwrap();
    let id = project
        .store()
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "The heron worker retries at most three times.",
            )
            .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap()
        .id;

    const BODY: &str = "retries at most three times";

    assert!(
        project
            .store()
            .get(&id)
            .unwrap()
            .unwrap()
            .last_validated_at
            .is_none(),
        "a freshly recorded memory has never been validated"
    );

    let before =
        crate::commands::memory::memory_report(&fixture.runtime, "heron", false, 10).unwrap();
    assert!(before.contains(BODY), "{before}");

    crate::commands::memory::memory_challenge(&fixture.runtime, id.as_str(), "project_state")
        .unwrap();
    let after_challenge =
        crate::commands::memory::memory_report(&fixture.runtime, "heron", false, 10).unwrap();
    assert!(
        !after_challenge.contains(BODY),
        "a challenged memory must drop out of a default search:\n{after_challenge}"
    );

    let revalidated = crate::commands::memory::memory_revalidate(
        &fixture.runtime,
        id.as_str(),
        "reaffirmed",
        None,
        None,
        false,
    )
    .unwrap();
    assert!(revalidated.contains("active"), "{revalidated}");

    // Back in a default search.
    let after_revalidate =
        crate::commands::memory::memory_report(&fixture.runtime, "heron", false, 10).unwrap();
    assert!(
        after_revalidate.contains(BODY),
        "a reaffirmed memory must return to a default search:\n{after_revalidate}"
    );

    // ... with a fresh validation timestamp and the matching status.
    let record = project.store().get(&id).unwrap().unwrap();
    assert_eq!(record.status, MemoryStatus::Active);
    assert!(
        record.last_validated_at.is_some(),
        "reaffirming must record a validation timestamp"
    );

    // An outcome that is not one of the four is refused.
    let refused = crate::commands::memory::memory_revalidate(
        &fixture.runtime,
        id.as_str(),
        "vibes",
        None,
        None,
        false,
    );
    assert!(refused.is_err());
}

/// Phase 21G line 950, acceptance test 4 — `--list` is bounded to
/// `NeedsReview` memories and touches nothing. Enters through
/// `memory_revalidate_list`, exactly what `glasshouse memory revalidate
/// --list` runs. Wires `MemoryStore::with_status`, which had no
/// production caller before this.
#[test]
fn revalidate_list_is_bounded_to_needs_review_memories_and_touches_nothing() {
    use glasshouse::memory::{MemoryKind, MemoryStatus, NewMemory, ProjectMemory, ReviewReason};

    let fixture = CliFixture::new();
    let project = ProjectMemory::open(&fixture.runtime).unwrap();
    let store = project.store();

    let mut needing_review = Vec::new();
    for i in 0..3 {
        let record = store
            .record(NewMemory::new(
                MemoryKind::Finding,
                format!("egret finding {i}"),
            ))
            .unwrap();
        store
            .mark_for_review(&record.id, ReviewReason::ProjectState)
            .unwrap();
        needing_review.push(record.id);
    }
    // An active memory and an invalidated one: neither is waiting for
    // review, and neither may ever appear in the listing.
    store
        .record(NewMemory::new(MemoryKind::Finding, "an active finding"))
        .unwrap();
    let invalidated = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "an invalidated finding",
        ))
        .unwrap();
    store
        .set_status(&invalidated.id, MemoryStatus::Invalidated)
        .unwrap();

    let listing = crate::commands::memory::memory_revalidate_list(&fixture.runtime, 2).unwrap();
    let lines: Vec<&str> = listing.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "the listing must not exceed --limit:\n{listing}"
    );
    for line in &lines {
        assert!(
            needing_review
                .iter()
                .any(|id| line.starts_with(id.as_str())),
            "every listed entry must be one of the needs-review memories: {line}"
        );
    }

    // Nothing was touched: every needs-review memory is still
    // needs-review, and the untouched active/invalidated rows are
    // unchanged.
    for id in &needing_review {
        assert_eq!(
            store.get(id).unwrap().unwrap().status,
            MemoryStatus::NeedsReview
        );
    }
    assert_eq!(
        store.get(&invalidated.id).unwrap().unwrap().status,
        MemoryStatus::Invalidated
    );
}

/// Phase 21F line 936, on the CLI's own text report — the machine door's
/// half is `tests/memory_query_api.rs`, and this is the surface a person
/// reads. A binding memory's validity and invalidation conditions are
/// printed; a non-binding one's are not, even when the row carries them.
#[test]
fn the_report_prints_validity_and_invalidation_conditions_only_for_binding_memories() {
    use glasshouse::memory::{
        DecisionProvenance, MemoryAuthority, MemoryKind, NewMemory, ProjectMemory,
    };

    let fixture = CliFixture::new();
    let project = ProjectMemory::open(&fixture.runtime).unwrap();
    let store = project.store();
    store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "The kite export must be single-writer.",
            )
            .with_authority(Some(MemoryAuthority::Constraint))
            .with_provenance(DecisionProvenance {
                rationale: Some("a partial file broke a downstream job".to_owned()),
                ..DecisionProvenance::default()
            })
            .with_validity_conditions(Some("the export stays single-writer"))
            .with_invalidation_conditions(Some("the export gains concurrent writers")),
        )
        .unwrap();
    store
        .record(
            NewMemory::new(
                MemoryKind::Finding,
                "The kite export could maybe batch writes.",
            )
            .with_authority(Some(MemoryAuthority::Idea))
            .with_validity_conditions(Some("nobody has decided this yet")),
        )
        .unwrap();

    let report =
        crate::commands::memory::memory_report(&fixture.runtime, "kite", false, 10).unwrap();
    assert!(
        report.contains("valid while  the export stays single-writer"),
        "{report}"
    );
    assert!(
        report.contains("invalid if   the export gains concurrent writers"),
        "{report}"
    );
    assert!(
        !report.contains("nobody has decided this yet"),
        "a non-binding memory's validity condition must not be printed:\n{report}"
    );
}

// ---------------------------------------------------------------------
// Phase 21 — extraction after task completion, and the promise that its
// failure never costs the coding session anything.
//
// These drive `report_hook_with`, which *is* `glasshouse hook`: the same
// session lookup, the same translation, the same event record, the same
// state change. Only the model is supplied, because the model is the one
// piece Phase 39 owns and nothing has built.
// ---------------------------------------------------------------------

/// An extraction model whose reply is fixed, and which records that it
/// was asked.
struct Canned {
    reply: String,
    asked: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl glasshouse::memory::ExtractionModel for Canned {
    fn describe(&self) -> String {
        "test/canned".to_owned()
    }
    fn complete(
        &self,
        _prompt: &glasshouse::memory::extract::Prompt,
    ) -> Result<String, glasshouse::memory::ModelError> {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.reply.clone())
    }
}

/// A model that does the one thing a support job must never be able to do
/// to the session that triggered it.
struct Hostile(HostileKind);

enum HostileKind {
    Refuses,
    Panics,
    Hangs,
}

impl glasshouse::memory::ExtractionModel for Hostile {
    fn describe(&self) -> String {
        "test/hostile".to_owned()
    }
    fn complete(
        &self,
        _prompt: &glasshouse::memory::extract::Prompt,
    ) -> Result<String, glasshouse::memory::ModelError> {
        match self.0 {
            HostileKind::Refuses => Err(glasshouse::memory::ModelError::Refused),
            HostileKind::Panics => panic!("the extraction model fell over"),
            // Far longer than `EXTRACTION_BOUND`, so the test measures the
            // bound rather than the sleep.
            HostileKind::Hangs => {
                std::thread::sleep(std::time::Duration::from_secs(60));
                Ok(String::new())
            }
        }
    }
}

const ONE_FINDING: &str = r#"{"memories":[{"kind":"finding","authority":"constraint",
     "disposition":"accepted","support":"established","confidence":"certain",
     "rationale":"the hook process is the only thing that sees a turn end",
     "project_phase":"alpha",
     "body":"Extraction after a task runs in the hook process."}]}"#;

/// A session this project has recorded, ready to receive a harness event.
fn recorded_session(runtime: &Runtime) -> glasshouse::session::SessionId {
    use glasshouse::session::NewSession;

    let sessions = ProjectSessions::open(runtime).unwrap();
    let record = sessions
        .store()
        .create(NewSession::embedded("claude-code"))
        .unwrap();
    record.id
}

fn stored_memories(runtime: &Runtime) -> Vec<glasshouse::memory::MemoryRecord> {
    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::search::SearchScope;

    ProjectMemory::open(runtime)
        .unwrap()
        .store()
        .search("extraction", SearchScope::Current, 10)
        .unwrap()
}

/// Line: *"Allow memory extraction to run after task completion."*
///
/// The trigger is a harness saying `Stop`, which is the only report
/// `session::lifecycle::event_for` turns into a completed turn. What the
/// stored memory carries is the other half of the evidence: the session
/// it came from, and the **range of this project's event log** the
/// extractor was actually shown — Phase 21's *"store the originating
/// session and event references so extracted memory retains
/// provenance."*
#[test]
fn a_completed_task_runs_extraction_and_the_memory_names_where_it_came_from() {
    let fixture = CliFixture::new();
    let id = recorded_session(&fixture.runtime);
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    {
        let asked = std::sync::Arc::clone(&asked);
        crate::commands::hook::report_hook_with(&fixture.runtime, id.as_str(), "Stop", move |_| {
            Box::new(Canned {
                reply: ONE_FINDING.to_owned(),
                asked: std::sync::Arc::clone(&asked),
            })
        });
    }

    assert_eq!(
        asked.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a completed task must ask the extraction model exactly once"
    );

    let stored = stored_memories(&fixture.runtime);
    assert_eq!(stored.len(), 1, "the memory reached the project's store");
    assert_eq!(stored[0].source_session_id.as_deref(), Some(id.as_str()));
    let events = stored[0]
        .source_events
        .expect("a memory extracted from the event log names the slice it came from");
    assert!(
        events.first >= 1 && events.last >= events.first,
        "the provenance range must name real log positions, got {events}"
    );
    assert_eq!(
        stored[0].provenance.project_phase,
        Some(glasshouse::memory::ProjectPhase::Alpha)
    );
}

/// Dogfooding 2026-09-06, finding 4: `hook_extraction`'s durable trace of
/// what a completed task's extraction did survives the process that ran it
/// — a `MemoryExtractionObserved` row in the evaluation ledger, not only the
/// stderr line nothing reads back. Through `report_hook_with`, the same
/// production seam the test above uses.
///
/// The planted memory names a credential-shaped value on purpose: the
/// existing extraction contract refuses to store it at all
/// (`Refusal::Credential`), which is what turns this into a *rejected*
/// count rather than a *recorded* one — and proves the row still never
/// carries the value even when the memory it describes was refused for
/// carrying one.
///
/// Deleting the `record_memory_extraction` call from `hook_extraction`
/// kills this test.
#[test]
fn a_completed_task_running_through_the_hook_records_its_extraction_outcome() {
    let fixture = CliFixture::new();
    let id = recorded_session(&fixture.runtime);
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    const PLANTED_CREDENTIAL: &str = "sk-planted-hook-test-credential-7c2b";
    let reply = format!(
        r#"{{"memories":[{{"kind":"finding","authority":"constraint",
     "disposition":"accepted","support":"established","confidence":"certain",
     "rationale":"the hook process is the only thing that sees a turn end",
     "project_phase":"alpha",
     "body":"The deploy credential is {PLANTED_CREDENTIAL}."}}]}}"#
    );

    {
        let asked = std::sync::Arc::clone(&asked);
        crate::commands::hook::report_hook_with(&fixture.runtime, id.as_str(), "Stop", move |_| {
            Box::new(Canned {
                reply: reply.clone(),
                asked: std::sync::Arc::clone(&asked),
            })
        });
    }
    assert_eq!(
        asked.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a completed task must ask the extraction model exactly once"
    );
    assert!(
        stored_memories(&fixture.runtime).is_empty(),
        "a credential-shaped body must be refused by the extraction contract, not stored"
    );

    let ledger = glasshouse::evaluation::EvaluationObservations::open(&fixture.runtime).unwrap();
    let rows = ledger.recent(10).unwrap();
    let row = rows
        .iter()
        .find(|row| row.kind == glasshouse::evaluation::EvaluationKind::MemoryExtractionObserved)
        .expect("a completed task must record its extraction outcome in the evaluation ledger");
    assert_eq!(row.subject.as_deref(), Some("task_completed"));
    assert_eq!(row.session_id.as_deref(), Some(id.as_str()));
    let detail = row.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("test/canned"), "{detail}");
    assert!(
        detail.contains("recorded 0, lowered 0, speculative 0, duplicates 0, rejected 1"),
        "{detail}"
    );
    assert!(detail.contains(" ms"), "{detail}");
    assert!(
        !detail.contains(PLANTED_CREDENTIAL),
        "a memory's own body must never reach the evaluation ledger, refused or not: {detail}"
    );
    assert!(
        !detail.contains("deploy credential"),
        "the memory's rationale/body text must never reach the ledger: {detail}"
    );
}

/// The trigger is *task completion*, not *any harness event*.
///
/// `StopFailure` is a turn that ended badly and `UserPromptSubmit` is a
/// turn starting; neither is a completed task, and extraction that ran on
/// them would be extraction running on a schedule rather than on the map's
/// line. This is the discriminating half of the test above — without it,
/// "runs after task completion" would be satisfied by "runs always".
#[test]
fn an_event_that_is_not_a_completed_task_asks_no_model() {
    for event in ["StopFailure", "UserPromptSubmit", "PermissionRequest"] {
        let fixture = CliFixture::new();
        let id = recorded_session(&fixture.runtime);
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        {
            let asked = std::sync::Arc::clone(&asked);
            crate::commands::hook::report_hook_with(
                &fixture.runtime,
                id.as_str(),
                event,
                move |_| {
                    Box::new(Canned {
                        reply: ONE_FINDING.to_owned(),
                        asked: std::sync::Arc::clone(&asked),
                    })
                },
            );
        }

        assert_eq!(
            asked.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "`{event}` is not a completed task and must not run extraction"
        );
        assert!(stored_memories(&fixture.runtime).is_empty());
    }
}

/// Line: *"Keep memory-extraction failure non-fatal to the coding
/// session."*
///
/// This is the line's real setting: a `glasshouse hook` process running
/// **inside the user's session**, where Claude Code treats a non-zero
/// exit as a veto on the turn. Three failures a support job can produce,
/// and after each one the session must be exactly as it would have been
/// with no extraction at all — the event recorded, the lifecycle applied,
/// nothing propagated.
///
/// Note what is *not* asserted: that extraction succeeded. It did not,
/// three times. That is the point.
#[test]
fn a_failing_extraction_model_costs_the_coding_session_nothing() {
    use glasshouse::session::SessionLifecycle;

    for kind in [HostileKind::Refuses, HostileKind::Panics] {
        let fixture = CliFixture::new();
        let id = recorded_session(&fixture.runtime);

        crate::commands::hook::report_hook_with(&fixture.runtime, id.as_str(), "Stop", move |_| {
            Box::new(Hostile(match kind {
                HostileKind::Refuses => HostileKind::Refuses,
                HostileKind::Panics => HostileKind::Panics,
                HostileKind::Hangs => HostileKind::Hangs,
            }))
        });

        // The session's own bookkeeping happened anyway.
        let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
        let record = sessions.store().get(&id).unwrap().unwrap();
        assert_eq!(
            record.lifecycle,
            SessionLifecycle::Idle,
            "a failed extraction must not stop the turn being recorded as ended"
        );

        let log = EventLog::open(&fixture.runtime).unwrap();
        assert_eq!(
            log.for_session(&id).unwrap().len(),
            1,
            "the lifecycle event is recorded whatever extraction did"
        );
        assert!(stored_memories(&fixture.runtime).is_empty());
    }
}

/// The fourth failure, and the only one a `Result` could never have
/// absorbed: a model that never answers at all.
///
/// `EXTRACTION_BOUND` is what stands between a hung provider and a user
/// whose turn will not finish. The model here sleeps for a minute; the
/// hook must be long gone.
#[test]
fn an_extraction_model_that_never_answers_is_abandoned_at_its_bound() {
    use glasshouse::session::SessionLifecycle;

    let fixture = CliFixture::new();
    let id = recorded_session(&fixture.runtime);

    let started = std::time::Instant::now();
    crate::commands::hook::report_hook_with(&fixture.runtime, id.as_str(), "Stop", |_| {
        Box::new(Hostile(HostileKind::Hangs))
    });
    let waited = started.elapsed();

    assert!(
        waited < crate::commands::memory_extraction::EXTRACTION_BOUND * 3,
        "the hook waited {waited:?} on a model that sleeps for a minute;              the bound is {:?}",
        crate::commands::memory_extraction::EXTRACTION_BOUND
    );
    assert!(
        waited >= crate::commands::memory_extraction::EXTRACTION_BOUND,
        "waiting {waited:?} means the bound was not what ended the wait"
    );

    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    assert_eq!(
        sessions.store().get(&id).unwrap().unwrap().lifecycle,
        SessionLifecycle::Idle
    );
    assert!(stored_memories(&fixture.runtime).is_empty());
}

/// What the shipped binary does today, stated as a test so it cannot
/// quietly become something else.
///
/// Phase 21 has two lines here and only one of them is built: the
/// trigger is real and the **model is Phase 39's**. So a real
/// `glasshouse hook` runs extraction after every completed task and
/// reports that no model is available — and `NoExtractionModel::describe`
/// says so in words, for the same reason `glasshouse memory extract`
/// prints `no model was called`: an evaluation must never be mistakeable
/// later for evidence a model did the work.
#[test]
fn the_shipped_binary_runs_extraction_after_a_task_and_reports_that_it_has_no_model() {
    use glasshouse::memory::ExtractionModel as _;

    let fixture = CliFixture::new();
    let id = recorded_session(&fixture.runtime);

    // The production entry point, with production's own model.
    crate::commands::hook::report_hook(&fixture.runtime, id.as_str(), "Stop");

    assert!(stored_memories(&fixture.runtime).is_empty());
    let described = crate::commands::memory_extraction::NoExtractionModel.describe();
    assert!(
        described.contains("none configured"),
        "the production model must name itself as absent: {described}"
    );
    assert_eq!(
        glasshouse::memory::ModelError::Unavailable.to_string(),
        "no extraction model is available"
    );
}

/// `report_hook` — not `report_hook_with`, which every fixture above
/// supplies its own fake model to — must itself ask
/// `disposable_extraction_model` for its model, and never
/// `NoExtractionModel` directly. A source scan, in the same style as
/// `hook_handler_source`'s: the alternative is a runtime assertion that
/// needs the model to actually run, and `report_hook`'s own body is two
/// lines specifically so that reading it settles the question.
#[test]
fn report_hook_routes_extraction_through_disposable_extraction_model() {
    let full = all_production_code();
    let start = full
        .find("fn report_hook(runtime: &Runtime, session: &str, event: &str) {")
        .expect("report_hook must exist in this file");
    let after_start = &full[start..];
    let end = after_start
        .find("\n}")
        .expect("report_hook must have a top-level closing brace");
    let body = strip_comments(&after_start[..end]);

    assert!(
        body.contains("disposable_extraction_model"),
        "report_hook must ask disposable_extraction_model for its model: {body}"
    );
    assert!(
        !body.contains("NoExtractionModel"),
        "report_hook must not name NoExtractionModel itself — that is \
         disposable_extraction_model's own fallback for a configuration it could not read: \
         {body}"
    );
}

/// Phase 9I line 532, at the real production entry point — this file's
/// own `gateway_upstream` wrapper, not `glasshouse::profile::gateway_upstream`
/// directly. A provider the user marked a free model on, written to disk
/// exactly as Settings would write it, backs the gateway at `Cost::Free`.
#[test]
fn a_configured_free_model_backs_the_gateway_at_no_cost() {
    const VAR: &str = "GLASSHOUSE_TEST_ONLY_WIRE_DISPOSABLE_GATEWAY_FREE_KEY";
    // SAFETY: `VAR` is unique to this test and removed again below.
    unsafe {
        std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
    }

    let fixture = CliFixture::new();
    let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
    let mut provider = glasshouse::config::ProviderConfig::new("anthropic-compatible");
    provider.set_base_url(Some("https://example.invalid/api".to_owned()));
    provider.set_credential_env(vec![VAR.to_owned()]);
    provider.set_free_models(vec!["a-free-model".to_owned()]);
    user.providers_mut()
        .set("wire-disposable-gateway-provider", provider);
    user.save(fixture.runtime.paths()).unwrap();

    let user = UserConfig::load(fixture.runtime.paths()).unwrap();
    let project = config::load_project_config(fixture.runtime.project()).unwrap();
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();

    let upstream = crate::commands::resume::gateway_upstream(
        &user,
        project.as_ref(),
        &effective,
        &secrets,
        None,
        fixture.runtime.paths(),
    )
    .unwrap();
    let rendered = format!("{upstream:?}");

    unsafe {
        std::env::remove_var(VAR);
    }

    assert!(
        rendered.contains("cost: \"free\""),
        "a provider the user marked a free model on must back the gateway at no cost: \
         {rendered}"
    );
}

/// Line: *"Treat a decision with missing rationale and missing
/// assumptions as lower-confidence than a well-proven decision of the
/// same authority class"*, at the surface a person reads.
///
/// The ranking is the behaviour — `memory::search::demote_thin_decisions`
/// puts such a decision behind a better-proven one of its own class, and
/// four tests in `memory_provenance.rs` pin each clause of that. This is
/// the other half, and it is not decoration: a reader handed a reordering
/// with no reason for it has been given a mystery. The word `unclassified`
/// earned its place in this output for the same reason.
///
/// It also pins the negative case, which is where a label goes wrong: a
/// decision that recorded *why* must not be marked, and neither must a
/// finding that recorded nothing — the map's line is about decisions.
#[test]
fn a_search_marks_a_thinly_provenanced_decision_and_shows_the_provenance_of_the_others() {
    use glasshouse::memory::{
        DecisionProvenance, MemoryAuthority, MemoryKind, NewMemory, ProjectMemory, ProjectPhase,
    };

    let fixture = CliFixture::new();
    let project = ProjectMemory::open(&fixture.runtime).unwrap();
    let store = project.store();

    store
        .record(
            NewMemory::new(MemoryKind::Decision, "Kestrel runs on one instance.")
                .with_subject(Some("kestrel topology"))
                .with_authority(Some(MemoryAuthority::Decision))
                .with_provenance(DecisionProvenance {
                    rationale: Some("the deploy target has one machine".to_owned()),
                    project_phase: Some(ProjectPhase::Beta),
                    operational_assumptions: Some("single instance, no daemon".to_owned()),
                    ..DecisionProvenance::default()
                }),
        )
        .unwrap();
    store
        .record(
            NewMemory::new(MemoryKind::Decision, "Kestrel logs to stderr.")
                .with_subject(Some("kestrel logging"))
                .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();
    store
        .record(
            NewMemory::new(MemoryKind::Finding, "Kestrel starts in under a second.")
                .with_subject(Some("kestrel startup"))
                .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();

    let report =
        crate::commands::memory::memory_report(&fixture.runtime, "kestrel", false, 10).unwrap();

    // The well-proven decision shows its reasoning, labelled.
    assert!(report.contains("phase      beta"), "{report}");
    assert!(
        report.contains("why        the deploy target has one machine"),
        "{report}"
    );
    assert!(
        report.contains("ops        single instance, no daemon"),
        "{report}"
    );

    // Exactly one line carries the marker, and it is the bare decision's.
    let marked: Vec<&str> = report
        .lines()
        .filter(|line| line.contains("lower-confidence"))
        .collect();
    assert_eq!(marked.len(), 1, "expected one marked line in:\n{report}");
    assert!(marked[0].contains("kestrel logging"), "{}", marked[0]);
}

/// Line: *"Store the originating session and event references so
/// extracted memory retains provenance"* — at the surface a person
/// reaches, which is what `glasshouse memory extract --from-events`
/// exists for.
///
/// The file-fed form of the same command cannot produce this: activity
/// read out of a file has no position in the project's log to name.
#[test]
fn extracting_from_a_sessions_events_records_the_slice_of_the_log_it_read() {
    let fixture = CliFixture::new();
    let id = recorded_session(&fixture.runtime);

    // Give the session a history, the way a session gets one: through
    // the same hook path a harness drives.
    crate::commands::hook::report_hook(&fixture.runtime, id.as_str(), "UserPromptSubmit");
    crate::commands::hook::report_hook(&fixture.runtime, id.as_str(), "Stop");

    let dir = tempfile::tempdir().unwrap();
    let reply = dir.path().join("reply.json");
    std::fs::write(&reply, ONE_FINDING).unwrap();

    let report =
        crate::commands::memory::memory_extract(&fixture.runtime, id.as_str(), None, true, &reply)
            .unwrap();

    assert!(report.contains("recorded events for session"), "{report}");
    assert!(report.contains("provenance: event"), "{report}");
    assert!(report.contains("stored 1"), "{report}");

    let stored = stored_memories(&fixture.runtime);
    assert_eq!(stored.len(), 1);
    let events = stored[0].source_events.expect("an event range");
    assert_eq!(events.first, 1);
    assert_eq!(events.last, 2, "both recorded events reached the model");
}

/// Phase 21 — extraction runs manually, for debugging and evaluation.
///
/// The model half is supplied from a file, which is what makes this
/// runnable before Phase 39 exists. Everything else is the production
/// path, and the assertions below are on that: the reply is validated,
/// classified conservatively, screened and stored.
#[test]
fn a_manual_extraction_runs_the_whole_pipeline_and_says_no_model_was_called() {
    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::search::SearchScope;

    let fixture = CliFixture::new();
    let dir = tempfile::tempdir().unwrap();
    let activity = dir.path().join("activity.txt");
    let reply = dir.path().join("reply.json");
    std::fs::write(&activity, "the kestrel migration ran twice\n").unwrap();
    std::fs::write(
        &reply,
        r#"{"memories":[{"kind":"finding","authority":"constraint",
             "disposition":"accepted","support":"established",
             "confidence":"certain",
             "rationale":"the runner resumes from MAX(version)",
             "body":"A migration rollback must delete a contiguous range."}]}"#,
    )
    .unwrap();

    let report = crate::commands::memory::memory_extract(
        &fixture.runtime,
        "s-1",
        Some(&activity),
        false,
        &reply,
    )
    .unwrap();

    assert!(report.contains("stored 1"), "{report}");
    // The output must never let an evaluation run be mistaken later for
    // evidence that a model performed extraction.
    assert!(
        report.contains("no model was called"),
        "the run must say a model was not called:\n{report}"
    );

    let stored = ProjectMemory::open(&fixture.runtime)
        .unwrap()
        .store()
        .search("migration", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(stored.len(), 1, "the memory reached the real store");
    assert_eq!(stored[0].source_session_id.as_deref(), Some("s-1"));
}

/// Map line 779, at the surface a person actually runs: `glasshouse
/// memory extract` in a real Git repository records the commit the
/// project was standing at, resolved with the same
/// `GitPosition::detect` reading that a checkpoint uses. The fixture
/// hand-writes `HEAD`/`refs` the way `checkpoint/git.rs`'s own tests do,
/// rather than shelling out to `git`, and a sanity assertion proves that
/// setup actually produces the commit before the extraction runs at all.
#[test]
fn manual_extraction_in_a_git_repository_records_the_head_commit() {
    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::search::SearchScope;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    let fixture = CliFixture::new();
    let git_dir = fixture._workspace.path().join(".git");
    std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
    std::fs::write(git_dir.join("refs/heads/main"), format!("{COMMIT}\n")).unwrap();
    assert_eq!(
        GitPosition::detect(fixture.runtime.project().root()).map(|position| position.commit),
        Some(COMMIT.to_owned()),
        "fixture setup must produce the commit this test checks for"
    );

    let dir = tempfile::tempdir().unwrap();
    let activity = dir.path().join("activity.txt");
    let reply = dir.path().join("reply.json");
    std::fs::write(&activity, "we settled on a checkpoint format\n").unwrap();
    std::fs::write(
        &reply,
        r#"{"memories":[{"kind":"finding","authority":"historical",
             "disposition":"accepted","support":"established",
             "confidence":"certain",
             "body":"A checkpoint fixture proved the commit round-trips."}]}"#,
    )
    .unwrap();

    let report = crate::commands::memory::memory_extract(
        &fixture.runtime,
        "s-1",
        Some(&activity),
        false,
        &reply,
    )
    .unwrap();
    assert!(report.contains("stored 1"), "{report}");

    let stored = ProjectMemory::open(&fixture.runtime)
        .unwrap()
        .store()
        .search("round-trips", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(stored.len(), 1, "the memory reached the real store");
    assert_eq!(
        stored[0].source_commit.as_deref(),
        Some(COMMIT),
        "the stored memory must carry the repository's head commit"
    );
}

/// The other half of map line 779: a project that is not a Git
/// repository still extracts normally, with no commit recorded rather
/// than an error. `CliFixture` gives an empty `.git` directory with no
/// `HEAD` — the "unreadable HEAD" case `GitPosition::detect` folds into
/// the same `None` as "no repository at all".
#[test]
fn manual_extraction_outside_a_repository_stores_no_commit_and_does_not_error() {
    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::search::SearchScope;

    let fixture = CliFixture::new();
    assert_eq!(
        GitPosition::detect(fixture.runtime.project().root()),
        None,
        "fixture setup sanity: an empty .git directory has no readable HEAD"
    );

    let dir = tempfile::tempdir().unwrap();
    let activity = dir.path().join("activity.txt");
    let reply = dir.path().join("reply.json");
    std::fs::write(&activity, "we settled on a checkpoint format\n").unwrap();
    std::fs::write(
        &reply,
        r#"{"memories":[{"kind":"finding","authority":"historical",
             "disposition":"accepted","support":"established",
             "confidence":"certain",
             "body":"A non-repository extraction stores no commit."}]}"#,
    )
    .unwrap();

    let report = crate::commands::memory::memory_extract(
        &fixture.runtime,
        "s-1",
        Some(&activity),
        false,
        &reply,
    );
    assert!(
        report.is_ok(),
        "extraction outside a repository must not error: {report:?}"
    );
    assert!(report.unwrap().contains("stored 1"));

    let stored = ProjectMemory::open(&fixture.runtime)
        .unwrap()
        .store()
        .search("non-repository", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(stored.len(), 1, "the memory reached the real store");
    assert_eq!(stored[0].source_commit, None);
}

/// Line 1641, exercised at the surface a person actually runs. Against
/// `checkpoint_command` itself, not a hand-built `Handoff` — a
/// `skip-state-update` mutation that quietly replaced
/// `binding_memory_lines(runtime)` with `Vec::new()` would be invisible to
/// any test that only exercises `Checkpoint`/`Handoff` directly, because
/// those never call `ProjectMemory` at all. This is the caller §35 asks
/// for: the one the shipped binary actually reaches.
#[test]
fn checkpoint_save_carries_binding_project_memory_into_the_handoff() {
    use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};

    let fixture = CliFixture::new();
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let session = sessions
        .store()
        .create(NewSession::embedded("claude-code"))
        .unwrap();

    ProjectMemory::open(&fixture.runtime)
        .unwrap()
        .store()
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "never store secrets in a checkpoint",
            )
            .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .unwrap();
    // Present in the project, but never binding — must not leak in.
    ProjectMemory::open(&fixture.runtime)
        .unwrap()
        .store()
        .record(NewMemory::new(
            MemoryKind::Finding,
            "the CI runner is slow on Mondays",
        ))
        .unwrap();

    let command = CheckpointCommand::Save {
        objective: "prove project memory reaches the handoff".to_owned(),
        state: "wiring checkpoint_command to ProjectMemory".to_owned(),
        session: Some(session.id.as_str().to_owned()),
        decisions: Vec::new(),
        failed_approaches: Vec::new(),
        files: Vec::new(),
        tests: None,
        next_actions: Vec::new(),
    };
    let status =
        crate::commands::checkpoint::checkpoint_command(&fixture.runtime, &command).unwrap();
    assert_eq!(status, ExitCode::SUCCESS);

    let checkpoints = ProjectCheckpoints::open(&fixture.runtime).unwrap();
    let stored = checkpoints.store().list().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].checkpoint.handoff.memory,
        vec!["never store secrets in a checkpoint".to_owned()],
        "the binding memory must reach the checkpoint's handoff, and the \
         unclassified one must not"
    );
    assert!(
        stored[0]
            .checkpoint
            .bootstrap_prompt()
            .contains("RELEVANT MEMORY"),
        "the bootstrap prompt must carry it forward"
    );
}

/// A checkpoint with no memory section is strictly better than a
/// checkpoint that never happened: `binding_memory_lines` degrades to an
/// empty list rather than propagating, even when the project's memory
/// database cannot be opened at all. Unix-only because the failure is
/// forced through a permission bit; the guard itself is not platform
/// specific.
#[cfg(unix)]
#[test]
fn binding_memory_lines_degrades_to_empty_when_the_database_cannot_be_opened() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = CliFixture::new();
    // Force the project database open, so the file exists, then take
    // away every permission on it — `ProjectMemory::open` must now fail.
    glasshouse::session::ProjectSessions::open(&fixture.runtime).unwrap();
    std::fs::set_permissions(
        fixture.runtime.database_path(),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();

    let lines = crate::commands::resume::binding_memory_lines(&fixture.runtime);

    assert_eq!(
        lines,
        Vec::<String>::new(),
        "an unopenable database must degrade to no memory, not panic"
    );

    // Restore permissions so the fixture's own directories can still be
    // cleaned up on drop.
    std::fs::set_permissions(
        fixture.runtime.database_path(),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
}

/// Map line 1515: `disposable_candidates` builds one `DisposableCandidate`
/// per configured provider's free and metered models
/// (`main.rs:6901-6960`). The census's mutation (make the function
/// return empty when free/metered models are configured) leaves a
/// configured provider with no candidate at all.
#[test]
fn disposable_candidates_builds_one_per_configured_free_and_metered_model_1515() {
    const VAR: &str = "GLASSHOUSE_TEST_ONLY_1515_CANDIDATE_KEY";
    // SAFETY: `VAR` is unique to this test and removed again below.
    unsafe {
        std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
    }

    let fixture = CliFixture::new();
    let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
    let mut provider = glasshouse::config::ProviderConfig::new("openai-compatible");
    provider.set_credential_env(vec![VAR.to_owned()]);
    provider.set_free_models(vec!["free-model-1515".to_owned()]);
    provider.set_metered_models(vec!["metered-model-1515".to_owned()]);
    user.providers_mut().set("test-provider-1515", provider);
    user.save(fixture.runtime.paths()).unwrap();

    let user = UserConfig::load(fixture.runtime.paths()).unwrap();
    let effective = EffectiveConfig::new(&user, None);
    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let telemetry = glasshouse::provider::resources::GatheredTelemetry::new();

    let candidates = crate::commands::shared::disposable_candidates(
        &user, None, &effective, &secrets, &telemetry,
    );

    unsafe {
        std::env::remove_var(VAR);
    }

    let models: Vec<&str> = candidates
        .iter()
        .filter(|candidate| candidate.provider() == "test-provider-1515")
        .map(|candidate| candidate.model())
        .collect();
    assert!(
        models.contains(&"free-model-1515"),
        "a configured free model must produce a candidate: {models:?}"
    );
    assert!(
        models.contains(&"metered-model-1515"),
        "a configured metered model must produce a candidate: {models:?}"
    );
}
