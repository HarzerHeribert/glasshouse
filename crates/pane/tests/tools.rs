//! Acceptance for the tool registry and the confined call — map line 2463's
//! per-call half, and the first production caller map line 2455's sandbox has
//! ever had.
//!
//! **Nothing model-authored runs here — map line 2457.** Every program these
//! tests cause to be spawned is one of the platform's own (`cat`, `grep`,
//! `find`, `bash`) resolved from a name fixed in `registry.rs` at compile time,
//! plus one shell script written by this file to stand in for the
//! `glasshouse` binary. There is no generated code and no path from any
//! model output into any of it.
//!
//! And no test here asserts that a sandbox works by relying on the sandbox.
//! The refusal test has a paired unconfined half that runs the *same argv*
//! against the *same file* and must succeed, so a profile that refused
//! everything — or a fixture whose file was never written — fails that half
//! instead of passing quietly.

use pane::contract::SessionId;
use pane::glasshouse::Glasshouse;
// `Access` is only asked for by the resolved-path test, which needs a real
// applier; gating the test without gating its import is a build failure
// under `warnings = "deny"`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
use pane::sandbox::profile::Access;
use pane::sandbox::profile::Profile;
use pane::tools::invoke::{self, Args, ToolContext};
use pane::tools::registry::{self, Purity, Tool};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The registry's own source, for the one claim that is a property of the
/// declaration rather than of any call: purity cannot be omitted.
const REGISTRY_SOURCE: &str = include_str!("../src/tools/registry.rs");

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A throwaway project with a `.claude/`, a file inside the root, and a file
/// outside it. Removed when the test finishes.
struct Fixture {
    root: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let stem = format!("pane-tools-{}-{label}-{n}", std::process::id());
        let root = std::env::temp_dir().join(&stem);
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        let outside = std::env::temp_dir().join(format!("{stem}-outside"));
        std::fs::create_dir_all(&outside).unwrap();
        Self { root, outside }
    }

    fn profile(&self) -> Profile {
        Profile::compile(&self.root, Some(&settings()))
    }

    /// Used only by the tests that put a real file in front of a real
    /// child. Windows has no applier that has ever executed, so those
    /// tests are gated and this helper has no caller there.
    #[cfg(unix)]
    fn write(&self, path: &Path, contents: &str) -> PathBuf {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
        path.to_path_buf()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.outside);
    }
}

/// A settings document that admits one command prefix and nothing outside
/// the project root. `Bash(echo*)` is argv admission and grants no file
/// access at all (`sandbox-grants.md` §2) — which is exactly why it is safe
/// to have in a fixture.
fn settings() -> String {
    r#"{"permissions":{"allow":["Bash(echo*)","Bash(cat*)"]}}"#.to_string()
}

fn context<'a>(
    profile: &'a Profile,
    glasshouse: &'a Glasshouse,
    session: &'a SessionId,
) -> ToolContext<'a> {
    ToolContext {
        profile,
        glasshouse,
        session,
    }
}

// --- the declaration ---------------------------------------------------

/// **Chosen: it does not compile.** A tool with no purity is not rejected at
/// run time, because there is no value to reject — `Tool::declare` takes
/// `Purity` positionally, `Purity` has no `Default`, and `Tool` has no
/// builder and no `Default`. The executable proof is the `compile_fail`
/// doctest on `Tool::declare`, which `cargo test -p pane` runs; this test
/// holds the properties that doctest depends on, so removing them fails here
/// rather than silently turning the doctest into a tautology.
#[test]
fn a_tool_declaring_no_purity_does_not_compile_or_is_rejected() {
    assert!(
        REGISTRY_SOURCE.contains("```compile_fail"),
        "the compile_fail doctest that proves the omission is gone"
    );
    assert!(
        REGISTRY_SOURCE.contains("let _ = Tool::declare(\"read\", \"cat\", &[], Argv::ReadPath);"),
        "the doctest no longer omits the purity argument"
    );
    assert!(
        !REGISTRY_SOURCE.contains("impl Default for Purity"),
        "Purity acquired a Default, so a declaration can now say nothing"
    );
    assert!(
        !REGISTRY_SOURCE.contains("impl Default for Tool"),
        "Tool acquired a Default, so a declaration can now say nothing"
    );
    assert!(
        !REGISTRY_SOURCE.contains("Option<Purity>"),
        "purity became optional"
    );

    // And every tool that exists has actually declared one.
    for tool in registry::ALL {
        assert!(
            matches!(tool.purity(), Purity::Pure | Purity::Effectful),
            "{} declared no purity",
            tool.name()
        );
    }
    assert_eq!(
        registry::lookup("bash").unwrap().purity(),
        Purity::Effectful
    );
    assert_eq!(registry::lookup("read").unwrap().purity(), Purity::Pure);
}

/// `sandbox-grants.md` §4.1: a tool that needs the network is **absent**, not
/// present and failing. Asserted positively against the names the spec calls
/// out and against every declared executable, so adding a `curl` tool under
/// any name fails here.
#[test]
fn no_registered_tool_needs_the_network() {
    let names = registry::names();
    for absent in registry::NEVER_REGISTERED {
        assert!(
            !names.iter().any(|name| name.eq_ignore_ascii_case(absent)),
            "`{absent}` is registered, and §4.1 says it must be absent"
        );
    }
    // `flatten` and not `unwrap`: a tool pane performs itself names no
    // program at all, and "it runs no network binary" is trivially true of
    // one that runs no binary.
    for program in registry::ALL.iter().filter_map(|tool| tool.executable()) {
        let program = Path::new(program)
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        assert!(
            !registry::NETWORK_PROGRAMS
                .iter()
                .any(|network| program == *network),
            "a tool runs `{program}`, which reaches a network"
        );
    }
    assert_eq!(names, vec!["read", "glob", "grep", "bash", "write"]);

    // The profile half of the same clause: no `permissions` pattern can
    // produce a network grant, and the two named MCP network tools are not
    // admitted however the document is written.
    let fixture = Fixture::new("network");
    let profile = fixture.profile();
    assert!(!profile.grants_network());
    assert!(!profile.admits_mcp_tool("WebFetch"));
    assert!(!profile.admits_mcp_tool("WebSearch"));
}

