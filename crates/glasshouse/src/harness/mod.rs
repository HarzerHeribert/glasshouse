//! The contract every supported harness is reached through.
//!
//! Glasshouse core knows how to start a process in a pseudo-terminal, draw it,
//! and record it. It does not know that Claude Code resumes with `--resume`
//! while Codex resumes with a `resume` subcommand, that Codex reads hooks from
//! a file inside the project while Claude Code reads them from a settings
//! document, or that the Antigravity CLI is installed under the name `agy`.
//! All of that lives here, behind [`HarnessAdapter`], and nowhere else.
//!
//! # Declarations are evidence, not recollection
//!
//! Every fact an adapter states about its harness is a [`Declared`] value: it
//! is either `Verified`, carrying the exact place the fact came from — a line
//! of the installed binary's `--help`, one of its own configuration files, a
//! session record it wrote — or it is `Unverified`, which is what an
//! unavailable fact looks like. There is deliberately no third state that
//! means "probably".
//!
//! This is not ceremony. An adapter is the one place in Glasshouse where a
//! confidently wrong sentence launches the wrong program, resumes the wrong
//! conversation, or tells a user a capability exists that does not. A missing
//! declaration costs a feature; an invented one costs trust, and quietly.
//!
//! The declarations here were derived on 2026-08-25 from Claude Code 2.1.245,
//! Codex 0.149.0, Antigravity CLI 1.1.20, OpenCode 1.18.22, Cursor CLI
//! 2026.08.11, Pi 0.73.1, and Hermes Agent 0.15.1, every one of them installed
//! on the development machine and interrogated there. Each adapter module
//! records what it read, and a declaration nobody could read is `Unverified`
//! rather than filled in from the obvious answer.
//!
//! # What core may and may not do with an adapter
//!
//! An adapter produces *descriptions*: the executable names to look for, the
//! arguments that start or resume a session, the bytes that deliver a message
//! or an interrupt. It never spawns anything, never touches a
//! [`crate::session::runtime::SessionRuntime`], and never parses terminal
//! output. That direction is the architecture: the generic runtime stays
//! usable for any process, and adapters stay small enough to be read in one
//! sitting and checked against a real install.

pub mod antigravity;
pub mod claude_code;
pub mod codex;
pub mod cursor;
pub mod hermes;
pub mod opencode;
pub mod pairing;
pub mod pane;
pub mod pi;
pub mod response;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::integrations::{IntegrationId, IntegrationKind};

/// A fact about a harness, and where it came from.
///
/// `Verified` carries the evidence string so a diagnostic can show *why*
/// Glasshouse believes something — "because `--chrome` is in its `--help`" is
/// an answer a user can check, and "because Glasshouse says so" is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Declared<T> {
    /// Established from the installed harness itself. `evidence` names the
    /// source concretely enough to re-check: a `--help` line, a configuration
    /// file, an on-disk session record.
    Verified { value: T, evidence: &'static str },
    /// Nothing available in this environment established it. Not "no", and
    /// never a guess — see the module documentation.
    Unverified,
}

impl<T> Declared<T> {
    /// Declare `value`, citing `evidence`.
    pub const fn verified(value: T, evidence: &'static str) -> Self {
        Self::Verified { value, evidence }
    }

    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Verified { value, .. } => Some(value),
            Self::Unverified => None,
        }
    }

    pub fn evidence(&self) -> Option<&'static str> {
        match self {
            Self::Verified { evidence, .. } => Some(evidence),
            Self::Unverified => None,
        }
    }

    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified { .. })
    }
}

impl Declared<bool> {
    /// Whether the harness is known to have the capability.
    ///
    /// `Unverified` reads as `false` here, which is the safe direction: a
    /// caller asking "may I rely on this" must be told no when nobody has
    /// checked. Callers that need to distinguish "verified absent" from "not
    /// checked" match on the variant instead.
    pub fn is_known_present(&self) -> bool {
        matches!(self, Self::Verified { value: true, .. })
    }
}

/// Who publishes the harness executable.
///
/// Deliberately **not** who developed the model it talks to, and not who
/// serves that model. Claude Code is Anthropic's program whether it is
/// running Anthropic's models through Anthropic's API, the same models
/// through a cloud reseller, or something else entirely through a gateway.
/// Collapsing those three into one "vendor" field is how a router ends up
/// believing a harness and a model are first-party partners because their
/// names rhyme. That reasoning holds even when the publisher is us: `pane`
/// is Glasshouse's program whether it ends up talking to Anthropic direct,
/// through our own gateway, or to some other model behind it, and a router
/// must not read `Vendor::Glasshouse` as a pairing signal for any model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Anthropic,
    OpenAi,
    Google,
    Cursor,
    OpenCode,
    Pi,
    Hermes,
    Glasshouse,
}

impl Vendor {
    pub fn display_name(self) -> &'static str {
        match self {
            Vendor::Anthropic => "Anthropic",
            Vendor::OpenAi => "OpenAI",
            Vendor::Google => "Google",
            Vendor::Cursor => "Cursor",
            Vendor::OpenCode => "opencode-ai",
            Vendor::Pi => "Pi",
            Vendor::Hermes => "Hermes Agent",
            Vendor::Glasshouse => "Glasshouse",
        }
    }
}

impl std::fmt::Display for Vendor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.display_name())
    }
}

/// A backend wire protocol, in the vocabulary Phase 9C fixes for provider
/// compatibility. Kept identical on purpose: a protocol a harness speaks and a
/// protocol a provider serves have to be comparable without a translation
/// table between two spellings of the same idea.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireProtocol {
    AnthropicMessages,
    OpenAiResponses,
    OpenAiChat,
    /// Google's Generative Language API — `generateContent` and
    /// `streamGenerateContent` under `…/v1beta/models/<model>:<method>`,
    /// credentialled with `x-goog-api-key`.
    ///
    /// Added by Phase 56's T3 package for the *provider* side: the gateway
    /// translates a harness protocol **to** it. No installed harness speaks
    /// it at the ingress yet — the Gemini CLI adapter is T3b — so every
    /// `gemini-generate-content -> …` row in the gateway's pair table is
    /// refused by name rather than merely untested.
    GeminiGenerateContent,
}

impl WireProtocol {
    pub fn slug(self) -> &'static str {
        match self {
            WireProtocol::AnthropicMessages => "anthropic-messages",
            WireProtocol::OpenAiResponses => "openai-responses",
            WireProtocol::OpenAiChat => "openai-chat",
            WireProtocol::GeminiGenerateContent => "gemini-generate-content",
        }
    }
}

impl std::fmt::Display for WireProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.slug())
    }
}

