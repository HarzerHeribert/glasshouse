//! GH-PROVE-IT-39 — Phase 39's disposable-job interface, proved rather than
//! built.
//!
//! Every line below (1621-1628) is a claim the codebase already satisfies —
//! see `.agent-runtime/report-map-side-effect-audit.md`'s Phase 39 rows and
//! `docs/product/evidence/phase-39.md`. This file adds one test and one
//! mutation per line; nothing here changes production behaviour.
//!
//! Several lines (1621, 1626, 1627, 1628) are structural or definitional
//! rather than behavioural — the same shape `docs/product/evidence/phase-9e.md`
//! used for `SecretRef`: a scan over the type declaration or the source file
//! itself, with a mutation that introduces the forbidden shape and shows the
//! scan catches it.

use glasshouse::harness::Declared;
use glasshouse::memory::extract::chunk::{ChunkLimits, SessionChunk};
use glasshouse::memory::extract::{
    ExtractionModel, ExtractionTrigger, Extractor, ModelError, Prompt,
};
use glasshouse::memory::{ConfiguredModel, ConfiguredModelError, ProjectMemory};
use glasshouse::provider::{ProtocolSupport, Provider};
use glasshouse::routing::disposable::JobKind;
use glasshouse::{Cli, Runtime};

use clap::Parser as _;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// 1621 — disposable jobs are a closed, bounded set of internal calls.
// ---------------------------------------------------------------------------

/// `JobKind`'s variant set is exactly {Classification, MemoryExtraction,
/// Reranking, Evaluation, ContextReduction} and every one is a name, not a
/// session: the exhaustive match below stops compiling the day a variant is
/// added without updating this test, which is what makes the list a claim
/// about *all* of them rather than the ones somebody remembered.
///
/// `ContextReduction` (Phase 57B, map line 1997) is this roster's designed
/// tripwire firing: adding a fifth variant broke this match on purpose, and
/// updating it here is the signal — per this line's own doc comment and
/// Phase 39's 1625 refusal — to re-read whether 1625 is now reachable. It is
/// not: 1625 is about *reranking* (a job that reorders candidates), and the
/// context reducer never reorders anything — it only keeps or drops the
/// candidates it is given, by id. `docs/product/evidence/phase-39.md`'s
/// 1625 row stays open, and this package does not close it.
///
/// `DisposableRouting::choose` takes a `JobKind` by value — there is no
/// "unknown kind" it could be handed, because the type has no variant able to
/// represent one. That refusal is structural (the type system), and this
/// test is its falsifiable half: the names below are the only vocabulary
/// `choose` can ever be called with.
///
/// Mutation: `JobKind::as_str`'s `Reranking` arm, `"reranking"` ->
/// `"unknown"`.
#[test]
fn job_kind_is_a_closed_vocabulary_of_bounded_internal_calls() {
    let kinds = [
        JobKind::Classification,
        JobKind::MemoryExtraction,
        JobKind::Reranking,
        JobKind::Evaluation,
        JobKind::ContextReduction,
    ];

    // Exhaustiveness: a sixth variant stops this compiling.
    for kind in kinds {
        match kind {
            JobKind::Classification
            | JobKind::MemoryExtraction
            | JobKind::Reranking
            | JobKind::Evaluation
            | JobKind::ContextReduction => {}
        }
    }

    let names: Vec<&str> = kinds.iter().map(|kind| kind.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "classification",
            "memory extraction",
            "reranking",
            "evaluation",
            "context-reduction",
        ],
        "the closed vocabulary a disposable job may name itself with has changed"
    );

    // None of the four names anything session-shaped or PTY-shaped — a
    // disposable job is bounded internal work, never a native interactive
    // session (map lines 1621, 1626).
    for name in &names {
        assert!(
            !name.contains("session") && !name.contains("pty"),
            "a JobKind variant now names something session-shaped: {name}"
        );
    }
}

// ---------------------------------------------------------------------------
// 1622 — a simple provider interface: `ExtractionModel` is the whole seam.
// ---------------------------------------------------------------------------

/// Implements exactly `ExtractionModel::describe` and `::complete`, relying
/// on the trait's own default for `complete_observed` — the minimal
/// implementation the trait's doc comment says a Phase 39 provider must
/// supply, and nothing more.
struct ProbeModel {
    describe_calls: AtomicUsize,
    complete_calls: AtomicUsize,
}

impl ProbeModel {
    fn new() -> Self {
        Self {
            describe_calls: AtomicUsize::new(0),
            complete_calls: AtomicUsize::new(0),
        }
    }
}

impl ExtractionModel for ProbeModel {
    fn describe(&self) -> String {
        self.describe_calls.fetch_add(1, Ordering::SeqCst);
        "test/probe-model-exactly-the-trait".to_owned()
    }

    fn complete(&self, _prompt: &Prompt) -> Result<String, ModelError> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        Ok(r#"{"memories": []}"#.to_owned())
    }
}

