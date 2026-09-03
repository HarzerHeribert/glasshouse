//! `commands::resume` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use glasshouse::checkpoint::{Checkpoint, CheckpointReason, Handoff, ProjectCheckpoints};
use glasshouse::config::response::ResponseRequest;
use glasshouse::config::{self, EffectiveConfig, ProjectConfig, UserConfig};
use glasshouse::events::{EventBus, EventLog, LifecycleEvent, Observation, ProcessExit};
use glasshouse::guardrails::GuardrailOverride;
use glasshouse::launch::HarnessLaunch;
use glasshouse::pty::ExitStatus;
use glasshouse::session;
use glasshouse::session::{
    ProjectSessions, SessionId, SessionLifecycle, SessionPresentation, SessionRecord,
    SessionRuntime,
};
use glasshouse::{Runtime, shutdown};

/// Run a harness session that never takes this terminal — Phase 4's headless
/// presentation mode.
///
/// The mirror image of [`session::attach`]. The harness gets a real
/// pseudo-terminal in the project root exactly as it always does, but this
/// process's own terminal is never claimed: no raw mode, no alternate screen,
/// no output relayed to standard output. What the harness prints goes into
/// the session's own bounded scrollback, which is where an embedded session's
/// output goes too. That is the whole of "a PTY continues running without
/// occupying the visible session viewport" from the launch side; the shell
/// side is `shell::run`, which never makes a headless session the viewport's.
///
/// Glasshouse stays in the foreground for the session's whole life on
/// purpose. Returning early would drop the [`SessionRuntime`], and with it
/// the pseudo-terminal the harness is writing to — a detached session needs a
/// supervisor process, which is a different capability from this one.
///
/// **The terminal queries have to be answered here.** A headless session has
/// no emulator on the other end: on Windows nothing gets past ConPTY's
/// startup handshake without a reply, and on any platform a harness asking
/// `ESC[6n` waits forever for one. [`SessionRuntime`] knows how to answer but
/// cannot do it from its reader thread, so whoever owns the runtime must — in
/// the shell that is the tick, and here it is this loop.
///
/// # A signal here is a forced exit, and that is why the cleanup exists
///
/// [`shutdown::install_signal_handler`] ends the process immediately when the
/// terminal is not engaged, on the reasonable premise that a Glasshouse with
/// nothing to restore has nothing to wind down. **This path breaks that
/// premise**: it engages no terminal — that is what makes it headless — and
/// it owns a child process that stops receiving a hangup the moment Glasshouse
/// dies. Forced exit calls [`std::process::exit`], which runs no destructor,
/// so without the registration below a Ctrl-C would leave the harness running
/// with nothing left able to reach it.
///
/// Found by sending a real `SIGINT` to a real headless launch and looking for
/// the child afterwards; it was still there. `shutdown`'s own documentation
/// had already named this as the thing a second caller would have to get
/// right, which is exactly what this is.
///
/// Deliberately **not** solved by claiming the terminal is engaged. That flag
/// means "raw mode and the alternate screen are on", and `restore_terminal`
/// acts on it — setting it here would write escape sequences to a terminal
/// Glasshouse never touched.
pub(crate) fn run_headless(
    runtime: &Runtime,
    store: &glasshouse::session::SessionStore<'_>,
    id: &SessionId,
    launch: HarnessLaunch<'_>,
    deferred_briefing: Option<crate::commands::launch::DeferredBriefing>,
) -> anyhow::Result<ExitStatus> {
    /// How often the loop wakes to answer queries and check on the child.
    const POLL: std::time::Duration = std::time::Duration::from_millis(20);

    let live = Arc::new(Mutex::new(SessionRuntime::new()));
    lock(&live).start(id.clone(), SessionPresentation::Headless, &launch)?;

    // `GH-LAUNCH-BRIEFING`'s rung two: no adapter additive mechanism existed
    // to ride at `install_session_document` time, but this session runtime
    // now holds the PTY — the exact condition the design ruling names for
    // falling back to the door's own labelled-message delivery. Delivered
    // here, immediately after `start` registers the session as live and
    // before this loop's first poll, so it is the first thing the harness
    // reads after its own startup.
    if let Some(briefing) = deferred_briefing {
        let mut guard = lock(&live);
        let mut api = glasshouse::session::api::SessionApi::new(store, &mut guard);
        let delivered = api.send_text(
            id,
            briefing.injection.text(),
            glasshouse::events::MessageOrigin::Machine,
        );
        drop(guard);
        match delivered {
            Ok(()) => {
                glasshouse::evaluation::record_memory_retrieval(
                    runtime,
                    glasshouse::evaluation::RetrievalScope::Injection,
                    briefing
                        .injection
                        .memories()
                        .iter()
                        .map(glasshouse::memory::MemoryId::as_str),
                    Some(id.as_str()),
                    glasshouse::evaluation::now_unix(),
                );
                eprintln!("glasshouse: {}", briefing.announcement());
            }
            Err(err) => eprintln!(
                "glasshouse: warning: could not deliver this project's memory to session {id}; \
                 its task is being sent without it ({err:#})"
            ),
        }
    }

    // Best effort by construction, exactly as `session::attach`'s is:
    // `try_lock` gives up rather than risk blocking the one path whose whole
    // purpose is to always work. The loop below holds the lock only for the
    // moment it takes to poll, and never across its sleep. The guard
    // unregisters on the way out, so the callback never outlives the session
    // it refers to.
    let _forced_exit = {
        let live = Arc::clone(&live);
        let id = id.clone();
        shutdown::on_forced_exit(move || {
            close_before_forced_exit(&live, &id, FORCED_EXIT_BOUND);
        })
    };

    // A blocking process that prints nothing is indistinguishable from a hung
    // one. On stderr, because standard output belongs to whatever the caller
    // is piping this into.
    eprintln!("glasshouse: session {id} is running headless; nothing is drawn here");

    loop {
        {
            let mut live = lock(&live);
            live.answer_terminal_queries();
            for (ended, status) in live.poll_exits() {
                if &ended == id {
                    return Ok(status);
                }
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Take the headless runtime's lock, ignoring poisoning.
///
/// A panicking thread must not strand a live harness: the process is still
/// running and still needs to be polled and eventually hung up, and refusing
/// to touch the runtime would guarantee the orphan the registration above
/// exists to prevent.
fn lock(live: &Mutex<SessionRuntime>) -> std::sync::MutexGuard<'_, SessionRuntime> {
    live.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How long a forced-exit cleanup may spend trying to reach the runtime.
///
/// Far below any threshold a person would read as a hang, and far above the
/// microseconds the poll loop actually holds the lock for.
const FORCED_EXIT_BOUND: std::time::Duration = std::time::Duration::from_millis(500);

/// Close `id` on the way out of a forced exit, retrying briefly rather than
/// once.
///
/// [`glasshouse::shutdown`]'s rule is that a forced-exit callback must never
/// wait indefinitely: failing to clean up is survivable, failing to exit is
/// not. A **single** `try_lock` honours the letter of that rule and still
/// gets the wrong answer. The headless poll loop takes this same lock every
/// `POLL`, so one attempt is a coin flip, and losing it orphans a real
/// harness permanently with no second chance — there is no retry anywhere
/// above this.
///
/// That is not theoretical. It was **measured at 1 orphan in 100 runs under
/// 3x CPU load**, and it turned up first as an intermittent red
/// `test (macos-latest)` that passed on rerun against the identical commit.
///
/// A bound keeps the guarantee that actually matters — this returns, always,
/// and quickly — while removing the coin flip. Poisoning is treated as
/// ownership rather than as a reason to give up, for the same reason
/// [`lock`] does: a panicked thread must not strand a live child, and a
/// poisoned mutex would otherwise make `try_lock` fail for as long as we were
/// willing to retry.
///
/// Returns whether the runtime was reached.
pub(crate) fn close_before_forced_exit(
    live: &Mutex<SessionRuntime>,
    id: &SessionId,
    bound: std::time::Duration,
) -> bool {
    const RETRY: std::time::Duration = std::time::Duration::from_millis(1);
    let deadline = std::time::Instant::now() + bound;
    loop {
        match live.try_lock() {
            Ok(mut live) => {
                let _ = live.close(id);
                return true;
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                let _ = poisoned.into_inner().close(id);
                return true;
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(RETRY);
            }
        }
    }
}

/// The provider the local Glasshouse gateway forwards to, with its
/// credential resolved.
///
/// The configuration lookup is the caller's job, exactly as it already is
/// for a direct-provider profile: `glasshouse::profile` never imports
/// `glasshouse::config`, so every configured provider is read here and
/// handed in as a value. Which of them the gateway uses — and why more than
/// one is a refusal rather than a choice — is
/// `glasshouse::profile::gateway_upstream`'s decision, not this function's.
pub(crate) fn gateway_upstream(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    secrets: &dyn glasshouse::secret::SecretStore,
) -> anyhow::Result<glasshouse::gateway::Upstream> {
    let mut providers = Vec::new();
    for name in effective.provider_names() {
        providers.push(effective.configured_provider(&name)?.value);
    }
    // Phase 9I line 532: a provider the user has marked at least one free
    // model on backs this launch with `Cost::Free` rather than the fail-closed
    // `Cost::Metered` every backend got before. Looked up the same way
    // `disposable_candidates` looks it up — project layer winning over user —
    // because `glasshouse::profile` may not import `glasshouse::config` to
    // answer this itself.
    let free = |name: &str| -> bool {
        project
            .and_then(|p| p.providers().get(name))
            .or_else(|| user.providers().get(name))
            .is_some_and(|config| !config.free_models().is_empty())
    };
    Ok(glasshouse::profile::gateway_upstream(
        &providers, secrets, &free,
    )?)
}

/// Re-resolve `profile_name`'s overlay for a resumed session — Phase 9A line
/// 368's resume half, production caller of `resume_session`.
///
/// Exactly [`launch_session`]'s own resolution: the same lookup, the same
/// secret store, the same gateway start. A resumed session's overlay is not a
/// smaller thing than a fresh one's, so there is no separate, weaker path
/// here for it to take.
///
/// # Errors here are never fatal to the resume
///
/// The caller treats any `Err` as "resume without the overlay, and say why" —
/// never as a reason to refuse the resume outright. `open_for_resume` has
/// already proven this session is safe to continue; a bypass acknowledgement
/// withdrawn since the original launch, or a provider since removed from
/// configuration, is a reason to fall back to a plain native resume, not a
/// reason to make an otherwise-healthy session unresumable.
/// The routing evidence ledger for this project — **only when a gateway will
/// actually be started** — or `None` with a warning.
///
/// # Why the gate, and what it cost to learn
///
/// The first version opened the ledger unconditionally, before
/// `start_if_required_with_telemetry` decided whether a gateway was needed at
/// all. On macOS and Linux that was merely wasted work. On Windows it **hung
/// six memory-extraction tests indefinitely** — a 37-minute stall with no
/// output, on a tree whose local gate was 13/13 green.
///
/// [`crate::routing::evidence::EvidenceLedger`] holds `Mutex<Connection>`: an
/// open SQLite handle for its whole lifetime. SQLite locks with advisory
/// POSIX locks on Unix and with mandatory `LockFileEx` on Windows, so a handle
/// this function opened on a launch that never needed it blocks a later writer
/// on Windows and is invisible on Unix. **Opening a database you may not use is
/// not free, and the platform that charges for it is not the one this project
/// develops on.**
///
/// Gating on [`glasshouse::gateway::gateway_is_required`] makes the open happen
/// exactly when the gateway that consumes it is started, which is also what
/// `start_if_required_with_telemetry` would have decided a moment later.
///
/// Phase 33A records an observation per forwarded gateway exchange. Opening
/// its store touches the project database, and both callers evaluate this
/// **before** `start_if_required_with_telemetry` decides whether a gateway is
/// needed at all — so this runs on every launch and every resume.
///
/// It therefore must not fail the caller. A launch that refused to start
/// because a telemetry table could not be opened would trade the user's whole
/// session for a row nobody is waiting on, and this project's own product
/// invariant is that Glasshouse orchestrates real harnesses rather than
/// standing between the user and one. The warning is `tracing::warn!` for the
/// same reason `set_lifecycle`'s is: it belongs in the log, not on the
/// terminal the harness is about to take over.
pub(crate) fn evidence_ledger(
    runtime: &glasshouse::Runtime,
    profiles: &[glasshouse::profile::LaunchProfile],
) -> Option<std::sync::Arc<glasshouse::routing::evidence::EvidenceLedger>> {
    if !glasshouse::gateway::gateway_is_required(profiles) {
        return None;
    }
    match glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        Ok(ledger) => Some(std::sync::Arc::new(ledger)),
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; this session's turns will not be recorded"
            );
            None
        }
    }
}

// Eight parameters, and the eighth is the session id below. It stays a
// parameter rather than moving to the caller so that the gateway is told
// which session it serves *inside the function that started it*, before the
// gateway can be returned to anyone: a caller-side call is exactly the shape
// practice §35 warns about — a production step a later edit can drop with
// nothing to object. None of the eight names a fact any other one carries.
#[allow(clippy::too_many_arguments)]
fn resolve_resume_overlay(
    effective: &EffectiveConfig<'_>,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    selection: &session::HarnessSelection,
    profile_name: &str,
    // The whole `Runtime`, not just its paths: `EvidenceLedger::open` reads
    // the project's database and its project id, and narrowing to `paths`
    // here would put the ledger out of reach on the resume path alone.
    runtime: &glasshouse::Runtime,
    // Capability map line 2019 and `glasshouse::database` migration 24: the
    // session this resume continues, handed to the gateway started below.
    // The second of the binary's two doors, and the easier one — a resume's
    // record already exists, so there is no ordering to get right the way
    // `launch_session` has to wait for `store.create`.
    session_id: &session::SessionId,
    // Map line 1735. Built by `resume_session`, which is where the recorder
    // this eventually writes into is opened; this function only starts the
    // gateway, so it is a parameter rather than something resolved here.
    degrade_sink: glasshouse::gateway::DegradeSink,
) -> anyhow::Result<(
    glasshouse::profile::LaunchProfile,
    glasshouse::profile::LaunchOverlay,
    Option<glasshouse::gateway::Gateway>,
)> {
    let launch_profile = effective
        .launch_profile(profile_name, selection.id())?
        .value;
    let acknowledged_bypass = effective.bypass_acknowledged(selection.id()).value;
    let provider = match &launch_profile.backend {
        glasshouse::profile::BackendResource::DirectProvider { provider } => {
            Some(effective.configured_provider(provider)?.value)
        }
        _ => None,
    };
    // Phase 9E: the same store `launch_session` prefers, resolved fresh — a
    // credential is never carried across processes, let alone across the gap
    // between the original launch and this resume.
    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let gateway = glasshouse::gateway::start_if_required_with_degrade_sink(
        std::slice::from_ref(&launch_profile),
        || gateway_upstream(user, project, effective, &secrets),
        Some(glasshouse::provider::telemetry::GatewayQuotaCache::new(
            runtime.paths(),
        )),
        evidence_ledger(runtime, std::slice::from_ref(&launch_profile)),
        Some(glasshouse::provider::telemetry::GatewayHealthCache::new(
            runtime.paths(),
        )),
        Some(degrade_sink),
        // Line 1851, on the resume path too: a resumed session's gateway
        // fails over exactly as a launched one's does, and counting only the
        // launched ones would make the denominator a subset nobody stated.
        Some(crate::commands::routing_destinations::failover_prevention_sink(runtime)),
    )?;
    if let Some(gateway) = gateway.as_ref() {
        gateway.routing().serve_session(session_id.as_str());
    }
    let resolution = glasshouse::profile::Resolution {
        adapter: selection.adapter(),
        acknowledged_bypass,
        provider: provider.as_ref(),
        secrets: &secrets,
    };
    // Phase 9J line 576 — see `launch_session`'s own call for why this is
    // resolved here rather than inside `profile/**`.
    let pairing = crate::commands::launch::resolved_gateway_pairing(effective);
    let overlay = glasshouse::profile::resolve_with_gateway(
        &launch_profile,
        &resolution,
        gateway.as_ref(),
        &pairing,
    )?;
    Ok((launch_profile, overlay, gateway))
}

/// A one-line summary of a resolved overlay's mechanisms, for the "opening a
/// harness session" log line — category and detail only, exactly what
/// [`glasshouse::profile::LaunchOverlay::mechanisms`] exposes for rendering.
/// An environment *value* is never in here, because the overlay never puts
/// one in a `MechanismNote` to begin with.
pub(crate) fn mechanism_summary(overlay: &glasshouse::profile::LaunchOverlay) -> String {
    if overlay.mechanisms().is_empty() {
        return "none".to_owned();
    }
    overlay
        .mechanisms()
        .iter()
        .map(|note| format!("{}: {}", note.category, note.detail))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Install lifecycle hooks for a session that is about to start, returning
/// the arguments that make the harness read them.
///
/// Best effort by construction. A harness that reports nothing is a harness
/// Glasshouse knows less about, which is a smaller loss than refusing to start
/// a session the user asked for because a configuration file could not be
/// written.
pub(crate) fn install_session_document(
    runtime: &Runtime,
    selection: &session::HarnessSelection,
    id: &session::SessionId,
    project_hooks_consent: bool,
    response: &glasshouse::harness::response::Application,
) -> Vec<std::ffi::OsString> {
    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(err) => {
            tracing::warn!(error = %err, "could not find the Glasshouse executable for hooks");
            return Vec::new();
        }
    };
    let report = glasshouse::harness::HookCommand::new(
        program,
        id.as_str(),
        runtime.session_dir(id.as_str()),
        runtime.project().root(),
        runtime.paths().data_dir(),
        runtime.paths().config_dir(),
    );
    match selection.install_session_document(&report, project_hooks_consent, response) {
        Ok(document) => document.args,
        Err(err) => {
            tracing::warn!(
                session = %id,
                error = %err,
                "could not write the session's harness document"
            );
            Vec::new()
        }
    }
}

/// Map lines 1991-1996: register the context firewall's `PostToolUse` hook
/// for a Claude Code session, when the effective configuration enables it.
///
/// **Never a second `--settings` flag.** Claude Code 2.1.247 silently
/// discards every `--settings` but the last (verified in
/// `session::HarnessSelection::install_session_document`'s own doc), so the
/// only safe way to add a hook is to merge it into the SAME document
/// [`install_session_document`] already wrote — this function reads that
/// file back, adds one `PostToolUse` key, and writes it in place. `args`
/// itself is never touched, which is what makes `mode = "off"` byte-identical
/// to a session built before this phase existed: this function returns
/// before touching anything when the harness is not Claude Code or the
/// effective mode is `off`.
///
/// Best effort, matching [`install_session_document`]'s own policy: any
/// failure here is a session that starts without the firewall bridge rather
/// than one that fails to start, and is logged rather than propagated.
///
/// Map lines 2023/2024: `entitlement` and `backend` are read only to
/// *classify* the reduction policy (subscription, metered or local) and to
/// resolve its thresholds through `effective`'s new accessors — never baked
/// into the registered command line themselves. The firewall core and the
/// hook subprocess this command line invokes stay entitlement-blind, exactly
/// as before this package: only numbers and a mode word ever reach them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn install_context_firewall_hook(
    runtime: &Runtime,
    selection: &session::HarnessSelection,
    effective: config::EffectiveConfig<'_>,
    session_dir: &std::path::Path,
    entitlement: Option<&glasshouse::config::ResolvedEntitlement>,
    backend: &glasshouse::profile::BackendResource,
    profile_name: &str,
    session: &SessionId,
) {
    use glasshouse::config::firewall::{FirewallMode, ReductionPolicyKind};
    use glasshouse::config::{EntitlementBacking, EntitlementKind};
    use glasshouse::harness::claude_code;
    use glasshouse::profile::BackendResource;
    use glasshouse::provider::registry::{Locality, ResourceKind};

    if selection.id() != glasshouse::integrations::IntegrationId::ClaudeCode {
        // A non-claude-code harness gets no registration and no warning
        // spam — one debug line, per the packet's own instruction.
        tracing::debug!(
            harness = selection.id().slug(),
            "context firewall: no verified PostToolUse bridge for this harness"
        );
        return;
    }

    // Map lines 2023/2024's classification: a subscription pays in rate
    // limits and context window, a key in tokens, local inference in
    // latency — so each gets its own default thresholds. Locality outranks
    // `EntitlementKind` here: an entitlement backed by a local provider is
    // classified `Local` regardless of what its own `kind` says, because
    // latency is what actually drives the policy for that resource. An
    // unresolved entitlement (`None` here) never guesses a kind.
    let provider_is_local =
        |provider: &str| ResourceKind::from_direct_provider(provider).locality() == Locality::Local;
    let kind = match entitlement {
        Some(entitlement) => match entitlement.backing() {
            EntitlementBacking::Provider(provider) if provider_is_local(provider) => {
                Some(ReductionPolicyKind::Local)
            }
            _ => match entitlement.kind() {
                Some(
                    EntitlementKind::Claude | EntitlementKind::ChatGpt | EntitlementKind::Gemini,
                ) => Some(ReductionPolicyKind::Subscription),
                Some(EntitlementKind::ApiKey) => Some(ReductionPolicyKind::Metered),
                None => None,
            },
        },
        // No entitlement describes this resource — still `Local` when the
        // session is served by a local provider directly (map lines
        // 2023/2024's "or a session served by such a provider with no
        // entitlement").
        None => match backend {
            BackendResource::DirectProvider { provider } if provider_is_local(provider) => {
                Some(ReductionPolicyKind::Local)
            }
            _ => None,
        },
    };

    let configured_mode =
        effective.context_firewall_policy_mode(kind, Some(profile_name), entitlement);
    if configured_mode == FirewallMode::Off {
        return;
    }

    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "context firewall: could not find the Glasshouse executable; not registered"
            );
            return;
        }
    };

    // Map line 1994: verify at session start against the installed
    // harness. Cached for exactly this one registration — the hook
    // subprocess Claude Code spawns later for every tool call never
    // re-probes anything; the decision made here is baked into the
    // registered command line's own `--mode` flag.
    let probe = std::process::Command::new(selection.executable().path())
        .arg("--version")
        .output();
    let effective_mode = match probe {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            match claude_code::parse_version(&text)
                .filter(|version| claude_code::supports_updated_tool_output(*version))
            {
                Some(_) => configured_mode,
                None => {
                    eprintln!(
                        "glasshouse: the installed Claude Code (`{}`) is below the verified \
                         floor for tool-output replacement ({}); the context firewall \
                         registers in shadow mode for this session",
                        text.trim(),
                        claude_code::MIN_UPDATED_OUTPUT_VERSION_STRING,
                    );
                    record_context_firewall_registration_fallback(runtime, "version-floor");
                    FirewallMode::Shadow
                }
            }
        }
        _ => {
            eprintln!(
                "glasshouse: could not verify the installed Claude Code's version; the \
                 context firewall registers in shadow mode for this session"
            );
            record_context_firewall_registration_fallback(runtime, "version-unprobed");
            FirewallMode::Shadow
        }
    };

    let passthrough_tokens = effective.context_firewall_policy_passthrough_tokens(
        effective_mode,
        kind,
        Some(profile_name),
        entitlement,
    );
    let min_semantic_tokens = effective.context_firewall_policy_min_semantic_tokens(
        kind,
        Some(profile_name),
        entitlement,
    );
    // Map line 1992: no mode, including aggressive, ever names a reducer —
    // there is no flag here that could carry one, which is the guard by
    // construction the box asks for.
    let emit_updated_output = effective_mode != FirewallMode::Shadow;
    // Map line 1139's producer needs the *Glasshouse* session, and this is
    // the only place that knows it: a `PostToolUse` payload carries Claude
    // Code's own identifier, which no table here has ever seen. Baked into
    // the command line exactly as the lifecycle hook's `--session` is
    // (`harness::HookCommand::shell_command`), for that function's stated
    // reason — a hook is a fresh process and must not discover anything from
    // its surroundings.
    let command_line = claude_code::context_firewall_command_line(
        &program,
        effective_mode,
        passthrough_tokens,
        emit_updated_output,
        min_semantic_tokens,
        session.as_str(),
    );
    let hook_entry = claude_code::context_firewall_hook_entry(&command_line);

    let path = session_dir.join(claude_code::SETTINGS_FILE_NAME);
    let existing = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %path.display(),
                "context firewall: could not read the settings document to merge its hook \
                 into; not registered"
            );
            return;
        }
    };
    match claude_code::merge_context_firewall_hook(&existing, &hook_entry) {
        Ok(merged) => {
            if let Err(err) = std::fs::write(&path, merged) {
                tracing::warn!(
                    error = %err,
                    path = %path.display(),
                    "context firewall: could not write the merged settings document; not \
                     registered"
                );
            }
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "context firewall: could not merge the PostToolUse hook; not registered"
            );
        }
    }
}