/// How a harness can be told to use a different model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelOverride {
    /// A command-line flag, named here exactly as the harness spells it.
    CommandLine(&'static str),
    /// A key in the harness's own configuration.
    Configuration(&'static str),
    /// An environment variable read by the child process.
    Environment(&'static str),
}

impl std::fmt::Display for ModelOverride {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelOverride::CommandLine(flag) => write!(f, "command line `{flag}`"),
            ModelOverride::Configuration(key) => write!(f, "configuration `{key}`"),
            ModelOverride::Environment(name) => write!(f, "environment `{name}`"),
        }
    }
}

/// How a harness is pointed at a different backend.
///
/// The four variants are the four mechanisms the capability map names, in its
/// order: child environment, command-line arguments, an isolated generated
/// configuration, or another explicit launch mechanism. Each carries the
/// concrete mechanism rather than only its category, because Phase 9A has to
/// build a launch overlay out of these and "environment, somehow" is not
/// enough to build anything from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendSelection {
    ChildEnvironment(&'static str),
    CommandLineArguments(&'static str),
    GeneratedConfiguration(&'static str),
    Other(&'static str),
}

impl BackendSelection {
    /// The mechanism's own description, without its category.
    pub fn mechanism(self) -> &'static str {
        match self {
            BackendSelection::ChildEnvironment(m)
            | BackendSelection::CommandLineArguments(m)
            | BackendSelection::GeneratedConfiguration(m)
            | BackendSelection::Other(m) => m,
        }
    }

    pub fn category(self) -> &'static str {
        match self {
            BackendSelection::ChildEnvironment(_) => "child environment",
            BackendSelection::CommandLineArguments(_) => "command-line arguments",
            BackendSelection::GeneratedConfiguration(_) => "generated configuration",
            BackendSelection::Other(_) => "other launch mechanism",
        }
    }
}

impl std::fmt::Display for BackendSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.category(), self.mechanism())
    }
}

/// Where a credential value has to be placed for a harness to use it.
///
/// A *destination*, never a value. An adapter returns one of these to say
/// "put it here"; only [`crate::profile::resolve`] ever holds the
/// [`crate::secret::Secret`] that fills it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialPlacement {
    /// Into this environment variable of the child process.
    Environment(String),
}

/// The Glasshouse-owned directory a generated configuration document may be
/// written into, offered to an adapter so it can name the resulting file in
/// its own arguments and environment.
///
/// The same shape, and for the same reason, as [`HookCommand::file`]: the
/// adapter needs the absolute path to *point* the harness at, while core
/// needs to be the only thing that decides *where* the file may land. So the
/// adapter is handed a site and asks it for a path, rather than being handed
/// a free hand with a [`PathBuf`].
///
/// A site is only ever the directory Glasshouse owns for one session —
/// `Runtime::session_dir` — and [`GeneratedConfigSite::file`] refuses a name
/// that is not a plain file name, so no adapter can walk out of it with
/// `..` or an absolute path. That refusal is checked again by
/// [`crate::profile::resolve`] against the name the plan comes back with,
/// because the two are separate answers and only the second one is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneratedConfigSite<'a> {
    directory: &'a Path,
}

impl<'a> GeneratedConfigSite<'a> {
    /// `directory` must be a directory Glasshouse owns. It is created, if it
    /// does not exist, only when something is actually written into it — see
    /// [`crate::profile::LaunchOverlay::install`].
    pub fn new(directory: &'a Path) -> Self {
        Self { directory }
    }

    pub fn directory(&self) -> &Path {
        self.directory
    }

    /// Where a document called `file_name` will be written, or `None` when
    /// `file_name` is not a plain file name.
    pub fn file(&self, file_name: &str) -> Option<PathBuf> {
        unsafe_config_file_name(file_name)
            .is_none()
            .then(|| self.directory.join(file_name))
    }
}

/// Why `name` may not be used as a generated configuration file name, or
/// `None` when it is a plain one.
///
/// The rule is deliberately blunt — a non-empty name of letters, digits,
/// `.`, `-` and `_` that is neither `.` nor `..` — because the only thing
/// this name may ever do is sit inside the directory Glasshouse owns. An
/// adapter has no legitimate reason to name a path, and a mechanism that
/// writes into someone else's configuration directory is precisely what the
/// capability map forbids.
pub fn unsafe_config_file_name(name: &str) -> Option<ConfigFileNameProblem> {
    if name.is_empty() {
        return Some(ConfigFileNameProblem::Empty);
    }
    if name == "." || name == ".." {
        return Some(ConfigFileNameProblem::Traversal);
    }
    if let Some(offending) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
    {
        return Some(ConfigFileNameProblem::Character(offending));
    }
    None
}

/// What is wrong with a generated configuration file *name*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigFileNameProblem {
    #[error("it is empty")]
    Empty,
    #[error("it names a directory rather than a file")]
    Traversal,
    #[error("it contains `{0}`, which could leave the directory Glasshouse owns")]
    Character(char),
}

/// An isolated configuration document a harness needs written before it can
/// be pointed at a backend at all.
///
/// The document's *shape* is the harness's own business — exactly as for
/// [`HookInstallation`] — so the adapter composes it and core only writes it
/// down, inside the directory Glasshouse owns, and removes it when the
/// session ends.
///
/// **It must never carry a credential value.** An adapter is handed variable
/// *names* and never a value (see [`DirectProviderRequest`]), so the only
/// thing it can put in a document is a name; where a harness can read a
/// value out of its own environment from inside a configuration document,
/// that is the mechanism to use. OpenCode's `{env:NAME}` substitution is the
/// first such case and `crate::harness::opencode` documents the probe that
/// established it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedConfig {
    /// What to call the file Glasshouse writes, inside the directory it
    /// owns. A plain file name — see [`unsafe_config_file_name`]. **Never a
    /// path**: an adapter says what the document is called, and
    /// [`GeneratedConfigSite`] is the only thing that says where it may live.
    pub file_name: &'static str,
    /// Its contents.
    pub contents: String,
    /// How the child is told where the document ended up.
    pub path_placement: ConfigPathPlacement,
}

