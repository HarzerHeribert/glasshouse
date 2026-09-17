//! **`glasshouse cost`** — the 2026-09-16 ruling's one kept product decision
//! (*Glasshouse never decides which model is used*, but what was spent still
//! needs to be seen): per-purpose request and token consumption over a
//! window, and a session's own cached-input share, apart from every other
//! row this project's evidence ledger holds.
//!
//! Every test here drives the shipped binary, not
//! `EvidenceLedger::consumption_by_purpose` directly — practice §35's *"a
//! caller every test bypasses is not a caller"*: the aggregate existing is
//! not the same fact as the command surface reading it correctly.
//!
//! # The hazard this file exists to pin
//!
//! A relayed exchange whose reply the gateway could not read leaves its
//! token columns `NULL`, and a row nobody counted must never print as `0`.
//! "not counted" and "0" are different facts, and
//! [`section`]/[`value_after`] below assert the *exact* rendered value for a
//! token field precisely so a future change that coerces an absent count to
//! `0` fails a string-equality assertion rather than a loose `contains`.
//!
//! # `--session`'s one decision (`commands/cost.rs`'s own doc comment)
//!
//! A session filter narrows to exactly that session's own reading, never the
//! whole ledger's. `a_sessions_cost_report_never_leaks_another_sessions_tokens`
//! is the mutation target: dropping `cached_share_for_session`'s own
//! `session_id = ?2` SQL filter must make it fail.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use rusqlite::Connection;

use glasshouse::routing::evidence::{CostConfidence, EvidenceLedger, NewObservation, ObservedCost};
use glasshouse::{Cli, Runtime, bootstrap};

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots — the same shape `tests/memory_project_scope.rs` uses, so that two
/// fixtures over one `base` are two real projects on one machine, each with
/// its own canonicalised root and its own `glasshouse.db`.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

/// What one `glasshouse cost` run printed.
struct Report {
    stdout: String,
    stderr: String,
    status: std::process::ExitStatus,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root: PathBuf = base.join("workspace").join(name);
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
        let runtime = bootstrap(&cli, &root).unwrap();
        Self {
            base: base.to_path_buf(),
            root,
            runtime,
        }
    }

    fn project_id(&self) -> &str {
        self.runtime.project().id().as_str()
    }

    fn ledger(&self) -> EvidenceLedger {
        EvidenceLedger::open(&self.runtime).unwrap()
    }

    fn raw_connection(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }

    /// Record one observation through the real ledger API — every column a
    /// producer might set, so token counts left `None` become `NULL` exactly
    /// as `NewObservation::with_tokens`'s own doc comment requires.
    fn record(
        &self,
        provider: &str,
        model: &str,
        purpose: Option<&str>,
        tokens: Option<(i64, i64, i64)>,
        observed_at_unix: i64,
    ) {
        let mut observation = NewObservation::new(provider, model).with_purpose(purpose);
        if let Some((input, output, cached)) = tokens {
            observation = observation.with_tokens(Some(input), Some(output), Some(cached));
        }
        self.ledger().record(observation, observed_at_unix).unwrap();
    }

    /// Record one observation exactly the shape
    /// `crate::gateway::session::SessionRouting::record` writes for a real
    /// coding-agent exchange whose reply the gateway could not read: no
    /// purpose, a named harness, and no token counts.
    fn record_gateway_exchange(&self, provider: &str, model: &str, harness: &str, at: i64) {
        let observation = NewObservation::new(provider, model).with_harness(Some(harness));
        self.ledger().record(observation, at).unwrap();
    }

    /// Record one observation exactly the shape a real translated exchange
    /// leaves for [`EvidenceLedger::cached_share_for_session`] to read: the
    /// gateway relay's own purpose, a session id, and real token counts.
    fn record_translated_turn(
        &self,
        provider: &str,
        model: &str,
        session_id: &str,
        input_tokens: i64,
        cached_input_tokens: i64,
        at: i64,
    ) {
        let observation = NewObservation::new(provider, model)
            .with_purpose(Some(glasshouse::routing::evidence::HARNESS_TURN_PURPOSE))
            .with_session_id(Some(session_id))
            .with_tokens(Some(input_tokens), Some(0), Some(cached_input_tokens));
        self.ledger().record(observation, at).unwrap();
    }

    /// Run `glasshouse cost`, exactly as a person runs it.
    fn cost(&self, hours: Option<u32>) -> Report {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("cost");
        if let Some(hours) = hours {
            command.arg("--hours").arg(hours.to_string());
        }
        let output = command
            .output()
            .expect("the glasshouse binary must be runnable");
        Report {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        }
    }

    /// Run `glasshouse cost --json`, plus whatever extra flags the caller
    /// passes (`--since`, `--session`) — exactly as a person runs it.
    fn cost_json(&self, extra_args: &[&str]) -> Report {
        self.cost_raw(&[&["--json"], extra_args].concat())
    }

    /// Run `glasshouse cost --session <id>`, the prose path — exactly as a
    /// person runs it, with no `--json`.
    fn cost_session(&self, session_id: &str) -> Report {
        self.cost_raw(&["--session", session_id])
    }

    /// Run `glasshouse cost` with exactly the args given — no implicit
    /// `--json` or `--hours` — for the clap usage-error tests and the
    /// `--session` prose path, where the shape of the command line is the
    /// point.
    fn cost_raw(&self, args: &[&str]) -> Report {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("cost")
            .args(args);
        let output = command
            .output()
            .expect("the glasshouse binary must be runnable");
        Report {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        }
    }
}