/// Map line 1994's fallback: one bypass-family telemetry row recorded at
/// registration time, distinct from [`record_context_firewall_telemetry`]'s
/// per-tool-call rows — this one is about the registration decision itself,
/// taken once per launch rather than once per hook invocation.
fn record_context_firewall_registration_fallback(runtime: &Runtime, reason: &str) {
    use glasshouse::routing::evidence::{
        CONTEXT_FIREWALL_BYPASS_PURPOSE, EvidenceLedger, NewObservation,
    };

    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; a context-firewall registration \
                 fallback is not recorded"
            );
            return;
        }
    };
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let observation = NewObservation::new("glasshouse", "context-firewall")
        .with_harness(Some(
            glasshouse::integrations::IntegrationId::ClaudeCode.slug(),
        ))
        .with_purpose(Some(CONTEXT_FIREWALL_BYPASS_PURPOSE))
        .with_route(Some(reason))
        .with_quota_context(Some("registration".to_string()))
        .with_timing(Some(now_unix), Some(now_unix));
    if let Err(err) = ledger.record(observation, now_unix) {
        tracing::warn!(error = %err, "could not record a context-firewall registration fallback");
    }
}

/// Records lifecycle events durably from a command that is about to exit.
///
/// # Why this is not the sink the shell uses
///
/// [`glasshouse::events::EventLogSink`] queues behind a writer thread,
/// because the shell publishes from a thread that is sometimes draining a
/// pseudo-terminal and must never wait. None of that applies here: a
/// `glasshouse hook` process lives for a few milliseconds and then exits, and
/// queueing behind a thread it is about to drop would lose the event it was
/// run to record. So this writes synchronously.
///
/// # Why there is a bus at all
///
/// [`glasshouse::events::RecordedEvent`] cannot be built without a session
/// identifier and a timestamp — that is a property of the type rather than a
/// habit of its callers, and [`EventBus::publish`] is what stamps both. Using
/// it as the minting authority is what keeps "record every translated
/// lifecycle event with session ID and timestamp" true on this path as well
/// as in the interactive one. No sink is attached to it, so nothing is
/// written twice.
///
/// # Every failure is swallowed into the log, deliberately
///
/// This runs inside the user's own session — see [`report_hook`], which may
/// never fail — and it is also on the launch path, where a bookkeeping
/// failure must not turn into what looks like a harness failure. A project
/// whose database cannot be opened loses event history and keeps its session.
/// # Why the log is behind a `Mutex`
///
/// [`EventLog`] owns a `rusqlite::Connection`, which is `Send` and **not**
/// `Sync`. Since [`DegradeRelay`], a recorder is no longer touched only by
/// the thread that built it: the gateway's own connection thread reports a
/// failed upstream through it, so `&EventRecorder` crosses a thread boundary
/// and the type has to be `Sync` to be shared at all. The lock is what makes
/// it so, and it is uncontended in practice — the two writers are a launch
/// path making one bookkeeping call at a time and a gateway thread that only
/// speaks when its upstream has just failed.
pub(crate) struct EventRecorder {
    bus: EventBus,
    log: Option<Mutex<EventLog>>,
}

