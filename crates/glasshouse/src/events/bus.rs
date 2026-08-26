//! Getting a lifecycle event to everyone who needs it, without ever making
//! the harness wait.
//!
//! # The property this file exists for
//!
//! A harness writes into a pseudo-terminal. If the thread draining that
//! terminal ever waits on a Glasshouse consumer, the terminal's buffer fills
//! and the harness itself blocks on `write` — Glasshouse would have stopped
//! the product it exists to host, and it would look like the harness hanging.
//!
//! So publishing is bounded work with no waiting on a consumer at all. Each
//! subscriber owns a fixed-size queue; when it is full the **oldest** event
//! goes and a counter records that it did. A TUI that stops draining loses
//! history and can never apply backpressure. That is the right trade in both
//! directions: recent events are the useful ones, and a consumer that has
//! stopped consuming has stopped mattering.
//!
//! `a_subscriber_that_never_drains_cannot_stall_the_publisher` is the proof,
//! and `a_stalled_subscriber_does_not_stall_a_live_harness` in
//! `tests/events_bus.rs` is the same property against a real child process.
//!
//! # Why the bus keeps its own history as well
//!
//! Phase 45 requires a crashed worker's event history to survive the crash.
//! A subscriber's queue cannot serve that — it is drained, bounded to
//! whatever a viewport needs, and belongs to whoever subscribed. The bus
//! therefore holds its own bounded history, which is what a crash report and
//! [`crate::events::task_outcome`] read.
//!
//! # Poisoning is ownership, not a reason to give up
//!
//! Every lock here is taken through `own`, a private helper. A thread that panicked while
//! holding one leaves the data intact and the lock poisoned; refusing to
//! publish from then on would turn one panic into a permanently deaf event
//! stream. The data is taken and used.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use crate::session::SessionId;
use crate::session::store::Clock;

use super::LifecycleEvent;

/// How many events one bus keeps for the whole project by default.
///
/// Bounded because a Glasshouse left open for a day would otherwise grow
/// without limit, which is the same reason a session's scrollback is bounded.
pub const DEFAULT_HISTORY: usize = 4096;

/// How many events one subscriber may fall behind by before the oldest are
/// dropped.
pub const DEFAULT_SUBSCRIBER_QUEUE: usize = 1024;

/// Take a lock, treating poisoning as ownership.
///
/// See the module docs: a panic elsewhere must not silence the event stream.
fn own<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// One lifecycle event, with the two facts that make it a record rather than
/// a notification.
///
/// There is no way to build one without a session identifier and a timestamp:
/// both are plain fields the bus fills in at publish time, so "record every
/// translated lifecycle event with session ID and timestamp" is a property of
/// the type rather than a habit of its callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedEvent {
    seq: u64,
    session: SessionId,
    at: i64,
    event: LifecycleEvent,
}

impl RecordedEvent {
    /// Rebuild a record that was durably recorded somewhere else.
    ///
    /// Crate-private and deliberately narrow. Its only caller is the durable
    /// log's own reader, bringing an event a *different process* recorded —
    /// a harness reporting through a hook — into this process's consumers,
    /// so that the interface sees one stream rather than two.
    ///
    /// The invariant this type exists for is untouched: there is still no way
    /// to build one without a session identifier and a timestamp, and both
    /// here come from the stored row rather than from a caller's imagination.
    /// **`seq` is the position in whichever stream produced the record** —
    /// this bus's for a published one, the project log's for a rebuilt one —
    /// so it orders a stream and does not identify an event across streams.
    pub(crate) fn from_log(seq: u64, session: SessionId, at: i64, event: LifecycleEvent) -> Self {
        Self {
            seq,
            session,
            at,
            event,
        }
    }

    /// Position in this bus's stream, from 1. Lets a consumer that fell
    /// behind see that it did, and by how much.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The session this happened to.
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    /// Seconds since the Unix epoch, from the bus's clock.
    pub fn at(&self) -> i64 {
        self.at
    }

    pub fn event(&self) -> &LifecycleEvent {
        &self.event
    }
}

