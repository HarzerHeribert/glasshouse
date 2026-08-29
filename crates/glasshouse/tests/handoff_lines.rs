//! Phase 40 lines 1638, 1639, 1642, 1643, 1644, 1645 and 1646 — a
//! checkpoint's `--from-checkpoint` launch, exercised through the shipped
//! binary rather than through the library alone.
//!
//! # Why this exists beside `checkpoint_portability.rs`
//!
//! Every test in that file reaches the checkpoint format and store through
//! the library — `Checkpoint::capture`, `CheckpointStore::save`,
//! `Checkpoint::bootstrap_prompt` — never through `main.rs::launch_session`
//! and `main.rs::resolve_bootstrap_prompt`, which are what actually create a
//! fresh session from `--from-checkpoint` and hand it the prompt. Before this
//! file, `--from-checkpoint` had no test anywhere: `grep -rn
//! 'from.checkpoint' crates/glasshouse/tests/` matched nothing. §35's shape
//! exactly — a caller every test bypasses is not a caller — so these run the
//! real binary, under a real pseudo-terminal, against a fake harness that
//! records its own arguments to a file.
//!
//! # Why this is not `tests/pty_smoke.rs`, and not that file's `Session`
//!
//! `pty_smoke.rs` already has a `Session` helper that answers Windows
//! ConPTY's startup handshake, which is the correct machinery for a test
//! that watches a harness's own terminal output. These tests do not: the
//! fake harnesses here write nothing to their own terminal at all, and what
//! is being checked (the argument list Glasshouse handed them) is read
//! directly off disk. Reusing that file's ~300-line apparatus for a
//! capability that needs none of it would be duplication in the wrong
//! direction, and `pty_smoke.rs` was not this packet's file to edit. What
//! this file needs instead — spawn under a real pty, drain its output so
//! nothing backs up, wait for exit with a bound — is under thirty lines, in
//! [`run_to_exit`] below, and is Unix-only: the semantics under test are
//! platform-independent, and only the fake-harness shell scripts are not.
#![cfg(unix)]

use std::io::Read as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use clap::Parser as _;
use glasshouse::pty::{PtyProcess, TerminalCommand};
use glasshouse::session::ProjectSessions;
use glasshouse::{Cli, bootstrap};

const TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(20);

/// Spawn `command` under a real pty, drain its output on a background thread
/// (discarded — nothing here reads a harness's terminal output, and without
/// draining, enough of it can fill the pty's kernel buffer and block the
/// child on a `write`), and wait for exit bounded by [`TIMEOUT`].
fn run_to_exit(command: TerminalCommand) -> glasshouse::pty::ExitStatus {
    let (mut process, mut output) = PtyProcess::spawn(command).expect("spawn under a pty");
    std::thread::spawn(move || {
        let mut sink = [0u8; 4096];
        loop {
            match output.read(&mut sink) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    });
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = process.try_wait().expect("try_wait") {
            return status;
        }
        assert!(Instant::now() < deadline, "the process never exited");
        std::thread::sleep(POLL);
    }
}

