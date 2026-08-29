//! Driving the shipped interface's own event loop, on a real terminal.
//!
//! # The gap this exists for
//!
//! Nothing in this tree drove `shell::run`'s event loop. Measured in batch 50
//! and recorded as a limit in `docs/product/evidence/phase-47.md`, the
//! mutation
//!
//! ```text
//! state.open_route_health(build_route_health_table(runtime));
//!   ->  state.open_route_health(Vec::new());
//! ```
//!
//! **SURVIVED against 275 real tests.** Not a false survivor in any of §80's
//! four senses: the line is on the production path — pressing `h` in the
//! shipped binary reaches it — and no test reached it, because it lives
//! inside the run loop. Every other overlay dispatch arm in that same `match`
//! has the identical hole, which is why this file is a harness with two tests
//! on it rather than one test.
//!
//! This is practice §35 one layer further out. There, a caller every test
//! bypassed was not a caller; here, a whole `match` arm that no test can
//! reach is not code anything is watching. The in-crate tests prove
//! `build_route_health_table` reads the caches and `render_route_health`
//! draws five concepts; nothing proved the run loop connects the two.
//!
//! # How a key gets in, and how the screen gets read
//!
//! The technique is `tests/terminal_loss.rs`'s and deliberately not a second
//! one: `portable_pty` opens a real pty, the shipped binary is spawned on the
//! slave, and the master is drained non-blocking on this thread with no
//! cloned reader. That file's own comment says why there is no reader thread,
//! and it applies here too even though nothing here closes a terminal.
//!
//! What is new is the reading. `terminal_loss` matches against the byte
//! stream with the escape sequences stripped out, which is the right
//! instrument for the question it asks (*"did it lay itself out at the new
//! size?"* is carried by the cursor moves, not by the text). It is the wrong
//! instrument here. Ratatui redraws by diffing, so pressing a key emits only
//! the cells that changed, in runs separated by cursor moves; strip those and
//! two unrelated rows concatenate. So this file feeds every byte to a real
//! terminal emulator — `vt100`, which this crate already depends on and
//! already uses to read a session's screen — and asserts against
//! [`vt100::Screen::rows`], **one rendered row at a time**. An assertion here
//! is about what a user would see on a given line, which is also what makes
//! "these two things are not on the same line" expressible at all.
//!
//! # The ready signal, which is not a sleep (§38)
//!
//! `shutdown::TerminalGuard::acquire` enables raw mode *before* it touches
//! the alternate screen, `tui::Screen::acquire` calls it before building the
//! Ratatui terminal, and `shell::run` draws its first frame after that. So
//! **the version banner appearing on the emulated screen proves raw mode is
//! already on and the event source already exists**, and a byte written to
//! the master after that point is read by the loop rather than eaten by a
//! line discipline. That is a causal signal, not a delay: nothing here
//! sleeps for a fixed period, and no assertion depends on how fast the
//! machine is.
//!
//! # Why this is not timing-dependent, where `terminal_loss`'s tests are
//!
//! Practice §60: a single-trial test cannot tell "works" from "works most of
//! the time". That applies to a *race* — and the tests below are not sampling
//! one. A keystroke that reaches nothing stays unreached, because this file
//! types nothing after it; no amount of further waiting can convert a failure
//! into a pass. The deadlines therefore bound how long a **failure** takes to
//! be reported and never decide whether one happened, which is the same
//! reasoning `terminal_loss::KEYSTROKE_DEADLINE` carries.
//!
//! Measured anyway rather than argued: the rate is in
//! `.agent-runtime/report-gh-tui-harness.md`.
//!
//! # Two stages per test, because of §80 case 5
//!
//! A mutation that empties a table must fail the assertion the test is *named*
//! for, not the fixture's own timeout. So each test waits twice:
//!
//! 1. for the overlay's own border title — the fixture's readiness check,
//!    which says the key arrived and the arm ran;
//! 2. for **either** the seeded value **or** the view's own empty-state
//!    sentence, whichever the interface drew.
//!
//! On the mutated tree stage 1 still succeeds (the overlay opens, empty), and
//! stage 2 finds the empty-state sentence and fails immediately, quoting it.
//! The evidence is the assertion, not the clock.
//!
//! # Unix only, and said out loud (§18)
//!
//! `#![cfg(unix)]`, for two reasons rather than one. `terminal_loss.rs` is
//! Unix-only because Windows has no hangup path; this file's reason is
//! different and independent: `portable_pty` on Windows is ConPTY, which is a
//! screen buffer rather than a byte pipe (practice §21), so the master does
//! not carry a stream a `vt100::Parser` can reconstruct a Ratatui frame from.
//! Making these run on Windows is a separate piece of work with its own
//! reading technique, not a `cfg` to widen.
//!
//! # Adding a contract to this file
//!
//! [`Shell`] is the reusable part and the tests are ~15 lines each: seed
//! whatever the arm reads, `start`, `wait_for_first_frame`, `press`, two
//! waits. It is one file because Rust gives each `tests/*.rs` its own crate,
//! so sharing this across files needs a `tests/` module directory — a change
//! this package was not scoped to make, and one that would want
//! `terminal_loss.rs` moved onto it too.