/// Somewhere recorded events go to outlive the process.
///
/// The seam Phase 18 attaches to. Nothing in this phase implements it: the
/// project database belongs to another phase, and building half a schema
/// here would be a migration someone else has to undo. A sink must not block
/// — it is called on the publishing thread, which is sometimes the thread
/// draining a pseudo-terminal.
pub trait EventSink: Send + Sync {
    fn record(&self, event: &RecordedEvent);
}

/// A consumer's end of the stream.
///
/// Holds its own queue. Dropping it unsubscribes: the bus holds only a
/// [`Weak`] reference, so a closed viewport stops costing anything without
/// having to say so.
#[derive(Debug)]
pub struct Subscription {
    queue: Arc<Mutex<Queue>>,
}

#[derive(Debug)]
struct Queue {
    events: VecDeque<RecordedEvent>,
    capacity: usize,
    dropped: u64,
}

impl Subscription {
    /// Take everything waiting, oldest first.
    pub fn drain(&self) -> Vec<RecordedEvent> {
        let mut queue = own(&self.queue);
        queue.events.drain(..).collect()
    }

    /// How many events are waiting.
    pub fn pending(&self) -> usize {
        own(&self.queue).events.len()
    }

    /// How many events were dropped because this subscriber fell too far
    /// behind.
    ///
    /// Exposed rather than hidden: a viewport that has lost events should be
    /// able to say so, the same way a truncated scrollback does.
    pub fn dropped(&self) -> u64 {
        own(&self.queue).dropped
    }

    pub fn capacity(&self) -> usize {
        own(&self.queue).capacity
    }
}

/// The one normalized lifecycle-event stream for a project.
///
/// Shared by cloning: every consumer holds the same bus, and publishing is
/// `&self` so a caller holding one immutably can still record.
#[derive(Clone)]
pub struct EventBus {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
    clock: Clock,
}

struct State {
    seq: u64,
    history: VecDeque<RecordedEvent>,
    history_capacity: usize,
    history_dropped: u64,
    subscribers: Vec<Weak<Mutex<Queue>>>,
    sink: Option<Arc<dyn EventSink>>,
}

