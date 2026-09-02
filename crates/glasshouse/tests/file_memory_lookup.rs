//! Line 1140 and line 1143: installing `MemoryStore::for_path`
//! (`memory_path_lookup.rs` proves the door itself) at its two named
//! production call sites — `memory::inject::briefing`'s context injection,
//! and the machine door's `query_memory`.
//!
//! Two fixtures, matched to each caller's own proof discipline:
//!
//! - `briefing` is a pure function over a [`MemoryStore`], so its tests here
//!   are in-process, the same shape `memory_path_lookup.rs` already uses for
//!   `for_path` itself.
//! - `query_memory` is `crate::api`'s own external door, whose module doc
//!   comment states it "is proven only by running the shipped binary ... never
//!   by an in-process unit test" — so its tests spawn the real
//!   `glasshouse api serve` and drive its real socket, the shape
//!   `memory_query_api.rs` already uses for the rest of that door.
//!
//! Every row this build's only writer (`MemoryStore::record_observed_files`)
//! can produce carries `FileAssociation::Observed` — never `Referenced`,
//! which is deliberately unreachable (`docs/product/design-decisions.md`,
//! *"A file association is observed, never inferred"*: the qualifier *"the
//! memory refers to this file"* is a claim about a memory's meaning, and no
//! production extraction path reads one). So every assertion below
//! that a row says `observed` is also, structurally, an assertion that it
//! does not say `referenced`.

use std::collections::HashSet;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::{Child, Command, Stdio};
#[cfg(unix)]
use std::time::{Duration, Instant};

use clap::Parser;

use glasshouse::memory::inject::{MAX_INJECTED_BYTES, MEMORY_MARKER, briefing};
use glasshouse::memory::{MemoryAuthority, MemoryId, MemoryKind, NewMemory, ProjectMemory};
use glasshouse::{Cli, Runtime};

/// Only the control-door half waits on a socket.
#[cfg(unix)]
const TIMEOUT: Duration = Duration::from_secs(15);

// -------------------------------------------------------------------------
// Line 1140 — `memory::inject::briefing`, in-process over a `MemoryStore`.
// -------------------------------------------------------------------------

/// The same bootstrapped-project shape `memory_path_lookup.rs` and
/// `memory_query_api.rs` both use.
struct Fixture {
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &std::path::Path, name: &str) -> Self {
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
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();
        Fixture { runtime }
    }

    fn memory(&self) -> ProjectMemory {
        ProjectMemory::open(&self.runtime).unwrap()
    }
}

/// A task naming a path the store has never observed anything against, and
/// no matching text, injects nothing at all — the same "nothing to say" a
/// search that matches nothing already answers, so line 1140 must not change
/// the launch of a session whose task never names a file this project has
/// learned anything beside.
#[test]
fn a_task_naming_no_observed_file_adds_no_section() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "the loader mmaps the index",
        ))
        .unwrap();

    let result = briefing(&store, "fix the bug", &HashSet::new())
        .unwrap()
        .into_injection();
    assert!(
        result.is_none(),
        "a task naming no path and matching no text must inject nothing: {result:?}"
    );
}

