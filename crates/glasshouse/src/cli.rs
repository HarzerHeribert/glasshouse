//! Command-line surface.
//!
//! Bare `glasshouse` operates on the current project. Every option here is
//! global because Glasshouse is project scoped: the project must be resolved
//! before any subcommand can do anything.

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

const AFTER_HELP: &str = "\
ENVIRONMENT:
  GLASSHOUSE_DATA_DIR    Override the per-user application-data directory.
  GLASSHOUSE_CONFIG_DIR  Override the per-user configuration directory.
  GLASSHOUSE_LOG         Enable logging with a tracing filter, e.g. `debug`.

PROJECT SCOPE:
  Glasshouse operates on exactly one project root. The root is the containing
  Git repository when there is one, otherwise the current directory. All state,
  sessions, and memory are isolated per project root.
";

#[derive(Debug, Parser)]
#[command(
    name = "glasshouse",
    version,
    about = "A lean, project-scoped control plane for native coding-agent harnesses.",
    after_help = AFTER_HELP,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Select the project root explicitly instead of discovering it from Git.
    #[arg(long, value_name = "PATH", global = true)]
    pub scope: Option<PathBuf>,

    /// Permit a project root that Glasshouse would normally refuse.
    #[arg(long, global = true)]
    pub allow_unsafe_scope: bool,

    /// Override the per-user application-data directory.
    #[arg(long, value_name = "PATH", global = true)]
    pub data_dir: Option<PathBuf>,

    /// Override the per-user configuration directory.
    #[arg(long, value_name = "PATH", global = true)]
    pub config_dir: Option<PathBuf>,

    /// Enable logging at a tracing filter level, e.g. `info` or `glasshouse=debug`.
    #[arg(long, value_name = "FILTER", global = true)]
    pub log_level: Option<String>,

    /// Write logs to this file instead of the project log file.
    #[arg(long, value_name = "PATH", global = true, value_parser = parse_log_file)]
    pub log_file: Option<PathBuf>,

    /// Write logs to stderr. Not usable while the interactive TUI is running.
    #[arg(long, global = true, conflicts_with = "log_file")]
    pub log_stderr: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// clap value parser for `--log-file`: refuses a literal `~`, the same
/// hazard already guarded for `--data-dir`/`--config-dir` and their
/// environment twins in [`crate::paths::reject_literal_tilde`] — there is no
/// shell in this argument's path to expand it.
fn parse_log_file(value: &str) -> Result<PathBuf, String> {
    crate::paths::reject_literal_tilde(Path::new(value), "--log-file")
        .map_err(|err| err.to_string())
}

