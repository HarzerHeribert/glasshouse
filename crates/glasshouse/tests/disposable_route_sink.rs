//! The disposable-job routing rationale, made durable and read back.
//!
//! # What this file is proving, and why it is one file
//!
//! `docs/product/design-decisions.md`'s *"A durable observation sink"* asked
//! for a **producer**, and its own scoping correction says the honest first
//! package closes no capability-map box. Nothing here claims one. What is
//! claimed is narrower and checkable: one real routing rationale survives the
//! process that produced it, and the shipped interface shows it.
//!
//! Both halves are here because a producer with no reader is this
//! repository's most common defect — a mechanism built, wired and never read.
//! Splitting them into two files would have let either half land alone and
//! look finished.
//!
//! # Nothing is seeded through a back door
//!
//! Every test below produces its observation by running `glasshouse hook`,
//! exactly as a harness runs it at the end of a turn. Practice §35 is the
//! reason: a fixture that called `evaluation::record_disposable_route` itself
//! would keep passing with the production call site deleted, which is
//! precisely the failure this package exists not to add. The only thing this
//! file writes directly is a user configuration, which is what a person
//! writes.
//!
//! # What is cross-platform and what is not
//!
//! The producer half and the isolation read run everywhere. The three tests
//! that read a screen are `#[cfg(unix)]`, for `tests/tui_harness.rs`'s own
//! reason: `portable_pty` on Windows is ConPTY, a screen buffer rather than a
//! byte pipe, so the master carries no stream a `vt100::Parser` can rebuild a
//! Ratatui frame from. That is a separate reading technique, not a `cfg` to
//! widen — and it is why the `cfg` is on the tests rather than on the file,
//! so the producer is still proved on Windows.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Parser as _;
use glasshouse::config::{ProviderConfig, UserConfig};
use glasshouse::evaluation::{EvaluationKind, EvaluationObservation, EvaluationObservations};
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

/// The provider, free model and credential every fixture configures.
///
/// The model name is one no catalogue contains, so a row on screen carrying
/// it can only have come from this fixture's own decision. The credential's
/// value is asserted **absent** from what was stored — the packet's security
/// invariant, checked rather than reasoned.
const PROVIDER: &str = "route-sink-runner";
const FREE_MODEL: &str = "probe-free-model-d";
const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_ROUTE_SINK_KEY";
const CREDENTIAL: &str = "sk-fabricated-route-sink-value-not-a-real-credential";

/// A second project's model name, used only by the isolation tests. Distinct
/// from [`FREE_MODEL`] so "this project cannot see the other one's decision"
/// is a claim about a specific string rather than about a row count.
const OTHER_FREE_MODEL: &str = "probe-free-model-elsewhere";

/// What `shell::view::render_route_decisions` draws when nothing is stored.
/// Every screen assertion waits for this alongside the value it wants, so a
/// view that drew the empty state fails on its own terms rather than by
/// running out of patience (practice §80 case 5).
#[cfg(unix)] // read only from the `#[cfg(unix)]` screen module below
const DECISIONS_EMPTY: &str = "no routing decision has been recorded yet";

/// The overlay's own border title — the readiness signal that says the key
/// arrived and the run loop's arm ran. True on a tree whose reader has been
/// emptied, which is exactly why it is never the assertion.
#[cfg(unix)] // read only from the `#[cfg(unix)]` screen module below
const DECISIONS_TITLE: &str = " routing decisions ";

// ---------------------------------------------------------------------------
// A project, and the binary run against it.
// ---------------------------------------------------------------------------

