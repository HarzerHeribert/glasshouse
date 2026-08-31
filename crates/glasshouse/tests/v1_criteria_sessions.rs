//! Phase 55's seven V1-completion criteria (map lines 1917–1922, 1939), proved
//! against the shipped binary rather than against a hand-built fixture. Each
//! test's doc comment names the existing suite whose shape it borrows and the
//! `docs/product/evidence/phase-*.md` entry that already proves the
//! underlying mechanism.
//!
//! macOS/Linux only (`#[cfg(unix)]` at the top): every interactive test here
//! drives a real pseudo-terminal, the same constraint `tests/pty_smoke.rs`
//! and `tests/orchestrator_role.rs` are already under.

#![cfg(unix)]

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser as _;

use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};
use glasshouse::pty::{PtyOutput, PtyProcess, TerminalCommand};
use glasshouse::{Cli, Project, bootstrap};

const TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(25);

// ---------------------------------------------------------------------------
// A minimal outer-terminal harness, trimmed from `tests/pty_smoke.rs`'s own
// `Session`/`Collector`: unix only, so none of that file's Windows ConPTY
// Device-Status-Report handshake applies here.
// ---------------------------------------------------------------------------

struct Collector {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl Collector {
    fn start(mut output: PtyOutput) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let thread_buffer = Arc::clone(&buffer);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match output.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => thread_buffer.lock().unwrap().extend_from_slice(&chunk[..n]),
                }
            }
        });
        Self { buffer }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap()).into_owned()
    }
}

struct Session {
    process: PtyProcess,
    collector: Collector,
}

impl Session {
    fn spawn(command: TerminalCommand) -> Self {
        let (process, output) = PtyProcess::spawn(command).expect("spawn");
        Self {
            process,
            collector: Collector::start(output),
        }
    }

    fn send(&mut self, text: &str) {
        self.process.send_text(text).expect("send_text");
    }

    /// Wait until `needle` appears in the cumulative output, or fail with
    /// what was seen. Cumulative, not a rendered screen: bytes already
    /// written are never lost even after a later redraw, which is what lets
    /// [`v1_1920_switching_between_two_live_sessions_and_back_does_not_respawn_either`]
    /// find a startup marker printed long before the point it checks it.
    fn expect(&mut self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if self.collector.text().contains(needle) {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "timed out waiting for {needle:?} in pty output.\n--- output ---\n{}\n--- end ---",
            self.collector.text()
        );
    }

    fn output(&self) -> String {
        self.collector.text()
    }

    /// Like [`Session::expect`], but only looks at bytes captured *after*
    /// `since` (a length previously returned by [`Session::output`]). A
    /// marker like `"ctrl-]"` that this session's status line prints every
    /// time session mode is entered would otherwise be found instantly on a
    /// second entry — matching leftover text from the *first* one — which is
    /// exactly the shape practice's own §68/§54 family warns about: a stale
    /// match that reads as a fresh one.
    fn expect_since(&mut self, since: usize, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            let text = self.collector.text();
            if text.len() > since && text.get(since..).is_some_and(|s| s.contains(needle)) {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "timed out waiting for {needle:?} in pty output captured after byte {since}.\n--- output ---\n{}\n--- end ---",
            self.collector.text()
        );
    }

    fn wait_for_exit(&mut self) -> glasshouse::pty::ExitStatus {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.process.try_wait().expect("try_wait") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "child process did not exit within {TIMEOUT:?}"
            );
            std::thread::sleep(POLL);
        }
    }
}

// ---------------------------------------------------------------------------
// Shared fixture helpers
// ---------------------------------------------------------------------------

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn git_project(base: &Path, name: &str) -> PathBuf {
    let root = base.join("workspace").join(name);
    std::fs::create_dir_all(root.join(".git")).expect("create project root");
    std::fs::canonicalize(&root).expect("canonicalize project root")
}

fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "\\\\")
}