/// Now, in the same clock `EvidenceLedger::consumption_by_purpose` reads
/// its window against — every fixture below records observations a few
/// seconds in the past so they land comfortably inside the default 24-hour
/// window without this file needing to know the command's own default.
fn now() -> i64 {
    glasshouse::provider::cache::now_unix_seconds()
}

/// Insert one observation directly, bypassing `EvidenceLedger::record` and
/// the project-id trigger — the only way to plant a row belonging to another
/// project, which is exactly what the trigger exists to prevent. Models a
/// row that reached the file by a route the trigger never saw: a restored
/// backup, a hand-edited file, a build whose schema predates the guard —
/// the same premise `tests/memory_project_scope.rs::plant_foreign_memory`
/// uses for the memory store's own version of this boundary.
fn plant_foreign_observation(conn: &Connection, project_id: &str, purpose: Option<&str>, at: i64) {
    conn.execute_batch("DROP TRIGGER routing_observations_reject_foreign_project_insert;")
        .unwrap();
    conn.execute(
        "INSERT INTO routing_observations
            (project_id, observed_at, provider, model, purpose,
             input_tokens, output_tokens, cached_input_tokens)
         VALUES (?1, ?2, 'foreign-provider', 'foreign-model', ?3, 999, 999, 999)",
        rusqlite::params![project_id, at, purpose],
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER routing_observations_reject_foreign_project_insert
         BEFORE INSERT ON routing_observations
         FOR EACH ROW
         WHEN NEW.project_id IS NOT (
             SELECT value FROM project_metadata WHERE key = 'project_id'
         )
         BEGIN
             SELECT RAISE(ABORT, 'routing observation belongs to a different project');
         END;",
    )
    .unwrap();
}

/// The rendered block for one purpose group's label, exactly as
/// `commands::cost::render_cost_by_purpose` writes it: from the blank line
/// before `  {label}` to the next blank line (or the end of the report).
fn section(report: &str, label: &str) -> String {
    let marker = format!("\n  {label}\n");
    let start = report
        .find(&marker)
        .unwrap_or_else(|| panic!("no section for {label:?} in:\n{report}"));
    let rest = &report[start + 1..];
    let end = rest.find("\n\n").unwrap_or(rest.len());
    rest[..end].to_owned()
}

/// The exact value printed after one field's fixed-width label, up to the
/// end of its line — strict on purpose, so a render that slips a stray digit
/// or a different word into a "not counted" field fails a string comparison
/// rather than surviving a loose `contains`.
fn value_after(text: &str, field_prefix: &str) -> String {
    let start = text
        .find(field_prefix)
        .unwrap_or_else(|| panic!("missing {field_prefix:?} in:\n{text}"))
        + field_prefix.len();
    let rest = &text[start..];
    let end = rest.find('\n').unwrap_or(rest.len());
    rest[..end].to_owned()
}

const REQUESTS: &str = "    requests            : ";
const INPUT_TOKENS: &str = "    input tokens        : ";
const OUTPUT_TOKENS: &str = "    output tokens       : ";
const CACHED_TOKENS: &str = "    cached input tokens : ";

// ---------------------------------------------------------------------------
// 1. Attribution: one purpose's spend, apart from every other row.
// ---------------------------------------------------------------------------