fn bootstrap_runtime(base: &std::path::Path) -> Runtime {
    let root = base.join("project");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    glasshouse::bootstrap(&cli, &root).unwrap()
}

/// A minimal implementation of `ExtractionModel` — describe + complete only —
/// drives `Extractor::run` end to end: `describe()` names the resource on the
/// outcome, and `complete()` (reached through the trait's default
/// `complete_observed`) answers the prompt that gets parsed into the outcome.
/// Both are the whole interface Phase 39 line 1622 asks for.
///
/// Mutation: `Extractor::run`'s `self.model.describe()` ->
/// `String::new()` — removes the interface's first method from the call
/// Phase 39 requires.
#[test]
fn a_minimal_extraction_model_impl_drives_extractor_run_through_both_its_methods() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = bootstrap_runtime(tmp.path());
    let memory = ProjectMemory::open(&runtime).unwrap();
    let store = memory.store();

    let model = ProbeModel::new();
    let chunk = SessionChunk::build(
        "session-probe",
        Some("deadbeef"),
        vec!["something happened during the session".to_owned()],
        ChunkLimits::default(),
    );

    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    assert_eq!(
        outcome.model, "test/probe-model-exactly-the-trait",
        "Extractor::run must call ExtractionModel::describe() to name the resource"
    );
    assert_eq!(
        model.describe_calls.load(Ordering::SeqCst),
        1,
        "describe() must be called exactly once per run"
    );
    assert_eq!(
        model.complete_calls.load(Ordering::SeqCst),
        1,
        "complete() (via the trait's default complete_observed) must be called once per run"
    );
    assert!(
        outcome.failure.is_none(),
        "a minimal describe+complete implementation must be enough to answer: {:?}",
        outcome.failure
    );
}

// ---------------------------------------------------------------------------
// 1623 — an OpenAI-compatible gateway: `{base_url}/chat/completions`.
// ---------------------------------------------------------------------------

fn provider(name: &str, base_url: &str) -> Provider {
    Provider {
        name: name.to_owned(),
        protocols: vec![ProtocolSupport {
            protocol: glasshouse::harness::WireProtocol::OpenAiChat,
            base_url: base_url.to_owned(),
            streaming: Declared::Unverified,
            tool_calls: Declared::Unverified,
            reasoning: Declared::Unverified,
        }],
        model_list_endpoint: Declared::Unverified,
        usage_telemetry: Declared::Unverified,
        credential_env: vec![],
        headers: vec![],
    }
}