/// Non-interactive commands.
///
/// Every one of these is project scoped: it operates on the project resolved
/// from the working directory or `--scope`, never across projects.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print a concise project and resource summary.
    ///
    /// One screenful: the project identity, how many harnesses are usable,
    /// how many sessions are recorded and which one was active most
    /// recently, and how many model resources Glasshouse can describe.
    ///
    /// This composes what `doctor`, `sessions` and `resources` already
    /// compute rather than re-deriving any of it, and it never goes deeper
    /// than a count — a harness's setup problems, a session's seven facts,
    /// and a resource's quota detail are what those three commands are for.
    Status,
    /// Show the entitlement pool — every configured account, one row each.
    ///
    /// Map line 1972's inspectable view. Per entitlement: how much of its
    /// allowance is left, when that allowance resets, what throttling has
    /// recently been observed against it, and what it has actually served —
    /// the sessions this project recorded against that account.
    ///
    /// Every configured entitlement appears, including one nothing has ever
    /// measured, which reads `unknown` on each facet it has no reading for.
    /// `unknown` is a rendered word and never a number: an account no
    /// telemetry describes is not a full one and it is not an empty one.
    ///
    /// Accounts are named, never their credentials. An entitlement's
    /// authentication is a reference into the operating system's own secret
    /// storage, and nothing here resolves one.
    Entitlements,
    /// Report detected harnesses, optional integrations, and setup problems.
    Doctor,
    /// Reopen the first-run setup wizard.
    ///
    /// Setup runs by itself the first time Glasshouse is used in an
    /// interactive terminal; this is how to revisit those choices later.
    Setup,
    /// Report what Glasshouse believes about each harness-and-model pairing.
    ///
    /// Who publishes a harness, who developed a model, and who serves that
    /// model are three different questions, and a router that ran them
    /// together would treat a reseller as an author. This prints them apart,
    /// with the class each pairing falls into and the evidence behind it.
    ///
    /// Anything Glasshouse has not read stays `unknown` — a model named
    /// after a company is not evidence it was made there. Correct one with a
    /// `[pairing.models."<model id>"]` table in the configuration file; the
    /// next run reflects it, and no router code changes.
    Pairing {
        /// Ask about one model by its exact id, whether or not any launch
        /// profile names it.
        #[arg(long, value_name = "ID")]
        model: Option<String>,

        /// Narrow `--model` to one harness, by its identifier — for example
        /// `claude-code`.
        #[arg(long, value_name = "ID")]
        harness: Option<String>,
    },
    /// Report the response profile in effect and what each harness would do
    /// with it.
    ///
    /// A response profile governs how an answer reads — its verbosity,
    /// audience, progress narration, evidence presentation and final-answer
    /// format — and nothing else. It cannot change reasoning depth,
    /// diligence, validation, permissions, safety or tool use, and no profile
    /// can use concision to reduce what is reported.
    ///
    /// This prints the resolved profile with the precedence layer each of the
    /// five axes came from, the reports no profile may drop, and the native
    /// mechanism, additive instruction or refusal each harness would answer
    /// with. Record one with a `[response]` table in the configuration file;
    /// the next run reflects it.
    ///
    /// Deliberately not called `profile`: `--profile` already names a
    /// *launch* profile, and the two are separate things that a shared name
    /// would collapse.
    Response {
        /// Resolve for one role — `orchestrator`, `worker`, `reviewer`,
        /// `explainer`, or `interactive`. Absent means `interactive`.
        #[arg(long, value_name = "ROLE")]
        role: Option<String>,

        /// A preset asked for at the session layer, as `glasshouse launch
        /// --response-profile` would.
        #[arg(long, value_name = "NAME")]
        session: Option<String>,

        /// Override one axis for this task, above every other layer.
        #[arg(long, value_name = "VALUE")]
        verbosity: Option<String>,

        /// Override the intended audience for this task.
        #[arg(long, value_name = "VALUE")]
        audience: Option<String>,

        /// Override progress narration for this task.
        #[arg(long, value_name = "VALUE")]
        narration: Option<String>,

        /// Override evidence presentation for this task.
        #[arg(long, value_name = "VALUE")]
        evidence: Option<String>,

        /// Override the final-answer format for this task.
        #[arg(long, value_name = "VALUE")]
        format: Option<String>,
    },
    /// Report what Glasshouse knows about each model resource's quota, and
    /// where it learned it.
    ///
    /// Lists every resource Glasshouse can describe — each harness's own
    /// subscription, every configured provider and local server, and the
    /// gateway — with the shape of its quota, what can exhaust it, and for
    /// every capacity number whether it is authoritative, observed,
    /// estimated, manual, or unknown.
    ///
    /// **A value Glasshouse never read prints as `unknown`, and never as
    /// zero or as full.** Most of them are unknown today, and that is the
    /// answer rather than a gap: a subscription's remaining allowance is
    /// published by nobody, and an inferred figure presented as a measurement
    /// is worse than no figure at all. An estimate is always marked as one.
    ///
    /// Makes no network request unless `--probe` names a provider. It does
    /// ask each installed harness for its own status, which is a local
    /// command costing no quota, and it reads the plan out of the answer and
    /// nothing else.
    ///
    /// Where a provider publishes nothing, record what you know in a
    /// `[providers.<name>.quota]` table — a `plan`, a `budget`, or a
    /// `stale_after_seconds`. Those are read as `manual`, and a provider's
    /// own word always outranks them.
    Resources {
        /// Show every pool, window and rate ceiling, including the ones
        /// nothing is known about — the debug view.
        #[arg(long)]
        verbose: bool,

        /// Make one request to this configured provider and read the
        /// rate-limit headers it answers with. Repeatable.
        ///
        /// The request goes to the same model-list URL `glasshouse doctor`
        /// already probes, so it costs one catalogue read and no inference.
        #[arg(long, value_name = "PROVIDER")]
        probe: Vec<String>,

        /// Do not run any harness's status command.
        #[arg(long)]
        no_harness: bool,
    },
    /// Report what Glasshouse believes a request needs, without acting on it.
    ///
    /// Reads the words as free-form request text — not a query language —
    /// and answers whether it needs repository context, code modification, a
    /// shell, or browser interaction; a coarse complexity and workload tier;
    /// whether it looks safe to hand to a disposable free or local model; and
    /// how confident the classification is, so an uncertain answer can be
    /// escalated rather than trusted outright.
    ///
    /// Asks the configured routing model when one is pinned or resolved
    /// automatically; falls back to the deterministic heuristic path when
    /// none is configured, or when the model call fails. Either way the
    /// report says which one actually answered.
    Classify {
        /// The request text to classify.
        text: Vec<String>,
    },
    /// Show where Glasshouse would send this work, and why.
    ///
    /// Ranks every destination — the sessions this project already has and a
    /// fresh one per launch profile — and prints the contributions behind the
    /// winner, every alternative's score, and anything a hard constraint
    /// removed. Decides nothing and starts nothing.
    ///
    /// The same ranking runs on `glasshouse launch` and `glasshouse run`,
    /// which do start something; this is how to see the answer first. The one
    /// difference is stated in the report: a session that is still *running*
    /// is a destination a person can act on and not one a second process can
    /// enter, so it is ranked here and left out there.
    Route {
        /// `session-start`, `task-boundary`, or `mid-turn`.
        #[arg(long, value_name = "MOMENT", default_value = "session-start")]
        moment: String,
        /// Send the work to this destination whatever the ranking says.
        #[arg(long, value_name = "ID")]
        to: Option<String>,
        /// Start a fresh session whatever the ranking says.
        #[arg(long, conflicts_with = "to")]
        fresh: bool,
        /// Decide now, even mid-turn.
        #[arg(long)]
        now: bool,
        /// Describe the work, so the ranking can weigh what it actually
        /// needs (repository access, shell execution, browser interaction)
        /// instead of nothing. Optional; omitting it leaves routing exactly
        /// as it behaves today.
        #[arg(long, value_name = "TEXT")]
        task: Option<String>,
    },
    /// List the sessions Glasshouse has recorded for this project, or act on
    /// one of them.
    ///
    /// Glasshouse keeps its own record of every session it starts, separate
    /// from whatever session files the harness writes for itself, so the list
    /// is the same whether or not a harness kept its own history.
    ///
    /// With no subcommand this prints the list, which is what it has always
    /// done. `show` prints everything one session recorded; `rename`, `tag`
    /// and `close` are the three things a person can change about a record,
    /// and none of them touches the harness's own history.
    Sessions {
        #[command(subcommand)]
        command: Option<SessionCommand>,
    },
    /// Search this project's durable memory.
    ///
    /// Memory is project-scoped: this reads the database belonging to the
    /// project you are standing in, and there is no way to ask it for
    /// another project's knowledge.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Take, list, or read a portable session checkpoint.
    ///
    /// A checkpoint is what one session hands the next when work has to
    /// move: the objective, where it got to, what was already ruled out, and
    /// what to do next. It is deliberately small, it is project-scoped, and
    /// it is kept apart from this project's durable memory.
    ///
    /// Glasshouse fills in the session, the harness, the timestamp and the
    /// Git position by itself. It does not invent the objective or the state
    /// — those are yours, and a checkpoint whose objective Glasshouse had
    /// guessed would be worse than no checkpoint at all.
    Checkpoint {
        #[command(subcommand)]
        command: CheckpointCommand,
    },
    /// Report a harness lifecycle event. Run by harnesses, not by people.
    ///
    /// Glasshouse installs hooks that invoke this command, so a session's
    /// state comes from the harness saying what happened rather than from
    /// Glasshouse reading its terminal and guessing.
    #[command(hide = true)]
    Hook {
        /// The Glasshouse session the event belongs to.
        #[arg(long)]
        session: String,

        /// The harness's own name for the event.
        #[arg(long)]
        event: String,
    },
    /// Resume a recorded session in its own harness, inside this project.
    ///
    /// Only a session this project recorded, and only one that has something
    /// to resume to: a harness that never produced an identifier, or one that
    /// is still running, is refused rather than reopened as something blank.
    Resume {
        /// Which session, by the identifier `glasshouse sessions` prints.
        ///
        /// The listing shows the first twelve characters, and that short form
        /// is enough — any leading part of an identifier works, as long as it
        /// picks out exactly one session.
        session: String,

        /// Check point the session this work is leaving, before it moves —
        /// capability map line 1716.
        ///
        /// The resume half of `glasshouse launch --checkpoint-first`, and it
        /// means the same thing: if resuming this session moves the work out
        /// of the one this project was most recently in, that session gets a
        /// checkpoint first. Resuming the session you were already in leaves
        /// nothing, and says so.
        #[arg(long)]
        checkpoint_first: bool,

        /// Arguments passed straight through to the harness, after `--`.
        #[arg(last = true, allow_hyphen_values = true)]
        harness_args: Vec<String>,
    },
    /// Open a session in an installed harness, inside this project.
    ///
    /// The harness runs in a pseudo-terminal whose working directory is this
    /// project's root, attached directly to the current terminal: its own
    /// interface, its own key bindings, its own session. Glasshouse starts it
    /// and stays out of the way.
    Launch {
        /// Which harness to open, by its identifier — for example
        /// `claude-code`, `codex`, or `opencode`.
        ///
        /// Optional when exactly one harness is enabled. With several
        /// enabled, Glasshouse asks rather than guessing.
        harness: Option<String>,

        /// Open the session with this response profile, by preset name.
        ///
        /// This is the session layer of the response-profile precedence
        /// chain: it wins over the role's default, this project's
        /// configuration and your user default, and loses to nothing but a
        /// task override. `glasshouse response --session <name>` prints
        /// exactly what a session started this way would resolve to.
        #[arg(long, value_name = "NAME")]
        response_profile: Option<String>,

        /// Open the session in this role, for response-profile purposes —
        /// `orchestrator`, `worker`, `reviewer`, `explainer`, or
        /// `interactive`.
        ///
        /// A spawned worker never inherits a communication style from
        /// whatever started it: its profile is resolved here, explicitly, and
        /// recorded in the launch log.
        #[arg(long, value_name = "ROLE")]
        response_role: Option<String>,

        /// Which launch profile to resolve the session through.
        ///
        /// Names a profile configured in `.glasshouse/config.toml` or the
        /// user-level configuration file — see `glasshouse setup`. Absent
        /// means the selected harness's implied Native profile, which uses
        /// the harness's own first-party authentication and configuration
        /// unchanged.
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,

        /// Start the harness with a stored checkpoint's handoff as its
        /// opening prompt.
        ///
        /// Takes the identifier `glasshouse checkpoint list` prints, or
        /// `latest` for the most recent one in this project. The prompt is
        /// plain text that names no harness, so a checkpoint written while
        /// one harness was running can start the work in another.
        ///
        /// It is appended as the harness's own trailing argument, which is
        /// exactly what typing the prompt after `--` would do — so a harness
        /// that does not take an opening prompt reports that itself, in its
        /// own words, rather than having Glasshouse guess on its behalf.
        /// Your own `--` arguments still come after it and still win.
        #[arg(long, value_name = "ID")]
        from_checkpoint: Option<String>,

        /// Continue this destination whatever the ranking says, by the
        /// identifier `glasshouse route` prints.
        ///
        /// A recorded session's identifier continues that session; a
        /// `fresh:<harness>:<profile>` identifier starts a new one under that
        /// profile.
        #[arg(long, value_name = "ID")]
        to: Option<String>,

        /// Start a fresh session whatever the ranking says.
        ///
        /// Glasshouse prefers a relevant session this project already has
        /// when its affinity outweighs starting over. This is how to say no
        /// to that, once, without changing any configuration.
        #[arg(long, conflicts_with = "to")]
        fresh: bool,

        /// Take no routing decision for this launch — capability map line
        /// 1712.
        ///
        /// Glasshouse normally ranks this project's warm sessions against a
        /// new one and continues the best of them. This turns that off for
        /// one launch: what `--to`, `--fresh` and `--profile` say still
        /// happens, and with none of them the session is a new one under the
        /// profile this launch would have used anyway.
        ///
        /// The same switch, standing, is `automatic = false` under
        /// `[routing]` in this project's or your own configuration. A launch
        /// with routing off does **not** compute the ranking in order to tell
        /// you what it would have chosen — see `glasshouse route`, which
        /// answers that question without starting anything.
        #[arg(long)]
        no_routing: bool,

        /// Check point the session this work is leaving, before it moves —
        /// capability map line 1716.
        ///
        /// Only does something when this launch continues a session that is
        /// not the one this project was most recently working in: that is
        /// what "leaving" means, and it is the only case where anything could
        /// be lost. A fresh launch, or one that continues the session you
        /// were already in, says that no checkpoint was needed rather than
        /// writing one nobody can use.
        ///
        /// The checkpoint records what Glasshouse knows — which session was
        /// left, where the work went, this project's Git position and its
        /// binding memories — and invents no objective from the session's
        /// terminal. See `glasshouse checkpoint save` for why.
        #[arg(long)]
        checkpoint_first: bool,

        /// Run the session with no terminal of its own.
        ///
        /// The harness still runs in a real pseudo-terminal, inside this
        /// project's root, with its output captured — it simply never takes
        /// over the terminal you started it from, and Glasshouse records it
        /// as a headless session. Useful for a harness given its whole task
        /// on the command line, and for starting one from a terminal you
        /// want to keep.
        ///
        /// Glasshouse stays in the foreground until the harness exits: there
        /// is no daemon behind this, and a session whose parent went away
        /// would lose the pseudo-terminal it is reading from.
        #[arg(long)]
        headless: bool,

        /// Describe the work, so the destination is chosen for what it
        /// actually needs.
        ///
        /// With a routing model configured, Glasshouse classifies the task
        /// through it before deciding where the work goes — never sending
        /// repository files, transcripts or secrets, only the task and a few
        /// facts about the session — and falls back to deterministic
        /// heuristics when none is configured or it does not answer. `--to`
        /// and `--fresh` decide on their own and ask no model. Omitting
        /// `--task` leaves the launch exactly as it has always been.
        #[arg(long, value_name = "TEXT")]
        task: Option<String>,
        /// Force, skip, or lower the assumption guardrail for this task —
        /// `force`, `skip` or `lower`.
        ///
        /// Recorded on the new session's assumption ledger before the
        /// harness starts, so every preflight the agent runs in it answers
        /// under this override and names it. `skip` records a
        /// `waived_by_user` row saying you waived the gate; `force` gates
        /// every substantial change whatever `guardrails.mode` says; `lower`
        /// keeps a substantial change advisory and lets an ordinary one
        /// proceed. Trivial edits never gate either way.
        #[arg(long, value_name = "force|skip|lower")]
        guardrail: Option<String>,
        /// Present the session in an external backend instead of this
        /// terminal. `cmux` is the only backend today.
        ///
        /// With cmux available — Glasshouse is running inside a cmux
        /// surface and `cmux ping` answers — Glasshouse opens a new cmux
        /// workspace in the project root and runs this same launch inside
        /// it. The session is recorded as `external` with the workspace it
        /// lives in, and `glasshouse sessions focus` brings it to the front.
        /// Without cmux, the launch says so and runs embedded, exactly as it
        /// would have without this flag.
        #[arg(long, value_name = "BACKEND", conflicts_with = "presentation_ref")]
        presentation: Option<String>,

        /// Record this session as presented in an external pane. The
        /// process inside a pane opened by `--presentation` passes this.
        ///
        /// Takes a cmux reference (`workspace:<n>` or `surface:<n>`), or
        /// `caller`, which asks cmux which workspace this process is in. It
        /// changes nothing about how the session runs; it only records
        /// where it is shown.
        #[arg(long, value_name = "REF", hide = true)]
        presentation_ref: Option<String>,

        /// Arguments passed straight through to the harness, after `--`.
        ///
        /// Glasshouse does not interpret these; `glasshouse launch
        /// claude-code -- --resume` starts the harness with `--resume`.
        #[arg(last = true, allow_hyphen_values = true)]
        harness_args: Vec<String>,
    },
    /// Open a session exactly like `launch`, under the name a generated shim
    /// expects.
    ///
    /// This is not a second launch path: it dispatches through the same
    /// code as `launch`, so the two can never come to behave differently. It
    /// exists only because a shim (`glasshouse shim`) needs a stable
    /// subcommand to `exec` into.
    Run {
        /// Which harness to open, by its identifier — for example
        /// `claude-code`, `codex`, or `opencode`.
        ///
        /// Optional when exactly one harness is enabled. With several
        /// enabled, Glasshouse asks rather than guessing.
        harness: Option<String>,

        /// Open the session with this response profile, by preset name.
        ///
        /// This is the session layer of the response-profile precedence
        /// chain: it wins over the role's default, this project's
        /// configuration and your user default, and loses to nothing but a
        /// task override. `glasshouse response --session <name>` prints
        /// exactly what a session started this way would resolve to.
        #[arg(long, value_name = "NAME")]
        response_profile: Option<String>,

        /// Open the session in this role, for response-profile purposes —
        /// `orchestrator`, `worker`, `reviewer`, `explainer`, or
        /// `interactive`.
        ///
        /// A spawned worker never inherits a communication style from
        /// whatever started it: its profile is resolved here, explicitly, and
        /// recorded in the launch log.
        #[arg(long, value_name = "ROLE")]
        response_role: Option<String>,

        /// Which launch profile to resolve the session through.
        ///
        /// Names a profile configured in `.glasshouse/config.toml` or the
        /// user-level configuration file — see `glasshouse setup`. Absent
        /// means the selected harness's implied Native profile, which uses
        /// the harness's own first-party authentication and configuration
        /// unchanged.
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,

        /// Start the harness with a stored checkpoint's handoff as its
        /// opening prompt.
        ///
        /// Takes the identifier `glasshouse checkpoint list` prints, or
        /// `latest` for the most recent one in this project. The prompt is
        /// plain text that names no harness, so a checkpoint written while
        /// one harness was running can start the work in another.
        ///
        /// It is appended as the harness's own trailing argument, which is
        /// exactly what typing the prompt after `--` would do — so a harness
        /// that does not take an opening prompt reports that itself, in its
        /// own words, rather than having Glasshouse guess on its behalf.
        /// Your own `--` arguments still come after it and still win.
        #[arg(long, value_name = "ID")]
        from_checkpoint: Option<String>,

        /// Continue this destination whatever the ranking says, by the
        /// identifier `glasshouse route` prints.
        ///
        /// A recorded session's identifier continues that session; a
        /// `fresh:<harness>:<profile>` identifier starts a new one under that
        /// profile.
        #[arg(long, value_name = "ID")]
        to: Option<String>,

        /// Start a fresh session whatever the ranking says.
        ///
        /// Glasshouse prefers a relevant session this project already has
        /// when its affinity outweighs starting over. This is how to say no
        /// to that, once, without changing any configuration.
        #[arg(long, conflicts_with = "to")]
        fresh: bool,

        /// Take no routing decision for this launch — capability map line
        /// 1712.
        ///
        /// Glasshouse normally ranks this project's warm sessions against a
        /// new one and continues the best of them. This turns that off for
        /// one launch: what `--to`, `--fresh` and `--profile` say still
        /// happens, and with none of them the session is a new one under the
        /// profile this launch would have used anyway.
        ///
        /// The same switch, standing, is `automatic = false` under
        /// `[routing]` in this project's or your own configuration. A launch
        /// with routing off does **not** compute the ranking in order to tell
        /// you what it would have chosen — see `glasshouse route`, which
        /// answers that question without starting anything.
        #[arg(long)]
        no_routing: bool,

        /// Check point the session this work is leaving, before it moves —
        /// capability map line 1716.
        ///
        /// Only does something when this launch continues a session that is
        /// not the one this project was most recently working in: that is
        /// what "leaving" means, and it is the only case where anything could
        /// be lost. A fresh launch, or one that continues the session you
        /// were already in, says that no checkpoint was needed rather than
        /// writing one nobody can use.
        ///
        /// The checkpoint records what Glasshouse knows — which session was
        /// left, where the work went, this project's Git position and its
        /// binding memories — and invents no objective from the session's
        /// terminal. See `glasshouse checkpoint save` for why.
        #[arg(long)]
        checkpoint_first: bool,

        /// Run the session with no terminal of its own.
        ///
        /// The harness still runs in a real pseudo-terminal, inside this
        /// project's root, with its output captured — it simply never takes
        /// over the terminal you started it from, and Glasshouse records it
        /// as a headless session. Useful for a harness given its whole task
        /// on the command line, and for starting one from a terminal you
        /// want to keep.
        ///
        /// Glasshouse stays in the foreground until the harness exits: there
        /// is no daemon behind this, and a session whose parent went away
        /// would lose the pseudo-terminal it is reading from.
        #[arg(long)]
        headless: bool,

        /// Describe the work, so the destination is chosen for what it
        /// actually needs.
        ///
        /// With a routing model configured, Glasshouse classifies the task
        /// through it before deciding where the work goes — never sending
        /// repository files, transcripts or secrets, only the task and a few
        /// facts about the session — and falls back to deterministic
        /// heuristics when none is configured or it does not answer. `--to`
        /// and `--fresh` decide on their own and ask no model. Omitting
        /// `--task` leaves the launch exactly as it has always been.
        #[arg(long, value_name = "TEXT")]
        task: Option<String>,
        /// Force, skip, or lower the assumption guardrail for this task —
        /// `force`, `skip` or `lower`. The same override `launch` takes.
        #[arg(long, value_name = "force|skip|lower")]
        guardrail: Option<String>,
        /// Present the session in an external backend instead of this
        /// terminal. `cmux` is the only backend today.
        ///
        /// With cmux available — Glasshouse is running inside a cmux
        /// surface and `cmux ping` answers — Glasshouse opens a new cmux
        /// workspace in the project root and runs this same launch inside
        /// it. The session is recorded as `external` with the workspace it
        /// lives in, and `glasshouse sessions focus` brings it to the front.
        /// Without cmux, the launch says so and runs embedded, exactly as it
        /// would have without this flag.
        #[arg(long, value_name = "BACKEND", conflicts_with = "presentation_ref")]
        presentation: Option<String>,

        /// Record this session as presented in an external pane. The
        /// process inside a pane opened by `--presentation` passes this.
        ///
        /// Takes a cmux reference (`workspace:<n>` or `surface:<n>`), or
        /// `caller`, which asks cmux which workspace this process is in. It
        /// changes nothing about how the session runs; it only records
        /// where it is shown.
        #[arg(long, value_name = "REF", hide = true)]
        presentation_ref: Option<String>,

        /// Arguments passed straight through to the harness, after `--`.
        ///
        /// Glasshouse does not interpret these; `glasshouse run claude-code
        /// -- --resume` starts the harness with `--resume`.
        #[arg(last = true, allow_hyphen_values = true)]
        harness_args: Vec<String>,
    },
    /// Generate a small executable that opens a harness through a launch
    /// profile.
    ///
    /// Writes exactly one file to `--dir`, which is required: there is no
    /// default system-wide location and no `PATH` guessing. The file's
    /// entire job is to `exec` `glasshouse run <harness> --profile <name>`,
    /// forwarding its own arguments — it names no secret, no base URL, and
    /// copies no profile, only the harness name, the profile name, and this
    /// executable's own path.
    ///
    /// Deleting the generated file is all it takes to remove it. Glasshouse
    /// never writes to a shell startup file to make it reachable on `PATH`;
    /// if the chosen directory is not already there, that is left for the
    /// user to decide.
    Shim {
        /// Which harness the shim opens, by its identifier.
        harness: String,

        /// Which launch profile the shim resolves the session through.
        #[arg(long, value_name = "NAME")]
        profile: String,

        /// Directory to write the shim into. Required: there is no default.
        #[arg(long, value_name = "PATH")]
        dir: PathBuf,

        /// File name for the shim. Defaults to the harness name (`.cmd` on
        /// Windows).
        #[arg(long, value_name = "FILE")]
        name: Option<String>,

        /// Overwrite a file already at the destination.
        #[arg(long)]
        force: bool,
    },
    /// Show the assumptions agents have stated in this project, and what
    /// became of each — Phase 21K.
    ///
    /// An assumption is a premise an agent said a substantial change rests
    /// on, through the control API or its MCP tools, with its evidence, the
    /// evidence's source class, its uncertainty, the scope it affects and
    /// the cheapest way to check it. Glasshouse never infers one from an
    /// agent's output; this prints only what was stated. Each is shown with
    /// its current state — proposed, probing, supported, refuted, unresolved
    /// or waived-by-user — and, for one session, the gates that fired for
    /// it, which factor fired them, and any override you recorded.
    Assumptions {
        /// One session's only, by its identifier or the leading part of it.
        #[arg(long, value_name = "ID")]
        session: Option<String>,

        /// At most this many assumptions, newest first.
        #[arg(long, value_name = "N", default_value_t = 50)]
        limit: usize,
    },

    /// Print Glasshouse's own project implementation policy — Phase 21H-21J.
    ///
    /// The simplicity-first rules, the production-aware checks, and the
    /// pre-completion review checklist an agent Glasshouse briefs is given
    /// alongside the project's memory. This is Glasshouse's own text, not a
    /// memory it extracted, and it says so in the block it prints.
    ///
    /// **What it prints is byte for byte what an agent receives**, markers
    /// and all, so the policy is inspectable before it is ever delivered
    /// rather than only afterwards in somebody's transcript. It reads
    /// nothing — no project, no session, no memory, no configuration — and
    /// prints the same text in any directory.
    ///
    /// Turn delivery off with `implementation_policy = false` in
    /// `.glasshouse/config.toml` or the user configuration file. That
    /// silences delivery and not this command: a person who asked to read the
    /// policy is not being spoken to unbidden.
    Policy {
        /// One part of the policy rather than all three.
        #[arg(long, value_name = "PART")]
        part: Option<crate::policy::Part>,
    },
    /// Run the local, project-scoped control API — Phase 42.
    ///
    /// Everything else in this file is a one-shot invocation: open the
    /// project's database, do one thing, exit. `api serve` is the one
    /// long-running door, so that something other than a person typing
    /// commands — an orchestrator, a script — can list, spawn, message, and
    /// interrupt this project's sessions, query its memory, and take
    /// checkpoints, without a shell already open.
    Api {
        #[command(subcommand)]
        command: ApiCommand,
    },
    /// Serve this project's control operations as MCP tools — Phase 43.
    ///
    /// The same door as `api serve`, spoken as the Model Context Protocol
    /// over stdio, so an orchestrator harness that speaks MCP (Claude Code,
    /// Codex, and others) can list, spawn, message, and interrupt this
    /// project's sessions, search its memory, and read its checkpoints as
    /// ordinary tools — gated by that harness's own permission controls.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// Report what Glasshouse's own routing model has consumed, in tokens
    /// and requests, apart from every other row this project's evidence
    /// ledger holds — capability map line 1464.
    ///
    /// Groups every recorded observation by what it was for: `classification`
    /// is `glasshouse classify`'s own calls; the coding-agent group is every
    /// exchange the gateway relayed for a harness; everything else groups
    /// together under no purpose and no harness. This build never parses the
    /// coding-agent traffic the gateway relays, so that group always carries
    /// a real request count and no token count at all — it prints as *not
    /// counted*, never as a `0` this build never measured.
    RoutingCost {
        /// How far back to look, in hours.
        #[arg(long, value_name = "N", default_value_t = 24)]
        hours: u32,
    },
    /// The context firewall — Phase 57's tool-output compaction between
    /// harness and model, map lines 1980-1990.
    ContextFirewall {
        #[command(subcommand)]
        command: ContextFirewallCommand,
    },
}

