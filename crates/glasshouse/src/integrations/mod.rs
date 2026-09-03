//! Integration catalog and non-destructive discovery for coding-agent
//! harnesses and optional local tools.
//!
//! Glasshouse launches the user's *existing* harness installations — it
//! never installs, replaces, or reconfigures them. Everything in this module
//! and its submodules follows from that:
//!
//! - Discovery only ever reads (`PATH` lookups, `--version` probes with a
//!   null stdin, filesystem existence checks). It never writes to a
//!   third-party config file, never imports credentials, and never triggers
//!   an interactive login.
//! - Every result is advisory. A missed detection is not a bug report — the
//!   user can always configure an explicit executable path (see
//!   [`crate::platform::exec::resolve_explicit`]) for a harness whose
//!   binary this module could not find or guess correctly.
//! - Nothing here ever prints or logs a secret value. See
//!   [`providers`] for the structural guarantee behind that.

pub mod cmux;
pub mod providers;
pub mod version;

use std::path::{Path, PathBuf};

use crate::Project;
use crate::platform::HostPlatform;
use crate::platform::exec::{self, ResolveError, ResolvedExecutable};
use providers::ProviderSignals;
use version::{DEFAULT_PROBE_TIMEOUT, ProbeError, Version};

/// What kind of thing an integration is, for grouping in reports and UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntegrationKind {
    /// A native coding-agent harness Glasshouse can launch a session in.
    Harness,
    /// A terminal/session multiplexer Glasshouse can optionally use.
    Multiplexer,
    /// A local inference server/runtime.
    LocalInference,
}

/// Stable identifier for a known integration. The variant name is never
/// shown to the user or persisted to disk — see [`IntegrationId::slug`] for
/// the stable string form used for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntegrationId {
    ClaudeCode,
    Codex,
    Antigravity,
    OpenCode,
    Cursor,
    Pi,
    Hermes,
    Cmux,
    Ollama,
    LlamaCpp,
}

impl IntegrationId {
    /// Every known integration, in the order they should be presented.
    pub const ALL: &'static [IntegrationId] = &[
        IntegrationId::ClaudeCode,
        IntegrationId::Codex,
        IntegrationId::Antigravity,
        IntegrationId::OpenCode,
        IntegrationId::Cursor,
        IntegrationId::Pi,
        IntegrationId::Hermes,
        IntegrationId::Cmux,
        IntegrationId::Ollama,
        IntegrationId::LlamaCpp,
    ];

    /// Stable, machine-readable identifier (used for config keys, not shown
    /// as the primary label in reports — see [`IntegrationId::display_name`]).
    pub fn slug(self) -> &'static str {
        match self {
            IntegrationId::ClaudeCode => "claude-code",
            IntegrationId::Codex => "codex",
            IntegrationId::Antigravity => "antigravity",
            IntegrationId::OpenCode => "opencode",
            IntegrationId::Cursor => "cursor",
            IntegrationId::Pi => "pi",
            IntegrationId::Hermes => "hermes",
            IntegrationId::Cmux => "cmux",
            IntegrationId::Ollama => "ollama",
            IntegrationId::LlamaCpp => "llama-cpp",
        }
    }

    /// Human-readable display name.
    pub fn display_name(self) -> &'static str {
        match self {
            IntegrationId::ClaudeCode => "Claude Code",
            IntegrationId::Codex => "Codex",
            IntegrationId::Antigravity => "Antigravity",
            IntegrationId::OpenCode => "OpenCode",
            IntegrationId::Cursor => "Cursor CLI",
            IntegrationId::Pi => "Pi",
            IntegrationId::Hermes => "Hermes Agent",
            IntegrationId::Cmux => "cmux",
            IntegrationId::Ollama => "Ollama",
            IntegrationId::LlamaCpp => "llama.cpp",
        }
    }

    pub fn kind(self) -> IntegrationKind {
        match self {
            IntegrationId::ClaudeCode
            | IntegrationId::Codex
            | IntegrationId::Antigravity
            | IntegrationId::OpenCode
            | IntegrationId::Cursor
            | IntegrationId::Pi
            | IntegrationId::Hermes => IntegrationKind::Harness,
            IntegrationId::Cmux => IntegrationKind::Multiplexer,
            IntegrationId::Ollama | IntegrationId::LlamaCpp => IntegrationKind::LocalInference,
        }
    }

    /// Executable names to search `PATH` for, in priority order — the first
    /// one that resolves to a usable executable wins. These are defaults,
    /// not guarantees: the user can always point Glasshouse at an explicit
    /// path when a real install uses a different name than the one recorded
    /// here.
    ///
    /// **A harness's names come from its adapter**, not from this list.
    /// Phase 6 fixes the architecture that harness commands stay isolated
    /// inside adapters, and the executable name is the first and most
    /// consequential of those commands — get it wrong and Glasshouse starts
    /// the wrong program, or nothing at all. Keeping a second copy here would
    /// be a second place for it to be wrong, and the two would drift.
    ///
    /// What stays here are the integrations that are *not* harnesses: cmux
    /// multiplexes terminals and Ollama and llama.cpp serve models, so none
    /// of them has a session to start or an adapter to own it.
    pub fn executable_candidates(self) -> &'static [&'static str] {
        if let Some(adapter) = crate::harness::adapter_for(self) {
            return adapter.executable_candidates();
        }
        match self {
            IntegrationId::Cmux => &["cmux"],
            IntegrationId::Ollama => &["ollama"],
            IntegrationId::LlamaCpp => &["llama-server", "llama-cli"],
            // Every harness is answered by its adapter above. This arm is
            // unreachable rather than a fallback: a harness with no adapter
            // would be a harness Glasshouse cannot open a session in, and
            // `every_harness_has_an_adapter` fails before it can ship.
            IntegrationId::ClaudeCode
            | IntegrationId::Codex
            | IntegrationId::Antigravity
            | IntegrationId::OpenCode
            | IntegrationId::Cursor
            | IntegrationId::Pi
            | IntegrationId::Hermes => &[],
        }
    }

    /// The non-interactive flag used to ask this executable for its
    /// version, if it has one.
    pub fn version_arg(self) -> Option<&'static str> {
        Some("--version")
    }

    /// The declared minimum supported version, if any.
    ///
    /// Every integration declares `None` today. This is not an oversight:
    /// there is no verified minimum-supported version for any of these
    /// tools available from this environment, and inventing one would
    /// produce false [`IntegrationStatus::UnsupportedVersion`] reports for
    /// users running perfectly good installs. The comparison machinery
    /// ([`Version::satisfies_minimum`]) is fully implemented and tested so a
    /// real minimum can be declared here later purely as a data change, once
    /// one has actually been verified against real releases.
    pub fn minimum_version(self) -> Option<Version> {
        None
    }
}