/// How a harness is told the path of a document Glasshouse generated for it.
///
/// The same shape as [`CredentialPlacement`], and for the same reason: the
/// adapter says *where a value goes* and core is the only thing that knows
/// the value — here, the path, which does not exist until the session
/// directory is known.
///
/// That split is what lets an adapter declare a generated configuration
/// during resolution, which happens before a session record exists, and have
/// it written afterwards into the directory that record owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPathPlacement {
    /// Into this environment variable of the child process. OpenCode's
    /// `OPENCODE_CONFIG` is this.
    Environment(&'static str),
    /// After this command-line flag, as its own argument. Claude Code's
    /// `--settings <file>` is this shape, although its settings document
    /// reaches the child through `session::select` rather than through a
    /// launch overlay.
    Argument(&'static str),
}

/// What a harness needs in order to talk to a direct provider — handed to an
/// adapter deliberately WITHOUT the credential value.
///
/// Splitting the request from the value is the whole secret boundary: an
/// adapter composes arguments and environment out of names and URLs, and has
/// no way to accidentally interpolate a credential into either, because it
/// never receives one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectProviderRequest<'a> {
    pub provider_name: &'a str,
    pub protocol: WireProtocol,
    pub base_url: &'a str,
    pub model: Option<&'a str>,
    /// The environment variable the provider declares its credential comes
    /// from, when it declares one. A NAME, never a value.
    pub credential_var: Option<&'a str>,
    /// Extra HTTP headers this provider needs, as name/value pairs — see
    /// [`crate::provider::Provider::headers`]. Configuration, not
    /// credentials: names and values here are never resolved through a
    /// [`crate::secret::SecretStore`], and `crate::config`'s
    /// `ProviderConfig::to_provider` has already refused anything that
    /// could not survive being interpolated into a header line.
    pub headers: &'a [(String, String)],
}

/// How this harness will be pointed at that provider, for one child process.
/// Carries no credential value — see [`CredentialPlacement`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectProviderPlan {
    pub args: Vec<OsString>,
    /// Non-secret environment only: base URL, model. NEVER a credential.
    pub env: Vec<(OsString, OsString)>,
    /// Where [`crate::profile::resolve`] must put the credential value, if
    /// any.
    pub credential: Option<CredentialPlacement>,
    /// An isolated configuration document that has to exist before the child
    /// starts, for a harness whose provider configuration is file-shaped.
    ///
    /// The adapter composes it and has already pointed itself at
    /// [`GeneratedConfigSite::file`]'s answer through its own `args` or
    /// `env`. Nothing is written while a profile is being resolved — see
    /// [`crate::profile::LaunchOverlay::install`], which is the only writer
    /// and hands back the guard that removes it again.
    pub config: Option<GeneratedConfig>,
    /// Names and mechanism only, for diagnostics. Never a value.
    pub mechanism: String,
}

/// The first character of `name` that must not reach a
/// [`DirectProviderRequest::provider_name`], or `None` when every character
/// is safe.
///
/// A provider name is interpolated by an adapter into a command line — for
/// Codex, into a *dotted TOML path* (`model_providers.<id>.base_url`), where
/// `.` is a separator rather than a character to escape. So the allow-list
/// here is narrower than [`crate::shim`]'s `check_name`, which permits `.`
/// because a shim's script has no such structure: letters, digits, `-` and
/// `_` only.
///
/// This is checked by [`crate::profile::resolve`] before any adapter sees a
/// request, rather than inside each adapter, for two reasons.
/// [`HarnessAdapter::direct_provider_launch`] deliberately has no error
/// channel — `None` there means "this harness declares no such mechanism",
/// which is a different answer from "this name is dangerous" and must not
/// be spelled the same way. And a rule enforced once, before the request
/// exists, protects every adapter that will ever be written, including the
/// ones that have not been.
pub fn unsafe_provider_name_char(name: &str) -> Option<char> {
    name.chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
}

/// Why `name` cannot be used as a [`DirectProviderRequest::credential_var`],
/// or `None` when it is a usable environment variable name.
///
/// The same class of problem as [`unsafe_provider_name_char`] and refused the
/// same way: Codex interpolates this name into a `-c
/// model_providers.<id>.env_key=<VAR>` value. `-` is excluded as well here,
/// because this is an environment variable name rather than an identifier,
/// and a leading digit is refused because such a name is not portably
/// settable.
pub fn unusable_credential_var(name: &str) -> Option<CredentialVarProblem> {
    if name.is_empty() {
        return Some(CredentialVarProblem::Empty);
    }
    if let Some(offending) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '_'))
    {
        return Some(CredentialVarProblem::Character(offending));
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        return Some(CredentialVarProblem::LeadingDigit);
    }
    None
}

/// What is wrong with a credential variable *name*. Never carries a value —
/// this type describes the name only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialVarProblem {
    Empty,
    Character(char),
    LeadingDigit,
}

impl std::fmt::Display for CredentialVarProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredentialVarProblem::Empty => f.write_str("it is empty"),
            CredentialVarProblem::Character(c) => {
                write!(f, "it contains `{c}`")
            }
            CredentialVarProblem::LeadingDigit => f.write_str("it starts with a digit"),
        }
    }
}

/// Structured lifecycle hooks a harness offers, and how they are configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hooks {
    /// Where hooks are declared for this harness.
    pub mechanism: &'static str,
    /// Event names Glasshouse has actually seen this harness accept.
    ///
    /// Named `verified_events` rather than `events` because it is not a
    /// catalogue: it is the subset that was observed. A harness may well
    /// support more, and an adapter must never present this list as the
    /// complete set.
    pub verified_events: &'static [&'static str],
}

/// Whether a harness's own session identifiers can be known to Glasshouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionIds {
    /// Glasshouse can choose the identifier and hand it to the harness when
    /// the session starts. Strictly better than discovering one afterwards:
    /// the identifier is known before the process exists, so a session that
    /// dies during startup still has one.
    Assigned { flag: &'static str },
    /// The harness chooses the identifier; Glasshouse can read it back from
    /// this source afterwards.
    Discoverable { source: &'static str },
}

/// Where a harness keeps the session identity Glasshouse discovers, and in
/// which of the two shapes it keeps it.
///
/// This is the *machine* counterpart to [`SessionIds::Discoverable`]'s
/// `source`, which is a human-readable citation for [`describe`](HarnessAdapter::describe)'s
/// evidence. The two must agree in substance, but only this one is actually
/// read.
///
/// # Why this is an enum rather than one struct with optional fields
///
/// Harnesses do not agree on what a "session record" is, and the difference
/// is not cosmetic: one shape means opening every file that survives a name
/// filter, the other means opening exactly one named file and no other. A
/// struct that could describe both would let a declaration send
/// [`mod@crate::session::native_id`] walking a directory of SQLite
/// conversation databases — which is precisely what must never happen for
/// Antigravity, whose records are the user's private conversations. Stating
/// the shape in the type makes the walk unreachable from the index variant
/// by construction rather than by a rule someone has to remember.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeSessionSource {
    /// One record per session, self-describing in its own first line.
    /// Paired with [`HarnessAdapter::read_session_record`].
    RecordPerSession(RecordPerSessionSource),
    /// The identifier lives in one shared index keyed by project path; no
    /// session record is ever opened. Paired with
    /// [`HarnessAdapter::read_index_entry`].
    SharedIndex(SharedIndexSource),
}

impl NativeSessionSource {
    /// How this source's state root is found: the environment variable that
    /// relocates it, if the harness honours one, and its default place under
    /// the user's home directory.
    pub fn home(&self) -> (Option<&'static str>, &'static str) {
        match self {
            Self::RecordPerSession(source) => (source.home_env, source.home_default),
            Self::SharedIndex(source) => (source.home_env, source.home_default),
        }
    }
}