#![cfg(unix)]

use std::os::fd::RawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// How long the interface gets to draw its first frame.
///
/// Generous for the same reason `terminal_loss::STARTUP_TIMEOUT` is: a loaded
/// container starting a debug build and migrating a fresh SQLite database is
/// slow, and being slow to start is not what any test here is about.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a key gets to reach the interface and change the screen.
///
/// An honest budget is one 16ms tick. This is three hundred of them, and the
/// slack is free: a keystroke the loop never saw is never answered, so
/// waiting longer cannot turn a failure into a pass. It only bounds how long
/// a real failure takes to be reported.
const RESPONSE_DEADLINE: Duration = Duration::from_secs(10);

/// How long `Shell::kill` keeps draining and reaping before giving up.
/// `terminal_loss::KILL_DEADLINE`'s own reasoning, unchanged.
const KILL_DEADLINE: Duration = Duration::from_secs(10);

const READ_POLL: Duration = Duration::from_millis(10);

/// The two terminal sizes every assertion below is made at — practice §17.
///
/// The first is a terminal a person would actually have. The second is wide
/// enough that nothing can be clipped, which is what tells a match that
/// survives truncation from one that depends on it: a row this file looks for
/// and cannot find at 120 columns but finds at 220 is a layout finding, not a
/// dispatch failure, and the failure message says which width it was.
const SIZES: [(u16, u16); 2] = [(40, 120), (48, 220)];

/// The provider, model and credential the route-health fixture seeds.
///
/// A model name no catalogue contains, on purpose: the assertion has to be
/// about *this* fixture's reading crossing the process boundary, and a real
/// model identifier could in principle be drawn by something else.
const HEALTH_PROVIDER: &str = "anyrouter";
const HEALTH_MODEL: &str = "probe-model-h";
const HEALTH_CREDENTIAL: &str = "anyrouter/ANYROUTER_API_KEY";

/// What the provider stated about its own rate limit, seeded through
/// `GatewayQuotaCache` — the *second* cache `build_route_health_table` reads.
/// Asserting on this as well as on the model proves both loads crossed, not
/// merely that the table came back non-empty.
const HEALTH_STATED_LIMIT: i64 = 300;
const HEALTH_STATED_WINDOW: i64 = 60;

/// The identity the route-evidence fixture records in the ledger.
const EVIDENCE_PROVIDER: &str = "groq";
const EVIDENCE_MODEL: &str = "probe-model-r";
const EVIDENCE_ROUTE: &str = "probe-route";

/// `shell::view::render_route_health`'s own empty state, and
/// `render_route_evidence`'s. These are what an emptied table draws, and each
/// test waits for its own alongside the value it wants — see the header's
/// note on §80 case 5.
const HEALTH_EMPTY: &str = "no gateway exchange has been observed for any resource yet";
const EVIDENCE_EMPTY: &str = "no routing evidence recorded yet";

/// What the cadence line reads when the quota cache said nothing, and what
/// the route column reads when the observation recorded no route. Each is the
/// other half of a two-needle wait: without it the corresponding assertion
/// would report its failure as a timeout, which is §80's fifth case.
const HEALTH_CADENCE_UNSTATED: &str = "provider stated: unknown";
const EVIDENCE_NO_ROUTE: &str = "(no route)";

