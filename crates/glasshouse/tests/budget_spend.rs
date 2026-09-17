//! Capability map lines 1263 and 1519 — counting money spent against a
//! provider's own `[providers.<name>.quota] budget`, through the shipped
//! binary.
//!
//! Money is a read-time product of recorded tokens (`routing_observations`)
//! and `pricing.toml` rates, over the budget's own period
//! (`provider::telemetry::budget_period_start`). This file plants both —
//! ledger rows via a bootstrapped [`Runtime`], a real `pricing.toml` on
//! disk — and drives `glasshouse resources`.
//!
//! The destination-ranking (`glasshouse route`) and support-work-dispatch
//! preview (`glasshouse resources --no-harness`'s "would select" line) this
//! file used to exercise budget exhaustion through are both gone with the
//! routing deletion (design-decisions, 2026-09-16); the exhaustion refusal
//! itself survives at launch time (`commands::shared::entitlement_refusal_message`)
//! and through the context-firewall reducer, proven below.
//!
//! Each test is its own `Binary`: sharing one across tests would let a
//! ledger row planted for one budget's period leak into another's window.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clap::Parser;

use glasshouse::routing::evidence::{EvidenceLedger, NewObservation, Outcome};
use glasshouse::{Cli, Runtime};

const VAR: &str = "GLASSHOUSE_BUDGET_SPEND_TEST_KEY";
const FREE_VAR: &str = "GLASSHOUSE_BUDGET_SPEND_TEST_FREE_KEY";

// ---------------------------------------------------------------------------
// The fixture: a bootstrapped project the binary and this test share, so
// ledger rows planted in-process are the exact rows the child process reads.
// ---------------------------------------------------------------------------

