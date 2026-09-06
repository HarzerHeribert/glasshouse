//! Acceptance tests for GH-PANE-61G-WINDOW against
//! `docs/product/pane/events-contract.md` §1, §2, §3, §10.

use pane::events::window::{Accepted, DropReason, Window, WindowConfig};
use pane::events::{Event, Kind, PayloadRef, Priority, Stamp};

fn time_of_day_millis(h: i64, m: i64, s: i64, ms: i64) -> i64 {
    ((h * 60 + m) * 60 + s) * 1_000 + ms
}

fn hook_event(name: &str, source: &str, tool_call_id: &str, at_ms: i64, summary: &str) -> Event {
    Event::pending_hook(
        name,
        source,
        Stamp::from_millis(at_ms),
        PayloadRef::new(format!("payload-{tool_call_id}")),
        Priority::Batch,
        summary,
        tool_call_id,
    )
}

fn ci_cell_event(cell: &str, source: &str, conclusion: &str, at_ms: i64, summary: &str) -> Event {
    Event::pending(
        Kind::CiCell {
            cell: cell.to_string(),
            conclusion: conclusion.to_string(),
        },
        source,
        Stamp::from_millis(at_ms),
        PayloadRef::new(format!("payload-{cell}")),
        Priority::Batch,
        summary,
    )
}

fn message_event(message_id: &str, source: &str, at_ms: i64, summary: &str) -> Event {
    Event::pending(
        Kind::Message {
            message_id: message_id.to_string(),
        },
        source,
        Stamp::from_millis(at_ms),
        PayloadRef::new(format!("payload-{message_id}")),
        Priority::Batch,
        summary,
    )
}

fn worker_report_event(path: &str, source: &str, at_ms: i64, summary: &str) -> Event {
    Event::pending(
        Kind::WorkerReport {
            path: path.to_string(),
            mtime: format!("mtime-{path}"),
        },
        source,
        Stamp::from_millis(at_ms),
        PayloadRef::new(format!("payload-{path}")),
        Priority::Batch,
        summary,
    )
}

fn worker_quiet_event(source: &str, quiet_since: &str, at_ms: i64) -> Event {
    Event::pending(
        Kind::WorkerQuiet {
            quiet_since: quiet_since.to_string(),
        },
        source,
        Stamp::from_millis(at_ms),
        PayloadRef::new("payload-quiet"),
        Priority::Batch,
        "quiet",
    )
}

fn ci_run_interrupt(source: &str, conclusion: &str, at_ms: i64, summary: &str) -> Event {
    Event::pending(
        Kind::CiRun {
            conclusion: conclusion.to_string(),
        },
        source,
        Stamp::from_millis(at_ms),
        PayloadRef::new("payload-ci-run"),
        Priority::Interrupt,
        summary,
    )
}

/// Accepts `event` and asserts it was kept.
fn accept_kept(window: &mut Window, event: Event, now_ms: i64) {
    assert_eq!(
        window.accept(event, Stamp::from_millis(now_ms)),
        Accepted::Kept
    );
}

