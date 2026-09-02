//! Phase 57B — map lines 1997-2003: the semantic reducer, through the
//! shipped binary against a canned OpenAI chat-completions endpoint on
//! loopback, exactly `tests/classification_call.rs`'s pattern for the same
//! reason: `DisposableRouting::choose` and the entitlement job-kind gate are
//! the production callers under test, and a fake `Reducer` built by hand
//! would not exercise either.
//!
//! The two flagship recall fixtures — the one relevant line the reducer
//! marks `uncertain` (safe mode must forward it) and the one it discards
//! outright (the rebuilt result must drop it, and `show` must still have it)
//! — are this file's centerpiece, per the package's own requirement.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clap::Parser;
use glasshouse::config::{EntitlementConfig, EntitlementCredential, ProviderConfig, UserConfig};
use glasshouse::routing::disposable::JobKind;
use glasshouse::{Cli, Runtime};

const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_FIREWALL_REDUCER_KEY";
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";

// ===========================================================================
// A canned OpenAI chat-completions endpoint — `classification_call.rs`'s
// exact shape, parsing the request itself rather than reusing anything in
// this crate.
// ===========================================================================

#[derive(Debug, Clone)]
struct Seen {
    body: String,
}

enum Answer {
    Content(String),
}

struct FakeModel {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
}

impl FakeModel {
    fn answering(content: &str) -> Self {
        let content = content.to_owned();
        Self::start(move |_| Answer::Content(content.clone()))
    }

    fn start(responder: impl Fn(&str) -> Answer + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback must bind");
        listener
            .set_nonblocking(true)
            .expect("the accept loop polls its stop flag");
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_seen = Arc::clone(&seen);
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        serve(stream, &thread_seen, &responder);
                    }
                    Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            address,
            seen,
            stop,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.address)
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for FakeModel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn serve(
    mut stream: TcpStream,
    seen: &Arc<Mutex<Vec<Seen>>>,
    responder: &(impl Fn(&str) -> Answer + ?Sized),
) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }

    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    seen.lock().unwrap().push(Seen { body: body.clone() });

    let response = match responder(&body) {
        Answer::Content(content) => {
            let document = serde_json::json!({
                "choices": [{ "message": { "role": "assistant", "content": content } }],
                "usage": { "prompt_tokens": 314, "completion_tokens": 15 }
            })
            .to_string();
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n{document}",
                document.len()
            )
        }
    };
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

// ===========================================================================
// A project, and the binary run against it.
// ===========================================================================

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &std::path::Path) -> Self {
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        std::fs::create_dir_all(base.join("config")).unwrap();

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();
        Fixture {
            base: base.to_path_buf(),
            root,
            runtime,
        }
    }

    fn config(&self) -> UserConfig {
        UserConfig::load(self.runtime.paths()).unwrap()
    }

    fn save(&self, user: UserConfig) {
        user.save(self.runtime.paths()).unwrap();
    }

    /// A free provider speaking OpenAI chat completions at `base_url`,
    /// naming `model` as a free candidate — `disposable_candidates`' own
    /// input shape.
    fn add_provider(&self, name: &str, model: &str, base_url: &str) {
        let mut user = self.config();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_base_url(Some(base_url.to_owned()));
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        provider.set_free_models(vec![model.to_owned()]);
        user.providers_mut().set(name, provider);
        self.save(user);
    }

    /// `[context_firewall] reducer = "<provider>"`, and the fields the
    /// semantic gate reads besides mode.
    fn set_reducer(&self, provider: &str, model: Option<&str>) {
        let mut user = self.config();
        user.context_firewall_mut()
            .set_reducer(Some(provider.to_owned()))
            .set_reducer_model(model.map(str::to_owned))
            .set_min_semantic_tokens(Some(1));
        self.save(user);
    }

    fn set_aggressive_drops_uncertain(&self, value: bool) {
        let mut user = self.config();
        user.context_firewall_mut()
            .set_aggressive_drops_uncertain(Some(value));
        self.save(user);
    }

    /// An entitlement naming `provider` that refuses
    /// [`JobKind::ContextReduction`] — the shipped-binary proof that map
    /// line 1947's per-entitlement job-kind rule applies to this job kind
    /// unchanged.
    fn deny_context_reduction_for(&self, entitlement_name: &str, provider: &str) {
        let mut user = self.config();
        let mut entitlement = EntitlementConfig::default();
        entitlement
            .set_provider(Some(provider.to_owned()))
            .set_credential(Some(EntitlementCredential::environment(CREDENTIAL_VAR)))
            .set_deny_job_kinds([JobKind::ContextReduction]);
        user.entitlements_mut().set(entitlement_name, entitlement);
        self.save(user);
    }

    fn run(&self, args: &[&str], stdin_bytes: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn glasshouse");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin_bytes)
            .expect("write stdin");
        child.wait_with_output().expect("wait for glasshouse")
    }

    /// Drive `context-firewall hook` with `event` on stdin, and parse the
    /// hook response. Always exits 0 — fail-open is part of what is under
    /// test.
    fn hook(&self, event: &serde_json::Value, extra_args: &[&str]) -> serde_json::Value {
        let mut args = vec!["context-firewall", "hook", "--emit-updated-output"];
        args.extend_from_slice(extra_args);
        let bytes = serde_json::to_vec(event).unwrap();
        let output = self.run(&args, &bytes);
        assert!(
            output.status.success(),
            "the hook must always exit 0 (fail open): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("hook response must be valid JSON")
    }

    fn show(&self, id: &str) -> String {
        let output = self.run(&["context-firewall", "show", id], b"");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

fn post_tool_use(
    tool_name: &str,
    tool_response: serde_json::Value,
    tool_input: serde_json::Value,
    session_id: &str,
    tool_use_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_response": tool_response,
        "tool_use_id": tool_use_id,
        "session_id": session_id,
        "cwd": "/tmp",
    })
}

