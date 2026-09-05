//! Map line 2439 — launching `pane` from Glasshouse as a real session over a
//! real pseudo-terminal, through the shipped `glasshouse` binary, exactly the
//! way `pty_smoke.rs`'s own
//! `launching_a_harness_records_a_session_that_a_later_command_reads_back`
//! proves it for every harness that came before it.
//!
//! # Why this test builds `pane` itself
//!
//! `cargo test -p glasshouse` never builds `crates/pane` — that is the whole
//! point of Phase 61B's `--exclude pane` (`GH-PANE-KICKOFF`), which keeps
//! `glasshouse` free of any compile-time dependency on it. So a bare `cargo
//! test -p glasshouse --test pane_launch` has to produce the `pane` binary
//! itself, which [`build_pane_binary`] does with a child `cargo build -p
//! pane` process — never a `[dependencies]` or `[dev-dependencies]` edge.
//! `ci-local.sh`'s macOS lane already builds `pane` earlier in the run, but
//! this test does not rely on that ordering: it builds (or reuses an
//! already-fresh build of) the binary itself, so it also passes when run
//! alone.
//!
//! `pane`'s own binary is pointed at directly through `executable = "…"` in
//! a generated `config.toml`, the same mechanism `pty_smoke.rs` already uses
//! for its fake harnesses — never a `PATH` entry, so this never leaks the
//! developer's real `PATH` into the launched harness.
//!
//! Unix-only, matching the existing PTY fixtures this is modelled on
//! (`pty_smoke.rs`'s own `#[cfg(unix)]` split between its marker-harness
//! shell scripts and their `.cmd` Windows equivalents): `pane` is a real
//! compiled binary rather than a shell script, so there is no Windows
//! equivalent to write, and nothing here needs one.

#![cfg(unix)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glasshouse::pty::{PtyOutput, PtyProcess, TerminalCommand};

const TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(25);

/// Build `crates/pane`'s binary as a child `cargo build -p pane` process and
/// return its path, in the same target directory and build profile this
/// test binary itself was built in.
fn build_pane_binary() -> PathBuf {
    let glasshouse_bin = PathBuf::from(env!("CARGO_BIN_EXE_glasshouse"));
    let profile_dir = glasshouse_bin
        .parent()
        .expect("the glasshouse binary path has a parent directory")
        .to_path_buf();

    let mut command = std::process::Command::new(env!("CARGO"));
    command.args(["build", "-p", "pane"]);
    if profile_dir.file_name().and_then(|n| n.to_str()) == Some("release") {
        command.arg("--release");
    }
    let status = command.status().expect("run `cargo build -p pane`");
    assert!(status.success(), "`cargo build -p pane` failed");

    let pane_bin = profile_dir.join("pane");
    assert!(
        pane_bin.is_file(),
        "`cargo build -p pane` did not produce {} -- did pane's binary name or the \
         workspace's target layout change?",
        pane_bin.display()
    );
    pane_bin
}

/// Drains a [`PtyOutput`] on a background thread into a growable buffer, so
/// the test thread can poll accumulated text without blocking on a read that
/// has nothing yet to deliver.
struct Output {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl Output {
    fn start(mut output: PtyOutput) -> Self {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let reader_buf = Arc::clone(&buf);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match output.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => reader_buf
                        .lock()
                        .expect("output buffer poisoned")
                        .extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
            }
        });
        Self { buf }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.buf.lock().expect("output buffer poisoned")).into_owned()
    }
}

/// TOML needs its backslashes escaped, which matters only on Windows but is
/// harmless everywhere -- same helper `pty_smoke.rs` writes inline at each
/// of its own call sites.
fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "\\\\")
}

/// The whole production consumer, end to end: the real `glasshouse` binary,
/// running `glasshouse launch pane`, inside a real pseudo-terminal, typing a
/// line into the session and reading pane's own echo back through it -- and
/// then a second, separate `glasshouse sessions` invocation reading back the
/// record the first process wrote, proving the session is visible in the
/// session list with `pane` as its harness. Closes map line 2439.
#[test]
fn pane_launches_over_a_pty_and_is_visible_in_the_session_list() {
    let pane_bin = build_pane_binary();

    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
    let state_dir = tmp.path().join("state");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&state_dir).expect("create state dir");
    std::fs::create_dir_all(&config_dir).expect("create config dir");

    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[integrations.pane]\nenabled = true\nexecutable = \"{}\"\n",
            toml_path(&pane_bin)
        ),
    )
    .expect("write config");

    let command = TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), tmp.path())
        .arg("--scope")
        .arg(&project_dir)
        .arg("--data-dir")
        .arg(&state_dir)
        .arg("--config-dir")
        .arg(&config_dir)
        .arg("launch")
        .arg("pane");

    let (mut process, output) = PtyProcess::spawn(command).expect("spawn glasshouse launch pane");
    let output = Output::start(output);

    // A line typed into the session, terminated the way a real terminal
    // sends Enter -- see `harness::Message::typed`'s own doc for why `\r`
    // and not `\n`.
    const TYPED: &str = "hello, pane";
    process
        .write_input(format!("{TYPED}\r").as_bytes())
        .expect("type a line into the session");

    let deadline = Instant::now() + TIMEOUT;
    let mut seen = String::new();
    while Instant::now() < deadline {
        seen = output.text();
        if seen.contains(TYPED) {
            break;
        }
        std::thread::sleep(POLL);
    }
    assert!(
        seen.contains(TYPED),
        "typed input never came back through the session.\n--- output ---\n{seen}\n--- end ---"
    );

    // `pane::echo_line` returns after one line, so the process exits on its
    // own; `glasshouse launch` must propagate that clean exit.
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = process.try_wait().expect("poll the launch") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "glasshouse launch pane never exited"
        );
        std::thread::sleep(POLL);
    };
    assert!(
        status.success(),
        "glasshouse launch pane exited abnormally: {status}\n--- output ---\n{}\n--- end ---",
        output.text()
    );

    let listed = std::process::Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .args([
            "--scope",
            &project_dir.display().to_string(),
            "--data-dir",
            &state_dir.display().to_string(),
            "--config-dir",
            &config_dir.display().to_string(),
            "sessions",
        ])
        .output()
        .expect("run glasshouse sessions");
    let text = String::from_utf8_lossy(&listed.stdout);

    let row = text
        .lines()
        .find(|line| line.contains("pane"))
        .unwrap_or_else(|| panic!("no session row names `pane` as its harness:\n{text}"));
    // A clean exit with no assigned or discovered native identifier reads as
    // `closed`, not `resumable` -- `Pane::describe`'s `session_ids` is
    // `Unverified`, honestly, because the binary has no such mechanism today.
    assert!(
        row.contains("closed"),
        "a cleanly exited pane session with no native identifier should read as \
         closed:\n{row}"
    );
}
