//! The machine door onto this project's memory, against the shipped binary
//! and its real socket — Phase 26 lines 1111-1116, and Phase 21F lines
//! 935/936 before them.
//!
//! `crate::api`'s own module doc comment (`src/api/mod.rs`) states this
//! door's proof requirement directly: "this module is proven only by running
//! the shipped binary... never by an in-process unit test, which is the
//! right proof for an external door anyway." `capacity_api.rs` already
//! follows that for `resource_capacity`; this file is the same shape for
//! `query_memory`.
//!
//! Not named in the packet's `EXPECTED FILES` — reported in `packet_errors`.
//! An in-process unit test on `api::unix::query_memory` would have
//! contradicted the architecture note above, so this file exists instead of
//! one.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use clap::Parser;

use rusqlite::Connection;

use glasshouse::memory::{
    DecisionProvenance, MemoryAuthority, MemoryKind, NewMemory, ProjectMemory, ProjectPhase,
    ReviewReason, SourceEvents,
};
use glasshouse::{Cli, Runtime};

const TIMEOUT: Duration = Duration::from_secs(15);

/// A project with its own data and config roots — seeded in-process (see
/// `runtime`) before the real binary is spawned against the same files, the
/// combined pattern `tests/session_model.rs` already uses.
struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");
        std::fs::create_dir_all(base.join("config")).expect("create config dir");
        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn runtime(&self) -> Runtime {
        self.runtime_at(&self.root)
    }

    fn runtime_at(&self, root: &std::path::Path) -> Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        glasshouse::bootstrap(&cli, root).unwrap()
    }

    /// A second real, canonicalised project root under *this* fixture's own
    /// `--data-dir` and `--config-dir`.
    ///
    /// Two projects on one machine — the shape `tests/project_isolation.rs`
    /// uses, and the only shape in which line 1114's question is askable at
    /// all: "another project's memory store" is not a thing that exists
    /// until a second real project does. Placed beside `workspace` rather
    /// than beneath it, so `Project::discover` finds this root and not the
    /// served one.
    fn sibling(&self, name: &str) -> Runtime {
        let root = self.base.join("siblings").join(name);
        std::fs::create_dir_all(root.join(".git")).expect("create sibling project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize sibling project root");
        self.runtime_at(&root)
    }

    /// A second, independent connection to the served project's own database
    /// file — reached through the path `Runtime` already makes public, the
    /// only way an external test can, exactly as `project_isolation.rs` does.
    fn raw_connection(&self) -> Connection {
        Connection::open(self.runtime().database_path()).expect("open the project database")
    }
}

/// Insert a memory row directly, bypassing `MemoryStore` and the project-id
/// trigger entirely — the only way to plant a row belonging to another
/// project, which is exactly what the trigger exists to prevent. Models a row
/// that reached the file by some route the trigger never saw: a restored
/// backup, a hand-edited file, a build whose schema predates the guard.
///
/// Copied from `tests/project_isolation.rs` rather than reinvented, the way
/// that file copied its own fixture, so the two prove the same boundary from
/// the same starting state — one below the door, one through it.
///
/// Only `memories_reject_foreign_project_insert` is dropped. The FTS5 sync
/// trigger is left in place, so the planted row **is** indexed and **is**
/// matchable — see `the_planted_memory_is_really_in_this_files_index`, the
/// control without which "the door returned nothing" would prove nothing.
fn plant_foreign_memory(conn: &Connection, id: &str, project_id: &str, subject: &str, body: &str) {
    conn.execute_batch("DROP TRIGGER memories_reject_foreign_project_insert;")
        .unwrap();
    conn.execute(
        "INSERT INTO memories (id, project_id, kind, status, subject, body, created_at, updated_at) \
         VALUES (?1, ?2, 'finding', 'active', ?3, ?4, 0, 0)",
        rusqlite::params![id, project_id, subject, body],
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER memories_reject_foreign_project_insert
         BEFORE INSERT ON memories
         FOR EACH ROW
         WHEN NEW.project_id IS NOT (
             SELECT value FROM project_metadata WHERE key = 'project_id'
         )
         BEGIN
             SELECT RAISE(ABORT, 'memory belongs to a different project');
         END;",
    )
    .unwrap();
}

struct Server {
    child: Child,
    socket: PathBuf,
}