// --- the exec grant ----------------------------------------------------

/// The 61D exec-roots ruling: the child is exec'd on the tool's **resolved**
/// binary, and the platform applier's directory roots are a logged fallback
/// for a name that resolves to nothing.
///
/// The decisive half is the `bash` tool running `echo $0`: a POSIX shell sets
/// `$0` to its own `argv[0]`, so the string the child prints *is* the path
/// pane handed to `execvp`. A bare `bash` would print `bash`.
#[test]
fn exec_is_granted_on_the_resolved_binary() {
    // `registry::READ` declares `cat`, which this half resolves and checks
    // is absolute and canonical. A Windows runner has no `cat` on `PATH` —
    // Git for Windows' `usr\bin` is not on it — so resolution legitimately
    // falls back there rather than exercising what this half is for.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let grant = invoke::exec_grant(registry::lookup("read").unwrap().executable().unwrap());
        assert!(!grant.fell_back_to_roots, "{grant:?}");
        assert!(grant.binary.is_absolute(), "{grant:?}");
        assert_eq!(
            grant.binary,
            std::fs::canonicalize(&grant.binary).unwrap(),
            "the grant is not on a canonical path"
        );
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    eprintln!("skipped: registry::READ's `cat` does not resolve on this runner's PATH");

    let missing = invoke::exec_grant("pane-no-such-program-8f3a1c");
    assert!(
        missing.fell_back_to_roots,
        "an unresolvable name did not report the fallback: {missing:?}"
    );
    assert_eq!(missing.binary, PathBuf::from("pane-no-such-program-8f3a1c"));

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let shell = invoke::exec_grant(registry::lookup("bash").unwrap().executable().unwrap());
        let fixture = Fixture::new("exec-grant");
        let profile = fixture.profile();
        let glasshouse = Glasshouse::None;
        let session = SessionId::new("exec-grant");
        let result = invoke::run(
            &context(&profile, &glasshouse, &session),
            "bash",
            &Args::new().with("command", "echo $0"),
        )
        .expect("the fixture admits `echo`");
        assert_eq!(result.grant, shell);
        assert_eq!(
            result.stdout.trim(),
            shell.binary.to_string_lossy(),
            "the child's argv[0] was not the resolved binary: {result:?}"
        );
    }
}

// --- the confined call -------------------------------------------------

/// Map line 2455 reaching a caller, and `sandbox-grants.md` §1.4.
///
/// Two halves in one test on purpose: the in-root read must **succeed**, so
/// a sandbox that refused everything fails here, and the outside read must
/// come back as a returned refusal rather than a panic, a prompt or an
/// escalation.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_tool_runs_confined_and_a_refusal_is_a_value() {
    let fixture = Fixture::new("confined");
    let inside = fixture.write(&fixture.root.join("inside.txt"), "inside-content\n");
    let outside = fixture.write(&fixture.outside.join("secret.txt"), "outside-secret\n");
    let profile = fixture.profile();
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("confined");
    let ctx = context(&profile, &glasshouse, &session);

    let ok = invoke::run(
        &ctx,
        "read",
        &Args::new().with("path", &*inside.to_string_lossy()),
    )
    .expect("a file inside the project root is readable");
    assert_eq!(ok.stdout, "inside-content\n");
    assert_eq!(ok.exit_code, Some(0));
    #[cfg(target_os = "macos")]
    assert_eq!(ok.confinement, invoke::Confinement::Seatbelt);
    #[cfg(target_os = "linux")]
    assert_eq!(ok.confinement, invoke::Confinement::Landlock);

    let refused = invoke::run(
        &ctx,
        "read",
        &Args::new().with("path", &*outside.to_string_lossy()),
    )
    .expect_err("a file outside every grant is refused");
    let denied = refused.denied().expect("a refusal, not a spawn failure");
    assert_eq!(denied.tool, "read");
    assert!(
        denied
            .rule
            .contains("the project root is the only readable root"),
        "{denied}"
    );
    // The refusal names the resolved path, and it is a value: it is `Display`
    // and it did not end anything — the next call still works.
    assert!(denied.to_string().starts_with("PermissionDenied: read("));
    assert!(
        invoke::run(
            &ctx,
            "read",
            &Args::new().with("path", &*inside.to_string_lossy())
        )
        .is_ok(),
        "a refusal ended the session"
    );

    // The kernel half, and it is the half that proves *confinement* rather
    // than the pre-call check. `Bash(cat*)` admits the command line and
    // grants no file access whatsoever (§2), so nothing in process refuses
    // this call: the child is spawned and the OS layer is the only thing
    // between it and the file. `the_same_tool_reads_the_path_unconfined`
    // runs the same command line without confinement and must read it.
    let escaped = invoke::run(
        &ctx,
        "bash",
        &Args::new().with("command", format!("cat {}", outside.display())),
    )
    .expect("the command line is admitted, so the call reaches a child");
    assert!(
        !escaped.stdout.contains("outside-secret"),
        "a confined child read outside the project root: {escaped:?}"
    );
    assert_ne!(escaped.exit_code, Some(0), "{escaped:?}");
}

