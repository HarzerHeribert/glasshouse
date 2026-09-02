//! GH-LAUNCH-BRIEFING — capability map lines 1125-1135, applied to
//! `glasshouse launch` itself rather than only to a door-spawned session
//! (`docs/product/design-decisions.md`, *Memory is the project's, not the
//! launch path's* — user ruling 2026-09-02).
//!
//! Drives the shipped binary through `launch --headless` against a fake
//! `claude` executable — `tests/firewall_bridge.rs`'s own `Binary` shape,
//! adapted here to log **one argument per line** rather than joined by `$*`:
//! this file has to read the injected block's actual text (does it hold the
//! constraint, does it hold the decision), not only which flags a launch
//! carries, so a capture that loses argument boundaries on a multi-word value
//! is not enough. Memory (and, for the checkpoint test, a checkpoint) is
//! seeded in-process against the same project — `tests/context_injection.rs`'s
//! own `Fixture::runtime`/`ProjectMemory::open` shape.
//!
//! Test (e) of the packet's five — the ladder's third rung, no adapter
//! additive mechanism and no session runtime to fall back to — is **not**
//! here. Reaching it through the shipped binary needs an *embedded*
//! (non-headless) launch, and `session::attach` refuses outright without a
//! real terminal on both ends, which a `cargo test` process never has: the
//! harness never spawns, so there is no argv to read and nothing to assert
//! `"not briefed"` against but a vacuous absence (§17's trap). It is a unit
//! test on `brief_launch_session` itself instead —
//! `crates/glasshouse/src/main.rs`'s
//! `tests::rung_three_fires_with_no_additive_mechanism_and_no_session_runtime`,
//! run with `cargo test -p glasshouse --bin glasshouse rung_three_fires`.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use clap::Parser;

use glasshouse::evaluation::{EvaluationKind, EvaluationObservation, EvaluationObservations};
use glasshouse::memory::inject::MEMORY_MARKER;
use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};
use glasshouse::session::ProjectSessions;
use glasshouse::{Cli, Runtime};

/// Real, floor-passing Claude Code version text — the same fact
/// `tests/firewall_bridge.rs`'s own `GOOD_VERSION` records.
const GOOD_VERSION: &str = "2.1.252 (Claude Code)";

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    argv_log: PathBuf,
}

