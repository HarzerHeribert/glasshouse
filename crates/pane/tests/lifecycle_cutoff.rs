//! Real-process regressions for cell deadlines, task cancellation and SIGTERM.
#![cfg(unix)]
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pane::contract::SessionId;
use pane::glasshouse::Glasshouse;
use pane::runtime::isolate::{DEFAULT_HEAP_LIMIT_BYTES, Runtime};
use pane::runtime::outcome::CellOutcome;
use pane::runtime::preview::Value;
use pane::sandbox::profile::Profile;

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "pane-cutoff-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self {
            root: fs::canonicalize(root).unwrap(),
        }
    }
    fn profile(&self) -> Profile {
        Profile::compile(&self.root, Some(&serde_json::json!({"permissions":{"allow":[
            format!("Read({}/**)",self.root.display()),format!("Write({}/**)",self.root.display()),"Bash".to_string()
        ]}}).to_string()))
    }
    fn command(&self, base: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pane"));
        command
            .args(["session", "--root"])
            .arg(&self.root)
            .args([
                "--task",
                "Verify cutoff",
                "--session",
                "cutoff",
                "--yolo",
                "--rollout",
            ])
            .arg(self.root.join("rollout.jsonl"))
            .arg("--glasshouse")
            .arg(self.root.join("absent"))
            .env("ANTHROPIC_BASE_URL", base)
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("ANTHROPIC_API_KEY")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn wait_started(f: &Fixture, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if f.root.join("started").exists() {
            return;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("Pane exited before its child started: {status}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("child never started");
}
fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("Pane did not finish within the bound");
}
fn code(source: &str) -> String {
    serde_json::json!({"role":"assistant","content":[{"type":"tool_use","id":"cell","name":"execute_cell","input":{"code":source}}]}).to_string()
}
fn provider(first: String) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let captured = bodies.clone();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    return;
                }
                if line == "\r\n" {
                    break;
                }
                if let Some(n) = line.to_lowercase().strip_prefix("content-length:") {
                    length = n.trim().parse::<usize>().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let mut captured = captured.lock().unwrap();
            let index = captured.len();
            captured.push(String::from_utf8(body).unwrap());
            drop(captured);
            let response = if index == 0 {
                first.clone()
            } else {
                code("return 'recovered';")
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
        }
    });
    (address, bodies)
}

#[test]
fn foreground_deadline_kills_late_write_and_remains_recoverable() {
    let command = "echo $$ > started; sleep 3; printf late > marker";
    let f = Fixture::new();
    let mut runtime = Runtime::with_limits(
        &f.profile(),
        &Glasshouse::None,
        &SessionId::new("deadline"),
        DEFAULT_HEAP_LIMIT_BYTES,
        Duration::from_secs(1),
    );
    let began = Instant::now();
    let outcome = runtime.run_cell(&format!(
        "await bash({{command:{}}});",
        serde_json::to_string(command).unwrap()
    ));
    assert!(f.root.join("started").exists(), "fixture did not run");
    assert!(
        matches!(&outcome,CellOutcome::Threw {error,..} if error.class=="RuntimeTimeout"),
        "{outcome:?}"
    );
    assert!(
        began.elapsed() < Duration::from_millis(2500),
        "host ignored the deadline"
    );
    assert!(
        !runtime.poisoned(),
        "a cooperative child poisoned the isolate"
    );
    assert!(matches!(
        runtime.run_cell("return 7;"),
        CellOutcome::Returned {
            value: Value::Number(7.0),
            ..
        }
    ));
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !f.root.join("marker").exists(),
        "a child wrote after timeout"
    );
}

#[test]
fn task_interrupt_kills_late_write_then_recovers_without_poisoning() {
    let f = Fixture::new();
    let (base, bodies) = provider(code(
        "await bash({command:'echo $$ > started; sleep 3; printf late > marker'});",
    ));
    let mut child = f.command(&base).spawn().unwrap();
    wait_started(&f, &mut child);
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(wait_exit(&mut child).success());
    assert_eq!(bodies.lock().unwrap().len(), 2);
    std::thread::sleep(Duration::from_secs(3));
    assert!(!f.root.join("marker").exists());
    let rollout = fs::read_to_string(f.root.join("rollout.jsonl")).unwrap();
    assert!(rollout.contains("Cancelled"));
}