/// **The joined link.** A ledger holding one `classification`-purposed row
/// with real token counts and one row with no purpose and no counts: the
/// report attributes the counted tokens to `classification` and does not
/// smear them onto the other group. Asserts the exact numbers, not just the
/// labels. (`classification` is an arbitrary purpose string here — the
/// classifier itself is gone; this exercises the reader's own grouping.)
#[test]
fn the_classification_group_is_attributed_its_own_tokens_and_no_others() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;

    fixture.record(
        "alpha-runner",
        "alpha-model",
        Some("classification"),
        Some((111, 222, 333)),
        at,
    );
    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at);

    let run = fixture.cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let classification = section(&run.stdout, "classification");
    assert_eq!(value_after(&classification, REQUESTS), "1");
    assert_eq!(value_after(&classification, INPUT_TOKENS), "111");
    assert_eq!(value_after(&classification, OUTPUT_TOKENS), "222");
    assert_eq!(value_after(&classification, CACHED_TOKENS), "333");

    let coding_agent = section(&run.stdout, "coding-agent (gateway relay)");
    assert_eq!(value_after(&coding_agent, REQUESTS), "1");
    assert!(
        !coding_agent.contains("111")
            && !coding_agent.contains("222")
            && !coding_agent.contains("333"),
        "the coding-agent group must never carry the classification group's own numbers:\n{}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------
// 2. The hazard: an uncounted group renders "not counted", never a digit.
// ---------------------------------------------------------------------------

/// **The hazard this package exists to pin.** A group whose every row left
/// its token columns `NULL` — the coding-agent shape, a relayed exchange
/// whose reply the gateway could not read — renders the words *not
/// counted*, and the token fields carry no digit at all, even though its
/// request count is a real, nonzero number.
#[test]
fn a_group_with_no_counted_tokens_never_renders_a_digit_for_them() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;

    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at);
    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at - 1);

    let run = fixture.cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let coding_agent = section(&run.stdout, "coding-agent (gateway relay)");
    assert_eq!(value_after(&coding_agent, REQUESTS), "2");
    for field in [INPUT_TOKENS, OUTPUT_TOKENS, CACHED_TOKENS] {
        let value = value_after(&coding_agent, field);
        assert_eq!(
            value, "not counted",
            "a group with no counted rows must say so, never a number: {field:?} was {value:?}"
        );
        assert!(
            !value.chars().any(|c| c.is_ascii_digit()),
            "\"not counted\" must never carry a stray digit: {field:?} was {value:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. An empty ledger is an honest, zero-exit report — never an error.
// ---------------------------------------------------------------------------

/// A brand-new project with no routing observations at all exits `0` and
/// says so in words, rather than erroring or printing nothing.
#[test]
fn an_empty_ledger_reports_honestly_and_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let run = fixture.cost(None);
    assert!(
        run.status.success(),
        "an empty ledger is not an error: {}",
        run.stderr
    );
    assert!(
        run.stdout
            .contains("no observations recorded in this window"),
        "an empty ledger must say so rather than printing a blank report:\n{}",
        run.stdout
    );
}

/// The same, for a project that has observations under other purposes but
/// none at all under `classification` — the other half of requirement 4.
#[test]
fn a_ledger_with_no_classification_row_still_reports_honestly() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;
    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at);

    let run = fixture.cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(
        !run.stdout.contains("\n  classification\n"),
        "a ledger with no classification row must not fabricate one:\n{}",
        run.stdout
    );
    let coding_agent = section(&run.stdout, "coding-agent (gateway relay)");
    assert_eq!(value_after(&coding_agent, REQUESTS), "1");
}

// ---------------------------------------------------------------------------
// 2b. `purpose` alone cannot tell two `NULL`-purpose producers apart — only
// `harness_recorded` can, and the two must never be merged.
// ---------------------------------------------------------------------------

