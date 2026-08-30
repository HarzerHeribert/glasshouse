//! **`glasshouse routing-cost`** — capability map line 1464: what
//! Glasshouse's own routing model has consumed, in tokens and requests,
//! apart from every other row this project's evidence ledger holds.
//!
//! Every test here drives the shipped binary, not
//! `EvidenceLedger::consumption_by_purpose` directly — practice §35's *"a
//! caller every test bypasses is not a caller"*: the aggregate existing is
//! not the same fact as the command surface reading it correctly, and the
//! command is what map line 1464 is actually about.
//!
//! # The hazard this file exists to pin
//!
//! A row nobody counted (this build's gateway relay never parses a reply
//! body, so every coding-agent exchange leaves its token columns `NULL`)
//! must never print as `0`. "not counted" and "0" are different facts, and
//! [`section`]/[`value_after`] below assert the *exact* rendered value for a
//! token field precisely so a future change that coerces an absent count to
//! `0` fails a string-equality assertion rather than a loose `contains`.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use rusqlite::Connection;

use glasshouse::routing::evidence::{EvidenceLedger, NewObservation};
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

/// What one `glasshouse routing-cost` run printed.
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
    /// coding-agent exchange: no purpose, a named harness, and no token
    /// counts — the relay never parses a reply body to read any.
    fn record_gateway_exchange(&self, provider: &str, model: &str, harness: &str, at: i64) {
        let observation = NewObservation::new(provider, model).with_harness(Some(harness));
        self.ledger().record(observation, at).unwrap();
    }

    /// Run `glasshouse routing-cost`, exactly as a person runs it.
    fn routing_cost(&self, hours: Option<u32>) -> Report {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("routing-cost");
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
/// `main.rs::render_routing_cost` writes it: from the blank line before
/// `  {label}` to the next blank line (or the end of the report).
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
// 1. Attribution: the routing model's own spend, apart from every other row.
// ---------------------------------------------------------------------------

/// **The joined link.** A ledger holding one `classification` row with real
/// token counts and one row with no purpose and no counts: the report
/// attributes the counted tokens to `classification` and does not smear them
/// onto the other group. Asserts the exact numbers, not just the labels.
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

    let run = fixture.routing_cost(None);
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
/// its token columns `NULL` — the coding-agent shape, gateway rows this
/// build never parses — renders the words *not counted*, and the token
/// fields carry no digit at all, even though its request count is a real,
/// nonzero number.
#[test]
fn a_group_with_no_counted_tokens_never_renders_a_digit_for_them() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;

    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at);
    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at - 1);

    let run = fixture.routing_cost(None);
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

    let run = fixture.routing_cost(None);
    assert!(
        run.status.success(),
        "an empty ledger is not an error: {}",
        run.stderr
    );
    assert!(
        run.stdout
            .contains("no routing observations recorded in this window"),
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

    let run = fixture.routing_cost(None);
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
/// token counts; the gateway's never do (`gateway/ingress.rs` never parses a
/// reply body). Grouping on `purpose` alone would fold a genuinely counted
/// total into the one group line 1464 asks to be reported as *not counted*.
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

    let run = fixture.routing_cost(None);
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

    let run = beta.routing_cost(None);
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
    assert!(
        !run.stdout.contains("999"),
        "a row planted under another project's id must never appear in this project's totals:\n{}",
        run.stdout
    );
}