impl Server {
    fn start(fixture: &Fixture) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&fixture.root)
            .arg("--data-dir")
            .arg(fixture.base.join("data"))
            .arg("--config-dir")
            .arg(fixture.base.join("config"))
            .arg("api")
            .arg("serve")
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn `glasshouse api serve`");

        let stderr = child.stderr.take().expect("captured stderr");
        let mut reader = BufReader::new(stderr);
        let deadline = Instant::now() + TIMEOUT;
        let socket = loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line).expect("read server stderr");
            assert!(read > 0, "the server exited before announcing its socket");
            if let Some(path) = line
                .trim_end()
                .strip_prefix("glasshouse: control API listening on ")
            {
                break PathBuf::from(path);
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the server to announce its socket"
            );
        };

        Self { child, socket }
    }

    fn call(&self, request: serde_json::Value) -> serde_json::Value {
        let deadline = Instant::now() + TIMEOUT;
        let mut stream = loop {
            match UnixStream::connect(&self.socket) {
                Ok(stream) => break stream,
                Err(err) => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out connecting to the control socket: {err}"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        let mut payload = serde_json::to_string(&request).expect("encode request");
        payload.push('\n');
        stream.write_all(payload.as_bytes()).expect("write request");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read response");
        serde_json::from_str(line.trim_end()).expect("parse response")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Acceptance test 4: the machine door carries a constraint's authority,
/// validity state, rationale and invalidation conditions as structured
/// fields, not only inside a rendered string.
#[test]
fn query_memory_carries_authority_validity_rationale_and_invalidation_conditions() {
    let fixture = Fixture::new();
    let runtime = fixture.runtime();
    let project = ProjectMemory::open(&runtime).unwrap();
    let store = project.store();
    store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "The heron export must never write partial files.",
            )
            .with_authority(Some(MemoryAuthority::Constraint))
            .with_provenance(DecisionProvenance {
                rationale: Some("a partial file was read by a downstream job once".to_owned()),
                ..DecisionProvenance::default()
            })
            .with_validity_conditions(Some("the export stays single-writer"))
            .with_invalidation_conditions(Some("the export gains concurrent writers")),
        )
        .unwrap();

    let server = Server::start(&fixture);
    let response = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "heron",
        "history": false,
        "limit": 10,
    }));
    assert_eq!(response["status"], "ok", "{response}");

    let rules = response["result"]["invariants_and_constraints"]
        .as_array()
        .expect("an invariants_and_constraints array");
    assert_eq!(rules.len(), 1, "{response}");
    let entry = &rules[0];
    assert_eq!(entry["authority"], "constraint", "{entry}");
    assert_eq!(entry["status"], "active", "{entry}");
    assert_eq!(entry["current"], true, "{entry}");
    assert_eq!(entry["may_constrain_implementation"], true, "{entry}");
    assert_eq!(
        entry["rationale"], "a partial file was read by a downstream job once",
        "{entry}"
    );
    assert_eq!(
        entry["invalidation_conditions"], "the export gains concurrent writers",
        "{entry}"
    );
    assert_eq!(
        entry["validity_conditions"], "the export stays single-writer",
        "{entry}"
    );
}

/// A memory that is not binding does not carry validity/invalidation
/// conditions into the machine door even if the row happens to hold them —
/// 936's condition is explicit on authority, not on field presence.
#[test]
fn query_memory_does_not_carry_conditions_for_a_non_binding_memory() {
    let fixture = Fixture::new();
    let runtime = fixture.runtime();
    let project = ProjectMemory::open(&runtime).unwrap();
    let store = project.store();
    store
        .record(
            NewMemory::new(MemoryKind::Finding, "The ibis job could maybe run hourly.")
                .with_authority(Some(MemoryAuthority::Idea))
                .with_validity_conditions(Some("nobody has decided this yet")),
        )
        .unwrap();

    let server = Server::start(&fixture);
    let response = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "ibis",
        "history": false,
        "limit": 10,
    }));
    assert_eq!(response["status"], "ok", "{response}");

    let other = response["result"]["other"]
        .as_array()
        .expect("an other array");
    assert_eq!(other.len(), 1, "{response}");
    let entry = &other[0];
    assert_eq!(entry["may_constrain_implementation"], false, "{entry}");
    assert!(entry["validity_conditions"].is_null(), "{entry}");
    assert!(entry["invalidation_conditions"].is_null(), "{entry}");
}