/// Phase 47 line 1765's dispatch arm, driven by a key on a real terminal.
///
/// # What this proves that no in-crate test could
///
/// `shell::route_health_tests` proves `build_route_health_table` reads both
/// gateway caches; `shell::view::tests` and `tests/observability_views.rs`
/// prove `render_route_health` draws five separate concepts from a
/// `RouteHealthRow`. Between them sits one line inside `shell::run`'s event
/// loop, and deleting its argument changed nothing anywhere. This test is the
/// only thing that fails when it is deleted.
///
/// # Why the readings are written through the production writers
///
/// `GatewayHealthCache::store` and `GatewayQuotaCache::store` are the same two
/// calls `gateway::mod`'s accept loop makes on every forwarded exchange. A
/// fixture that wrote the JSON itself would prove the shell can read a file
/// this test invented; practice §35's whole point is that such a fixture also
/// keeps the real seam deletable.
#[test]
fn pressing_h_draws_the_route_health_the_run_loop_read_from_disk() {
    for (rows, cols) in SIZES {
        let fixture = Fixture::new();
        seed_gateway_telemetry(&fixture);

        let mut shell = Shell::start(fixture, rows, cols);
        shell.wait_for_first_frame();
        shell.press(b"h");

        // Stage 1: the arm ran and the overlay is on screen. This is the
        // fixture's readiness check and nothing else — it is true on a tree
        // whose table has been emptied, which is exactly why the assertion
        // below is not allowed to be a timeout.
        shell.wait_for_row(
            &[" route health "],
            "the route-health overlay to open",
            cols,
        );

        // Stage 2: what the run loop actually handed the view. `Vec::new()`
        // in the arm draws `HEALTH_EMPTY` here instead, so this fails on its
        // own terms rather than by running out of patience.
        let resource = format!("{HEALTH_PROVIDER} / {HEALTH_MODEL}");
        let (found, _) = shell.wait_for_row(
            &[&resource, HEALTH_EMPTY],
            "the route-health overlay to draw its body",
            cols,
        );
        assert_eq!(
            found,
            resource,
            "at {cols}x{rows} the interface opened route health and drew its empty state. The \
             gateway caches on disk hold a reading for `{HEALTH_PROVIDER}`, and \
             `build_route_health_table` returns it — so the run loop's `Action::OpenRouteHealth` \
             arm did not carry it to the view.\n--- what the interface drew ---\n{}\n--- end ---",
            shell.screen_text()
        );

        // Stage 3: the *second* cache. `GatewayQuotaCache` is a separate load
        // inside the same builder, and a resource row that reached the screen
        // without it would satisfy stage 2 on its own.
        //
        // A wait rather than a read, and that is a measured correction rather
        // than caution: this was written as an immediate
        // `row_containing(&cadence)` and failed once in thirty runs against a
        // frame that had arrived as far as `anyrouter / probe-model-h (anyrouter/ANYROU`.
        // A frame is a few kilobytes of escape sequences read 4096 bytes at a
        // time, so *every* assertion about drawn content has to be a wait —
        // see `Shell::wait_for_row`'s own note.
        let cadence = format!("{HEALTH_STATED_LIMIT} request(s) per {HEALTH_STATED_WINDOW}s");
        let (found, _) = shell.wait_for_row(
            &[&cadence, HEALTH_CADENCE_UNSTATED],
            "route health to draw its cadence line",
            cols,
        );
        assert_eq!(
            found,
            cadence,
            "at {cols}x{rows} route health drew `{resource}` but said the provider had stated \
             nothing about its own pacing. The quota cache on disk states \
             `{HEALTH_STATED_LIMIT}` per `{HEALTH_STATED_WINDOW}`s, so only one of the builder's \
             two loads crossed.\n--- the screen ---\n{}\n--- end ---",
            shell.screen_text()
        );
    }
}