/// The paired half of the test above, and the reason it is a separate test
/// with its own name: it runs the **same argv on the same file** without any
/// confinement and without consulting the profile. If this fails, the
/// refusal above proved nothing about the profile — the fixture, the binary
/// or the platform would be the explanation.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn the_same_tool_reads_the_path_unconfined() {
    let fixture = Fixture::new("unconfined-control");
    let outside = fixture.write(&fixture.outside.join("secret.txt"), "outside-secret\n");

    let grant = invoke::exec_grant(registry::lookup("read").unwrap().executable().unwrap());
    let output = std::process::Command::new(&grant.binary)
        .arg("--")
        .arg(&outside)
        .output()
        .expect("the resolved `cat` runs");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "outside-secret\n");

    // And the same for the `bash` tool's command line, whose confined half is
    // the kernel assertion in `a_tool_runs_confined_and_a_refusal_is_a_value`.
    // Without this, a sandbox that refused every exec would pass that one.
    let shell = invoke::exec_grant(registry::lookup("bash").unwrap().executable().unwrap());
    let output = std::process::Command::new(&shell.binary)
        .arg("-c")
        .arg(format!("cat {}", outside.display()))
        .output()
        .expect("the resolved `bash` runs");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "outside-secret\n");
}

/// `Profile::check` returns the resolved path and that is what reaches the
/// child — the defect the 61D compiler's blocker 1 was.
///
/// The spelling is a `..` **after** a symlink, which is the case where the
/// two answers differ: textually `b/lnk/../a/f.txt` is `b/a/f.txt`, which
/// does not exist; resolved, `b/lnk` is `a`, so `..` is the root and the
/// path is `a/f.txt`. A child handed the original spelling exits non-zero
/// with nothing on stdout.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn the_resolved_path_reaches_the_child_not_the_original_spelling() {
    let fixture = Fixture::new("resolved-path");
    fixture.write(&fixture.root.join("a").join("f.txt"), "A-CONTENT\n");
    std::fs::create_dir_all(fixture.root.join("b")).unwrap();
    std::os::unix::fs::symlink(fixture.root.join("a"), fixture.root.join("b").join("lnk")).unwrap();

    let spelling = fixture
        .root
        .join("b")
        .join("lnk")
        .join("..")
        .join("a")
        .join("f.txt");
    let textual = fixture.root.join("b").join("a").join("f.txt");
    assert!(
        !textual.exists(),
        "the fixture no longer distinguishes the two answers"
    );

    let profile = fixture.profile();
    let resolved = profile.check("read", Access::Read, &spelling).unwrap();
    assert_eq!(resolved, profile.root().join("a").join("f.txt"));

    let glasshouse = Glasshouse::None;
    let session = SessionId::new("resolved-path");
    let result = invoke::run(
        &context(&profile, &glasshouse, &session),
        "read",
        &Args::new().with("path", &*spelling.to_string_lossy()),
    )
    .expect("the resolved path is inside the root");
    assert_eq!(result.stdout, "A-CONTENT\n", "{result:?}");
    assert_eq!(result.exit_code, Some(0));
}

// --- the hooks ---------------------------------------------------------

/// A stand-in for the `glasshouse` binary that records every invocation:
/// one `ARGS` line, then the payload it was given on stdin.
#[cfg(unix)]
fn fake_glasshouse(dir: &Path, log: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("fake-glasshouse.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf 'ARGS %s\\n' \"$*\" >> '{log}'\ncat >> '{log}'\nprintf '\\n' >> '{log}'\nexit 0\n",
            log = log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

/// Map line 2463's per-call half: both events fire, once each, and through
/// `context-firewall hook` rather than the lifecycle `hook`.
///
/// The subcommand matters and is not a detail: `PostToolUse` is not in Claude
/// Code's `REPORTED_EVENTS`, so a tool event sent to plain `hook` reaches no
/// consumer at all.
#[cfg(unix)]
#[test]
fn pre_and_post_tool_use_fire_once_per_call_to_the_context_firewall() {
    let fixture = Fixture::new("hooks");
    let inside = fixture.write(&fixture.root.join("inside.txt"), "hook-content\n");
    let log = fixture.root.join("hook.log");
    let script = fake_glasshouse(&fixture.root, &log);

    let profile = fixture.profile();
    let glasshouse = Glasshouse::Command { glasshouse: script };
    let session = SessionId::new("hook-session");
    let ctx = context(&profile, &glasshouse, &session);

    let _ = invoke::run(
        &ctx,
        "read",
        &Args::new().with("path", &*inside.to_string_lossy()),
    );

    let recorded = std::fs::read_to_string(&log).expect("the hook was delivered");
    assert_eq!(
        recorded
            .matches(r#""hook_event_name":"PreToolUse""#)
            .count(),
        1,
        "{recorded}"
    );
    assert_eq!(
        recorded
            .matches(r#""hook_event_name":"PostToolUse""#)
            .count(),
        1,
        "{recorded}"
    );
    assert_eq!(
        recorded
            .lines()
            .filter(|line| line.starts_with("ARGS "))
            .count(),
        2,
        "{recorded}"
    );
    for line in recorded.lines().filter(|line| line.starts_with("ARGS ")) {
        assert_eq!(
            line, "ARGS context-firewall hook --session hook-session",
            "a tool event went somewhere other than the context firewall"
        );
    }
    // The preview is the observed output, and it is what `PostToolUse`
    // carries.
    assert!(recorded.contains("hook-content"), "{recorded}");

    // A refusal is an observed output too: a firewall that only saw
    // successes would report a probing program as having done nothing.
    std::fs::write(&log, "").unwrap();
    let outside = fixture.write(&fixture.outside.join("secret.txt"), "outside-secret\n");
    let _ = invoke::run(
        &ctx,
        "read",
        &Args::new().with("path", &*outside.to_string_lossy()),
    );
    let recorded = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        recorded
            .matches(r#""hook_event_name":"PostToolUse""#)
            .count(),
        1,
        "{recorded}"
    );
    assert!(recorded.contains("PermissionDenied"), "{recorded}");
    assert!(
        !recorded.contains("outside-secret"),
        "the refused file's contents reached a hook: {recorded}"
    );
}

// --- the session -------------------------------------------------------

/// `sandbox-grants.md` §1.5: computed once, at session start, immutable for
/// the session's life.
///
/// `session::compile_profile_once` is the only expression in `pane session`
/// that produces a `Profile`, and it prints as it does so — so a second
/// compilation prints a second line. Two tool calls in one session, one
/// notice.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn the_profile_is_built_once_per_session() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let fixture = Fixture::new("one-profile");
    fixture.write(&fixture.root.join("inside.txt"), "session-content\n");
    std::fs::write(
        fixture.root.join(".claude").join("settings.json"),
        settings(),
    )
    .unwrap();
    let log = fixture.root.join("hook.log");
    let script = fake_glasshouse(&fixture.root, &log);
    let path = fixture.root.join("inside.txt");

    let mut child = Command::new(env!("CARGO_BIN_EXE_pane"))
        .arg("session")
        .arg("--root")
        .arg(&fixture.root)
        .arg("--glasshouse")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("pane starts");
    let line = format!("/tool read path={}\n", path.display());
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{line}{line}").as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(
        stdout
            .matches("profile compiled once for this session")
            .count(),
        1,
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        stdout.matches("/tool read: exit 0 under").count(),
        2,
        "two calls did not both run: {stdout}"
    );
    assert_eq!(
        stdout.matches("session-content").count(),
        2,
        "two calls did not both read the file: {stdout}"
    );
}

/// The tool path is reachable from `main` — `pane session` is the caller,
/// and this asserts it through the shipped binary rather than through the
/// library.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_refusal_reaches_the_binary_as_a_value_and_the_session_continues() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let fixture = Fixture::new("binary-refusal");
    fixture.write(&fixture.root.join("inside.txt"), "still-here\n");
    let outside = fixture.write(&fixture.outside.join("secret.txt"), "outside-secret\n");
    let log = fixture.root.join("hook.log");
    let script = fake_glasshouse(&fixture.root, &log);

    let mut child = Command::new(env!("CARGO_BIN_EXE_pane"))
        .arg("session")
        .arg("--root")
        .arg(&fixture.root)
        .arg("--glasshouse")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("pane starts");
    let input = format!(
        "/tool read path={}\n/tool read path={}\n",
        outside.display(),
        fixture.root.join("inside.txt").display()
    );
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "a refusal ended the session: {output:?}"
    );
    assert!(stdout.contains("PermissionDenied: read("), "{stdout}");
    assert!(!stdout.contains("outside-secret"), "{stdout}");
    assert!(
        stdout.contains("still-here"),
        "the session did not continue after a refusal: {stdout}"
    );
}