/// §10's worked turn: 30 `hook.PostToolUse` from one worker's loop, 4
/// `message`, 3 `ci.cell`, 2 `worker.report`, one failing `ci.run` that
/// closes the window 1,204 ms after the first event.
#[test]
fn forty_events_in_one_window_are_one_batch_with_the_interrupt_first() {
    let mut window = Window::new(WindowConfig::default());

    // First accepted event doubles as the window's first-event marker
    // (now = 0) and as sample [4] -- the earliest `hook.PostToolUse`.
    accept_kept(
        &mut window,
        hook_event(
            "PostToolUse",
            "worker/api-routing",
            "tc-0",
            time_of_day_millis(16, 58, 31, 6),
            "Edit routing/mod.rs",
        ),
        0,
    );
    for i in 1..30 {
        accept_kept(
            &mut window,
            hook_event(
                "PostToolUse",
                "worker/api-routing",
                &format!("tc-{i}"),
                time_of_day_millis(16, 58, 31, 6) + i,
                &format!("tool call {i}"),
            ),
            0,
        );
    }

    // 4 message events; the first is sample [3].
    accept_kept(
        &mut window,
        message_event(
            "m1",
            "session/glasshouse-9c",
            time_of_day_millis(16, 58, 33, 402),
            "61G drafted; the primary appends it",
        ),
        0,
    );
    accept_kept(
        &mut window,
        message_event("m2", "session/pane-spec", 0, "noise"),
        0,
    );
    accept_kept(
        &mut window,
        message_event("m3", "session/glasshouse-9c", 0, "noise"),
        0,
    );
    accept_kept(
        &mut window,
        message_event("m4", "session/pane-spec", 0, "noise"),
        0,
    );

    // 3 ci.cell events, all from the same run; the first is sample [2].
    accept_kept(
        &mut window,
        ci_cell_event(
            "ubuntu-24.04 · 1.90.0",
            "github/ci-extended#4471",
            "success",
            time_of_day_millis(16, 58, 36, 744),
            "ubuntu-24.04 · 1.90.0 — success",
        ),
        0,
    );
    accept_kept(
        &mut window,
        ci_cell_event(
            "windows-11-arm · 1.90.0",
            "github/ci-extended#4471",
            "success",
            0,
            "noise",
        ),
        0,
    );
    accept_kept(
        &mut window,
        ci_cell_event(
            "macos-14 · 1.90.0",
            "github/ci-extended#4471",
            "success",
            0,
            "noise",
        ),
        0,
    );

    // 2 worker.report events; pane-events is the oldest, so it is sample
    // [1] and board-watch is the cycled-back sample [5].
    accept_kept(
        &mut window,
        worker_report_event(
            "report-pane-events.md",
            "worker/pane-events",
            time_of_day_millis(16, 58, 39, 881),
            "report-pane-events.md",
        ),
        0,
    );
    accept_kept(
        &mut window,
        worker_report_event(
            "report-board-watch.md",
            "worker/board-watch",
            time_of_day_millis(16, 58, 40, 117),
            "report-board-watch.md",
        ),
        0,
    );

    // The failing ci.run interrupt, 1,204 ms after the first event.
    accept_kept(
        &mut window,
        ci_run_interrupt(
            "github/ci-extended#4471",
            "failure",
            time_of_day_millis(16, 58, 41, 204),
            "failure — 1 of 12 cells failed",
        ),
        1_204,
    );

    assert!(window.is_closed());
    let batch = window.take_batch().expect("window closed on the interrupt");
    assert_eq!(batch.n, 40);

    let rendered = batch.preview(256);
    let golden = include_str!("fixtures/events_view1.golden").replace("\r\n", "\n");
    assert_eq!(rendered, golden.trim_end_matches('\n'));
}

#[test]
fn a_second_arrival_of_one_dedup_key_is_dropped_and_the_first_keeps_its_stamp() {
    let mut window = Window::new(WindowConfig::default());

    let first_at = time_of_day_millis(10, 0, 0, 0);
    accept_kept(
        &mut window,
        ci_cell_event("ubuntu · 1.0", "github/run#1", "success", first_at, "first"),
        0,
    );

    let second = ci_cell_event(
        "ubuntu · 1.0",
        "github/run#1",
        "success",
        first_at + 5_000,
        "second, should be dropped",
    );
    assert_eq!(
        window.accept(second, Stamp::from_millis(0)),
        Accepted::Dropped(DropReason::Dedup)
    );

    accept_kept(
        &mut window,
        ci_run_interrupt("github/run#1", "failure", 0, "close it"),
        10,
    );
    let batch = window.take_batch().unwrap();
    assert_eq!(batch.n, 2, "the duplicate must not have been kept");

    let surviving = batch.where_(Some("ci.cell"), None);
    assert_eq!(surviving.len(), 1);
    assert_eq!(surviving[0].at, Stamp::from_millis(first_at));
    assert_eq!(surviving[0].summary, "first");
}