impl EventRecorder {
    pub(crate) fn open(runtime: &Runtime) -> Self {
        let log = match EventLog::open(runtime) {
            Ok(log) => Some(Mutex::new(log)),
            Err(err) => {
                tracing::warn!(error = %format!("{err:#}"), "could not open the project event log");
                None
            }
        };
        Self {
            bus: EventBus::new(),
            log,
        }
    }

    /// Record that one backend resource stopped serving — map line 1735's
    /// durable half, on the path the shipped binary actually takes.
    ///
    /// # Why `degrade_resource` is called rather than reimplemented
    ///
    /// Which sessions a failing resource affects is one rule, and it lives in
    /// [`glasshouse::events::degrade_resource`]: *a session is affected if,
    /// and only if, its own record says it resolved to this backend
    /// resource.* Selecting the sessions here instead would be a second copy
    /// of that rule, and it would leave `degrade_resource` with no production
    /// caller again — the exact state the evidence ledger refused this line
    /// in.
    ///
    /// # Why it publishes on a bus that keeps nothing
    ///
    /// `degrade_resource` publishes each `GatewayUnhealthy` on the bus it is
    /// given, and the durable write on this path is [`Self::append`], which
    /// publishes on *this* recorder's bus to mint the record. Handing it
    /// `self.bus` would mint every event twice. A history of zero makes the
    /// bus purely the question-asking apparatus: nothing is kept, nothing is
    /// dropped, and the returned [`glasshouse::events::Degradation`] is the
    /// answer this method acts on.
    fn degrade(
        &self,
        records: &[SessionRecord],
        resource: &str,
        reason: glasshouse::events::GatewayFailure,
    ) {
        let selection = EventBus::with_history(0);
        let degradation =
            glasshouse::events::degrade_resource(&selection, records, resource, reason);
        for id in &degradation.affected {
            self.record(
                id,
                LifecycleEvent::GatewayUnhealthy {
                    resource: degradation.resource.clone(),
                    reason,
                },
            );
        }
    }