/// `glasshouse context-firewall` subcommands.
#[derive(Debug, Subcommand)]
pub enum ContextFirewallCommand {
    /// Read one `PostToolUse` hook event on stdin, write the hook response
    /// on stdout. This is the exact command a Claude Code bridge (map line
    /// 1994, gated separately) will register per session; run by hand or by
    /// a test today, it is already the production caller these box lines
    /// need.
    ///
    /// The full pipeline — deterministic reduction, raw preservation,
    /// provenance, telemetry — runs on every eligible result regardless of
    /// this flag. What `--emit-updated-output` gates is only whether the
    /// response actually asks the harness to substitute the reduced text:
    /// until the replacement premise is verified for the session in hand,
    /// the default response is a no-op so these boxes close honestly ahead
    /// of that verification.
    Hook {
        /// Below this many estimated tokens, a result passes through
        /// untouched. Units match this build's own chars/4 estimator.
        #[arg(long, value_name = "TOKENS", default_value_t = 4000)]
        passthrough_tokens: u64,

        /// Map line 1997's semantic gate: the deterministic ladder's own
        /// forwarded size must exceed this before the semantic reducer is
        /// ever asked, whatever mode or reducer `[context_firewall]`
        /// configures. Renamed from batch 71's reserved `--target-tokens`,
        /// which this package is.
        #[arg(long, value_name = "TOKENS", default_value_t = crate::config::firewall::DEFAULT_MIN_SEMANTIC_TOKENS)]
        min_semantic_tokens: u64,

        /// The stated task this hook invocation is part of — map line
        /// 1998's "the stated task the hook was given". Empty (the
        /// default) is allowed and common: not every invocation has one,
        /// and the semantic reducer never sees the conversational
        /// transcript regardless.
        #[arg(long, default_value = "")]
        task: String,

        /// A tool eligible for reduction. Repeatable. Unset, this resolves
        /// to Grep, Glob, Read, and Bash's stdout; Edit, Write, and any
        /// permission- or security-shaped tool are never eligible, named
        /// here or not.
        #[arg(long = "tool", value_name = "TOOL")]
        tools: Vec<String>,

        /// Ask the harness to substitute the reduced text. Defaults off:
        /// map line 1994's session-start verification is what the 57A
        /// bridge uses to decide when this is safe to set.
        #[arg(long)]
        emit_updated_output: bool,

        /// One of `off`, `shadow`, `safe`, `aggressive` — map line 1991.
        /// `shadow` overrides `--emit-updated-output` outright: the response
        /// never carries `updatedToolOutput` under shadow, whatever that
        /// flag says, because shadow's whole point is that Claude Code sees
        /// only originals while the pipeline still runs in full. `off` and
        /// `safe`/`aggressive` leave `--emit-updated-output` as the caller
        /// set it; the 57A bridge's own registration is what actually
        /// decides that flag's value per box 1991's four modes.
        #[arg(long, default_value = "safe")]
        mode: String,
    },
    /// Print a previously stored raw tool result, byte-identically.
    ///
    /// Whole-result expansion only — a later package covers range and
    /// candidate-scoped expansion (map line 1984's remaining shape).
    Show {
        /// The `gh-tool://` reference `context-firewall hook` recorded in a
        /// provenance header (the bare id after `gh-tool://` also works).
        id: String,
    },
}