/// Result of checking whether an integration shows evidence of being
/// configured for use, beyond merely being installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigEvidence {
    /// Evidence of configuration was found.
    Configured,
    /// The integration was checked and no evidence was found.
    Unconfigured,
    /// This integration does not require separate configuration; its presence
    /// is sufficient for it to be available for use.
    Available,
    /// This integration requires configuration, but Glasshouse has no
    /// reliable configuration signal to check; whether it is set up for use
    /// cannot be determined.
    Unknown,
}

/// What discovery could determine about one integration.
///
/// The capability map (`docs/product/capability-map.md`, Phase
/// 2B: "Mark every detected integration as available, configured,
/// unconfigured, unsupported-version, or unknown") lists five statuses, and
/// all five are here (`Available`, `Configured`, `Unconfigured`,
/// `UnsupportedVersion`, `Unknown`). Every integration that is *detected*
/// carries exactly one of those five states.
///
/// [`IntegrationStatus::NotFound`] is the sixth variant this type adds for
/// the case the spec's five don't cover at all: searched for and confirmed
/// absent. That is not a contradiction of the spec, it is the determinate
/// absence ("not installed") underneath it, and it matters: conflating "not
/// installed" with "unknown" would tell the Phase 2C onboarding wizard and
/// Phase 2D settings view nothing about whether to offer "add a path" versus
/// "we found something but can't tell what state it's in".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationStatus {
    /// Found on the machine and ready for use without requiring a separate
    /// user configuration or credential step (e.g. terminal multiplexers
    /// like cmux or local inference tools like llama.cpp).
    ///
    /// Present on the machine is not the same as set up for use: tools that
    /// require credentials or configuration are [`IntegrationStatus::Configured`],
    /// [`IntegrationStatus::Unconfigured`], or [`IntegrationStatus::Unknown`] —
    /// never guessed at as `Available`.
    Available,
    /// Found, and there is positive evidence it is already set up for use
    /// (e.g. valid config files, active environment variables, or live
    /// control socket).
    Configured,
    /// Found, but requires configuration and no evidence was found that it
    /// has been set up.
    Unconfigured,
    /// Found, but the probed version is below the declared minimum supported
    /// version (as evaluated by [`crate::integrations::version::Version::satisfies_minimum`]).
    UnsupportedVersion,
    /// Searched for on `PATH` and confirmed absent: every candidate name
    /// resolved to [`crate::platform::exec::ResolveError::NotFound`]. This
    /// is a determinate fact ("not installed"), not an unknown — see the
    /// enum-level documentation for why it is a distinct variant from
    /// [`IntegrationStatus::Unknown`].
    NotFound,
    /// Detection ran and could not determine the integration's state or
    /// configuration status.
    ///
    /// **`Unknown` is not a failure and not a default.** It means detection
    /// ran and could not tell. A detection that never ran is a different
    /// thing; in Glasshouse, discovery always runs non-destructively across
    /// the full catalog ([`IntegrationId::ALL`]), so unprobed integrations
    /// do not exist.
    ///
    /// This state covers:
    /// 1. Candidate paths found but unusable or indeterminate (e.g. a
    ///    Windows-interop-only PATH hit under WSL, or a file not executable
    ///    by the current user; recorded in [`DetectedIntegration::problems`]).
    /// 2. Integrations present on the machine that require credentials or
    ///    configuration, but where Glasshouse has no reliable configuration
    ///    signal to tell whether they are set up for use ([`IntegrationStatus::Configured`])
    ///    or not ([`IntegrationStatus::Unconfigured`]). Present on the machine is not
    ///    the same as set up for use, and when discovery cannot tell them
    ///    apart, the status is `Unknown`, not a guess.
    Unknown,
}

impl std::fmt::Display for IntegrationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            IntegrationStatus::Available => "available",
            IntegrationStatus::Configured => "configured",
            IntegrationStatus::Unconfigured => "unconfigured",
            IntegrationStatus::UnsupportedVersion => "unsupported version",
            IntegrationStatus::NotFound => "not found",
            IntegrationStatus::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

/// Everything discovery could determine about one integration.
#[derive(Debug, Clone)]
pub struct DetectedIntegration {
    id: IntegrationId,
    status: IntegrationStatus,
    executable: Option<ResolvedExecutable>,
    version: Option<Version>,
    evidence: Vec<String>,
    problems: Vec<String>,
}

impl DetectedIntegration {
    pub fn id(&self) -> IntegrationId {
        self.id
    }

    pub fn kind(&self) -> IntegrationKind {
        self.id.kind()
    }

    pub fn display_name(&self) -> &'static str {
        self.id.display_name()
    }

    pub fn status(&self) -> IntegrationStatus {
        self.status
    }

    pub fn executable(&self) -> Option<&ResolvedExecutable> {
        self.executable.as_ref()
    }

    pub fn version(&self) -> Option<&Version> {
        self.version.as_ref()
    }

    /// Human-readable, secret-free notes about what was observed.
    pub fn evidence(&self) -> &[String] {
        &self.evidence
    }

    /// Human-readable, secret-free, actionable setup problems.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Whether this integration has a usable executable Glasshouse could
    /// launch right now, regardless of whether it looks configured yet.
    pub fn is_usable(&self) -> bool {
        self.executable.is_some() && self.status != IntegrationStatus::UnsupportedVersion
    }
}