/// An unknown tool name is a refusal, not a panic — and no hook fires for it,
/// because there is no tool for the event to name.
#[test]
fn an_unregistered_name_is_a_refusal_and_fires_no_hook() {
    let fixture = Fixture::new("unknown");
    let profile = fixture.profile();
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("unknown");
    let error = invoke::run(
        &context(&profile, &glasshouse, &session),
        "webfetch",
        &Args::new(),
    )
    .expect_err("an unregistered name is refused");
    let denied = error.denied().expect("a refusal");
    assert!(
        denied
            .rule
            .contains("no tool named `webfetch` is registered")
    );
    assert!(
        registry::ALL
            .iter()
            .map(Tool::name)
            .all(|name| name != "webfetch")
    );
}

// --- cancellation ------------------------------------------------------

/// A settings document for the cancellation tests, and the reason it is a
/// second one: the long-running child is built entirely out of `bash`
/// builtins, so its command line has segments the shared [`settings`] does
/// not admit.
///
/// It is a *busy* loop rather than `sleep 30` on purpose, and the purpose is
/// the sandbox: the seatbelt names the one resolved binary in `process-exec*`
/// (the 61D exec-roots ruling), so a confined `bash` cannot exec `/bin/sleep`
/// at all. A builtin loop is the only thing that reliably runs long inside
/// the confinement these tests are supposed to be running inside.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn cancellable_settings() -> String {
    // `Bash(do*)` admits `do`, `done` and the `done &` that backgrounds the
    // loop; `Bash(wait*)` admits the builtin the grandchild fixture waits
    // on. Every pattern here is argv admission and grants no file access
    // whatsoever (§2).
    r#"{"permissions":{"allow":["Bash(echo*)","Bash(while*)","Bash(do*)","Bash(wait*)"]}}"#
        .to_string()
}

/// The command line the two cancellation tests share: write one file, then
/// run until something stops us.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn writes_then_runs_forever(marker: &Path, contents: &str) -> String {
    format!(
        "echo {contents} > {}; while :; do :; done",
        marker.display()
    )
}

/// Decision 8 of the guarded-continuations plan: a call already in flight can
/// be stopped, and stopping it leaves no process behind.
///
/// The three claims, and each has its own assertion: the call **returns**
/// (within two seconds, against a child that would otherwise never exit), it
/// returns `ToolError::Cancelled` rather than a refusal or a result, and the
/// child is **reaped** — `kill -0` on its own recorded pid fails, which a
/// zombie would not do, so this distinguishes `kill` alone from `kill` then
/// `wait`.
///
/// The pid file is also this test's non-vacuity control: it exists only if a
/// child really ran, which is what makes
/// `a_token_set_before_the_call_spawns_nothing` mean anything.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_running_call_is_killed_reaped_and_returned_as_a_throw() {
    let fixture = Fixture::new("cancel-running");
    let pid_file = fixture.root.join("child.pid");
    let profile = Profile::compile(&fixture.root, Some(&cancellable_settings()));
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("cancel-running");
    let ctx = context(&profile, &glasshouse, &session);

    let token = invoke::CancellationToken::new();
    let setter = token.clone();
    let watched = pid_file.clone();
    let canceller = std::thread::spawn(move || {
        // Cancel once the child has recorded its pid, not after a fixed
        // sleep. The fixed sleep was a race: on a loaded machine the child
        // had not reached its `echo` after 100 ms, the call was cancelled
        // before it, and the assertion below failed with `NotFound` on a pid
        // file that was never written.
        let _ = wait_for_pid_file(&watched, std::time::Duration::from_secs(10));
        setter.cancel();
    });

    let started = std::time::Instant::now();
    let error = invoke::run_cancellable(
        &ctx,
        &token,
        "bash",
        &Args::new().with("command", writes_then_runs_forever(&pid_file, "$$")),
    )
    .expect_err("a cancelled call is not a result");
    let elapsed = started.elapsed();
    canceller.join().unwrap();

    assert!(
        matches!(&error, invoke::ToolError::Cancelled { tool } if tool == "bash"),
        "{error:?}"
    );
    assert!(
        error.denied().is_none(),
        "a cancellation reported itself as a permission decision"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the call did not return promptly: {elapsed:?}"
    );

    // The child ran (so the test is not vacuous), and it is gone — reaped,
    // not a zombie, because `kill -0` succeeds against a zombie.
    let pid = std::fs::read_to_string(&pid_file)
        .expect("the child ran and recorded its pid")
        .trim()
        .to_string();
    assert!(pid.parse::<u32>().is_ok(), "not a pid: {pid:?}");
    assert!(
        !alive(&pid),
        "pid {pid} is still addressable after the call returned"
    );
    // And the doc comment's actual promise, which the pid alone does not
    // carry: *no* process behind, not just this one.
    let survivors = reap_survivors(&fixture.root.display().to_string());
    assert!(
        survivors.is_empty(),
        "the cancelled call left processes behind: {survivors:?}"
    );
}