/// A memory that has been challenged is not presented as settled: its
/// status and reason reach the machine door, and it never appears under a
/// default (current) search.
#[test]
fn query_memory_reflects_a_challenged_memory_as_not_settled() {
    let fixture = Fixture::new();
    let runtime = fixture.runtime();
    let project = ProjectMemory::open(&runtime).unwrap();
    let store = project.store();
    let recorded = store
        .record(
            NewMemory::new(MemoryKind::Decision, "The osprey job runs hourly.")
                .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();
    store
        .mark_for_review(&recorded.id, ReviewReason::ProductionIncident)
        .unwrap();

    let server = Server::start(&fixture);

    let historical = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "osprey",
        "history": true,
        "limit": 10,
    }));
    assert_eq!(historical["status"], "ok", "{historical}");
    let other = historical["result"]["other"]
        .as_array()
        .expect("an other array");
    let entry = other
        .iter()
        .find(|entry| entry["id"].as_str() == Some(recorded.id.as_str()))
        .expect("the challenged memory must still be findable as history");
    assert_eq!(entry["status"], "needs_review", "{entry}");
    assert_eq!(entry["current"], false, "{entry}");
    assert_eq!(entry["review"]["reason"], "production_incident", "{entry}");

    let current_only = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "osprey",
        "history": false,
        "limit": 10,
    }));
    assert_eq!(current_only["status"], "ok", "{current_only}");
    let rules = current_only["result"]["invariants_and_constraints"]
        .as_array()
        .unwrap();
    let other2 = current_only["result"]["other"].as_array().unwrap();
    assert!(
        rules
            .iter()
            .chain(other2.iter())
            .all(|entry| entry["id"].as_str() != Some(recorded.id.as_str())),
        "a challenged memory must not be returned as current: {current_only}"
    );
}

// -------------------------------------------------------------------------
// Phase 26 — the memory query door for agents.
// -------------------------------------------------------------------------

/// A memory carrying every locating field an agent needs, recorded through
/// the real store.
fn seed_traceable_memory(runtime: &Runtime) -> glasshouse::memory::MemoryRecord {
    let project = ProjectMemory::open(runtime).unwrap();
    let store = project.store();
    store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "The pelican importer retries three times and then parks the batch.",
            )
            .with_subject(Some("pelican importer retries"))
            .with_authority(Some(MemoryAuthority::Decision))
            .with_source_session(Some("sess-pelican-1"))
            .with_source_commit(Some("0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c"))
            .with_source_events(SourceEvents::new(41, 57))
            .with_provenance(DecisionProvenance {
                rationale: Some("the upstream feed rate-limits bursts".to_owned()),
                project_phase: Some(ProjectPhase::Production),
                problem: Some("batches were being dropped on transient 429s".to_owned()),
                assumptions: Some("the feed's limiter resets within a minute".to_owned()),
                scale_assumptions: Some("under ten thousand rows a batch".to_owned()),
                security_assumptions: Some("the feed is authenticated per batch".to_owned()),
                compatibility_assumptions: Some("the v2 feed keeps 429 semantics".to_owned()),
                operational_assumptions: Some("one importer instance, not a pool".to_owned()),
                evidence: Some("incident 2026-03-11, and importer_retry_bench".to_owned()),
                source_excerpt: Some("\"just park it after the third 429\"".to_owned()),
            }),
        )
        .unwrap()
}

/// Line 1112, acceptance test 1: `memory.get` answers over the real socket
/// and returns the selected memory whole — by full identifier and by the
/// prefix a listing would have shown.
#[test]
fn get_memory_returns_one_selected_memory_in_full_over_the_socket() {
    let fixture = Fixture::new();
    let recorded = seed_traceable_memory(&fixture.runtime());

    let server = Server::start(&fixture);
    let response = server.call(serde_json::json!({
        "op": "get_memory",
        "memory": recorded.id.as_str(),
    }));
    assert_eq!(response["status"], "ok", "{response}");
    let entry = &response["result"];

    assert_eq!(entry["id"], recorded.id.as_str(), "{entry}");
    assert_eq!(entry["kind"], "decision", "{entry}");
    assert_eq!(entry["authority"], "decision", "{entry}");
    assert_eq!(entry["status"], "active", "{entry}");
    assert_eq!(entry["current"], true, "{entry}");
    assert_eq!(entry["subject"], "pelican importer retries", "{entry}");
    assert_eq!(
        entry["body"], "The pelican importer retries three times and then parks the batch.",
        "in full means the whole body, uncut: {entry}"
    );
    // The field that distinguishes this verb's answer from the same memory
    // seen through `memory.current`, where a body may have been cut.
    assert_eq!(entry["body_truncated"], false, "{entry}");
    // Only a single-memory lookup has room for these.
    assert!(entry["superseded_by"].is_null(), "{entry}");
    assert!(entry["superseded_reason"].is_null(), "{entry}");
    assert_eq!(entry["open_todo"], false, "{entry}");
    assert!(entry["updated_at"].is_i64(), "{entry}");

    // A prefix resolves to the same memory: a listing prints the short form,
    // so the short form has to work.
    let by_prefix = server.call(serde_json::json!({
        "op": "get_memory",
        "memory": &recorded.id.as_str()[..8],
    }));
    assert_eq!(by_prefix["status"], "ok", "{by_prefix}");
    assert_eq!(
        by_prefix["result"]["id"],
        recorded.id.as_str(),
        "{by_prefix}"
    );

    // An identifier that names nothing is an error, not an empty success.
    let missing = server.call(serde_json::json!({
        "op": "get_memory",
        "memory": "ffffffffffffffff",
    }));
    assert_eq!(missing["status"], "error", "{missing}");
}

