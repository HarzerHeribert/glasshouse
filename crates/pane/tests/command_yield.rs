//! A `bash` call whose command outlives `[limits] command_yield_s` goes on
//! as a job and the cell moves on; `bg.wait` collects exactly what the call
//! would have returned (`runtime/bindings/jobs.rs`).
//!
//! Every command here is shell builtins only: a confined `bash` cannot exec
//! `/bin/sleep` (`tests/events.rs`' spinner says why), so "runs until told"
//! is a loop on a flag file the test itself creates.
#![cfg(unix)]

use pane::bg;
use pane::config::PaneConfig;
use pane::contract::SessionId;
use pane::glasshouse::Glasshouse;
use pane::runtime::isolate::Runtime;
use pane::runtime::outcome::CellOutcome;
use pane::runtime::preview::Value;
use pane::sandbox::profile::Profile;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    session: SessionId,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let stem = format!("pane-yield-{}-{label}-{n}", std::process::id());
        let root = std::env::temp_dir().join(&stem);
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        Self {
            root,
            session: SessionId::new(format!("{stem}-session")),
        }
    }

    /// A runtime whose `bash` calls hand a command on after one second.
    fn runtime(&self, glasshouse: &Glasshouse) -> Runtime {
        let profile = Profile::compile(
            &self.root,
            Some(r#"{"permissions":{"allow":["Bash(echo*)","Bash(while*)","Bash(do*)"]}}"#),
        );
        let config = PaneConfig::parse_profile("[limits]\ncommand_yield_s = 1\n", None)
            .expect("a one-second hand-off is a valid [limits] table");
        Runtime::new(&profile, glasshouse, &self.session)
            .with_config(config)
            .expect("the configuration binds")
    }

    /// A command that runs until [`Fixture::release`] creates its flag, then
    /// prints `finished`.
    fn held(&self) -> String {
        format!(
            "while [ ! -e '{}' ]; do :; done; echo finished",
            self.root.join("flag").display()
        )
    }

    fn release(&self) {
        std::fs::write(self.root.join("flag"), "").unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        bg::shutdown(&self.session);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn returned_string(outcome: &CellOutcome) -> String {
    match outcome {
        CellOutcome::Returned {
            value: Value::String(text),
            ..
        } => text.head().to_string(),
        other => panic!("expected a string, got {other:?}"),
    }
}

/// Whether any process's command line contains `marker`.
fn running(marker: &str) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-A", "-ww", "-o", "command"])
        .output()
        .expect("ps is on every unix pane builds for");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.contains(marker) && !line.contains("ps -A"))
}

#[test]
fn a_command_still_running_at_the_bound_comes_back_as_a_job_and_bg_wait_collects_it() {
    let fixture = Fixture::new("collect");
    let glasshouse = Glasshouse::None;
    let mut runtime = fixture.runtime(&glasshouse);
    // A backstop, so a runtime that never hands the command on fails this
    // test in bounded time instead of waiting on a flag nobody creates.
    let flag = fixture.root.join("flag");
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(8));
        let _ = std::fs::write(flag, "");
    });

    let started = Instant::now();
    let first = runtime.run_cell(&format!(
        "const r = await bash({{command: {:?}}});\n\
         return JSON.stringify({{running: r.running, id: r.job && r.job.id, exit: r.exit_code, note: r.stderr.includes(\"bg.wait\")}});\n",
        fixture.held()
    ));
    let elapsed = started.elapsed();
    assert_eq!(
        returned_string(&first),
        r#"{"running":true,"id":"job1","exit":null,"note":true}"#
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the cell waited {elapsed:?} for a command that had not finished; it should have moved on at the one-second bound"
    );
    assert_eq!(
        bg::live(&fixture.session),
        1,
        "the command stopped when the call returned"
    );

    fixture.release();
    let second = runtime.run_cell(
        "const r = await bg.wait(\"job1\", {timeout: 20000});\n\
         return JSON.stringify({out: r.stdout.trim(), exit: r.exit_code, running: r.running === true});\n",
    );
    assert_eq!(
        returned_string(&second),
        r#"{"out":"finished","exit":0,"running":false}"#
    );
    // Collected, so not delivered a second time in the next batch.
    assert!(
        bg::drain(&fixture.session)
            .iter()
            .all(|event| event.payload.as_str() != "job1#exit"),
        "a collected exit was left for the batch"
    );
}

#[test]
fn a_command_that_finishes_within_the_bound_returns_as_it_always_did() {
    let fixture = Fixture::new("quick");
    let glasshouse = Glasshouse::None;
    let mut runtime = fixture.runtime(&glasshouse);

    let outcome = runtime.run_cell(
        "const r = await bash({command: \"echo quick\"});\n\
         return JSON.stringify({running: r.running === undefined, job: r.job === undefined, out: r.stdout.trim(), exit: r.exit_code});\n",
    );
    assert_eq!(
        returned_string(&outcome),
        r#"{"running":true,"job":true,"out":"quick","exit":0}"#
    );
    assert_eq!(
        bg::live(&fixture.session),
        0,
        "a finished command left a job"
    );
}

#[test]
fn wait_true_waits_to_the_end() {
    let fixture = Fixture::new("wait-true");
    let glasshouse = Glasshouse::None;
    let mut runtime = fixture.runtime(&glasshouse);
    let flag = fixture.root.join("flag");
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2500));
        std::fs::write(flag, "").unwrap();
    });

    let outcome = runtime.run_cell(&format!(
        "const r = await bash({{command: {:?}, wait: true}});\n\
         return JSON.stringify({{running: r.running === undefined, out: r.stdout.trim(), exit: r.exit_code}});\n",
        fixture.held()
    ));
    releaser.join().unwrap();
    assert_eq!(
        returned_string(&outcome),
        r#"{"running":true,"out":"finished","exit":0}"#
    );
    assert_eq!(bg::live(&fixture.session), 0);
}

#[test]
fn a_handed_on_command_dies_with_the_task() {
    let fixture = Fixture::new("task-end");
    let glasshouse = Glasshouse::None;
    let mut runtime = fixture.runtime(&glasshouse);
    let marker = format!("pane-yield-marker-{}", std::process::id());

    let outcome = runtime.run_cell(&format!(
        "const r = await bash({{command: \"while :; do : {marker}; done\"}});\n\
         return String(r.running);\n"
    ));
    assert_eq!(returned_string(&outcome), "true");
    assert!(running(&marker), "the handed-on command was not running");

    // What a task's end does: the runtime forgets the task, and the board's
    // jobs are stopped (`session.rs::run_task_inner`).
    runtime.end_task();
    bg::shutdown(&fixture.session);
    let deadline = Instant::now() + Duration::from_secs(5);
    while running(&marker) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !running(&marker),
        "the task ended and its command is still running"
    );
    assert_eq!(bg::live(&fixture.session), 0);
}