/// A fake installed harness that exits immediately without reading or
/// writing anything — for the criteria that only care about the session
/// *record* a launch leaves behind, not about interactive behaviour.
fn install_quiet_harness(bin_dir: &Path, name: &str) -> PathBuf {
    let path = bin_dir.join(name);
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write quiet harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// A fake installed harness that reads exactly one line and echoes it back
/// tagged `GOT:<line>` — the same shape `tests/pty_smoke.rs::install_echo_harness`
/// uses, reproduced here because that file's helper is private to it.
fn install_echo_harness(bin_dir: &Path, name: &str) -> PathBuf {
    let path = bin_dir.join(name);
    std::fs::write(&path, "#!/bin/sh\nread line\necho GOT:$line\n").expect("write echo harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// A fake installed harness that writes its own pid to a file named after
/// the `--session-id` it was launched under — a side channel the TUI's own
/// rendering never touches, the same shape
/// `tests/orchestrator_role.rs::install_looping_echo_harness` uses its own
/// `pid`/`received.log` files for — then loops echoing every line it reads,
/// tagged, the way `tests/pty_smoke.rs::install_tagged_echo_harness` does.
/// The pid file is what lets this test tell a respawn from a redraw: the
/// *rendered* screen only ever shows the presently focused session's raw
/// output, so a background session's own startup text is not something a
/// byte-stream capture can rely on seeing.
fn install_looping_tagged_echo_harness(bin_dir: &Path, name: &str) -> PathBuf {
    let path = bin_dir.join(name);
    std::fs::write(
        &path,
        "#!/bin/sh\n\
         id=\"\"\n\
         while [ $# -gt 0 ]; do\n\
         \x20\x20if [ \"$1\" = \"--session-id\" ]; then id=\"$2\"; fi\n\
         \x20\x20shift\n\
         done\n\
         echo $$ > \"$PWD/pid-$id\"\n\
         while IFS= read -r line; do\n\
         echo \"GOT:$id:$line\"\n\
         done\n",
    )
    .expect("write looping tagged echo harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn run(data_dir: &Path, config_dir: &Path, root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("--scope")
        .arg(root)
        .arg("--data-dir")
        .arg(data_dir)
        .arg("--config-dir")
        .arg(config_dir)
        .args(args)
        .output()
        .expect("the glasshouse binary must be runnable")
}

/// Record one memory in `root`'s own store, through the store the way
/// `glasshouse memory` records one — the same shape
/// `tests/mcp_project_scope.rs::Fixture::seed_memory` uses.
fn seed_memory(data_dir: &Path, config_dir: &Path, root: &Path, body: &str) {
    let runtime = bootstrap_at(data_dir, config_dir, root);
    ProjectMemory::open(&runtime)
        .expect("open the memory store")
        .store()
        .record(NewMemory::new(MemoryKind::Finding, body))
        .expect("seed a memory");
}

fn bootstrap_at(data_dir: &Path, config_dir: &Path, root: &Path) -> glasshouse::Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--scope",
        root.to_str().unwrap(),
        "--data-dir",
        data_dir.to_str().unwrap(),
        "--config-dir",
        config_dir.to_str().unwrap(),
    ])
    .expect("parse the fixture command line");
    bootstrap(&cli, root).expect("bootstrap the fixture runtime")
}

fn all_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(all_files_under(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// One field's value from `glasshouse sessions show`'s
/// `{label:<19}{value}` layout (`main.rs::session_report`).
fn field<'a>(detail: &'a str, label: &str) -> &'a str {
    detail
        .lines()
        .find_map(|line| line.strip_prefix(label))
        .map(str::trim)
        .unwrap_or_else(|| panic!("no {label:?} line in:\n{detail}"))
}

// ---------------------------------------------------------------------------
// 1917 — isolate all state to the project Glasshouse started in
// ---------------------------------------------------------------------------

/// Shape: `tests/project_isolation.rs`'s two-real-projects-one-machine
/// pattern. Entry: `docs/product/evidence/phase-1.md` ("keep cross-project
/// memory retrieval disabled by design", the producer of the per-project
/// database directory), `phase-2.md` (session metadata keyed the same way).
///
/// Mutation: `project/mod.rs::Project::discover` canonicalizing the parent
/// of the requested root instead of the root itself — "the project-root
/// resolver returns the parent directory."
#[test]
fn v1_1917_state_is_isolated_to_the_starting_projects_own_runtime_directory() {
    let tmp = tempdir();
    let base = tmp.path();
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    let fake_home = base.join("fake-home");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&fake_home).unwrap();

    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let harness = install_quiet_harness(&bin_dir, "quiet-claude-code");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n",
            toml_path(&harness)
        ),
    )
    .unwrap();

    let touched = git_project(base, "touched");
    let untouched = git_project(base, "untouched");
    let touched_id = Project::discover(&touched, None, false)
        .unwrap()
        .id()
        .as_str()
        .to_owned();
    let untouched_id = Project::discover(&untouched, None, false)
        .unwrap()
        .id()
        .as_str()
        .to_owned();

    // Run only in `touched`. `HOME` is set to a directory no explicit flag
    // ever names, so any code path that fell back to a real per-user default
    // instead of the explicit `--data-dir`/`--config-dir` would leak there.
    let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("--scope")
        .arg(&touched)
        .arg("--data-dir")
        .arg(&data_dir)
        .arg("--config-dir")
        .arg(&config_dir)
        .args(["launch", "claude-code", "--headless"])
        .env("HOME", &fake_home)
        .output()
        .expect("run glasshouse launch");
    assert!(
        output.status.success(),
        "launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let touched_state = data_dir.join("projects").join(&touched_id);
    assert!(
        touched_state.join("glasshouse.db").exists(),
        "the touched project's own database must exist under its own runtime directory: {}",
        touched_state.display()
    );

    let untouched_state = data_dir.join("projects").join(&untouched_id);
    assert!(
        !untouched_state.exists(),
        "a project nothing ever ran in must have no runtime directory at all: {}",
        untouched_state.display()
    );

    let leaked = all_files_under(&fake_home);
    assert!(
        leaked.is_empty(),
        "the fake $HOME, named by no explicit flag, must receive nothing: {leaked:?}"
    );
}