#[test]
fn worker_quiet_never_displaces_worker_report() {
    let mut window = Window::new(WindowConfig::default());

    // Quiet arriving after the report is dropped outright.
    accept_kept(
        &mut window,
        worker_report_event("report.md", "worker/a", 0, "report"),
        0,
    );
    let quiet_after = worker_quiet_event("worker/a", "q1", 0);
    assert_eq!(
        window.accept(quiet_after, Stamp::from_millis(0)),
        Accepted::Dropped(DropReason::QuietUnderReport)
    );

    // Quiet arriving *before* the report must not survive either -- the
    // reverse reading (dropping the report instead) is the mistake the
    // contract calls out by name.
    accept_kept(&mut window, worker_quiet_event("worker/b", "q2", 0), 0);
    accept_kept(
        &mut window,
        worker_report_event("report-b.md", "worker/b", 0, "report b"),
        0,
    );

    accept_kept(
        &mut window,
        ci_run_interrupt("github/run#1", "failure", 0, "close it"),
        0,
    );
    let batch = window.take_batch().unwrap();

    let reports = batch.where_(Some("worker.report"), None);
    assert_eq!(reports.len(), 2, "both reports must survive");
    let quiets = batch.where_(Some("worker.quiet"), None);
    assert!(
        quiets.is_empty(),
        "no quiet may share a window with that worker's report, either order"
    );
}

#[test]
fn the_window_closes_two_seconds_after_its_first_event_not_its_last() {
    let mut window = Window::new(WindowConfig::default());

    accept_kept(
        &mut window,
        ci_cell_event("a", "github/run#1", "success", 0, "a"),
        0,
    );
    assert!(window.close_if_due(Stamp::from_millis(1_999)).is_none());

    // Two more arrivals inside the window must not push the deadline out.
    accept_kept(
        &mut window,
        ci_cell_event("b", "github/run#1", "success", 0, "b"),
        500,
    );
    accept_kept(
        &mut window,
        ci_cell_event("c", "github/run#1", "success", 0, "c"),
        1_000,
    );
    assert!(
        window.close_if_due(Stamp::from_millis(1_999)).is_none(),
        "1,999 ms after the first event is still short of the 2,000 ms deadline"
    );

    let batch = window
        .close_if_due(Stamp::from_millis(2_000))
        .expect("2,000 ms after the first event, measured from it and not the 1,000 ms arrival");
    assert_eq!(batch.n, 3);
}

#[test]
fn an_interrupt_closes_the_window_at_once() {
    let mut window = Window::new(WindowConfig::default());

    accept_kept(
        &mut window,
        ci_cell_event("a", "github/run#1", "success", 0, "a"),
        0,
    );
    assert!(!window.is_closed());

    accept_kept(
        &mut window,
        ci_run_interrupt("github/run#1", "failure", 0, "boom"),
        50,
    );
    assert!(window.is_closed(), "an interrupt closes the window at once");

    let batch = window.take_batch().unwrap();
    assert_eq!(batch.n, 2);

    // The next accept opens a new window.
    assert!(!window.is_closed());
    accept_kept(
        &mut window,
        ci_cell_event("d", "github/run#2", "success", 0, "d"),
        1_000,
    );
    assert!(!window.is_closed());
    assert!(window.close_if_due(Stamp::from_millis(1_000)).is_none());
}