/// A token already set when the call starts: nothing is spawned at all, and
/// both hook events still fire.
///
/// The marker is the assertion that no child ran, and it is credible only
/// because `a_running_call_is_killed_reaped_and_returned_as_a_throw` runs the
/// same command shape and finds its file written. The elapsed bound is the
/// second half of the same claim: the command would never have terminated on
/// its own, so returning in well under a second is not something a spawned
/// child could have produced.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_token_set_before_the_call_spawns_nothing() {
    let fixture = Fixture::new("cancel-before");
    let marker = fixture.root.join("marker.txt");
    let log = fixture.root.join("hook.log");
    let script = fake_glasshouse(&fixture.root, &log);

    let profile = Profile::compile(&fixture.root, Some(&cancellable_settings()));
    let glasshouse = Glasshouse::Command { glasshouse: script };
    let session = SessionId::new("cancel-before");
    let ctx = context(&profile, &glasshouse, &session);

    let token = invoke::CancellationToken::new();
    token.cancel();

    let started = std::time::Instant::now();
    let error = invoke::run_cancellable(
        &ctx,
        &token,
        "bash",
        &Args::new().with("command", writes_then_runs_forever(&marker, "spawned")),
    )
    .expect_err("a cancelled call is not a result");
    let elapsed = started.elapsed();

    assert!(
        matches!(&error, invoke::ToolError::Cancelled { tool } if tool == "bash"),
        "{error:?}"
    );
    assert!(
        !marker.exists(),
        "a child ran under a token that was set before the call"
    );
    // Five seconds, not one: the call's cost here is two executions of the
    // fake `glasshouse` hook script, freshly written for this test, which
    // macOS's Gatekeeper scans on first exec (measured 1.39 s, three times in
    // one evening). The bound only says the call came back promptly; the
    // marker above is the assertion that nothing spawned.
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the call took long enough to have run something: {elapsed:?}"
    );

    // A cancellation is an observed output, exactly as a refusal is: a
    // firewall that saw only the calls that ran would report an abandoned
    // branch as never having been attempted.
    let recorded = std::fs::read_to_string(&log).expect("the hooks were delivered");
    assert_eq!(
        recorded
            .matches(r#""hook_event_name":"PreToolUse""#)
            .count(),
        1,
        "{recorded}"
    );
    assert_eq!(
        recorded
            .matches(r#""hook_event_name":"PostToolUse""#)
            .count(),
        1,
        "{recorded}"
    );
    assert!(recorded.contains("Cancelled"), "{recorded}");
}

/// Whether `pid` is still addressable. A zombie answers yes, which is the
/// point: it distinguishes `kill` alone from `kill` then `wait`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid)
        .output()
        .expect("`kill` runs")
        .status
        .success()
}

/// Polls until `pid` is gone, up to `limit`. Answers whether it went.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn gone_within(pid: &str, limit: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + limit;
    loop {
        if !alive(pid) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Waits for a file the child writes, and answers its trimmed contents.
///
/// The cancellation tests cancel on *this* rather than on a fixed sleep, and
/// that is not a style choice: a fixed 100 ms raced the child on a loaded
/// machine and made
/// `a_running_call_is_killed_reaped_and_returned_as_a_throw` red with
/// `NotFound` on the pid file — a red whose own leaked children then made an
/// unrelated target red. Cancelling once the child has said it is running
/// removes the race and makes the test assert the case it names.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn wait_for_pid_file(path: &Path, limit: std::time::Duration) -> Option<String> {
    let deadline = std::time::Instant::now() + limit;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let text = text.trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Every process whose command line still names `needle`, killed, and their
/// pids answered.
///
/// It kills before the caller asserts on purpose. A fixture here is a busy
/// loop, so a test that asserted first and left the survivors running would
/// saturate the machine it just failed on — which is the exact way one red
/// manufactured the next one.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reap_survivors(needle: &str) -> Vec<String> {
    let listing = std::process::Command::new("ps")
        .args(["-axo", "pid=,command="])
        .output()
        .expect("`ps` runs");
    let listing = String::from_utf8_lossy(&listing.stdout).into_owned();
    let mut found = Vec::new();
    for line in listing.lines() {
        let line = line.trim_start();
        let Some((pid, command)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        // `ps` lists this test's own `ps` invocation too on some hosts; a
        // command line that merely *searches* for the needle is not a
        // survivor of the call.
        if !command.contains(needle) || command.starts_with("ps ") {
            continue;
        }
        let _ = std::process::Command::new("kill")
            .arg("-9")
            .arg(pid)
            .output();
        found.push(format!("{pid} {command}"));
    }
    found
}

/// The command line the grandchild test needs: put the busy loop in a
/// **background job**, record that job's pid, record the shell's own, and
/// wait.
///
/// `$!` is the grandchild — bash forks a subshell for a background job, and
/// its pid is one pane never held a handle to, because `Command::spawn`
/// returns a handle to the shell and to nothing the shell starts. `$$` is
/// the direct child, so one fixture yields both halves of "no process
/// behind".
///
/// The loop is backgrounded with a bare `done &` rather than a `(…) &`
/// subshell for one reason: the profile admits this command line segment by
/// segment, and `(while :` is a segment no `Bash(…)` pattern here matches.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn starts_a_grandchild_then_waits(child_pid: &Path, grandchild_pid: &Path) -> String {
    format!(
        "while :; do :; done & echo $! > {}; echo $$ > {}; wait",
        grandchild_pid.display(),
        child_pid.display()
    )
}