/// One bootstrapped project sharing `base`'s data and config roots — the
/// shape `tests/evaluation_observations.rs` uses, so two fixtures over one
/// `base` are two real projects on one machine, each with its own
/// canonicalised root and its own `glasshouse.db`.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap(base, &root);
        Self {
            base: base.to_path_buf(),
            root,
            runtime,
        }
    }

    /// The configuration a person writes to give Glasshouse's own support
    /// work a zero-cost model, plus the onboarding flag that makes the binary
    /// open the session shell rather than the first-run wizard.
    ///
    /// Deliberately **no** `memory_extraction_model`: naming one takes
    /// `disposable_extraction_model`'s early return, where no disposable
    /// routing decision is made at all. This fixture is about the branch that
    /// does decide.
    fn configure(&self, free_model: &str) {
        let mut user = UserConfig::load(self.runtime.paths()).unwrap();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        provider.set_free_models(vec![free_model.to_owned()]);
        user.providers_mut().set(PROVIDER, provider);
        user.onboarding_mut()
            .mark_completed(glasshouse::VERSION.to_owned());
        user.save(self.runtime.paths()).unwrap();
    }

    /// A session in this project, running, as a harness's would be.
    fn running_session(&self) -> SessionId {
        let sessions = ProjectSessions::open(&self.runtime).unwrap();
        let store = sessions.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();
        record.id
    }

    /// Run `glasshouse hook`, exactly as a harness runs it at the end of a
    /// turn, and assert it exited zero.
    ///
    /// The exit status is asserted here rather than returned because it is
    /// never the subject: `report_hook`'s own contract is that a hook may
    /// never fail the user's turn, so a non-zero exit is a defect in every
    /// test below rather than a result any of them is interested in.
    fn completed_turn(&self, session: &SessionId) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("hook")
            .arg("--session")
            .arg(session.as_str())
            .arg("--event")
            .arg("Stop")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(PAYLOAD.as_bytes())
            .expect("the handler must read its payload rather than closing the pipe");
        let output = child.wait_with_output().expect("the hook must finish");
        assert!(
            output.status.success(),
            "a hook must exit zero whatever routing decided: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Every disposable-routing decision this project has recorded, newest
    /// first, read back through the store's own door.
    fn decisions(&self) -> Vec<EvaluationObservation> {
        let ledger = EvaluationObservations::open(&self.runtime).expect("open the ledger");
        let rows = ledger
            .recent_of_kind(EvaluationKind::DisposableRouteDecided, 20)
            .expect("read the ledger");
        // Dropped before the caller can spawn anything: practice §65, and the
        // same rule `tui_harness::seed_route_evidence` follows.
        drop(ledger);
        rows
    }
}

/// A harness payload with the conversation in it. Never read by the hook, and
/// therefore never available to anything this package records — the reason
/// "no raw prompt text in `detail`" is structural here rather than filtered.
const PAYLOAD: &str = concat!(
    r#"{"session_id":"native-1","hook_event_name":"Stop","cwd":"/somewhere","#,
    r#""prompt":"PAYLOAD-PROMPT-MUST-NEVER-BE-STORED","#,
    r#""last_assistant_message":"PAYLOAD-REPLY-MUST-NEVER-BE-STORED"}"#
);

const PROMPT_MARKER: &str = "PAYLOAD-PROMPT-MUST-NEVER-BE-STORED";