#[test]
fn a_storm_fills_the_batch_oldest_first_and_spills_the_rest_in_order() {
    let mut window = Window::new(WindowConfig::default());

    for i in 0..250 {
        accept_kept(
            &mut window,
            ci_cell_event(&format!("cell-{i}"), "github/run#1", "success", 0, "storm"),
            0,
        );
    }

    let batch1 = window
        .close_if_due(Stamp::from_millis(2_000))
        .expect("the deadline is due");
    assert_eq!(batch1.n, 200);
    let kept = batch1.rest();
    assert_eq!(kept.len(), 200);
    assert_eq!(first_cell(kept[0]), "cell-0");
    assert_eq!(first_cell(kept[199]), "cell-199");

    // The 50 spilled events lead the next window, still oldest first.
    accept_kept(
        &mut window,
        ci_cell_event("fresh", "github/run#1", "success", 0, "fresh"),
        2_000,
    );
    let batch2 = window
        .close_if_due(Stamp::from_millis(4_000))
        .expect("the second window's own deadline is due");
    assert_eq!(batch2.n, 51);
    let kept2 = batch2.rest();
    assert_eq!(first_cell(kept2[0]), "cell-200");
    assert_eq!(first_cell(kept2[49]), "cell-249");
    assert_eq!(first_cell(kept2[50]), "fresh");
}

fn first_cell(event: &Event) -> &str {
    match &event.kind {
        Kind::CiCell { cell, .. } => cell,
        other => panic!("expected a ci.cell event, got {other:?}"),
    }
}

#[test]
fn unacked_events_roll_with_an_age_and_drop_at_four() {
    let mut window = Window::new(WindowConfig::default());

    accept_kept(
        &mut window,
        ci_cell_event("a", "worker/x", "success", 0, "a"),
        0,
    );
    accept_kept(
        &mut window,
        ci_cell_event("b", "worker/x", "success", 0, "b"),
        0,
    );
    accept_kept(
        &mut window,
        ci_run_interrupt("github/run#1", "failure", 0, "close"),
        0,
    );
    let mut batch = window.take_batch().unwrap();
    let interrupt_id = batch.where_(Some("ci.run"), None)[0].id;
    batch.ack(&[interrupt_id]);

    let rolled = batch.roll();
    assert_eq!(rolled.events.len(), 2);
    assert!(
        rolled.events.iter().all(|(_, age)| *age == 1),
        "the first roll delivers both unacked events at age 1"
    );
    assert!(rolled.dropped.is_empty());

    let mut rolled = rolled;
    for expected_age in [2u32, 3] {
        window.carry_forward(rolled);
        accept_kept(
            &mut window,
            ci_run_interrupt("github/run#1", "failure", 0, "close"),
            0,
        );
        let mut batch = window.take_batch().unwrap();
        let interrupt_id = batch.where_(Some("ci.run"), None)[0].id;
        batch.ack(&[interrupt_id]);
        rolled = batch.roll();
        assert!(
            rolled.events.iter().all(|(_, age)| *age == expected_age),
            "expected age {expected_age}, got {:?}",
            rolled.events.iter().map(|(_, a)| a).collect::<Vec<_>>()
        );
    }

    // The fourth roll would make the age 4: both events are dropped instead.
    window.carry_forward(rolled);
    accept_kept(
        &mut window,
        ci_run_interrupt("github/run#1", "failure", 0, "close"),
        0,
    );
    let mut batch = window.take_batch().unwrap();
    let interrupt_id = batch.where_(Some("ci.run"), None)[0].id;
    batch.ack(&[interrupt_id]);
    let rolled = batch.roll();
    assert!(rolled.events.is_empty(), "age 4 must not roll forward");
    assert_eq!(rolled.dropped.len(), 2, "both must be dropped at age 4");
}