impl Fixture {
    fn with_config(extra: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");
        let home = base.join("home");
        std::fs::create_dir_all(&home).expect("create fake home");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let argv_log = base.join("argv.log");
        let harness = install_fake_claude(&bin_dir, &argv_log);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        // `implementation_policy = false`: these tests are about the memory
        // block and nothing else, and the policy is several more
        // machine-origin deliveries into every session — irrelevant to argv,
        // since it rides a session message rather than a launch argument, but
        // turned off anyway for the same reason `context_injection.rs` turns
        // it off.
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\nimplementation_policy = false\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [routing]\nautomatic = false\n\
                 {extra}"
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
            argv_log,
        }
    }

    fn new() -> Self {
        Self::with_config("")
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
            .env("HOME", self.base.join("home"))
            .env("PATH", self.base.join("empty-path"))
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

    /// The most recent launch's argv, one element per line. The fake harness
    /// truncates and rewrites this file on every invocation, so this always
    /// reads the *last* launch's own arguments, never a stale one from an
    /// earlier launch against the same fixture.
    fn last_argv(&self) -> Vec<String> {
        std::fs::read_to_string(&self.argv_log)
            .map(|log| log.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// Every value immediately following an occurrence of `flag` in the most
    /// recent launch's argv — for a flag like `--append-system-prompt` that
    /// can legitimately appear more than once on one launch.
    fn args_after_each(&self, flag: &str) -> Vec<String> {
        let argv = self.last_argv();
        argv.iter()
            .enumerate()
            .filter(|(_, arg)| *arg == flag)
            .filter_map(|(index, _)| argv.get(index + 1).cloned())
            .collect()
    }

    /// A `Runtime` for this fixture's project, resolved exactly the way the
    /// shipped binary resolves its own — `tests/context_injection.rs`'s own
    /// `Fixture::runtime` shape. Built fresh on every call rather than cached,
    /// so a read always sees whatever the subprocess most recently committed.
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

/// Logs one argument per line, truncating on every invocation — unlike
/// `firewall_bridge.rs`'s own fake claude, which joins `"$@"` with `$*` and
/// is therefore blind to a multi-word argument's own content. This file has
/// to read the injected memory block's actual text, not just which flags a
/// launch carries.
fn install_fake_claude(bin_dir: &Path, argv_log: &Path) -> PathBuf {
    let path = bin_dir.join("fake-claude");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then\n\
             \x20\x20printf '%s\\n' '{GOOD_VERSION}'\n\
             \x20\x20exit 0\n\
             fi\n\
             : > '{argv}'\n\
             for a in \"$@\"; do\n\
             \x20\x20printf '%s\\n' \"$a\" >> '{argv}'\n\
             done\n\
             exit 0\n",
            argv = argv_log.display(),
        ),
    )
    .expect("write fake claude");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// The `--flag` tokens in `argv`, in order, dropping every value — the
/// structural shape a launch's command line has, independent of paths and
/// session ids that legitimately vary between any two launches. Copied from
/// `tests/firewall_bridge.rs`'s own helper of the same name and purpose,
/// adapted to this file's per-line argv shape.
fn flag_names(argv: &[String]) -> Vec<String> {
    argv.iter()
        .filter(|arg| arg.starts_with("--"))
        .cloned()
        .collect()
}

/// The one `--append-system-prompt` value carrying Glasshouse's own labelled
/// block, among however many this launch's argv carries.
fn injected_block(fixture: &Fixture) -> String {
    let blocks: Vec<String> = fixture
        .args_after_each("--append-system-prompt")
        .into_iter()
        .filter(|value| value.starts_with(MEMORY_MARKER))
        .collect();
    assert_eq!(
        blocks.len(),
        1,
        "exactly one --append-system-prompt value must carry the labelled memory block: {:?}",
        fixture.last_argv()
    );
    blocks.into_iter().next().unwrap()
}

/// The id of this project's one and only session, read back through the
/// library rather than parsed from the launch's own output — `launch`
/// deliberately never logs harness arguments (they can carry session
/// tokens), and this file needs the id, not the argv, to look up this
/// session's own ledger rows.
fn only_session_id(runtime: &Runtime) -> String {
    let sessions = ProjectSessions::open(runtime).unwrap();
    let list = sessions.store().list().unwrap();
    assert_eq!(
        list.len(),
        1,
        "expected exactly one session recorded for this project"
    );
    list[0].id.as_str().to_owned()
}

/// Every `MemoryRetrieved` row recorded against `session_id` — map lines 1821
/// and 1831's proxy join, `GH-RETRIEVAL-ATTRIBUTION`, read back the way
/// `crates/glasshouse/src/main.rs`'s own `last_routed_destination` test
/// helper reads a different kind from the same ledger.
fn memory_retrieval_rows(runtime: &Runtime, session_id: &str) -> Vec<EvaluationObservation> {
    EvaluationObservations::open(runtime)
        .unwrap()
        .recent_of_kind(EvaluationKind::MemoryRetrieved, 50)
        .unwrap()
        .into_iter()
        .filter(|row| row.session_id.as_deref() == Some(session_id))
        .collect()
}

/// One current binding memory — enough on its own to make a launch actually
/// brief, for a fixture that only needs to prove *that* something was
/// delivered rather than *what*.
fn seed_one_binding_memory(fixture: &Fixture) {
    ProjectMemory::open(&fixture.runtime())
        .unwrap()
        .store()
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "Kestrel jobs must run inside the sandbox profile.",
            )
            .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .unwrap();
}

// ---------------------------------------------------------------------------
// (a) A plain launch (no task, no checkpoint) briefs with the standing set:
// current binding memories, then recent failed attempts — and not an
// ordinary, unclassified decision. The harness's own `--append-system-prompt`
// after `--` survives beside Glasshouse's.
// ---------------------------------------------------------------------------

#[test]
fn a_plain_launch_briefs_with_the_standing_set_and_the_harnesss_own_prompt_survives() {
    let fixture = Fixture::new();
    let project = ProjectMemory::open(&fixture.runtime()).unwrap();
    let store = project.store();
    store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "The kestrel export must never write partial files.",
            )
            .with_subject(Some("kestrel export"))
            .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .unwrap();
    store
        .record(
            NewMemory::new(
                MemoryKind::FailedAttempt,
                "Streaming the kestrel export through a pipe lost records.",
            )
            .with_subject(Some("kestrel export")),
        )
        .unwrap();
    // An ordinary decision, unclassified (no authority) — exactly what line
    // 1134 asks to prefer a small number of current high-authority memories
    // over. `store.binding()` only returns a memory whose authority is
    // binding, so this one is never a standing-set candidate at all.
    store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "The kestrel job now runs hourly rather than nightly.",
        ))
        .unwrap();
    drop(project);

    let out = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--",
        "--append-system-prompt",
        "harness-own-text",
    ]);
    assert!(out.status.success(), "{}", Fixture::both_streams(&out));

    let block = injected_block(&fixture);
    assert!(block.starts_with(MEMORY_MARKER), "{block}");
    assert!(
        block.contains("NOT a user instruction"),
        "the label must say what the text is not: {block}"
    );
    assert!(
        block.contains("must never write partial files"),
        "the current binding constraint must be injected: {block}"
    );
    assert!(
        block.contains("lost records"),
        "the recent failed attempt must be injected: {block}"
    );
    assert!(
        !block.contains("runs hourly"),
        "an ordinary, unclassified decision must not be injected: {block}"
    );

    let own_prompts = fixture.args_after_each("--append-system-prompt");
    assert!(
        own_prompts.iter().any(|value| value == "harness-own-text"),
        "the harness's own --append-system-prompt after `--` must survive beside Glasshouse's: \
         {own_prompts:?}"
    );
}