/// The same proof for a second arm, three lines above the first in the same
/// `match` — the one `phase-47.md` names as having the identical gap.
///
/// # Why this one and not a cheaper neighbour
///
/// Generality is the claim, so the second arm is deliberately the one with a
/// *different shape*: `Action::OpenRouteEvidence` calls a **fallible** builder
/// with an error branch beside the success branch, and it reads **SQLite**
/// rather than two JSON caches. If the harness only worked for infallible
/// builders over files it would be shaped around its first case.
///
/// # The ledger is closed before the binary opens it
///
/// Practice §65: an open SQLite handle is a lock, and on Windows a mandatory
/// one. This fixture opens the project database, records one observation, and
/// **drops the ledger before the child is spawned** — `seed_route_evidence`
/// returns nothing for exactly that reason.
#[test]
fn pressing_r_draws_the_route_evidence_the_run_loop_read_from_the_ledger() {
    for (rows, cols) in SIZES {
        let fixture = Fixture::new();
        seed_route_evidence(&fixture);

        let mut shell = Shell::start(fixture, rows, cols);
        shell.wait_for_first_frame();
        shell.press(b"r");

        shell.wait_for_row(
            &[" route evidence "],
            "the route-evidence overlay to open",
            cols,
        );

        let (found, _) = shell.wait_for_row(
            &[EVIDENCE_MODEL, EVIDENCE_EMPTY],
            "the route-evidence overlay to draw its body",
            cols,
        );
        assert_eq!(
            found,
            EVIDENCE_MODEL,
            "at {cols}x{rows} the interface opened route evidence and drew its empty state. The \
             project ledger holds an observation of `{EVIDENCE_PROVIDER}`/`{EVIDENCE_MODEL}` \
             inside the view's window — so the run loop's `Action::OpenRouteEvidence` arm did \
             not carry it to the view.\n--- what the interface drew ---\n{}\n--- end ---",
            shell.screen_text()
        );

        // The route column is a field the builder maps by hand out of
        // `ObservedIdentity`, so it is worth a second look for the same reason
        // the cadence is above — and it is a wait for the same reason too.
        let (found, row) = shell.wait_for_row(
            &[EVIDENCE_ROUTE, EVIDENCE_NO_ROUTE],
            "route evidence to draw its route column",
            cols,
        );
        assert_eq!(
            found,
            EVIDENCE_ROUTE,
            "at {cols}x{rows} route evidence drew a row whose route column is empty, but the \
             observation in the ledger records `{EVIDENCE_ROUTE}`.\n--- the screen ---\n{}\n--- \
             end ---",
            shell.screen_text()
        );
        assert!(
            row.contains(EVIDENCE_MODEL),
            "at {cols}x{rows} `{EVIDENCE_ROUTE}` and `{EVIDENCE_MODEL}` came back on different \
             rows, so they are not one observation.\n--- the row ---\n{row}\n--- end ---"
        );
    }
}

// ---------------------------------------------------------------------------
// The fixtures: what is on disk before the binary starts.
// ---------------------------------------------------------------------------

/// A project, a state directory and a user configuration, all thrown away
/// with the test.
///
/// Same shape as `terminal_loss::Fixture`, plus the two accessors a seeder
/// needs: this file's whole technique is *put something on disk, then ask the
/// interface to show it*, which the hangup tests never had to do.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        for sub in ["project", "state", "config"] {
            std::fs::create_dir_all(dir.path().join(sub)).expect("fixture directory");
        }
        // Onboarding marked done, so the binary opens the session shell
        // rather than the first-run wizard — the loop whose dispatch arms
        // this file is about.
        std::fs::write(
            dir.path().join("config").join("config.toml"),
            "version = 1\n\n[onboarding]\ncompleted = true\n",
        )
        .expect("write user config");
        Self { dir }
    }

    fn project_dir(&self) -> PathBuf {
        self.dir.path().join("project")
    }

    fn data_dir(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn config_dir(&self) -> PathBuf {
        self.dir.path().join("config")
    }

    /// The same paths the child will resolve from `--data-dir`/`--config-dir`.
    fn paths(&self) -> glasshouse::RuntimePaths {
        glasshouse::RuntimePaths::new(self.data_dir(), self.config_dir())
    }

    /// A [`glasshouse::Runtime`] for **this fixture's** project, built through
    /// the same `bootstrap` the binary calls and given the same arguments, so
    /// the project identity — and therefore the database file and the
    /// `project_id` every ledger row is scoped to — is the child's own.
    fn runtime(&self) -> glasshouse::Runtime {
        use clap::Parser as _;

        let project = self.project_dir();
        let cli = glasshouse::Cli::try_parse_from([
            "glasshouse",
            "--scope",
            &project.display().to_string(),
            "--data-dir",
            &self.data_dir().display().to_string(),
            "--config-dir",
            &self.config_dir().display().to_string(),
        ])
        .expect("the fixture's own arguments must parse");
        glasshouse::bootstrap(&cli, &project).expect("bootstrap the fixture's project")
    }
}