#[test]
fn rolled_events_never_take_more_than_half_the_cap() {
    let config = WindowConfig {
        deadline_ms: 2_000,
        cap: 4,
        drop_age: 4,
    };
    let mut window = Window::new(config);

    accept_kept(
        &mut window,
        ci_cell_event("a", "worker/x", "success", 0, "a"),
        0,
    );
    accept_kept(
        &mut window,
        ci_cell_event("b", "worker/x", "success", 0, "b"),
        0,
    );
    accept_kept(
        &mut window,
        ci_cell_event("c", "worker/x", "success", 0, "c"),
        0,
    );
    accept_kept(
        &mut window,
        ci_run_interrupt("github/run#1", "failure", 0, "close"),
        0,
    );
    let mut batch = window.take_batch().unwrap();
    let interrupt_id = batch.where_(Some("ci.run"), None)[0].id;
    batch.ack(&[interrupt_id]);

    let rolled = batch.roll();
    assert_eq!(
        rolled.events.len(),
        2,
        "half of a cap-4 batch is 2, even though 3 events wanted to roll"
    );
    assert_eq!(rolled.dropped.len(), 1);
}

#[test]
fn where_matches_hooks_by_prefix_and_rest_is_what_was_not_acked() {
    let mut window = Window::new(WindowConfig::default());

    accept_kept(
        &mut window,
        hook_event("PreToolUse", "worker/a", "tc-1", 0, "pre"),
        0,
    );
    accept_kept(
        &mut window,
        hook_event("PostToolUse", "worker/a", "tc-2", 0, "post"),
        0,
    );
    accept_kept(
        &mut window,
        worker_report_event("r1.md", "worker/a", 0, "report one"),
        0,
    );
    accept_kept(
        &mut window,
        worker_report_event("r2.md", "worker/b", 0, "report two"),
        0,
    );
    accept_kept(
        &mut window,
        ci_run_interrupt("github/run#1", "failure", 0, "close"),
        0,
    );
    let mut batch = window.take_batch().unwrap();
    assert_eq!(batch.n, 5);

    let all_hooks = batch.where_(Some("hook.*"), None);
    assert_eq!(all_hooks.len(), 2, "the prefix matches every hook name");

    let only_post = batch.where_(Some("hook.PostToolUse"), None);
    assert_eq!(only_post.len(), 1);

    let mut to_ack: Vec<_> = batch
        .where_(Some("hook.*"), None)
        .into_iter()
        .map(|e| e.id)
        .collect();
    to_ack.extend(batch.where_(Some("ci.run"), None).into_iter().map(|e| e.id));
    to_ack.push(9_999); // an id the batch never held

    let acked = batch.ack(&to_ack);
    assert_eq!(acked.acked.len(), 3);
    assert_eq!(acked.unknown, vec![9_999]);

    let rest = batch.rest();
    assert_eq!(
        rest.len(),
        2,
        "the two worker.report events are still unacked"
    );
    assert!(
        rest.iter()
            .all(|e| matches!(e.kind, Kind::WorkerReport { .. }))
    );
}

#[test]
fn the_preview_shrinks_its_samples_before_cutting_anything_above_them() {
    let mut window = Window::new(WindowConfig::default());

    for i in 0..12 {
        accept_kept(
            &mut window,
            hook_event(
                "PostToolUse",
                "worker/a",
                &format!("tc-{i}"),
                0,
                &format!("a fairly long summary line for tool call number {i}"),
            ),
            0,
        );
    }
    accept_kept(
        &mut window,
        ci_run_interrupt("github/run#1", "failure", 0, "the interrupt summary line"),
        0,
    );
    let batch = window.take_batch().unwrap();

    let roomy = batch.preview(10_000);
    assert!(roomy.contains("[1]"), "a generous cap keeps all 5 samples");
    assert!(roomy.contains("[5]"));

    let tight = batch.preview(1);
    assert!(
        !tight.contains('['),
        "a 1-token cap must shrink samples to zero: {tight}"
    );
    assert!(
        tight.contains("batch  Events.Batch"),
        "the header is never cut: {tight}"
    );
    assert!(
        tight.contains("!  ci.run"),
        "the interrupt is never cut: {tight}"
    );
    assert!(
        tight.contains("hook.PostToolUse"),
        "the counts line is never cut: {tight}"
    );
}