// ---------------------------------------------------------------------------
// 1918 / 1919 — Claude Code and Codex as fully interactive embedded sessions
// ---------------------------------------------------------------------------

/// Shape: `tests/pty_smoke.rs`'s
/// `launching_a_harness_records_a_session_that_a_later_command_reads_back`
/// (session recording through the shipped binary) combined with its
/// `install_echo_harness` (interactive echo, proving a keystroke really
/// reaches the child). Entry: `phase-7.md` ("Add a Claude Code adapter that
/// starts the real claude executable...").
///
/// Mutation: `integrations/mod.rs::IntegrationId::slug`'s `ClaudeCode` arm
/// returning `"codex"` instead of `"claude-code"` — "the adapter's harness
/// id," on the actual production seam: `HarnessAdapter::id` (the trait
/// method on `ClaudeCode`/`Codex` themselves) turned out to have no
/// production caller outside `session::select` — mutating it there
/// SURVIVED, because `HarnessSelection::id` is set independently during
/// `select()` and never calls the adapter's own `id()`. `slug()` is what
/// `main.rs::launch_session` actually records
/// (`NewSession::embedded(selection.id().slug())`), so that is the line
/// this mutation targets instead.
#[test]
fn v1_1918_claude_code_runs_as_a_fully_interactive_embedded_session() {
    session_records_an_interactive_embedded_launch("claude-code");
}

/// The same proof for Codex. Entry: `phase-8.md` ("the Codex adapter's first
/// three lines").
///
/// Mutation: `integrations/mod.rs::IntegrationId::slug`'s `Codex` arm
/// returning `"claude-code"` instead of `"codex"` — see the doc comment on
/// [`v1_1918_claude_code_runs_as_a_fully_interactive_embedded_session`] for
/// why this targets `slug` rather than `Codex::id`.
#[test]
fn v1_1919_codex_runs_as_a_fully_interactive_embedded_session() {
    session_records_an_interactive_embedded_launch("codex");
}

