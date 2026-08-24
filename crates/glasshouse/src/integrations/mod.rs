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
            | IntegrationId::OpenCode => IntegrationKind::Harness,
            IntegrationId::Cmux => IntegrationKind::Multiplexer,
            IntegrationId::Ollama | IntegrationId::LlamaCpp => IntegrationKind::LocalInference,
        }
    }

    /// Executable names to search `PATH` for, in priority order — the first
    /// one that resolves to a usable executable wins. These are defaults,
    /// not guarantees: the user can always point Glasshouse at an explicit
    /// path when a real install uses a different name than the one guessed
    /// here.
    ///
    /// The Antigravity entry is deliberately a single name. The real
    /// Antigravity CLI executable name could not be verified from this
    /// development environment (no reference install was available to
    /// inspect). Rather than guess additional short aliases — `ag` in
    /// particular collides with the unrelated, widely-installed
    /// the-silver-searcher tool and would produce a confident, wrong
    /// detection — this only ever searches for the literal name
    /// `antigravity`. A missed detection here is safe (the user adds an
    /// explicit path); a false detection of an unrelated binary is not.
    pub fn executable_candidates(self) -> &'static [&'static str] {
        match self {
            IntegrationId::ClaudeCode => &["claude"],
            IntegrationId::Codex => &["codex"],
            IntegrationId::Antigravity => &["antigravity"],
            IntegrationId::OpenCode => &["opencode"],
            IntegrationId::Cmux => &["cmux"],
            IntegrationId::Ollama => &["ollama"],
            IntegrationId::LlamaCpp => &["llama-server", "llama-cli"],
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
    /// This integration has no reliable configuration signal Glasshouse
    /// knows how to check; presence of the executable is all there is.
    NoSignal,
}

/// What discovery could determine about one integration.
///
/// The capability map (`GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md`, Phase
/// 2B: "Mark every detected integration as available, configured,
/// unconfigured, unsupported-version, or unknown") lists five statuses, and
/// all five are here (`Available`, `Configured`, `Unconfigured`,
/// `UnsupportedVersion`, `Unknown`). Those five describe integrations that
/// were *detected* — the spec's wording presupposes a `PATH` hit exists to
/// describe. [`IntegrationStatus::NotFound`] is the sixth variant this type
/// adds for the case the spec's five don't cover at all: searched for and
/// confirmed absent. That is not a contradiction of the spec, it is the
/// missing "zero case" underneath it, and it matters: conflating "not
/// installed" with "unknown" would tell the Phase 2C onboarding wizard and
/// Phase 2D settings view nothing about whether to offer "add a path" versus
/// "we found something but can't tell what state it's in".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationStatus {
    /// Found and usable, but Glasshouse cannot tell whether it still needs
    /// configuring (no reliable signal exists for this integration).
    Available,
    /// Found, and there is evidence it is already set up for use.
    Configured,
    /// Found, but no evidence it has been configured.
    Unconfigured,
    /// Found, but the version is below the declared minimum.
    UnsupportedVersion,
    /// Searched for on `PATH` and confirmed absent: every candidate name
    /// resolved to [`crate::platform::exec::ResolveError::NotFound`]. This
    /// is a determinate fact ("not installed"), not an unknown — see the
    /// enum-level documentation for why it is a distinct variant from
    /// [`IntegrationStatus::Unknown`].
    NotFound,
    /// Present but Glasshouse could not tell what state it is in — e.g.
    /// found only as an unusable Windows-interop hit under WSL (present,
    /// but not usable from here; the reason is recorded in
    /// [`DetectedIntegration::problems`]), a resolved path that exists but
    /// is not executable by the current user, or any other indeterminate
    /// resolution outcome that is neither a clean success nor a clean
    /// absence. Different from [`IntegrationStatus::NotFound`]: something
    /// was found, Glasshouse just cannot fully characterize it.
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
    /// itself a problem — see [`detect_one`] for why plain absence produces
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
    detect_one_with(id, home, project, exec::resolve)
}