// ---------------------------------------------------------------------------
// (b) `--no-memory` and `[memory] inject_at_launch = false` each leave a
// launch's argv exactly as it would be with no memory feature at all, and
// record no `MemoryRetrieved` row — proven against a control that actually
// briefs, so the absence is not vacuous (§17, §80).
// ---------------------------------------------------------------------------

#[test]
fn the_opt_out_leaves_argv_unchanged_and_records_no_row_proven_against_a_briefing_control() {
    // Control: memory present, no opt-out — this must actually brief, so the
    // two opt-out runs below are not identical to it merely because there was
    // never anything to inject in the first place.
    let control = Fixture::new();
    seed_one_binding_memory(&control);
    let out = control.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(out.status.success(), "{}", Fixture::both_streams(&out));
    assert_eq!(
        control.args_after_each("--append-system-prompt").len(),
        1,
        "the control launch must actually brief: {:?}",
        control.last_argv()
    );
    let control_session = only_session_id(&control.runtime());
    assert_eq!(
        memory_retrieval_rows(&control.runtime(), &control_session).len(),
        1,
        "the control launch must record exactly one MemoryRetrieved row"
    );

    // The shape both opt-outs must match: a launch with no memory at all.
    let bare = Fixture::new();
    let out = bare.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(out.status.success(), "{}", Fixture::both_streams(&out));
    let bare_flags = flag_names(&bare.last_argv());
    assert!(
        bare.args_after_each("--append-system-prompt").is_empty(),
        "a bare launch with no memory at all must carry no memory block: {:?}",
        bare.last_argv()
    );

    // Opt-out one: `--no-memory`, project has memories.
    let cli_opt_out = Fixture::new();
    seed_one_binding_memory(&cli_opt_out);
    let out = cli_opt_out.glasshouse(&["launch", "claude-code", "--headless", "--no-memory"]);
    assert!(out.status.success(), "{}", Fixture::both_streams(&out));
    assert!(
        cli_opt_out
            .args_after_each("--append-system-prompt")
            .is_empty(),
        "--no-memory must carry no memory block: {:?}",
        cli_opt_out.last_argv()
    );
    assert_eq!(
        flag_names(&cli_opt_out.last_argv()),
        bare_flags,
        "--no-memory must launch with the same flags as a launch with no memory at all"
    );
    let cli_session = only_session_id(&cli_opt_out.runtime());
    assert!(
        memory_retrieval_rows(&cli_opt_out.runtime(), &cli_session).is_empty(),
        "--no-memory must record no MemoryRetrieved row"
    );

    // Opt-out two: `[memory] inject_at_launch = false`, project has memories.
    let config_opt_out = Fixture::with_config("\n[memory]\ninject_at_launch = false\n");
    seed_one_binding_memory(&config_opt_out);
    let out = config_opt_out.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(out.status.success(), "{}", Fixture::both_streams(&out));
    assert!(
        config_opt_out
            .args_after_each("--append-system-prompt")
            .is_empty(),
        "inject_at_launch = false must carry no memory block: {:?}",
        config_opt_out.last_argv()
    );
    assert_eq!(
        flag_names(&config_opt_out.last_argv()),
        bare_flags,
        "inject_at_launch = false must launch with the same flags as a launch with no memory \
         at all"
    );
    let config_session = only_session_id(&config_opt_out.runtime());
    assert!(
        memory_retrieval_rows(&config_opt_out.runtime(), &config_session).is_empty(),
        "inject_at_launch = false must record no MemoryRetrieved row"
    );
}