/// `glasshouse mcp` subcommands.
#[derive(Debug, Subcommand)]
pub enum McpCommand {
    /// Answer MCP requests on stdin/stdout until the client closes stdin.
    ///
    /// Start it INSIDE THE PROJECT: the server binds to the project it is
    /// started in (the working directory's Git root, or `--scope`), and no
    /// tool takes a project, path, or socket argument — the process is the
    /// scope. Register it with the harness that will call it, for example:
    ///
    ///   Claude Code   claude mcp add glasshouse -- glasshouse mcp serve
    ///
    ///   .mcp.json /   {"mcpServers": {"glasshouse":
    ///   settings        {"command": "glasshouse", "args": ["mcp", "serve"]}}}
    ///
    ///   Codex         [mcp_servers.glasshouse]   (in ~/.codex/config.toml)
    ///                 command = "glasshouse"
    ///                 args = ["mcp", "serve"]
    ///
    /// Eight tools: five read-only (list sessions, session status, recent
    /// output, search memory, get checkpoint) and three that change a
    /// session's state and say so in their own descriptions — spawn a
    /// session (starts a process), send a message (injects input into a
    /// running harness), and interrupt a session. They are separate tools
    /// rather than one tool with an action argument precisely so a harness
    /// can allow the five and ask about the three.
    ///
    /// Stdout carries protocol frames only; diagnostics go to stderr.
    Serve,
}