/// **The orchestrator's own correction to this package.** `routing_observations`
/// has three production writers, and two of them — memory extraction and the
/// gateway relay — both leave `purpose` `NULL`. Extraction's rows carry real
/// token counts; a relayed exchange whose reply the gateway could not read
/// leaves its token columns `NULL` instead. Grouping on `purpose` alone
/// would fold a genuinely counted total into the one group line 1464 asks to
/// be reported as *not counted*.
/// `harness_recorded` — set only by the gateway's own producer — is what
/// keeps them apart.
#[test]
fn coding_agent_rows_and_other_unpurposed_rows_are_never_merged() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;

    // The extraction shape: no purpose, no harness, real tokens.
    fixture.record(
        "omega-runner",
        "extraction-model",
        None,
        Some((40, 41, 42)),
        at,
    );
    // The gateway shape: no purpose, a named harness, no tokens.
    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at - 1);

    let run = fixture.cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let other = section(&run.stdout, "(no purpose or harness recorded)");
    assert_eq!(value_after(&other, REQUESTS), "1");
    assert_eq!(value_after(&other, INPUT_TOKENS), "40");
    assert_eq!(value_after(&other, OUTPUT_TOKENS), "41");
    assert_eq!(value_after(&other, CACHED_TOKENS), "42");

    let coding_agent = section(&run.stdout, "coding-agent (gateway relay)");
    assert_eq!(value_after(&coding_agent, REQUESTS), "1");
    assert_eq!(value_after(&coding_agent, INPUT_TOKENS), "not counted");
    assert_eq!(value_after(&coding_agent, OUTPUT_TOKENS), "not counted");
    assert_eq!(value_after(&coding_agent, CACHED_TOKENS), "not counted");
    assert!(
        !coding_agent.contains("40")
            && !coding_agent.contains("41")
            && !coding_agent.contains("42"),
        "the coding-agent group must never inherit another producer's counted tokens:\n{}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------
// 4. Cross-project isolation.
// ---------------------------------------------------------------------------

/// **Line 1343's "physically project-scoped," proved against the aggregate
/// itself, not just against the file boundary.** Two real projects share one
/// `--data-dir`, so each still gets its own `glasshouse.db`
/// (`Runtime::state_dir` is keyed by project id) — but that alone would let
/// this test pass even if `consumption_by_purpose`'s own `WHERE project_id =
/// ?1` were deleted, because there would be nothing in the same file to leak.
/// So the foreign row is planted **inside beta's own database file**, under
/// the *same* purpose as beta's real row, which is what makes the SQL
/// `WHERE` clause the only thing that can keep the totals apart.
#[test]
fn a_row_planted_under_another_projects_id_never_contributes_to_this_projects_totals() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");
    let at = now() - 60;

    // beta's own, legitimate row — so a totals report of nothing at all
    // could not pass this test by accident.
    beta.record(
        "beta-runner",
        "beta-model",
        Some("classification"),
        Some((5, 6, 7)),
        at,
    );

    let conn = beta.raw_connection();
    plant_foreign_observation(&conn, alpha.project_id(), Some("classification"), at);
    drop(conn);

    let run = beta.cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let classification = section(&run.stdout, "classification");
    assert_eq!(
        value_after(&classification, REQUESTS),
        "1",
        "a foreign-project row must not inflate this project's request count:\n{}",
        run.stdout
    );
    assert_eq!(value_after(&classification, INPUT_TOKENS), "5");
    assert_eq!(value_after(&classification, OUTPUT_TOKENS), "6");
    assert_eq!(value_after(&classification, CACHED_TOKENS), "7");
    // The header names the project id, a hex hash that can itself contain
    // "999" (CI saw `beta-599c7cd8455a0a67b12c08e9999f9925` fail this test
    // with correct totals), so the planted row is looked for in the body
    // only. The header check keeps the strip honest: it must remove the
    // header line and never a line of totals.
    let (header, body) = run
        .stdout
        .split_once('\n')
        .expect("the report must have a header line");
    assert!(
        header.starts_with("Cost for project "),
        "the first line must be the report header:\n{}",
        run.stdout
    );
    assert!(
        !body.contains("999"),
        "a row planted under another project's id must never appear in this project's totals:\n{}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------
// `--json` (capability map line 2430): JSON Lines, one object per
// observation, `null` never `0` for a column nobody counted.
// ---------------------------------------------------------------------------

/// **Requirements 1 and 2, and this package's one mutation.** A
/// gateway-shaped row — no purpose, no tokens, no outcome, exactly the shape
/// a relayed exchange whose reply the gateway could not read always has —
/// prints one JSON line whose token columns are the literal substring
/// `null`, in the exact key order the packet names, never `0`.
///
/// Mutation target: `.unwrap_or(0)` (or the `Serialize` equivalent) on
/// `input_tokens` in `observation_json` must fail this assertion —
/// `"input_tokens":null` becomes `"input_tokens":0`.
#[test]
fn a_row_with_no_tokens_prints_null_never_zero_in_json() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;

    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at);

    let run = fixture.cost_json(&[]);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "one row must print exactly one line:\n{}",
        run.stdout
    );
    let line = lines[0];
    assert!(
        line.contains("\"input_tokens\":null,\"output_tokens\":null,\"cached_input_tokens\":null"),
        "a row with no counted tokens must print null, in this exact order, never 0: {line}"
    );
    assert!(
        !line.contains("\"input_tokens\":0")
            && !line.contains("\"output_tokens\":0")
            && !line.contains("\"cached_input_tokens\":0"),
        "a token column nobody counted must never print as 0: {line}"
    );

    let value: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        value["outcome"],
        serde_json::Value::Null,
        "a row with no recorded outcome must print outcome:null (requirement 5): {line}"
    );
    for key in [
        "failure_class",
        "session_id",
        "route",
        "purpose",
        "quota_context",
        "tool_rounds",
        "retries",
        "repairs",
        "failovers",
        "dispatched_at",
        "completed_at",
        "first_byte_ms",
        "completed_ms",
        "cost_micro_usd",
        "cost_confidence",
    ] {
        assert_eq!(
            value[key],
            serde_json::Value::Null,
            "column {key:?} was never written and must print null: {line}"
        );
    }
    assert_eq!(value["harness"], "claude-code");
    assert_eq!(value["provider"], "gateway-provider");
    assert_eq!(value["model"], "gateway-model");
    assert!(value["seq"].is_i64(), "{line}");
    assert_eq!(value["observed_at"], at);
}