/// A fake harness that, on every invocation, overwrites `dump_to` with its
/// own arguments, NUL-separated (never space-separated: a checkpoint's
/// bootstrap prompt is itself multi-line, and a NUL cannot appear inside a
/// shell argument, which makes it an unambiguous separator that a newline is
/// not). It writes nothing to its own stdout or stderr.
fn install_argv_dump_harness(bin_dir: &Path, name: &str, dump_to: &Path) -> PathBuf {
    let path = bin_dir.join(name);
    let script = format!(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > '{}'\nexit 0\n",
        dump_to.display()
    );
    std::fs::write(&path, script).expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// The last NUL-separated argument [`install_argv_dump_harness`] recorded —
/// the bootstrap prompt, always the last argument the launch path appends,
/// since these tests pass no `--` arguments of their own.
fn last_argument(dump_path: &Path) -> String {
    let bytes = std::fs::read(dump_path)
        .unwrap_or_else(|err| panic!("the harness never wrote {}: {err}", dump_path.display()));
    let text = String::from_utf8(bytes).expect("argv must be UTF-8");
    text.split('\0')
        .rfind(|part| !part.is_empty())
        .unwrap_or_else(|| panic!("the harness recorded no arguments"))
        .to_owned()
}

/// A project with three fake harnesses configured — `claude-code`, `codex`
/// and `antigravity` — each dumping its own arguments to its own file, plus
/// the plumbing to run `glasshouse` against it both under a pty (`launch`,
/// which needs a controlling terminal) and without one (`checkpoint save`,
/// which does not).
struct Project {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    data_dir: PathBuf,
    config_dir: PathBuf,
    dumps: std::collections::HashMap<&'static str, PathBuf>,
}

impl Project {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let data_dir = base.join("data");
        let config_dir = base.join("config");

        let mut dumps = std::collections::HashMap::new();
        let mut integrations = String::new();
        for slug in ["claude-code", "codex", "antigravity"] {
            let dump = base.join(format!("{slug}.argv"));
            let harness = install_argv_dump_harness(&bin_dir, slug, &dump);
            integrations.push_str(&format!(
                "[integrations.{slug}]\nenabled = true\nexecutable = \"{}\"\n\n",
                harness.display()
            ));
            dumps.insert(slug, dump);
        }
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!("version = 1\n\n{integrations}"),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            root,
            data_dir,
            config_dir,
            dumps,
        }
    }

    fn base_args(&self) -> Vec<String> {
        vec![
            "--scope".to_owned(),
            self.root.display().to_string(),
            "--data-dir".to_owned(),
            self.data_dir.display().to_string(),
            "--config-dir".to_owned(),
            self.config_dir.display().to_string(),
        ]
    }

    /// `glasshouse launch <harness>`, under a real pty, waited to exit.
    fn launch(&self, extra: &[&str]) {
        let mut command = TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), &self.root);
        for arg in self.base_args() {
            command = command.arg(arg);
        }
        command = command.arg("launch");
        for arg in extra {
            command = command.arg(*arg);
        }
        let status = run_to_exit(command);
        assert!(
            status.success(),
            "`glasshouse launch {extra:?}` did not exit cleanly: {status:?}"
        );
    }

    /// `glasshouse checkpoint save ...`, which needs no terminal of its own.
    /// Returns the checkpoint id `glasshouse checkpoint list` would print.
    fn checkpoint_save(&self, extra: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .args(self.base_args())
            .arg("checkpoint")
            .arg("save")
            .args(extra)
            .output()
            .expect("run glasshouse checkpoint save");
        assert!(
            output.status.success(),
            "checkpoint save failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let first_line = stdout
            .lines()
            .next()
            .unwrap_or_else(|| panic!("checkpoint save printed nothing:\n{stdout}"));
        first_line
            .strip_prefix("checkpoint ")
            .unwrap_or_else(|| panic!("unexpected checkpoint save output: {first_line}"))
            .trim()
            .to_owned()
    }

    fn runtime(&self) -> glasshouse::Runtime {
        let cli =
            Cli::try_parse_from(std::iter::once("glasshouse".to_owned()).chain(self.base_args()))
                .expect("cli");
        bootstrap(&cli, &self.root).expect("bootstrap")
    }

    /// Every session record currently in the project, read the way
    /// production reads it back — a second process, not the value this
    /// process happened to build.
    fn sessions(&self) -> Vec<glasshouse::session::SessionRecord> {
        ProjectSessions::open(&self.runtime())
            .expect("open sessions")
            .store()
            .list()
            .expect("list sessions")
    }

    fn dump(&self, slug: &str) -> &Path {
        self.dumps.get(slug).expect("configured harness")
    }
}