/// A harness whose session store holds one record per session, each naming
/// its own id, cwd and start time in its first line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordPerSessionSource {
    /// Environment variable that relocates the harness's state root, or
    /// `None` when the harness honours none and its root is only ever found
    /// under the user's home directory.
    pub home_env: Option<&'static str>,
    /// The root's default place under the user's home directory.
    pub home_default: &'static str,
    /// Subdirectory of that root holding session records.
    pub subdirectory: &'static str,
    /// Session record file names start with this.
    pub file_prefix: &'static str,
    /// Session record file names end with this.
    pub file_extension: &'static str,
}

/// A harness that keeps every project's last conversation identifier in one
/// shared index file, keyed by project path.
///
/// There is deliberately no field naming the records themselves. The whole
/// point of this variant is that the records are never reached: Antigravity's
/// are `conversations/<uuid>.db`, SQLite databases holding the user's private
/// conversations, and nothing in Glasshouse may open, list or glob them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedIndexSource {
    /// Environment variable that relocates the harness's state root, or
    /// `None` when the harness honours none.
    pub home_env: Option<&'static str>,
    /// The root's default place under the user's home directory.
    pub home_default: &'static str,
    /// The index's path within that root. Exactly one file, named in full —
    /// not a directory, not a pattern.
    pub index_path: &'static str,
}

/// One harness session, as the harness itself recorded it — what
/// [`HarnessAdapter::read_session_record`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSessionRecord {
    pub id: String,
    pub cwd: PathBuf,
    /// When the harness says the session began.
    pub started_at: SystemTime,
    pub kind: NativeSessionKind,
}

/// What kind of session a [`NativeSessionRecord`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeSessionKind {
    /// An interactive terminal session — the kind Glasshouse starts.
    Interactive,
    /// Something else written to the same place: a subagent thread, a
    /// headless run, or another client's session.
    Other,
}

/// What a harness is known to be able to do.
///
/// Every field is a [`Declared`] rather than a `bool`, because the capability
/// map asks for these "when known" and the difference between "this harness
/// cannot use a browser" and "nobody checked" decides whether a router should
/// avoid it or investigate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub code_editing: Declared<bool>,
    pub shell_access: Declared<bool>,
    pub browser_use: Declared<bool>,
    pub mcp: Declared<bool>,
    pub subagents: Declared<bool>,
}

impl Capabilities {
    /// Nothing known — the starting point an adapter fills in.
    pub const UNVERIFIED: Self = Self {
        code_editing: Declared::Unverified,
        shell_access: Declared::Unverified,
        browser_use: Declared::Unverified,
        mcp: Declared::Unverified,
        subagents: Declared::Unverified,
    };

    /// Each capability with its name, for rendering.
    pub fn named(&self) -> [(&'static str, Declared<bool>); 5] {
        [
            ("code editing", self.code_editing),
            ("shell access", self.shell_access),
            ("browser use", self.browser_use),
            ("MCP", self.mcp),
            ("subagents", self.subagents),
        ]
    }
}

/// Which backends a harness can be pointed at, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backends {
    pub protocols: Declared<&'static [WireProtocol]>,
    pub model_override: Declared<&'static [ModelOverride]>,
    pub selection: Declared<&'static [BackendSelection]>,
}

impl Backends {
    pub const UNVERIFIED: Self = Self {
        protocols: Declared::Unverified,
        model_override: Declared::Unverified,
        selection: Declared::Unverified,
    };
}

/// A harness's own mechanism for controlling how it talks to the user.
///
/// Claude Code output styles are the example the capability map names. This
/// is communication policy only: it is not reasoning effort, not permission
/// mode, and not tool access, and an adapter that maps a Glasshouse response
/// profile onto one of these must not reach for a mechanism that changes any
/// of those.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommunicationStyle {
    pub mechanism: &'static str,
    pub change: StyleChange,
    /// Whether applying the change disturbs the harness's prompt cache.
    ///
    /// Orthogonal to `change`: a style can be applied `InPlace` and still
    /// invalidate part of the cache on the turn it lands — measured for
    /// Claude Code, whose `change` is `NewSession` for an unrelated reason
    /// (the mechanism is launch-only) while the cache hit was observed with
    /// `--resume`, no new session at all.
    pub cache_invalidation: Declared<CacheInvalidation>,
}

/// Whether changing the communication style needs a new native session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleChange {
    /// The style of a running session can be changed in place.
    InPlace,
    /// A change takes effect only in a session started afterwards, so
    /// changing it means giving up a warm session.
    NewSession,
}

/// What a measured style change costs a harness's prompt cache.
///
/// The only shape observed so far (Claude Code 2.1.252, 2026-09-01): the
/// turn the change lands rebuilds part of the cache, and the effect does not
/// persist past that turn. A bare `bool` would flatten "invalidates" into a
/// claim no measurement supports — the finding is partial and one-turn, not
/// total or sticky, and the type says so rather than a caller having to
/// remember it from a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheInvalidation {
    Partial { one_turn: bool },
}

/// One approval mode, as the harness's own launch interface exposes it.
///
/// Two fields because they answer different questions, and conflating them
/// caused a real defect: `description` is what the harness's documentation
/// says, for a human reading `glasshouse doctor`; `args` is what actually
/// selects the mode on a launch. Claude Code is why they cannot be one field —
/// its classifier is inspected by an `auto-mode` *subcommand*, which an earlier
/// declaration cited, while the thing that selects the mode for a session is
/// `--permission-mode auto`. Appending the subcommand to a launch would not
/// have started a session at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalMode {
    /// The exact argv that selects this mode, in order.
    pub args: &'static [&'static str],
    /// How the harness's own documentation describes the mode.
    pub description: &'static str,
}

/// A harness's sandbox selector.
///
/// `values` is empty when the flag is a boolean switch that takes no value —
/// Antigravity's `--sandbox` ("Run in a sandbox with terminal restrictions
/// enabled") is one, while Codex's and Cursor's both take a value from a fixed
/// set. A caller that appends a value to a valueless flag, or omits one from a
/// flag that requires it, produces an invocation the harness rejects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxSelector {
    pub flag: &'static str,
    pub values: &'static [&'static str],
}

/// How a harness decides whether a tool call may run.
///
/// `automatic_review` is the one that matters: a mode where the harness
/// classifies its own tool calls and only asks about the ones that warrant
/// it. A blanket bypass is NOT automatic review and must never be recorded
/// as one — the difference is the entire point of the declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalModes {
    /// A native mode that classifies rather than prompts.
    pub automatic_review: Declared<ApprovalMode>,
    /// A mode that skips checks entirely.
    pub bypass: Declared<ApprovalMode>,
    /// A sandbox policy selector, where the harness has one.
    pub sandbox: Declared<SandboxSelector>,
}