/// GH-GATEWAY-SERVED-BY, requirement 6: a row that carries a cost prints
/// `cost_micro_usd` and `cost_confidence` as its **last two keys**, after
/// `failovers` — the widened key list `ObservationJson`'s own doc comment
/// pins at twenty-four.
#[test]
fn a_planted_cost_prints_as_the_lines_last_two_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;

    fixture
        .ledger()
        .record(
            NewObservation::new("cost-provider", "cost-model").with_cost(Some(ObservedCost {
                micro_usd: 1234,
                confidence: CostConfidence::Estimated,
            })),
            at,
        )
        .unwrap();

    let run = fixture.cost_json(&[]);
    assert!(run.status.success(), "stderr: {}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "one row must print exactly one line:\n{}",
        run.stdout
    );
    let line = lines[0];
    assert!(
        line.ends_with("\"cost_micro_usd\":1234,\"cost_confidence\":\"estimated\"}"),
        "cost_micro_usd and cost_confidence must be the line's last two keys: {line}"
    );
}

/// **Requirement 1's ordering.** Two rows recorded out of insertion order
/// print in `observed_at` ascending order.
#[test]
fn json_lines_are_ordered_by_observed_at_ascending() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let earlier = now() - 120;
    let later = now() - 60;

    fixture.record("later-provider", "later-model", None, None, later);
    fixture.record("earlier-provider", "earlier-model", None, None, earlier);

    let run = fixture.cost_json(&[]);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", run.stdout);
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(first["provider"], "earlier-provider");
    assert_eq!(second["provider"], "later-provider");
}

/// An empty window prints nothing and exits `0` — never a wrapper array,
/// never an error.
#[test]
fn json_empty_window_prints_nothing_and_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let run = fixture.cost_json(&[]);
    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert_eq!(
        run.stdout, "",
        "an empty window must print nothing at all in --json mode: {}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------
// `--session` and `--since` (requirement 3).
// ---------------------------------------------------------------------------

/// `--session <ID>` keeps only that session's rows, filtered after the read.
#[test]
fn json_session_filters_to_that_sessions_rows_only() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;

    let a = NewObservation::new("provider-a", "model-a").with_session_id(Some("session-a"));
    fixture.ledger().record(a, at).unwrap();
    let b = NewObservation::new("provider-b", "model-b").with_session_id(Some("session-b"));
    fixture.ledger().record(b, at - 1).unwrap();

    let run = fixture.cost_json(&["--session", "session-a"]);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "--session must keep only that session's rows:\n{}",
        run.stdout
    );
    let value: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(value["session_id"], "session-a");
    assert_eq!(value["provider"], "provider-a");
}

/// `--since <UNIX>` starts the window at that second and ends now, excluding
/// a row observed before it even though it would fall inside the default
/// `--hours` window.
#[test]
fn json_since_bounds_the_window() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let now_unix = now();
    let old = now_unix - 7_200;
    let recent = now_unix - 60;

    fixture.record("old-provider", "old-model", None, None, old);
    fixture.record("recent-provider", "recent-model", None, None, recent);

    let since = now_unix - 300;
    let run = fixture.cost_json(&["--since", &since.to_string()]);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "--since must exclude a row observed before it:\n{}",
        run.stdout
    );
    let value: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(value["provider"], "recent-provider");
}