    pub(crate) fn record(&self, id: &SessionId, event: LifecycleEvent) {
        self.append(id, event, None);
    }

    /// Record an event together with the harness report it was translated
    /// from — the harness's own two words, and nothing else.
    pub(crate) fn record_observed(
        &self,
        id: &SessionId,
        event: LifecycleEvent,
        observed: Observation,
    ) {
        self.append(id, event, Some(observed));
    }

    fn append(&self, id: &SessionId, event: LifecycleEvent, observed: Option<Observation>) {
        let recorded = self.bus.publish(id, event);
        let Some(log) = &self.log else {
            return;
        };
        let log = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(err) = log.append(&recorded, observed.as_ref()) {
            tracing::warn!(session = %id, error = %err, "could not record a lifecycle event");
        }
    }
}

/// The most gateway failures held while there is still nowhere to record
/// them.
///
/// A bound rather than an unbounded queue for the reason every other buffer
/// in this crate is bounded: the window is meant to be empty, and a window
/// that is not empty is a defect, not a workload. Anything past this is
/// counted and reported rather than kept — see [`DegradeRelay::report`].
const EARLY_GATEWAY_FAILURES: usize = 32;

/// Where a gateway failure is recorded, given that the recorder does not
/// exist yet when the gateway starts.
///
/// # The ownership problem, stated exactly
///
/// [`glasshouse::gateway::DegradeSink`] has to be handed to the gateway at
/// `start_if_required_with_degrade_sink`, and **both** of this binary's
/// gateway starts happen before anything the sink needs exists:
/// `launch_session` starts the gateway 184 lines before it opens its
/// [`EventRecorder`], and it has no `SessionRecord` at all until the store
/// has created one. So the sink cannot close over a bus and a session list;
/// there is nothing to close over. This is the handle it closes over
/// instead, created before the gateway and filled by [`Self::install`] once
/// both halves exist.
///
/// # Why the session records are a snapshot, and whose sessions they are
///
/// [`glasshouse::events::degrade_resource`] takes the records it should
/// consider. This relay is given **the sessions this process owns** — one, on
/// either path — and not a fresh read of the project's whole session table.
/// Two reasons, and the second is the load-bearing one:
///
/// - reading fresh would mean a `SessionStore` on the gateway's thread, which
///   means a second open connection held for the life of the session for a
///   read that fires only when an upstream has failed. §65's Windows hang was
///   exactly that shape;
/// - and a gateway is **per instance**. Another Glasshouse process's session
///   is served by *its* gateway, which does its own detecting. Degrading it
///   from here would report a failure this process never observed on that
///   session's behalf. The narrower snapshot is the honest claim.
///
/// # Lifetime
///
/// The sink holds an `Arc<DegradeRelay>` and the relay holds an
/// `Arc<EventRecorder>`; neither points back, so there is no cycle to leak.
/// No thread is started here and none is kept alive: the relay is inert
/// between calls, and the gateway's own guard is what stops the threads that
/// call it.
pub(crate) struct DegradeRelay {
    state: Mutex<RelayState>,
}