/// **The one that matters.** For three cross-harness pairs — Codex to Claude
/// Code (line 1643), Claude Code to Codex (line 1642), and Claude Code to
/// Antigravity (line 1644, "either" harness to Antigravity) — record a
/// session under the source harness, take a checkpoint from it naming no
/// target, launch the target harness `--from-checkpoint` that id, and check,
/// against the shipped binary:
///
/// - **1638**: a *fresh* session is recorded — the session count grows by
///   exactly one, under the target harness, distinct from the source.
/// - **1639**: what the target harness actually received is plain text, not
///   JSON, names no harness at all, and carries the explicit `OBJECTIVE` and
///   `CURRENT STATE` sections the map asks for — an explicit handoff, not a
///   copy of anything.
/// - **1642/1643/1644**: the launch succeeds under the *requested* harness
///   regardless of which harness the checkpoint itself was recorded under —
///   nothing in the launch path consults the checkpoint's own `harness`
///   field to decide where the session may go.
/// - **1645**: the source session's own record — harness, lifecycle,
///   presentation — is byte-for-byte the same after the checkpoint was taken
///   and the target was launched as it was before either happened.
#[test]
fn a_checkpoint_bootstraps_a_fresh_session_under_a_different_harness_through_the_shipped_binary() {
    let project = Project::new();

    for (source_slug, target_slug, line) in [
        ("codex", "claude-code", "1643 (Codex -> Claude Code)"),
        ("claude-code", "codex", "1642 (Claude Code -> Codex)"),
        ("claude-code", "antigravity", "1644 (-> Antigravity)"),
    ] {
        let before = project.sessions();

        project.launch(&[source_slug]);

        let after_source = project.sessions();
        assert_eq!(
            after_source.len(),
            before.len() + 1,
            "[{line}] launching the source harness must record exactly one session"
        );
        let source_id = after_source
            .iter()
            .find(|record| !before.iter().any(|old| old.id == record.id))
            .unwrap_or_else(|| panic!("[{line}] no new session record after the source launch"))
            .id
            .clone();
        let source_before = after_source
            .iter()
            .find(|record| record.id == source_id)
            .unwrap()
            .clone();
        // 1646, the negative half: a session started with no
        // `--from-checkpoint` at all must record no source, never an
        // invented one.
        assert_eq!(
            source_before.source_session_id, None,
            "[{line}] a session launched without --from-checkpoint recorded a source"
        );

        let marker = format!("HANDOFF-{}", line.split_whitespace().next().unwrap());
        let checkpoint_id = project.checkpoint_save(&[
            "--session",
            source_id.as_str(),
            "--objective",
            &format!("{marker} objective: finish the cross-harness handoff test"),
            "--state",
            &format!("{marker} state: the source session just finished"),
            "--decision",
            &format!("{marker} decision: use a NUL-separated argv dump"),
            "--failed",
            &format!("{marker} failed: tried reading the harness's own terminal output"),
            "--next",
            &format!("{marker} next: launch the target harness from this checkpoint"),
        ]);

        project.launch(&[target_slug, "--from-checkpoint", &checkpoint_id]);

        // 1638: exactly one more session, under the target harness.
        let after_target = project.sessions();
        assert_eq!(
            after_target.len(),
            after_source.len() + 1,
            "[{line}] launching --from-checkpoint must record exactly one fresh session"
        );
        let target_record = after_target
            .iter()
            .find(|record| !after_source.iter().any(|old| old.id == record.id))
            .unwrap_or_else(|| panic!("[{line}] no new session record after the target launch"));
        assert_eq!(target_record.harness, target_slug, "[{line}]");
        assert_ne!(
            target_record.id, source_id,
            "[{line}] the fresh session must not be the source session"
        );

        // 1646: the fresh session records which session it was bootstrapped
        // from.
        assert_eq!(
            target_record.source_session_id,
            Some(source_id.clone()),
            "[{line}] the fresh session did not record its source session"
        );

        // 1639: an explicit, plain-text, harness-agnostic handoff.
        let prompt = last_argument(project.dump(target_slug));
        assert!(
            !prompt.trim_start().starts_with('{'),
            "[{line}] the harness received JSON, not a plain-text prompt: {prompt}"
        );
        assert!(prompt.contains("OBJECTIVE"), "[{line}]: {prompt}");
        assert!(prompt.contains("CURRENT STATE"), "[{line}]: {prompt}");
        assert!(
            prompt.contains(&format!("{marker} objective")),
            "[{line}]: {prompt}"
        );
        assert!(
            prompt.contains(&format!("{marker} state")),
            "[{line}]: {prompt}"
        );
        let lowered = prompt.to_ascii_lowercase();
        for harness in ["claude", "codex", "antigravity"] {
            assert!(
                !lowered.contains(harness),
                "[{line}] the prompt handed to the target harness names `{harness}`: {prompt}"
            );
        }

        // 1642/1643/1644: the launch above already succeeded under the
        // *requested* harness while the checkpoint itself was recorded under
        // `source_slug` — a different one whenever source != target, which
        // is every case in this table. Nothing refused it, and nothing
        // silently redirected it back to the source harness: `target_record`
        // above is recorded under `target_slug`, not `source_slug`.

        // 1645: the source session, re-read after the checkpoint and the
        // target launch, is exactly what it was before either happened.
        let source_after = project
            .sessions()
            .into_iter()
            .find(|record| record.id == source_id)
            .unwrap_or_else(|| panic!("[{line}] the source session disappeared"));
        assert_eq!(
            source_after.harness, source_before.harness,
            "[{line}] the source session's harness changed"
        );
        assert_eq!(
            source_after.lifecycle, source_before.lifecycle,
            "[{line}] the source session's lifecycle changed"
        );
        assert_eq!(
            source_after.presentation, source_before.presentation,
            "[{line}] the source session's presentation changed"
        );
        assert_eq!(
            source_after.native_session_id, source_before.native_session_id,
            "[{line}] the source session's native identifier changed"
        );
    }
}