fn bootstrap(base: &Path, root: &Path) -> Runtime {
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

// ---------------------------------------------------------------------------
// 1. The rationale becomes durable.
// ---------------------------------------------------------------------------

/// **The producer, through the shipped binary.**
///
/// A harness reports `Stop`; `glasshouse hook` routes its own memory
/// extraction over the free model this project configured, and the reason it
/// landed there is in the project's database when the process is gone.
///
/// This fails on `main` for the reason that nothing writes it: before this
/// package the rationale reached `tracing::info!` and nothing else, so the
/// ledger held no row of this kind and `EvaluationKind::DisposableRouteDecided`
/// did not exist to ask for.
///
/// Deleting the `record_disposable_route` call in
/// `main.rs::disposable_extraction_model` kills this test, which is the
/// point (§35): nothing else in the suite enters through that function.
#[test]
fn a_completed_turn_makes_the_disposable_routing_rationale_durable() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "durable");
    fixture.configure(FREE_MODEL);
    let session = fixture.running_session();

    fixture.completed_turn(&session);

    let decisions = fixture.decisions();
    assert_eq!(
        decisions.len(),
        1,
        "one completed turn is one routing decision, and it must be recorded once: {decisions:#?}"
    );
    let decision = &decisions[0];
    assert_eq!(
        decision.session_id.as_deref(),
        Some(session.as_str()),
        "the decision must be recorded against the session it was made for"
    );
    assert_eq!(
        decision.subject.as_deref(),
        Some("memory extraction"),
        "`subject` is the job kind's own name"
    );

    let detail = decision
        .detail
        .as_deref()
        .expect("a decision with no rationale is the thing this package exists to prevent");
    assert!(
        detail.contains(FREE_MODEL),
        "the rationale must name the resource that was chosen:\n{detail}"
    );
    assert!(
        detail.contains(PROVIDER),
        "the rationale must name the provider it was chosen on:\n{detail}"
    );
    assert!(
        detail.contains("line 530 prefers free capacity"),
        "the rationale must carry the named contributions behind the decision, not only its \
         outcome:\n{detail}"
    );

    // The columns this path cannot honestly fill stay absent — map line
    // 1294. `routing_seq` in particular: the disposable policy calls no
    // model, so no `routing_observations` row exists for this turn, and a
    // `seq` pointing at some other turn's measurement would invert the
    // provenance rather than supply it.
    assert_eq!(
        decision.routing_seq, None,
        "no routing observation exists to point at"
    );
    assert_eq!(decision.memory_id, None);
    assert_eq!(decision.feature, None);
    assert_eq!(decision.arm, None);

    // The packet's security invariant, asserted rather than reasoned.
    assert!(
        !detail.contains(CREDENTIAL),
        "a credential reached the durable rationale:\n{detail}"
    );
    assert!(
        !detail.contains(CREDENTIAL_VAR),
        "a credential's variable name reached the durable rationale:\n{detail}"
    );
    assert!(
        !detail.contains(PROMPT_MARKER),
        "the conversation reached the durable rationale:\n{detail}"
    );
}

// ---------------------------------------------------------------------------
// 3. Cross-project isolation — the read half.
// ---------------------------------------------------------------------------

/// **Isolation, proved rather than read.**
///
/// Two real projects on one machine, sharing one data root and one
/// configuration. The second completes a turn and records a decision naming
/// its own model; the first has completed none. The first project's own read
/// must return nothing at all — not a filtered view of the other's row, and
/// not a row whose model it could name.
#[test]
fn another_projects_routing_decision_is_not_readable_from_this_project() {
    let tmp = tempfile::tempdir().unwrap();
    let here = Fixture::new(tmp.path(), "here");
    let elsewhere = Fixture::new(tmp.path(), "elsewhere");
    // One configuration, because both projects share a config root — which
    // is what makes this a test about project scope rather than about two
    // differently configured machines.
    elsewhere.configure(OTHER_FREE_MODEL);

    let session = elsewhere.running_session();
    elsewhere.completed_turn(&session);

    let theirs = elsewhere.decisions();
    assert_eq!(
        theirs.len(),
        1,
        "the other project must actually have recorded one, or this proves nothing"
    );
    assert!(
        theirs[0]
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains(OTHER_FREE_MODEL)),
        "the other project's decision must name its own model: {:#?}",
        theirs[0]
    );

    let ours = here.decisions();
    assert!(
        ours.is_empty(),
        "a project that has completed no turn must read no decision, and certainly not another \
         project's: {ours:#?}"
    );
}