/// `glasshouse api` subcommands.
#[derive(Debug, Subcommand)]
pub enum ApiCommand {
    /// Listen on this project's control socket until killed.
    ///
    /// Binds a Unix domain socket restricted to this machine's user (socket
    /// permissions plus a peer-credential check) and answers one JSON
    /// request per connection. Refuses on any platform without a Unix
    /// domain socket.
    Serve {
        /// Bind here instead of the project's own state directory.
        #[arg(long, value_name = "PATH")]
        socket: Option<PathBuf>,
    },
    /// Send one line of text to a session this project's control API is
    /// running.
    ///
    /// Capability map line 746. The direct path from a person's terminal
    /// into an orchestrated worker's own terminal: no agent is consulted and
    /// none need be running, because the text is delivered by the process
    /// that owns the worker's pseudo-terminal, which is `glasshouse api
    /// serve` itself.
    ///
    /// There is deliberately no `--socket` here, unlike `glasshouse api
    /// serve`. A server may be told where to bind; a client told where to
    /// connect could be aimed at another project's door, and this door's
    /// project scope is the door itself. Address another project with
    /// `--scope`, which changes which project this invocation is rather than
    /// letting one project reach into another.
    Send {
        /// The session to deliver to, as `glasshouse sessions` lists it.
        #[arg(long, value_name = "ID")]
        session: String,

        /// The line to deliver. Data, never a command: it is carried as one
        /// JSON string and is not expanded, interpreted, or given to a shell
        /// anywhere on its way to the session.
        #[arg(long, value_name = "TEXT")]
        text: String,
    },
    /// Interrupt a session this project's control API is running.
    ///
    /// Capability map line 747. A real interrupt on the session's own
    /// terminal, not a message asking it to stop. As with `send`, there is
    /// no `--socket`.
    Interrupt {
        /// The session to interrupt, as `glasshouse sessions` lists it.
        #[arg(long, value_name = "ID")]
        session: String,
    },
    /// Show the recent terminal output of a session this project's control
    /// API is running.
    ///
    /// Capability map line 745, and the half of it `send` and `interrupt`
    /// could not do: a person typing into a running worker from their own
    /// terminal could not see a single character of what came back. This is
    /// the reading half — words in, an interrupt, and the terminal read back
    /// are together a person being in a running worker without an agent
    /// between them.
    ///
    /// Read-only. It sends nothing to the worker, raises no signal, starts
    /// nothing, and records nothing about having looked. As with `send` and
    /// `interrupt`, there is no `--socket`.
    ///
    /// The worker's output goes to standard output exactly as its terminal
    /// holds it, with nothing added, so it can be piped. Anything Glasshouse
    /// has to say about the read — including that a running worker has
    /// printed nothing yet — goes to standard error instead, so the two are
    /// never mixed into one stream.
    ///
    /// A session **no process is running** is an error rather than an empty
    /// read: Glasshouse does not keep a session's output after its process is
    /// gone, and reporting that as "printed nothing" would be indistinguishable
    /// from a worker that is alive and quiet.
    Read {
        /// The session to read, as `glasshouse sessions` lists it.
        #[arg(long, value_name = "ID")]
        session: String,

        /// Return at most this many bytes of the most recent output.
        ///
        /// The control API bounds this again on its own side, so this can
        /// lower the ceiling and cannot raise it. Absent means the door's own
        /// default, which is stated in one place — the protocol — rather than
        /// copied here where it could drift from the door being talked to.
        #[arg(long, value_name = "BYTES")]
        max_bytes: Option<usize>,
    },
    /// Stop this project's control API delivering orchestrator messages to
    /// one session, for a time you name — capability map line 1717.
    ///
    /// This is how a person takes a worker for themselves without stopping
    /// it. While it is muted, a message from an orchestrator is refused with
    /// the remaining time named; your own `glasshouse api send` still
    /// arrives, and `glasshouse api interrupt` is never muted — a person who
    /// has quieted a worker must still be able to stop one that is running
    /// away.
    ///
    /// The mute lives in the `glasshouse api serve` process that owns the
    /// session, so restarting that process lifts it. Nothing is written to
    /// this project's database.
    ///
    /// As with `send`, `interrupt` and `read`, there is no `--socket`.
    Mute {
        /// The session to mute, as `glasshouse sessions` lists it.
        #[arg(long, value_name = "ID")]
        session: String,

        /// How long to mute it for, in seconds.
        ///
        /// Required: *temporarily* is the whole of what this offers, and a
        /// mute with no end would be a session quietly out of the
        /// orchestrator's reach with nothing to say when it came back. The
        /// door caps it and tells you what it granted.
        #[arg(long = "for", value_name = "SECONDS")]
        seconds: u64,
    },
    /// Lift a mute before it expires — capability map line 1717.
    ///
    /// Safe against a session nobody muted: it says which it was rather than
    /// failing.
    Unmute {
        /// The session to unmute, as `glasshouse sessions` lists it.
        #[arg(long, value_name = "ID")]
        session: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn version_is_the_crate_version() {
        assert_eq!(
            Cli::command().get_version(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn parses_scope_and_logging_options() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--scope",
            "/tmp/p",
            "--log-level",
            "debug",
            "--log-stderr",
        ])
        .unwrap();
        assert_eq!(cli.scope, Some(PathBuf::from("/tmp/p")));
        assert_eq!(cli.log_level.as_deref(), Some("debug"));
        assert!(cli.log_stderr);
    }

    #[test]
    fn parses_launch_with_a_named_harness_and_passthrough_arguments() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "launch",
            "claude-code",
            "--",
            "--resume",
            "--model=x",
        ])
        .unwrap();
        let Some(Command::Launch {
            harness,
            response_profile,
            response_role,
            profile,
            from_checkpoint,
            to,
            fresh,
            no_routing,
            checkpoint_first,
            headless,
            task,
            guardrail,
            presentation,
            presentation_ref,
            harness_args,
        }) = cli.command
        else {
            panic!("expected a launch command");
        };
        assert_eq!(guardrail, None, "no override unless `--guardrail` is given");
        assert_eq!(harness.as_deref(), Some("claude-code"));
        assert_eq!(profile, None);
        // Opt-in, like every routing flag: a launch that describes no task
        // classifies nothing and routes exactly as it always has.
        assert_eq!(task, None);
        // Opt-in like `--headless`: a launch that names no presentation
        // backend and no pane is shown where it always was.
        assert_eq!(presentation, None);
        assert_eq!(presentation_ref, None);
        // Opt-in like every other launch flag: a launch that names no
        // response profile and no role leaves the harness's own
        // communication behaviour untouched.
        assert_eq!(response_profile, None);
        assert_eq!(response_role, None);
        // Opt-in, like every routing flag: a launch that names neither is
        // the plain launch it has always been, and the router's automatic
        // answer stands.
        assert_eq!(to, None);
        assert!(!fresh);
        // And the two controls this launch did not ask for. Line 1712's off
        // switch and line 1716's checkpoint are opt-in for the same reason
        // every other flag here is: a plain `glasshouse launch` must keep
        // meaning exactly what it meant before either existed.
        assert!(!no_routing);
        assert!(!checkpoint_first);
        // Opt-in, like `--headless`: a launch that does not name a checkpoint
        // is the plain launch it has always been.
        assert_eq!(from_checkpoint, None);
        // A session takes the terminal unless it is explicitly told not to:
        // the flag is opt-in, so a launch that does not name it is the
        // attached one it has always been.
        assert!(!headless);
        // Hyphenated arguments after `--` reach the harness untouched rather
        // than being parsed as Glasshouse options.
        assert_eq!(harness_args, vec!["--resume", "--model=x"]);
    }

    /// `--presentation` and `--presentation-ref` are two halves of one
    /// mechanism — the outer process passes the first, the process inside
    /// the pane the second — and a launch naming both would be claiming to
    /// be on both sides of the pane at once.
    #[test]
    fn parses_a_presentation_backend_and_a_pane_ref_but_never_both() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "launch",
            "claude-code",
            "--presentation",
            "cmux",
        ])
        .unwrap();
        let Some(Command::Launch {
            presentation,
            presentation_ref,
            ..
        }) = cli.command
        else {
            panic!("expected a launch command");
        };
        assert_eq!(presentation.as_deref(), Some("cmux"));
        assert_eq!(presentation_ref, None);

        let cli = Cli::try_parse_from([
            "glasshouse",
            "run",
            "claude-code",
            "--presentation-ref",
            "caller",
        ])
        .unwrap();
        let Some(Command::Run {
            presentation,
            presentation_ref,
            ..
        }) = cli.command
        else {
            panic!("expected a run command");
        };
        assert_eq!(presentation, None);
        assert_eq!(presentation_ref.as_deref(), Some("caller"));

        assert!(
            Cli::try_parse_from([
                "glasshouse",
                "launch",
                "claude-code",
                "--presentation",
                "cmux",
                "--presentation-ref",
                "workspace:3",
            ])
            .is_err(),
            "both halves at once must be refused by the parser"
        );
    }

    #[test]
    fn launch_without_a_harness_name_is_allowed() {
        let cli = Cli::try_parse_from(["glasshouse", "launch"]).unwrap();
        let Some(Command::Launch {
            harness,
            profile,
            harness_args,
            ..
        }) = cli.command
        else {
            panic!("expected a launch command");
        };
        assert_eq!(harness, None);
        assert_eq!(profile, None);
        assert!(harness_args.is_empty());
    }

    #[test]
    fn parses_launch_with_an_explicit_profile() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "launch",
            "claude-code",
            "--profile",
            "fast",
            "--",
            "--resume",
        ])
        .unwrap();
        let Some(Command::Launch {
            harness, profile, ..
        }) = cli.command
        else {
            panic!("expected a launch command");
        };
        assert_eq!(harness.as_deref(), Some("claude-code"));
        assert_eq!(profile.as_deref(), Some("fast"));
    }

    #[test]
    fn log_file_and_log_stderr_conflict() {
        assert!(
            Cli::try_parse_from(["glasshouse", "--log-file", "a.log", "--log-stderr"]).is_err()
        );
    }

    #[test]
    fn a_literal_tilde_log_file_is_refused_with_the_same_wording_as_data_dir() {
        let err = Cli::try_parse_from(["glasshouse", "--log-file", "~/x.log"]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--log-file"), "{msg}");
        assert!(msg.contains("literal `~`"), "{msg}");
    }

    #[test]
    fn an_expanded_log_file_path_is_accepted() {
        let cli =
            Cli::try_parse_from(["glasshouse", "--log-file", "/tmp/glasshouse-x.log"]).unwrap();
        assert_eq!(cli.log_file, Some(PathBuf::from("/tmp/glasshouse-x.log")));
    }

    // --- `run` parses the same shape as `launch` --------------------------

    #[test]
    fn parses_run_with_a_named_harness_and_passthrough_arguments() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "run",
            "claude-code",
            "--",
            "--resume",
            "--model=x",
        ])
        .unwrap();
        let Some(Command::Run {
            harness,
            profile,
            harness_args,
            ..
        }) = cli.command
        else {
            panic!("expected a run command");
        };
        assert_eq!(harness.as_deref(), Some("claude-code"));
        assert_eq!(profile, None);
        assert_eq!(harness_args, vec!["--resume", "--model=x"]);
    }

    #[test]
    fn run_without_a_harness_name_is_allowed() {
        let cli = Cli::try_parse_from(["glasshouse", "run"]).unwrap();
        let Some(Command::Run {
            harness,
            profile,
            harness_args,
            ..
        }) = cli.command
        else {
            panic!("expected a run command");
        };
        assert_eq!(harness, None);
        assert_eq!(profile, None);
        assert!(harness_args.is_empty());
    }

    /// `glasshouse run` exists so a generated shim has a stable name to
    /// `exec` into, and Phase 9B's guarantee is that it behaves exactly like
    /// `launch` — proved here at the point both commands are parsed into
    /// their fields, and in `main.rs` at the point those fields are
    /// dispatched (see `glasshouse_run_and_glasshouse_launch_take_the_same_path`).
    #[test]
    fn a_profile_behaves_identically_from_run_and_from_launch() {
        let run = Cli::try_parse_from([
            "glasshouse",
            "run",
            "claude-code",
            "--profile",
            "fast",
            "--",
            "--resume",
            "--model=x",
        ])
        .unwrap();
        let launch = Cli::try_parse_from([
            "glasshouse",
            "launch",
            "claude-code",
            "--profile",
            "fast",
            "--",
            "--resume",
            "--model=x",
        ])
        .unwrap();

        let Some(Command::Run {
            harness: run_harness,
            profile: run_profile,
            harness_args: run_args,
            ..
        }) = run.command
        else {
            panic!("expected a run command");
        };
        let Some(Command::Launch {
            harness: launch_harness,
            profile: launch_profile,
            harness_args: launch_args,
            ..
        }) = launch.command
        else {
            panic!("expected a launch command");
        };

        assert_eq!(run_harness, launch_harness);
        assert_eq!(run_profile, launch_profile);
        assert_eq!(run_args, launch_args);
        // The user's trailing arguments stay last, identically, from both
        // entry points.
        assert_eq!(run_args, vec!["--resume", "--model=x"]);
    }

    // --- `shim` --------------------------------------------------------

    #[test]
    fn parses_shim_with_required_and_optional_flags() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "shim",
            "claude-code",
            "--profile",
            "fast",
            "--dir",
            "/tmp/tools",
        ])
        .unwrap();
        let Some(Command::Shim {
            harness,
            profile,
            dir,
            name,
            force,
        }) = cli.command
        else {
            panic!("expected a shim command");
        };
        assert_eq!(harness, "claude-code");
        assert_eq!(profile, "fast");
        assert_eq!(dir, PathBuf::from("/tmp/tools"));
        assert_eq!(name, None);
        assert!(!force);
    }

    #[test]
    fn shim_accepts_a_custom_name_and_force() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "shim",
            "claude-code",
            "--profile",
            "fast",
            "--dir",
            "/tmp/tools",
            "--name",
            "claude",
            "--force",
        ])
        .unwrap();
        let Some(Command::Shim { name, force, .. }) = cli.command else {
            panic!("expected a shim command");
        };
        assert_eq!(name.as_deref(), Some("claude"));
        assert!(force);
    }

    #[test]
    fn shim_requires_a_profile_and_a_dir() {
        assert!(Cli::try_parse_from(["glasshouse", "shim", "claude-code"]).is_err());
        assert!(
            Cli::try_parse_from(["glasshouse", "shim", "claude-code", "--profile", "fast"])
                .is_err()
        );
    }
}