impl ApprovalModes {
    /// Nothing known — the starting point an adapter fills in.
    pub const UNVERIFIED: Self = Self {
        automatic_review: Declared::Unverified,
        bypass: Declared::Unverified,
        sandbox: Declared::Unverified,
    };

    /// Whether this harness can be asked to classify instead of prompt.
    pub fn has_automatic_review(&self) -> bool {
        self.automatic_review.is_verified()
    }
}

/// Which of a harness's two approval axes [`HarnessAdapter::approval_args`]
/// is being asked for.
///
/// Sandbox is deliberately not a variant here: it takes a value and is a
/// separate axis from "does this tool call need to be asked about at all",
/// so it gets its own accessor later rather than a third variant of this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalKind {
    AutomaticReview,
    Bypass,
}

/// Everything an adapter declares about its harness.
///
/// One value rather than a dozen trait methods: these are read together —
/// by a diagnostic describing a harness, by Phase 9A composing a launch
/// profile, by a router deciding what a session can do — and splitting them
/// across accessors would only make every reader reassemble the same struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessDescription {
    pub vendor: Declared<Vendor>,
    pub hooks: Declared<Hooks>,
    pub session_ids: Declared<SessionIds>,
    pub capabilities: Capabilities,
    pub backends: Backends,
    pub approvals: ApprovalModes,
    pub communication_style: Declared<CommunicationStyle>,
}

/// The command a harness should run to report one lifecycle event.
///
/// Glasshouse reports to *itself*: the program is the running Glasshouse
/// executable and the arguments name the session and the event. That is
/// deliberate — a shell one-liner appending to a file would need different
/// quoting on every platform, and a harness's configuration is not the place
/// to hide shell portability.
#[derive(Debug, Clone)]
pub struct HookCommand {
    program: std::path::PathBuf,
    session: String,
    directory: std::path::PathBuf,
    scope: std::path::PathBuf,
    data_dir: std::path::PathBuf,
    config_dir: std::path::PathBuf,
}

/// Wrap `value` in single quotes for a POSIX shell.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

impl HookCommand {
    /// `program` is the Glasshouse executable, `session` the session it should
    /// report against, and `directory` the Glasshouse-owned place its
    /// configuration document will be written — the adapter needs that to name
    /// the file in its own arguments.
    pub fn new(
        program: impl Into<std::path::PathBuf>,
        session: impl Into<String>,
        directory: impl Into<std::path::PathBuf>,
        scope: impl Into<std::path::PathBuf>,
        data_dir: impl Into<std::path::PathBuf>,
        config_dir: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            program: program.into(),
            session: session.into(),
            directory: directory.into(),
            scope: scope.into(),
            data_dir: data_dir.into(),
            config_dir: config_dir.into(),
        }
    }

    /// Where a document named `file_name` will be written, inside the
    /// Glasshouse-owned directory this command reports against.
    pub fn file(&self, file_name: &str) -> std::path::PathBuf {
        self.directory.join(file_name)
    }

    /// The Glasshouse-owned directory itself, for a
    /// [`HookDestination::GlasshouseOwned`] installation that needs to create
    /// it before [`HookCommand::file`] can be written into it.
    pub fn directory(&self) -> &std::path::Path {
        &self.directory
    }

    /// The project root, for a [`HookDestination::ProjectLocal`] installation
    /// that needs to resolve its `relative_path` against it.
    pub fn scope(&self) -> &std::path::Path {
        &self.scope
    }

    /// The command line that reports `event`, quoted for a shell.
    ///
    /// **Every path is pinned explicitly.** A hook runs as a fresh process
    /// with whatever working directory and environment the harness gives it,
    /// so a command that relied on discovering the project from its
    /// surroundings would report into whichever project it happened to land
    /// in — or into the user's real data directory while the session lived in
    /// a temporary one. That was not a hypothetical: the first version omitted
    /// them, ran cleanly, exited zero, and silently updated nothing.
    ///
    /// Paths are quoted; the session identifier is hexadecimal and the event
    /// name comes from the adapter's own constant list, so neither can carry a
    /// space. A single quote inside a path is escaped the POSIX way.
    pub fn shell_command(&self, event: &str) -> String {
        format!(
            "{program} --scope {scope} --data-dir {data} --config-dir {config} \
             hook --session {session} --event {event}",
            program = quote(&self.program.display().to_string()),
            scope = quote(&self.scope.display().to_string()),
            data = quote(&self.data_dir.display().to_string()),
            config = quote(&self.config_dir.display().to_string()),
            session = self.session,
        )
    }

    pub fn session(&self) -> &str {
        &self.session
    }
}

/// Where a harness insists on reading its hooks from.
///
/// Claude Code's `--settings` flag means Glasshouse can put its hook document
/// anywhere and simply point the harness at it. Codex has no such flag: it
/// reads hooks from exactly one place, and that place is inside the user's
/// own project. The two cases need different handling — a `GlasshouseOwned`
/// document is always written, a `ProjectLocal` one only with the user's
/// explicit consent — and this is what lets [`mod@crate::session::select`]
/// enforce that rule in one place rather than trusting every adapter to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookDestination {
    /// A directory Glasshouse owns, inside the project's own state. The
    /// harness is pointed at it by the installation's own `args`, and
    /// nothing of the user's is touched.
    GlasshouseOwned,
    /// Inside the user's project, at this relative path, because the
    /// harness reads hooks from nowhere else. Requires explicit consent.
    ProjectLocal { relative_path: &'static str },
}

/// A harness's lifecycle hooks, ready to install for one session.
///
/// The document's *shape* is the harness's own business — Claude Code reads a
/// settings JSON, Codex reads a `hooks.json` inside the project — so the
/// adapter builds it and core only writes it down and passes the arguments.
#[derive(Debug, Clone)]
pub struct HookInstallation {
    /// What to call the file Glasshouse writes.
    pub file_name: &'static str,
    /// Its contents.
    pub contents: String,
    /// Arguments that make the harness read it, for this session only.
    pub args: Invocation,
    /// The events this installation asked for, in the order declared.
    pub events: &'static [&'static str],
    /// Where this document must be written — see [`HookDestination`].
    pub destination: HookDestination,
}