/// Stopping a call leaves **no** process behind — not the child, and not a
/// process the child started.
///
/// [`a_running_call_is_killed_reaped_and_returned_as_a_throw`] proves the
/// direct child dies, and the direct child is the only thing a [`Child`]
/// handle can name. This one runs the loop in a background job of the
/// child's, so its pid is a grandchild no handle reaches: killing the handle
/// alone leaves it spinning at 100% forever, and killing the child's
/// **process group** is what reaches it. A model's `bash` call that starts a
/// server and is then cancelled is exactly this shape, which is why the
/// grandchild is the case that matters rather than an exotic one.
///
/// [`Child`]: std::process::Child
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_cancelled_call_leaves_no_process_behind_not_even_a_grandchild() {
    let fixture = Fixture::new("cancel-grandchild");
    let child_pid = fixture.root.join("child.pid");
    let grandchild_pid = fixture.root.join("grandchild.pid");
    let profile = Profile::compile(&fixture.root, Some(&cancellable_settings()));
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("cancel-grandchild");
    let ctx = context(&profile, &glasshouse, &session);

    let token = invoke::CancellationToken::new();
    let setter = token.clone();
    let watched = grandchild_pid.clone();
    let canceller = std::thread::spawn(move || {
        // Cancel once the grandchild exists, so the call being cancelled is
        // certainly one with a grandchild running. The bound is a
        // liveness guard, not the assertion: the pid files below say
        // whether anything ran.
        let _ = wait_for_pid_file(&watched, std::time::Duration::from_secs(10));
        setter.cancel();
    });

    let error = invoke::run_cancellable(
        &ctx,
        &token,
        "bash",
        &Args::new().with(
            "command",
            starts_a_grandchild_then_waits(&child_pid, &grandchild_pid),
        ),
    )
    .expect_err("a cancelled call is not a result");
    canceller.join().unwrap();

    assert!(
        matches!(&error, invoke::ToolError::Cancelled { tool } if tool == "bash"),
        "{error:?}"
    );

    let child = std::fs::read_to_string(&child_pid)
        .expect("the child ran and recorded its pid")
        .trim()
        .to_string();
    let grandchild = std::fs::read_to_string(&grandchild_pid)
        .expect("the grandchild ran and recorded its pid")
        .trim()
        .to_string();
    assert!(child.parse::<u32>().is_ok(), "not a pid: {child:?}");
    assert!(
        grandchild.parse::<u32>().is_ok(),
        "not a pid: {grandchild:?}"
    );
    assert_ne!(child, grandchild, "the fixture started no grandchild");

    // Both, within one second, and killed before the assertion so a failure
    // does not leave a busy loop running on the machine.
    let child_gone = gone_within(&child, std::time::Duration::from_secs(1));
    let grandchild_gone = gone_within(&grandchild, std::time::Duration::from_secs(1));
    let survivors = reap_survivors(&fixture.root.display().to_string());
    assert!(child_gone, "the child {child} outlived the cancelled call");
    assert!(
        grandchild_gone,
        "the grandchild {grandchild} outlived the cancelled call; survivors: {survivors:?}"
    );
    assert!(
        survivors.is_empty(),
        "the cancelled call left processes behind: {survivors:?}"
    );
}

/// A cancellation that lands in the window between the pre-spawn check and
/// the first poll still kills.
///
/// The pre-spawn check makes "never created" a property of the control flow,
/// and the poll loop covers a child that has been running for a while. The
/// gap between them is the one a real cancellation is most likely to land
/// in, because a caller that cancels at all usually cancels early.
///
/// The delay is **swept** across the rounds rather than fixed, and the sweep
/// is the whole design: a fixed one millisecond loses every race to the
/// pre-spawn check — measured, 50 rounds, not one of them reached a child —
/// so it asserts nothing while looking like it asserts everything. Nought to
/// forty-nine milliseconds brackets a confined spawn on this host (a
/// freshly written fixture is Gatekeeper-scanned before `execve`, so the
/// spawn is milliseconds, not microseconds). The assertion is not *which*
/// side a round fell on, but that no round left anything running — and
/// `ran` is what refuses to let the test pass vacuously if the sweep ever
/// stops bracketing it.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_cancel_that_races_the_spawn_still_kills() {
    let fixture = Fixture::new("cancel-race");
    let profile = Profile::compile(&fixture.root, Some(&cancellable_settings()));
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("cancel-race");
    let ctx = context(&profile, &glasshouse, &session);
    let needle = fixture.root.display().to_string();

    let mut ran = 0_u32;
    for round in 0..50_u64 {
        let child_pid = fixture.root.join(format!("child-{round}.pid"));
        let grandchild_pid = fixture.root.join(format!("grandchild-{round}.pid"));

        let token = invoke::CancellationToken::new();
        let setter = token.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(round));
            setter.cancel();
        });

        let error = invoke::run_cancellable(
            &ctx,
            &token,
            "bash",
            &Args::new().with(
                "command",
                starts_a_grandchild_then_waits(&child_pid, &grandchild_pid),
            ),
        )
        .expect_err("a cancelled call is not a result");
        canceller.join().unwrap();
        assert!(
            matches!(&error, invoke::ToolError::Cancelled { tool } if tool == "bash"),
            "round {round}: {error:?}"
        );

        // Whichever side of the window this round fell on, anything it did
        // start must be gone. A round that spawned nothing writes no pid
        // file and asserts nothing here; `ran` is what says the fifty rounds
        // were not all vacuous.
        for recorded in [&child_pid, &grandchild_pid] {
            if let Some(pid) = wait_for_pid_file(recorded, std::time::Duration::ZERO) {
                ran += 1;
                let gone = gone_within(&pid, std::time::Duration::from_secs(1));
                let survivors = reap_survivors(&needle);
                assert!(
                    gone,
                    "round {round}: pid {pid} outlived the cancelled call; survivors: \
                     {survivors:?}"
                );
            }
        }
    }

    let survivors = reap_survivors(&needle);
    assert!(
        survivors.is_empty(),
        "fifty racing cancellations left processes behind: {survivors:?}"
    );
    // Non-vacuity: if a one-millisecond cancel always beat the spawn, this
    // test would pass without ever having killed anything.
    assert!(
        ran > 0,
        "no round of the sweep got far enough to start a child, so nothing was proved about \
         killing one; the delays no longer bracket a spawn on this host"
    );
}

