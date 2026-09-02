//! GH-FIREWALL-BRIDGE — Phase 57 map lines 1991-1996: modes, session-scoped
//! Claude Code registration, session-start verification with shadow
//! fallback, the Bash adapter completed empirically, and per-session
//! separation proven through the registered path.
//!
//! Two halves. The first drives `glasshouse context-firewall hook` directly
//! (`tests/context_firewall.rs`'s own `Fixture` shape) for box lines that
//! are properties of the hook subprocess itself — the mode decision and the
//! real captured Bash shape. The second drives the shipped binary's
//! `launch --headless` against a fake `claude` executable
//! (`tests/entitlement_pool.rs`'s own `Binary` shape) for box lines that are
//! properties of *registration* — what `launch_session` writes before the
//! harness ever starts.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// ===========================================================================
// Half one — the hook subprocess directly.
// ===========================================================================

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    fn run(&self, args: &[&str], stdin: &[u8]) -> Output {
        use std::io::Write;
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.dir)
            .arg("--data-dir")
            .arg(self.dir.join("data"))
            .arg("--config-dir")
            .arg(self.dir.join("config"))
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("glasshouse must be spawnable");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin)
            .expect("stdin must accept the event");
        child.wait_with_output().expect("glasshouse must exit")
    }

    fn hook(&self, event: &serde_json::Value, extra_args: &[&str]) -> serde_json::Value {
        let mut args = vec!["context-firewall", "hook"];
        args.extend_from_slice(extra_args);
        let bytes = serde_json::to_vec(event).unwrap();
        let output = self.run(&args, &bytes);
        assert!(
            output.status.success(),
            "the hook must always exit 0 (fail open): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("hook response must be valid JSON")
    }
}

fn post_tool_use(
    tool_name: &str,
    tool_response: serde_json::Value,
    session_id: &str,
    tool_use_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "tool_name": tool_name,
        "tool_input": {},
        "tool_response": tool_response,
        "tool_use_id": tool_use_id,
        "session_id": session_id,
        "cwd": "/tmp",
    })
}

fn updated_output(response: &serde_json::Value) -> Option<&str> {
    response
        .get("hookSpecificOutput")
        .and_then(|v| v.get("updatedToolOutput"))
        .and_then(|v| v.as_str())
}

/// The REAL captured Bash `PostToolUse` payload — installed Claude Code
/// 2.1.252, a throwaway hook teeing stdin to a file, driven by
/// `claude -p "...echo probe-token-$RANDOM-marker..."` in a scratch project
/// (documented in this package's report, and reproduced as a fixture in
/// `firewall::adapter::tests`). Personal paths are scrubbed; every key and
/// value below — including the absent `exit_code` — is the real capture.
fn real_captured_bash_success() -> serde_json::Value {
    serde_json::json!({
        "stdout": "capture-probe-line-one\ncapture-probe-line-two",
        "stderr": "",
        "interrupted": false,
        "isImage": false,
        "noOutputExpected": false
    })
}

/// **Map line 1995, through the shipped binary.** The real captured Bash
/// success shape — no `exit_code` key at all — reduces exactly like any
/// other confirmed-clean command result, because a real `PostToolUse` event
/// for Bash is itself the positive exit signal: a failing Bash call never
/// reaches this hook (it fires `PostToolUseFailure` instead, which this
/// build does not register). `firewall::adapter::tests` proves the same
/// fact at the normalization layer; this proves it end to end.
#[test]
fn line_1995_the_real_captured_bash_shape_reduces_through_the_shipped_hook() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let mut big = String::new();
    for i in 0..300 {
        big.push_str(&format!("real capture noise line {i}\n"));
    }
    let mut response = real_captured_bash_success();
    response["stdout"] = serde_json::Value::String(big.clone());

    let event = post_tool_use("Bash", response, "s-1995", "tu-1");
    let hook_response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&hook_response)
        .expect("the real captured shape (no exit_code key) must reduce and emit");
    assert!(forwarded.contains("glasshouse context firewall"));
}