/// Render `value` as a JSON string literal.
///
/// Hand-written rather than pulled from `serde_json` because the only values
/// passed here are an event name from an adapter's own constant list and a
/// command line built from an executable path — but a Windows path is full
/// of backslashes, and emitting those unescaped would produce a document the
/// harness cannot parse.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Build the `{"hooks": {...}}` document both Claude Code and Codex read:
/// each of `events` maps to a single entry holding one
/// `{type: "command", command, timeout}` hook. Shared because the shape is
/// identical between the two harnesses — only the event list, the reporting
/// command, and the timeout differ, and those are exactly this function's
/// parameters.
fn hooks_document(events: &[&'static str], report: &HookCommand, timeout_seconds: u32) -> String {
    let entries: Vec<String> = events
        .iter()
        .map(|event| {
            format!(
                "    {}: [\n      {{ \"hooks\": [ {{ \"type\": \"command\", \
                 \"command\": {}, \"timeout\": {timeout_seconds} }} ] }}\n    ]",
                json_string(event),
                json_string(&report.shell_command(event)),
            )
        })
        .collect();
    format!("{{\n  \"hooks\": {{\n{}\n  }}\n}}\n", entries.join(",\n"))
}

/// Arguments that start or resume a native session.
///
/// The program itself is never in here. It comes from
/// [`mod@crate::session::select`], which resolves one of the adapter's
/// [`HarnessAdapter::executable_candidates`] or an explicitly configured
/// path, and from
/// [`crate::platform::exec::ResolvedExecutable::spawn_command`], which owns
/// the Windows script-shim wrapping. An adapter that also chose the program
/// would be a second place for that to go wrong.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Invocation {
    args: Vec<OsString>,
}

impl Invocation {
    /// A session started with no arguments beyond the executable itself.
    ///
    /// This is what every supported harness needs today: they all open an
    /// interactive session in the current working directory when run bare,
    /// and Glasshouse has already made that directory the project root.
    pub fn bare() -> Self {
        Self::default()
    }

    pub fn of<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    pub fn is_bare(&self) -> bool {
        self.args.is_empty()
    }
}

/// Bytes that deliver one message to a running harness.
///
/// A harness in a pseudo-terminal is typed at, so a message is its text
/// followed by whatever submits it. That trailing byte is the part that can
/// differ between harnesses, which is why this is an adapter concern and not
/// a `SessionRuntime` one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    bytes: Vec<u8>,
}

impl Message {
    /// The generic form: the text, then a carriage return.
    ///
    /// `\r` is what a terminal sends when Enter is pressed, which is exactly
    /// what this is imitating — see `crate::tui::event`'s encoding of the
    /// Enter key, which had to answer the same question and reached the same
    /// answer for the same reason.
    pub fn typed(text: &str) -> Self {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(b'\r');
        Self { bytes }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// The byte sequence that interrupts a running harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interrupt(&'static [u8]);

impl Interrupt {
    /// `0x03` — what a terminal sends when Ctrl-C is pressed, and what a
    /// process attached to a pseudo-terminal receives as an interrupt.
    ///
    /// This is a property of terminals, not of any harness, which is why it
    /// is the default every adapter inherits. A harness with a gentler native
    /// interrupt — cancel the turn, keep the session — can override it, but
    /// only with evidence that the sequence is real.
    pub const CONTROL_C: Self = Self(&[0x03]);

    pub fn bytes(self) -> &'static [u8] {
        self.0
    }
}

/// One harness, behind one contract.
///
/// Implementations are zero-sized and their answers are constants: an adapter
/// is a description of an installed program, not a live connection to one.
/// That is what lets [`adapter_for`] hand out `&'static dyn HarnessAdapter`
/// with no ownership or lifetime for callers to manage.
pub trait HarnessAdapter: std::fmt::Debug + Send + Sync {
    /// Which integration this adapter speaks for.
    fn id(&self) -> IntegrationId;

    /// Executable names to look for on `PATH`, in priority order.
    ///
    /// This is the harness half of "the executable command used to start a
    /// new native session"; [`HarnessAdapter::start`] is the argument half.
    /// Resolution itself belongs to [`mod@crate::session::select`], which also
    /// honours an explicitly configured path — an adapter's list is what to
    /// try when the user has not said.
    fn executable_candidates(&self) -> &'static [&'static str];

    /// Arguments that start a new native session.
    fn start(&self) -> Invocation;

    /// Arguments that assign `native_session` to a session being started,
    /// or `None` when this harness does not let Glasshouse choose its
    /// identifier.
    ///
    /// Assigning beats discovering. An identifier chosen before the process
    /// exists is known even if the harness dies during startup, needs no
    /// filesystem watching and no parsing, and cannot be confused with
    /// another session started at the same moment. A harness that returns
    /// `None` here has to have its identifier found afterwards, which is
    /// strictly more work and strictly less certain.
    ///
    /// Must agree with [`HarnessDescription::session_ids`]: returning
    /// `Some` here and declaring anything but [`SessionIds::Assigned`] is a
    /// contradiction, and `assignment_agrees_with_the_declaration` fails on
    /// it.
    fn assign_session_id(&self, native_session: &str) -> Option<Invocation> {
        let _ = native_session;
        None
    }

    /// Where this harness records the sessions it writes, or `None` when
    /// Glasshouse cannot discover an identifier for it afterwards.
    ///
    /// Must agree with [`HarnessDescription::session_ids`]: returning `Some`
    /// here and declaring anything but [`SessionIds::Discoverable`] is a
    /// contradiction, and `a_discoverable_adapter_declares_discoverable_session_ids`
    /// fails on it — the same pattern
    /// `assignment_agrees_with_the_declaration` checks for
    /// [`HarnessAdapter::assign_session_id`].
    fn session_id_source(&self) -> Option<NativeSessionSource> {
        None
    }

    /// Read one session record from the first line of an artifact this
    /// harness wrote.
    ///
    /// Pure: it is handed text and returns a description. The walking, the
    /// time bound and the ambiguity rule belong to
    /// [`mod@crate::session::native_id`], which knows no harness.
    ///
    /// Paired with [`NativeSessionSource::RecordPerSession`]; an adapter
    /// declaring [`NativeSessionSource::SharedIndex`] implements
    /// [`HarnessAdapter::read_index_entry`] instead.
    fn read_session_record(&self, header: &str) -> Option<NativeSessionRecord> {
        let _ = header;
        None
    }

    /// The identifier this harness's shared index holds for `project_root`.
    ///
    /// Pure in exactly the sense [`HarnessAdapter::read_session_record`] is:
    /// handed the index's own text, it returns an identifier and opens
    /// nothing. Core resolves the path and reads the one file; this never
    /// touches a filesystem, and in particular never reaches the harness's
    /// conversation records — for Antigravity those are SQLite databases
    /// holding the user's private conversations.
    ///
    /// Paired with [`NativeSessionSource::SharedIndex`]. Returning `None` is
    /// the answer for "this index says nothing about that project", which
    /// [`mod@crate::session::native_id`] turns into recording nothing.
    fn read_index_entry(&self, index: &str, project_root: &Path) -> Option<String> {
        let _ = (index, project_root);
        None
    }