// ---------------------------------------------------------------------------
// 2 and 4. The reader, on a real terminal.
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod screen {
    use super::{
        CREDENTIAL, CREDENTIAL_VAR, DECISIONS_EMPTY, DECISIONS_TITLE, FREE_MODEL, Fixture,
        OTHER_FREE_MODEL, PROVIDER,
    };

    use std::os::fd::RawFd;
    use std::time::{Duration, Instant};

    use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

    /// `tests/tui_harness.rs`'s own budgets, unchanged, and its reasoning with
    /// them: a keystroke the loop never saw is never answered, so waiting
    /// longer cannot turn a failure into a pass. These bound how long a real
    /// failure takes to report and never decide whether one happened.
    const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
    const RESPONSE_DEADLINE: Duration = Duration::from_secs(10);
    const KILL_DEADLINE: Duration = Duration::from_secs(10);
    const READ_POLL: Duration = Duration::from_millis(10);

    /// A realistic terminal and one wide enough that nothing can be clipped —
    /// practice §17. A row found at 220 columns and not at 120 is a layout
    /// finding rather than a dispatch failure, and the message says which.
    const SIZES: [(u16, u16); 2] = [(40, 120), (48, 220)];

    /// **The reader, driven by a key on a real terminal.**
    ///
    /// The decision on screen was made by a real `glasshouse hook` process
    /// that has already exited. Nothing in this test writes to the ledger, so
    /// what it proves is the whole chain: the hook decided and recorded, the
    /// run loop's `Action::OpenRouteDecisions` arm read, and the view drew.
    ///
    /// Emptying `build_route_decision_table` leaves stage 1 passing — the
    /// overlay still opens — and fails stage 2 on its own assertion, quoting
    /// the empty state it drew instead.
    #[test]
    fn pressing_d_draws_the_routing_rationale_the_run_loop_read_from_the_ledger() {
        for (rows, cols) in SIZES {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "shown");
            fixture.configure(FREE_MODEL);
            let session = fixture.running_session();
            fixture.completed_turn(&session);

            let mut shell = Shell::start(&fixture, rows, cols);
            shell.wait_for_first_frame();
            shell.press(b"d");

            // Stage 1: the key arrived and the arm ran. Readiness only — it
            // is true of a tree whose reader returns nothing.
            shell.wait_for_row(
                &[DECISIONS_TITLE],
                "the routing-decisions overlay to open",
                cols,
            );

            // Stage 2: what the run loop actually handed the view.
            let (found, row) = shell.wait_for_row(
                &[FREE_MODEL, DECISIONS_EMPTY],
                "the routing-decisions overlay to draw its body",
                cols,
            );
            assert_eq!(
                found,
                FREE_MODEL,
                "at {cols}x{rows} the interface opened routing decisions and drew its empty \
                 state. A `glasshouse hook` process recorded a decision naming \
                 `{FREE_MODEL}` in this project's ledger before the shell started — so the run \
                 loop's `Action::OpenRouteDecisions` arm did not carry it to the \
                 view.\n--- what the interface drew ---\n{}\n--- end ---",
                shell.screen_text()
            );
            assert!(
                row.contains(PROVIDER),
                "at {cols}x{rows} the chosen model and its provider came back on different rows, \
                 so they are not one decision.\n--- the row ---\n{row}\n--- end ---"
            );

            // Stage 3: the contributions, which are a *different* part of the
            // stored string — the heading alone would satisfy stage 2, and a
            // rationale reduced to its outcome is what map line 1766 asks
            // this view not to be. A wait rather than a read, because a frame
            // is read 4096 bytes at a time and a single look can land inside
            // one (`tui_harness::Shell::wait_for_row`'s measured note).
            let contribution = "line 530 prefers free capacity";
            let (found, _) = shell.wait_for_row(
                &[contribution, DECISIONS_EMPTY],
                "routing decisions to draw the named contributions",
                cols,
            );
            assert_eq!(
                found,
                contribution,
                "at {cols}x{rows} the view drew which resource was chosen but not one reason it \
                 won, so the stored rationale reached the screen only in part.\n--- the screen \
                 ---\n{}\n--- end ---",
                shell.screen_text()
            );

            // The security invariant again, on the other side of the process
            // boundary: what is stored is also what is shown.
            let screen = shell.screen_text();
            assert!(
                !screen.contains(CREDENTIAL),
                "a credential reached the screen:\n{screen}"
            );
            assert!(
                !screen.contains(CREDENTIAL_VAR),
                "a credential's variable name reached the screen:\n{screen}"
            );
        }
    }

    /// **Nothing recorded, nothing shown.**
    ///
    /// Without this, the test above would pass on a view that drew the same
    /// text whatever the ledger held — a hardcoded row is indistinguishable
    /// from a read one when the only project ever looked at has data in it.
    /// The project here has completed no turn, so the honest answer is the
    /// empty state, and drawing it is not an error.
    #[test]
    fn with_nothing_recorded_the_routing_decisions_view_is_empty_and_does_not_error() {
        for (rows, cols) in SIZES {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "quiet");
            fixture.configure(FREE_MODEL);

            let mut shell = Shell::start(&fixture, rows, cols);
            shell.wait_for_first_frame();
            shell.press(b"d");

            shell.wait_for_row(
                &[DECISIONS_TITLE],
                "the routing-decisions overlay to open",
                cols,
            );
            let (found, _) = shell.wait_for_row(
                &[DECISIONS_EMPTY, FREE_MODEL],
                "the routing-decisions overlay to say what it has",
                cols,
            );
            assert_eq!(
                found,
                DECISIONS_EMPTY,
                "at {cols}x{rows} a project that has recorded no decision was shown one, so this \
                 view draws something it did not read.\n--- what the interface drew ---\n{}\n--- \
                 end ---",
                shell.screen_text()
            );

            let screen = shell.screen_text();
            assert!(
                !screen.contains("unavailable"),
                "an empty ledger is an honest answer, not a read failure:\n{screen}"
            );
        }
    }

    /// **Isolation, on screen.**
    ///
    /// The read half is proved without a terminal by
    /// `another_projects_routing_decision_is_not_readable_from_this_project`.
    /// This is the other half of the packet's invariant — *neither read nor
    /// rendered* — because a view that reached past its own `Runtime` would
    /// fail here and nowhere else.
    #[test]
    fn another_projects_routing_decision_is_never_drawn_here() {
        let (rows, cols) = SIZES[1];
        let tmp = tempfile::tempdir().unwrap();
        let here = Fixture::new(tmp.path(), "here");
        let elsewhere = Fixture::new(tmp.path(), "elsewhere");
        elsewhere.configure(OTHER_FREE_MODEL);
        let session = elsewhere.running_session();
        elsewhere.completed_turn(&session);

        let mut shell = Shell::start(&here, rows, cols);
        shell.wait_for_first_frame();
        shell.press(b"d");

        shell.wait_for_row(
            &[DECISIONS_TITLE],
            "the routing-decisions overlay to open",
            cols,
        );
        let (found, _) = shell.wait_for_row(
            &[DECISIONS_EMPTY, OTHER_FREE_MODEL],
            "the routing-decisions overlay to say what this project has",
            cols,
        );
        assert_eq!(
            found,
            DECISIONS_EMPTY,
            "at {cols}x{rows} this project's interface drew a decision belonging to another \
             project on the same machine.\n--- what the interface drew ---\n{}\n--- end ---",
            shell.screen_text()
        );
    }

    // -----------------------------------------------------------------------
    // The harness. `tests/tui_harness.rs`'s technique, trimmed to what these
    // three tests use.
    //
    // Duplicated rather than shared because Rust gives each `tests/*.rs` its
    // own crate: sharing it needs a `tests/` module directory, which would
    // want `tui_harness.rs` and `terminal_loss.rs` moved onto it too — a
    // change this package was not scoped to make. Said here so the next
    // person reads it as a known cost rather than an oversight.
    // -----------------------------------------------------------------------

    /// The shipped binary on a real terminal, with the screen it has drawn.
    ///
    /// Field order is load-bearing in the original and would be here too if
    /// this owned its fixture: a `TempDir` dropped while the child is alive
    /// deletes the database that child is using. This one borrows the
    /// fixture instead, so the caller's `tmp` outlives the child by
    /// construction — `Drop` kills the child before this scope ends.
    struct Shell {
        _master: Box<dyn MasterPty + Send>,
        fd: RawFd,
        child: Box<dyn portable_pty::Child + Send + Sync>,
        parser: vt100::Parser,
        raw: Vec<u8>,
    }

    impl Shell {
        fn start(fixture: &Fixture, rows: u16, cols: u16) -> Self {
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("open pty");

            let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_glasshouse"));
            // portable-pty's defaults are not this process's environment, and
            // the child needs the credential variable the fixture's provider
            // names — without it the provider contributes no candidate and
            // the decision under test is a different one.
            command.env_clear();
            for (key, value) in std::env::vars_os() {
                command.env(key, value);
            }
            command.env(CREDENTIAL_VAR, CREDENTIAL);
            command.cwd(&fixture.root);
            command.args([
                "--scope",
                &fixture.root.display().to_string(),
                "--data-dir",
                &fixture.base.join("data").display().to_string(),
                "--config-dir",
                &fixture.base.join("config").display().to_string(),
            ]);

            let child = pair.slave.spawn_command(command).expect("spawn glasshouse");
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
                parser: vt100::Parser::new(rows, cols, 0),
                raw: Vec::new(),
            }
        }

        fn drain(&mut self) {
            let mut buffer = [0u8; 4096];
            loop {
                // SAFETY: `buffer` is a live, initialised array and its length
                // is passed alongside it. The descriptor is owned by the
                // `MasterPty` this struct holds, alive for the whole of `self`.
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
                return;
            }
        }

        /// The causal ready signal, not a delay: the banner cannot be on
        /// screen unless raw mode is already on and the event source exists.
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
                "glasshouse never drew {banner:?} within {STARTUP_TIMEOUT:?}.\n--- the screen \
                 ---\n{}\n--- what it wrote ---\n{}\n--- end ---",
                self.screen_text(),
                self.raw_text()
            );
        }

        fn press(&mut self, keys: &[u8]) {
            // SAFETY: `keys` is a live slice and its length is passed
            // alongside it. The descriptor is owned by the `MasterPty` this
            // struct holds.
            let written =
                unsafe { libc::write(self.fd, keys.as_ptr().cast::<libc::c_void>(), keys.len()) };
            assert_eq!(
                usize::try_from(written).unwrap_or(0),
                keys.len(),
                "could not put {keys:?} into the terminal"
            );
        }

        /// Wait until one of `needles` is on a single rendered row, and say
        /// which. More than one needle so a caller can wait for the answer it
        /// wants *and* the answer that means the thing under test is broken,
        /// then assert on which arrived — otherwise every failure reports as
        /// a timeout, which is §80's fifth case.
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
                        "glasshouse exited ({status:?}) while this was waiting for {what}. A \
                         process that is gone answers no keys; this is not a slow machine.\n--- \
                         the last screen ---\n{}\n--- what it wrote ---\n{}\n--- end ---",
                        self.screen_text(),
                        self.raw_text()
                    );
                }
                if Instant::now() >= deadline {
                    panic!(
                        "{RESPONSE_DEADLINE:?} after the key was typed, the interface had still \
                         not drawn any of {needles:?} on one row — this was waiting for {what}. \
                         The process is still alive, so the key reached nothing.\n--- the screen \
                         ---\n{}\n--- end ---",
                        self.screen_text()
                    );
                }
                std::thread::sleep(READ_POLL);
            }
        }

        /// Only correct inside a wait loop — a single look can be taken
        /// between two reads of one frame.
        fn row_containing(&self, needle: &str, cols: u16) -> Option<String> {
            self.parser
                .screen()
                .rows(0, cols)
                .find(|row| row.contains(needle))
        }

        fn screen_text(&self) -> String {
            self.parser.screen().contents()
        }

        fn raw_text(&self) -> String {
            String::from_utf8_lossy(&self.raw).into_owned()
        }

        /// `SIGKILL` rather than `Child::kill`'s `SIGHUP`, and draining while
        /// waiting: a child blocked writing its last frame to a pty nobody is
        /// draining cannot finish dying, and this thread is the only reader.
        fn kill(&mut self) {
            if let Some(pid) = self.child.process_id() {
                // SAFETY: `kill` takes no pointers. A pid already reaped fails
                // with `ESRCH`, which is not an error here.
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
            // A failed assertion leaves this scope by unwinding, and
            // `Child::drop` does not do this.
            if self.child.try_wait().expect("try_wait").is_none() {
                self.kill();
            }
        }
    }

    fn set_nonblocking(fd: RawFd) {
        // SAFETY: `fd` is owned by a live `MasterPty`; both calls only read
        // and replace its status flags.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            assert!(flags >= 0, "could not read the pty master's flags");
            assert!(
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0,
                "could not make the pty master non-blocking"
            );
        }
    }
}