/// The token changes nothing about a call that finishes: `run` and
/// `run_cancellable` with an unset token produce equal results, field for
/// field.
///
/// This is what makes `run`'s unchanged signature a claim rather than a
/// hope — `run` *is* `run_cancellable` with a token nobody can set, so an
/// inequality here would be an inequality for every existing caller.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_call_that_finishes_first_is_unchanged_by_the_token() {
    let fixture = Fixture::new("cancel-unset");
    let profile = fixture.profile();
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("cancel-unset");
    let ctx = context(&profile, &glasshouse, &session);
    let args = Args::new().with("command", "echo finished-first");

    let plain = invoke::run(&ctx, "bash", &args).expect("the command line is admitted");
    let cancellable =
        invoke::run_cancellable(&ctx, &invoke::CancellationToken::new(), "bash", &args)
            .expect("the command line is admitted");

    assert_eq!(plain.stdout, "finished-first\n");
    assert_eq!(plain, cancellable);
}

/// The drain threads' whole reason to exist, executed: a child that writes
/// more than a pipe buffer (64 KiB on both platforms) to stdout **completes**,
/// and the call returns with all of it. `Command::output` drained both pipes
/// before waiting; `spawn_confined` now waits while two threads drain, and
/// this is the test that would hang if either thread were missing. The call
/// runs on its own thread under a bounded wait, so a deadlock is a failure
/// here and not a hung suite.
///
/// The loop is bash builtins only -- brace expansion, `echo`, `done` -- because
/// the confined `bash` can exec nothing else, which is also why it is `bash`.
/// It is stdout rather than stderr because `>&2` is split at the `&` by the
/// command admission and refused; the drain is one function on either pipe,
/// so the mechanism proven here is the one that serves both.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_call_whose_stdout_exceeds_the_pipe_buffer_completes() {
    let fixture = Fixture::new("large-stdout");
    let profile = Profile::compile(
        &fixture.root,
        Some(r#"{"permissions":{"allow":["Bash(for*)","Bash(do*)"]}}"#),
    );
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("large-stdout");

    let (sender, receiver) = std::sync::mpsc::channel();
    let root = fixture.root.clone();
    std::thread::spawn(move || {
        let profile = profile;
        let ctx = context(&profile, &glasshouse, &session);
        let line = "x".repeat(64);
        let result = invoke::run(
            &ctx,
            "bash",
            &Args::new().with(
                "command",
                format!("for i in {{1..2000}}; do echo {line}; done"),
            ),
        );
        let _ = root;
        let _ = sender.send(result);
    });

    let result = receiver
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the call completed instead of deadlocking on a full stdout pipe")
        .expect("the loop is admitted and runs");
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert!(
        result.stdout.len() > 65_536,
        "the child did not write past a pipe buffer: {} bytes",
        result.stdout.len()
    );
    assert!(result.stderr.is_empty(), "{result:?}");
}

/// `macos::exec_scope` and `linux::exec_scope` decide "resolved" from
/// `Path::is_absolute`, and `invoke::exec_grant` returns an absolute path only
/// for a name it resolved on `PATH` and found runnable. The two agree exactly
/// as long as every declared executable is a bare name: an absolute declared
/// executable that does not resolve would be reported as resolved by the
/// appliers and as fallen-back by the grant (the 61D verifier's finding 4).
/// The registry is where that precondition could break, so it is pinned here.
#[test]
fn every_declared_executable_is_a_bare_name() {
    for tool in registry::ALL.iter() {
        // A tool with no executable has nothing to be a bare name; the
        // precondition this pins is about resolution, and it never resolves.
        let Some(executable) = tool.executable() else {
            continue;
        };
        assert!(
            !executable.contains('/')
                && !executable.contains('\\')
                && !Path::new(executable).is_absolute(),
            "{}: `{executable}` is not a bare name, and exec_scope would disagree with exec_grant about it",
            tool.name()
        );
    }
}

// --- write: the one tool pane performs itself --------------------------

/// The reason `write` exists rather than a `bash` heredoc: a file's contents
/// are arbitrary bytes. This content contains the heredoc delimiter, a `$`, a
/// backtick and a quote — through a shell it would be corrupted or would end
/// the heredoc early; here it round-trips exactly.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn write_puts_arbitrary_bytes_on_disk_exactly() {
    let fixture = Fixture::new("write-exact");
    let profile = Profile::compile(&fixture.root, Some(&write_settings(&fixture.root)));
    let target = fixture.root.join("nested").join("deep").join("out.txt");
    let content = "EOF\n$HOME `whoami` \"quoted\" 'single'\nlast";

    let glasshouse = Glasshouse::None;
    let session = SessionId::new("write-exact");
    let ctx = context(&profile, &glasshouse, &session);
    let result = invoke::run(
        &ctx,
        "write",
        &Args::new()
            .with("path", &*target.to_string_lossy())
            .with("content", content),
    )
    .expect("write should be admitted inside the project");

    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        content,
        "the bytes on disk are not the bytes asked for"
    );
    assert_eq!(
        result.confinement,
        invoke::Confinement::InProcess,
        "write reported a child it never created"
    );
    assert!(result.stdout.contains("wrote"), "{}", result.stdout);
}

