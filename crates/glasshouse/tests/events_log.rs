//! The append-only project event log, exercised the way a caller reaches it:
//! through a real `Runtime`, a real `EventBus`, and a real project database on
//! disk.
//!
//! Behavioral contract: given a project with a live event bus, when
//! Glasshouse records lifecycle events, they are appended to the project's
//! own database with their session and timestamp, can be read back after the
//! process that produced them is gone, cannot be rewritten or deleted by
//! anything, and recording them never makes the publishing thread wait.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use clap::Parser;

use glasshouse::events::{
    EventBus, EventLog, EventLogSink, EventSink, GatewayFailure, LifecycleEvent, MessageOrigin,
    Observation, ProcessExit, TurnOutcome,
};
use glasshouse::pty::ExitStatus;
use glasshouse::session::SessionId;
use glasshouse::session::store::Clock;
use glasshouse::{Cli, Runtime};

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots — the same idiom `tests/memory_store.rs` and
/// `src/checkpoint/store.rs`'s own tests use.
struct Fixture {
    base: std::path::PathBuf,
    root: std::path::PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap_at(base, &root);
        Fixture {
            base: base.to_path_buf(),
            root,
            runtime,
        }
    }

    fn sibling(&self, name: &str) -> Runtime {
        let root = self.base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        bootstrap_at(&self.base, &root)
    }

    /// Reopen the project through `bootstrap` again, exactly as a fresh
    /// process launch would — the point of the "survives the process" test.
    fn reopen(&self) -> Runtime {
        bootstrap_at(&self.base, &self.root)
    }
}

fn bootstrap_at(base: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    glasshouse::bootstrap(&cli, root).unwrap()
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn ticking(start: i64, step: i64) -> Clock {
    let next = AtomicI64::new(start);
    Arc::new(move || next.fetch_add(step, Ordering::SeqCst))
}

fn session(name: &str) -> SessionId {
    SessionId::new(name)
}

fn crashy_exit() -> ProcessExit {
    ProcessExit::from_status(&ExitStatus::from(portable_pty::ExitStatus::with_exit_code(
        9,
    )))
}

/// One event of each kind the map lists, so the round trip below actually
/// exercises every payload column rather than just the ones that happen to be
/// easy.
fn every_kind() -> Vec<LifecycleEvent> {
    vec![
        LifecycleEvent::SessionStarted,
        LifecycleEvent::TurnStarted,
        LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
        LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Failed,
        },
        LifecycleEvent::TextDelivered {
            origin: MessageOrigin::UserKeystroke,
            bytes: 7,
        },
        LifecycleEvent::TextDelivered {
            origin: MessageOrigin::Machine,
            bytes: 42,
        },
        LifecycleEvent::InterruptDelivered {
            origin: MessageOrigin::UserKeystroke,
        },
        LifecycleEvent::ProcessExited {
            exit: crashy_exit(),
        },
        LifecycleEvent::OutputEnded,
        LifecycleEvent::GatewayUnhealthy {
            resource: "glasshouse-gateway".to_owned(),
            reason: GatewayFailure::TimedOut,
        },
        LifecycleEvent::GatewayBackendChanged {
            provider: "anthropic".to_owned(),
            model: "claude-sonnet-5".to_owned(),
            cause: "failover".to_owned(),
        },
    ]
}