    /// A lifecycle-hook installation for one session, or `None` when this
    /// harness has no verified hook mechanism.
    ///
    /// The installation's [`HookInstallation::destination`] says where
    /// Glasshouse writes the returned document: a
    /// [`HookDestination::GlasshouseOwned`] one is always written, and the
    /// returned `args` point the harness at it; a
    /// [`HookDestination::ProjectLocal`] one is written only with the user's
    /// explicit consent, because it lands inside their own project. Either
    /// way, this never edits the harness's own *global* configuration: a
    /// Glasshouse session must leave the user's `claude` or `codex` exactly
    /// as it found it.
    fn hook_installation(&self, report: &HookCommand) -> Option<HookInstallation> {
        let _ = report;
        None
    }

    /// Arguments that resume `native_session`, or `None` when this harness has
    /// no verified resume mechanism.
    ///
    /// `native_session` is the harness's own identifier as Glasshouse
    /// recorded it. It reaches the child as one `argv` entry and is never
    /// interpreted by a shell; the Windows script-shim path additionally
    /// rejects arguments it cannot pass safely, in
    /// [`crate::platform::exec::ResolvedExecutable::spawn_command`].
    fn resume(&self, native_session: &str) -> Option<Invocation>;

    /// Everything this adapter declares about its harness.
    fn describe(&self) -> HarnessDescription;

    /// Which models this harness's own vendor says it operates — Phase 9J
    /// lines 557, 558 and 562.
    ///
    /// A *declaration*, exactly like everything in
    /// [`HarnessAdapter::describe`], and kept off [`HarnessDescription`] on
    /// purpose: a harness adding or dropping official support for a model
    /// must be an edit to one array in one adapter, and widening the struct
    /// every reader of a description already destructures would make it an
    /// edit to all of them.
    ///
    /// The default is [`pairing::OfficialModelSupport::UNVERIFIED`] — "nobody read
    /// this harness's model list" — which is what an adapter that has not
    /// been asked the question should say. It is never "this harness
    /// supports nothing": see [`pairing::classify`] for the difference that
    /// makes.
    fn official_model_support(&self) -> pairing::OfficialModelSupport {
        pairing::OfficialModelSupport::UNVERIFIED
    }

    /// The arguments that select `mode` on this harness, or `None` when this
    /// harness declares no such mode.
    ///
    /// `None` is the fail-closed answer a caller needs: it means "this harness
    /// cannot be launched that way", never "launch it some other way". Callers
    /// must not substitute a different mode for a `None` — a bypass standing in
    /// for automatic review is exactly the silent downgrade the design forbids.
    fn approval_args(&self, mode: ApprovalKind) -> Option<Vec<&'static str>> {
        let approvals = self.describe().approvals;
        let declared = match mode {
            ApprovalKind::AutomaticReview => approvals.automatic_review,
            ApprovalKind::Bypass => approvals.bypass,
        };
        declared.value().map(|mode| mode.args.to_vec())
    }

    /// How this harness is pointed at a direct provider, or `None` when it
    /// declares no mechanism. Default `None` — never a guess.
    ///
    /// `None` is the same fail-closed answer [`HarnessAdapter::approval_args`]
    /// gives: "this harness cannot be launched that way", never "launch it
    /// some other way". A harness that speaks a protocol the request does not
    /// name answers `None` rather than composing a configuration the harness
    /// itself would reject.
    ///
    /// The request carries no credential and the plan returns none — see
    /// [`DirectProviderRequest`] and [`CredentialPlacement`]. An adapter says
    /// *where* a value goes; [`crate::profile::resolve`] is the only thing
    /// that ever holds one.
    fn direct_provider_launch(
        &self,
        request: &DirectProviderRequest<'_>,
    ) -> Option<DirectProviderPlan> {
        let _ = request;
        None
    }

    /// Whether this harness's direct-provider mechanism is unusable without a
    /// model named for it.
    ///
    /// Default `false`, which is what every harness answered before this
    /// existed: pointing it at a provider is enough and the harness picks a
    /// model itself.
    ///
    /// `true` is for a harness that selects the *provider* through the
    /// model. OpenCode's `--model <provider>/<model>` is the case this
    /// exists for: a profile that configures a provider and names no model
    /// leaves the harness configured and unused, starting on whatever
    /// backend it defaulted to. For a user who asked to be pointed somewhere
    /// else that is the silent, billable failure
    /// [`crate::profile::resolve`]'s credential step already refuses, so it
    /// is refused too — and never guessed at, because a model identifier
    /// invented by Glasshouse is exactly the invention that module exists to
    /// refuse.
    ///
    /// Read **before** [`HarnessAdapter::direct_provider_launch`] is called,
    /// so a missing model is a refusal that says so rather than a `None`
    /// that reads as "this harness declares no mechanism".
    fn direct_provider_requires_model(&self) -> bool {
        false
    }

    /// This harness's own communication-style mechanism, set to whatever it
    /// calls the style closest to `profile` — or `None`.
    ///
    /// `None` is the fail-closed answer [`HarnessAdapter::approval_args`] and
    /// [`HarnessAdapter::direct_provider_launch`] already give, and it means
    /// the same thing here: **this harness cannot express that profile
    /// natively**, never "express it some other way". Two quite different
    /// situations answer `None` and neither may be papered over — a harness
    /// whose communication-style mechanism nobody has read, and a harness
    /// whose styles exist but would weaken its coding instructions if
    /// selected. Line 601 asks for the native mechanism *"when it can
    /// represent the selected profile without weakening coding
    /// instructions"*, and the adapter is the only thing that knows its own
    /// styles well enough to answer that.
    ///
    /// Default `None` — never a guess, exactly as for
    /// [`HarnessDescription::communication_style`]. An adapter that has not
    /// read its harness's mechanism must not acquire one by inheriting a
    /// generic implementation.
    fn native_response_style(
        &self,
        profile: &crate::profile::response::ResponseProfile,
    ) -> Option<response::NativeStyle> {
        let _ = profile;
        None
    }

    /// How this harness accepts an instruction **beside** its own system
    /// prompt, or `None`.
    ///
    /// The word is *beside*. An adapter may declare a mechanism here only if
    /// its harness's own documentation says the mechanism appends; a
    /// mechanism that replaces the system prompt is forbidden by line 607 and
    /// there is nowhere in [`mod@response`] to record having used one.
    ///
    /// Takes no profile because it describes the *mechanism*, not the text:
    /// the text is always [`crate::profile::response::ResponseProfile::instruction`],
    /// composed in one place so a terse profile cannot grow a shorter variant
    /// that drops the floor.
    fn additive_response_injection(&self) -> Option<response::AdditiveInjection> {
        None
    }

    /// Bytes that deliver `text` to a running session of this harness.
    fn message(&self, text: &str) -> Message {
        Message::typed(text)
    }

    /// Bytes that interrupt a running session of this harness.
    fn interrupt(&self) -> Interrupt {
        Interrupt::CONTROL_C
    }
}