fn text_response(text: &str) -> serde_json::Value {
    serde_json::json!({"type": "text", "text": text})
}

fn updated_output(response: &serde_json::Value) -> Option<&str> {
    response
        .get("hookSpecificOutput")
        .and_then(|v| v.get("updatedToolOutput"))
        .and_then(|v| v.as_str())
}

fn extract_raw_ref(text: &str) -> String {
    let start = text
        .find("gh-tool://")
        .expect("provenance header must state a raw ref");
    let rest = &text[start..];
    let end = rest.find(']').unwrap_or(rest.len());
    rest[..end].to_string()
}

/// A needle among thousands of duplicate hits — oversized enough to cross
/// `--min-semantic-tokens` after the deterministic ladder, exactly the
/// shape the flagship fixture needs.
fn needle_text() -> (String, &'static str) {
    let mut text = String::new();
    for _ in 0..2000 {
        text.push_str("distinct unique noise line long enough to add up quickly\n");
    }
    text.push_str("THE-ONE-RELEVANT-NEEDLE-LINE\n");
    (text, "THE-ONE-RELEVANT-NEEDLE-LINE")
}

// ===========================================================================
// The flagship recall fixtures.
// ===========================================================================

/// **Centerpiece, first half.** The reducer marks the one relevant candidate
/// `uncertain`; safe mode must forward it anyway (map lines 1999, 2000).
#[test]
fn safe_mode_forwards_a_needle_the_reducer_marked_uncertain() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let (text, needle) = needle_text();

    // The endpoint answers by finding the candidate id whose prompt line
    // names the needle, and marking exactly that id uncertain — every
    // other id relevant, so the assertion is about the needle's own
    // verdict, not about a lucky default.
    let needle_owned = needle.to_string();
    let model = FakeModel::start(move |body| {
        let id = candidate_id_for(body, &needle_owned).expect("the needle must be a candidate");
        Answer::Content(format!(
            r#"{{"selections":[{{"id":{id},"relevance":"uncertain","reason":"maybe"}}]}}"#
        ))
    });
    fixture.add_provider("fixture-provider", "fixture-model", &model.base_url());
    fixture.set_reducer("fixture-provider", None);

    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-needle",
        "tu-1",
    );
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");

    assert!(
        forwarded.contains(needle),
        "safe mode must forward a candidate the reducer only marked uncertain: {forwarded}"
    );
    assert_eq!(model.requests().len(), 1, "exactly one reducer call");
}