/// Result of a full, non-destructive discovery pass.
#[derive(Debug, Clone)]
pub struct Discovery {
    integrations: Vec<DetectedIntegration>,
    providers: ProviderSignals,
}

impl Discovery {
    /// Run the full discovery pass: resolve every known integration's
    /// executable, probe its version non-interactively, check for
    /// presence-only configuration evidence, and collect provider signals.
    ///
    /// Never fails: an absent or misbehaving executable is reported as a
    /// [`DetectedIntegration`] with problems recorded, not as an `Err`.
    pub fn run(project: &Project) -> Discovery {
        let home = home_dir();
        let integrations = IntegrationId::ALL
            .iter()
            .map(|&id| detect_one(id, home.as_deref(), project))
            .collect();
        Discovery {
            integrations,
            providers: providers::detect(),
        }
    }

    pub fn all(&self) -> &[DetectedIntegration] {
        &self.integrations
    }

    pub fn get(&self, id: IntegrationId) -> Option<&DetectedIntegration> {
        self.integrations.iter().find(|d| d.id() == id)
    }

    pub fn harnesses(&self) -> impl Iterator<Item = &DetectedIntegration> {
        self.integrations
            .iter()
            .filter(|d| d.kind() == IntegrationKind::Harness)
    }

    /// Harnesses discovery found a usable executable for, regardless of
    /// configuration status.
    pub fn available_harnesses(&self) -> impl Iterator<Item = &DetectedIntegration> {
        self.harnesses().filter(|d| d.is_usable())
    }

    pub fn providers(&self) -> &ProviderSignals {
        &self.providers
    }

    /// Every actionable problem found, in catalog order, plus — appended
    /// last — a single synthesized problem when *no* harness was detected
    /// at all. That combination (zero harnesses) is the one case where
    /// absence itself is actionable: Glasshouse cannot do anything useful
    /// without at least one harness to launch. Absence of any individual
    /// optional integration, or of one harness among several, is not by
    /// itself a problem — see `detect_one` for why plain absence produces
    /// no per-integration problem entry.
    pub fn problems(&self) -> Vec<String> {
        let mut problems: Vec<String> = self
            .integrations
            .iter()
            .flat_map(|d| d.problems.iter().cloned())
            .collect();

        let any_harness_detected = self.harnesses().any(|d| d.executable().is_some());
        if !any_harness_detected {
            problems.push(
                "no supported coding-agent harness was detected; Glasshouse needs at least one \
                 of Claude Code, Codex, Antigravity, or OpenCode, or an explicit executable path"
                    .to_string(),
            );
        }
        problems
    }
}

/// Resolve the current user's home directory via `directories`, rather than
/// reading `$HOME` directly, so the same per-platform convention used
/// elsewhere in Glasshouse (see [`crate::paths`]) governs this lookup too.
pub(crate) fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// Resolve one integration's executable and build its full
/// [`DetectedIntegration`], using the real [`exec::resolve`] as the PATH
/// resolver.
///
/// Plain absence (every candidate name resolves to
/// [`ResolveError::NotFound`]) is not treated as a reportable problem: a
/// user who does not have Codex installed has not done anything wrong, and
/// six "not found" lines in the doctor report's Problems section would bury
/// anything actually actionable. It is still fully visible in the report —
/// as [`IntegrationStatus::NotFound`] on the integration's row, with the
/// candidate names that were searched for kept in
/// [`DetectedIntegration::evidence`] — just not counted as a problem. The
/// one place total harness absence *is* treated as actionable is
/// [`Discovery::problems`], at the whole-discovery level.
fn detect_one(id: IntegrationId, home: Option<&Path>, project: &Project) -> DetectedIntegration {
    detect_one_with(
        id,
        home,
        project,
        exec::resolve,
        presence_without_executable,
    )
}

/// Core of [`detect_one`], with the executable resolver and the
/// non-executable presence lookup both injected so tests can
/// deterministically exercise the `NotFound` vs "found but unusable" vs
/// "absent but present another way" branches without depending on what
/// happens to be on the test machine's real `PATH` or environment (see the
/// `resolve_with_interop_predicate` test pattern in `platform::exec` for the
/// same idea applied there).
fn detect_one_with(
    id: IntegrationId,
    home: Option<&Path>,
    project: &Project,
    resolver: impl Fn(&str) -> Result<ResolvedExecutable, ResolveError>,
    presence: impl Fn(IntegrationId) -> Vec<String>,
) -> DetectedIntegration {
    detect_one_with_prober(
        id,
        home,
        project,
        resolver,
        presence,
        |exe, arg, proj| version::probe_version(exe, arg, proj, DEFAULT_PROBE_TIMEOUT),
        IntegrationId::minimum_version,
    )
}