/// **`shadow` never emits `updatedToolOutput`, even when the caller also
/// passes `--emit-updated-output`.** This is the mode decision enforced
/// inside the hook subprocess itself — defense in depth beneath
/// `install_context_firewall_hook`'s own choice never to bake
/// `--emit-updated-output` onto a shadow registration in the first place.
/// Mutation target (a): inverting this check must be KILLED by this test.
#[test]
fn shadow_mode_overrides_emit_updated_output_inside_the_hook_subprocess() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let big = "distinct oversized line for shadow mode testing purposes only\n".repeat(50);
    let event = post_tool_use(
        "Read",
        serde_json::json!({"type": "text", "text": big}),
        "s-shadow",
        "tu-1",
    );

    let response = fixture.hook(
        &event,
        &[
            "--passthrough-tokens",
            "10",
            "--emit-updated-output",
            "--mode",
            "shadow",
        ],
    );
    assert_eq!(
        response,
        serde_json::json!({}),
        "shadow must never carry updatedToolOutput, whatever --emit-updated-output says"
    );

    // The contrast: the identical event under `safe` DOES emit, proving the
    // difference is the mode flag and not some other suppression.
    let safe_response = fixture.hook(
        &event,
        &[
            "--passthrough-tokens",
            "10",
            "--emit-updated-output",
            "--mode",
            "safe",
        ],
    );
    assert!(updated_output(&safe_response).is_some());
}

/// **Map line 1992's tripwire, at the hook subprocess level.** No mode's
/// registered command line — reconstructed here as the exact flags a
/// registration would pass — ever appears alongside a `--reducer` or
/// `--provider` flag, because no such flag exists on the subcommand at all.
/// `clap` itself is the enforcement; this asserts the vocabulary stays
/// absent from `--help`, so an accidental future addition is caught here
/// rather than discovered empirically.
#[test]
fn line_1992_the_hook_subcommand_has_no_reducer_or_provider_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let output = fixture.run(&["context-firewall", "hook", "--help"], b"");
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(!help.contains("--reducer"));
    assert!(!help.contains("--provider"));
}

/// An unrecognized `--mode` value is refused, naming the vocabulary — the
/// same discipline `parse_guardrail_override` uses for `--guardrail`.
#[test]
fn an_unrecognized_mode_is_refused_and_names_the_vocabulary() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let event = post_tool_use(
        "Read",
        serde_json::json!({"type": "text", "text": "x"}),
        "s",
        "tu",
    );
    let output = fixture.run(
        &["context-firewall", "hook", "--mode", "stealth"],
        &serde_json::to_vec(&event).unwrap(),
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("stealth"));
    assert!(stderr.contains("off") && stderr.contains("aggressive"));
}

// ===========================================================================
// Half two — the shipped binary through `launch --headless`, against a fake
// `claude` executable. `tests/entitlement_pool.rs`'s `Binary` shape,
// reproduced for the reason that file gives for reproducing its own
// fixture: integration tests are separate crates.
// ===========================================================================

struct Binary {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    argv_log: PathBuf,
    home: PathBuf,
}

impl Binary {
    /// `version_line` is exactly what the fake harness's `--version` prints
    /// — the seam that lets one test drive a floor-passing harness and
    /// another an old or unparseable one, without touching production code.
    fn with_config(extra: &str, version_line: &str) -> Self {
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
        let harness = install_fake_claude(&bin_dir, &argv_log, version_line);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
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
            home,
        }
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
            .env("HOME", &self.home)
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

    fn harness_invocations(&self) -> Vec<String> {
        match std::fs::read_to_string(&self.argv_log) {
            Ok(log) => log.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Every `claude-settings.json` Glasshouse wrote anywhere under its own
    /// data directory for this project — one per session, by construction.
    fn written_settings_documents(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        find_named(&self.base.join("data"), "claude-settings.json", &mut found);
        found
    }

    /// Whether Glasshouse ever wrote anything under the fake `$HOME` or the
    /// project root — the invariant that the real user's own `~/.claude` and
    /// project `.claude/` are never touched (map line 1993, REQUIRED
    /// BEHAVIOR).
    fn user_owned_claude_dirs_are_untouched(&self) -> bool {
        !self.home.join(".claude").exists() && !self.root.join(".claude").exists()
    }
}

fn find_named(dir: &Path, name: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_named(&path, name, out);
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            out.push(path);
        }
    }
}