fn session_records_an_interactive_embedded_launch(harness_slug: &str) {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let harness = install_echo_harness(&bin_dir, harness_slug);
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[integrations.{harness_slug}]\nenabled = true\nexecutable = \"{}\"\n",
            toml_path(&harness)
        ),
    )
    .unwrap();

    let mut session = Session::spawn(
        TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), base)
            .arg("--scope")
            .arg(&project)
            .arg("--data-dir")
            .arg(&data_dir)
            .arg("--config-dir")
            .arg(&config_dir)
            .arg("launch")
            .arg(harness_slug),
    );

    // A keystroke sent to Glasshouse's own outer terminal must reach the
    // spawned harness child through the PTY it was embedded in, and the
    // child's reply must come back out the same way.
    session.send("hello-from-the-outer-terminal\n");
    session.expect("GOT:hello-from-the-outer-terminal");

    let status = session.wait_for_exit();
    assert!(
        status.success(),
        "the harness exited cleanly and glasshouse must propagate that: {status}\n--- output ---\n{}\n--- end ---",
        session.output()
    );

    let listed = run(&data_dir, &config_dir, &project, &["sessions"]);
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(
        text.contains(harness_slug),
        "the recorded session must name `{harness_slug}` as its harness:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// 1920 — switching between two live sessions and back respawns neither
// ---------------------------------------------------------------------------

/// Shape: `tests/pty_smoke.rs::enter_from_the_overview_focuses_the_cursors_session_not_the_presented_one`
/// (two live sessions, the overview, `Enter` to focus the cursor's session).
/// Entry: `phase-10.md` (the unified session model), `phase-11.md` line 687
/// ("Allow the user to focus any live embedded session from the overview").
///
/// Mutation: `shell/state.rs::ShellState::focus_overview_target`'s
/// `self.selected = index;` replaced with a no-op (`self.selected =
/// self.selected;`), so the cursor's session is never actually focused —
/// the packet's own suggested mutation ("switching respawns the session")
/// has no one-line production seam to target: `sync_focus`
/// (`shell/mod.rs`) only ever calls `SessionRuntime::focus`, which its own
/// doc comment states "never touches a process," so there is no single
/// literal edit that turns a switch into a respawn without a larger,
/// multi-line change outside this packet's no-production-change contract.
/// This mutation instead kills the test on the switching mechanism this
/// line's contract is actually about (moving focus without touching the
/// process), which the PID-unchanged assertions below prove is what
/// happens on unmutated code.
#[test]
fn v1_1920_switching_between_two_live_sessions_and_back_does_not_respawn_either() {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let harness = install_looping_tagged_echo_harness(&bin_dir, "tagged-loop-harness");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[onboarding]\ncompleted = true\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n",
            toml_path(&harness)
        ),
    )
    .unwrap();

    let mut shell = Session::spawn(
        TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), base)
            .arg("--scope")
            .arg(&project)
            .arg("--data-dir")
            .arg(&data_dir)
            .arg("--config-dir")
            .arg(&config_dir),
    );
    shell.expect("root ");

    let runtime = bootstrap_at(&data_dir, &config_dir, &project);
    let native_ids = |shell: &mut Session, count: usize| -> Vec<String> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let sessions =
                glasshouse::session::ProjectSessions::open(&runtime).expect("open sessions");
            let ids: Vec<String> = sessions
                .store()
                .list()
                .expect("list sessions")
                .into_iter()
                .map(|record| record.native_session_id.expect("claude-code assigns one"))
                .collect();
            if ids.len() >= count {
                return ids;
            }
            assert!(
                Instant::now() < deadline,
                "expected {count} recorded session(s); got {ids:?}"
            );
            std::thread::sleep(POLL);
            let _ = shell;
        }
    };

    shell.send("n");
    shell.expect("claude-code");
    let presented = native_ids(&mut shell, 1)[0].clone();

    shell.send("n");
    shell.expect("2 claude-code");
    let both = native_ids(&mut shell, 2);
    let target = both
        .into_iter()
        .find(|id| id != &presented)
        .expect("the second session must have a different native id");

    let pid_file = |id: &str| project.join(format!("pid-{id}"));
    let read_pid = |id: &str| -> String {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Ok(text) = std::fs::read_to_string(pid_file(id)) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_owned();
                }
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {id}'s pid file"
            );
            std::thread::sleep(POLL);
        }
    };
    let presented_pid_before = read_pid(&presented);
    let target_pid_before = read_pid(&target);

    // Switch to the cursor's session (the one not currently presented).
    let mark = shell.output().len();
    shell.send("o");
    shell.expect_since(mark, "sessions");
    shell.send("\x1b[B");
    shell.send("\r");
    shell.expect_since(mark, "ctrl-]");
    shell.send("still-there\r");
    shell.expect(&format!("GOT:{target}:still-there"));

    // Switch back to the originally presented session.
    let mark = shell.output().len();
    shell.send("\x1d");
    shell.send("o");
    shell.expect_since(mark, "sessions");
    shell.send("\x1b[A");
    shell.send("\r");
    shell.expect_since(mark, "ctrl-]");
    shell.send("back-again\r");
    shell.expect(&format!("GOT:{presented}:back-again"));

    assert_eq!(
        read_pid(&presented),
        presented_pid_before,
        "the presented session's pid changed across the switch — it was respawned"
    );
    assert_eq!(
        read_pid(&target),
        target_pid_before,
        "the target session's pid changed across the switch — it was respawned"
    );

    shell.send("\x1d");
    shell.send("q");
    let status = shell.wait_for_exit();
    assert!(
        status.success(),
        "the shell should exit cleanly on `q`: {status}\n--- output ---\n{}\n--- end ---",
        shell.output()
    );
}