/// Core detection implementation with executable resolution, non-executable presence,
/// version probing, and minimum-supported-version comparison all injected for
/// deterministic unit testing without depending on external environment or PATH.
fn detect_one_with_prober(
    id: IntegrationId,
    home: Option<&Path>,
    project: &Project,
    resolver: impl Fn(&str) -> Result<ResolvedExecutable, ResolveError>,
    presence: impl Fn(IntegrationId) -> Vec<String>,
    prober: impl Fn(&ResolvedExecutable, &str, &Project) -> Result<Option<Version>, ProbeError>,
    min_version: impl Fn(IntegrationId) -> Option<Version>,
) -> DetectedIntegration {
    let mut evidence = Vec::new();
    let mut problems = Vec::new();

    let exe = match resolve_first_usable_with(id.executable_candidates(), resolver) {
        ResolveOutcome::NotFound => {
            evidence.push(format!(
                "candidates tried: {}",
                id.executable_candidates().join(", ")
            ));
            // No executable anywhere, but the integration may still be
            // demonstrably present another way (cmux running inside its own
            // control environment; Ollama reachable at a configured local
            // endpoint). When such evidence exists the integration is
            // reported as `Configured` with no executable — visible, but
            // never launchable (`is_usable()` stays false because
            // `executable` is `None`). With no such evidence this arm
            // behaves exactly as before: plain `NotFound`.
            let presence_notes = presence(id);
            let status = if presence_notes.is_empty() {
                IntegrationStatus::NotFound
            } else {
                evidence.extend(presence_notes);
                IntegrationStatus::Configured
            };
            return DetectedIntegration {
                id,
                status,
                executable: None,
                version: None,
                evidence,
                problems,
            };
        }
        ResolveOutcome::Unusable(reason) => {
            // Something was found but Glasshouse cannot use or fully
            // characterize it (e.g. a Windows-interop-only PATH hit under
            // WSL). Unlike plain absence, this *is* actionable: the reason
            // already names exactly what to do about it.
            problems.push(reason);
            return DetectedIntegration {
                id,
                status: IntegrationStatus::Unknown,
                executable: None,
                version: None,
                evidence,
                problems,
            };
        }
        ResolveOutcome::Usable(exe) => exe,
    };

    let version = match id.version_arg() {
        Some(arg) => match prober(&exe, arg, project) {
            Ok(Some(v)) => Some(v),
            Ok(None) => {
                evidence.push(format!(
                    "`{} {arg}` did not print a recognizable version number",
                    exe.path().display()
                ));
                None
            }
            Err(err) => {
                problems.push(describe_probe_problem(id, &err));
                None
            }
        },
        None => None,
    };

    let mut status = match config_evidence(id, home) {
        (ConfigEvidence::Configured, notes) => {
            evidence.extend(notes);
            IntegrationStatus::Configured
        }
        (ConfigEvidence::Unconfigured, notes) => {
            // Not yet configured is visible on the status field and in
            // `evidence`, but it is deliberately *not* pushed to
            // `problems`: it is not one of the actionable categories
            // (unusable resolution, failed/timed-out version probe on a
            // found harness, unsupported version) that section is scoped
            // to, and a user simply hasn't logged into a harness yet is not
            // something `glasshouse doctor` should nag about as a problem.
            evidence.extend(notes);
            IntegrationStatus::Unconfigured
        }
        (ConfigEvidence::Available, _) => IntegrationStatus::Available,
        (ConfigEvidence::Unknown, notes) => {
            // Indeterminate configuration is visible on the status field and
            // in `evidence`, but like Unconfigured it is not an actionable
            // problem: detection ran and could not tell, which is not an error.
            evidence.extend(notes);
            IntegrationStatus::Unknown
        }
    };

    if let (Some(v), Some(min)) = (&version, min_version(id))
        && !v.satisfies_minimum(&min)
    {
        problems.push(format!(
            "{} version {v} is below the minimum supported version {min}",
            id.display_name()
        ));
        status = IntegrationStatus::UnsupportedVersion;
    }

    DetectedIntegration {
        id,
        status,
        executable: Some(exe),
        version,
        evidence,
        problems,
    }
}

/// What trying every candidate name for one integration established.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolveOutcome {
    /// A usable executable was found.
    Usable(ResolvedExecutable),
    /// Every candidate resolved to [`ResolveError::NotFound`]: confirmed
    /// absent, not merely undetermined.
    NotFound,
    /// At least one candidate was found but could not be used (a
    /// Windows-interop-only PATH hit under WSL, or a resolved path that is
    /// not executable). The explanatory message is `ResolveError`'s own
    /// `Display` text, which already names the reason and the remedy.
    Unusable(String),
}

/// Try each candidate name in order, returning the first usable resolution,
/// or — if none are usable — whether that is because every candidate was
/// plainly absent ([`ResolveOutcome::NotFound`]) or because at least one was
/// found but unusable ([`ResolveOutcome::Unusable`], which takes priority:
/// a found-but-unusable hit is a more specific, more actionable finding than
/// "not found" and must not be lost behind a later plain miss).
///
/// The resolver is injected (rather than this calling [`exec::resolve`]
/// directly) so tests can deterministically exercise the `NotFound` vs
/// `Unusable` split without depending on the real `PATH` or host platform —
/// mirrors `platform::exec`'s own `resolve_with_interop_predicate` test
/// pattern. [`detect_one`] is the only non-test caller, and always passes
/// [`exec::resolve`].
fn resolve_first_usable_with(
    candidates: &[&str],
    resolver: impl Fn(&str) -> Result<ResolvedExecutable, ResolveError>,
) -> ResolveOutcome {
    let mut unusable_reason: Option<String> = None;
    for &name in candidates {
        match resolver(name) {
            Ok(exe) => return ResolveOutcome::Usable(exe),
            Err(
                err
                @ (ResolveError::WindowsInteropOnly { .. } | ResolveError::NotExecutable { .. }),
            ) => {
                unusable_reason.get_or_insert_with(|| err.to_string());
            }
            Err(ResolveError::NotFound { .. }) => {}
        }
    }
    match unusable_reason {
        Some(reason) => ResolveOutcome::Unusable(reason),
        None => ResolveOutcome::NotFound,
    }
}

fn describe_probe_problem(id: IntegrationId, err: &ProbeError) -> String {
    format!(
        "could not determine {} version non-interactively: {err}",
        id.display_name()
    )
}