/// Line 1116, acceptance test 4: a retrieved memory carries provenance
/// sufficient to locate its source — through both retrieval verbs, in one
/// vocabulary, and `tests/memory_provenance.rs`'s vocabulary rather than a
/// second one invented for this door.
#[test]
fn every_retrieval_verb_carries_provenance_sufficient_to_locate_the_source() {
    let fixture = Fixture::new();
    let recorded = seed_traceable_memory(&fixture.runtime());

    let server = Server::start(&fixture);

    let searched = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "pelican",
        "history": false,
        "limit": 10,
    }));
    assert_eq!(searched["status"], "ok", "{searched}");
    let hits = searched["result"]["other"].as_array().expect("other");
    let hit = hits
        .iter()
        .find(|entry| entry["id"].as_str() == Some(recorded.id.as_str()))
        .unwrap_or_else(|| panic!("the recorded memory must be findable: {searched}"));

    let got = server.call(serde_json::json!({
        "op": "get_memory",
        "memory": recorded.id.as_str(),
    }));
    assert_eq!(got["status"], "ok", "{got}");

    for (verb, entry) in [("query_memory", hit), ("get_memory", &got["result"])] {
        let provenance = &entry["provenance"];
        // Where to look: the commit for source, the session and event slice
        // for the conversation that produced it.
        assert_eq!(
            provenance["source_commit"], "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c",
            "{verb}: {entry}"
        );
        assert_eq!(
            provenance["source_session_id"], "sess-pelican-1",
            "{verb}: {entry}"
        );
        assert_eq!(provenance["source_events"]["first"], 41, "{verb}: {entry}");
        assert_eq!(provenance["source_events"]["last"], 57, "{verb}: {entry}");
        // Why to believe it: all ten of Phase 21B's fields, by their own
        // names.
        assert_eq!(
            provenance["rationale"], "the upstream feed rate-limits bursts",
            "{verb}: {entry}"
        );
        assert_eq!(provenance["project_phase"], "production", "{verb}: {entry}");
        assert_eq!(
            provenance["problem"], "batches were being dropped on transient 429s",
            "{verb}: {entry}"
        );
        assert_eq!(
            provenance["assumptions"], "the feed's limiter resets within a minute",
            "{verb}: {entry}"
        );
        assert_eq!(
            provenance["scale_assumptions"], "under ten thousand rows a batch",
            "{verb}: {entry}"
        );
        assert_eq!(
            provenance["security_assumptions"], "the feed is authenticated per batch",
            "{verb}: {entry}"
        );
        assert_eq!(
            provenance["compatibility_assumptions"], "the v2 feed keeps 429 semantics",
            "{verb}: {entry}"
        );
        assert_eq!(
            provenance["operational_assumptions"], "one importer instance, not a pool",
            "{verb}: {entry}"
        );
        assert_eq!(
            provenance["evidence"], "incident 2026-03-11, and importer_retry_bench",
            "{verb}: {entry}"
        );
        assert_eq!(
            provenance["source_excerpt"], "\"just park it after the third 429\"",
            "{verb}: {entry}"
        );
    }

    // Absence is `null`, never `""` or `0`: a memory nobody recorded a
    // security assumption for is a different fact from one that recorded
    // there was none.
    let bare = ProjectMemory::open(&fixture.runtime())
        .unwrap()
        .store()
        .record(NewMemory::new(
            MemoryKind::Finding,
            "The pelican dashboard reads from the replica.",
        ))
        .unwrap();
    let bare = server.call(serde_json::json!({
        "op": "get_memory",
        "memory": bare.id.as_str(),
    }));
    assert_eq!(bare["status"], "ok", "{bare}");
    let provenance = &bare["result"]["provenance"];
    for field in [
        "source_session_id",
        "source_commit",
        "source_events",
        "project_phase",
        "problem",
        "rationale",
        "assumptions",
        "scale_assumptions",
        "security_assumptions",
        "compatibility_assumptions",
        "operational_assumptions",
        "evidence",
        "source_excerpt",
    ] {
        assert!(
            provenance[field].is_null(),
            "{field} must be null when nothing was recorded, not empty: {provenance}"
        );
    }
}