// ---------------------------------------------------------------------------
// 1921 — designate an orchestrator, spawn one worker, see it in the listing
// ---------------------------------------------------------------------------

/// Shape: `tests/orchestrator_role.rs::spawning_tags_a_worker_by_default_and_an_explicit_role_is_honored`.
/// Entry: `phase-14.md` (Phase 14, boxes 1 and 5).
///
/// Mutation: `api/unix.rs::parse_role`'s `None => Ok(SessionRole::Worker)`
/// changed to `Ok(SessionRole::Normal)` — the packet's own suggested
/// mutation ("drop the worker's owning-orchestrator link") has no seam to
/// target, since no such link is ever written; this mutation instead kills
/// the test on the role-tagging behaviour that line 1921 partially rests
/// on and that this test can actually prove.
///
/// **This line is reported `open`, not `closed`.** What is proven below —
/// spawning a session tagged `role: "orchestrator"` through the control
/// socket, then spawning a second session with no stated role, and finding
/// both in `list_sessions` with `role: "orchestrator"` and `role: "worker"`
/// respectively — is real and mutation-proofed. What the box's own words ask
/// for beyond that — "with its **owning** orchestrator" — is not: nothing in
/// `SessionRecord`, `NewSession`, or `api::unix::session_summary`
/// (`crates/glasshouse/src/api/unix.rs:1019`) persists which session's
/// `spawn_session` call produced a given worker. `spawn_session`'s own
/// comment at that file's line 1150 ("the door records who asked rather than
/// who the orchestrator was acting for") is about a guardrail override's
/// origin, not about session-to-session attribution, and no other field
/// carries it. A worker is visible, and its role is visible, but which
/// orchestrator session owns it is not a fact the shipped binary records
/// anywhere this test could read back. See `packet_errors` in the facts
/// block.
#[test]
fn v1_1921_a_designated_orchestrator_spawns_a_worker_visible_in_the_listing_by_role() {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let harness = install_looping_tagged_echo_harness(&bin_dir, "orchestrator-role-harness");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n",
            toml_path(&harness)
        ),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("--scope")
        .arg(&project)
        .arg("--data-dir")
        .arg(&data_dir)
        .arg("--config-dir")
        .arg(&config_dir)
        .arg("api")
        .arg("serve")
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn `glasshouse api serve`");

    let stderr = child.stderr.take().expect("captured stderr");
    let mut reader = std::io::BufReader::new(stderr);
    let deadline = Instant::now() + TIMEOUT;
    let socket = loop {
        let mut line = String::new();
        let read = std::io::BufRead::read_line(&mut reader, &mut line).expect("read stderr");
        assert!(read > 0, "the server exited before announcing its socket");
        if let Some(path) = line
            .trim_end()
            .strip_prefix("glasshouse: control API listening on ")
        {
            break PathBuf::from(path);
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the socket"
        );
    };

    let call = |request: serde_json::Value| -> serde_json::Value {
        use std::io::{BufRead as _, Write as _};
        let deadline = Instant::now() + TIMEOUT;
        let mut stream = loop {
            match std::os::unix::net::UnixStream::connect(&socket) {
                Ok(stream) => break stream,
                Err(err) => {
                    assert!(Instant::now() < deadline, "timed out connecting: {err}");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        let mut payload = serde_json::to_string(&request).expect("encode request");
        payload.push('\n');
        stream.write_all(payload.as_bytes()).expect("write request");
        let mut reader = std::io::BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read response");
        serde_json::from_str(line.trim_end()).expect("parse response")
    };

    let orchestrator = call(serde_json::json!({
        "op": "spawn_session",
        "harness": "claude-code",
        "role": "orchestrator",
    }));
    assert_eq!(orchestrator["status"], "ok", "{orchestrator}");
    let orchestrator_id = orchestrator["result"]["session"]
        .as_str()
        .unwrap()
        .to_owned();

    let worker = call(serde_json::json!({
        "op": "spawn_session",
        "harness": "claude-code",
    }));
    assert_eq!(worker["status"], "ok", "{worker}");
    let worker_id = worker["result"]["session"].as_str().unwrap().to_owned();

    let listed = call(serde_json::json!({"op": "list_sessions"}));
    let entries = listed["result"].as_array().expect("a session list");

    let orchestrator_role = entries
        .iter()
        .find(|entry| entry["session"] == orchestrator_id)
        .unwrap_or_else(|| panic!("the orchestrator-role session must be listed: {listed}"))["role"]
        .as_str()
        .unwrap();
    assert_eq!(orchestrator_role, "orchestrator");

    let worker_role = entries
        .iter()
        .find(|entry| entry["session"] == worker_id)
        .unwrap_or_else(|| panic!("the spawned worker must be listed: {listed}"))["role"]
        .as_str()
        .unwrap();
    assert_eq!(
        worker_role, "worker",
        "a session spawned with no stated role is a worker by default"
    );

    // What is NOT asserted here, deliberately: that `worker`'s listing entry
    // names `orchestrator_id` as its owner. No such field exists to read —
    // see this test's own doc comment.
    assert!(
        entries.iter().all(|entry| entry.get("owner").is_none()
            && entry.get("orchestrator").is_none()
            && entry.get("spawned_by").is_none()),
        "if this ever starts failing, a per-worker owning-orchestrator field \
         now exists and line 1921 should be re-evaluated as `closed`: {listed}"
    );

    let _ = child.kill();
    let _ = child.wait();
}

// ---------------------------------------------------------------------------
// 1922 — native, direct-provider and gateway-backed launches all record
// harness, launch profile and backend resource
// ---------------------------------------------------------------------------

/// Shape: `tests/route_command.rs`'s `Fixture` (direct-provider profile TOML,
/// a fake harness that just logs and exits) and `tests/gateway_degrade.rs`'s
/// `BinaryFixture` (`kind = "glasshouse-gateway"` backend TOML). Entry:
/// `phase-9a.md` ("line 368 — record the six resolved facts, for every
/// session"), `phase-9h.md`.
///
/// Mutation: `main.rs::launch_session` dropping its
/// `.with_backend_resource(Some(launch_profile.backend.slug()))` call —
/// "stop writing the launch profile on one of the three paths" (all three
/// paths go through this one call, so this mutation covers all three at
/// once; the loop below re-checks every one of the three sessions
/// independently, so a mutation that only broke one path would still be
/// caught by that session's own assertion).
#[test]
fn v1_1922_native_direct_and_gateway_launches_all_record_harness_launch_profile_and_backend() {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let harness = install_quiet_harness(&bin_dir, "quiet-claude-code");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n\n\
             [providers.v1922-probe]\ntemplate = \"anthropic-compatible\"\n\
             base_url = \"http://127.0.0.1:1\"\n\
             credential_env = [\"V1922_PROBE_KEY\"]\n\n\
             [profiles.direct]\nharness = \"claude-code\"\n\n\
             [profiles.direct.backend]\nkind = \"direct-provider\"\n\
             provider = \"v1922-probe\"\n\n\
             [profiles.gateway]\nharness = \"claude-code\"\n\n\
             [profiles.gateway.backend]\nkind = \"glasshouse-gateway\"\n",
            toml_path(&harness)
        ),
    )
    .unwrap();

    let launch = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&project)
            .arg("--data-dir")
            .arg(&data_dir)
            .arg("--config-dir")
            .arg(&config_dir)
            .args(args)
            .env("V1922_PROBE_KEY", "sk-planted-not-a-real-key-1922")
            .output()
            .expect("run glasshouse launch")
    };

    // Native — the implied `native` profile, no `--profile` flag at all.
    let native = launch(&["launch", "claude-code", "--headless"]);
    assert!(
        native.status.success(),
        "native launch failed: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    let direct = launch(&["launch", "claude-code", "--headless", "--profile", "direct"]);
    assert!(
        direct.status.success(),
        "direct-provider launch failed: {}",
        String::from_utf8_lossy(&direct.stderr)
    );

    let gateway = launch(&[
        "launch",
        "claude-code",
        "--headless",
        "--profile",
        "gateway",
    ]);
    assert!(
        gateway.status.success(),
        "gateway-backed launch failed: {}",
        String::from_utf8_lossy(&gateway.stderr)
    );

    let listing = run(&data_dir, &config_dir, &project, &["sessions"]);
    let listing_text = String::from_utf8_lossy(&listing.stdout);
    let ids: Vec<&str> = listing_text
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(
        ids.len(),
        3,
        "expected one recorded session per launch:\n{listing_text}"
    );

    for id in ids {
        let show = run(&data_dir, &config_dir, &project, &["sessions", "show", id]);
        let detail = String::from_utf8_lossy(&show.stdout).into_owned();
        assert_ne!(
            field(&detail, "harness"),
            "-",
            "session {id} has no recorded harness:\n{detail}"
        );
        assert_ne!(
            field(&detail, "launch profile"),
            "-",
            "session {id} has no recorded launch profile:\n{detail}"
        );
        assert_ne!(
            field(&detail, "backend resource"),
            "-",
            "session {id} has no recorded backend resource:\n{detail}"
        );
    }
}