/// `WritePath` asks `Profile::check` the *write* question, and this is where
/// the two kinds part company.
///
/// Inside the project root both are granted unconditionally — the root is the
/// workspace (`sandbox-grants.md` §1.3), which is why a `Write(<root>/**)`
/// pattern changes nothing there. **Outside** the root a grant is per-access,
/// so a directory granted for reading alone is readable by `read` and refused
/// to `write`. A `write` declared with `ArgKind::Path` would be allowed here.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn write_is_refused_outside_the_root_where_only_reading_is_granted() {
    let fixture = Fixture::new("write-readonly");
    let outside = fixture.outside.to_string_lossy().replace('\\', "/");
    let profile = Profile::compile(
        &fixture.root,
        Some(&format!(
            r#"{{"permissions":{{"allow":["Read({outside}/**)"]}}}}"#
        )),
    );
    let readable = fixture.outside.join("readable.txt");
    std::fs::write(&readable, "from outside\n").unwrap();

    let glasshouse = Glasshouse::None;
    let session = SessionId::new("write-readonly");
    let ctx = context(&profile, &glasshouse, &session);

    // The grant is real: `read` reaches it.
    invoke::run(
        &ctx,
        "read",
        &Args::new().with("path", &*readable.to_string_lossy()),
    )
    .expect("the read grant should let `read` through");

    // The same path, the other access, refused.
    let error = invoke::run(
        &ctx,
        "write",
        &Args::new()
            .with("path", &*readable.to_string_lossy())
            .with("content", "overwritten"),
    )
    .expect_err("a read-only grant must refuse a write");
    assert!(
        matches!(error, invoke::ToolError::Denied(_)),
        "expected a refusal, got {error:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&readable).unwrap(),
        "from outside\n",
        "the refused write still changed the file"
    );
}

/// Outside the project root is refused exactly as every other tool's path is.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn write_cannot_reach_outside_the_project() {
    let fixture = Fixture::new("write-outside");
    let profile = Profile::compile(&fixture.root, Some(&write_settings(&fixture.root)));
    let target = fixture.outside.join("escaped.txt");

    let glasshouse = Glasshouse::None;
    let session = SessionId::new("write-outside");
    let ctx = context(&profile, &glasshouse, &session);
    let error = invoke::run(
        &ctx,
        "write",
        &Args::new()
            .with("path", &*target.to_string_lossy())
            .with("content", "x"),
    )
    .expect_err("a path outside the root must be refused");
    assert!(matches!(error, invoke::ToolError::Denied(_)), "{error:?}");
    assert!(!target.exists());
}

/// A grant that lets the model write the whole project root.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_settings(root: &Path) -> String {
    let root = root.to_string_lossy().replace('\\', "/");
    format!(r#"{{"permissions":{{"allow":["Read({root}/**)","Write({root}/**)"]}}}}"#)
}

// --- the session's credentials are not the model's ---------------------

/// The policy, as a pure function: pane's own three, and anything whose name
/// ends in a credential word.
#[test]
fn credential_shaped_variable_names_are_recognised() {
    for withheld in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "GITHUB_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
        "SOME_VENDOR_CREDENTIALS",
        "DB_PASSWORD",
    ] {
        assert!(
            invoke::is_credential_variable(withheld),
            "`{withheld}` is a credential and would reach the model"
        );
    }
    // `TOKENIZER` is the one that makes the rule "a whole segment" rather
    // than "a substring": it contains `TOKEN` and is not a credential.
    for kept in [
        "PATH", "HOME", "LANG", "TERM", "CARGO_TARGET_DIR", "TOKENIZER", "KEYBOARD_LAYOUT",
    ] {
        assert!(
            !invoke::is_credential_variable(kept),
            "`{kept}` is not a credential and withholding it breaks tools"
        );
    }
}

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The wiring, through a real confined child: a provider key in the session's
/// environment is not in the child's.
///
/// Measured 2026-09-06, before this: `printenv ANTHROPIC_API_KEY` from inside
/// a cell returned the key, and a cell's output reaches the transcript, the
/// rollout file on disk and every hook payload — an exfiltration path that
/// needs no network at all, which is why denying the network did not cover it.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_confined_child_cannot_read_the_sessions_provider_key() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new("env-scrub");
    let root = fixture.root.to_string_lossy().replace('\\', "/");
    let profile = Profile::compile(
        &fixture.root,
        Some(&format!(
            r#"{{"permissions":{{"allow":["Read({root}/**)","Bash"]}}}}"#
        )),
    );
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("env-scrub");
    let ctx = context(&profile, &glasshouse, &session);

    // SAFETY: `_guard` holds `ENV_LOCK` for the whole test, so no other test
    // in this binary mutates the environment while these are set.
    unsafe {
        std::env::set_var("ANTHROPIC_API_KEY", "pane-test-canary-key");
        std::env::set_var("PANE_TEST_ORDINARY", "pane-test-ordinary-value");
    }
    let result = invoke::run(&ctx, "bash", &Args::new().with("command", "env"));
    unsafe {
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("PANE_TEST_ORDINARY");
    }

    let out = result.expect("`env` should run").stdout;
    assert!(
        !out.contains("pane-test-canary-key"),
        "the provider key reached the model's own command:\n{out}"
    );
    assert!(
        out.contains("pane-test-ordinary-value"),
        "an ordinary variable was withheld too, which breaks every build:\n{out}"
    );
    assert!(out.contains("PATH="), "PATH did not survive:\n{out}");
}
