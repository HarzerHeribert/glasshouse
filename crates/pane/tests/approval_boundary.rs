//! Host-channel seam tests against the real V8 callback stack. These are not
//! interactive-approval acceptance: the shipped TUI does not install the seam,
//! and no decision in this file adds a permission or an OS sandbox grant.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use pane::approval::{Decision, Gate};
use pane::contract::SessionId;
use pane::glasshouse::Glasshouse;
use pane::runtime::isolate::{DEFAULT_HEAP_LIMIT_BYTES, Runtime};
use pane::runtime::outcome::CellOutcome;
use pane::runtime::preview::Value;
use pane::sandbox::profile::Profile;
use pane::tools::invoke::CancellationToken;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "pane-approval-boundary-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        Self(std::fs::canonicalize(root).unwrap())
    }

    fn runtime(&self, settings: Option<&str>) -> Runtime {
        Runtime::new(
            &Profile::compile(&self.0, settings),
            &Glasshouse::None,
            &SessionId::new("approval-boundary"),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn returned(outcome: &CellOutcome, expected: &str) {
    match outcome {
        CellOutcome::Returned { value, .. } => {
            assert_eq!(value, &Value::string(expected));
        }
        other => panic!("expected return {expected:?}, got {other:?}"),
    }
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn once_resumes_the_suspended_call_without_replaying_an_earlier_effect() {
    let fixture = Fixture::new();
    let (gate, requests) = Gate::channel();
    let earlier = fixture.0.join("earlier");
    let target = fixture.0.join("target");
    let observed_earlier = earlier.clone();
    let observed_target = target.clone();
    let responder = std::thread::spawn(move || {
        let first = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(first.action().tool(), "bash");
        assert!(!observed_earlier.exists());
        assert!(first.respond(Decision::AllowOnce));

        let second = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(second.action().tool(), "write");
        assert_eq!(std::fs::read_to_string(&observed_earlier).unwrap(), "x");
        assert!(!observed_target.exists(), "the pending call already ran");
        assert!(second.respond(Decision::AllowOnce));

        let third = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(third.action().tool(), "write");
        assert_eq!(std::fs::read_to_string(&observed_target).unwrap(), "one");
        assert!(third.respond(Decision::Deny));
        requests
    });
    let mut runtime = fixture
        .runtime(Some(
            r#"{"permissions":{"allow":["Bash(printf x >> earlier)"]}}"#,
        ))
        .with_approval_gate(gate.clone());
    let outcome = runtime.run_cell(
        r#"bash({command: "printf x >> earlier"});
           write({path: "target", content: "one"});
           try { write({path: "./target", content: "one"}); }
           catch (e) { return e.name; }
           return "unexpected";"#,
    );
    returned(&outcome, "PermissionDenied");
    assert_eq!(runtime.cell(), 1, "the runtime re-entered the cell");
    assert_eq!(outcome.turn().record.calls.len(), 3);
    assert_eq!(std::fs::read_to_string(earlier).unwrap(), "x");
    assert_eq!(std::fs::read_to_string(target).unwrap(), "one");
    assert!(gate.session_actions().is_empty());
    assert!(responder.join().unwrap().try_recv().is_err());
}

#[test]
fn session_decisions_match_all_canonical_arguments_and_summaries_hide_values() {
    let fixture = Fixture::new();
    let (gate, requests) = Gate::channel();
    let root = fixture.0.clone();
    let responder = std::thread::spawn(move || {
        let first = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            first.action().arguments()["path"],
            root.join("target").to_string_lossy()
        );
        assert!(!format!("{:?}", first.action()).contains("secret-content"));
        assert!(first.respond(Decision::AllowForSession));

        let changed = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(changed.action().arguments()["content"], "different-content");
        assert!(changed.respond(Decision::Deny));
        requests
    });
    let mut runtime = fixture.runtime(None).with_approval_gate(gate.clone());
    let outcome = runtime.run_cell(
        r#"write({path: "target", content: "secret-content"});
           write({path: "./target", content: "secret-content"});
           try { write({path: "target", content: "different-content"}); }
           catch (e) { console.log(e.name); }
           write({path: "./target", content: "secret-content"});
           return "finished";"#,
    );
    returned(&outcome, "finished");
    assert_eq!(outcome.turn().record.calls.len(), 4);
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("target")).unwrap(),
        "secret-content"
    );
    let summaries = gate.session_actions();
    assert_eq!(summaries.len(), 1);
    let fingerprint = summaries[0].strip_prefix("write · exact action ").unwrap();
    assert_eq!(fingerprint.len(), 12);
    assert!(fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(!summaries[0].contains("secret-content"));
    assert!(!summaries[0].contains(fixture.0.to_string_lossy().as_ref()));
    assert!(responder.join().unwrap().try_recv().is_err());
}

#[test]
fn explicit_denies_never_grantable_actions_and_missing_grants_never_reach_the_gate() {
    let fixture = Fixture::new();
    let (gate, requests) = Gate::channel();
    let mut runtime = fixture
        .runtime(Some(
            r#"{"permissions":{"allow":["Bash"],"deny":["Write(blocked)","Bash(echo *)"]}}"#,
        ))
        .with_approval_gate(gate);
    let outcome = runtime.run_cell(
        r#"const refusals = [];
           try { write({path: "blocked", content: "no"}); } catch (e) { refusals.push(e.name); }
           try { write({path: ".claude/settings.json", content: "no"}); } catch (e) { refusals.push(e.name); }
           try { read({path: "~/.ssh/id_ed25519"}); } catch (e) { refusals.push(e.name); }
           try { bash({command: "echo forbidden"}); } catch (e) { refusals.push(e.name); }
           try { bash({command: "bwrap true"}); } catch (e) { refusals.push(e.name); }
           try { read({path: "../missing-grant"}); } catch (e) { refusals.push(e.name); }
           return refusals.join(",");"#,
    );
    returned(&outcome, &["PermissionDenied"; 6].join(","));
    assert!(
        requests.try_recv().is_err(),
        "a refused call reached the host"
    );
    assert!(!fixture.0.join("blocked").exists());
    assert!(!fixture.0.join(".claude/settings.json").exists());
}

#[test]
fn an_unattached_runtime_stays_fail_closed_for_missing_grants() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime(None);
    let outcome = runtime.run_cell(
        r#"try { bash({command: "echo no"}); }
           catch (e) { return e.name; }
           return "unexpected";"#,
    );
    returned(&outcome, "PermissionDenied");
}