impl std::fmt::Debug for EventBus {
    /// Hand-written, and it prints no events. A `Debug` that dumped the
    /// stream would put a session's activity into logs and panic messages.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = own(&self.inner.state);
        f.debug_struct("EventBus")
            .field("recorded", &state.seq)
            .field("subscribers", &state.subscribers.len())
            .finish_non_exhaustive()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self::with_history(DEFAULT_HISTORY)
    }

    /// A bus keeping `history` events.
    pub fn with_history(history: usize) -> Self {
        Self::with_history_and_clock(history, Arc::new(system_clock))
    }

    /// [`EventBus::with_history`] with the clock replaced, so a test can
    /// assert on exact timestamps rather than sleeping.
    pub fn with_history_and_clock(history: usize, clock: Clock) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    seq: 0,
                    history: VecDeque::new(),
                    history_capacity: history,
                    history_dropped: 0,
                    subscribers: Vec::new(),
                    sink: None,
                }),
                clock,
            }),
        }
    }

    /// Send everything recorded from now on to `sink` as well.
    ///
    /// One sink, replaced rather than stacked: durable recording has exactly
    /// one destination and a second would mean two orderings of the same
    /// stream.
    pub fn attach_sink(&self, sink: Arc<dyn EventSink>) {
        own(&self.inner.state).sink = Some(sink);
    }

    /// Record an event and hand it to every live subscriber.
    ///
    /// Returns the record, which is what a caller that wants to log or assert
    /// on the timestamp reads. Never waits on a consumer — see the module
    /// docs.
    pub fn publish(&self, session: &SessionId, event: LifecycleEvent) -> RecordedEvent {
        let at = (self.inner.clock)();
        let mut state = own(&self.inner.state);

        state.seq += 1;
        let recorded = RecordedEvent {
            seq: state.seq,
            session: session.clone(),
            at,
            event,
        };

        // Own history first: a subscriber cannot cost the bus its own record.
        let capacity = state.history_capacity;
        while state.history.len() >= capacity && capacity > 0 {
            state.history.pop_front();
            state.history_dropped += 1;
        }
        if capacity > 0 {
            state.history.push_back(recorded.clone());
        }

        // Drop subscribers whose consumer has gone, and push to the rest.
        state.subscribers.retain(|weak| {
            let Some(queue) = weak.upgrade() else {
                return false;
            };
            let mut queue = own(&queue);
            while queue.events.len() >= queue.capacity && queue.capacity > 0 {
                queue.events.pop_front();
                queue.dropped += 1;
            }
            if queue.capacity > 0 {
                queue.events.push_back(recorded.clone());
            } else {
                queue.dropped += 1;
            }
            true
        });

        if let Some(sink) = state.sink.clone() {
            // Outside the borrow of `state.sink` but still under the lock, so
            // a sink sees the same order the history has. A sink that blocks
            // breaks the promise this module makes; its contract says not to.
            drop(state);
            sink.record(&recorded);
        }

        recorded
    }

    /// Start receiving events, keeping at most [`DEFAULT_SUBSCRIBER_QUEUE`]
    /// behind.
    pub fn subscribe(&self) -> Subscription {
        self.subscribe_with_capacity(DEFAULT_SUBSCRIBER_QUEUE)
    }

    /// Start receiving events with a queue of `capacity`.
    pub fn subscribe_with_capacity(&self, capacity: usize) -> Subscription {
        let queue = Arc::new(Mutex::new(Queue {
            events: VecDeque::new(),
            capacity,
            dropped: 0,
        }));
        own(&self.inner.state)
            .subscribers
            .push(Arc::downgrade(&queue));
        Subscription { queue }
    }

    /// How many subscribers are still listening.
    ///
    /// Counts only live ones; a dropped [`Subscription`] is pruned on the
    /// next publish, and this reports the truth before then.
    pub fn subscribers(&self) -> usize {
        own(&self.inner.state)
            .subscribers
            .iter()
            .filter(|weak| weak.strong_count() > 0)
            .count()
    }

    /// Everything recorded, oldest first.
    pub fn history(&self) -> Vec<RecordedEvent> {
        own(&self.inner.state).history.iter().cloned().collect()
    }

    /// Everything recorded for one session, oldest first.
    ///
    /// This is what survives a crash: the bus is not the crashed session's,
    /// so its history outlives the process that produced it.
    pub fn history_for(&self, session: &SessionId) -> Vec<RecordedEvent> {
        own(&self.inner.state)
            .history
            .iter()
            .filter(|recorded| &recorded.session == session)
            .cloned()
            .collect()
    }

    /// How many events fell out of the bus's own history.
    pub fn history_dropped(&self) -> u64 {
        own(&self.inner.state).history_dropped
    }

    /// How many events have been published, ever.
    pub fn recorded(&self) -> u64 {
        own(&self.inner.state).seq
    }
}