// ---------------------------------------------------------------------------
// `--session` without `--json`: the prose path (`commands/cost.rs`'s own
// "the one decision this command makes").
// ---------------------------------------------------------------------------

/// A session with no translated exchange at all is honest about it rather
/// than printing a blank line or falling back to the whole project's cost.
#[test]
fn a_session_with_no_cached_share_says_so_honestly() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let run = fixture.cost_session("session-nothing");
    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(
        run.stdout
            .contains("no translated exchange has reported cached-input tokens"),
        "{}",
        run.stdout
    );
}

/// **The mutation target.** Two sessions in the same project, each with its
/// own real translated turn: `cost --session <A>` reports only `A`'s own
/// input and cached-input tokens, never `B`'s. Dropping
/// `cached_share_for_session`'s own `session_id = ?2` SQL filter — reporting
/// the whole project's cached share instead of one session's — must fail
/// this assertion: session A's report would then also carry session B's 900
/// input tokens.
#[test]
fn a_sessions_cost_report_never_leaks_another_sessions_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;

    fixture.record_translated_turn("provider-a", "model-a", "session-a", 100, 40, at);
    fixture.record_translated_turn("provider-b", "model-b", "session-b", 900, 300, at - 1);

    let run = fixture.cost_session("session-a");
    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(
        run.stdout.starts_with("Session session-a: "),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout
            .contains("1 exchanges, prompt-cache reads 40 of 140 translated input tokens"),
        "session A's own reading: {}",
        run.stdout
    );
    assert!(
        !run.stdout.contains("900") && !run.stdout.contains("300"),
        "session A's report must never carry session B's tokens: {}",
        run.stdout
    );
}

/// `--since` without `--json` is a clap usage error (exit 2), same reason.
#[test]
fn since_flag_without_json_is_a_clap_usage_error() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let run = fixture.cost_raw(&["--since", "0"]);
    assert_eq!(
        run.status.code(),
        Some(2),
        "--since without --json must be a clap usage error: {}",
        run.stderr
    );
}

/// `--since` and `--hours` together is a clap usage error (exit 2) —
/// `--since` replaces `--hours`'s own start of window, not add to it.
#[test]
fn since_and_hours_together_is_a_clap_usage_error() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let run = fixture.cost_raw(&["--json", "--hours", "1", "--since", "0"]);
    assert_eq!(
        run.status.code(),
        Some(2),
        "--since and --hours together must be a clap usage error: {}",
        run.stderr
    );
}

// ---------------------------------------------------------------------------
// Cross-project isolation (SECURITY / ISOLATION INVARIANTS): the same proof
// `a_row_planted_under_another_projects_id_never_contributes_to_this_projects_totals`
// gives the prose path, for `--json`.
// ---------------------------------------------------------------------------

/// A row planted under another project's id, inside this project's own
/// database file, must never appear in this project's `--json` output.
#[test]
fn a_row_planted_under_another_projects_id_never_appears_in_json() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");
    let at = now() - 60;

    beta.record(
        "beta-runner",
        "beta-model",
        Some("classification"),
        Some((5, 6, 7)),
        at,
    );

    let conn = beta.raw_connection();
    plant_foreign_observation(&conn, alpha.project_id(), Some("classification"), at);
    drop(conn);

    let run = beta.cost_json(&[]);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "a foreign-project row must never appear in this project's --json output:\n{}",
        run.stdout
    );
    // Every line is judged by its parsed fields, never by a substring of the
    // raw text: a line also carries `seq` and unix-second timestamps, and a
    // timestamp can spell "999" on its own. The planted row would show up
    // as `foreign-provider`/`foreign-model` with 999 in every token column;
    // the one line here must be beta's own row and nothing of the foreign
    // one.
    for line in &lines {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["provider"], "beta-runner", "line: {line}");
        assert_eq!(value["model"], "beta-model", "line: {line}");
        assert_eq!(value["purpose"], "classification", "line: {line}");
        assert_eq!(value["input_tokens"], 5, "line: {line}");
        assert_eq!(value["output_tokens"], 6, "line: {line}");
        assert_eq!(value["cached_input_tokens"], 7, "line: {line}");
        assert_ne!(value["provider"], "foreign-provider", "line: {line}");
        assert_ne!(value["model"], "foreign-model", "line: {line}");
    }
}