/// Line 1113, acceptance test 5: `memory.current` returns the snapshot's
/// sections, not a flattened dump — every kind present even when empty, and
/// each section reporting what it left out.
#[test]
fn current_memory_returns_the_snapshots_sections_not_a_flattened_dump() {
    let fixture = Fixture::new();
    let runtime = fixture.runtime();
    let project = ProjectMemory::open(&runtime).unwrap();
    let store = project.store();
    store
        .record(
            NewMemory::new(MemoryKind::Constraint, "The grebe export is single-writer.")
                .with_authority(Some(MemoryAuthority::Constraint))
                .with_source_commit(Some("abc123")),
        )
        .unwrap();
    let resolved = store
        .record(NewMemory::new(
            MemoryKind::Todo,
            "Retire the grebe v1 endpoint.",
        ))
        .unwrap();
    for index in 0..4 {
        store
            .record(NewMemory::new(
                MemoryKind::Todo,
                format!("Grebe follow-up number {index} is still open."),
            ))
            .unwrap();
    }
    store
        .set_status(&resolved.id, glasshouse::memory::MemoryStatus::Resolved)
        .unwrap();

    let server = Server::start(&fixture);
    let response = server.call(serde_json::json!({
        "op": "current_memory",
        "limit": 2,
        "body_chars": 12,
    }));
    assert_eq!(response["status"], "ok", "{response}");

    let sections = response["result"]["sections"]
        .as_array()
        .expect("a sections array");
    let kinds: Vec<&str> = sections
        .iter()
        .map(|section| section["kind"].as_str().expect("a kind"))
        .collect();
    assert_eq!(
        kinds,
        vec![
            "decision",
            "constraint",
            "feature",
            "finding",
            "failed_attempt",
            "todo"
        ],
        "every kind is present in schema order, so a caller never has to \
         guess whether a missing section means empty or not queried: {response}"
    );

    let section = |kind: &str| {
        sections
            .iter()
            .find(|section| section["kind"] == kind)
            .unwrap_or_else(|| panic!("a {kind} section"))
    };

    let constraints = section("constraint");
    assert_eq!(
        constraints["entries"].as_array().unwrap().len(),
        1,
        "{response}"
    );
    assert_eq!(constraints["omitted"], 0, "{response}");
    assert_eq!(
        constraints["entries"][0]["provenance"]["source_commit"], "abc123",
        "a snapshot entry still says where to look: {response}"
    );

    // The per-section cap holds and what it left out is counted, not dropped
    // silently. Four open todos, two asked for.
    let todos = section("todo");
    assert_eq!(todos["entries"].as_array().unwrap().len(), 2, "{response}");
    assert_eq!(todos["omitted"], 2, "{response}");
    // A resolved todo is not current, so it is in neither number.
    assert!(
        todos["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["id"] != resolved.id.as_str()),
        "{response}"
    );
    assert_eq!(
        todos["entries"][0]["body"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        12,
        "the body cap is applied per entry: {response}"
    );
    assert_eq!(
        todos["entries"][0]["body_truncated"], true,
        "a cut body says it was cut: {response}"
    );

    // An empty section is present and empty, never absent.
    assert_eq!(section("feature")["entries"].as_array().unwrap().len(), 0);
    assert_eq!(section("feature")["omitted"], 0);

    // And the budget that was actually applied comes back, so a caller
    // learns what it got rather than inferring it.
    assert_eq!(
        response["result"]["budget"]["per_section_limit"], 2,
        "{response}"
    );
    assert_eq!(
        response["result"]["budget"]["max_body_chars"], 12,
        "{response}"
    );
}

/// Line 1114, acceptance test 2: no memory verb on this door can be made to
/// read another project's memory, and the refusal is distinguishable from an
/// empty result.
///
/// # The shape of the attack this models
///
/// There is no project field on any request — the door is opened for one
/// resolved `Runtime` and the scope *is* the door (`src/api/mod.rs`). So the
/// only foreign thing a caller can name is an identifier, and the only way a
/// foreign row can be present to be named is the one
/// [`plant_foreign_memory`] models: a restored backup, a hand-edited file, a
/// build whose schema predates the trigger. That is exactly the case the
/// write-side trigger cannot cover and `MemoryStore::get`'s read boundary
/// exists for.
///
/// # Mutation (§16)
///
/// In `memory/search.rs::MemoryStore::search`, delete
/// `AND memories.project_id = ?2` from the SQL and renumber the remaining
/// parameters — `search_returns_nothing_from_the_planted_row` fails. In
/// `memory/store.rs::MemoryStore::get`, change
/// `record.project_id != self.project_id` to `false` —
/// `get_memory_refuses_the_planted_row` fails.
#[test]
fn no_memory_verb_can_be_made_to_read_another_projects_memory() {
    let fixture = Fixture::new();
    let runtime = fixture.runtime();
    let served_id = runtime.project().id().as_str().to_owned();
    let alpha = fixture.sibling("alpha");
    let alpha_id = alpha.project().id().as_str().to_owned();
    assert_ne!(
        alpha_id, served_id,
        "the fixture must use two distinct real projects"
    );

    // A local memory matching the same query word, so that "nothing came
    // back" cannot be confused with "the search does not work".
    let local = ProjectMemory::open(&runtime)
        .unwrap()
        .store()
        .record(NewMemory::new(
            MemoryKind::Finding,
            "The kestrel dashboard in this project is read-only.",
        ))
        .unwrap();

    const PLANTED: &str = "dddddddddddddddddddddddddddddddd";
    plant_foreign_memory(
        &fixture.raw_connection(),
        PLANTED,
        &alpha_id,
        "kestrel export",
        "The alpha kestrel export must never write partial files.",
    );

    // The control, without which every assertion below would pass against a
    // row that was never there: the planted row is in this file, and it is
    // in this file's full-text index under the very word the door is about
    // to be asked for.
    let conn = fixture.raw_connection();
    let indexed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories_fts \
             JOIN memories ON memories.rowid = memories_fts.rowid \
             WHERE memories_fts MATCH 'kestrel' AND memories.id = ?1",
            [PLANTED],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        indexed, 1,
        "the_planted_memory_is_really_in_this_files_index: without this, a \
         door that returned nothing would prove nothing"
    );

    let server = Server::start(&fixture);

    // -- memory.get: refused, and the refusal says which project it belongs
    // to. An error, never a null result an agent would read as "no such
    // memory".
    let refusal = |response: &serde_json::Value, context: &str| {
        assert_eq!(response["status"], "error", "{context}: {response}");
        let message = response["message"].as_str().expect("a message");
        assert!(
            message.contains(&alpha_id) && message.contains(&served_id),
            "{context}: the refusal must name both projects: {message}"
        );
        assert!(
            message.contains("refusing to read another project's memory"),
            "{context}: {message}"
        );
    };

    // get_memory_refuses_the_planted_row
    refusal(
        &server.call(serde_json::json!({"op": "get_memory", "memory": PLANTED})),
        "the full identifier",
    );
    // Differing only by case, and by trailing space: `resolve_id` trims and
    // lowercases, so both reach the same row — and the same refusal, rather
    // than the silent absence a normalization difference could otherwise
    // produce.
    refusal(
        &server.call(serde_json::json!({
            "op": "get_memory",
            "memory": format!("  {}  ", PLANTED.to_uppercase()),
        })),
        "upper case with surrounding space",
    );
    // A prefix of it is the same row and the same refusal.
    refusal(
        &server.call(serde_json::json!({"op": "get_memory", "memory": &PLANTED[..10]})),
        "a prefix",
    );

    // A crafted project id in the request is inert, because no request field
    // names a project: the extra key is ignored and the answer is unchanged.
    for crafted in [alpha_id.as_str(), "", "ALPHA ", "../alpha", "%"] {
        refusal(
            &server.call(serde_json::json!({
                "op": "get_memory",
                "memory": PLANTED,
                "project": crafted,
                "scope": crafted,
            })),
            "a crafted project id",
        );
    }

    // And a refusal is distinguishable from an absence: an identifier that
    // names nothing gets a different message.
    let absent = server.call(serde_json::json!({
        "op": "get_memory",
        "memory": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    }));
    assert_eq!(absent["status"], "error", "{absent}");
    let absent_message = absent["message"].as_str().unwrap();
    assert!(
        !absent_message.contains("another project"),
        "an absent memory must not be reported as a foreign one: {absent_message}"
    );
    assert!(
        !absent_message.contains(&alpha_id),
        "an absent memory must not name a project it never belonged to: {absent_message}"
    );

    // -- memory.search: search_returns_nothing_from_the_planted_row.
    let searched = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "kestrel",
        "history": true,
        "limit": 100,
    }));
    assert_eq!(searched["status"], "ok", "{searched}");
    let returned: Vec<&str> = searched["result"]["invariants_and_constraints"]
        .as_array()
        .unwrap()
        .iter()
        .chain(searched["result"]["other"].as_array().unwrap().iter())
        .map(|entry| entry["id"].as_str().expect("an id"))
        .collect();
    assert!(
        returned.contains(&local.id.as_str()),
        "this project's own matching memory must come back, or the query \
         proves nothing: {searched}"
    );
    assert!(
        !returned.contains(&PLANTED),
        "the planted row matched the query and is in the index, and must \
         still not be returned: {searched}"
    );
    let report = searched["result"]["report"].as_str().expect("a report");
    assert!(
        !report.contains("alpha kestrel export"),
        "the rendered report is the same result and must not leak it either: {report}"
    );

    // -- memory.current: the planted row is active and of a kind that has a
    // section, and must still be in none of them.
    let current = server.call(serde_json::json!({"op": "current_memory"}));
    assert_eq!(current["status"], "ok", "{current}");
    let sections = current["result"]["sections"].as_array().unwrap();
    assert!(
        sections
            .iter()
            .flat_map(|section| section["entries"].as_array().unwrap())
            .all(|entry| entry["id"].as_str() != Some(PLANTED)),
        "{current}"
    );
    assert!(
        sections
            .iter()
            .flat_map(|section| section["entries"].as_array().unwrap())
            .any(|entry| entry["id"].as_str() == Some(local.id.as_str())),
        "this project's own memory must be in the snapshot, or the absence \
         above proves nothing: {current}"
    );
}

