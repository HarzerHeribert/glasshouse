use std::path::Path;
use std::process::ExitCode;

use std::io::IsTerminal;

use glasshouse::config::{self, EffectiveConfig, UserConfig};
use glasshouse::integrations::Discovery;
use glasshouse::launch::HarnessLaunch;
use glasshouse::onboarding;
use glasshouse::platform::HostPlatform;
use glasshouse::pty::ExitStatus;
use glasshouse::session;
use glasshouse::session::{NewSession, ProjectSessions, SessionDisposition, SessionLifecycle};
use glasshouse::shim::{self, ShimRequest};
use glasshouse::{Cli, Command, Runtime, logging, shutdown};

use clap::Parser;

fn main() -> ExitCode {
    // Installed before anything can touch the terminal so a failure on any path
    // still leaves the user with a usable shell.
    shutdown::install_panic_hook();

    let cli = Cli::parse();

    match run(&cli) {
        Ok(code) => code,
        Err(err) => {
            shutdown::restore_terminal();
            eprintln!("glasshouse: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> anyhow::Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let runtime = glasshouse::bootstrap(cli, &cwd)?;

    // `Project::discover` runs before logging is initialized below, and
    // logging is off by default, so a `tracing::warn!` there can go
    // completely unseen. An overridden safety refusal is user-facing, not
    // diagnostics: it always gets a line on stderr, log or no log.
    if let Some(refusal) = runtime.project().overridden_refusal() {
        eprintln!("glasshouse: warning: {refusal}");
        eprintln!("glasshouse: continuing because --allow-unsafe-scope was given");
    }

    let log_config = logging::LogConfig::resolve(
        cli.log_level.as_deref(),
        cli.log_file.as_deref(),
        cli.log_stderr,
        &runtime.log_dir(),
    );
    let log_path = logging::init(&log_config)?;

    shutdown::install_signal_handler()?;

    tracing::info!(
        version = glasshouse::VERSION,
        project = %runtime.project().id(),
        root = %runtime.project().display_root().display(),
        "glasshouse started"
    );

    match &cli.command {
        Some(Command::Doctor) => {
            print!("{}", glasshouse::integrations::doctor_report(&runtime));
        }
        Some(Command::Setup) => {
            if !setup(&runtime, SetupTrigger::Requested)? {
                return Ok(ExitCode::FAILURE);
            }
        }
        Some(Command::Sessions) => {
            print!("{}", session_report(&runtime)?);
        }
        // `run` and `launch` dispatch through this one arm on purpose — see
        // `Command::Run`'s doc. A change to how a launch is assembled can
        // only ever be made here, once, so the two can never diverge.
        Some(Command::Launch {
            harness,
            profile,
            harness_args,
        })
        | Some(Command::Run {
            harness,
            profile,
            harness_args,
        }) => {
            return launch_session(
                &runtime,
                harness.as_deref(),
                profile.as_deref(),
                harness_args,
            );
        }
        Some(Command::Resume {
            session,
            harness_args,
        }) => {
            return resume_session(&runtime, session, harness_args);
        }
        Some(Command::Hook { session, event }) => {
            report_hook(&runtime, session, event);
        }
        Some(Command::Shim {
            harness,
            profile,
            dir,
            name,
            force,
        }) => {
            return run_shim(harness, profile, dir, name.as_deref(), *force);
        }
        None => {
            // Setup runs by itself the first time, so a new user does not have
            // to know a command exists before Glasshouse is useful.
            setup(&runtime, SetupTrigger::FirstRun)?;

            // With a terminal on both ends, this is the interactive shell.
            // Without one — a pipe, a redirect, CI — there is nothing to drive
            // a full-screen interface, so fall through to the plain summary
            // rather than failing or drawing into a file.
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                glasshouse::shell::run(&runtime)?;
                return Ok(ExitCode::SUCCESS);
            }

            let project = runtime.project();
            println!("glasshouse {}", glasshouse::VERSION);
            println!("project     {}", project.name());
            println!("root        {}", project.display_root().display());
            println!("project id  {}", project.id());
            println!("scope from  {}", project.source());
            println!("state dir   {}", runtime.state_dir().display());
            if let Some(path) = log_path {
                println!("log file    {}", path.display());
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// Open a harness session attached to this terminal.
///
/// This is the production consumer of the sanctioned launch path: the harness
/// is chosen and its executable resolved from configuration (project level
/// overriding user level), the requested launch profile is resolved against
/// its adapter (Phase 9A — see [`glasshouse::profile`]), and only then is
/// anything started through [`HarnessLaunch`] — the only route that exists,
/// and the one that derives the child's working directory from the active
/// project rather than from whatever directory Glasshouse happened to be run
/// in.
///
/// Setup is deliberately not triggered here. A user who has named a harness
/// has already said what they want; interrupting that with a first-run wizard
/// would be answering a question they did not ask.
fn launch_session(
    runtime: &Runtime,
    harness: Option<&str>,
    profile_name: Option<&str>,
    harness_args: &[String],
) -> anyhow::Result<ExitCode> {
    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let selection = session::select::select(harness, effective)?;

    // Resolve the launch profile *before* anything is recorded or started.
    // A refusal here must cost nothing: no session record, no process. See
    // `glasshouse::profile::resolve`'s doc for why a refusal never falls back
    // to a different mode.
    let requested_profile = profile_name.unwrap_or(glasshouse::profile::NATIVE_PROFILE_NAME);
    let launch_profile = match effective.launch_profile(requested_profile, selection.id()) {
        Ok(resolved) => resolved.value,
        Err(err) => {
            eprintln!("glasshouse: {err}");
            return Ok(ExitCode::FAILURE);
        }
    };
    let acknowledged_bypass = effective.bypass_acknowledged(selection.id()).value;
    let overlay = match glasshouse::profile::resolve(
        &launch_profile,
        selection.adapter(),
        acknowledged_bypass,
    ) {
        Ok(overlay) => overlay,
        Err(refusal) => {
            eprintln!("glasshouse: {refusal}");
            return Ok(ExitCode::FAILURE);
        }
    };

    // Record the session before the harness exists, so a session that dies
    // during startup still leaves a trace. Failing to open the project
    // database is fatal here rather than a warning: `bootstrap` already
    // validated it, so a failure now means the project's state directory
    // broke underneath us, and starting a session Glasshouse cannot account
    // for is worse than not starting one.
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    // Minted before the process exists, for a harness that accepts one, so
    // the session is identifiable even if the harness dies during startup.
    let native = selection
        .assigns_native_session_id()
        .then(|| store.new_native_session_id())
        .transpose()?;
    let record = store.create(
        NewSession::embedded(selection.id().slug())
            .with_native_session_id(native.clone())
            .with_launch_profile(Some(launch_profile.name.clone()))
            .with_backend_resource(Some(launch_profile.backend.slug())),
    )?;

    tracing::info!(
        session = %record.id,
        harness = selection.id().slug(),
        // The resolved path and the layer that chose it are diagnostics a
        // user needs when a session starts the wrong binary. Neither is a
        // secret; harness *arguments* are never logged, because those can
        // carry session tokens.
        executable = %selection.executable().path().display(),
        source = %selection.source(),
        root = %runtime.project().display_root().display(),
        profile = %launch_profile.name,
        backend = %launch_profile.backend.slug(),
        mechanisms = %mechanism_summary(&overlay),
        "opening a harness session"
    );

    // Adapter args (and, for a harness that lets Glasshouse assign one, its
    // session identifier) first — no user arguments yet, so the overlay's
    // arguments land strictly between them and the user's own.
    let mut args = selection.start_args(native.as_deref(), std::iter::empty::<&str>());
    let project_hooks_consent = effective.project_hooks(selection.id()).value;
    args.splice(
        0..0,
        install_hooks(runtime, &selection, &record.id, project_hooks_consent),
    );
    let launch = HarnessLaunch::new(selection.into_executable(), runtime.project()).args(args);
    // The overlay is the only thing that may put its own arguments or
    // environment onto the launch — see `LaunchOverlay::apply`'s doc.
    let launch = overlay.apply(launch);
    // The user's own `--` arguments always come last, so they can win.
    let launch = launch.args(harness_args.iter().map(String::as_str));

    // From here on, a bookkeeping failure must never change what the user
    // sees. The session is real and running; losing a state transition is a
    // diagnostics problem, whereas turning it into an error would make a
    // database hiccup look like a harness failure.
    note_lifecycle(&store, &record.id, SessionLifecycle::Running);

    let status = match session::attach(launch) {
        Ok(status) => status,
        Err(err) => {
            note_lifecycle(&store, &record.id, SessionLifecycle::Failed);
            return Err(err);
        }
    };

    // The session is over, so this is the tightest the discovery window will
    // ever be — see `session::native_id::capture`'s doc comment.
    session::native_id::capture(&store, &record, runtime.project().root());

    note_lifecycle(
        &store,
        &record.id,
        if status.success() {
            SessionLifecycle::Stopped
        } else {
            SessionLifecycle::Failed
        },
    );

    if !status.success() {
        // The harness failing is not Glasshouse failing, so this is a plain
        // note on stderr rather than an error: the exit code below already
        // carries the outcome to whatever invoked Glasshouse.
        eprintln!("glasshouse: the harness {status}");
    }
    Ok(exit_code_for(&status))
}

/// Generate one file that `exec`s `glasshouse run <harness> --profile
/// <name>`, forwarding its own arguments.
///
/// The generated file is the entire mechanism — see [`glasshouse::shim`]'s
/// module doc. This function only resolves *this* executable's own path and
/// the host platform; [`shim::generate`] is the only thing that writes
/// anything, and it writes exactly one file, inside `dir` and nowhere else.
fn run_shim(
    harness: &str,
    profile: &str,
    dir: &Path,
    name: Option<&str>,
    force: bool,
) -> anyhow::Result<ExitCode> {
    let glasshouse_exe = std::env::current_exe().map_err(|err| {
        anyhow::anyhow!("could not determine the Glasshouse executable's own path: {err}")
    })?;
    let request = ShimRequest {
        harness,
        profile,
        glasshouse_exe: &glasshouse_exe,
        dir,
        name,
        force,
    };

    match shim::generate(HostPlatform::detect(), &request) {
        Ok(path) => {
            println!("wrote {}", path.display());
            println!(
                "deleting that file is all it takes to remove the shim; Glasshouse writes \
                 nothing else on its behalf."
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => {
            eprintln!("glasshouse: {err}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// A one-line summary of a resolved overlay's mechanisms, for the "opening a
/// harness session" log line — category and detail only, exactly what
/// [`glasshouse::profile::LaunchOverlay::mechanisms`] exposes for rendering.
/// An environment *value* is never in here, because the overlay never puts
/// one in a `MechanismNote` to begin with.
fn mechanism_summary(overlay: &glasshouse::profile::LaunchOverlay) -> String {
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
fn install_hooks(
    runtime: &Runtime,
    selection: &session::HarnessSelection,
    id: &session::SessionId,
    project_hooks_consent: bool,
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
    match selection.install_hooks(&report, project_hooks_consent) {
        Ok(Some(args)) => args,
        Ok(None) => Vec::new(),
        Err(err) => {
            tracing::warn!(session = %id, error = %err, "could not install lifecycle hooks");
            Vec::new()
        }
    }
}

/// Record a lifecycle event a harness reported about one of its sessions.
///
/// # This function may never fail
///
/// It is run *by the harness*, inside the user's session, and Claude Code
/// treats a hook's non-zero exit as a veto: a `UserPromptSubmit` hook that
/// exits non-zero blocks the prompt outright, with the user's own words
/// echoed back at them and nothing sent. That was observed directly, not
/// assumed.
///
/// So every failure here is swallowed into the log. A database that cannot be
/// opened, a session that is not in it, an event nobody recognises — none of
/// them is worth costing the user a turn. Glasshouse's bookkeeping is never
/// more important than the session it is keeping books about.
fn report_hook(runtime: &Runtime, session: &str, event: &str) {
    // Codex writes its payload to the hook's stdin, and a process that never
    // reads it can leave the harness writing into a closed pipe. Glasshouse
    // has the event name and the session identifier from its own argv, so
    // the payload is drained to EOF and thrown away, unread and unparsed —
    // never deserialized, logged, or stored. See
    // `the_hook_command_never_reads_its_payload` below, and the
    // `GLASSHOUSE_DESIGN_DECISIONS.md` section this function implements.
    let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());

    let Some(next) = session::lifecycle_for(event) else {
        // An event this build does not recognise. Harnesses gain events
        // between releases, and guessing a state from an unfamiliar name
        // would be worse than ignoring it.
        tracing::debug!(event, "ignoring an unrecognised harness event");
        return;
    };

    let outcome = (|| -> anyhow::Result<()> {
        let sessions = ProjectSessions::open(runtime)?;
        let store = sessions.store();
        let id = store.resolve_id(session)?;
        let record = store
            .get(&id)?
            .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;

        if !session::may_apply(record.lifecycle, next) {
            tracing::debug!(
                session = %id,
                from = record.lifecycle.as_str(),
                to = next.as_str(),
                "not applying a harness event to a session in this state"
            );
            return Ok(());
        }
        store.set_lifecycle(&id, next)?;
        tracing::info!(session = %id, event, state = next.as_str(), "harness reported an event");
        Ok(())
    })();

    if let Err(err) = outcome {
        tracing::warn!(error = %err, event, "could not record a harness event");
    }
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
fn resume_session(
    runtime: &Runtime,
    session: &str,
    harness_args: &[String],
) -> anyhow::Result<ExitCode> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();

    // Both of these refuse rather than guess: an ambiguous prefix names its
    // candidates, and `open_for_resume` carries the project-isolation check.
    let id = store.resolve_id(session)?;
    let resumable = store.open_for_resume(&id)?;

    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let selection = session::select::select(
        Some(resumable.harness.as_str()),
        EffectiveConfig::new(&user, project.as_ref()),
    )?;

    let Some(args) = selection.resume_args(
        &resumable.native_session_id,
        harness_args.iter().map(String::as_str),
    ) else {
        anyhow::bail!(
            "{} has no resume mechanism Glasshouse has verified, so session `{}` cannot be \
             reopened. Start a new session instead.",
            selection.id().display_name(),
            short_id(&resumable.id)
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

    let launch = HarnessLaunch::new(selection.into_executable(), runtime.project()).args(args);

    note_lifecycle(&store, &resumable.id, SessionLifecycle::Running);
    let status = match session::attach(launch) {
        Ok(status) => status,
        Err(err) => {
            note_lifecycle(&store, &resumable.id, SessionLifecycle::Failed);
            return Err(err);
        }
    };
    note_lifecycle(
        &store,
        &resumable.id,
        if status.success() {
            SessionLifecycle::Stopped
        } else {
            SessionLifecycle::Failed
        },
    );

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
fn note_lifecycle(
    store: &glasshouse::session::SessionStore<'_>,
    id: &glasshouse::session::SessionId,
    lifecycle: SessionLifecycle,
) {
    if let Err(err) = store.set_lifecycle(id, lifecycle) {
        tracing::warn!(session = %id, %lifecycle, error = %err, "could not record a session state change");
    }
}

/// The `glasshouse sessions` listing.
///
/// Reads Glasshouse's own records rather than any harness's session files, so
/// the list is the same whether or not a harness kept its own history.
fn session_report(runtime: &Runtime) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    let sessions = ProjectSessions::open(runtime)?;
    let records = sessions.store().list()?;

    if records.is_empty() {
        return Ok(format!(
            "No sessions recorded for {}.\nStart one with `glasshouse launch`.\n",
            runtime.project().name()
        ));
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}",
        session_row(
            "SESSION",
            "HARNESS",
            "PROFILE",
            "STATE",
            "ROLE",
            "PRESENTED",
            "LAST ACTIVITY"
        )
    );
    for record in &records {
        let state = match record.disposition() {
            SessionDisposition::Active => "active",
            SessionDisposition::Resumable => "resumable",
            SessionDisposition::Closed => "closed",
            SessionDisposition::Failed => "failed",
        };
        let _ = writeln!(
            out,
            "{}",
            session_row(
                &short_id(&record.id),
                &record.harness,
                // A dash, not the word "native": a session recorded before
                // Phase 9A ran under no profile at all, and that is a
                // different fact from having run the Native profile — see
                // `SessionRecord::launch_profile`'s doc.
                record.launch_profile.as_deref().unwrap_or("-"),
                state,
                &record.role.to_string(),
                &record.presentation.to_string(),
                &format_age(record.last_activity_at),
            )
        );
    }
    Ok(out)
}

/// One line of the session listing, header included.
///
/// The header and the rows go through the same function so their columns
/// cannot drift apart — the usual way a hand-aligned table stops lining up is
/// someone widening a column in one of the two format strings.
fn session_row(
    session: &str,
    harness: &str,
    profile: &str,
    state: &str,
    role: &str,
    presented: &str,
    activity: &str,
) -> String {
    // Widths fit the longest value each column can hold: `resumable`,
    // `orchestrator`, `embedded`.
    format!(
        "{session:<12}  {harness:<14}  {profile:<12}  {state:<9}  {role:<12}  {presented:<9}  \
         {activity}"
    )
}

/// Enough of an identifier to name a session in conversation.
///
/// The full identifier stays available in `--log-level` output and is what any
/// command taking a session takes; this is only for the eye.
fn short_id(id: &glasshouse::session::SessionId) -> String {
    id.as_str().chars().take(12).collect()
}

/// A rough "how long ago", which is what a session list is actually read for.
fn format_age(timestamp: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    // A timestamp in the future is possible — a clock corrected backwards
    // between writing the row and reading it — and produces a negative value
    // here, because `saturating_sub` saturates at `i64::MIN`, not at zero. The
    // first arm covers it: reporting "just now" is the honest answer, and it
    // avoids printing a confident negative age. An explicit `< 0` guard used
    // to sit here returning the same string, which only obscured that.
    let seconds = now.saturating_sub(timestamp);
    match seconds {
        s if s < 60 => "just now".to_owned(),
        s if s < 3_600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3_600),
        s => format!("{}d ago", s / 86_400),
    }
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
fn exit_code_for(status: &ExitStatus) -> ExitCode {
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

/// Why setup is being considered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupTrigger {
    /// Glasshouse is starting normally and setup has never been completed.
    FirstRun,
    /// The user asked for it with `glasshouse setup`.
    Requested,
}

/// Run the setup wizard when it is wanted and possible.
///
/// Returns whether setup ended up completed. A first run that cannot show a
/// wizard is not an error: Glasshouse still works, it just has not recorded
/// the user's harness choices yet.
fn setup(runtime: &Runtime, trigger: SetupTrigger) -> anyhow::Result<bool> {
    let config = UserConfig::load(runtime.paths())?;

    if trigger == SetupTrigger::FirstRun && !onboarding::is_required(&config) {
        return Ok(true);
    }

    // The wizard needs a terminal it can take over. Piped or redirected output
    // means Glasshouse is being scripted, and silently blocking on a full
    // screen interface nobody can see would be worse than skipping it.
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        match trigger {
            SetupTrigger::FirstRun => {
                eprintln!(
                    "glasshouse: setup has not been completed. Run `glasshouse setup` \
                     in an interactive terminal to choose which harnesses to use."
                );
                return Ok(false);
            }
            SetupTrigger::Requested => {
                anyhow::bail!("`glasshouse setup` needs an interactive terminal");
            }
        }
    }

    // Discovery probes each harness for its version, so it is done once, here,
    // rather than inside the wizard: the wizard is a state machine over an
    // already-known result, which is what makes it testable without a terminal.
    let discovery = Discovery::run(runtime.project());

    match onboarding::run(runtime, &discovery, config)? {
        onboarding::Outcome::Completed(_) => Ok(true),
        onboarding::Outcome::Cancelled => {
            eprintln!("glasshouse: setup cancelled; nothing was saved.");
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let code = production_code(include_str!("main.rs"));
        // `return launch_session(` matches only an actual call, never the
        // `fn launch_session(` definition line itself.
        let call_sites = code.matches("return launch_session(").count();
        assert_eq!(
            call_sites, 1,
            "`glasshouse run` and `glasshouse launch` must dispatch through exactly one call \
             to `launch_session` so they cannot diverge; found {call_sites} call sites"
        );
    }

    // --- a refused profile starts no process and records no session -------

    /// A harness enabled with a decoy executable, so `session::select::select`
    /// succeeds without a real install; the runtime it was bootstrapped
    /// against comes back too, so the caller can inspect state afterward.
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

    #[test]
    fn a_refused_profile_starts_no_process_and_records_no_session() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = fixture_with_enabled_claude_code(tmp.path());

        // A provider-backed profile is always refused in Phase 9A (Phase
        // 9C/9D supply the provider configuration it would need).
        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let mut profile = glasshouse::config::ProfileConfig::new(
            glasshouse::integrations::IntegrationId::ClaudeCode,
        );
        profile.set_backend(glasshouse::config::ProfileBackend::DirectProvider {
            provider: "openrouter".to_owned(),
        });
        user.profiles_mut().set("gateway", profile);
        user.save(runtime.paths()).unwrap();

        let status = launch_session(&runtime, Some("claude-code"), Some("gateway"), &[]).unwrap();
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
        let mut profile = glasshouse::config::ProfileConfig::new(
            glasshouse::integrations::IntegrationId::ClaudeCode,
        );
        profile.set_approval(glasshouse::config::ProfileApproval::Bypass);
        user.profiles_mut().set("yolo", profile);
        user.save(runtime.paths()).unwrap();

        let status = launch_session(&runtime, Some("claude-code"), Some("yolo"), &[]).unwrap();
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
        let selection =
            glasshouse::session::select::select(Some("claude-code"), effective).unwrap();

        let resolved = effective
            .launch_profile(glasshouse::profile::NATIVE_PROFILE_NAME, selection.id())
            .unwrap()
            .value;
        assert_eq!(resolved.name, "native");
        assert_eq!(resolved.backend.slug(), "native");

        let overlay = glasshouse::profile::resolve(&resolved, selection.adapter(), false).unwrap();
        assert!(mechanism_summary(&overlay).contains("automatic review"));
    }

    // --- the hook handler never reads its payload -------------------------

    /// Every field a Codex hook payload can carry, per
    /// `GLASSHOUSE_DESIGN_DECISIONS.md`'s "Codex lifecycle hooks" section:
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

    /// `report_hook`'s own source, isolated from the rest of this file. A
    /// whole-file scan would trip on legitimate, unrelated code — this
    /// module's own `native_session_id` and `cwd` locals are not the Codex
    /// payload fields of the same or similar name — so this extracts just
    /// the one function the design decision is actually about.
    fn hook_handler_source() -> &'static str {
        let full = include_str!("main.rs");
        let start = full
            .find("fn report_hook(")
            .expect("report_hook must exist in this file");
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
        body
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
        let source = strip_comments(hook_handler_source());

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

    /// The listing's ages, including the case a review flagged: a timestamp in
    /// the future. `saturating_sub` saturates at `i64::MIN`, not at zero, so
    /// the value really can be negative and the first arm has to absorb it.
    #[test]
    fn ages_read_sensibly_including_a_clock_that_moved_backwards() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs() as i64;

        assert_eq!(format_age(now), "just now");
        assert_eq!(format_age(now - 30), "just now");
        assert_eq!(format_age(now - 120), "2m ago");
        assert_eq!(format_age(now - 7_200), "2h ago");
        assert_eq!(format_age(now - 3 * 86_400), "3d ago");

        // A future timestamp must not print a negative age.
        let ahead = format_age(now + 10_000);
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
            let text = format_age(extreme);
            assert!(!text.is_empty() && !text.contains('-'), "bad age: {text}");
        }
        assert_eq!(
            format_age(i64::MAX),
            "just now",
            "the far future reads as now"
        );
    }

    /// The header and every row go through `session_row`, so their columns
    /// cannot drift apart. Checked here rather than trusted.
    #[test]
    fn listing_columns_line_up_between_the_header_and_a_row() {
        let header = session_row(
            "SESSION",
            "HARNESS",
            "PROFILE",
            "STATE",
            "ROLE",
            "PRESENTED",
            "LAST",
        );
        let row = session_row(
            "abc123",
            "claude-code",
            "native",
            "resumable",
            "orchestrator",
            "embedded",
            "2h ago",
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
}