/// `ConfiguredModel` is built from any provider that names the OpenAI
/// chat-completions protocol, and the endpoint is exactly `{base_url}/chat/
/// completions` — proved black-box, through `Debug` (the only surface that
/// exposes the endpoint outside the module; the field itself is private).
///
/// Mutation: `ConfiguredModel::new`'s `endpoint: format!("{base_url}/chat/
/// completions")` -> `format!("{base_url}/v1/completions")`.
#[test]
fn an_openai_compatible_gateway_reaches_the_chat_completions_path() {
    let built = ConfiguredModel::new(
        &provider("hosted-gateway", "https://gateway.example.invalid/v1"),
        "a-configured-model",
        None,
    )
    .expect("a well-formed openai-chat provider must build");

    let rendered = format!("{built:?}");
    assert!(
        rendered.contains("https://gateway.example.invalid/v1/chat/completions"),
        "the endpoint must be the base URL with /chat/completions appended: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// 1624 — a local Ollama/llama.cpp endpoint is nothing special.
// ---------------------------------------------------------------------------

/// Ollama's real default port and path (`http://127.0.0.1:11434/v1`) round-
/// trips through exactly the same generic construction as any other OpenAI-
/// compatible gateway — no code anywhere branches on that port or on the
/// words "ollama" or "llama.cpp".
///
/// Mutation: `ConfiguredModel::new`'s endpoint construction special-cased for
/// port 11434 (`if base_url.contains("11434") { .../api/chat } else {
/// .../chat/completions }`).
#[test]
fn a_local_ollama_endpoint_round_trips_with_no_port_special_case() {
    let built = ConfiguredModel::new(
        &provider("local-ollama", "http://127.0.0.1:11434/v1"),
        "qwen2.5-coder:7b",
        None,
    )
    .expect("a local runner needs no credential");

    let rendered = format!("{built:?}");
    assert!(
        rendered.contains("http://127.0.0.1:11434/v1/chat/completions"),
        "Ollama's default endpoint must reach the generic chat-completions path, unmodified: \
         {rendered}"
    );
    assert_eq!(
        built.describe(),
        "local-ollama/qwen2.5-coder:7b via openai-chat",
        "describe() must say the same generic 'via openai-chat' route it says for any other \
         OpenAI-compatible provider"
    );
}

/// `ConfiguredModel::new` also refuses on the two protocols this module does
/// not speak, regardless of host — proving the local/hosted distinction is
/// not encoded in the protocol check either.
#[test]
fn a_non_openai_chat_protocol_is_refused_even_on_a_loopback_host() {
    let mut local_but_wrong_protocol = provider("local-other", "http://127.0.0.1:11434/v1");
    local_but_wrong_protocol.protocols[0].protocol =
        glasshouse::harness::WireProtocol::AnthropicMessages;

    let err = ConfiguredModel::new(&local_but_wrong_protocol, "a-model", None)
        .expect_err("only openai-chat is spoken here, loopback or not");
    assert!(matches!(
        err,
        ConfiguredModelError::UnsupportedProtocol { .. }
    ));
}

// ---------------------------------------------------------------------------
// 1625 — the four uses, each with a production caller.
// ---------------------------------------------------------------------------

/// Phase 39 line 1625 names four uses: classification, memory extraction,
/// reranking, and "other bounded support tasks". Each now has a real
/// production caller through `DisposableRouting`:
///
/// - classification — `main.rs::choose_for_automatic_classification`;
/// - memory extraction — `main.rs`'s extraction seat, `JobKind::MemoryExtraction`;
/// - reranking — the seat in the library, `memory/rerank.rs::resolve_rerank_model`,
///   `JobKind::Reranking` (landed 2026-09-02, `GH-MEMORY-RERANKER`; it lives in
///   the library because the machine door cannot call the binary crate);
/// - another bounded support task — the context-firewall reducer,
///   `main.rs::disposable_reducer`, `JobKind::ContextReduction`.
///
/// This test was a tripwire until 2026-09-02: it asserted that *no* reranking
/// caller existed, so that the day one was added it would fail by name and
/// send line 1625 back for a ruling rather than stay silently green. It fired
/// in the waves 101–102 trailing sweep exactly as designed, and the ruling is
/// in `docs/product/evidence/phase-39.md`. It is now the census of the four
/// callers; the behaviour of each seat is proven by its own shipped-binary
/// tests (`classification_call.rs`, `routed_extraction.rs`,
/// `memory_reranker.rs`, `firewall_reducer.rs`), not by this scan.
#[test]
fn disposable_jobs_serve_classification_extraction_reranking_and_reduction_in_production() {
    let main_source = include_str!("../src/main.rs");
    let rerank_source = include_str!("../src/memory/rerank.rs");

    assert!(
        main_source.contains("JobKind::MemoryExtraction"),
        "memory extraction's production caller must still route through JobKind"
    );
    assert!(
        main_source.contains("choose_for_automatic_classification"),
        "classification's production caller must still route through DisposableRouting"
    );
    assert!(
        rerank_source.contains("JobKind::Reranking"),
        "the reranking seat must still route through JobKind::Reranking — capability map \
         line 1625's reranking clause rests on it"
    );
    assert!(
        main_source.contains("JobKind::ContextReduction"),
        "the context-firewall reducer must still route through JobKind::ContextReduction — \
         line 1625's other bounded support task"
    );
}

// ---------------------------------------------------------------------------
// 1626, 1627, 1628 — distinct from a harness session, structurally.
// ---------------------------------------------------------------------------

/// A structural scan of `src/memory/extract/model.rs`'s production code (the
/// portion before its own `#[cfg(test)]` module, so a fixture's incidental
/// `tool_calls: Declared::Unverified` field does not count) — the same shape
/// `docs/product/evidence/phase-9e.md` used for `SecretRef`: the property is
/// the *absence* of a shape, proved by scanning the source that would have to
/// carry it.
///
/// - No `Pty` surface (map line 1626: distinct from a harness session).
/// - No `SessionId` (map line 1627: no native-session identity).
/// - No `tool_calls`/`function_call` surface (map line 1627: no unrestricted
///   repository tools, no autonomous coding-agent loop).
/// - `"stream": false` is present (map line 1628: one bounded call, not a
///   user-enterable, ongoing conversation).
///
/// Mutation: the doc comment on `ConfiguredModel::new`'s `credential`
/// parameter — "`credential` is resolved by the caller, because resolving a"
/// -> "...because resolving a SessionId or a" — introduces exactly the
/// forbidden word into the scanned file, in the shape this test would have
/// to catch wherever it appeared.
#[test]
fn disposable_model_calls_carry_no_pty_session_or_tool_surface() {
    let full_source = include_str!("../src/memory/extract/model.rs");
    let production_source = full_source
        .split("#[cfg(test)]")
        .next()
        .expect("split always yields at least one piece");

    for forbidden in ["Pty", "SessionId", "tool_calls", "function_call"] {
        assert!(
            !production_source.contains(forbidden),
            "src/memory/extract/model.rs's production code must carry no {forbidden} surface — \
             a disposable job is not a harness session"
        );
    }

    assert!(
        production_source.contains("\"stream\": false"),
        "a disposable job's call must be non-streaming: one bounded request, not an ongoing \
         session"
    );
}