impl DegradeRelay {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(RelayState::Waiting {
                held: Vec::new(),
                dropped: 0,
            }),
        })
    }

    fn own(&self) -> std::sync::MutexGuard<'_, RelayState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The sink to start a gateway with.
    pub(crate) fn sink(self: &Arc<Self>) -> glasshouse::gateway::DegradeSink {
        let relay = Arc::clone(self);
        Arc::new(move |resource: &str, reason| relay.report(resource, reason))
    }

    /// Called by the gateway's own connection thread, once per exchange whose
    /// outcome says the upstream failed.
    ///
    /// A failure that arrives before [`Self::install`] is **held, not
    /// dropped and not fatal**. Panicking would take down the user's session
    /// over a telemetry ordering problem, and discarding would make the one
    /// window this line exists to observe the one window it cannot see. Past
    /// the bound the failure is counted and named in the log, so even the
    /// discard leaves a trace.
    fn report(&self, resource: &str, reason: glasshouse::events::GatewayFailure) {
        match &mut *self.own() {
            RelayState::Waiting { held, dropped } => {
                if held.len() >= EARLY_GATEWAY_FAILURES {
                    *dropped += 1;
                    tracing::warn!(
                        resource,
                        %reason,
                        dropped = *dropped,
                        "a gateway failure arrived before this launch had anywhere to record \
                         it, and more than {} are already waiting",
                        EARLY_GATEWAY_FAILURES
                    );
                    return;
                }
                held.push((resource.to_owned(), reason));
            }
            RelayState::Ready { events, records } => {
                events.degrade(records, resource, reason);
            }
        }
    }

    /// Give the relay somewhere to write, and replay anything that arrived
    /// first.
    ///
    /// Called on the launch and resume paths at the first point where both
    /// the recorder and the session record exist. Nothing before this point
    /// waits on it: the gateway is already serving.
    pub(crate) fn install(&self, events: Arc<EventRecorder>, records: Vec<SessionRecord>) {
        let mut state = self.own();
        let ready = RelayState::Ready {
            events: Arc::clone(&events),
            records: records.clone(),
        };
        let RelayState::Waiting { held, dropped } = std::mem::replace(&mut *state, ready) else {
            // Each path builds its own relay and installs once. A second
            // install would mean two owners of one gateway's failures.
            tracing::warn!("a degrade relay was installed twice");
            return;
        };
        if dropped > 0 {
            tracing::warn!(
                dropped,
                "gateway failures were discarded before this launch could record them"
            );
        }
        // Still under the lock: a failure arriving now waits rather than
        // overtaking the replay, so the log keeps the order the gateway saw.
        for (resource, reason) in held {
            events.degrade(&records, &resource, reason);
        }
    }
}

impl Drop for DegradeRelay {
    /// The last resort against a silent loss: a relay dropped while still
    /// holding failures is a launch that ended before it could record them —
    /// a database that would not open, or an early return between the gateway
    /// and the recorder. Nothing can be written at that point; saying so in
    /// the log is what stops it from being invisible.
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let RelayState::Waiting { held, dropped } = state
            && (!held.is_empty() || *dropped > 0)
        {
            tracing::warn!(
                held = held.len(),
                dropped,
                "this launch ended without ever being able to record a gateway failure"
            );
        }
    }
}

/// [`DegradeRelay`]'s two lives, in one lock so that a failure arriving
/// during [`DegradeRelay::install`] cannot be pushed onto a queue that has
/// just been drained.
enum RelayState {
    /// Before the recorder exists. Failures accumulate, with their count.
    Waiting {
        held: Vec<(String, glasshouse::events::GatewayFailure)>,
        /// How many were dropped for exceeding [`EARLY_GATEWAY_FAILURES`].
        dropped: usize,
    },
    /// After it does. Failures are written straight through.
    Ready {
        events: Arc<EventRecorder>,
        records: Vec<SessionRecord>,
    },
}

/// The project's current binding memory, rendered for a checkpoint's
/// `Handoff::memory` — line 1641.
///
/// Opening the project's memory database or reading its binding records is
/// never allowed to fail a checkpoint: a checkpoint with no memory section is
/// strictly better than no checkpoint at all, so either failure degrades to
/// an empty list rather than propagating with `?`. `api/unix.rs::request_checkpoint`
/// carries the identical addition rather than calling through this one — see
/// its own comment on why that duplication stands.
pub(crate) fn binding_memory_lines(runtime: &Runtime) -> Vec<String> {
    use glasshouse::memory::ProjectMemory;

    let Ok(memory) = ProjectMemory::open(runtime) else {
        return Vec::new();
    };
    let Ok(records) = memory.store().binding(20) else {
        return Vec::new();
    };
    records
        .into_iter()
        .map(|record| match record.subject {
            // Phase 20 allows an absent subject; rendering an empty one would
            // print a heading nobody wrote.
            Some(subject) => format!("{subject}: {}", record.body),
            None => record.body,
        })
        .collect()
}

/// The session a checkpoint command means.
///
/// Named explicitly, or the project's most recently active one — which is what
/// "the active session" means outside the interactive interface, and is the
/// row `glasshouse sessions` already prints first.
pub(crate) fn active_session(
    sessions: &ProjectSessions,
    named: Option<&str>,
) -> anyhow::Result<Option<glasshouse::session::SessionRecord>> {
    let store = sessions.store();
    match named {
        Some(named) => {
            let id = store.resolve_id(named)?;
            Ok(Some(store.get(&id)?.ok_or_else(|| {
                anyhow::anyhow!("session `{id}` is not in this project")
            })?))
        }
        None => Ok(store.list()?.into_iter().next()),
    }
}