/// 1. The bus's sink seam actually reaches the database: every event
///    published through a real `EventBus`, with a real `EventLogSink`
///    attached, round-trips to an equal `LifecycleEvent`, in publish order,
///    with the right session and timestamp — read back through a *second*
///    `EventLog`, because the first was moved into the sink's writer thread.
///    That is the proof the row is in the file and not in a process's memory.
#[test]
fn every_published_kind_round_trips_through_the_sink_in_order() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let writer_log = EventLog::open(&fixture.runtime).unwrap();
    let sink = EventLogSink::spawn(writer_log);
    let bus = EventBus::with_history_and_clock(64, ticking(1_700_000_000, 3));
    bus.attach_sink(Arc::clone(&sink) as Arc<dyn EventSink>);

    let alice = session("alice");
    let bob = session("bob");

    let mut expected = Vec::new();
    for (index, event) in every_kind().into_iter().enumerate() {
        let who = if index % 2 == 0 { &alice } else { &bob };
        let recorded = bus.publish(who, event);
        expected.push(recorded);
    }

    assert!(
        sink.flush(Duration::from_secs(5)),
        "the sink must flush everything queued before this point"
    );

    let reader_log = EventLog::open(&fixture.runtime).unwrap();
    let all = reader_log.all().unwrap();
    assert_eq!(all.len(), expected.len());

    for (logged, recorded) in all.iter().zip(expected.iter()) {
        assert_eq!(&logged.session, recorded.session(), "session mismatch");
        assert_eq!(logged.at, recorded.at(), "timestamp mismatch");
        assert_eq!(&logged.event, recorded.event(), "event payload mismatch");
        assert_eq!(logged.observed, None, "nothing here was translated");
    }
}

/// 2. The log survives the process: write, drop everything, reopen the
///    project through `bootstrap` again, and read the same events back.
#[test]
fn the_log_survives_the_process_that_wrote_it() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let who = session("worker");

    {
        let log = EventLog::open(&fixture.runtime).unwrap();
        let sink = EventLogSink::spawn(log);
        let bus = EventBus::with_history_and_clock(16, ticking(0, 1));
        bus.attach_sink(Arc::clone(&sink) as Arc<dyn EventSink>);
        bus.publish(&who, LifecycleEvent::SessionStarted);
        bus.publish(&who, LifecycleEvent::TurnStarted);
        bus.publish(
            &who,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
        );
        assert!(sink.flush(Duration::from_secs(5)));
        // `bus`, `sink` and the underlying `EventLog` all go out of scope
        // here, standing in for the process exiting.
    }

    let reopened = fixture.reopen();
    let log = EventLog::open(&reopened).unwrap();
    let events = log.for_session(&who).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].event, LifecycleEvent::SessionStarted);
    assert_eq!(events[1].event, LifecycleEvent::TurnStarted);
    assert_eq!(
        events[2].event,
        LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Completed
        }
    );
}

/// 3. Append-only, enforced by the file itself: a raw `UPDATE` and a raw
///    `DELETE` against `lifecycle_events` both fail, name `append-only`, and
///    leave the row count unchanged. `is_err()` alone would pass on a typo in
///    the table name, so the count is checked too.
#[test]
fn the_database_refuses_to_update_or_delete_a_logged_event() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let who = session("worker");

    let log = EventLog::open(&fixture.runtime).unwrap();
    log.append(&bus_record(&who, LifecycleEvent::TurnStarted, 100), None)
        .unwrap();
    assert_eq!(log.len().unwrap(), 1);

    let conn = rusqlite::Connection::open(fixture.runtime.database_path()).unwrap();

    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM lifecycle_events", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(before, 1);

    let update_error = conn
        .execute("UPDATE lifecycle_events SET kind = 'output_ended'", [])
        .expect_err("an UPDATE against the event log must fail");
    assert!(
        update_error.to_string().contains("append-only"),
        "unexpected error: {update_error}"
    );

    let delete_error = conn
        .execute("DELETE FROM lifecycle_events", [])
        .expect_err("a DELETE against the event log must fail");
    assert!(
        delete_error.to_string().contains("append-only"),
        "unexpected error: {delete_error}"
    );

    let after: i64 = conn
        .query_row("SELECT COUNT(*) FROM lifecycle_events", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(after, before, "the row count must be unchanged");

    let kind: String = conn
        .query_row("SELECT kind FROM lifecycle_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kind, "turn_started", "the row itself must be unchanged");
}

/// 4. One project's events never appear in another's, even though both live
///    under the same data root.
#[test]
fn one_projects_events_never_appear_in_another_s_log() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let beta = fixture.sibling("beta");

    let alpha_log = EventLog::open(&fixture.runtime).unwrap();
    alpha_log
        .append(
            &bus_record(&session("s"), LifecycleEvent::SessionStarted, 1),
            None,
        )
        .unwrap();

    let beta_log = EventLog::open(&beta).unwrap();
    beta_log
        .append(
            &bus_record(&session("s"), LifecycleEvent::TurnStarted, 1),
            None,
        )
        .unwrap();

    assert_eq!(alpha_log.all().unwrap().len(), 1);
    assert_eq!(
        alpha_log.all().unwrap()[0].event,
        LifecycleEvent::SessionStarted
    );
    assert_eq!(beta_log.all().unwrap().len(), 1);
    assert_eq!(
        beta_log.all().unwrap()[0].event,
        LifecycleEvent::TurnStarted
    );
}