/// **Centerpiece, second half — the honest one.** The reducer discards the
/// one relevant candidate outright: the rebuilt result drops it, the
/// provenance header says so, and `show <id>` still has it, because the raw
/// store is written before the semantic stage ever runs.
#[test]
fn a_discarded_needle_is_dropped_but_show_still_has_it() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let (text, needle) = needle_text();

    let needle_owned = needle.to_string();
    let model = FakeModel::start(move |body| {
        let id = candidate_id_for(body, &needle_owned).expect("the needle must be a candidate");
        Answer::Content(format!(
            r#"{{"selections":[{{"id":{id},"relevance":"discard","reason":"noise"}}]}}"#
        ))
    });
    fixture.add_provider("fixture-provider", "fixture-model", &model.base_url());
    fixture.set_reducer("fixture-provider", None);

    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-discard",
        "tu-1",
    );
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");

    assert!(
        !forwarded.contains(needle),
        "an explicitly discarded candidate must not survive: {forwarded}"
    );
    assert!(
        forwarded.contains("semantic reduction by fixture-provider fixture-model kept"),
        "the provenance header must say the semantic stage ran, naming the reducer that \
         produced the reduction (Phase 58, map line 2030): {forwarded}"
    );

    let raw_ref = extract_raw_ref(forwarded);
    let raw = fixture.show(&raw_ref);
    assert!(
        raw.contains(needle),
        "`show` must still have the discarded evidence, from the untouched raw store: {raw}"
    );
}

// ===========================================================================
// Map line 2000: aggressive's explicit opt-in to drop uncertain.
// ===========================================================================

#[test]
fn aggressive_mode_keeps_uncertain_by_default_and_drops_it_only_when_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let (text, needle) = needle_text();

    let needle_owned = needle.to_string();
    let model = FakeModel::start(move |body| {
        let id = candidate_id_for(body, &needle_owned).expect("the needle must be a candidate");
        Answer::Content(format!(
            r#"{{"selections":[{{"id":{id},"relevance":"uncertain","reason":"maybe"}}]}}"#
        ))
    });
    fixture.add_provider("fixture-provider", "fixture-model", &model.base_url());
    fixture.set_reducer("fixture-provider", None);

    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-aggr",
        "tu-1",
    );
    let default_response = fixture.hook(
        &event,
        &[
            "--passthrough-tokens",
            "10",
            "--min-semantic-tokens",
            "10",
            "--mode",
            "aggressive",
        ],
    );
    let default_forwarded = updated_output(&default_response).expect("must reduce and emit");
    assert!(
        default_forwarded.contains(needle),
        "aggressive keeps uncertain by default, exactly like safe: {default_forwarded}"
    );

    fixture.set_aggressive_drops_uncertain(true);
    let event2 = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-aggr",
        "tu-2",
    );
    let dropped_response = fixture.hook(
        &event2,
        &[
            "--passthrough-tokens",
            "10",
            "--min-semantic-tokens",
            "10",
            "--mode",
            "aggressive",
        ],
    );
    let dropped_forwarded = updated_output(&dropped_response).expect("must reduce and emit");
    assert!(
        !dropped_forwarded.contains(needle),
        "an explicit opt-in must let aggressive drop uncertain: {dropped_forwarded}"
    );
}

// ===========================================================================
// Map line 2001: fail open.
// ===========================================================================

#[test]
fn an_unreachable_reducer_fails_open_to_the_deterministic_result() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let (text, needle) = needle_text();

    // A provider whose base URL is a loopback port nothing listens on.
    fixture.add_provider("fixture-provider", "fixture-model", "http://127.0.0.1:1/v1");
    fixture.set_reducer("fixture-provider", None);

    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-unreachable",
        "tu-1",
    );
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let forwarded = updated_output(&response).expect("must still reduce and emit");
    assert!(
        forwarded.contains(needle),
        "a reducer that cannot be reached must never lose the deterministic result: {forwarded}"
    );
    assert!(
        forwarded.contains("semantic reduction bypassed"),
        "the header must say the semantic stage was attempted and bypassed: {forwarded}"
    );
}

#[test]
fn a_malformed_reducer_reply_fails_open() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let (text, needle) = needle_text();

    let model = FakeModel::answering("I'm sorry, I can't help with that.");
    fixture.add_provider("fixture-provider", "fixture-model", &model.base_url());
    fixture.set_reducer("fixture-provider", None);

    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-malformed",
        "tu-1",
    );
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let forwarded = updated_output(&response).expect("must still reduce and emit");
    assert!(forwarded.contains(needle));
    assert!(forwarded.contains("semantic reduction bypassed (reducer-schema)"));
    assert_eq!(model.requests().len(), 1, "a real call was actually made");
}

// ===========================================================================
// Map line 1997: per-entitlement job-kind rules apply unchanged.
// ===========================================================================