/// Seconds since the Unix epoch, saturating rather than panicking on a clock
/// set before 1970 — the same rule the session store uses, and for the same
/// reason: a nonsensical timestamp on one record beats refusing to record.
fn system_clock() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{MessageOrigin, ProcessExit, TurnOutcome};
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    fn id(name: &str) -> SessionId {
        SessionId::new(name)
    }

    /// A clock that starts at `start` and advances by `step` per reading, so a
    /// test can assert exact timestamps instead of sleeping or accepting a
    /// range — the same idiom the session store's tests use.
    fn ticking(start: i64, step: i64) -> Clock {
        let next = AtomicI64::new(start);
        Arc::new(move || next.fetch_add(step, Ordering::SeqCst))
    }

    /// "Record every translated lifecycle event with session ID and
    /// timestamp." Both are read back off the record, from a clock the test
    /// controls, so this cannot pass on a record that stamped zero.
    #[test]
    fn every_recorded_event_carries_its_session_and_a_timestamp() {
        let bus = EventBus::with_history_and_clock(16, ticking(1_700_000_000, 5));
        let alpha = id("alpha");
        let beta = id("beta");

        bus.publish(&alpha, LifecycleEvent::TurnStarted);
        bus.publish(&beta, LifecycleEvent::WaitingForUser);
        bus.publish(
            &alpha,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
        );

        let history = bus.history();
        assert_eq!(history.len(), 3);

        assert_eq!(history[0].session(), &alpha);
        assert_eq!(history[0].at(), 1_700_000_000);
        assert_eq!(history[0].seq(), 1);

        assert_eq!(history[1].session(), &beta);
        assert_eq!(history[1].at(), 1_700_000_005);
        assert_eq!(history[1].seq(), 2);

        assert_eq!(history[2].session(), &alpha);
        assert_eq!(history[2].at(), 1_700_000_010);
        assert_eq!(history[2].seq(), 3);
    }

    /// One session's stream is not another's. An orchestrator asking about a
    /// worker must not be handed a different worker's activity.
    #[test]
    fn one_sessions_history_is_not_mixed_with_another() {
        let bus = EventBus::with_history_and_clock(16, ticking(0, 1));
        let alpha = id("alpha");
        let beta = id("beta");
        bus.publish(&alpha, LifecycleEvent::TurnStarted);
        bus.publish(&beta, LifecycleEvent::TurnStarted);
        bus.publish(&beta, LifecycleEvent::OutputEnded);

        assert_eq!(bus.history_for(&alpha).len(), 1);
        assert_eq!(bus.history_for(&beta).len(), 2);
        assert!(
            bus.history_for(&alpha)
                .iter()
                .all(|recorded| recorded.session() == &alpha)
        );
    }

    /// **The property this whole module exists for.**
    ///
    /// A consumer that has stopped consuming must not be able to make the
    /// publisher wait. The publishing thread is sometimes the thread draining
    /// a pseudo-terminal, and a reader that waits stops draining, which fills
    /// the terminal's buffer, which blocks the harness itself on `write`.
    ///
    /// So: a subscriber with room for eight, a hundred events published, and
    /// nothing ever drained. Every publish returns, the queue never grows past
    /// its bound, the drops are counted, and what is kept is the **newest**
    /// eight — recent output is the part worth having, exactly as in a
    /// session's scrollback.
    #[test]
    fn a_subscriber_that_never_drains_cannot_stall_the_publisher() {
        let bus = EventBus::with_history_and_clock(4096, ticking(0, 1));
        let subscriber = bus.subscribe_with_capacity(8);
        let session = id("busy");

        for index in 0..100u64 {
            bus.publish(
                &session,
                LifecycleEvent::TextDelivered {
                    origin: MessageOrigin::Machine,
                    bytes: index as usize,
                },
            );
        }

        assert_eq!(subscriber.pending(), 8, "never grows past its bound");
        assert_eq!(subscriber.dropped(), 92, "and says how much it lost");

        let kept = subscriber.drain();
        assert_eq!(kept.len(), 8);
        assert_eq!(
            kept.first().map(RecordedEvent::seq),
            Some(93),
            "the oldest go first, so the newest are what is left"
        );
        assert_eq!(kept.last().map(RecordedEvent::seq), Some(100));
        assert_eq!(subscriber.pending(), 0, "draining empties it");
    }

    /// A subscriber that cannot hold anything at all still must not stall the
    /// publisher, and must still be able to say it lost everything.
    #[test]
    fn a_subscriber_with_no_room_loses_events_rather_than_blocking() {
        let bus = EventBus::with_history_and_clock(16, ticking(0, 1));
        let subscriber = bus.subscribe_with_capacity(0);
        let session = id("s");
        bus.publish(&session, LifecycleEvent::TurnStarted);
        bus.publish(&session, LifecycleEvent::OutputEnded);

        assert_eq!(subscriber.pending(), 0);
        assert_eq!(subscriber.dropped(), 2);
        assert_eq!(bus.history().len(), 2, "the bus kept them regardless");
    }

    /// The bus's own history is not a subscriber's queue: it survives with
    /// nobody listening, which is what makes a crashed worker's event history
    /// readable afterwards.
    #[test]
    fn the_bus_keeps_its_own_history_with_nobody_listening() {
        let bus = EventBus::with_history_and_clock(16, ticking(0, 1));
        let session = id("s");
        assert_eq!(bus.subscribers(), 0);
        bus.publish(
            &session,
            LifecycleEvent::ProcessExited {
                exit: ProcessExit::from_status(&crate::pty::ExitStatus::from(
                    portable_pty::ExitStatus::with_exit_code(9),
                )),
            },
        );
        assert_eq!(bus.history_for(&session).len(), 1);
    }

    /// The bus's history is bounded for the same reason a scrollback is: a
    /// Glasshouse left open for a day must not grow without limit.
    #[test]
    fn the_history_is_bounded_and_says_what_it_dropped() {
        let bus = EventBus::with_history_and_clock(4, ticking(0, 1));
        let session = id("s");
        for _ in 0..10 {
            bus.publish(&session, LifecycleEvent::TurnStarted);
        }
        assert_eq!(bus.history().len(), 4);
        assert_eq!(bus.history_dropped(), 6);
        assert_eq!(bus.recorded(), 10, "the count is of everything, ever");
        assert_eq!(bus.history().first().map(RecordedEvent::seq), Some(7));
    }

    /// Dropping a subscription unsubscribes. A closed viewport must stop
    /// costing the bus work without having to remember to say so.
    #[test]
    fn a_dropped_subscription_stops_costing_the_bus() {
        let bus = EventBus::with_history_and_clock(16, ticking(0, 1));
        let session = id("s");
        let kept = bus.subscribe_with_capacity(4);
        {
            let temporary = bus.subscribe_with_capacity(4);
            bus.publish(&session, LifecycleEvent::TurnStarted);
            assert_eq!(temporary.pending(), 1);
            assert_eq!(bus.subscribers(), 2);
        }
        assert_eq!(bus.subscribers(), 1, "the dropped one is gone at once");
        bus.publish(&session, LifecycleEvent::OutputEnded);
        assert_eq!(kept.pending(), 2, "the live one still receives");
    }

    /// The seam Phase 18 attaches to: a sink sees every event, in the order
    /// the history has them.
    #[test]
    fn a_sink_sees_every_event_in_order() {
        struct Recorder {
            seen: Mutex<Vec<u64>>,
            calls: AtomicUsize,
        }
        impl EventSink for Recorder {
            fn record(&self, event: &RecordedEvent) {
                self.calls.fetch_add(1, Ordering::SeqCst);
                own(&self.seen).push(event.seq());
            }
        }

        let bus = EventBus::with_history_and_clock(16, ticking(0, 1));
        let sink = Arc::new(Recorder {
            seen: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
        });
        bus.attach_sink(Arc::clone(&sink) as Arc<dyn EventSink>);

        let session = id("s");
        for _ in 0..3 {
            bus.publish(&session, LifecycleEvent::TurnStarted);
        }

        assert_eq!(sink.calls.load(Ordering::SeqCst), 3);
        assert_eq!(*own(&sink.seen), vec![1, 2, 3]);
    }

    /// A thread that panicked holding a subscriber's lock must not silence
    /// the stream for everyone else. One panic turning the event bus
    /// permanently deaf is a far worse outcome than a gap in one queue.
    #[test]
    fn a_panicked_consumer_does_not_silence_the_stream() {
        let bus = EventBus::with_history_and_clock(16, ticking(0, 1));
        let subscriber = bus.subscribe_with_capacity(4);
        let session = id("s");

        // Poison the subscriber's own lock the way a panicking consumer would.
        let queue = Arc::clone(&subscriber.queue);
        let panicked = std::thread::spawn(move || {
            let _guard = queue.lock().unwrap();
            panic!("a consumer fell over");
        })
        .join();
        assert!(panicked.is_err(), "the helper thread must have panicked");
        assert!(subscriber.queue.is_poisoned());

        bus.publish(&session, LifecycleEvent::TurnStarted);
        assert_eq!(
            subscriber.pending(),
            1,
            "publishing goes on through a poisoned lock"
        );
        assert_eq!(bus.history_for(&session).len(), 1);
    }
}