#[test]
fn a_disconnected_host_and_a_dropped_request_deny_without_an_effect() {
    for drop_request in [false, true] {
        let fixture = Fixture::new();
        let (gate, requests) = Gate::channel();
        let responder = std::thread::spawn(move || {
            if drop_request {
                drop(requests.recv_timeout(Duration::from_secs(5)).unwrap());
            }
        });
        let mut runtime = fixture.runtime(None).with_approval_gate(gate);
        let outcome = runtime.run_cell(
            r#"try { write({path: "target", content: "no"}); }
               catch (e) { return e.name; }
               return "unexpected";"#,
        );
        returned(&outcome, "PermissionDenied");
        responder.join().unwrap();
        assert!(!fixture.0.join("target").exists());
    }
}

#[test]
fn cancellation_denies_the_pending_call_and_rejects_a_late_session_answer() {
    let fixture = Fixture::new();
    let (gate, requests) = Gate::channel();
    let token = CancellationToken::new();
    let cancelled = token.clone();
    let (finished, completion) = mpsc::channel();
    let responder = std::thread::spawn(move || {
        let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        cancelled.cancel();
        completion.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!request.respond(Decision::AllowForSession));
    });
    let mut runtime = fixture
        .runtime(None)
        .with_token(token)
        .with_approval_gate(gate.clone());
    let outcome = runtime.run_cell(
        r#"try { write({path: "target", content: "no"}); }
           catch (e) { return e.name; }
           return "unexpected";"#,
    );
    returned(&outcome, "PermissionDenied");
    finished.send(()).unwrap();
    responder.join().unwrap();
    assert!(gate.session_actions().is_empty());
    assert!(!fixture.0.join("target").exists());
}

#[test]
fn the_wall_clock_ends_a_blocked_host_callback_without_an_effect() {
    let fixture = Fixture::new();
    let (gate, requests) = Gate::channel();
    let (finished, completion) = mpsc::channel();
    let responder = std::thread::spawn(move || {
        let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        completion.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!request.respond(Decision::AllowOnce));
    });
    let mut runtime = Runtime::with_limits(
        &Profile::compile(&fixture.0, None),
        &Glasshouse::None,
        &SessionId::new("approval-timeout"),
        DEFAULT_HEAP_LIMIT_BYTES,
        Duration::from_millis(100),
    )
    .with_approval_gate(gate);
    let started = Instant::now();
    let outcome = runtime.run_cell(r#"write({path: "target", content: "no"});"#);
    finished.send(()).unwrap();
    responder.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(matches!(outcome, CellOutcome::Threw { .. }), "{outcome:?}");
    assert!(format!("{outcome:?}").contains("RuntimeTimeout"));
    assert!(!runtime.poisoned());
    assert!(!fixture.0.join("target").exists());
}

#[test]
fn an_allow_once_is_consumed_even_when_the_execution_fails() {
    let fixture = Fixture::new();
    let (gate, requests) = Gate::channel();
    let responder = std::thread::spawn(move || {
        let first = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        let action = first.action().clone();
        assert!(first.respond(Decision::AllowOnce));
        let second = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(second.action(), &action);
        assert!(second.respond(Decision::Deny));
    });
    let mut runtime = fixture.runtime(None).with_approval_gate(gate);
    let outcome = runtime.run_cell(
        r#"try { write({path: ".", content: "cannot replace a directory"}); } catch (e) { console.log(e.name); }
           try { write({path: ".", content: "cannot replace a directory"}); }
           catch (e) { return e.name; }
           return "unexpected";"#,
    );
    returned(&outcome, "PermissionDenied");
    responder.join().unwrap();
}