/// Presence-only configuration evidence for one integration.
///
/// This only ever checks *existence* of files/directories and *presence* of
/// environment variables — it never opens, parses, imports, or modifies
/// anything a harness owns. See the module documentation.
fn config_evidence(id: IntegrationId, home: Option<&Path>) -> (ConfigEvidence, Vec<String>) {
    let env_set = |name: &str| std::env::var(name).is_ok_and(|v| !v.is_empty());

    match id {
        IntegrationId::ClaudeCode => {
            let mut notes = Vec::new();
            let mut found = false;
            if let Some(home) = home {
                if home.join(".claude").is_dir() {
                    notes.push("~/.claude directory exists".to_string());
                    found = true;
                }
                if home.join(".claude.json").is_file() {
                    notes.push("~/.claude.json exists".to_string());
                    found = true;
                }
            }
            if env_set("ANTHROPIC_API_KEY") {
                notes.push("ANTHROPIC_API_KEY is set".to_string());
                found = true;
            }
            (evidence_result(found), notes)
        }
        IntegrationId::Codex => {
            let mut notes = Vec::new();
            let mut found = false;
            if let Some(home) = home
                && home.join(".codex").is_dir()
            {
                notes.push("~/.codex directory exists".to_string());
                found = true;
            }
            if env_set("OPENAI_API_KEY") {
                notes.push("OPENAI_API_KEY is set".to_string());
                found = true;
            }
            (evidence_result(found), notes)
        }
        IntegrationId::OpenCode => {
            let mut notes = Vec::new();
            let mut found = false;
            if let Some(home) = home {
                if home.join(".config").join("opencode").is_dir() {
                    notes.push("~/.config/opencode directory exists".to_string());
                    found = true;
                }
                if home.join(".opencode").is_dir() {
                    notes.push("~/.opencode directory exists".to_string());
                    found = true;
                }
            }
            (evidence_result(found), notes)
        }
        IntegrationId::Cursor => {
            let mut notes = Vec::new();
            let mut found = false;
            if let Some(home) = home
                && home.join(".cursor").is_dir()
            {
                notes.push("~/.cursor directory exists".to_string());
                found = true;
            }
            if env_set("CURSOR_API_KEY") {
                notes.push("CURSOR_API_KEY is set".to_string());
                found = true;
            }
            (evidence_result(found), notes)
        }
        IntegrationId::Pi => {
            let mut notes = Vec::new();
            let mut found = false;
            if let Some(home) = home
                && home.join(".pi").is_dir()
            {
                notes.push("~/.pi directory exists".to_string());
                found = true;
            }
            (evidence_result(found), notes)
        }
        IntegrationId::Hermes => {
            let mut notes = Vec::new();
            let mut found = false;
            if let Some(home) = home {
                if home.join(".hermes").is_dir() {
                    notes.push("~/.hermes directory exists".to_string());
                    found = true;
                }
                if home.join(".hermes").join("config.yaml").is_file() {
                    notes.push("~/.hermes/config.yaml exists".to_string());
                    found = true;
                }
            }
            (evidence_result(found), notes)
        }
        IntegrationId::Ollama => {
            let mut notes = Vec::new();
            let mut found = false;
            if let Some(home) = home
                && home.join(".ollama").is_dir()
            {
                notes.push("~/.ollama directory exists".to_string());
                found = true;
            }
            if env_set("OLLAMA_HOST") {
                notes.push("OLLAMA_HOST is set".to_string());
                found = true;
            }
            (evidence_result(found), notes)
        }
        // Antigravity requires credentials/login, but has no established
        // per-user config convention that can be safely verified non-destructively
        // from this environment. Present on the machine is not the same as set
        // up for use: because we cannot tell them apart, it is reported as
        // `Unknown`, not guessed at as `Available`, `Configured`, or `Unconfigured`.
        IntegrationId::Antigravity => (
            ConfigEvidence::Unknown,
            vec![
                "no reliable configuration signal is known; cannot determine if set up for use"
                    .to_string(),
            ],
        ),
        // cmux is a session multiplexer with no credential/config file of its own
        // to check, and llama.cpp provides local inference binaries. Neither requires
        // credentials or setup before use: a usable executable is ready and available.
        IntegrationId::Cmux | IntegrationId::LlamaCpp => (ConfigEvidence::Available, Vec::new()),
    }
}

fn evidence_result(found: bool) -> ConfigEvidence {
    if found {
        ConfigEvidence::Configured
    } else {
        ConfigEvidence::Unconfigured
    }
}

/// Evidence that an integration is present even though none of its
/// executable candidates resolved on `PATH`.
///
/// This is the other half of each capability-map line's "OR": cmux is
/// present when Glasshouse is running *inside* a cmux surface (its control
/// environment variables are set), and Ollama is present when the user has
/// configured a local endpoint for it — in both cases regardless of whether
/// a matching binary happens to be on `PATH`.
///
/// SECURITY: like [`config_evidence`] (whose note style this follows), this
/// only ever records the *names* of environment variables that are set and
/// non-empty. It never reads, formats, logs, or stores any value: socket
/// paths can be capability-bearing, endpoint URLs can carry credentials,
/// and `CMUX_SURFACE_ID`/`CMUX_WORKSPACE_ID` are treated with the same
/// name-only discipline. `CMUX_SOCKET_CAPABILITY` in particular is a
/// capability token and is deliberately not consulted at all.
fn presence_without_executable(id: IntegrationId) -> Vec<String> {
    presence_without_executable_with(id, |name| std::env::var(name).ok())
}

/// Core of [`presence_without_executable`], with the variable lookup
/// injected so tests can exercise the decision without ever mutating the
/// real process environment (parallel test runs share it; mutation would
/// corrupt unrelated tests). Mirrors the injected-resolver pattern of
/// [`detect_one_with`] and [`resolve_first_usable_with`].
pub(crate) fn presence_without_executable_with(
    id: IntegrationId,
    env: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let mut notes = Vec::new();
    let env_set = |name: &str| env(name).is_some_and(|v| !v.is_empty());

    match id {
        // A set, non-empty `CMUX_SOCKET_PATH` means a usable cmux control
        // environment. `CMUX_SURFACE_ID` / `CMUX_WORKSPACE_ID`, when also
        // present, corroborate that this is a real cmux surface rather than
        // a stray variable.
        IntegrationId::Cmux => {
            if env_set("CMUX_SOCKET_PATH") {
                notes.push("CMUX_SOCKET_PATH is set".to_string());
                for corroborating in ["CMUX_SURFACE_ID", "CMUX_WORKSPACE_ID"] {
                    if env_set(corroborating) {
                        notes.push(format!("{corroborating} is set"));
                    }
                }
            }
        }
        // A set, non-empty `OLLAMA_HOST` means the user has configured a
        // local Ollama endpoint. The condition is a match guard rather than
        // an `if` inside the arm so that an Ollama install with no endpoint
        // configured falls through to the catch-all, which is the same
        // "nothing to report" answer every other integration gives.
        IntegrationId::Ollama if env_set("OLLAMA_HOST") => {
            notes.push("OLLAMA_HOST is set".to_string());
        }
        _ => {}
    }

    notes
}