/// The finding this file exists for: a task whose text names a path the
/// store has an `observed` association for gets a section built from
/// `for_path`, distinct from whatever the text search itself matched.
///
/// Also the "drop the `for_path` call" mutation's target: with that call
/// dropped, `file_observed` is always empty and this assertion fails.
#[test]
fn briefing_adds_a_section_for_memories_observed_beside_a_named_file() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    let observed = store
        .record(
            NewMemory::new(
                MemoryKind::Finding,
                "walrus batching happens in threes, never singly",
            )
            .with_subject(Some("walrus batching")),
        )
        .unwrap()
        .id;
    store
        .record_observed_files(
            std::slice::from_ref(&observed),
            &["src/parser.rs".to_owned()],
        )
        .unwrap();

    // A task whose text shares no token with the observed memory above (no
    // "walrus", "batching", "threes" or "singly"), so the section can only be
    // explained by the path, not by a lucky text match.
    let result = briefing(&store, "add a test for src/parser.rs", &HashSet::new())
        .unwrap()
        .into_injection()
        .expect("a task naming an observed path must inject something");

    assert!(
        result.text().starts_with(MEMORY_MARKER),
        "{}",
        result.text()
    );
    assert!(
        result
            .text()
            .contains("observed beside the files you named"),
        "the file-observed section's own heading must appear: {}",
        result.text()
    );
    assert!(
        result.memories().contains(&observed),
        "the observed memory's id must be among what this block carries, so a later \
         `already_injected` set excludes it from a repeat delivery: {}",
        result.text()
    );

    // Line 1140's own wording, and the register's ruling: every row this
    // build can write is `observed`, never `referenced`.
    assert!(
        result.text().contains("assoc=observed"),
        "the row must say observed: {}",
        result.text()
    );
    assert!(
        !result.text().contains("referenced"),
        "no row may claim the stronger, unbuilt association: {}",
        result.text()
    );
}

/// A memory the text search already selected is not repeated in the
/// file-observed section, and a memory already delivered to this session is
/// excluded from it exactly as the search half already excludes it.
#[test]
fn the_file_observed_section_excludes_memories_already_carried() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    let matched = store
        .record(
            NewMemory::new(MemoryKind::Finding, "marmot handling in src/parser.rs")
                .with_subject(Some("marmot parser handling")),
        )
        .unwrap()
        .id;
    let already_sent = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "an older parser finding",
        ))
        .unwrap()
        .id;
    store
        .record_observed_files(
            &[matched.clone(), already_sent.clone()],
            &["src/parser.rs".to_owned()],
        )
        .unwrap();

    let mut already_injected: HashSet<MemoryId> = HashSet::new();
    already_injected.insert(already_sent.clone());

    let result = briefing(
        &store,
        "marmot handling: work on src/parser.rs",
        &already_injected,
    )
    .unwrap()
    .into_injection()
    .expect("something must still be injected");

    assert!(
        !result.memories().contains(&already_sent),
        "a memory already sent to this session must not be repeated: {:?}",
        result.memories()
    );
    // `matched` came back from the text search itself (it matches "marmot"),
    // so it must appear exactly once — not a second time in the file section.
    let occurrences = result
        .memories()
        .iter()
        .filter(|id| **id == matched)
        .count();
    assert_eq!(
        occurrences,
        1,
        "a memory the search already selected must not be duplicated by the file section: {}",
        result.text()
    );
}

/// The "exceed the byte ceiling" mutation's target: a file-observed section
/// that would push the block past `MAX_INJECTED_BYTES` is dropped **whole**,
/// never truncated into a partial, misleading list of what this project
/// observed.
#[test]
fn the_file_observed_section_is_dropped_whole_rather_than_truncated() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    // One small primary match, so the block still has something to say and
    // `render` is exercised past its first "nothing at all" return.
    store
        .record(
            NewMemory::new(MemoryKind::Finding, "the marmot loader retries once")
                .with_subject(Some("marmot loader retry")),
        )
        .unwrap();

    // Three constraints with maximal subject/body/rationale/validity/
    // invalidation text — each near [`MAX_INJECTED_SUBJECT_CHARS`]/
    // [`MAX_INJECTED_BODY_CHARS`]/[`MAX_INJECTED_DETAIL_CHARS`]'s own ceiling
    // — all associated with the named path and none matching the task's own
    // text, so they can only ever reach the block through the file-observed
    // section, and three of them together cannot fit under what remains of
    // the 900-byte ceiling after the primary entry above and this section's
    // own heading.
    let filler = |label: &str| "x".repeat(200) + " " + label;
    let mut heavy: Vec<MemoryId> = Vec::new();
    for index in 0..3 {
        let id = store
            .record(
                NewMemory::new(MemoryKind::Constraint, filler(&format!("body-{index}")))
                    .with_subject(Some(filler(&format!("subject-{index}"))))
                    .with_authority(Some(MemoryAuthority::Constraint))
                    .with_provenance(glasshouse::memory::DecisionProvenance {
                        rationale: Some(filler(&format!("why-{index}"))),
                        ..Default::default()
                    })
                    .with_validity_conditions(Some(filler(&format!("valid-{index}"))))
                    .with_invalidation_conditions(Some(filler(&format!("invalid-{index}")))),
            )
            .unwrap()
            .id;
        heavy.push(id);
    }
    store
        .record_observed_files(&heavy, &["src/loader.rs".to_owned()])
        .unwrap();

    let result = briefing(
        &store,
        "extend the marmot loader at src/loader.rs",
        &HashSet::new(),
    )
    .unwrap()
    .into_injection()
    .expect("the small primary match alone must still inject something");

    assert!(
        result.text().len() <= MAX_INJECTED_BYTES,
        "the block must never exceed the ceiling: {} bytes",
        result.text().len()
    );
    assert!(
        !result
            .text()
            .contains("observed beside the files you named"),
        "a section that cannot fit whole must not appear in part: {}",
        result.text()
    );
    assert!(
        heavy.iter().all(|id| !result.memories().contains(id)),
        "a dropped section's memories must not be counted as delivered either, or a later \
         `already_injected` set would wrongly believe this session already has them: {:?}",
        result.memories()
    );
}