// ---------------------------------------------------------------------------
// 1939 — project isolation and cross-contamination
// ---------------------------------------------------------------------------

/// The four named isolation suites (`tests/project_isolation.rs`,
/// `tests/memory_project_scope.rs`, `tests/mcp_project_scope.rs`,
/// `tests/cmux_project_scope.rs`) are run and their `test result:` lines
/// quoted in this packet's own facts block — not reproduced here, since a
/// `#[test]` in this file cannot invoke `cargo test` on sibling targets
/// without spawning cargo itself, which is what the VERIFICATION COMMANDS'
/// separate `cargo test --test project_isolation --test memory_project_scope
/// --test mcp_project_scope --test cmux_project_scope` invocation is for.
///
/// The MCP door's cross-contamination case is already covered end to end by
/// `tests/mcp_project_scope.rs::memory_and_checkpoints_are_answered_only_for_the_project_the_server_was_started_in`
/// (two real projects, `glasshouse_search_memory`, checked both directions).
/// What none of the four suites drives is `glasshouse memory search` itself
/// — the CLI command a person actually runs — against a memory planted from
/// another project. This is that one addition.
///
/// Shape: `tests/memory_project_scope.rs::plant_foreign_memory` (the
/// trigger-drop-insert-recreate technique) driven through the shipped
/// binary instead of through `MemoryStore` directly. Entry: `phase-1.md`
/// ("keep cross-project memory retrieval disabled by design").
///
/// Mutation: `memory/search.rs::MemoryStore::search_matching` dropping
/// `AND memories.project_id = ?2` from its SQL — "make the memory store
/// [stop filtering by] project."
#[test]
fn v1_1939_memory_search_over_the_cli_refuses_a_memory_planted_from_another_project() {
    let tmp = tempdir();
    let base = tmp.path();
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), "version = 1\n").unwrap();

    let beta = git_project(base, "beta");
    let alpha = git_project(base, "alpha");
    // `alpha` is never bootstrapped — only its identifier is needed, to plant
    // a row that claims to belong to it without ever creating its database.
    let alpha_id = Project::discover(&alpha, None, false)
        .unwrap()
        .id()
        .as_str()
        .to_owned();

    seed_memory(
        &data_dir,
        &config_dir,
        &beta,
        "beta-only fact about the aqueduct",
    );

    let db_path = bootstrap_at(&data_dir, &config_dir, &beta)
        .database_path()
        .to_path_buf();
    let conn = rusqlite::Connection::open(&db_path).expect("open beta's database directly");
    conn.execute_batch("DROP TRIGGER memories_reject_foreign_project_insert;")
        .unwrap();
    conn.execute(
        "INSERT INTO memories (id, project_id, kind, status, body, review_reason, \
         created_at, updated_at) \
         VALUES ('planted-foreign', ?1, 'finding', 'active', ?2, 'project_state', 0, 0)",
        rusqlite::params![alpha_id, "alpha-only fact about the aqueduct"],
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
    drop(conn);

    let search = run(
        &data_dir,
        &config_dir,
        &beta,
        &["memory", "search", "aqueduct"],
    );
    let text = String::from_utf8_lossy(&search.stdout);
    assert!(
        text.contains("beta-only fact"),
        "beta must find its own memory before the boundary means anything: {text}"
    );
    assert!(
        !text.contains("alpha-only fact"),
        "`glasshouse memory search` returned a memory planted from another project: {text}"
    );
}