#[test]
fn subagents_do_not_prompt_even_when_a_host_attaches_a_gate() {
    let fixture = Fixture::new();
    let (gate, requests) = Gate::channel();
    let mut runtime = fixture.runtime(None).with_approval_gate(gate).as_subagent();
    let outcome = runtime.run_cell(
        r#"write({path: "target", content: "base profile grants this"});
           try { bash({command: "echo no grant"}); }
           catch (e) { return e.name; }
           return "unexpected";"#,
    );
    returned(&outcome, "PermissionDenied");
    assert!(requests.try_recv().is_err());
    assert!(fixture.0.join("target").exists());
}

#[test]
#[cfg(unix)]
fn a_symlink_retargeted_during_the_wait_invalidates_the_answer() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("original"), "original").unwrap();
    std::fs::write(fixture.0.join("other"), "other").unwrap();
    std::os::unix::fs::symlink("original", fixture.0.join("link")).unwrap();
    let (gate, requests) = Gate::channel();
    let root = fixture.0.clone();
    let responder = std::thread::spawn(move || {
        let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            request.action().arguments()["path"],
            root.join("original").to_string_lossy()
        );
        std::fs::remove_file(root.join("link")).unwrap();
        std::os::unix::fs::symlink("other", root.join("link")).unwrap();
        assert!(request.respond(Decision::AllowOnce));
    });
    let mut runtime = fixture.runtime(None).with_approval_gate(gate);
    let outcome = runtime.run_cell(
        r#"try { write({path: "link", content: "no"}); }
           catch (e) { return e.name; }
           return "unexpected";"#,
    );
    returned(&outcome, "PermissionDenied");
    responder.join().unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("original")).unwrap(),
        "original"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("other")).unwrap(),
        "other"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn a_non_utf8_canonical_path_is_refused_before_a_lossy_key_can_be_approved() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let target = fixture
        .0
        .join(std::ffi::OsString::from_vec(b"private-\xff".to_vec()));
    std::fs::write(&target, "untouched").unwrap();
    std::os::unix::fs::symlink(&target, fixture.0.join("link")).unwrap();
    let (gate, requests) = Gate::channel();
    let mut runtime = fixture.runtime(None).with_approval_gate(gate);
    let outcome = runtime.run_cell(
        r#"try { write({path: "link", content: "no"}); }
           catch (e) { return e.name; }
           return "unexpected";"#,
    );
    returned(&outcome, "PermissionDenied");
    assert!(requests.try_recv().is_err());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "untouched");
}

#[test]
fn remembered_actions_do_not_cross_roots_or_override_a_later_profiles_deny() {
    let first = Fixture::new();
    let second = Fixture::new();
    let (gate, requests) = Gate::channel();
    let first_root = first.0.clone();
    let second_root = second.0.clone();
    let responder = std::thread::spawn(move || {
        let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(request.action().root(), first_root.to_string_lossy());
        assert!(request.respond(Decision::AllowForSession));
        let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(request.action().root(), second_root.to_string_lossy());
        assert!(request.respond(Decision::Deny));
        requests
    });
    let cell = r#"try { write({path: "target", content: "exact"}); }
                  catch (e) { return e.name; }
                  return "written";"#;
    let mut runtime = first.runtime(None).with_approval_gate(gate.clone());
    returned(&runtime.run_cell(cell), "written");
    let mut runtime = second.runtime(None).with_approval_gate(gate.clone());
    returned(&runtime.run_cell(cell), "PermissionDenied");
    let mut runtime = first
        .runtime(Some(r#"{"permissions":{"deny":["Write(target)"]}}"#))
        .with_approval_gate(gate);
    returned(&runtime.run_cell(cell), "PermissionDenied");
    assert!(!second.0.join("target").exists());
    assert!(responder.join().unwrap().try_recv().is_err());
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn answering_the_gate_does_not_expand_the_os_sandbox() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let private = outside.0.join("private");
    std::fs::write(&private, "private contents\n").unwrap();
    let command = format!(
        "if IFS= read -r contents < '{}'; then printf leaked; else printf confined; fi",
        private.display()
    );
    // The same command can read the file without confinement, so the negative
    // half below is an OS restriction, not a missing fixture or a bad command.
    let unconfined = std::process::Command::new("bash")
        .args(["-c", &command])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(unconfined.stdout).unwrap(), "leaked");
    let (gate, requests) = Gate::channel();
    let responder = std::thread::spawn(move || {
        let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(request.action().tool(), "bash");
        assert!(request.respond(Decision::AllowOnce));
    });
    let mut runtime = fixture
        .runtime(Some(r#"{"permissions":{"allow":["Bash"]}}"#))
        .with_approval_gate(gate);
    let outcome = runtime.run_cell(&format!(
        "return bash({{command: {}}}).stdout;",
        serde_json::to_string(&command).unwrap()
    ));
    returned(&outcome, "confined");
    responder.join().unwrap();
}
