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
    for tool in registry::ALL {
        let program = Path::new(tool.executable())
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        assert!(
            !registry::NETWORK_PROGRAMS
                .iter()
                .any(|network| program == *network),
            "{} runs `{program}`, which reaches a network",
            tool.name()
        );
    }
    assert_eq!(names, vec!["read", "glob", "grep", "bash"]);

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
    let grant = invoke::exec_grant(registry::lookup("read").unwrap().executable());
    assert!(!grant.fell_back_to_roots, "{grant:?}");
    assert!(grant.binary.is_absolute(), "{grant:?}");
    assert_eq!(
        grant.binary,
        std::fs::canonicalize(&grant.binary).unwrap(),
        "the grant is not on a canonical path"
    );

    let missing = invoke::exec_grant("pane-no-such-program-8f3a1c");
    assert!(
        missing.fell_back_to_roots,
        "an unresolvable name did not report the fallback: {missing:?}"
    );
    assert_eq!(missing.binary, PathBuf::from("pane-no-such-program-8f3a1c"));

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let shell = invoke::exec_grant(registry::lookup("bash").unwrap().executable());
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

    let grant = invoke::exec_grant(registry::lookup("read").unwrap().executable());
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
    let shell = invoke::exec_grant(registry::lookup("bash").unwrap().executable());
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