/// Line 1115, acceptance test 3: a caller passing an absurd `limit` still
/// gets a bounded response. A caller-supplied bound may only lower the
/// server's ceiling, never raise it.
///
/// # Mutation (§16)
///
/// In `api/unix.rs`, raise `MAX_MEMORY_LIMIT` to `10_000` — the search half
/// fails. Raise `MAX_SNAPSHOT_SECTION_LIMIT` to `10_000`, or
/// `MAX_SNAPSHOT_BODY_CHARS` to `100_000` — the snapshot half fails, on the
/// entry count and on the body length respectively.
#[test]
fn an_absurd_limit_still_gets_a_bounded_response_from_every_memory_verb() {
    // `usize::MAX` on the wire, expressed so it fits a 32-bit `usize` too.
    const ABSURD: u32 = u32::MAX;
    // The ceilings in `api/unix.rs`. Stated here rather than imported
    // because they are private to the binary's own module, which is the
    // point: a caller only ever learns them by being bounded by them.
    const MAX_MEMORY_LIMIT: usize = 100;
    const MAX_SNAPSHOT_SECTION_LIMIT: usize = 50;
    const MAX_SNAPSHOT_BODY_CHARS: usize = 2_000;

    let fixture = Fixture::new();
    let runtime = fixture.runtime();
    let project = ProjectMemory::open(&runtime).unwrap();
    let store = project.store();
    // Comfortably more than either ceiling, all matching one query word, all
    // of one kind so they land in one snapshot section — and **every one of
    // them longer than the body ceiling**, so that the per-entry assertion
    // below cannot pass by happening to draw fifty short ones. A snapshot
    // section is ordered by `updated_at DESC, id ASC`, and these are all
    // written in the same second, so which fifty come back is decided by
    // random identifiers; one long body among many would make that
    // assertion a coin flip.
    for index in 0..(MAX_MEMORY_LIMIT + 25) {
        store
            .record(NewMemory::new(
                MemoryKind::Finding,
                format!(
                    "Avocet finding number {index} concerns retry timing and backoff. {}",
                    "avocet ".repeat(400)
                ),
            ))
            .unwrap();
    }

    let server = Server::start(&fixture);

    let searched = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "avocet",
        "history": true,
        "limit": ABSURD,
    }));
    assert_eq!(searched["status"], "ok", "{searched}");
    let returned = searched["result"]["invariants_and_constraints"]
        .as_array()
        .unwrap()
        .len()
        + searched["result"]["other"].as_array().unwrap().len();
    assert!(
        returned <= MAX_MEMORY_LIMIT,
        "a caller asking for {ABSURD} got {returned} memories; the ceiling \
         is {MAX_MEMORY_LIMIT} and a caller may not raise it"
    );
    assert!(
        returned > 0,
        "the bound must not be reached by returning nothing: {searched}"
    );

    let current = server.call(serde_json::json!({
        "op": "current_memory",
        "limit": ABSURD,
        "body_chars": ABSURD,
    }));
    assert_eq!(current["status"], "ok", "{current}");
    assert_eq!(
        current["result"]["budget"]["per_section_limit"], MAX_SNAPSHOT_SECTION_LIMIT,
        "the applied budget is the ceiling, not what was asked for: {current}"
    );
    let findings = current["result"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["kind"] == "finding")
        .expect("a finding section");
    let entries = findings["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        MAX_SNAPSHOT_SECTION_LIMIT,
        "a caller asking for {ABSURD} entries must get the ceiling: {current}"
    );
    assert!(
        findings["omitted"].as_u64().unwrap() > 0,
        "and be told how many were left out: {current}"
    );
    // Every seeded body is longer than the ceiling, so every returned entry
    // has to be cut to exactly it — and to say that it was.
    for entry in entries {
        assert_eq!(
            entry["body"].as_str().unwrap().chars().count(),
            MAX_SNAPSHOT_BODY_CHARS,
            "every body is capped regardless of the body_chars asked for: {entry}"
        );
        assert_eq!(
            entry["body_truncated"], true,
            "and a cut body says so: {entry}"
        );
    }
    assert_eq!(
        current["result"]["budget"]["max_body_chars"], MAX_SNAPSHOT_BODY_CHARS,
        "{current}"
    );

    // A caller may still lower either ceiling.
    let narrowed = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "avocet",
        "history": true,
        "limit": 3,
    }));
    assert_eq!(narrowed["status"], "ok", "{narrowed}");
    assert_eq!(
        narrowed["result"]["invariants_and_constraints"]
            .as_array()
            .unwrap()
            .len()
            + narrowed["result"]["other"].as_array().unwrap().len(),
        3,
        "{narrowed}"
    );
}