/// Check point the session this work is leaving, before it moves —
/// capability map line 1716.
///
/// `moving_to` is where the work is going: a session identifier when this
/// launch or resume is continuing one, and `None` when it is starting a new
/// session. The session being **left** is whichever this project was most
/// recently active in, which is the same `active_session` rule
/// `glasshouse checkpoint save` and `Request::TakeCheckpoint` use for "the
/// current session".
///
/// # Three of the four cases are a no-op, and each says which
///
/// Nothing is being left when this project has no recorded session, when the
/// launch is starting a fresh one, or when the destination *is* the session
/// already in hand. Writing a checkpoint for any of those would produce a
/// handoff describing a migration that did not happen. The flag says so
/// instead of passing silently: a person who asked for a checkpoint and did
/// not get one needs to know which of the two occurred, and a silent no-op is
/// indistinguishable from a checkpoint that was taken (practice §68's shape).
///
/// # It invents nothing, and it fails loudly
///
/// The handoff records only what Glasshouse knows: which session was left,
/// where the work went, the Git position and this project's binding memories,
/// all through the same [`Checkpoint::capture`] the two existing checkpoint
/// paths use. It does not read the session's terminal for an objective —
/// `checkpoint_command`'s own doc says why that would be a confident fiction.
///
/// A failure here **stops the launch**. The person asked for a checkpoint
/// before the move; moving anyway would lose exactly what they asked to keep.
pub(crate) fn checkpoint_before_moving(
    runtime: &Runtime,
    moving_to: Option<&str>,
) -> anyhow::Result<()> {
    // Its own scope, closed before the caller opens the session store the
    // resume path needs — practice §65: two live connections to one SQLite
    // database in one process is invisible on Unix and a lock on Windows.
    let leaving = {
        let sessions = ProjectSessions::open(runtime)?;
        let store = sessions.store();
        let Some(record) = active_session(&sessions, None)? else {
            eprintln!(
                "glasshouse: --checkpoint-first had nothing to check point: this project has \
                 no recorded session to leave."
            );
            return Ok(());
        };
        let Some(destination) = moving_to else {
            eprintln!(
                "glasshouse: --checkpoint-first had nothing to check point: this starts a new \
                 session and leaves nothing behind."
            );
            return Ok(());
        };
        // Resolved through the store so a short identifier — the twelve
        // characters `glasshouse sessions` prints — compares equal to the
        // full one. Comparing the strings would make `--to <short id>` look
        // like a different session from the one it names.
        let destination = store.resolve_id(destination)?;
        if destination == record.id {
            eprintln!(
                "glasshouse: --checkpoint-first had nothing to check point: session {} is \
                 already where this work is.",
                crate::commands::shared::short_id(&record.id)
            );
            return Ok(());
        }
        (record, destination)
    };
    let (record, destination) = leaving;

    let checkpoints = ProjectCheckpoints::open(runtime)?;
    let store = checkpoints.store();
    let stored = store.save(Checkpoint::capture(
        &record.id,
        &record.harness,
        // The person asked for it by passing the flag, which is exactly what
        // `Manual` means here. There is deliberately no new reason: the
        // stored vocabulary is fixed by a `CHECK` constraint in the schema,
        // and a third spelling would be a migration for a distinction the
        // handoff text below already carries.
        CheckpointReason::Manual,
        store.now(),
        runtime.project().root(),
        Handoff {
            objective: format!(
                "preserve session {} before this project's work moved to {}",
                crate::commands::shared::short_id(&record.id),
                crate::commands::shared::short_id(&destination)
            ),
            implementation_state: format!(
                "Glasshouse took this checkpoint because --checkpoint-first was passed to a \
                 command that moved work out of session {}. Nothing here was read from that \
                 session's terminal: what Glasshouse knows is which session was left, where \
                 the work went, this project's Git position, and its binding memories.",
                crate::commands::shared::short_id(&record.id)
            ),
            decisions: Vec::new(),
            memory: binding_memory_lines(runtime),
            failed_approaches: Vec::new(),
            files: Vec::new(),
            test_state: None,
            next_actions: vec![format!(
                "continue in session {}, or reopen this one with `glasshouse resume {}`",
                crate::commands::shared::short_id(&destination),
                crate::commands::shared::short_id(&record.id)
            )],
        },
    ))?;

    eprintln!(
        "glasshouse: checkpoint {} saved for session {} before this work moved to {}.",
        stored.id.short(),
        crate::commands::shared::short_id(&record.id),
        crate::commands::shared::short_id(&destination)
    );
    Ok(())
}

/// The handoff prompt a `--from-checkpoint` launch opens with, if one was
/// asked for, alongside the session the checkpoint was recorded from.
///
/// A named checkpoint that does not exist is an error rather than an empty
/// prompt: starting a fresh session that silently lost its handoff is the
/// worst of the available outcomes, because it looks exactly like one that
/// worked.
pub(crate) fn resolve_bootstrap_prompt(
    runtime: &Runtime,
    named: Option<&str>,
) -> anyhow::Result<Option<(String, SessionId)>> {
    let Some(named) = named else {
        return Ok(None);
    };
    let checkpoints = ProjectCheckpoints::open(runtime)?;
    let store = checkpoints.store();
    let stored =
        crate::commands::checkpoint::resolve_checkpoint(&store, Some(named))?.ok_or_else(|| {
            anyhow::anyhow!(
                "this project has no checkpoints yet, so there is nothing to start from"
            )
        })?;

    tracing::info!(
        checkpoint = %stored.id,
        session = %stored.checkpoint.session,
        // The harness the checkpoint came *from*. Worth a line, because the
        // whole point is that it need not be the one about to start.
        recorded_by = %stored.checkpoint.harness,
        "starting a session from a checkpoint"
    );
    Ok(Some((
        stored.checkpoint.bootstrap_prompt(),
        stored.checkpoint.session.clone(),
    )))
}

/// Reopen a recorded session in its own harness.
///
/// The order here is the safety property. The store decides whether this
/// session may be resumed *at all* — it belongs to this project, it is not
/// still running, and it has a native identifier to resume to — before any
/// harness is selected and long before any process exists. A refusal costs
/// nothing; a session opened against the wrong project would be a breach of
/// the isolation the whole product rests on.
///
/// The harness is then whichever one the record names, not whichever one is
/// configured now: resuming a Codex conversation in Claude Code would be
/// nonsense, so a record's own harness is what gets selected.
/// Line 1592's task-boundary caller, and line 1601's explanation on it.
///
/// Prints where the router would have sent this work and what the named
/// session displaced. Never changes the destination — see `RouteOnResume`.
/// Everything it needs can fail (the session store, a deleted profile, a quota
/// cache that will not open), and none of those may cost a person their
/// resume, so the whole thing is best effort and silent when it has nothing to
/// say.
/// **It explains; it does not move the work.** The session was named on the
/// command line, and a router that answered "somewhere else" would overrule
/// the most explicit statement a person can make — so the named session goes
/// in as `RoutingOverride::to`, which is what line 1602 calls a user override,
/// and the ranking it displaced is printed beside it. Stated as a limit rather
/// than left to be discovered: **line 1593 is earned on the launch path**,
/// where the choice is genuinely open, and not here.
fn report_task_boundary_routing(runtime: &Runtime, session: &str) {
    use glasshouse::routing::session::{
        RouterInputs, RoutingMoment, RoutingOverride, TaskRequirements,
    };

    // Its own scope, and everything it opened is closed before it returns —
    // see the call site.
    let Some((id, harness)) = ({
        let Ok(sessions) = ProjectSessions::open(runtime) else {
            return;
        };
        let store = sessions.store();
        store
            .resolve_id(session)
            .ok()
            .and_then(|id| store.get(&id).ok().flatten())
            .map(|record| (record.id.clone(), record.harness.clone()))
    }) else {
        return;
    };
    let Some(harness) = glasshouse::integrations::IntegrationId::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.slug() == harness)
    else {
        return;
    };

    let Ok(user) = UserConfig::load(runtime.paths()) else {
        return;
    };
    let Ok(project) = config::load_project_config(runtime.project()) else {
        return;
    };
    let effective = EffectiveConfig::new(&user, project.as_ref());

    let Ok(destinations) = crate::commands::routing_destinations::routing_destinations(
        runtime,
        &effective,
        harness,
        crate::commands::routing_destinations::DestinationScope::Everything,
        None,
    ) else {
        return;
    };
    let current = destinations
        .iter()
        .find(|destination| destination.id() == id.as_str())
        .cloned();
    let overrides = effective.pairing_overrides();
    // Line 1599's bridge again — see `observed_provider_health`. This report
    // is read beside the launch path's own decision, so it weighs the same
    // persisted readings that path does.
    let health = crate::commands::routing_destinations::observed_provider_health(
        runtime,
        &effective,
        &destinations,
    );
    // Phase 34D does not reach this report: `glasshouse resume` carries no
    // task text, so there is nothing to classify and nothing is invented.
    // The moment a `resume` learns what the next task is, this is the site
    // that hands `classify_for_routing` a `TaskBoundary` moment.
    let inputs = RouterInputs {
        overrides: &overrides,
        health: health.pool(),
        now: std::time::Instant::now(),
        requirements: TaskRequirements::default(),
    };
    let Some(routed) = crate::commands::routing_destinations::session_router(
        runtime,
        &effective,
        RoutingOverride::to(id.as_str()),
    )
    .choose(
        RoutingMoment::TaskBoundary,
        current.as_ref(),
        &destinations,
        &inputs,
    ) else {
        return;
    };
    // A ranking that agreed with the user says nothing worth a line on their
    // terminal; one that would have chosen differently is the whole reason
    // line 1601 exists.
    if let Some(automatic) = routed.overrode() {
        eprintln!(
            "glasshouse: resuming {} because you named it; the ranking would have chosen `{}` \
             at this task boundary. `glasshouse route --moment task-boundary` says why.",
            crate::commands::shared::short_id(&id),
            automatic
        );
    }
}

