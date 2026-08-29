//! Capability map line 925 — "record why a decision was superseded so future
//! agents do not resurrect it without context."
//!
//! # Why this enters through the binary
//!
//! The reason's whole point is that a **later reader** finds it. A test that
//! called `MemoryStore::supersede_with_reason` and read the record back would
//! prove the column round-trips and nothing about the capability: the two
//! things that make it a capability are the door an operator types
//! (`glasshouse memory revalidate <id> superseded --by <ID> --reason …`, which
//! **rejected** `--reason` until this batch) and the place a later agent reads
//! it (`glasshouse memory search --history`). Both live in `main.rs`, so both
//! are reached the way `launch_overlay.rs` reaches its own — by running the
//! shipped binary. Practice §35: a caller every test bypasses is not a caller.
//!
//! Seeding the memories goes straight through `memory::ProjectMemory`, the
//! same way `events_api.rs` seeds its event log rather than driving the hook
//! CLI: this file proves the supersession door and the reading door, and
//! `memory extract` is a third thing with its own tests.

use std::path::PathBuf;
use std::process::{Command, Output};

use glasshouse::cli::Cli;
use glasshouse::memory::{MemoryId, MemoryKind, MemoryStatus, NewMemory, ProjectMemory};

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = std::fs::canonicalize(tmp.path()).expect("canonicalize the fixture base");
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        std::fs::create_dir_all(base.join("config")).expect("create config dir");
        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn cli(&self) -> Cli {
        Cli {
            scope: Some(self.root.clone()),
            allow_unsafe_scope: false,
            data_dir: Some(self.base.join("data")),
            config_dir: Some(self.base.join("config")),
            log_level: None,
            log_file: None,
            log_stderr: false,
            command: None,
        }
    }

    /// Record two memories — one to retire, one to retire it with — and hand
    /// back their identifiers.
    fn seed_pair(&self, old_body: &str, new_body: &str) -> (MemoryId, MemoryId) {
        let cli = self.cli();
        let runtime = glasshouse::bootstrap(&cli, &self.root).expect("bootstrap");
        let memory = ProjectMemory::open(&runtime).expect("open the project memory");
        let store = memory.store();
        let old = store
            .record(NewMemory::new(MemoryKind::Decision, old_body))
            .expect("record the memory to be superseded");
        let new = store
            .record(NewMemory::new(MemoryKind::Decision, new_body))
            .expect("record the successor");
        (old.id, new.id)
    }

    /// The memory as it stands now, read through the store's own hydration.
    fn read(&self, id: &MemoryId) -> glasshouse::memory::MemoryRecord {
        let cli = self.cli();
        let runtime = glasshouse::bootstrap(&cli, &self.root).expect("bootstrap");
        let memory = ProjectMemory::open(&runtime).expect("open the project memory");
        let record = memory.store().get(id).expect("read the memory");
        record.expect("the memory exists")
    }

    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
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
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The whole of line 925, through the command a person types and the command a
/// later agent types.
///
/// - `glasshouse memory revalidate <id> superseded --by <ID> --reason "…"`
///   **succeeds** — it refused `--reason` outright before this batch;
/// - the reason is on the row afterwards, beside the successor;
/// - and `memory search --history` **prints it**, which is the half that makes
///   it context a future agent actually receives rather than a column.
#[test]
fn superseding_with_a_reason_records_it_and_a_later_search_shows_it() {
    let fixture = Fixture::new();
    let (old, new) = fixture.seed_pair(
        "the gateway forwards on a background thread per connection",
        "the gateway forwards on a bounded pool",
    );

    // Premise first: nothing is superseded and no reason exists yet.
    let before = fixture.read(&old);
    assert_eq!(before.status, MemoryStatus::Active);
    assert_eq!(before.superseded_reason, None);

    let reason = "measured: one thread per connection cost 40MB at 500 sessions";
    let output = fixture.glasshouse(&[
        "memory",
        "revalidate",
        old.as_str(),
        "superseded",
        "--by",
        new.as_str(),
        "--reason",
        reason,
    ]);
    assert!(
        output.status.success(),
        "`superseded --reason` must be accepted:\nstdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );

    let after = fixture.read(&old);
    assert_eq!(after.status, MemoryStatus::Superseded);
    assert_eq!(after.superseded_by.as_ref(), Some(&new));
    assert_eq!(
        after.superseded_reason.as_deref(),
        Some(reason),
        "the operator's own words must survive verbatim"
    );

    // The reading door. `--history` is what includes a superseded memory at
    // all; without the reason on the rendered record, a reader sees that the
    // decision went and not why, which is the resurrection this line names.
    let search = fixture.glasshouse(&["memory", "search", "--history", "gateway", "forwards"]);
    assert!(
        search.status.success(),
        "search failed: {}",
        stderr(&search)
    );
    let rendered = stdout(&search);
    assert!(
        rendered.contains(reason),
        "a superseded memory must carry the reason it was superseded into what a later agent \
         reads:\n{rendered}"
    );
}

/// Superseding without a reason stays legal, and stores nothing rather than
/// something empty.
///
/// The negative half matters as much as the positive one: a build that made
/// `--reason` mandatory would have broken every existing caller, and one that
/// stored `""` would make "no reason recorded" indistinguishable from "a
/// reason was recorded and it was blank".
#[test]
fn superseding_without_a_reason_stays_legal_and_records_nothing() {
    let fixture = Fixture::new();
    let (old, new) = fixture.seed_pair("retire me quietly", "the quiet successor");

    let output = fixture.glasshouse(&[
        "memory",
        "revalidate",
        old.as_str(),
        "superseded",
        "--by",
        new.as_str(),
    ]);
    assert!(
        output.status.success(),
        "`superseded` with no reason must stay legal:\nstderr: {}",
        stderr(&output)
    );

    let after = fixture.read(&old);
    assert_eq!(after.status, MemoryStatus::Superseded);
    assert_eq!(after.superseded_by.as_ref(), Some(&new));
    assert_eq!(
        after.superseded_reason, None,
        "no reason given must store NULL, never an empty string"
    );

    // And whitespace is not a reason either — the same distinction, from the
    // one direction a user can actually reach it.
    let (blank_old, blank_new) = fixture.seed_pair("retire me blankly", "the blank successor");
    let output = fixture.glasshouse(&[
        "memory",
        "revalidate",
        blank_old.as_str(),
        "superseded",
        "--by",
        blank_new.as_str(),
        "--reason",
        "   ",
    ]);
    assert!(
        output.status.success(),
        "a blank reason must not be an error the user cannot act on:\nstderr: {}",
        stderr(&output)
    );
    assert_eq!(
        fixture.read(&blank_old).superseded_reason,
        None,
        "whitespace is not a reason"
    );
}

/// A memory brought back out of `superseded` loses the explanation of a
/// supersession that no longer holds.
///
/// Migration 13 could not give the column a `CHECK` tying it to `status` —
/// `ALTER TABLE ADD COLUMN` cannot add a table constraint — so
/// `MemoryStore::set_status` clearing it in the same expression it clears
/// `superseded_by` is the only thing keeping the two consistent. That makes it
/// a behaviour rather than an implementation detail, and it gets a test.
#[test]
fn revalidating_a_superseded_memory_as_reaffirmed_clears_the_reason_with_the_successor() {
    let fixture = Fixture::new();
    let (old, new) = fixture.seed_pair("a decision that comes back", "a successor that does not");

    let output = fixture.glasshouse(&[
        "memory",
        "revalidate",
        old.as_str(),
        "superseded",
        "--by",
        new.as_str(),
        "--reason",
        "superseded in haste",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(fixture.read(&old).superseded_reason.is_some());

    let output = fixture.glasshouse(&["memory", "revalidate", old.as_str(), "reaffirmed"]);
    assert!(
        output.status.success(),
        "reaffirming must be accepted: {}",
        stderr(&output)
    );

    let back = fixture.read(&old);
    assert_eq!(back.status, MemoryStatus::Active);
    assert_eq!(
        back.superseded_by, None,
        "a memory that is current again was not replaced by anything"
    );
    assert_eq!(
        back.superseded_reason, None,
        "and it must not keep the explanation of a supersession that no longer holds"
    );
}