/// The packet's secret/path boundary, executable: a memory verb that cannot
/// open the project's database says so **without naming the file**.
///
/// `database::DatabaseError` names the database's absolute path in every
/// variant. That is right for a person reading their own terminal and wrong
/// for a socket response — the path is outside the project root, and a
/// caller on the far end cannot repair the file anyway. `api::unix`'s
/// `memory_error_message` is what keeps the two apart.
///
/// The failure is induced the only way an external test can induce it
/// deterministically: replace the database file with a directory, which
/// `database::prepare_file` refuses as `NotARegularFile { path, .. }`. The
/// server's already-open connections keep their inode, so this fails exactly
/// the handlers that open the memory store per request — which is all three
/// of them.
///
/// # Mutation (§16)
///
/// In `api/unix.rs::memory_error_message`, replace the `None` arm with
/// `err.to_string()` — every one of the three assertions below fails.
#[test]
fn a_memory_verb_that_cannot_open_the_database_says_so_without_naming_the_file() {
    let fixture = Fixture::new();
    let runtime = fixture.runtime();
    ProjectMemory::open(&runtime).unwrap();
    let database = runtime.database_path();

    let server = Server::start(&fixture);
    assert_eq!(
        server.call(serde_json::json!({"op": "current_memory"}))["status"],
        "ok",
        "the door must work before it is broken, or this test proves nothing"
    );

    std::fs::remove_file(&database).expect("remove the project database");
    std::fs::create_dir(&database).expect("put a directory in its place");

    let database = database.to_str().expect("a utf-8 database path").to_owned();
    let base = fixture.base.to_str().expect("a utf-8 base path").to_owned();
    for request in [
        serde_json::json!({"op": "current_memory"}),
        serde_json::json!({"op": "get_memory", "memory": "abcdef01"}),
        serde_json::json!({"op": "query_memory", "query": "anything", "history": false, "limit": 5}),
    ] {
        let response = server.call(request.clone());
        assert_eq!(response["status"], "error", "{request}: {response}");
        let message = response["message"].as_str().expect("a message");
        assert!(
            !message.contains(&database) && !message.contains(&base),
            "{request}: the database's path must not leave the door: {message}"
        );
        assert!(
            !message.contains('/'),
            "{request}: no filesystem path at all: {message}"
        );
    }
}