/// Whether a resume is the moment a routing decision is taken, or the tail of
/// one that already was.
///
/// `glasshouse resume` is line 1592's **task boundary**: one piece of work
/// finished and another is beginning, which is exactly when the map allows the
/// work to move. `launch_session` reaches the same code after having already
/// decided at a *session* boundary, and routing twice for one launch would
/// re-decide something nobody asked to have re-decided — the failure mode line
/// 1592 is written against, one layer up from the per-turn one it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteOnResume {
    /// Take the task-boundary decision here.
    AtTaskBoundary,
    /// The caller already routed; this is the tail of its decision.
    AlreadyRouted,
}

pub(crate) fn resume_session(
    runtime: &Runtime,
    session: &str,
    harness_args: &[String],
    headless: bool,
    routing: RouteOnResume,
) -> anyhow::Result<ExitCode> {
    // Line 1592's other moment, and it runs **before** this function's own
    // session store is opened.
    //
    // Not a stylistic choice. `routing_destinations` opens a connection to
    // this project's session database, and practice §65 is the record of what
    // an extra open handle costs on a path nobody asserts about: SQLite takes
    // advisory locks on Unix and mandatory `LockFileEx` locks on Windows, so
    // two live connections to one database in one process is invisible on the
    // machine this is developed on and a hang on the one it ships to. Running
    // the routing report to completion first means exactly one connection is
    // ever live.
    if routing == RouteOnResume::AtTaskBoundary {
        report_task_boundary_routing(runtime, session);
    }

    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();

    // Both of these refuse rather than guess: an ambiguous prefix names its
    // candidates, and `open_for_resume` carries the project-isolation check.
    let id = store.resolve_id(session)?;
    let resumable = store.open_for_resume(&id)?;
    // Phase 9A line 368's resume half. `ResumableSession` carries only what
    // project isolation needs to check; the record's six facts — which
    // profile, which backend, which response profile — live on the full
    // `SessionRecord`, read with the store's existing `get`, not a new
    // column. `open_for_resume` already proved this session exists in this
    // project, so a `None` here would mean the store disagreed with itself
    // between those two calls.
    let record = store
        .get(&resumable.id)?
        .expect("open_for_resume already proved this session's record exists");

    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let selection = session::select::select(Some(resumable.harness.as_str()), effective)?;

    let Some(mut args) = selection.resume_args(
        &resumable.native_session_id,
        harness_args.iter().map(String::as_str),
    ) else {
        anyhow::bail!(
            "{} has no resume mechanism Glasshouse has verified, so session `{}` cannot be \
             reopened. Start a new session instead.",
            selection.id().display_name(),
            crate::commands::shared::short_id(&resumable.id)
        );
    };

    tracing::info!(
        session = %resumable.id,
        harness = selection.id().slug(),
        executable = %selection.executable().path().display(),
        source = %selection.source(),
        // The native identifier is not a secret — it names a conversation in
        // the user's own harness history, and it is the one fact that makes a
        // failed resume diagnosable.
        native_session = %resumable.native_session_id,
        "resuming a harness session"
    );

    // Phase 9A line 368's resume half, the sharper part: re-resolve the
    // launch profile the record names, so the overlay this resumed process
    // actually receives — its environment, its arguments, any generated
    // provider configuration — matches the profile `sessions show` still
    // reports for it. Before this, a resume applied none of the six facts it
    // displays.
    //
    // A profile that no longer resolves — deleted from configuration, its
    // harness executable now missing, a bypass acknowledgement withdrawn
    // since the original launch — is reported and skipped rather than
    // refused: `open_for_resume` has already established this session is
    // safe to continue, and a resume is not the moment to apply "refuse
    // rather than invent" as though it were a fresh launch. The user gets a
    // plain native resume and a line on stderr explaining why, not a session
    // that no longer opens at all.
    // Map line 1735: built before the gateway `resolve_resume_overlay` starts,
    // installed below beside the recorder — see `DegradeRelay`.
    let degrade_relay = DegradeRelay::new();
    let overlay_resolution = record.launch_profile.as_deref().and_then(|name| {
        match resolve_resume_overlay(
            &effective,
            &user,
            project.as_ref(),
            &selection,
            name,
            runtime,
            &record.id,
            degrade_relay.sink(),
        ) {
            Ok(resolved) => Some(resolved),
            Err(err) => {
                eprintln!(
                    "glasshouse: resuming session `{}` without launch profile `{name}`'s overlay: \
                     {err:#}",
                    crate::commands::shared::short_id(&resumable.id)
                );
                None
            }
        }
    });

    // Phase 56 line 1954 on the path that continues a session — reached by
    // `glasshouse resume` and by a launch the router steered into an existing
    // session. The profile is re-read the way `routing_destinations` reads it
    // for a recorded session, with the same fallback to the implied Native
    // profile, so the announcement names the entitlement the router weighed.
    // Announced and not gated: a session that already exists on this resource
    // is one the person asked to continue, and a resume is not the moment to
    // refuse — the comment above `overlay_resolution` says the same of an
    // overlay that no longer resolves.
    //
    // The gateway shape: `entitlement_for` still answers `None` for
    // `GlasshouseGateway` by construction, so a resumed gateway-backed
    // session reads the provider off the gateway `overlay_resolution` above
    // already started for this resume — no second gateway is started here —
    // and asks `entitlement_for_provider` instead, exactly as
    // `launch_session`'s gateway branch does. `None` when that overlay
    // itself did not resolve (reported above already): the fallback text is
    // still true then, because nothing here knows the provider either.
    let resume_entitlement = {
        let profile = record
            .launch_profile
            .as_deref()
            .and_then(|name| effective.launch_profile(name, selection.id()).ok())
            .map(|layered| layered.value)
            .unwrap_or_else(|| glasshouse::profile::LaunchProfile::native(selection.id()));
        let gateway_provider = matches!(
            profile.backend,
            glasshouse::profile::BackendResource::GlasshouseGateway
        )
        .then(|| {
            overlay_resolution
                .as_ref()
                .and_then(|(_, _, gateway)| gateway.as_ref())
        })
        .flatten()
        .map(|gateway| gateway.serving_provider());
        let lookup = match gateway_provider {
            Some(provider) => effective.entitlement_for_provider(provider),
            None => effective.entitlement_for(profile.harness, &profile.backend),
        };
        match lookup {
            Ok(entitlement) => {
                crate::commands::routing_destinations::announce_entitlement(
                    entitlement.as_ref(),
                    &profile,
                    gateway_provider,
                );
                entitlement
            }
            Err(err) => {
                eprintln!("glasshouse: {err}");
                None
            }
        }
    };

    // The response profile a fresh session opened under this record's role
    // would get today. Not the five axes stored on the record — those are a
    // `Copy` snapshot with no precedence chain attached, so replaying them
    // verbatim could not tell a project's own configuration from a session
    // preset — but the same layered resolution `launch_session` performs,
    // asked with the role this session was recorded under.
    let response_request = ResponseRequest {
        role: Some(ResponseRequest::role_for(record.role)),
        ..ResponseRequest::default()
    };
    let response_profile = effective.response_profile(&response_request);
    for problem in response_profile.problems() {
        eprintln!("glasshouse: {problem}");
    }
    let response_application =
        glasshouse::harness::response::apply(selection.adapter(), response_profile.resolved());

    let project_hooks_consent = effective.project_hooks(selection.id()).value;
    args.splice(
        0..0,
        install_session_document(
            runtime,
            &selection,
            &resumable.id,
            project_hooks_consent,
            &response_application,
        ),
    );

    let mut launch = HarnessLaunch::new(selection.into_executable(), runtime.project()).args(args);
    // Map line 1973, on the path that continues a session — the same scrub
    // `launch_session` applies, for the same reason: the child inherits this
    // process's environment, and another account's credential variable has
    // no business in it.
    for var in
        effective.foreign_entitlement_credential_vars(resume_entitlement.as_ref().map(|e| e.name()))
    {
        launch = launch.env_remove(var);
    }
    let launch = launch;
    // Both guards must outlive `session::attach` below — dropping either
    // early would delete the generated configuration file, or close the
    // gateway's loopback listener, out from under the harness that is about
    // to be pointed at them. Never read again, only held: see
    // `LaunchOverlay::install`'s own doc for why that is the whole contract.
    let (launch, _generated_guard, _gateway_guard) = match overlay_resolution {
        Some((_launch_profile, mut overlay, gateway)) => {
            let session_dir = runtime.session_dir(resumable.id.as_str());
            match overlay.install(glasshouse::harness::GeneratedConfigSite::new(&session_dir)) {
                Ok(generated) => (overlay.apply(launch), Some(generated), gateway),
                Err(err) => {
                    eprintln!(
                        "glasshouse: resuming session `{}` without its generated provider \
                         configuration: {err}",
                        crate::commands::shared::short_id(&resumable.id)
                    );
                    (launch, None, None)
                }
            }
        }
        None => (launch, None, None),
    };

    note_resume(&store, &resumable);

    // Phase 18's "record session resume events". A distinct event rather than
    // a second `SessionStarted`, because otherwise a reader has to infer a
    // resume from a session having started twice, and an inference is not a
    // recording.
    let events = Arc::new(EventRecorder::open(runtime));
    events.record(&resumable.id, LifecycleEvent::SessionResumed);

    // Map line 1735's resume half. `record` was read from the store above, so
    // its `backend_resource` is whatever the original launch resolved — which
    // is exactly the question `degrade_resource` asks of it.
    degrade_relay.install(Arc::clone(&events), vec![record.clone()]);

    // Headless is the caller's statement about *this* process, so it survives
    // a launch that the router turned into a resume — `glasshouse run
    // --headless` must not quietly take over the terminal because the best
    // destination happened to be a session that already existed.
    let attached = if headless {
        run_headless(runtime, &store, &resumable.id, launch, None)
    } else {
        session::attach(launch)
    };
    let status = match attached {
        Ok(status) => status,
        Err(err) => {
            note_lifecycle(&store, &resumable.id, SessionLifecycle::Failed);
            return Err(err);
        }
    };

    let exit = ProcessExit::from_status(&status);
    events.record(
        &resumable.id,
        LifecycleEvent::ProcessExited { exit: exit.clone() },
    );
    note_lifecycle(&store, &resumable.id, exit.session_state());

    if !status.success() {
        // A harness that refuses the identifier — "No conversation found with
        // session ID: …" is Claude Code's answer — exits non-zero, and that
        // is the honest outcome to pass on rather than dress up.
        eprintln!("glasshouse: the harness {status}");
    }
    Ok(exit_code_for(&status))
}