/// 5. A raw harness observation is stored beside the normalized event and
///    stays distinguishable from it: `Some` comes back as `Some`, and `None`
///    comes back as `None`.
#[test]
fn a_raw_observation_is_stored_beside_the_normalized_event_and_stays_distinguishable() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let who = session("worker");
    let log = EventLog::open(&fixture.runtime).unwrap();

    let observation = Observation::new("some-harness", "SomeEvent");
    log.append(
        &bus_record(&who, LifecycleEvent::TurnStarted, 10),
        Some(&observation),
    )
    .unwrap();
    log.append(&bus_record(&who, LifecycleEvent::OutputEnded, 11), None)
        .unwrap();

    let all = log.all().unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].event, LifecycleEvent::TurnStarted);
    assert_eq!(all[0].observed, Some(observation));
    assert_eq!(all[1].event, LifecycleEvent::OutputEnded);
    assert_eq!(all[1].observed, None);
}

/// 6. **The load-bearing test.** The publishing thread is never made to
///    wait: a sink whose writer blocks forever, five thousand events
///    published from a spawned thread, and the test must observe completion
///    within a bound rather than hang — hanging is worse evidence than
///    failing, so a timeout is a `panic!`, never a silent pass.
///
/// Changing `try_send` to `send` in `EventLogSink::record_observed` would
/// make the publishing thread block on the very first event past the queue's
/// capacity, and this test would then time out and panic rather than hang
/// forever, which is exactly the point of using `recv_timeout` here instead
/// of joining the publishing thread directly.
#[test]
fn publishing_never_waits_even_when_the_writer_is_permanently_stuck() {
    // Kept alive for the whole test so the writer's `recv()` blocks forever
    // rather than observing a disconnected channel and returning.
    let (_never_send, forever) = std::sync::mpsc::channel::<()>();

    let sink = EventLogSink::with_writer(8, move |_recorded, _observed| {
        let _ = forever.recv();
    });

    let bus = EventBus::with_history_and_clock(4096, ticking(0, 1));
    bus.attach_sink(Arc::clone(&sink) as Arc<dyn EventSink>);

    const COUNT: u64 = 5_000;
    let (done, is_done) = std::sync::mpsc::channel::<()>();
    let worker = std::thread::spawn(move || {
        let who = session("busy");
        for _ in 0..COUNT {
            bus.publish(&who, LifecycleEvent::OutputEnded);
        }
        let _ = done.send(());
    });

    match is_done.recv_timeout(Duration::from_secs(5)) {
        Ok(()) => {}
        Err(_) => panic!(
            "publishing {COUNT} events hung for more than 5 seconds; the \
             publishing thread must never wait on a stuck writer"
        ),
    }
    worker.join().unwrap();

    assert!(
        sink.dropped() > 0,
        "a permanently stuck writer must cause drops once the bounded queue fills"
    );
    assert_eq!(
        sink.accepted() + sink.dropped(),
        COUNT,
        "every published event is accounted for as either accepted or dropped"
    );
}

/// A small helper standing in for what the bus normally builds: a
/// `RecordedEvent` with a chosen timestamp, used by the tests above that
/// exercise `EventLog::append` directly rather than through a bus.
fn bus_record(
    who: &SessionId,
    event: LifecycleEvent,
    at: i64,
) -> glasshouse::events::RecordedEvent {
    let bus = EventBus::with_history_and_clock(4, Arc::new(move || at));
    bus.publish(who, event)
}