/// What to do with this project's session checkpoints.
#[derive(Debug, Subcommand)]
pub enum CheckpointCommand {
    /// Record a checkpoint for a session.
    ///
    /// With no `--session`, the project's most recently active session — the
    /// one `glasshouse sessions` prints first, which is what "the active
    /// session" means outside the interactive interface.
    Save {
        /// What this work is trying to achieve.
        #[arg(long, value_name = "TEXT")]
        objective: String,

        /// Where it has got to.
        #[arg(long, value_name = "TEXT")]
        state: String,

        /// Which session, by the identifier `glasshouse sessions` prints.
        #[arg(long, value_name = "ID")]
        session: Option<String>,

        /// A decision discovered during this task. Repeatable.
        #[arg(long = "decision", value_name = "TEXT")]
        decisions: Vec<String>,

        /// An approach already tried and abandoned. Repeatable.
        #[arg(long = "failed", value_name = "TEXT")]
        failed_approaches: Vec<String>,

        /// A file or symbol that matters to this work. Repeatable.
        #[arg(long = "file", value_name = "TEXT")]
        files: Vec<String>,

        /// What the tests currently say.
        #[arg(long, value_name = "TEXT")]
        tests: Option<String>,

        /// What to do next. Repeatable, in order.
        #[arg(long = "next", value_name = "TEXT")]
        next_actions: Vec<String>,
    },
    /// List this project's checkpoints, most recent first.
    List,
    /// Print a checkpoint.
    ///
    /// By default the plain-text handoff a fresh session in any harness can
    /// be given; `--document` prints the portable JSON instead.
    Show {
        /// Which checkpoint, by the identifier `list` prints. Omit for the
        /// most recent one in this project.
        checkpoint: Option<String>,

        /// Print the portable document rather than the handoff prompt.
        #[arg(long)]
        document: bool,
    },
}