const GOOD_VERSION: &str = "2.1.252 (Claude Code)";
const OLD_VERSION: &str = "2.1.100 (Claude Code)";
const UNPARSEABLE_VERSION: &str = "not a version at all";

#[cfg(unix)]
fn install_fake_claude(bin_dir: &Path, argv_log: &Path, version_line: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-claude");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then\n\
             \x20\x20printf '%s\\n' '{version_line}'\n\
             \x20\x20exit 0\n\
             fi\n\
             printf '%s\\n' \"$*\" >> '{argv_log}'\n\
             exit 0\n",
            argv_log = argv_log.display(),
        ),
    )
    .expect("write fake claude");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_claude(bin_dir: &Path, argv_log: &Path, version_line: &str) -> PathBuf {
    let path = bin_dir.join("fake-claude.cmd");
    // `version_line` (e.g. "2.1.252 (Claude Code)") lands inside a
    // parenthesized `if (...)` block. cmd.exe's block-form parser counts
    // parens to find the block's own closing one, so an unescaped `(`/`)`
    // in the echoed text terminates the block early and corrupts every
    // later line in the script -- confirmed on the VM: the unescaped form
    // truncated `--version`'s own output (`2.1.252 (Claude Code` with no
    // closing paren) and, for any other argv, failed to invoke the file at
    // all ("The system cannot find the file specified"), so argv.log was
    // never written. `^(` / `^)` escapes them as literal text.
    let escaped_version = version_line.replace('(', "^(").replace(')', "^)");
    std::fs::write(
        &path,
        format!(
            "@echo off\r\n\
             if \"%1\"==\"--version\" (\r\n\
             \x20\x20echo {escaped_version}\r\n\
             \x20\x20exit /b 0\r\n\
             )\r\n\
             echo %*>>\"{argv}\"\r\n\
             exit /b 0\r\n",
            argv = argv_log.display(),
        ),
    )
    .expect("write fake claude");
    path
}