struct Binary {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Binary {
    fn with_config(extra: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!("version = 1\n\n{extra}"),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn with_pricing(self, toml: &str) -> Self {
        std::fs::write(self.base.join("config").join("pricing.toml"), toml)
            .expect("write pricing.toml");
        self
    }

    /// Overwrite `config.toml` with a new `extra` body — for a test that
    /// changes a budget between two hook runs of the same fixture, the same
    /// project the first write already bootstrapped.
    fn rewrite_config(&self, extra: &str) {
        std::fs::write(
            self.base.join("config").join("config.toml"),
            format!("version = 1\n\n{extra}"),
        )
        .expect("rewrite user config");
    }

    /// Drive `context-firewall hook` with `event` on stdin, and parse the
    /// hook response — `tests/firewall_reducer.rs`'s own `Fixture::hook`.
    /// Always exits 0 — fail-open is part of what every reducer test here
    /// proves too.
    fn hook(&self, event: &serde_json::Value, extra_args: &[&str]) -> serde_json::Value {
        let mut args = vec!["context-firewall", "hook", "--emit-updated-output"];
        args.extend_from_slice(extra_args);
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .env(VAR, "sk-planted-budget-spend-test")
            .env(FREE_VAR, "sk-planted-budget-spend-test-free")
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn glasshouse");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(event).unwrap())
            .expect("write stdin");
        let output = child.wait_with_output().expect("wait for glasshouse");
        assert!(
            output.status.success(),
            "the hook must always exit 0 (fail open): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("hook response must be valid JSON")
    }

    /// A bootstrapped runtime over this fixture's own directories, for
    /// planting evidence-ledger rows before the binary runs — practice §65,
    /// opened, used and dropped here.
    fn runtime(&self) -> Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        glasshouse::bootstrap(&cli, &self.root).unwrap()
    }

    /// Plant one served exchange with `input`/`output` tokens against
    /// `provider`/`model`, `age_seconds` in the past — recent enough to sit
    /// inside any budget period this file tests with.
    fn plant_exchange(
        &self,
        provider: &str,
        model: &str,
        input: i64,
        output: i64,
        age_seconds: i64,
    ) {
        let runtime = self.runtime();
        let ledger = EvidenceLedger::open(&runtime).expect("open the ledger");
        let now = now_unix();
        let at = now - age_seconds;
        let row = NewObservation::new(provider, model)
            .with_route(Some("anthropic-messages"))
            .with_harness(Some("claude-code"))
            .with_timing(Some(at), Some(at + 1))
            .with_tokens(Some(input), Some(output), None)
            .with_outcome(Outcome::Succeeded);
        ledger.record(row, at + 1).expect("record the exchange");
    }

    /// A relayed exchange: no token count at all — *unread*, never zero.
    fn plant_relayed(&self, provider: &str, model: &str, age_seconds: i64) {
        let runtime = self.runtime();
        let ledger = EvidenceLedger::open(&runtime).expect("open the ledger");
        let now = now_unix();
        let at = now - age_seconds;
        let row = NewObservation::new(provider, model)
            .with_route(Some("anthropic-messages"))
            .with_harness(Some("claude-code"))
            .with_timing(Some(at), Some(at + 1))
            .with_outcome(Outcome::Succeeded);
        ledger
            .record(row, at + 1)
            .expect("record the relayed exchange");
    }

    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .env(VAR, "sk-planted-budget-spend-test")
            .env(FREE_VAR, "sk-planted-budget-spend-test-free")
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn both_streams(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// One provider, a $10 calendar-month budget, and a matching credential —
/// the shape every test below starts from.
fn provider_with_budget(name: &str, var: &str, amount_micro_usd: u64) -> String {
    format!(
        "[providers.{name}]\ntemplate = \"openrouter\"\ncredential_env = [\"{var}\"]\n\
         metered_models = [\"m\"]\n\n\
         [providers.{name}.quota]\nbudget = {{ amount_micro_usd = {amount_micro_usd}, \
         period = \"calendar-month\" }}\n"
    )
}

fn pricing_toml(
    provider: &str,
    model: &str,
    input_per_million: f64,
    output_per_million: f64,
) -> String {
    format!(
        "[[prices]]\nprovider = \"{provider}\"\nmodel = \"{model}\"\n\
         input_per_million_usd = {input_per_million}\noutput_per_million_usd = {output_per_million}\n"
    )
}

// ---------------------------------------------------------------------------
// (a) Priced rows under the budget: `resources` prints the counted spend and
// remaining, and the score is now bound by the budget pool.
// ---------------------------------------------------------------------------

#[test]
fn priced_rows_under_the_budget_are_counted_and_lower_the_score() {
    // `openrouter` deliberately, not a custom name: `glasshouse resources`
    // scores only the fixed set `provider::registry::registry()` knows, and
    // a `[providers.<custom-name>]` table renders in CONFIGURED QUOTA
    // OVERRIDES but never gets a `band`/`bound by` line of its own.
    let binary = Binary::with_config(&provider_with_budget("openrouter", VAR, 10_000_000))
        .with_pricing(&pricing_toml("openrouter", "m", 2.0, 2.0));
    // 1,000,000 input + 1,000,000 output @ $2/M each = $4 = 4_000_000 micro-USD.
    binary.plant_exchange("openrouter", "m", 1_000_000, 1_000_000, 60);

    let out = binary.glasshouse(&["resources", "--no-harness"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");

    assert!(
        said.contains(
            "4.000000 USD counted spent over 1 priced exchanges (0 unread, 0 unpriced), \
             6.000000 USD remaining"
        ),
        "the counted spend and remaining must both be printed:\n{said}"
    );
    assert!(
        said.contains("bound by user budget"),
        "the budget pool must now be the dimension the score binds on, since nothing else on \
         this provider was measured:\n{said}"
    );
    assert!(
        !said.contains("does not count spend against this"),
        "the old sentence must be gone:\n{said}"
    );
}

// ---------------------------------------------------------------------------
// (c)/(d) Nothing priceable: an unpriced row and a relayed (unread) row both
// leave the remaining half unmeasured and print the breakdown, never a zero.
// ---------------------------------------------------------------------------

#[test]
fn rows_with_no_price_entry_are_uncounted_never_zero() {
    // `openrouter` deliberately — see the comment on the test above; the
    // "not bound by user budget" half needs a registry-known name to mean
    // anything. No pricing.toml at all: every row with tokens is unpriced.
    let binary = Binary::with_config(&provider_with_budget("openrouter", VAR, 10_000_000));
    binary.plant_exchange("openrouter", "m", 1_000_000, 1_000_000, 60);

    let out = binary.glasshouse(&["resources", "--no-harness"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");

    assert!(
        said.contains("spend not counted (1 exchanges: 0 unread, 1 unpriced)"),
        "an unpriced row must be named, never treated as zero spend:\n{said}"
    );
    assert!(
        !said.contains("bound by user budget"),
        "an unmeasured remaining half must not become the binding dimension:\n{said}"
    );
}

#[test]
fn a_relayed_exchange_with_no_token_count_is_uncounted_as_unread() {
    let binary = Binary::with_config(&provider_with_budget("alpha", VAR, 10_000_000))
        .with_pricing(&pricing_toml("alpha", "m", 2.0, 2.0));
    binary.plant_relayed("alpha", "m", 60);

    let out = binary.glasshouse(&["resources", "--no-harness"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");

    assert!(
        said.contains("spend not counted (1 exchanges: 1 unread, 0 unpriced)"),
        "a relayed row must be named as unread, never treated as zero spend:\n{said}"
    );
}

/// A budget nobody could count against — no ledger rows at all — changes
/// nothing: no exclusion, remaining unmeasured, the honest zero-exchange
/// breakdown.
#[test]
fn a_budget_with_no_ledger_rows_leaves_remaining_unmeasured() {
    let binary = Binary::with_config(&provider_with_budget("alpha", VAR, 10_000_000))
        .with_pricing(&pricing_toml("alpha", "m", 2.0, 2.0));

    let out = binary.glasshouse(&["resources", "--no-harness"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");

    assert!(
        said.contains("spend not counted (0 exchanges: 0 unread, 0 unpriced)"),
        "{said}"
    );
}

// ---------------------------------------------------------------------------
// (f) The context-firewall reducer's own chooser — GH-BUDGET-SPEND-REMAINING
// -CALLERS's residue on `main.rs::disposable_reducer`: an exhausted
// provider's reducer candidate is excluded before it is ever dialled, the
// same way `disposable_candidates` already excludes it for extraction and
// classification, and raising the budget lets the same reducer run.
//
// A canned OpenAI chat-completions endpoint on loopback —
// `tests/firewall_reducer.rs`'s own shape, copied rather than shared: every
// integration test file in this crate is its own compilation unit.
// ---------------------------------------------------------------------------

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

fn reducer_provider_with_budget(
    name: &str,
    var: &str,
    base_url: &str,
    amount_micro_usd: u64,
) -> String {
    format!(
        "[providers.{name}]\ntemplate = \"openai-compatible\"\nbase_url = \"{base_url}\"\n\
         credential_env = [\"{var}\"]\nmetered_models = [\"m\"]\n\n\
         [providers.{name}.quota]\nbudget = {{ amount_micro_usd = {amount_micro_usd}, \
         period = \"calendar-month\" }}\n"
    )
}

fn context_firewall_config(reducer: &str, model: &str) -> String {
    format!(
        "[context_firewall]\nmode = \"safe\"\nreducer = \"{reducer}\"\n\
         reducer_model = \"{model}\"\nmin_semantic_tokens = 1\n"
    )
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

/// A needle among thousands of duplicate hits — oversized enough to cross
/// `--min-semantic-tokens` after the deterministic ladder, `firewall_reducer.rs`'s
/// own fixture.
fn needle_text() -> (String, &'static str) {
    let mut text = String::new();
    for _ in 0..2000 {
        text.push_str("distinct unique noise line long enough to add up quickly\n");
    }
    text.push_str("THE-ONE-RELEVANT-NEEDLE-LINE\n");
    (text, "THE-ONE-RELEVANT-NEEDLE-LINE")
}

#[test]
fn a_context_firewall_reducer_on_an_exhausted_provider_falls_open_and_runs_once_the_budget_is_raised()
 {
    let model =
        FakeModel::answering(r#"{"selections":[{"id":0,"relevance":"relevant","reason":"x"}]}"#);
    let base_url = model.base_url();

    let binary = Binary::with_config(&format!(
        "{}\n{}",
        reducer_provider_with_budget("alpha", VAR, &base_url, 10_000_000),
        context_firewall_config("alpha", "m"),
    ))
    .with_pricing(&pricing_toml("alpha", "m", 6.0, 6.0));
    // 1,000,000 input + 1,000,000 output @ $6/M each = $12 >= the $10 budget.
    binary.plant_exchange("alpha", "m", 1_000_000, 1_000_000, 60);

    let (text, needle) = needle_text();
    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-budget-exhausted",
        "tu-1",
    );
    let response = binary.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let forwarded = updated_output(&response).expect("must still reduce and emit");
    assert!(
        forwarded.contains(needle),
        "with no reducer candidate left once its provider's budget is exhausted, the \
         deterministic result stands: {forwarded}"
    );
    assert_eq!(
        model.requests().len(),
        0,
        "an exhausted provider's reducer must never be dialled: {:?}",
        model.requests()
    );

    // Raise the budget well past the counted spend; the same reducer runs.
    binary.rewrite_config(&format!(
        "{}\n{}",
        reducer_provider_with_budget("alpha", VAR, &base_url, 100_000_000),
        context_firewall_config("alpha", "m"),
    ));
    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-budget-raised",
        "tu-2",
    );
    binary.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let requests = model.requests();
    assert_eq!(
        requests.len(),
        1,
        "raising the budget must let the same reducer be dialled: {requests:?}"
    );
    assert!(
        requests[0].body.contains("\"model\":\"m\""),
        "the dialled request must ask the configured reducer model: {}",
        requests[0].body
    );
}
