//! The control-door consumer, without a compile-time Glasshouse dependency.
#![cfg(unix)]
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::{
    fs::PermissionsExt,
    net::{UnixListener, UnixStream},
};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

const FIXTURE_WAIT: Duration = Duration::from_secs(5);

/// Every fixture operation has a deadline and an explicit shutdown path.
/// A failed consumer must never leave libtest joining a blocking accept/read.
struct Server {
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<Result<(), String>>>,
}
impl Server {
    fn start(
        listener: UnixListener,
        expected: usize,
        timeout: Duration,
        answer: impl Fn(usize, serde_json::Value) -> Option<serde_json::Value> + Send + 'static,
    ) -> Self {
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let (ready, started) = mpsc::sync_channel(0);
        let worker = std::thread::spawn(move || {
            ready
                .send(())
                .map_err(|_| "fixture readiness receiver dropped".to_string())?;
            for index in 0..expected {
                let deadline = Instant::now() + timeout;
                let context = format!("request {} of {expected}", index + 1);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            fixture_wait(
                                &cancelled,
                                deadline,
                                &format!("missing {context}: accept"),
                            )?;
                        }
                        Err(error) => return Err(format!("{context}: accept failed: {error}")),
                    }
                };
                stream.set_nonblocking(true).map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                while !bytes.ends_with(b"\n") {
                    match stream.read(&mut buffer) {
                        Ok(0) => return Err(format!("{context}: EOF before request newline")),
                        Ok(count) => bytes.extend_from_slice(&buffer[..count]),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(error) => return Err(format!("{context}: read failed: {error}")),
                    }
                    if bytes.len() > 128 * 1024 {
                        return Err(format!("{context}: fixture request byte limit exceeded"));
                    }
                    if !bytes.ends_with(b"\n") {
                        fixture_wait(&cancelled, deadline, &format!("{context}: read"))?;
                    }
                }
                let request = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("{context}: invalid JSON: {error}"))?;
                if let Some(response) = answer(index, request) {
                    let mut response = serde_json::to_vec(&response).unwrap();
                    response.push(b'\n');
                    let mut remaining = response.as_slice();
                    while !remaining.is_empty() {
                        match stream.write(remaining) {
                            Ok(0) => {
                                return Err(format!("{context}: response write returned zero"));
                            }
                            Ok(count) => remaining = &remaining[count..],
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                fixture_wait(&cancelled, deadline, &format!("{context}: write"))?;
                            }
                            Err(error) => return Err(format!("{context}: write failed: {error}")),
                        }
                    }
                }
                // None deliberately drops the response after accepting the
                // request: this is DeliveryUnknown, not MessagingUnavailable.
            }
            let deadline = Instant::now() + timeout;
            loop {
                match listener.accept() {
                    Ok(_) => {
                        return Err(format!(
                            "unexpected request after {expected}; possible retry"
                        ));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(format!("checking for an extra request: {error}")),
                }
                // Check the queue even after shutdown: the synchronous consumer
                // has returned, so a retry it sent must already be queued.
                if cancelled.load(Ordering::SeqCst) {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err("consumer did not finish within the fixture deadline".into());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        started
            .recv_timeout(FIXTURE_WAIT)
            .expect("fixture server did not become ready");
        Self {
            stop,
            worker: Some(worker),
        }
    }

    fn finish(mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::SeqCst);
        let worker = self.worker.take().unwrap();
        let deadline = Instant::now() + FIXTURE_WAIT;
        while !worker.is_finished() {
            if Instant::now() >= deadline {
                return Err("fixture server did not stop within its join deadline".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        worker.join().map_err(|panic| {
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            format!("fixture assertion failed: {message}")
        })?
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        // Preserve the consumer's original assertion if it unwinds. All socket
        // loops observe this flag; Drop never performs an unconditional join.
        self.stop.store(true, Ordering::SeqCst);
    }
}
fn fixture_wait(stop: &AtomicBool, deadline: Instant, operation: &str) -> Result<(), String> {
    if stop.load(Ordering::SeqCst) {
        return Err(format!("{operation}: fixture shut down"));
    }
    if Instant::now() >= deadline {
        return Err(format!("{operation}: fixture deadline expired"));
    }
    std::thread::sleep(Duration::from_millis(1));
    Ok(())
}

use pane::contract::SessionId;
use pane::events::inbox::{Inbox, Message, send};
use pane::events::window::{Window, WindowConfig};
use pane::events::{Event, Kind, PayloadRef, Priority, Stamp};
use pane::glasshouse::Glasshouse;
use pane::runtime::isolate::Runtime;
use pane::runtime::outcome::CellOutcome;
use pane::runtime::preview::Value;
use pane::sandbox::profile::Profile;

struct Fixture {
    root: PathBuf,
    socket: PathBuf,
    glasshouse: Glasshouse,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "pi-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let socket = root.join("door.sock");
        let script = root.join("glasshouse");
        // Shell builtins validate the real cwd without spawning `/bin/pwd`.
        fs::write(&script, format!("#!/bin/sh\n[ \"$*\" = 'api socket-path' ] || exit 1\ncd -P . || exit 2\n[ \"$PWD\" = '{}' ] || exit 2\nprintf '%s\\n' '{}'\n", root.display(), socket.display())).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            socket,
            glasshouse: Glasshouse::Command { glasshouse: script },
        }
    }
    fn runtime(&self) -> Runtime {
        Runtime::new(
            &Profile::compile(&self.root, None),
            &self.glasshouse,
            &SessionId::new("sender"),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn pages_use_last_returned_sequence_and_survive_absence_restart_and_empty_reads() {
    let f = Fixture::new();
    let sid = SessionId::new("recipient");
    let mut inbox = Inbox::discover(&f.glasshouse, &f.root);
    assert!(inbox.poll(&sid).is_empty()); // discovery must work before the door starts
    let listener = UnixListener::bind(&f.socket).unwrap();
    let server = Server::start(listener, 3, FIXTURE_WAIT, |index, req| {
        let (after, count) = [(0, 32), (64, 32), (128, 1)][index];
        assert_eq!(req["after"], after);
        assert_eq!(req["limit"], 32);
        assert_eq!(req["session"], "recipient");
        let messages: Vec<_> = (1..=count)
            .map(|i| {
                serde_json::json!({
                    "seq": after + i * 2, "from": "sender", "text": "PRIVATE-雪-🙂", "at": 1
                })
            })
            .collect();
        Some(serde_json::json!({"status":"ok","result":{"messages":messages,"head":9999}}))
    });
    let mut window = Window::new(WindowConfig::default());
    let mut payloads = HashMap::new();
    let before = pane::events::now();
    inbox.drain_into(&sid, &mut window, &mut payloads);
    assert_eq!(
        window.depth(),
        65,
        "inbox did not consume the expected pages; discovery or exchange may have failed"
    );
    assert_eq!(payloads.len(), 65);
    server.finish().unwrap();
    let batch = window
        .close_if_due(Stamp::from_millis(before.as_millis() + 3_000))
        .unwrap();
    assert_eq!(batch.n, 65);
    assert!(batch.events().iter().all(|(event, _)| event.at >= before));
    assert!(!batch.preview(256).contains("PRIVATE"));
    assert!(inbox.poll(&sid).is_empty()); // dead door does not reset cursor
    fs::remove_file(&f.socket).unwrap();
    let listener = UnixListener::bind(&f.socket).unwrap();
    let server = Server::start(listener, 2, FIXTURE_WAIT, |_, req| {
        assert_eq!(req["after"], 130);
        Some(serde_json::json!({"status":"ok","result":{"messages":[],"head":9999}}))
    });
    assert!(inbox.poll(&sid).is_empty());
    assert!(inbox.poll(&sid).is_empty());
    server.finish().unwrap();
}

#[test]
fn send_validates_before_effects_preserves_utf8_attribution_and_never_retries_lost_reply() {
    let f = Fixture::new();
    let listener = UnixListener::bind(&f.socket).unwrap();
    let mut runtime = f.runtime();
    for program in [
        "send({}, 'body')",
        "send('', 'body')",
        "send('recipient', '🙂'.repeat(16385))",
    ] {
        assert!(matches!(
            runtime.run_cell(program),
            CellOutcome::Threw { .. }
        ));
    }
    listener.set_nonblocking(true).unwrap();
    assert!(
        listener.accept().unwrap_err().kind() == std::io::ErrorKind::WouldBlock,
        "invalid arguments had external effects"
    );
    let server = Server::start(listener, 1, FIXTURE_WAIT, |_, req| {
        assert_eq!(req["op"], "send_message");
        assert_eq!(req["from"], "sender");
        assert_eq!(req["origin"], "machine");
        assert_eq!(req["text"], "PRIVATE-雪-🙂");
        None
    });
    let outcome = runtime.run_cell("send('recipient', 'PRIVATE-雪-🙂')");
    assert!(matches!(&outcome, CellOutcome::Threw { .. }), "{outcome:?}");
    assert!(
        matches!(&outcome, CellOutcome::Threw { error, .. } if error.class == "DeliveryUnknown"),
        "expected accepted request with lost reply, actual outcome: {outcome:?}"
    );
    if let CellOutcome::Threw { error, .. } = &outcome {
        assert!(!format!("{error:?}").contains("PRIVATE"));
    }
    server.finish().unwrap();
    assert!(
        send(
            &Glasshouse::None,
            &f.root,
            &SessionId::new("sender"),
            "recipient",
            "body"
        )
        .unwrap_err()
        .contains("MessagingUnavailable")
    );
}

#[test]
fn missing_requests_and_unfinished_frames_fail_with_bounded_fixture_diagnostics() {
    for partial_frame in [false, true] {
        let f = Fixture::new();
        let listener = UnixListener::bind(&f.socket).unwrap();
        let server = Server::start(listener, 1, Duration::from_millis(100), |_, _| {
            panic!("an incomplete request must not reach the fixture answer")
        });
        let _peer = partial_frame.then(|| {
            let mut peer = UnixStream::connect(&f.socket).unwrap();
            peer.write_all(b"{\"op\":").unwrap();
            peer
        });
        let deadline = Instant::now() + FIXTURE_WAIT;
        while !server.worker.as_ref().unwrap().is_finished() {
            assert!(
                Instant::now() < deadline,
                "fixture did not bound its accept/read"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let error = server.finish().unwrap_err();
        assert!(error.contains("request 1 of 1"), "{error}");
        assert!(error.contains("deadline expired"), "{error}");
        assert!(
            error.contains(if partial_frame { "read" } else { "missing" }),
            "{error}"
        );
    }
}

#[test]
fn discovery_timeout_sends_nothing_and_never_strands_the_fixture_server() {
    for sending in [false, true] {
        let f = Fixture::new();
        let started = f.root.join("discovery-started");
        // exec replaces the shim, so production's timeout kills the sleeper
        // itself; this fault injection leaves no sleeping descendant behind.
        fs::write(f.root.join("glasshouse"), format!(
            "#!/bin/sh\n[ \"$*\" = 'api socket-path' ] || exit 1\nprintf started > '{}'\nexec /bin/sleep 3\n", started.display()
        )).unwrap();
        let listener = UnixListener::bind(&f.socket).unwrap();
        let server = Server::start(listener, 1, FIXTURE_WAIT, |_, _| {
            panic!("discovery timed out, so no request may have been sent")
        });
        let begin = Instant::now();
        if sending {
            let outcome = f.runtime().run_cell("send('recipient', 'PRIVATE-雪-🙂')");
            assert!(
                matches!(&outcome, CellOutcome::Threw { error, .. } if error.class == "MessagingUnavailable"),
                "{outcome:?}"
            );
        } else {
            let mut inbox = Inbox::discover(&f.glasshouse, &f.root);
            let mut window = Window::new(WindowConfig::default());
            let mut payloads = HashMap::new();
            inbox.drain_into(&SessionId::new("recipient"), &mut window, &mut payloads);
            assert_eq!(window.depth(), 0);
            assert!(payloads.is_empty());
        }
        assert!(started.exists(), "discovery fixture never started");
        let error = server.finish().unwrap_err();
        assert!(error.contains("missing request 1 of 1"), "{error}");
        assert!(
            begin.elapsed() < FIXTURE_WAIT,
            "discovery or fixture shutdown exceeded its bound"
        );
    }
}

fn event(i: i64, at: Stamp) -> Event {
    Event::pending(
        Kind::Message {
            message_id: i.to_string(),
        },
        "session/sender",
        at,
        PayloadRef::new(format!("message/{i}")),
        Priority::Batch,
        "message received",
    )
}

#[test]
fn spill_and_unacked_roll_have_deadlines_without_new_arrivals() {
    let mut window = Window::new(WindowConfig {
        cap: 2,
        ..WindowConfig::default()
    });
    for i in 1..=3 {
        window.accept(event(i, Stamp::from_millis(0)), Stamp::from_millis(0));
    }
    let mut first = window.close_if_due(Stamp::from_millis(2_000)).unwrap();
    first.ack(&[first.events()[1].0.id]);
    assert_eq!(first.rolling_depth(), 1);
    window.carry_forward(first.roll());
    let mut next = window
        .close_if_due(Stamp::from_millis(2_001))
        .expect("spill lost deadline");
    assert_eq!(next.n, 2);
    assert_eq!(
        next.events()[0].1,
        1,
        "unacked event skipped the next batch"
    );
    next.ack(&[next.events()[1].0.id]);
    window.carry_forward(next.roll());
    assert!(window.close_if_due(Stamp::from_millis(3_000)).is_none());
    assert!(
        window.close_if_due(Stamp::from_millis(5_000)).is_some(),
        "unacked-only roll had no deadline"
    );
}

#[test]
fn a_handler_reads_the_message_without_body_preview_or_an_extra_cell() {
    let f = Fixture::new();
    let mut runtime = f.runtime();
    let payloads = Rc::new(RefCell::new(HashMap::from([(
        "message/1".into(),
        Message {
            seq: 1,
            from: Some("sender".into()),
            text: "PRIVATE-雪-🙂".into(),
            at: 1,
        },
    )])));
    runtime.set_message_payloads(payloads);
    let registered = runtime.run_cell(r#"let consumed = 0; const h = on({kind:'message'}, "const p = batch.rest()[0].payload(); if(p.sender === 'sender' && p.body === 'PRIVATE-雪-🙂') consumed++; batch.ack(batch.rest().map(e=>e.id));");"#);
    assert!(matches!(registered, CellOutcome::Yielded { .. }));
    let mut window = Window::new(WindowConfig::default());
    window.accept(event(1, Stamp::from_millis(0)), Stamp::from_millis(0));
    runtime.deliver_batch(window.close_if_due(Stamp::from_millis(2000)).unwrap());
    assert_eq!(runtime.batch_rolling_depth(), 1);
    let runs = runtime.run_handlers();
    assert_eq!(runs.len(), 1);
    assert_eq!(runtime.cell(), 1);
    assert_eq!(runtime.batch_remaining(), 0);
    assert_eq!(runtime.batch_rolling_depth(), 0);
    assert!(!runtime.render_handles().contains("PRIVATE"));
    assert!(runtime.run_handlers().is_empty());
    assert!(matches!(
        runtime.run_cell("return consumed"),
        CellOutcome::Returned {
            value: Value::Number(1.0),
            ..
        }
    ));
}