/// A task naming more distinct paths than `inject::MAX_OBSERVED_PATHS` (8,
/// private to that module) must never reach the ones named after the bound: a
/// memory observed only beside a ninth named path must never appear, however
/// current and however unambiguous the association would otherwise be.
///
/// Paths 1-8 have no observed memories at all, so the only way this test can
/// pass with a non-empty result is if the ninth path was looked up — which
/// the bound must prevent. Task text and memory body share no tokens, so
/// nothing here can be explained by the text search instead.
#[test]
fn a_task_naming_more_paths_than_the_bound_never_reaches_the_one_named_last() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    let beyond_bound = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "walrus quokka retry loop past the bound",
        ))
        .unwrap()
        .id;
    store
        .record_observed_files(
            std::slice::from_ref(&beyond_bound),
            &["src/zzzlast.rs".to_owned()],
        )
        .unwrap();

    let mut paths: Vec<String> = (1..=8).map(|n| format!("src/f{n}.rs")).collect();
    paths.push("src/zzzlast.rs".to_owned());
    let task = format!("touch {}", paths.join(" "));

    let result = briefing(&store, &task, &HashSet::new())
        .unwrap()
        .into_injection();
    if let Some(injection) = result {
        assert!(
            !injection.memories().contains(&beyond_bound),
            "the ninth named path must never be looked up: {}",
            injection.text()
        );
    }
}

/// For an in-bounds task (fewer named paths than the bound), the returned
/// file-observed records must be identical — same ids, same order — to what
/// the old `retain` + `truncate(3)` kept: the first three, in first-mention
/// path order, of the ones that pass the currency filter.
#[test]
fn the_file_observed_records_for_an_in_bounds_task_keep_the_old_order_and_count() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    let mut ids: Vec<MemoryId> = Vec::new();
    for label in ["alpha", "bravo", "charlie", "delta"] {
        let id = store
            .record(NewMemory::new(
                MemoryKind::Finding,
                format!("gerbil {label} habit unrelated to the task text"),
            ))
            .unwrap()
            .id;
        store
            .record_observed_files(std::slice::from_ref(&id), &[format!("src/{label}.rs")])
            .unwrap();
        ids.push(id);
    }

    let task = "touch src/alpha.rs src/bravo.rs src/charlie.rs src/delta.rs";
    let injection = briefing(&store, task, &HashSet::new())
        .unwrap()
        .into_injection()
        .expect("four observed paths must inject something");

    assert_eq!(
        injection.memories(),
        &ids[0..3],
        "the first three named paths' memories, in that order, must be exactly what the \
         section carries: {}",
        injection.text()
    );
    assert!(
        !injection.memories().contains(&ids[3]),
        "the fourth path's memory must be truncated away exactly as `truncate(3)` did: {}",
        injection.text()
    );
}

