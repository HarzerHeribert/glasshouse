//! The control-door consumer, without a compile-time Glasshouse dependency.
#![cfg(unix)]
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::{fs::PermissionsExt, net::UnixListener};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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
        fs::write(&script, format!("#!/bin/sh\n[ \"$*\" = 'api socket-path' ] || exit 1\n[ \"$(/bin/pwd -P)\" = '{}' ] || exit 2\nprintf '%s\\n' '{}'\n", root.display(), socket.display())).unwrap();
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
    let server = std::thread::spawn(move || {
        for (after, count) in [(0, 32), (64, 32), (128, 1)] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let req: serde_json::Value = serde_json::from_str(&line).unwrap();
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
            writeln!(
                stream,
                "{}",
                serde_json::json!({"status":"ok","result":{"messages":messages,"head":9999}})
            )
            .unwrap();
        }
    });
    let mut window = Window::new(WindowConfig::default());
    let mut payloads = HashMap::new();
    let before = pane::events::now();
    inbox.drain_into(&sid, &mut window, &mut payloads);
    server.join().unwrap();
    assert_eq!(window.depth(), 65);
    assert_eq!(payloads.len(), 65);
    let batch = window
        .close_if_due(Stamp::from_millis(before.as_millis() + 3_000))
        .unwrap();
    assert_eq!(batch.n, 65);
    assert!(batch.events().iter().all(|(event, _)| event.at >= before));
    assert!(!batch.preview(256).contains("PRIVATE"));
    assert!(inbox.poll(&sid).is_empty()); // dead door does not reset cursor
    fs::remove_file(&f.socket).unwrap();
    let listener = UnixListener::bind(&f.socket).unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let req: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["after"], 130);
            writeln!(
                stream,
                "{{\"status\":\"ok\",\"result\":{{\"messages\":[],\"head\":9999}}}}"
            )
            .unwrap();
        }
    });
    assert!(inbox.poll(&sid).is_empty());
    assert!(inbox.poll(&sid).is_empty());
    server.join().unwrap();
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
        listener.accept().is_err(),
        "invalid arguments had external effects"
    );
    listener.set_nonblocking(false).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        let req: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(req["op"], "send_message");
        assert_eq!(req["from"], "sender");
        assert_eq!(req["origin"], "machine");
        assert_eq!(req["text"], "PRIVATE-雪-🙂");
        listener.set_nonblocking(true).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(listener.accept().is_err(), "ambiguous delivery was retried");
    });
    let outcome = runtime.run_cell("send('recipient', 'PRIVATE-雪-🙂')");
    assert!(matches!(&outcome, CellOutcome::Threw { .. }), "{outcome:?}");
    assert!(format!("{outcome:?}").contains("DeliveryUnknown"));
    if let CellOutcome::Threw { error, .. } = &outcome {
        assert!(!format!("{error:?}").contains("PRIVATE"));
    }
    server.join().unwrap();
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
