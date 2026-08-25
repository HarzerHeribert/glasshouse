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
pub mod pi;

use std::ffi::OsString;

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
/// names rhyme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Anthropic,
    OpenAi,
    Google,
    Cursor,
    OpenCode,
    Pi,
    Hermes,
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
}

impl WireProtocol {
    pub fn slug(self) -> &'static str {
        match self {
            WireProtocol::AnthropicMessages => "anthropic-messages",
            WireProtocol::OpenAiResponses => "openai-responses",
            WireProtocol::OpenAiChat => "openai-chat",
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
    pub communication_style: Declared<CommunicationStyle>,
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
        IntegrationId::Cmux | IntegrationId::Ollama | IntegrationId::LlamaCpp => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Production source of a module, with its test module and its comments
    /// removed.
    ///
    /// Comments are stripped because the architectural rules below are about
    /// what the code *depends on*, not what its prose mentions. `session/store`
    /// documents that it holds an `IntegrationId`'s string form, which is the
    /// architecture working, not breaking it — a scan that could not tell those
    /// apart would punish the comment that explains the boundary.
    fn production_code(source: &str) -> String {
        source
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields at least one part")
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // --- the catalogue and its adapters ---------------------------------

    #[test]
    fn every_harness_has_an_adapter_and_nothing_else_does() {
        for &id in IntegrationId::ALL {
            match id.kind() {
                IntegrationKind::Harness => {
                    let adapter = adapter_for(id)
                        .unwrap_or_else(|| panic!("{} is a harness with no adapter", id.slug()));
                    assert_eq!(
                        adapter.id(),
                        id,
                        "the adapter registered for {} reports a different identity",
                        id.slug()
                    );
                }
                IntegrationKind::Multiplexer | IntegrationKind::LocalInference => {
                    assert!(
                        adapter_for(id).is_none(),
                        "{} is not a harness but has an adapter",
                        id.slug()
                    );
                }
            }
        }
    }

    #[test]
    fn all_lists_exactly_the_harness_adapters() {
        let listed: Vec<IntegrationId> = all().map(|a| a.id()).collect();
        let harnesses: Vec<IntegrationId> = IntegrationId::ALL
            .iter()
            .copied()
            .filter(|id| id.kind() == IntegrationKind::Harness)
            .collect();
        assert_eq!(listed, harnesses);
    }

    #[test]
    fn no_two_adapters_claim_the_same_executable_name() {
        let mut seen: Vec<(&str, IntegrationId)> = Vec::new();
        for adapter in all() {
            for &name in adapter.executable_candidates() {
                if let Some((_, other)) = seen.iter().find(|(n, _)| *n == name) {
                    panic!(
                        "`{name}` is claimed by both {} and {}: PATH discovery would resolve \
                         one harness as the other",
                        other.slug(),
                        adapter.id().slug()
                    );
                }
                seen.push((name, adapter.id()));
            }
        }
    }

    #[test]
    fn every_adapter_names_at_least_one_executable() {
        for adapter in all() {
            let candidates = adapter.executable_candidates();
            assert!(
                !candidates.is_empty(),
                "{} names no executable, so it can never be found",
                adapter.id().slug()
            );
            for name in candidates {
                assert!(!name.trim().is_empty());
                assert!(
                    !name.contains(std::path::MAIN_SEPARATOR),
                    "{} names `{name}`, which is a path and not a PATH-searchable name",
                    adapter.id().slug()
                );
            }
        }
    }

    /// The catalogue must ask the adapter, not keep its own copy.
    #[test]
    fn the_catalogue_takes_harness_executable_names_from_the_adapter() {
        for adapter in all() {
            assert_eq!(
                adapter.id().executable_candidates(),
                adapter.executable_candidates(),
                "{} would be searched for under a different name than its adapter declares",
                adapter.id().slug()
            );
        }
    }

    // --- declarations are evidence --------------------------------------

    #[test]
    fn every_verified_declaration_cites_its_evidence() {
        // A `Verified` with an empty evidence string is the exact failure this
        // type exists to prevent: a claim with nothing behind it, wearing the
        // word "verified".
        fn check(what: &str, evidence: Option<&'static str>) {
            if let Some(evidence) = evidence {
                assert!(
                    evidence.trim().len() > 20,
                    "{what} is declared verified but cites no usable evidence: {evidence:?}"
                );
            }
        }

        for adapter in all() {
            let slug = adapter.id().slug();
            let d = adapter.describe();
            check(&format!("{slug} vendor"), d.vendor.evidence());
            check(&format!("{slug} hooks"), d.hooks.evidence());
            check(&format!("{slug} session ids"), d.session_ids.evidence());
            check(
                &format!("{slug} protocols"),
                d.backends.protocols.evidence(),
            );
            check(
                &format!("{slug} model override"),
                d.backends.model_override.evidence(),
            );
            check(
                &format!("{slug} backend selection"),
                d.backends.selection.evidence(),
            );
            check(
                &format!("{slug} communication style"),
                d.communication_style.evidence(),
            );
            for (name, declared) in d.capabilities.named() {
                check(&format!("{slug} {name}"), declared.evidence());
            }
        }
    }

    #[test]
    fn a_verified_hook_mechanism_is_never_empty() {
        for adapter in all() {
            if let Some(hooks) = adapter.describe().hooks.value() {
                assert!(
                    !hooks.mechanism.trim().is_empty(),
                    "{} declares hooks with no mechanism to configure them",
                    adapter.id().slug()
                );
            }
        }
    }

    #[test]
    fn a_verified_backend_declaration_is_never_an_empty_list() {
        for adapter in all() {
            let backends = adapter.describe().backends;
            if let Some(protocols) = backends.protocols.value() {
                assert!(!protocols.is_empty(), "{}", adapter.id().slug());
            }
            if let Some(overrides) = backends.model_override.value() {
                assert!(!overrides.is_empty(), "{}", adapter.id().slug());
            }
            if let Some(selection) = backends.selection.value() {
                assert!(!selection.is_empty(), "{}", adapter.id().slug());
            }
        }
    }

    #[test]
    fn unverified_declarations_carry_no_value_and_no_evidence() {
        let unverified: Declared<Vendor> = Declared::Unverified;
        assert!(unverified.value().is_none());
        assert!(unverified.evidence().is_none());
        assert!(!unverified.is_verified());
    }

    #[test]
    fn an_unverified_capability_is_not_treated_as_present() {
        let unverified: Declared<bool> = Declared::Unverified;
        assert!(!unverified.is_known_present());
        let absent = Declared::verified(false, "checked and it is not there");
        assert!(!absent.is_known_present());
        let present = Declared::verified(true, "checked and it is there");
        assert!(present.is_known_present());
    }

    // --- starting and resuming ------------------------------------------

    #[test]
    fn no_supported_harness_needs_an_argument_to_start_today() {
        // Every one of them opens an interactive session when run bare, and
        // Glasshouse has already put the child in the project root. If this
        // ever stops being true for a harness, that is a decision to make
        // deliberately rather than to discover in a session that came up
        // wrong.
        for adapter in all() {
            assert!(
                adapter.start().is_bare(),
                "{} now needs a start argument; update its adapter and this test together",
                adapter.id().slug()
            );
        }
    }

    #[test]
    fn resume_passes_the_identifier_as_one_whole_argument() {
        // Glued on with `=`, an identifier beginning with a dash could be
        // re-read as a flag, and one containing a space would split. Its own
        // argv entry cannot do either.
        let id = "9f1c0b2e-0000-4000-8000-0123456789ab";
        for adapter in all() {
            let Some(invocation) = adapter.resume(id) else {
                continue;
            };
            let args: Vec<String> = invocation
                .args()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert!(
                args.iter().any(|a| a == id),
                "{} does not pass the identifier as its own argument: {args:?}",
                adapter.id().slug()
            );
            assert_eq!(
                args.last().map(String::as_str),
                Some(id),
                "{} puts something after the identifier",
                adapter.id().slug()
            );
        }
    }

    #[test]
    fn every_supported_harness_can_be_resumed() {
        // All seven document a resume mechanism in their own `--help`. This is
        // what makes Phase 7's resume work possible at all, so losing one
        // should be loud.
        for adapter in all() {
            assert!(
                adapter.resume("some-id").is_some(),
                "{} lost its resume mechanism",
                adapter.id().slug()
            );
        }
    }

    /// Each harness's resume shape, exactly as its installed binary documents
    /// it. These four differ from one another in ways that matter — a flag, a
    /// subcommand, a differently-spelled flag — which is the whole reason the
    /// adapter layer exists.
    #[test]
    fn resume_shapes_match_the_installed_binaries() {
        let cases: [(IntegrationId, &[&str]); 7] = [
            (IntegrationId::ClaudeCode, &["--resume", "ID"]),
            (IntegrationId::Codex, &["resume", "ID"]),
            (IntegrationId::Antigravity, &["--conversation", "ID"]),
            (IntegrationId::OpenCode, &["--session", "ID"]),
            (IntegrationId::Cursor, &["--resume", "ID"]),
            (IntegrationId::Pi, &["--session", "ID"]),
            (IntegrationId::Hermes, &["--resume", "ID"]),
        ];
        for (id, expected) in cases {
            let adapter = adapter_for(id).expect("a harness");
            let invocation = adapter.resume("ID").expect("a resume mechanism");
            let args: Vec<String> = invocation
                .args()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert_eq!(args, expected, "{} resumes differently", id.slug());
        }
    }

    #[test]
    fn the_executable_names_match_the_installed_binaries() {
        // `agy` in particular: the Antigravity CLI's published package links
        // its binary onto PATH under that name, and Glasshouse searched only
        // for `antigravity` until a real install proved otherwise.
        assert_eq!(
            adapter_for(IntegrationId::Antigravity)
                .expect("a harness")
                .executable_candidates(),
            &["agy", "antigravity"]
        );
        assert_eq!(
            adapter_for(IntegrationId::Cursor)
                .expect("a harness")
                .executable_candidates(),
            &["cursor-agent"]
        );
    }

    // --- messaging and interrupting -------------------------------------

    #[test]
    fn a_message_is_the_text_and_then_a_carriage_return() {
        for adapter in all() {
            let message = adapter.message("run the tests");
            assert_eq!(
                message.bytes(),
                b"run the tests\r",
                "{}",
                adapter.id().slug()
            );
        }
    }

    #[test]
    fn an_interrupt_is_the_terminal_interrupt_byte() {
        for adapter in all() {
            assert_eq!(
                adapter.interrupt().bytes(),
                &[0x03],
                "{}",
                adapter.id().slug()
            );
        }
    }

    // --- the architecture the map fixes ---------------------------------

    /// "Make the generic PTY runtime independent from any specific harness
    /// adapter."
    #[test]
    fn the_generic_pty_runtime_depends_on_no_adapter() {
        let modules = [
            ("pty/mod.rs", include_str!("../pty/mod.rs")),
            ("pty/process.rs", include_str!("../pty/process.rs")),
            ("session/runtime.rs", include_str!("../session/runtime.rs")),
        ];
        for (name, source) in modules {
            let code = production_code(source);
            for forbidden in ["HarnessAdapter", "crate::harness", "IntegrationId"] {
                assert!(
                    !code.contains(forbidden),
                    "{name} names `{forbidden}` in production code: the generic runtime has \
                     become dependent on a harness adapter"
                );
            }
        }
    }

    /// "Keep adapter-specific parsing isolated from the core Glasshouse
    /// session model."
    #[test]
    fn the_session_model_depends_on_no_adapter() {
        let code = production_code(include_str!("../session/store.rs"));
        for forbidden in ["HarnessAdapter", "crate::harness", "IntegrationId"] {
            assert!(
                !code.contains(forbidden),
                "session/store.rs names `{forbidden}` in production code: the session model \
                 has become dependent on a harness adapter"
            );
        }
    }

    /// The scan above is only worth having if it can fail.
    #[test]
    fn the_dependency_scan_would_catch_a_violation() {
        let violating = "fn spawn() {\n    if id == IntegrationId::ClaudeCode { todo!() }\n}";
        assert!(production_code(violating).contains("IntegrationId"));
        // ... and does not fire on a doc comment that merely mentions one.
        let documented = "/// Holds an [`IntegrationId`] as a string.\nfn spawn() {}";
        assert!(!production_code(documented).contains("IntegrationId"));
        // ... nor on a test.
        let tested = "fn spawn() {}\n#[cfg(test)]\nmod tests { use IntegrationId; }";
        assert!(!production_code(tested).contains("IntegrationId"));
    }
}