/// What to do with this project's durable memory.
///
/// A subcommand rather than a bare `glasshouse memory <query>` because Phase
/// 48 names the command `glasshouse memory search <query>`, and because
/// memory will grow operations that are not searches.
/// Things a person can do to one recorded session.
///
/// Three of the four change something, which is why they are subcommands
/// rather than flags on the listing: renaming, tagging and closing are user
/// actions on a record, and a report is not the place to hide one.
#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// Print everything Glasshouse recorded about one session.
    ///
    /// The harness, the launch profile, the backend resource, the model, the
    /// pairing class, the wire protocol and the response profile are printed
    /// as seven separate answers, because they are seven separate facts —
    /// collapsing them into one "agent" line is what the session model exists
    /// to prevent.
    Show {
        /// The session, or the leading part of its identifier.
        session: String,
    },

    /// Give a session a name of your own.
    ///
    /// The name is Glasshouse's own label. It never changes the harness's
    /// native session identifier, which is what a resume continues from, and
    /// it never changes the Glasshouse session identifier either.
    Rename {
        /// The session, or the leading part of its identifier.
        session: String,

        /// The new name. Omit it with `--clear` to remove one.
        #[arg(value_name = "NAME", required_unless_present = "clear")]
        name: Option<String>,

        /// Remove the session's name instead of setting one.
        #[arg(long, conflicts_with = "name")]
        clear: bool,
    },

    /// Tag a session with a lightweight purpose, such as `auth`, `tests`, or
    /// `research`.
    ///
    /// Free text: the three above are examples, not a list to choose from.
    Tag {
        /// The session, or the leading part of its identifier.
        session: String,

        /// The purpose. Omit it with `--clear` to remove one.
        #[arg(value_name = "PURPOSE", required_unless_present = "clear")]
        purpose: Option<String>,

        /// Remove the session's purpose instead of setting one.
        #[arg(long, conflicts_with = "purpose")]
        clear: bool,
    },

    /// Bring a session's external pane to the front.
    ///
    /// Only for a session presented in cmux — `glasshouse sessions` shows it
    /// as `external workspace:<n>`. Glasshouse asks cmux to select that
    /// workspace, and nothing else. A session presented in this terminal or
    /// running headless has no pane to focus, and this says so.
    Focus {
        /// The session, or the leading part of its identifier.
        session: String,
    },

    /// Retire Glasshouse's record of a session.
    ///
    /// This closes Glasshouse's own record and nothing else. The harness's
    /// session files are not read, not moved and not deleted — Glasshouse
    /// does not own them — and the native session identifier stays recorded,
    /// so the history remains findable afterwards. A session that is still
    /// running is refused rather than closed underneath itself.
    Close {
        /// The session, or the leading part of its identifier.
        session: String,
    },

    /// Let this session's background jobs spend protected quota reserve.
    ///
    /// Glasshouse keeps a protected reserve of each premium resource's quota
    /// and normally refuses to spend it on the small background jobs it runs
    /// for itself — memory extraction and the like. This overrides that
    /// refusal, for the session named here and for no other.
    ///
    /// It is recorded in your user configuration and stays until you remove
    /// it with `--clear`. There is deliberately no way to say "every
    /// session": an override that covered everything would be the reserve
    /// switched off rather than overridden, and the reserve exists to stop
    /// background work exhausting the quota an interactive session needs.
    ///
    /// The override is never silent. When it is what allowed a spend, the
    /// routing explanation the decision carries names this session by
    /// identifier, so a reader can see whose override it was rather than only
    /// that one existed.
    Reserve {
        /// The session, or the leading part of its identifier.
        session: String,

        /// Withdraw this session's override instead of granting one.
        #[arg(long)]
        clear: bool,
    },

    /// Warn before, then carry out, a profile change on a running session —
    /// capability map line 619.
    ///
    /// Reads the harness's own communication-style declaration first. A
    /// harness that can change style in place proceeds without ceremony; one
    /// that would need a new native session is checked against this
    /// session's warmth, and a warm session is never given up silently —
    /// refusing the confirmation leaves the session, its settings and its
    /// stored response profile untouched. The requested profile is delivered
    /// as a one-turn instruction through the same input path `tell` uses; it
    /// never rewrites the settings document, the system prefix, or the
    /// stored profile.
    Restyle {
        /// The session, or the leading part of its identifier.
        session: String,

        /// The response preset to apply, by the name `glasshouse response
        /// --session` accepts.
        #[arg(long, value_name = "NAME")]
        profile: String,

        /// Proceed even though the harness's declaration says this would
        /// clear or recreate a valuable warm session.
        #[arg(long)]
        accept_loss: bool,
    },

    /// Deliver one lightweight communication instruction into a running
    /// session, for this turn only — capability map line 620.
    ///
    /// Framed so it reads as an instruction from the operator rather than as
    /// the session's own words, and sent through the session's existing
    /// input path — the same one a person's own typing uses. It never
    /// touches the settings document, the system prefix, or the stored
    /// response profile: an override for one turn, not a standing change.
    /// Refused, by name, for a harness whose communication-style mechanism
    /// nobody has read — typing an unframed instruction at a harness with no
    /// verified way to receive one is a guess, not an override.
    Tell {
        /// The session, or the leading part of its identifier.
        session: String,

        /// The instruction. No line breaks or other control bytes: this
        /// delivers exactly one line, and a payload that could smuggle a
        /// second one is refused rather than sanitized — the same
        /// conservatism `integrations::cmux`'s payload rule uses.
        instruction: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    /// Find memories matching free-form text.
    ///
    /// The text is not a query language: it is matched against every
    /// memory's subject and body, best match first.
    Search {
        /// Free-form text to look for.
        query: Vec<String>,

        /// Include superseded, rejected, resolved, invalidated,
        /// needs-review and conflicted memories.
        ///
        /// Off by default: current project knowledge is what a search
        /// normally means, and history is an explicit ask.
        #[arg(long)]
        history: bool,

        /// Most results to print.
        #[arg(long, value_name = "N",
              default_value_t = crate::memory::search::DEFAULT_SEARCH_LIMIT)]
        limit: usize,
    },

    /// Promote or demote a memory's authority class.
    ///
    /// The only way to create an `invariant`: automatic extraction is capped
    /// at `constraint`, because the only certainty it has access to is a
    /// model's report of its own confidence, and Phase 21K requires that be
    /// treated as presentation rather than evidence.
    Promote {
        /// The memory, or the leading part of its identifier.
        id: String,

        /// The class to set, or `unclassified` to clear it.
        #[arg(value_name = "AUTHORITY")]
        authority: String,
    },

    /// Flag a memory as requiring review because current evidence
    /// contradicts it — Phase 21F lines 937/938.
    ///
    /// The memory moves to `needs-review` (Phase 21C's existing status, not
    /// a new one) and drops out of every default search immediately; it
    /// stays reachable as history with `--history`, and the reason given
    /// here is printed beside it there. Resolving the challenge — deciding
    /// the memory is fine after all, invalidating it, or superseding it — is
    /// a review action this command does not perform; it only raises the
    /// flag.
    Challenge {
        /// The memory, or the leading part of its identifier.
        id: String,

        /// Why current evidence contradicts it — one of Phase 21C's six
        /// review reasons.
        #[arg(value_name = "REASON")]
        reason: String,
    },

    /// Record the outcome of reviewing a memory, or list what is waiting —
    /// Phase 21G.
    ///
    /// This is the resolution `memory challenge` has always promised and
    /// this build has never shipped: challenging a memory moves it to
    /// `needs-review` and out of every default search, but until now
    /// nothing could move it back. `<OUTCOME>` is exactly Phase 21G's four
    /// words: `reaffirmed`, `needs-review`, `superseded`, `invalidated`.
    ///
    /// An automatic reviewer is refused on a high-impact memory — a
    /// binding authority, or an unclassified one — the same gate Phase 22
    /// already built for conflict resolution, reused here rather than
    /// redesigned. This command defaults to the reviewed actor, because a
    /// person typing it by hand already is the review the gate asks for;
    /// `--automatic` invokes the automatic actor so the refusal is
    /// reachable and testable.
    ///
    /// `--list` shows the bounded queue of memories actually waiting for
    /// review instead of recording an outcome — there is no mode that
    /// revalidates every memory in the project.
    Revalidate {
        /// The memory, or the leading part of its identifier. Required
        /// unless `--list` is given.
        id: Option<String>,

        /// `reaffirmed`, `needs-review`, `superseded`, or `invalidated`.
        /// Required unless `--list` is given.
        #[arg(value_name = "OUTCOME")]
        outcome: Option<String>,

        /// The memory that replaced this one. Required for `superseded`;
        /// rejected for every other outcome.
        #[arg(long, value_name = "ID")]
        by: Option<String>,

        /// Why. Two different questions, decided by the outcome:
        ///
        /// For `needs-review`, one of Phase 21C's six review reasons, and
        /// required. For `superseded`, your own sentence about why this
        /// decision went — free text, optional, and recorded so a later agent
        /// does not resurrect it without the context. Rejected for
        /// `reaffirmed` and `invalidated`.
        #[arg(long, value_name = "REASON")]
        reason: Option<String>,

        /// Act as an automatic reviewer instead of a human one.
        #[arg(long)]
        automatic: bool,

        /// List memories waiting for review instead of recording an
        /// outcome.
        #[arg(long)]
        list: bool,

        /// Most memories to print, with `--list`.
        #[arg(long, value_name = "N", default_value_t = 20)]
        limit: usize,
    },

    /// List memories currently in conflict — Phase 22's raised-but-unresolved
    /// state.
    ///
    /// An ordinary `glasshouse memory search` can move two memories to
    /// `conflicted` (`MemoryStore::mark_conflicted`), which drops both out of
    /// every default search immediately. This is the door that shows what is
    /// stuck there; settle one with `glasshouse memory resolve`.
    Conflicts {
        /// Most memories to print.
        #[arg(long, value_name = "N", default_value_t = 20)]
        limit: usize,
    },

    /// Settle a memory that is `conflicted` — the other half of
    /// `glasshouse memory conflicts`.
    ///
    /// `<OUTCOME>` is required and never defaulted: `active` keeps this
    /// memory as current knowledge, `superseded` records it as replaced.
    /// Choosing between the two is exactly the judgment the conflicted state
    /// exists to defer to a reviewer, so there is no default to fall back on.
    /// Always acts as a reviewed human, never as an automatic reviewer — a
    /// person typing this command by hand already is the review Phase 22's
    /// gate asks for.
    Resolve {
        /// The memory, or the leading part of its identifier.
        id: String,

        /// `active` or `superseded`.
        #[arg(value_name = "OUTCOME")]
        outcome: String,
    },

    /// Commit what a session has learned to this project's durable memory.
    ///
    /// The same extraction a completed turn, a landed commit, and an imminent
    /// compaction each start automatically — Phase 29's *memory commit* —
    /// asked for by hand at a moment you choose. It reads this session's
    /// recorded activity, asks the extraction model your configuration names,
    /// and records what survives the contract; the memories it produces are
    /// stamped `manual`, so a run you asked for stays distinguishable from
    /// one a harness event started.
    ///
    /// Re-running it over the same work is safe and deliberately dull: a
    /// memory this project already holds is counted as a duplicate and not
    /// stored again.
    ///
    /// Unlike `glasshouse memory extract`, this calls a real model rather
    /// than reading a reply from a file, and it takes no activity file —
    /// "recently completed work" is what this project's own event log
    /// recorded.
    Commit {
        /// The session to commit, or the leading part of its identifier.
        ///
        /// Defaults to the most recently active session in this project.
        #[arg(long)]
        session: Option<String>,
    },

    /// Run memory extraction over a session's recorded activity.
    ///
    /// `--reply-from` supplies a model's reply from a file, which is what
    /// makes this usable before Phase 39 exists: everything except the model
    /// call runs for real. It is an evaluation harness, not a model call, and
    /// the output says so on every run.
    Extract {
        /// The session the activity belongs to, or the leading part of its
        /// identifier when reading from the event log.
        #[arg(long)]
        session: String,

        /// A file holding the activity, one entry per line.
        ///
        /// Mutually exclusive with `--from-events`, and one of the two is
        /// required: extraction is never run over activity nobody chose.
        #[arg(
            long,
            value_name = "PATH",
            conflicts_with = "from_events",
            required_unless_present = "from_events"
        )]
        activity: Option<std::path::PathBuf>,

        /// Read the activity from this session's own recorded lifecycle
        /// events instead of from a file.
        ///
        /// This is the same material automatic extraction reads after a
        /// completed turn, and the memories it produces carry the range of
        /// the event log they came from. It carries no conversation: the
        /// project event log has no column one could reach.
        #[arg(long)]
        from_events: bool,

        /// A file holding a model's reply, instead of calling a model.
        #[arg(long, value_name = "PATH")]
        reply_from: std::path::PathBuf,
    },

    /// Write selected durable decisions and constraints as human-readable
    /// files under `.glasshouse/knowledge/` — Phase 50's tracked project
    /// knowledge.
    ///
    /// Runtime memory lives outside this repository by default and stays
    /// there; this is the one door that copies any of it back in, and it
    /// never opens on its own. `--tracked` is not a formality: omit it and
    /// nothing is written, which is what keeps this an explicit opt-in
    /// rather than something that happens because the subcommand was typed.
    ///
    /// Findings are left out by default — decisions and constraints are the
    /// map's own words for what this exports — and `--include-findings`
    /// widens that. The projection is a copy, never a requirement: deleting
    /// `.glasshouse/knowledge/` loses nothing Glasshouse needs, and the
    /// canonical store stays the project database this command read from.
    Export {
        /// Write tracked knowledge. Required: this command writes nothing
        /// without it.
        #[arg(long)]
        tracked: bool,

        /// Also export findings, which are left out by default.
        #[arg(long)]
        include_findings: bool,

        /// Report what would be written without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
}