/// Core of [`detect_one`], with the executable resolver injected so tests
/// can deterministically exercise the `NotFound` vs "found but unusable"
/// branches without depending on what happens to be on the test machine's
/// real `PATH` (see the `resolve_with_interop_predicate` test pattern in
/// `platform::exec` for the same idea applied there).
fn detect_one_with(
    id: IntegrationId,
    home: Option<&Path>,
    project: &Project,
    resolver: impl Fn(&str) -> Result<ResolvedExecutable, ResolveError>,
) -> DetectedIntegration {
    let mut evidence = Vec::new();
    let mut problems = Vec::new();

    let exe = match resolve_first_usable_with(id.executable_candidates(), resolver) {
        ResolveOutcome::NotFound => {
            evidence.push(format!(
                "candidates tried: {}",
                id.executable_candidates().join(", ")
            ));
            return DetectedIntegration {
                id,
                status: IntegrationStatus::NotFound,
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
        Some(arg) => match version::probe_version(&exe, arg, project, DEFAULT_PROBE_TIMEOUT) {
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
        (ConfigEvidence::NoSignal, _) => IntegrationStatus::Available,
    };

    if let (Some(v), Some(min)) = (&version, id.minimum_version())
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
        // No reliable configuration signal is known for these. Antigravity
        // and llama.cpp have no established per-user config convention that
        // could be verified from this environment, and cmux is a session
        // multiplexer with no credential/config file of its own to check. A
        // usable executable is reported as `Available`, not guessed at as
        // configured or unconfigured.
        IntegrationId::Antigravity | IntegrationId::Cmux | IntegrationId::LlamaCpp => {
            (ConfigEvidence::NoSignal, Vec::new())
        }
    }
}

fn evidence_result(found: bool) -> ConfigEvidence {
    if found {
        ConfigEvidence::Configured
    } else {
        ConfigEvidence::Unconfigured
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_project() -> (tempfile::TempDir, Project) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        let project = Project::discover(tmp.path(), None, false).unwrap();
        (tmp, project)
    }

    // --- catalog integrity ---------------------------------------------

    #[test]
    fn every_integration_has_a_non_empty_slug_and_display_name() {
        for &id in IntegrationId::ALL {
            assert!(!id.slug().is_empty(), "{id:?} has an empty slug");
            assert!(
                !id.display_name().is_empty(),
                "{id:?} has an empty display name"
            );
            assert!(
                !id.executable_candidates().is_empty(),
                "{id:?} has no executable candidates"
            );
        }
    }

    #[test]
    fn slugs_are_unique() {
        let mut slugs: Vec<&str> = IntegrationId::ALL.iter().map(|&id| id.slug()).collect();
        slugs.sort_unstable();
        let mut deduped = slugs.clone();
        deduped.dedup();
        assert_eq!(slugs, deduped, "duplicate slug found in catalog");
    }

    #[test]
    fn no_minimum_version_is_declared_yet() {
        // Documents the deliberate current state; see minimum_version's doc
        // comment for why. If this ever legitimately changes, update this
        // test alongside the new minimum.
        for &id in IntegrationId::ALL {
            assert!(id.minimum_version().is_none());
        }
    }

    #[test]
    fn antigravity_only_searches_the_literal_name() {
        // Regression guard for the collision this module documents: no
        // short alias like `ag` may ever be added here.
        assert_eq!(
            IntegrationId::Antigravity.executable_candidates(),
            &["antigravity"]
        );
    }

    // --- status display ---------------------------------------------------

    #[test]
    fn status_display_has_no_debug_artifacts() {
        for status in [
            IntegrationStatus::Available,
            IntegrationStatus::Configured,
            IntegrationStatus::Unconfigured,
            IntegrationStatus::UnsupportedVersion,
            IntegrationStatus::NotFound,
            IntegrationStatus::Unknown,
        ] {
            let s = status.to_string();
            assert!(!s.contains("Integration"));
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn not_found_and_unknown_render_distinct_labels() {
        assert_eq!(IntegrationStatus::NotFound.to_string(), "not found");
        assert_eq!(IntegrationStatus::Unknown.to_string(), "unknown");
        assert_ne!(
            IntegrationStatus::NotFound.to_string(),
            IntegrationStatus::Unknown.to_string()
        );
    }

    // --- config_evidence ----------------------------------------------

    #[test]
    fn claude_code_evidence_detects_claude_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".claude")).unwrap();
        let (result, notes) = config_evidence(IntegrationId::ClaudeCode, Some(dir.path()));
        assert_eq!(result, ConfigEvidence::Configured);
        assert!(!notes.is_empty());
    }

    #[test]
    fn claude_code_evidence_is_unconfigured_with_nothing_present() {
        let dir = tempfile::tempdir().unwrap();
        let (result, _) = config_evidence(IntegrationId::ClaudeCode, Some(dir.path()));
        assert_eq!(result, ConfigEvidence::Unconfigured);
    }

    #[test]
    fn antigravity_cmux_llama_cpp_have_no_signal() {
        let dir = tempfile::tempdir().unwrap();
        for id in [
            IntegrationId::Antigravity,
            IntegrationId::Cmux,
            IntegrationId::LlamaCpp,
        ] {
            let (result, notes) = config_evidence(id, Some(dir.path()));
            assert_eq!(result, ConfigEvidence::NoSignal);
            assert!(notes.is_empty());
        }
    }

    // --- resolve_first_usable / resolve_first_usable_with ------------------

    #[test]
    fn resolve_first_usable_reports_plain_not_found_with_no_candidates_present() {
        let outcome = resolve_first_usable_with(
            &["definitely-not-a-real-glasshouse-integration-xyz"],
            exec::resolve,
        );
        assert!(matches!(outcome, ResolveOutcome::NotFound));
    }

    #[test]
    fn injected_not_found_resolver_yields_not_found_outcome() {
        let outcome = resolve_first_usable_with(&["codex"], |name| {
            Err(ResolveError::NotFound {
                name: name.to_string(),
            })
        });
        assert!(matches!(outcome, ResolveOutcome::NotFound));
    }

    #[test]
    fn injected_interop_only_resolver_yields_unusable_not_not_found() {
        let outcome = resolve_first_usable_with(&["codex"], |name| {
            Err(ResolveError::WindowsInteropOnly {
                name: name.to_string(),
                found_at: vec![PathBuf::from("/mnt/c/codex.exe")],
            })
        });
        assert!(matches!(outcome, ResolveOutcome::Unusable(_)));
    }

    #[test]
    fn unusable_hit_takes_priority_over_a_later_plain_miss() {
        // First candidate is interop-only, second is genuinely absent: the
        // more specific, more actionable finding must win.
        let outcome = resolve_first_usable_with(&["llama-server", "llama-cli"], |name| {
            if name == "llama-server" {
                Err(ResolveError::WindowsInteropOnly {
                    name: name.to_string(),
                    found_at: vec![PathBuf::from("/mnt/c/llama-server.exe")],
                })
            } else {
                Err(ResolveError::NotFound {
                    name: name.to_string(),
                })
            }
        });
        assert!(matches!(outcome, ResolveOutcome::Unusable(_)));
    }

    // --- detect_one_with -----------------------------------------------

    #[test]
    fn not_found_produces_no_problem_but_records_what_was_tried() {
        let (_guard, project) = test_project();
        let d = detect_one_with(IntegrationId::Codex, None, &project, |name| {
            Err(ResolveError::NotFound {
                name: name.to_string(),
            })
        });
        assert_eq!(d.status(), IntegrationStatus::NotFound);
        assert!(d.executable().is_none());
        assert!(
            d.problems().is_empty(),
            "plain absence must not be reported as a problem, got: {:?}",
            d.problems()
        );
        assert!(d.evidence().iter().any(|e| e.contains("codex")));
    }

    #[test]
    fn interop_only_hit_is_unknown_with_an_actionable_problem() {
        let (_guard, project) = test_project();
        let d = detect_one_with(IntegrationId::Codex, None, &project, |name| {
            Err(ResolveError::WindowsInteropOnly {
                name: name.to_string(),
                found_at: vec![PathBuf::from("/mnt/c/codex.exe")],
            })
        });
        assert_eq!(d.status(), IntegrationStatus::Unknown);
        assert!(d.executable().is_none());
        assert_eq!(d.problems().len(), 1);
    }

    // --- Discovery::run ------------------------------------------------

    #[test]
    fn discovery_runs_without_panicking_and_covers_the_whole_catalog() {
        let (_guard, project) = test_project();
        let discovery = Discovery::run(&project);
        assert_eq!(discovery.all().len(), IntegrationId::ALL.len());
        for &id in IntegrationId::ALL {
            assert!(discovery.get(id).is_some());
        }
        // A `NotFound` entry must never carry a problem (plain absence is
        // not a problem); an `Unknown` entry (found but unusable) must.
        for d in discovery.all() {
            match d.status() {
                IntegrationStatus::NotFound => assert!(
                    d.problems().is_empty(),
                    "{:?} is NotFound but has problems: {:?}",
                    d.id(),
                    d.problems()
                ),
                IntegrationStatus::Unknown if d.executable().is_none() => assert!(
                    !d.problems().is_empty(),
                    "{:?} is Unknown-and-absent but recorded no problem",
                    d.id()
                ),
                _ => {}
            }
        }
    }

    // --- Discovery::problems ---------------------------------------------

    #[test]
    fn no_harness_detected_produces_exactly_one_discovery_level_problem() {
        let integrations: Vec<DetectedIntegration> = IntegrationId::ALL
            .iter()
            .map(|&id| DetectedIntegration {
                id,
                status: IntegrationStatus::NotFound,
                executable: None,
                version: None,
                evidence: Vec::new(),
                problems: Vec::new(),
            })
            .collect();
        let discovery = Discovery {
            integrations,
            providers: ProviderSignals::default(),
        };
        let problems = discovery.problems();
        assert_eq!(
            problems.len(),
            1,
            "expected exactly one problem: {problems:?}"
        );
        assert!(problems[0].contains("no supported coding-agent harness"));
    }

    #[test]
    fn discovery_level_problem_absent_once_any_harness_is_detected() {
        let Ok(exe) = exec::resolve("sh") else {
            eprintln!("skipping: `sh` is not on PATH");
            return;
        };
        let mut integrations: Vec<DetectedIntegration> = IntegrationId::ALL
            .iter()
            .map(|&id| DetectedIntegration {
                id,
                status: IntegrationStatus::NotFound,
                executable: None,
                version: None,
                evidence: Vec::new(),
                problems: Vec::new(),
            })
            .collect();
        integrations[0] = DetectedIntegration {
            id: IntegrationId::ClaudeCode,
            status: IntegrationStatus::Available,
            executable: Some(exe),
            version: None,
            evidence: Vec::new(),
            problems: Vec::new(),
        };
        let discovery = Discovery {
            integrations,
            providers: ProviderSignals::default(),
        };
        assert!(discovery.problems().is_empty());
    }

    #[test]
    fn harnesses_and_available_harnesses_are_consistent() {
        let (_guard, project) = test_project();
        let discovery = Discovery::run(&project);
        let harness_ids: Vec<_> = discovery.harnesses().map(|d| d.id()).collect();
        assert!(harness_ids.contains(&IntegrationId::ClaudeCode));
        assert!(harness_ids.contains(&IntegrationId::Codex));
        assert!(harness_ids.contains(&IntegrationId::Antigravity));
        assert!(harness_ids.contains(&IntegrationId::OpenCode));
        assert!(!harness_ids.contains(&IntegrationId::Cmux));

        for d in discovery.available_harnesses() {
            assert!(d.is_usable());
            assert_eq!(d.kind(), IntegrationKind::Harness);
        }
    }

    // --- doctor_report ---------------------------------------------------

    #[test]
    fn doctor_report_includes_project_identity_and_never_panics() {
        use clap::Parser;

        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

        let cli = crate::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();

        let report = doctor_report(&runtime);
        assert!(report.contains(&runtime.project().name()));
        assert!(report.contains("Harnesses"));
        assert!(report.contains("Optional integrations"));
        assert!(report.contains("Provider signals"));
        assert!(report.contains("Problems"));
    }
}