#[test]
fn an_entitlement_that_denies_context_reduction_is_never_asked() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let (text, needle) = needle_text();

    let model = FakeModel::answering(
        r#"{"selections":[{"id":0,"relevance":"discard","reason":"should never be asked"}]}"#,
    );
    fixture.add_provider("fixture-provider", "fixture-model", &model.base_url());
    fixture.set_reducer("fixture-provider", None);
    fixture.deny_context_reduction_for("no-reduce", "fixture-provider");

    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-denied",
        "tu-1",
    );
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let forwarded = updated_output(&response).expect("must still reduce and emit");

    assert!(
        model.requests().is_empty(),
        "an entitlement that denies the job kind must mean no call is ever made"
    );
    assert!(
        forwarded.contains(needle),
        "with no reducer selectable, the deterministic result stands: {forwarded}"
    );
}

// ===========================================================================
// Map line 2002: a pinned model is used.
// ===========================================================================

#[test]
fn a_pinned_reducer_model_reaches_the_wire() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let (text, _needle) = needle_text();

    let model =
        FakeModel::answering(r#"{"selections":[{"id":0,"relevance":"relevant","reason":"x"}]}"#);
    // Two free models on the same provider; the pin must be what is asked.
    let mut user = fixture.config();
    let mut provider = ProviderConfig::new("openai-compatible");
    provider.set_base_url(Some(model.base_url()));
    provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
    provider.set_free_models(vec!["model-a".to_owned(), "model-b".to_owned()]);
    user.providers_mut().set("fixture-provider", provider);
    fixture.save(user);
    fixture.set_reducer("fixture-provider", Some("model-b"));

    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-pin",
        "tu-1",
    );
    fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );

    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].body.contains("model-b") && !requests[0].body.contains("model-a"),
        "the pinned model must be what is asked: {}",
        requests[0].body
    );
}

// ===========================================================================
// Map line 2003: the privacy gate.
// ===========================================================================

#[test]
fn a_secret_shaped_path_suppresses_semantic_reduction_entirely() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let (text, needle) = needle_text();

    let model = FakeModel::answering(
        r#"{"selections":[{"id":0,"relevance":"discard","reason":"should never be asked"}]}"#,
    );
    fixture.add_provider("fixture-provider", "fixture-model", &model.base_url());
    fixture.set_reducer("fixture-provider", None);

    let event = post_tool_use(
        "Read",
        text_response(&text),
        serde_json::json!({"file_path": ".env"}),
        "s-secret",
        "tu-1",
    );
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let forwarded = updated_output(&response).expect("must still reduce and emit");

    assert!(
        model.requests().is_empty(),
        "a .env-shaped path must suppress the reducer call entirely"
    );
    assert!(forwarded.contains(needle));
}

// ===========================================================================
// Map line 1998: never the conversational transcript, only task/tool/query.
// ===========================================================================

#[test]
fn the_reducer_request_carries_only_task_tool_and_candidates() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let (text, _needle) = needle_text();

    let model =
        FakeModel::answering(r#"{"selections":[{"id":0,"relevance":"relevant","reason":"x"}]}"#);
    fixture.add_provider("fixture-provider", "fixture-model", &model.base_url());
    fixture.set_reducer("fixture-provider", None);

    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({"pattern": "a-search-pattern"}),
        "s-shape",
        "tu-1",
    );
    fixture.hook(
        &event,
        &[
            "--passthrough-tokens",
            "10",
            "--min-semantic-tokens",
            "10",
            "--task",
            "a stated task",
        ],
    );

    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    let body = &requests[0].body;
    assert!(body.contains("a stated task"), "{body}");
    assert!(body.contains("Grep"), "{body}");
    assert!(body.contains("a-search-pattern"), "{body}");
    assert!(
        !body.contains("s-shape"),
        "the session id is not part of the request the model sees: {body}"
    );
}

/// Find the candidate id whose prompt line contains `needle` — the request
/// body carries lines shaped `id=<N> text=<candidate text>`, per
/// `firewall::reducer`'s own prompt template.
fn candidate_id_for(request_body: &str, needle: &str) -> Option<usize> {
    // The request body is a JSON document whose `messages[0].content` field
    // is the prompt; decoded as JSON first so escaped newlines inside it
    // are read correctly.
    let document: serde_json::Value = serde_json::from_str(request_body).ok()?;
    let prompt = document.get("messages")?.get(0)?.get("content")?.as_str()?;
    for line in prompt.lines() {
        if line.starts_with("id=") && line.contains(needle) {
            let rest = line.strip_prefix("id=")?;
            let id_str = rest.split(' ').next()?;
            return id_str.parse().ok();
        }
    }
    None
}