/// Reads the registered `PostToolUse` command line out of a written
/// `claude-settings.json`, or `None` when the key is absent.
fn registered_command_line(document_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(document_path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
    parsed["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .map(str::to_owned)
}

/// **Map line 1991's `off`, and REQUIRED BEHAVIOR's byte-identical
/// regression pin.** With no `[context_firewall]` table at all — the same
/// as an explicit `mode = "off"` — the launched command line is exactly
/// what a build before this phase existed would have produced, and no
/// `PostToolUse` key is ever added to the settings document.
#[test]
fn line_1991_mode_off_is_byte_identical_and_registers_nothing() {
    let no_config = Binary::with_config("", GOOD_VERSION);
    let explicit_off = Binary::with_config("\n[context_firewall]\nmode = \"off\"\n", GOOD_VERSION);

    for binary in [&no_config, &explicit_off] {
        let out = binary.glasshouse(&["launch", "claude-code", "--headless"]);
        assert!(out.status.success(), "{}", Binary::both_streams(&out));
        assert_eq!(binary.harness_invocations().len(), 1);

        let documents = binary.written_settings_documents();
        assert_eq!(
            documents.len(),
            1,
            "exactly one settings document per session"
        );
        assert!(
            registered_command_line(&documents[0]).is_none(),
            "mode off must never add a PostToolUse hook: {}",
            std::fs::read_to_string(&documents[0]).unwrap_or_default()
        );
    }

    // The regression pin itself: the two configurations produce
    // structurally identical argv — same flags, same order — because the
    // context firewall never touches `args` at all, only the settings
    // document's own contents, which off also leaves alone. Comparing the
    // flag *names* rather than the raw strings is deliberate: `--settings
    // <path>` and `--session-id <uuid>` legitimately differ in *value*
    // between any two launches (a fresh temp directory, a fresh random
    // session id) even with identical configuration — that variation is not
    // what this test is pinning.
    assert_eq!(
        flag_names(&no_config.harness_invocations()[0]),
        flag_names(&explicit_off.harness_invocations()[0]),
        "an absent [context_firewall] table and an explicit mode = \"off\" must launch with the \
         same flags:\n{}\n{}",
        no_config.harness_invocations()[0],
        explicit_off.harness_invocations()[0],
    );
}

/// The `--flag` tokens in `argv`, in order, dropping every value — the
/// structural shape a launch's command line has, independent of paths and
/// session ids that legitimately vary between any two launches.
fn flag_names(argv: &str) -> Vec<&str> {
    argv.split_whitespace()
        .filter(|token| token.starts_with("--"))
        .collect()
}

/// **Map line 1991's remaining three modes, and map line 1992's tripwire
/// against the real registered command line.** Each mode's registration
/// carries its own `--mode` flag and never names a reducer or provider.
#[test]
fn line_1991_shadow_safe_and_aggressive_each_register_their_own_mode_flag() {
    for (mode, expect_emit) in [("shadow", false), ("safe", true), ("aggressive", true)] {
        let binary = Binary::with_config(
            &format!("\n[context_firewall]\nmode = \"{mode}\"\n"),
            GOOD_VERSION,
        );
        let out = binary.glasshouse(&["launch", "claude-code", "--headless"]);
        assert!(out.status.success(), "{}", Binary::both_streams(&out));

        let documents = binary.written_settings_documents();
        assert_eq!(documents.len(), 1);
        let command = registered_command_line(&documents[0])
            .unwrap_or_else(|| panic!("mode {mode} must register a PostToolUse hook"));
        assert!(command.contains(&format!("--mode {mode}")), "{command}");
        assert_eq!(
            command.contains("--emit-updated-output"),
            expect_emit,
            "mode {mode}: {command}"
        );
        assert!(
            !command.contains("--reducer") && !command.contains("--provider"),
            "{command}"
        );
    }
}

/// **Map line 1993.** Registration merges the `PostToolUse` entry into the
/// SAME settings document lifecycle hooks already share — never a second
/// `--settings` flag (which would silently discard one or the other) — and
/// every existing lifecycle event survives untouched beside it. The command
/// itself invokes the absolute Glasshouse binary path.
#[test]
fn line_1993_registration_shares_one_settings_document_and_touches_no_other_hook() {
    let binary = Binary::with_config("\n[context_firewall]\nmode = \"safe\"\n", GOOD_VERSION);
    let out = binary.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(out.status.success(), "{}", Binary::both_streams(&out));

    // Exactly one `--settings` flag reaches the harness.
    let invocations = binary.harness_invocations();
    assert_eq!(invocations.len(), 1);
    let settings_flag_count = invocations[0].matches("--settings").count();
    assert_eq!(settings_flag_count, 1, "argv: {}", invocations[0]);

    let documents = binary.written_settings_documents();
    assert_eq!(documents.len(), 1);
    let text = std::fs::read_to_string(&documents[0]).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();

    // The lifecycle events this build always registers survive beside ours.
    for event in [
        "UserPromptSubmit",
        "PermissionRequest",
        "Stop",
        "StopFailure",
    ] {
        assert!(
            parsed["hooks"][event].is_array(),
            "existing lifecycle hook `{event}` must survive registration: {text}"
        );
    }
    let command = registered_command_line(&documents[0]).unwrap();
    assert!(command.contains("context-firewall hook"));

    assert!(
        binary.user_owned_claude_dirs_are_untouched(),
        "the real user's own ~/.claude and the project's .claude/ must never be written"
    );
    // Never in the user's own home or project — only under Glasshouse's own
    // data directory.
    for document in &binary.written_settings_documents() {
        assert!(document.starts_with(binary.base.join("data")));
    }
}

/// **Map line 1994, and REQUIRED BEHAVIOR's version-floor constant.** An
/// installed Claude Code below the verified floor registers in shadow
/// regardless of the configured mode, and says so on stderr.
#[test]
fn line_1994_a_harness_below_the_floor_falls_back_to_shadow() {
    let binary = Binary::with_config("\n[context_firewall]\nmode = \"safe\"\n", OLD_VERSION);
    let out = binary.glasshouse(&["launch", "claude-code", "--headless"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("shadow mode"),
        "the fallback reason must be stated on stderr: {said}"
    );

    let documents = binary.written_settings_documents();
    let command = registered_command_line(&documents[0]).expect("shadow still registers");
    assert!(command.contains("--mode shadow"), "{command}");
    assert!(!command.contains("--emit-updated-output"), "{command}");
}

/// The same fallback for a `--version` output this build cannot parse at
/// all — treated identically to below-the-floor, never as success.
#[test]
fn an_unparseable_version_also_falls_back_to_shadow() {
    let binary = Binary::with_config(
        "\n[context_firewall]\nmode = \"aggressive\"\n",
        UNPARSEABLE_VERSION,
    );
    let out = binary.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(out.status.success(), "{}", Binary::both_streams(&out));

    let documents = binary.written_settings_documents();
    let command = registered_command_line(&documents[0]).expect("shadow still registers");
    assert!(command.contains("--mode shadow"), "{command}");
}

/// A harness at exactly the verified floor registers in its configured
/// mode, unmodified — the floor is inclusive.
#[test]
fn a_harness_at_the_floor_registers_its_configured_mode() {
    let binary = Binary::with_config("\n[context_firewall]\nmode = \"safe\"\n", GOOD_VERSION);
    let out = binary.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(out.status.success(), "{}", Binary::both_streams(&out));
    let documents = binary.written_settings_documents();
    let command = registered_command_line(&documents[0]).unwrap();
    assert!(command.contains("--mode safe"), "{command}");
    assert!(command.contains("--emit-updated-output"), "{command}");
}

/// **Map line 1996, through the registered path.** Two sessions launched
/// concurrently each get their own session directory and their own
/// registered `PostToolUse` command line — the settings documents never
/// collide, and each one's command line is independently well-formed.
#[test]
fn line_1996_two_concurrent_sessions_never_collide_through_registration() {
    let binary = Binary::with_config("\n[context_firewall]\nmode = \"safe\"\n", GOOD_VERSION);

    let first = std::thread::spawn({
        let base = binary.base.clone();
        let root = binary.root.clone();
        let home = binary.home.clone();
        move || run_headless_launch(&base, &root, &home)
    });
    let second = std::thread::spawn({
        let base = binary.base.clone();
        let root = binary.root.clone();
        let home = binary.home.clone();
        move || run_headless_launch(&base, &root, &home)
    });

    let first_out = first.join().expect("first launch thread");
    let second_out = second.join().expect("second launch thread");
    assert!(
        first_out.status.success(),
        "{}",
        Binary::both_streams(&first_out)
    );
    assert!(
        second_out.status.success(),
        "{}",
        Binary::both_streams(&second_out)
    );

    let documents = binary.written_settings_documents();
    assert_eq!(
        documents.len(),
        2,
        "each session writes its own document: {documents:?}"
    );
    assert_ne!(
        documents[0].parent(),
        documents[1].parent(),
        "the two sessions must live in separate session directories: {documents:?}"
    );
    for document in &documents {
        let command = registered_command_line(document)
            .unwrap_or_else(|| panic!("{document:?} must carry a registered PostToolUse hook"));
        assert!(command.contains("context-firewall hook"));
    }
}

fn run_headless_launch(base: &Path, root: &Path, home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("--scope")
        .arg(root)
        .arg("--data-dir")
        .arg(base.join("data"))
        .arg("--config-dir")
        .arg(base.join("config"))
        .args(["launch", "claude-code", "--headless"])
        .env("HOME", home)
        .env("PATH", base.join("empty-path"))
        .output()
        .expect("the glasshouse binary must be runnable")
}