#[test]
fn sigterm_cancels_owned_foreground_group_before_exit() {
    let f = Fixture::new();
    let (base, bodies) = provider(code(
        "await bash({command:'echo $$ > started; sleep 3; printf late > marker'});",
    ));
    let mut child = f.command(&base).spawn().unwrap();
    wait_started(&f, &mut child);
    let began = Instant::now();
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(wait_exit(&mut child).code(), Some(143));
    assert!(began.elapsed() < Duration::from_secs(2));
    assert_eq!(
        bodies.lock().unwrap().len(),
        1,
        "termination started another provider request"
    );
    std::thread::sleep(Duration::from_secs(3));
    assert!(!f.root.join("marker").exists());
}

#[test]
fn poisoned_runtime_ends_incomplete_before_another_model_or_supervisor_request() {
    let f = Fixture::new();
    fs::create_dir_all(f.root.join(".glasshouse")).unwrap();
    fs::write(f.root.join(".glasshouse/pane.toml"),"[limits]\ncell_wall_clock_s = 1\n[supervisor]\nenabled = true\nevery = 1\nmodel = 'test-supervisor'\n").unwrap();
    // A real synchronous hook is an acknowledged remaining uninterruptible
    // host seam. Force it past the hard deadline to prove the session guard,
    // independently of the fixed foreground process polling.
    let hook = f.root.join("slow-hook");
    fs::write(&hook,"#!/bin/sh\nif [ \"$1\" = context-firewall ]; then\n input=$(cat)\n case \"$input\" in *PreToolUse*) sleep 4 ;; esac\nfi\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    let (base, bodies) = provider(code(
        "await bash({command:'printf should-not-run > marker'});",
    ));
    let command = f.command(&base);
    // Override the argument already supplied by the shared helper.
    let args: Vec<_> = command.get_args().map(|s| s.to_os_string()).collect();
    let mut actual = Command::new(env!("CARGO_BIN_EXE_pane"));
    for (index, arg) in args.iter().enumerate() {
        if index > 0 && args[index - 1] == "--glasshouse" {
            actual.arg(&hook);
        } else {
            actual.arg(arg);
        }
    }
    actual
        .env("ANTHROPIC_BASE_URL", base)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = actual.spawn().unwrap();
    let status = wait_exit(&mut child);
    assert!(
        !status.success(),
        "poison fixture did not poison: {}",
        fs::read_to_string(f.root.join("rollout.jsonl")).unwrap_or_default()
    );
    let mut error = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut error)
        .unwrap();
    assert!(error.contains("runtime is poisoned"), "{error}");
    assert_eq!(
        bodies.lock().unwrap().len(),
        1,
        "poison spent further inference"
    );
    assert!(
        !f.root.join("marker").exists(),
        "effect ran after hook exceeded deadline"
    );
}

#[test]
fn successful_foreground_exit_stops_remaining_writers_with_inherited_or_closed_stdio() {
    for command in [
        "echo $$ > started; (sleep 3; printf late > marker) & printf foreground; exit 7",
        "echo $$ > started; (sleep 3; printf late > marker) >/dev/null 2>&1 & printf foreground; exit 7",
    ] {
        let f = Fixture::new();
        let mut runtime = Runtime::with_limits(
            &f.profile(),
            &Glasshouse::None,
            &SessionId::new("detached"),
            DEFAULT_HEAP_LIMIT_BYTES,
            Duration::from_secs(2),
        );
        let outcome = runtime.run_cell(&format!("const result = await bash({{command:{}}}); return result.stdout + ':' + result.exit_code;", serde_json::to_string(command).unwrap()));
        assert!(
            matches!(&outcome, CellOutcome::Returned { value: Value::String(text), .. } if text.head() == "foreground:7"),
            "{outcome:?}"
        );
        assert!(f.root.join("started").exists());
        std::thread::sleep(Duration::from_secs(3));
        assert!(
            !f.root.join("marker").exists(),
            "successful foreground call leaked a writer"
        );
    }
}