/// The adapter for `id`, or `None` when `id` is not a harness.
///
/// Total over [`IntegrationKind::Harness`]: every harness Glasshouse can open
/// a session in has an adapter, and `every_harness_has_an_adapter` fails if a
/// future harness is added to the catalogue without one. The multiplexer and
/// local-inference integrations have none, because there is no session to
/// start in them.
pub fn adapter_for(id: IntegrationId) -> Option<&'static dyn HarnessAdapter> {
    match id {
        IntegrationId::ClaudeCode => Some(&claude_code::ClaudeCode),
        IntegrationId::Codex => Some(&codex::Codex),
        IntegrationId::Antigravity => Some(&antigravity::Antigravity),
        IntegrationId::OpenCode => Some(&opencode::OpenCode),
        IntegrationId::Cursor => Some(&cursor::Cursor),
        IntegrationId::Pi => Some(&pi::Pi),
        IntegrationId::Hermes => Some(&hermes::Hermes),
        IntegrationId::Pane => Some(&pane::Pane),
        IntegrationId::Cmux | IntegrationId::Ollama | IntegrationId::LlamaCpp => None,
    }
}

/// The harness's own structured pre-tool hook event, when Glasshouse has a
/// verified bridge to one — Phase 60 map line 2404.
///
/// **A statement about Glasshouse, not only about the harness.** A harness
/// may well fire something pre-tool that nobody here has read a payload
/// from; naming it would promise coordination this build cannot deliver. So
/// this answers `Some` only where the event is in that adapter's own
/// [`Hooks::verified_events`] *and* a launch actually registers a hook on it
/// — today, Claude Code's `PreToolUse` and nothing else
/// (`commands::launch::install_edit_intent_hook`).
///
/// `None` is line 2404's honest answer, and the *only* other one: where this
/// returns `None`, edit-intent coordination is simply absent for that
/// harness, and nothing — not a terminal scrape, not a prompt guess — is put
/// in its place.
pub fn structured_pre_tool_hook(id: IntegrationId) -> Option<&'static str> {
    match id {
        IntegrationId::ClaudeCode => Some("PreToolUse"),
        IntegrationId::Codex
        | IntegrationId::Antigravity
        | IntegrationId::OpenCode
        | IntegrationId::Cursor
        | IntegrationId::Pi
        | IntegrationId::Hermes
        | IntegrationId::Pane
        | IntegrationId::Cmux
        | IntegrationId::Ollama
        | IntegrationId::LlamaCpp => None,
    }
}

/// Every harness adapter, in catalogue order.
pub fn all() -> impl Iterator<Item = &'static dyn HarnessAdapter> {
    IntegrationId::ALL
        .iter()
        .copied()
        .filter(|id| id.kind() == IntegrationKind::Harness)
        .filter_map(adapter_for)
}

/// Whether a harness's declared executable candidates resolve to something
/// installed and directly usable on this machine.
///
/// This answers Phase 9F line 466's precondition — "require the selected
/// coding harness executable to be installed and usable before offering an
/// interactive direct-provider or gateway-backed launch profile" — as a
/// value, so [`crate::profile::resolve_checked`] can refuse on it without
/// this crate's `profile` module having to search `PATH` itself.
/// [`ExecutablePresence::detect`] performs the same search
/// [`mod@crate::session::select`] and `glasshouse doctor` already do: every
/// declared candidate name in turn, first usable one wins — see
/// `integrations::resolve_first_usable_with`, which this mirrors.
///
/// **This is `PATH` discovery only.** It does not know about an explicitly
/// configured executable path — that lookup belongs to
/// [`mod@crate::session::select`], which reads configuration this crate's
/// `harness` and `profile` modules deliberately do not import (see
/// `profile`'s own module documentation). A caller that has already resolved
/// a harness through `session::select` knows more than a fresh
/// [`ExecutablePresence::detect`] call can, and should hand
/// [`crate::profile::resolve_checked`] the [`ExecutablePresence::Usable`] it
/// already established instead of asking this type to search `PATH` again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutablePresence {
    /// A candidate resolved to something installed and directly usable.
    Usable,
    /// Every declared candidate name resolved to "not found": confirmed
    /// absent, not merely unchecked.
    NotFound,
    /// At least one candidate was found but could not be used — for example
    /// a Windows-interop-only `PATH` hit under WSL. More specific than
    /// [`ExecutablePresence::NotFound`], and carries why, taken from
    /// [`crate::platform::exec::ResolveError`]'s own message.
    Unusable { reason: String },
}

impl ExecutablePresence {
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Usable)
    }

    /// Why this presence is not usable, in one sentence a `Refusal` can
    /// print verbatim.
    ///
    /// "candidates tried: …" for [`ExecutablePresence::NotFound`] — the same
    /// phrase `glasshouse doctor` already prints for a harness nowhere on
    /// `PATH` — and the resolver's own reason for
    /// [`ExecutablePresence::Unusable`]. `id` is needed only to list
    /// candidate names; [`ExecutablePresence::Usable`] never calls this.
    pub fn detail(&self, id: IntegrationId) -> String {
        match self {
            Self::Usable => String::new(),
            Self::NotFound => {
                format!(
                    "candidates tried: {}",
                    id.executable_candidates().join(", ")
                )
            }
            Self::Unusable { reason } => reason.clone(),
        }
    }

    /// Check the real machine: every name
    /// [`IntegrationId::executable_candidates`] declares, against the real
    /// `PATH`, in priority order.
    pub fn detect(id: IntegrationId) -> Self {
        Self::detect_with(id.executable_candidates(), crate::platform::exec::resolve)
    }

    /// Core of [`ExecutablePresence::detect`], with the resolver injected so
    /// this can be exercised without depending on the real `PATH` — the same
    /// pattern `integrations::resolve_first_usable_with`'s own tests use.
    fn detect_with(
        candidates: &[&str],
        resolver: impl Fn(
            &str,
        ) -> Result<
            crate::platform::exec::ResolvedExecutable,
            crate::platform::exec::ResolveError,
        >,
    ) -> Self {
        use crate::platform::exec::ResolveError;

        let mut unusable_reason: Option<String> = None;
        for &name in candidates {
            match resolver(name) {
                Ok(_) => return Self::Usable,
                Err(
                    err @ (ResolveError::WindowsInteropOnly { .. }
                    | ResolveError::NotExecutable { .. }),
                ) => {
                    unusable_reason.get_or_insert_with(|| err.to_string());
                }
                Err(ResolveError::NotFound { .. }) => {}
            }
        }
        match unusable_reason {
            Some(reason) => Self::Unusable { reason },
            None => Self::NotFound,
        }
    }
}

#[cfg(test)]
mod tests;