/// Write one health reading and one set of rate-limit headers through the
/// **production** writers, exactly as `gateway::mod`'s accept loop does.
///
/// The reading is deliberately one whose concepts disagree — never failed,
/// yet unavailable because the credential was refused — because that is the
/// case a single collapsed status word cannot represent, and it is the same
/// fixture `shell::route_health_tests` uses in-crate.
fn seed_gateway_telemetry(fixture: &Fixture) {
    use glasshouse::provider::telemetry::{
        GatewayHealthCache, GatewayHealthReading, GatewayQuotaCache, RateLimitHeaders,
    };

    let paths = fixture.paths();
    let now = glasshouse::provider::cache::now_unix_seconds();

    GatewayHealthCache::new(&paths).store(
        HEALTH_PROVIDER,
        &[GatewayHealthReading {
            credential_label: HEALTH_CREDENTIAL.to_owned(),
            model: HEALTH_MODEL.to_owned(),
            consecutive_failures: 0,
            // No cooldown, so the cadence line stays one row at the narrower
            // of `SIZES` — the assertion is about the provider's stated
            // window crossing, not about how the view wraps.
            cooling_down_until_unix: None,
            credential_rejected: true,
        }],
        now,
    );
    GatewayQuotaCache::new(&paths).store(
        HEALTH_PROVIDER,
        &RateLimitHeaders::read([
            ("ratelimit-limit", HEALTH_STATED_LIMIT.to_string().as_str()),
            ("ratelimit-remaining", "12"),
            ("ratelimit-reset", "1800"),
            // The only header spelling that states a window; without it the
            // cadence line reads `an unknown window`, which would make the
            // second assertion above true for the wrong reason.
            (
                "x-ratelimit-window",
                HEALTH_STATED_WINDOW.to_string().as_str(),
            ),
        ]),
        now,
    );
}

/// Record one routing observation in the project's own ledger, then close it.
///
/// Returns nothing on purpose: the connection must not outlive this call. See
/// the test's own doc comment for why (§65).
fn seed_route_evidence(fixture: &Fixture) {
    use glasshouse::routing::evidence::{ContextState, EvidenceLedger, NewObservation, Outcome};

    let runtime = fixture.runtime();
    let ledger = EvidenceLedger::open(&runtime).expect("open the fixture's evidence ledger");
    let now = glasshouse::provider::cache::now_unix_seconds();
    ledger
        .record(
            NewObservation::new(EVIDENCE_PROVIDER, EVIDENCE_MODEL)
                .with_route(Some(EVIDENCE_ROUTE))
                .with_context_state(ContextState::Cold)
                .with_outcome(Outcome::Succeeded),
            now,
        )
        .expect("record an observation");
    drop(ledger);
    drop(runtime);
}

// ---------------------------------------------------------------------------
// The harness.
// ---------------------------------------------------------------------------

/// The shipped binary on a real terminal, with the screen it has drawn.
///
/// # Field order is load-bearing
///
/// `fixture` is declared **last** so it is dropped last. A failing assertion
/// leaves this scope by unwinding, and a `TempDir` dropped while the child is
/// still alive deletes the state directory and the database that child is
/// using — which in this repository has already turned one failing test into
/// a hanging one. [`Drop`] kills the child first; the field order is what
/// makes "first" true.
struct Shell {
    /// The master, kept alive for the whole test and never read again: the
    /// child's terminal exists for exactly as long as this does. Nothing here
    /// takes a terminal away — that is `terminal_loss.rs`'s subject, not this
    /// one.
    _master: Box<dyn MasterPty + Send>,
    fd: RawFd,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Every byte the child has written, replayed into a real terminal
    /// emulator. Assertions read this rather than the raw stream — see the
    /// file header.
    parser: vt100::Parser,
    /// Kept only for failure messages: `vt100` shows the screen as it is now,
    /// and a test that fails wants to say what came before it too.
    raw: Vec<u8>,
    /// Never read after construction. Its whole job is to be dropped **after**
    /// the child, which the declaration order arranges.
    _fixture: Fixture,
}

impl Shell {
    /// Start `glasshouse` on a `rows`x`cols` pty over `fixture`.
    fn start(fixture: Fixture, rows: u16, cols: u16) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty");

        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_glasshouse"));
        // The same exact-snapshot environment `pty::TerminalCommand` builds
        // and `terminal_loss` copies: portable-pty's own defaults are not
        // this process's environment.
        command.env_clear();
        for (key, value) in std::env::vars_os() {
            command.env(key, value);
        }
        let project = fixture.project_dir();
        command.cwd(&project);
        command.args([
            "--scope",
            &project.display().to_string(),
            "--data-dir",
            &fixture.data_dir().display().to_string(),
            "--config-dir",
            &fixture.config_dir().display().to_string(),
        ]);