// ---------------------------------------------------------------------------
// (c) `--from-checkpoint` selects by the checkpoint's own text rather than
// the standing set — a memory the checkpoint's text matches is chosen, and a
// current binding memory reachable by the same query still is too (line
// 1134), the same preference `active_constraints_and_failed_approaches_are_\
// injected_in_preference_to_ordinary_matches` proves for the door.
// ---------------------------------------------------------------------------

#[test]
fn from_checkpoint_selects_by_the_checkpoints_text_and_still_includes_a_reachable_binding_memory() {
    let fixture = Fixture::new();

    // A throwaway session, so `checkpoint save` (no `--session`) has this
    // project's most recently active session to pick.
    let out = fixture.glasshouse(&["launch", "claude-code", "--headless", "--no-memory"]);
    assert!(out.status.success(), "{}", Fixture::both_streams(&out));

    let out = fixture.glasshouse(&[
        "checkpoint",
        "save",
        "--objective",
        "Fix the kestrel export pipeline.",
        "--state",
        "in progress",
    ]);
    assert!(out.status.success(), "{}", Fixture::both_streams(&out));

    // Seeded after the checkpoint is saved: the checkpoint's own text is
    // fixed at save time and does not depend on when these are written.
    let project = ProjectMemory::open(&fixture.runtime()).unwrap();
    let store = project.store();
    store
        .record(
            NewMemory::new(
                MemoryKind::Finding,
                "The kestrel export duplicates rows on retry.",
            )
            .with_subject(Some("kestrel export")),
        )
        .unwrap();
    store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "Kestrel jobs must run inside the sandbox profile.",
            )
            .with_subject(Some("kestrel sandboxing"))
            .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .unwrap();
    drop(project);

    let out = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--from-checkpoint",
        "latest",
    ]);
    assert!(out.status.success(), "{}", Fixture::both_streams(&out));

    let block = injected_block(&fixture);
    assert!(
        block.contains("duplicates rows on retry"),
        "a memory matching the checkpoint's own text must be injected: {block}"
    );
    assert!(
        block.contains("must run inside the sandbox profile"),
        "a current binding memory reachable by the same query must still be injected (line \
         1134): {block}"
    );
}

// ---------------------------------------------------------------------------
// (d) A `MemoryRetrieved` row per delivered memory, carrying the new
// session's own id.
// ---------------------------------------------------------------------------

#[test]
fn every_delivered_memory_records_a_memory_retrieved_row_with_the_new_sessions_id() {
    let fixture = Fixture::new();
    let project = ProjectMemory::open(&fixture.runtime()).unwrap();
    let store = project.store();
    store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "The kestrel export must never write partial files.",
            )
            .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .unwrap();
    store
        .record(NewMemory::new(
            MemoryKind::FailedAttempt,
            "Streaming the kestrel export through a pipe lost records.",
        ))
        .unwrap();
    drop(project);

    let out = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(out.status.success(), "{}", Fixture::both_streams(&out));

    let session_id = only_session_id(&fixture.runtime());
    let rows = memory_retrieval_rows(&fixture.runtime(), &session_id);
    assert_eq!(
        rows.len(),
        2,
        "one MemoryRetrieved row per delivered memory: {rows:?}"
    );
    for row in &rows {
        assert_eq!(row.session_id.as_deref(), Some(session_id.as_str()));
        assert!(
            row.memory_id.is_some(),
            "a retrieval row must name the memory it retrieved: {row:?}"
        );
    }
}