/// Move a session to a new state, logging rather than failing.
///
/// See the call sites: once a harness is running, Glasshouse's own record
/// keeping is not worth failing the user's session over.
pub(crate) fn note_lifecycle(
    store: &glasshouse::session::SessionStore<'_>,
    id: &glasshouse::session::SessionId,
    lifecycle: SessionLifecycle,
) {
    if let Err(err) = store.set_lifecycle(id, lifecycle) {
        tracing::warn!(session = %id, %lifecycle, error = %err, "could not record a session state change");
    }
}

/// Move a resumed session back to `Running`, logging rather than failing.
///
/// [`note_lifecycle`]'s sibling, and **not** a call to it. `set_lifecycle`
/// declines to move a finished record back to a live state, because a hook
/// process outliving its harness must not resurrect a stopped session — and
/// this write went through that same door and was refused by that same rule,
/// so a session Glasshouse had just reopened kept reading `stopped`. Every
/// hook the resumed harness then sent was discarded for arriving at a
/// session the store believed was over.
///
/// `SessionStore::begin_resume` is the door for the case where Glasshouse is
/// the one acting. It re-checks the disposition under the write lock, so a
/// record another process closed between `open_for_resume` and here is
/// refused rather than revived.
///
/// Best effort, for `note_lifecycle`'s reason: the harness is about to be
/// handed the user's conversation, and Glasshouse's own record keeping is not
/// worth failing that over. A refusal is logged at `warn` rather than
/// swallowed, because a resume that could not be recorded is the exact
/// condition this function exists to make visible.
fn note_resume(
    store: &glasshouse::session::SessionStore<'_>,
    resumable: &glasshouse::session::ResumableSession,
) {
    if let Err(err) = store.begin_resume(resumable) {
        tracing::warn!(
            session = %resumable.id,
            error = %err,
            "could not record a session resume; the session will keep reading as finished"
        );
    }
}

/// `--guardrail`'s value, or a refusal naming the three spellings.
pub(crate) fn parse_guardrail_override(value: &str) -> anyhow::Result<GuardrailOverride> {
    GuardrailOverride::from_stored(value.trim()).ok_or_else(|| {
        anyhow::anyhow!(
            "`{value}` is not a guardrail override; use one of {}",
            GuardrailOverride::spellings()
        )
    })
}

/// Translate a harness's exit into Glasshouse's own.
///
/// A session's exit status belongs to the harness, so scripts wrapping
/// Glasshouse see what they would have seen running the harness directly.
/// Two cases cannot be represented faithfully and are mapped rather than
/// faked: a process killed by a signal has no exit code of its own, and a
/// code outside a byte cannot be returned by this process at all. Both become
/// a plain failure instead of being truncated into some unrelated code — in
/// particular into a `0` that would report success.
pub(crate) fn exit_code_for(status: &ExitStatus) -> ExitCode {
    if status.success() {
        return ExitCode::SUCCESS;
    }
    if status.signal().is_some() {
        return ExitCode::FAILURE;
    }
    match u8::try_from(status.code()) {
        Ok(0) | Err(_) => ExitCode::FAILURE,
        Ok(code) => ExitCode::from(code),
    }
}