        let child = pair.slave.spawn_command(command).expect("spawn glasshouse");
        // The last slave descriptor this process holds; keeping it would keep
        // the terminal half-alive from the child's point of view.
        drop(pair.slave);

        let fd = pair
            .master
            .as_raw_fd()
            .expect("a Unix pty master has a descriptor");
        set_nonblocking(fd);

        Self {
            _master: pair.master,
            fd,
            child,
            // No scrollback: every assertion here is about the visible
            // screen, which is what a user is looking at.
            parser: vt100::Parser::new(rows, cols, 0),
            raw: Vec::new(),
            _fixture: fixture,
        }
    }

    /// Read whatever the child has written and feed it to the emulator,
    /// without waiting for more.
    fn drain(&mut self) {
        let mut buffer = [0u8; 4096];
        loop {
            // SAFETY: `buffer` is a live, initialised array and its length is
            // passed alongside it. The descriptor is owned by the `MasterPty`
            // this struct holds, which is alive for the whole of `self`.
            let read = unsafe {
                libc::read(
                    self.fd,
                    buffer.as_mut_ptr().cast::<libc::c_void>(),
                    buffer.len(),
                )
            };
            if read > 0 {
                #[allow(clippy::cast_sign_loss)]
                let chunk = &buffer[..read as usize];
                self.raw.extend_from_slice(chunk);
                self.parser.process(chunk);
                continue;
            }
            // Zero is end-of-file and a negative is either "nothing waiting"
            // or a broken pty. None of the three is something to wait on
            // here; the caller's deadline decides what happens next.
            return;
        }
    }

    /// Wait until the interface has drawn itself, or fail saying what it did
    /// instead.
    ///
    /// This is the ready signal, and the file header explains why it is a
    /// causal one rather than a delay: the banner cannot be on screen unless
    /// raw mode is already on.
    fn wait_for_first_frame(&mut self) {
        let banner = format!("glasshouse {}", glasshouse::VERSION);
        let cols = self.parser.screen().size().1;
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline {
            self.drain();
            if self.row_containing(&banner, cols).is_some() {
                return;
            }
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                panic!(
                    "glasshouse exited ({status:?}) before drawing anything, so nothing below \
                     can say anything about a key reaching its event loop.\n--- what it \
                     wrote ---\n{}\n--- end ---",
                    self.raw_text()
                );
            }
            std::thread::sleep(READ_POLL);
        }
        panic!(
            "glasshouse never drew {banner:?} within {STARTUP_TIMEOUT:?}.\n--- the screen ---\n{}\
             \n--- what it wrote ---\n{}\n--- end ---",
            self.screen_text(),
            self.raw_text()
        );
    }

    /// Type at the terminal, exactly as a person does: the bytes go in at the
    /// master and come out of the child's standard input.
    fn press(&mut self, keys: &[u8]) {
        // SAFETY: `keys` is a live slice and its length is passed alongside
        // it. The descriptor is owned by the `MasterPty` this struct holds.
        let written =
            unsafe { libc::write(self.fd, keys.as_ptr().cast::<libc::c_void>(), keys.len()) };
        assert_eq!(
            usize::try_from(written).unwrap_or(0),
            keys.len(),
            "could not put {keys:?} into the terminal"
        );
    }

    /// Wait until one of `needles` is on a **single rendered row**, and say
    /// which.
    ///
    /// # Why one row rather than the whole screen
    ///
    /// A needle matched against the flattened screen can be satisfied by the
    /// end of one row and the start of the next, which is a match no user
    /// would ever see. Every row is checked whole, so a match is a line.
    ///
    /// # Why more than one needle
    ///
    /// So a caller can wait for the answer it wants *and* the answer that
    /// means the thing under test is broken, and then assert on which
    /// arrived. A test that waited only for the value it wanted would report
    /// every failure as a timeout, which is practice §80's fifth case: a
    /// verdict credited to an assertion that never ran.
    ///
    /// # Every assertion about drawn content goes through this
    ///
    /// **Measured, not stylistic.** A frame is a few kilobytes of escape
    /// sequences and this reads the master 4096 bytes at a time, so the
    /// emulator's screen between two reads is a *partial* frame. A second
    /// assertion written as an immediate [`Shell::row_containing`] after this
    /// one returned failed 1 run in 30 against
    /// `anyrouter / probe-model-h (anyrouter/ANYROU` — a row that had arrived
    /// as far as its 46th column. So [`Shell::row_containing`] is only ever
    /// called from inside a wait loop, and a test that wants two facts waits
    /// twice. Practice §60, on a stream rather than on a race: a single look
    /// cannot tell "not drawn" from "not read yet".
    ///
    /// Waiting costs nothing it should not: the interface draws a whole frame
    /// and then idles, so a fact that is missing after one frame stays missing.
    ///
    /// Fails loudly and differently for a child that has died — the packet's
    /// requirement, and the difference between "this defect is real" and
    /// "this runner is slow".
    fn wait_for_row(&mut self, needles: &[&str], what: &str, cols: u16) -> (String, String) {
        let deadline = Instant::now() + RESPONSE_DEADLINE;
        loop {
            self.drain();
            for needle in needles {
                if let Some(row) = self.row_containing(needle, cols) {
                    return ((*needle).to_owned(), row);
                }
            }
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                panic!(
                    "glasshouse exited ({status:?}) while this was waiting for {what}. A process \
                     that is gone answers no keys; this is not a slow machine.\n--- the last \
                     screen ---\n{}\n--- what it wrote ---\n{}\n--- end ---",
                    self.screen_text(),
                    self.raw_text()
                );
            }
            if Instant::now() >= deadline {
                panic!(
                    "{RESPONSE_DEADLINE:?} after the key was typed, the interface had still not \
                     drawn any of {needles:?} on one row — this was waiting for {what}. The \
                     process is still alive, so the key reached nothing.\n--- the screen ---\n{}\n\
                     --- end ---",
                    self.screen_text()
                );
            }
            std::thread::sleep(READ_POLL);
        }
    }

    /// The first rendered row containing `needle`, if any.
    ///
    /// **Only correct inside a wait loop** — see [`Shell::wait_for_row`], which
    /// is the only caller besides [`Shell::wait_for_first_frame`]. A single
    /// look at this can be taken between two reads of a frame.
    fn row_containing(&self, needle: &str, cols: u16) -> Option<String> {
        self.parser
            .screen()
            .rows(0, cols)
            .find(|row| row.contains(needle))
    }

    /// The screen as a user would see it, for a failure message.
    fn screen_text(&self) -> String {
        self.parser.screen().contents()
    }

    /// Everything the child ever wrote, escapes and all. Only for failure
    /// messages, and only where the *stream* is the interesting thing — a
    /// process that died before drawing has no screen to show.
    fn raw_text(&self) -> String {
        String::from_utf8_lossy(&self.raw).into_owned()
    }

    /// End the child now, and do not come back until it is reaped.
    ///
    /// `SIGKILL` rather than `portable_pty::Child::kill`, which sends
    /// `SIGHUP`, and draining while waiting: both are `terminal_loss::Shell::
    /// kill`'s findings, and both apply here. A child blocked writing its
    /// last frame to a pty nobody is draining cannot finish dying, and this
    /// thread is the only reader there is.
    fn kill(&mut self) {
        if let Some(pid) = self.child.process_id() {
            // SAFETY: `kill` takes no pointers. A pid that has already been
            // reaped fails with `ESRCH`, which is not an error here.
            unsafe {
                libc::kill(pid.cast_signed(), libc::SIGKILL);
            }
        }
        let deadline = Instant::now() + KILL_DEADLINE;
        while Instant::now() < deadline {
            if self.child.try_wait().expect("try_wait").is_some() {
                return;
            }
            self.drain();
            std::thread::sleep(READ_POLL);
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        // A failed assertion leaves this scope by unwinding, and four
        // orphaned `glasshouse` processes accumulated in this repository
        // before a guard of this shape existed. `Child::drop` does not do it.
        if self.child.try_wait().expect("try_wait").is_none() {
            self.kill();
        }
        // `_fixture` and `_master` are dropped after this returns, in
        // declaration order — the temporary directory outlives the process
        // that was using it, which is the point of the ordering.
    }
}

fn set_nonblocking(fd: RawFd) {
    // SAFETY: `fd` is owned by a live `MasterPty`; both calls only read and
    // replace its status flags.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        assert!(flags >= 0, "could not read the pty master's flags");
        assert!(
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0,
            "could not make the pty master non-blocking"
        );
    }
}