/// Render a plain-text `glasshouse doctor` report: detected harnesses and
/// their versions, optional integrations, provider signals (never secret
/// values), and actionable setup problems.
///
/// No ANSI colour and no box-drawing beyond simple ASCII, since this is
/// meant for a non-interactive command's stdout. Where discovery could not
/// determine something, the report says "unknown" — it never guesses.
pub fn doctor_report(runtime: &crate::Runtime) -> String {
    use std::fmt::Write as _;

    let discovery = Discovery::run(runtime.project());
    let mut out = String::new();

    let _ = writeln!(out, "Glasshouse doctor");
    let _ = writeln!(out, "=================");
    let _ = writeln!(out);
    let _ = writeln!(out, "Project");
    let _ = writeln!(out, "  name:      {}", runtime.project().name());
    // `display_root` and not `root`: the canonical root is a Windows
    // verbatim path, which is correct for identity but noise to a reader.
    let _ = writeln!(
        out,
        "  root:      {}",
        runtime.project().display_root().display()
    );
    let _ = writeln!(out, "  id:        {}", runtime.project().id());
    let _ = writeln!(out, "  state dir: {}", runtime.state_dir().display());
    let _ = writeln!(out);
    let _ = writeln!(out, "Host platform: {}", HostPlatform::detect());
    let _ = writeln!(out);

    // Sized to the longest status label actually appearing in *this*
    // report, not the longest label the enum could ever produce — a report
    // with nothing worse than "configured" and "not found" should not carry
    // padding sized for "unsupported version".
    let status_width = discovery
        .all()
        .iter()
        .map(|d| d.status().to_string().len())
        .max()
        .unwrap_or(0);

    let _ = writeln!(out, "Harnesses");
    for d in discovery.harnesses() {
        write_integration_line(&mut out, d, status_width);
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "Optional integrations");
    for d in discovery
        .all()
        .iter()
        .filter(|d| d.kind() != IntegrationKind::Harness)
    {
        write_integration_line(&mut out, d, status_width);
    }
    let _ = writeln!(out);

    // What Glasshouse believes about each harness, and why. This is the
    // answer to "would a session in this harness be able to resume, or run a
    // hook, or use MCP" — questions a user cannot otherwise ask, and which
    // Glasshouse itself will act on from Phase 7 onward. Every claim carries
    // the evidence it was read from, so a wrong one can be caught by reading
    // rather than by a surprise at launch.
    let _ = writeln!(out, "Harness adapters");
    for adapter in crate::harness::all() {
        write_adapter_report(&mut out, adapter);
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "Provider signals");
    let providers = discovery.providers();
    if providers.secret_vars_present().is_empty() {
        let _ = writeln!(out, "  secret env vars set: (none)");
    } else {
        let _ = writeln!(out, "  secret env vars set (values hidden):");
        for name in providers.secret_vars_present() {
            let _ = writeln!(out, "    - {name}: set (value hidden)");
        }
    }
    if providers.endpoint_vars_present().is_empty() {
        let _ = writeln!(out, "  endpoint env vars set: (none)");
    } else {
        let _ = writeln!(out, "  endpoint env vars set:");
        for name in providers.endpoint_vars_present() {
            let _ = writeln!(out, "    - {name}");
        }
    }
    if providers.config_files_present().is_empty() {
        let _ = writeln!(out, "  config files present: (none)");
    } else {
        let _ = writeln!(out, "  config files present:");
        for path in providers.config_files_present() {
            let _ = writeln!(out, "    - {}", path.display());
        }
    }
    let _ = writeln!(out);

    // Configured providers: what a user or project actually declared in
    // `config.toml`, resolved against the built-in templates. Distinct from
    // "Provider signals" above — that section is opportunistic evidence
    // (an env var happens to be set), this one is what Glasshouse itself
    // would use to answer "what can this provider serve". Never a value:
    // see `write_provider_report`.
    // Line 2 of Phase 9E, in the one place a user goes to find out what
    // Glasshouse believes: which store a credential would actually be read
    // from, said out loud. A fallback nobody is told about is a silent
    // degradation, and this is what stops it being one.
    let secrets = crate::secret::native::PreferNativeSecretStore::detect();
    let _ = writeln!(out, "Secret storage");
    let _ = writeln!(
        out,
        "  credentials resolve from: {}",
        crate::secret::SecretStore::describe(&secrets)
    );
    if let Err(reason) = secrets.native() {
        let _ = writeln!(
            out,
            "  native secure store:      unavailable ({})",
            reason.reason()
        );
    } else {
        let _ = writeln!(
            out,
            "  native secure store:      available, filed under service `{}`",
            crate::secret::native::SERVICE
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "Configured providers");
    write_configured_providers_report(&mut out, runtime, &secrets);
    let _ = writeln!(out);

    let _ = writeln!(out, "Problems");
    let problems = discovery.problems();
    if problems.is_empty() {
        let _ = writeln!(out, "  (none)");
    } else {
        for problem in problems {
            let _ = writeln!(out, "  - {problem}");
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Note: discovery is advisory. A harness auto-detection missed can still be added \
         manually with an explicit executable path."
    );

    out
}

fn write_integration_line(out: &mut String, d: &DetectedIntegration, status_width: usize) {
    use std::fmt::Write as _;

    let path = d
        .executable()
        .map(|e| e.path().display().to_string())
        .unwrap_or_else(|| "-".to_string());
    let version = d
        .version()
        .map(|v| v.to_string())
        .unwrap_or_else(|| "version unknown".to_string());
    let _ = writeln!(
        out,
        "  {:<14} [{:<status_width$}] {:<40} {}",
        d.display_name(),
        d.status().to_string(),
        path,
        version,
        status_width = status_width
    );
    for note in d.evidence() {
        let _ = writeln!(out, "      note: {note}");
    }
}

/// Render one harness adapter's declarations.
///
/// Generic over [`crate::harness::HarnessAdapter`] on purpose: this function
/// is in the report, not in an adapter, and it must stay unable to tell one
/// harness from another. If it ever needs to know which harness it is
/// printing, the thing it wanted belongs in the adapter's declarations
/// instead.
fn write_adapter_report(out: &mut String, adapter: &'static dyn crate::harness::HarnessAdapter) {
    use crate::harness::SessionIds;
    use std::fmt::Write as _;

    let described = adapter.describe();

    let vendor = described
        .vendor
        .value()
        .map(|v| v.to_string())
        .unwrap_or_else(|| "vendor unverified".to_string());
    let _ = writeln!(
        out,
        "  {:<14} {} · starts `{}`",
        adapter.id().display_name(),
        vendor,
        adapter.executable_candidates().join("` or `"),
    );

    // A stored identifier is what resume needs, so the two are shown
    // together: "resumes with X" is only useful alongside whether Glasshouse
    // can ever learn the X.
    match adapter.resume("<id>") {
        Some(invocation) => {
            let args: Vec<String> = invocation
                .args()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let _ = writeln!(out, "      resume:       {}", args.join(" "));
        }
        None => {
            let _ = writeln!(out, "      resume:       no verified mechanism");
        }
    }

    let session_ids = match described.session_ids.value() {
        Some(SessionIds::Assigned { flag }) => {
            format!("Glasshouse can assign one with `{flag}`")
        }
        Some(SessionIds::Discoverable { source }) => format!("discoverable from {source}"),
        None => "unverified".to_string(),
    };
    let _ = writeln!(out, "      session ids:  {session_ids}");

    let hooks = match described.hooks.value() {
        Some(hooks) => hooks.mechanism.to_string(),
        None => "unverified".to_string(),
    };
    let _ = writeln!(out, "      hooks:        {hooks}");

    // A blanket bypass is never worded as though it were review — that
    // distinction is the entire point of `ApprovalModes`, so the two are
    // rendered with different vocabulary ("auto review" vs. "no automatic
    // review") rather than a shared label that could blur them together.
    // Each mode is rendered as what the harness calls it *and* the argv that
    // selects it. Showing only the prose would hide the half that actually
    // reaches the process — and this row previously named Claude Code's
    // `auto-mode` subcommand, which could never have started a session, so
    // the concrete arguments are exactly what a reader needs to sanity-check.
    fn render_mode(mode: &crate::harness::ApprovalMode) -> String {
        format!("`{}` ({})", mode.description, mode.args.join(" "))
    }

    let mut approval_parts = Vec::new();
    match described.approvals.automatic_review.value() {
        Some(mode) => approval_parts.push(format!("auto review {}", render_mode(mode))),
        // Deliberately *not* "no automatic review". `Declared` has no way to
        // say "verified absent" for a mode name, so `Unverified` means nobody
        // established one — which is a different claim from the harness not
        // having one, and the difference is the reason `Declared` exists. Pi
        // is the case that makes it concrete: it is installed but not on
        // `PATH` here, so its `--help` could not be read at all.
        None => approval_parts.push("automatic review unverified".to_string()),
    }
    if let Some(bypass) = described.approvals.bypass.value() {
        approval_parts.push(format!("bypass {}", render_mode(bypass)));
    }
    if let Some(sandbox) = described.approvals.sandbox.value() {
        let rendered = if sandbox.values.is_empty() {
            sandbox.flag.to_string()
        } else {
            format!("{} <{}>", sandbox.flag, sandbox.values.join("|"))
        };
        approval_parts.push(format!("sandbox `{rendered}`"));
    }
    let _ = writeln!(out, "      approvals:    {}", approval_parts.join("; "));

    // Only what is known present is listed. An unverified capability is not
    // an absent one, so it is counted rather than named as missing.
    let named = described.capabilities.named();
    let known: Vec<&str> = named
        .iter()
        .filter(|(_, declared)| declared.is_known_present())
        .map(|(name, _)| *name)
        .collect();
    let unverified = named.len() - known.len();
    let capabilities = if known.is_empty() {
        format!("none verified ({unverified} unverified)")
    } else if unverified == 0 {
        known.join(", ")
    } else {
        format!("{} ({unverified} unverified)", known.join(", "))
    };
    let _ = writeln!(out, "      capabilities: {capabilities}");

    let protocols = match described.backends.protocols.value() {
        Some(protocols) => protocols
            .iter()
            .map(|p| p.slug())
            .collect::<Vec<_>>()
            .join(", "),
        None => "unverified".to_string(),
    };
    let _ = writeln!(out, "      protocols:    {protocols}");

    let model = match described.backends.model_override.value() {
        Some(overrides) => overrides
            .iter()
            .map(|o| o.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        None => "unverified".to_string(),
    };
    let _ = writeln!(out, "      model:        {model}");

    // Map line 290: each adapter declares which native communication-style
    // mechanism it supports, **whether changing it needs a new or cleared
    // native session**, and **whether it is likely to invalidate prompt
    // caching**. All three clauses are rendered, because each costs a user
    // something invisible otherwise: a warm session, or a cache rebuild.
    //
    // The cache clause covers `--append-system-prompt`'s one-turn mechanism
    // too, not only a persistent style change — GH-STYLE-CACHE-MEASUREMENT
    // found no behavioral difference between the two, so a sibling surface
    // that appends a one-turn instruction can point at this same
    // declaration rather than measuring its own.
    //
    // `Unverified` prints as itself rather than as "none". They are different
    // claims — `Declared`'s own documentation is that an unverified value is
    // "not `no`, and never a guess" — and a report that collapsed them would
    // tell a user a harness has no mechanism when nothing has looked.
    let communication_style = match described.communication_style.value() {
        Some(style) => {
            let change = match style.change {
                crate::harness::StyleChange::InPlace => "changeable in place",
                crate::harness::StyleChange::NewSession => "changing it needs a new session",
            };
            let cache = match style.cache_invalidation.value() {
                Some(crate::harness::CacheInvalidation::Partial { one_turn: true }) => {
                    "invalidates part of the prompt cache for the turn it changes on, including \
                     an appended one-turn instruction"
                }
                Some(crate::harness::CacheInvalidation::Partial { one_turn: false }) => {
                    "invalidates part of the prompt cache beyond the turn it changes on"
                }
                None => "prompt-cache effect unverified",
            };
            format!("{} ({change}; {cache})", style.mechanism)
        }
        None => "unverified".to_string(),
    };
    let _ = writeln!(out, "      comm style:   {communication_style}");
}

/// Render every configured provider (Phase 9C/9D), or a note explaining why
/// none could be shown.
///
/// Loads `config.toml` itself rather than taking an already-resolved
/// [`crate::config::EffectiveConfig`], because `doctor` is the one place this
/// runs standalone — every other caller already has a `Runtime` and nothing
/// else. A load failure is reported as a line in the report, not a panic:
/// `doctor` is diagnostic, and a broken config file is exactly the kind of
/// thing a user runs `doctor` to find out about.
fn write_configured_providers_report(
    out: &mut String,
    runtime: &crate::Runtime,
    secrets: &crate::secret::native::PreferNativeSecretStore,
) {
    use std::fmt::Write as _;

    let user = match crate::config::UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => {
            let _ = writeln!(out, "  configuration could not be loaded: {err}");
            return;
        }
    };
    let project = match crate::config::load_project_config(runtime.project()) {
        Ok(project) => project,
        Err(err) => {
            let _ = writeln!(out, "  configuration could not be loaded: {err}");
            return;
        }
    };
    let effective = crate::config::EffectiveConfig::new(&user, project.as_ref());

    let names = effective.provider_names();
    if names.is_empty() {
        let _ = writeln!(out, "  (none configured)");
        return;
    }

    for name in names {
        // The stored-credential *record* comes from the configuration entry
        // itself, which `Layered<Provider>` does not carry: `to_provider`
        // resolves a template into the domain type and a stored reference is
        // not part of that. Project wins over user, matching
        // `EffectiveConfig::configured_provider`'s own precedence.
        let stored = project
            .as_ref()
            .and_then(|p| p.providers().get(&name))
            .or_else(|| user.providers().get(&name))
            .and_then(crate::config::ProviderConfig::credential_store);
        match effective.configured_provider(&name) {
            Ok(layered) => write_provider_report(out, &name, &layered, secrets, stored),
            Err(err) => {
                let _ = writeln!(out, "  {name}: {err}");
            }
        }
    }
}

/// Render one resolved, configured provider: its protocols and their base
/// URLs, its declared capabilities, and its credential variable names —
/// **names only.**
///
/// Presence is answered through [`crate::secret::SecretStore::is_present`],
/// never [`crate::secret::SecretStore::resolve`]: this function must never
/// hold a value, even transiently. Which of the store's two sources answered
/// is printed alongside, so "is my key in the Keychain or in my shell
/// profile" is a question this report answers rather than one a user has to
/// infer from it.
fn write_provider_report(
    out: &mut String,
    name: &str,
    layered: &crate::config::Layered<crate::provider::Provider>,
    secrets: &crate::secret::native::PreferNativeSecretStore,
    stored: Option<&crate::config::StoredCredentialRef>,
) {
    use std::fmt::Write as _;

    fn declared_bool(declared: crate::harness::Declared<bool>) -> &'static str {
        match declared {
            crate::harness::Declared::Verified { value: true, .. } => "yes",
            crate::harness::Declared::Verified { value: false, .. } => "no",
            crate::harness::Declared::Unverified => "unverified",
        }
    }

    let layer = match layered.layer {
        crate::config::Layer::Project => "project",
        crate::config::Layer::User => "user",
        crate::config::Layer::Default => "default",
    };
    let provider = &layered.value;
    let _ = writeln!(out, "  {name} (layer: {layer})");

    for protocol in &provider.protocols {
        let base_url = if protocol.base_url.is_empty() {
            "(not set)"
        } else {
            protocol.base_url.as_str()
        };
        let _ = writeln!(out, "      {}  base url: {base_url}", protocol.protocol);
        let _ = writeln!(
            out,
            "          streaming: {}  tool calls: {}  reasoning: {}",
            declared_bool(protocol.streaming),
            declared_bool(protocol.tool_calls),
            declared_bool(protocol.reasoning),
        );
    }

    let _ = writeln!(
        out,
        "      model list endpoint: {}  usage telemetry: {}",
        declared_bool(provider.model_list_endpoint),
        declared_bool(provider.usage_telemetry),
    );

    if provider.credential_env.is_empty() {
        let _ = writeln!(out, "      credential env: (none configured)");
    } else {
        let statuses: Vec<String> = provider
            .credential_env
            .iter()
            .map(|var| {
                let reference = crate::secret::SecretRef::Environment { var: var.clone() };
                match secrets.source_of(&reference) {
                    Some(source) => format!("{var} (set in {source}, value hidden)"),
                    None => format!("{var} (not set, value hidden)"),
                }
            })
            .collect();
        let _ = writeln!(out, "      credential env: {}", statuses.join(", "));
    }

    // A configuration that says "this key is in the OS store" and a store
    // that does not answer it is exactly the state a user must be told
    // about rather than left to infer from a credential silently coming
    // from somewhere else. It has a real cause: on macOS an item's access
    // control list names the binary that wrote it, so a credential stored
    // by a different build of Glasshouse is not readable by this one — and
    // Glasshouse asks for no authorization dialog, by design, because
    // `doctor` and the session launcher must never block on one.
    if let Some(stored) = stored {
        let reference = stored.to_secret_ref();
        let state = match secrets.native() {
            Err(reason) => format!("recorded, but {}", reason.reason()),
            Ok(native) => {
                if crate::secret::SecretStore::is_present(native, &reference) {
                    "present".to_owned()
                } else {
                    "recorded, but the store did not return it; it may have been removed, "
                        .to_owned()
                        + "or stored by a different build of Glasshouse — store it again"
                }
            }
        };
        let _ = writeln!(
            out,
            "      stored credential: {}/{} — {state}",
            stored.service(),
            stored.account(),
        );
    }
}

#[cfg(test)]
mod tests;