// -------------------------------------------------------------------------
// Line 1143 — `query_memory`'s `path` mode, against the shipped binary.
// -------------------------------------------------------------------------

// The control-door half of this file speaks over a Unix domain socket
// (`api::unix`), which Windows has no drop-in equivalent for — see
// `api::no_unix_socket`. Everything above is platform-neutral briefing
// assembly and stays compiled everywhere.
#[cfg(unix)]
struct DoorFixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

#[cfg(unix)]
impl DoorFixture {
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
}

#[cfg(unix)]
struct DoorServer {
    child: Child,
    socket: PathBuf,
}

#[cfg(unix)]
impl DoorServer {
    fn start(fixture: &DoorFixture) -> Self {
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

#[cfg(unix)]
impl Drop for DoorServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The "ignore `path` on the door" mutation's target, and line 1143's own
/// acceptance case: a query whose text matches nothing still returns the
/// memory associated with `path`, and every returned row carries
/// `association: "observed"` alongside the rationale line 1143 asks for.
#[cfg(unix)]
#[test]
fn path_present_answers_from_for_path_with_the_association_kind_on_each_row() {
    let fixture = DoorFixture::new();
    let runtime = fixture.runtime();
    let project = ProjectMemory::open(&runtime).unwrap();
    let store = project.store();
    let id = store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "the heron export must never write partial files",
            )
            .with_authority(Some(MemoryAuthority::Constraint))
            .with_provenance(glasshouse::memory::DecisionProvenance {
                rationale: Some("a partial file was read by a downstream job once".to_owned()),
                ..Default::default()
            }),
        )
        .unwrap()
        .id;
    store
        .record_observed_files(
            std::slice::from_ref(&id),
            &["src/export/heron.rs".to_owned()],
        )
        .unwrap();

    let server = DoorServer::start(&fixture);
    // `query` deliberately matches nothing in the corpus: if the door ignored
    // `path`, this would fall through to the text search and return nothing.
    let response = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "zzz-no-such-token-zzz",
        "history": false,
        "limit": 10,
        "path": "src/export/heron.rs",
    }));
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(
        response["result"]["path"], "src/export/heron.rs",
        "{response}"
    );

    let rules = response["result"]["invariants_and_constraints"]
        .as_array()
        .expect("an invariants_and_constraints array");
    assert_eq!(rules.len(), 1, "{response}");
    let entry = &rules[0];
    assert_eq!(entry["id"], id.as_str(), "{entry}");
    assert_eq!(entry["association"], "observed", "{entry}");
    assert_eq!(
        entry["rationale"], "a partial file was read by a downstream job once",
        "{entry}"
    );
    assert_eq!(
        entry["body"], "the heron export must never write partial files",
        "{entry}"
    );
}

/// `path` absent leaves the verb exactly what it already was — no
/// `association` field, and the text-search `report` still present.
#[cfg(unix)]
#[test]
fn path_absent_leaves_the_verb_unchanged() {
    let fixture = DoorFixture::new();
    let runtime = fixture.runtime();
    let project = ProjectMemory::open(&runtime).unwrap();
    let store = project.store();
    store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "the ibis job could maybe run hourly",
        ))
        .unwrap();

    let server = DoorServer::start(&fixture);
    let response = server.call(serde_json::json!({
        "op": "query_memory",
        "query": "ibis",
        "history": false,
        "limit": 10,
    }));
    assert_eq!(response["status"], "ok", "{response}");
    assert!(
        response["result"]["report"].is_string(),
        "the text-search report must still be present when `path` is absent: {response}"
    );
    let other = response["result"]["other"]
        .as_array()
        .expect("an other array");
    assert_eq!(other.len(), 1, "{response}");
    assert!(
        other[0].get("association").is_none(),
        "a text-search row must not carry the path-mode's `association` field: {other:?}"
    );
}
