use super::*;
use crate::integrations::IntegrationId;
use crate::session::{
    SessionId, SessionLifecycle, SessionPresentation, SessionRecord, SessionRole,
};
use crate::shell::hotspot::Hotspot;
use crate::shell::state::Action;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn record(id: &str, harness: &str, lifecycle: SessionLifecycle) -> SessionRecord {
    SessionRecord {
        id: SessionId::new(id),
        project_id: "project".to_owned(),
        harness: harness.to_owned(),
        native_session_id: None,
        role: SessionRole::Normal,
        lifecycle,
        presentation: SessionPresentation::Embedded,
        created_at: 1_000,
        last_activity_at: 1_000,
        launch_profile: None,
        backend_resource: None,
        model: None,
        pairing_class: None,
        protocol: None,
        response_profile: None,
        response_mechanism: None,
        display_name: None,
        purpose: None,
        source_session_id: None,
        observed_compactions: None,
        presentation_ref: None,
        last_seen_commit: None,
        entitlement: None,
    }
}

/// One session, for the cases that need a project with nothing to navigate
/// between.
fn lone_session() -> SessionRecord {
    record("only-one", "claude-code", SessionLifecycle::Running)
}

/// The top row of a rendered frame, trimmed — session mode's whole chrome.
fn first_row(state: &ShellState, width: u16, height: u16) -> String {
    rendered(state, width, height)
        .lines()
        .next()
        .expect("a frame has rows")
        .trim_end()
        .to_owned()
}

/// The whole footer band, every row of it.
///
/// [`last_row`] is not the footer any more: the action bar wraps, so the
/// bottom row alone is its tail. Read through [`regions`] rather than by
/// counting from the bottom, so the band this asserts on is the band the
/// renderer was given.
fn footer(state: &ShellState, width: u16, height: u16) -> String {
    let band = regions(Rect::new(0, 0, width, height))[4];
    rendered(state, width, height)
        .lines()
        .skip(usize::from(band.y))
        .take(usize::from(band.height))
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The tab strip's band, which is `regions`' third — read through `regions`
/// for the same reason [`footer`] is, so the row asserted on is the row the
/// renderer was handed.
fn strip(state: &ShellState, width: u16, height: u16) -> String {
    let band = regions(Rect::new(0, 0, width, height))[2];
    rendered(state, width, height)
        .lines()
        .nth(usize::from(band.y))
        .expect("a frame has rows")
        .trim_end()
        .to_owned()
}

/// Five sessions: a tab pill is 28 columns, so two of them fit an
/// eighty-column strip and three cannot be on it.
fn five_sessions() -> ShellState {
    ShellState::new(
        "p",
        "/p",
        "0.1.0",
        (1..=5)
            .map(|n| {
                record(
                    &format!("session-{n}"),
                    "claude-code",
                    SessionLifecycle::Running,
                )
            })
            .collect(),
    )
}

/// The sixteen actions control mode advertises, each as the text its pill
/// paints. One list, so "on screen" and "clickable" are asserted about the
/// same set rather than about two lists that can drift.
///
/// **Sixteen, not fifteen**: `c connect` was added because connecting an
/// account is the first thing a new install must do and there was no key for
/// it at all. The three that lead the bar are listed first, in the order they
/// are drawn; the rest are the demoted run, and *demoted* is why they are
/// still in this list — `hotspot::Pill::subordinate` changes a colour and
/// nothing else, so every one of them must still be drawn and still be
/// clickable at eighty columns.
const CONTROL_ACTIONS: [&str; 16] = [
    "n new",
    "c connect",
    "s settings",
    "q quit",
    "o overview",
    "p project",
    "e events",
    "t theme:",
    "h health",
    "r routes",
    "d decisions",
    "k knowledge",
    "M memory",
    "N headless",
    "tab session",
    "enter session",
];

/// The three notes measured mid-sentence at eighty columns, verbatim from the
/// production code that sets them: `shell::liveness`, `state::overview` and
/// the run loop's launch arm.
const MEASURED_NOTES: [&str; 3] = [
    "session `c0d538f1877a` exited — back in control mode",
    "cannot resume `73b335015a80`: no native session id was recorded",
    "started session `cd3f5d6c7bc2` — Enter to type in it",
];

fn sample() -> ShellState {
    ShellState::new(
        "glasshouse",
        "/Users/someone/projects/glasshouse",
        "0.1.0",
        vec![
            record("aaaaaaaaaaaa1", "claude-code", SessionLifecycle::Running),
            record("bbbbbbbbbbbb2", "codex", SessionLifecycle::Stopped),
        ],
    )
}

/// Render and flatten the buffer to text, so a test can assert on what the
/// user would actually see rather than only that drawing did not panic.
fn rendered(state: &ShellState, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render(state, frame))
        .expect("draw must not panic");
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Phase 1: "Display the active canonical project root prominently in the
/// TUI." Asserted against the rendered buffer, not against the state.
#[test]
fn the_project_root_is_displayed_on_every_frame() {
    let state = sample();
    let text = rendered(&state, 100, 24);
    assert!(
        text.contains("/Users/someone/projects/glasshouse"),
        "the canonical root must be on screen:\n{text}"
    );
    assert!(
        text.contains("glasshouse"),
        "the project name must be shown"
    );
}

/// It has to stay visible from every screen, including behind an overlay,
/// or "prominently" would mean "until you open something".
#[test]
fn the_project_root_stays_visible_while_an_overlay_is_open() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    let text = rendered(&state, 100, 24);
    assert!(
        text.contains("/Users/someone/projects/glasshouse"),
        "the root must survive an overlay:\n{text}"
    );
}

/// The row the root is drawn on, so an assertion about the root cannot be
/// satisfied by the same word appearing in the title bar.
fn root_row(state: &ShellState, width: u16, height: u16) -> String {
    rendered(state, width, height)
        .lines()
        .nth(1)
        .expect("the root occupies the second row")
        .trim_end()
        .to_owned()
}

/// A narrow terminal must keep the identifying tail of the path, not the
/// useless head.
///
/// Asserted against the root row alone. An earlier version searched the
/// whole frame, which meant "glasshouse" matched the title bar and the test
/// passed even when the path was truncated from the wrong end.
#[test]
fn a_narrow_terminal_keeps_the_end_of_the_project_root() {
    let row = root_row(&sample(), 28, 12);
    assert!(row.contains('…'), "truncation must be visible: `{row}`");
    assert!(
        row.ends_with("glasshouse"),
        "the tail identifies the project and must survive: `{row}`"
    );
    assert!(
        !row.contains("/Users/someone"),
        "the head should have been dropped, not the tail: `{row}`"
    );
}

/// A wide terminal shows the root untouched — otherwise the test above
/// could be satisfied by always truncating.
#[test]
fn a_wide_terminal_shows_the_whole_project_root() {
    let row = root_row(&sample(), 120, 24);
    assert_eq!(row, "root /Users/someone/projects/glasshouse");
}

#[test]
fn the_session_bar_lists_every_known_session() {
    let text = rendered(&sample(), 100, 24);
    assert!(
        text.contains("claude-code"),
        "missing first session:\n{text}"
    );
    assert!(text.contains("codex"), "missing second session:\n{text}");
}

#[test]
fn an_empty_project_says_so_instead_of_showing_an_empty_bar() {
    let state = ShellState::new("p", "/p", "0.1.0", Vec::new());
    let text = rendered(&state, 80, 24);
    assert!(text.contains("no sessions yet"), "got:\n{text}");
    assert!(text.contains("No session is active"), "got:\n{text}");
}

#[test]
fn the_overview_shows_detail_the_session_bar_has_no_room_for() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    let text = rendered(&state, 100, 24);
    assert!(text.contains("HARNESS"), "overview header missing:\n{text}");
    assert!(
        text.contains("PRESENTED"),
        "overview header missing:\n{text}"
    );
    // A stopped session with no native identifier is over, not resumable.
    assert!(text.contains("closed"), "disposition missing:\n{text}");
}

/// The overview marks two different things, and confusing them would make
/// it useless for its own capability: `>` is the row an interrupt or a
/// sent line acts on, and `(viewport)` is the session the shell is
/// showing. This drives them apart and checks both are visible at once.
#[test]
fn the_overview_distinguishes_the_row_it_acts_on_from_the_one_on_screen() {
    let mut state = two_live_sessions();
    state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

    // Matched on the identifier *and* a column only the overview draws:
    // the top bar names the presented session too, and it is the first
    // line in the frame.
    let text = rendered(&state, 120, 24);
    let row = |needle: &str| {
        text.lines()
            .find(|line| line.contains(needle) && line.contains("embedded"))
            .unwrap_or_else(|| panic!("no overview row for {needle}:\n{text}"))
    };
    let cursor_row = row("bbbbbbbbbbbb");
    let viewport_row = row("aaaaaaaaaaaa");

    assert!(
        cursor_row.contains("> codex"),
        "the cursor row must be marked:\n{text}"
    );
    assert!(
        viewport_row.contains("(viewport)"),
        "the presented session must say so:\n{text}"
    );
    assert!(
        !viewport_row.contains("> claude-code"),
        "the cursor is not on the presented session here:\n{text}"
    );
    assert!(
        !cursor_row.contains("(viewport)"),
        "the session under the cursor is not the one on screen:\n{text}"
    );
}

/// Two *live* sessions, unlike [`sample`], whose second one has stopped:
/// every overview action is refused against a session that is not
/// running, so a stopped fixture would prove nothing about the actions.
fn two_live_sessions() -> ShellState {
    ShellState::new(
        "glasshouse",
        "/Users/someone/projects/glasshouse",
        "0.1.0",
        vec![
            record("aaaaaaaaaaaa1", "claude-code", SessionLifecycle::Running),
            record("bbbbbbbbbbbb2", "codex", SessionLifecycle::Running),
        ],
    )
}

/// A refusal about a row in the overview must be readable at an ordinary
/// terminal width — identifier and state both. The footer clips a note
/// once the bindings have had their share, which is why the overview
/// draws it inside the popup as well.
#[test]
fn a_refusal_is_readable_inside_the_overview_at_a_hundred_columns() {
    let mut state = ShellState::new(
        "glasshouse",
        "/Users/someone/projects/glasshouse",
        "0.1.0",
        vec![
            record("aaaaaaaaaaaa1", "claude-code", SessionLifecycle::Running),
            record("bbbbbbbbbbbb2", "codex", SessionLifecycle::Stopped),
        ],
    );
    state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));

    let text = rendered(&state, 100, 30);
    let note = text
        .lines()
        .find(|line| line.contains("cannot interrupt"))
        .unwrap_or_else(|| panic!("the refusal never reached the screen:\n{text}"));
    assert!(
        note.contains("bbbbbbbbbbbb"),
        "the refusal must still name the session at 100 columns: `{note}`"
    );
    assert!(
        note.contains("stopped"),
        "the refusal must still name the state at 100 columns: `{note}`"
    );
}

/// The field for sending a line names the session it is aimed at. A field
/// that hides its own target is how text ends up in the wrong session.
#[test]
fn the_send_field_names_the_session_it_is_aimed_at() {
    let mut state = two_live_sessions();
    state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
    for c in "run the tests".chars() {
        state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    let text = rendered(&state, 120, 30);
    assert!(
        text.contains("send to bbbbbbbbbbbb"),
        "the field must name its target:\n{text}"
    );
    assert!(
        text.contains("run the tests"),
        "the typed line must be visible:\n{text}"
    );
    assert!(
        text.contains("enter sends"),
        "the field must say how to send it:\n{text}"
    );
}

/// A headless session's screen must never reach the viewport, even when
/// a grid for it is sitting in the state — which is exactly what happens
/// on the frame drawn between the bar moving onto it and the next tick
/// rebuilding the grid.
///
/// Rendered wide as well as narrow: an absence assertion against a
/// truncated frame is trivially true, which this project has already paid
/// for once (practice §17).
#[test]
fn a_headless_sessions_screen_never_reaches_the_viewport() {
    let mut headless = record("hidden-one", "claude-code", SessionLifecycle::Running);
    headless.presentation = SessionPresentation::Headless;
    let mut state = ShellState::new("p", "/p", "0.1.0", vec![headless]);
    state.set_viewport_grid(grid_from_lines(&["HEADLESS-SCREEN-BYTES"]));

    for width in [100, 400] {
        let text = rendered(&state, width, 24);
        assert!(
            !text.contains("HEADLESS-SCREEN-BYTES"),
            "a headless session drew into the viewport at {width} columns:\n{text}"
        );
        assert!(
            text.contains("headless"),
            "and the viewport must say why it is empty:\n{text}"
        );
    }
}

/// The same grid, on an embedded session, *is* drawn — without this the
/// absence above would pass for a renderer that draws nothing at all.
#[test]
fn an_embedded_sessions_screen_does_reach_the_viewport() {
    let mut state = ShellState::new(
        "p",
        "/p",
        "0.1.0",
        vec![record(
            "shown-one",
            "claude-code",
            SessionLifecycle::Running,
        )],
    );
    state.set_viewport_grid(grid_from_lines(&["HEADLESS-SCREEN-BYTES"]));

    let text = rendered(&state, 400, 24);
    assert!(
        text.contains("HEADLESS-SCREEN-BYTES"),
        "an embedded session's screen must be drawn:\n{text}"
    );
}

/// The keys are only discoverable if the footer says they exist.
#[test]
fn the_footer_names_the_overview_and_headless_keys() {
    let mut state = sample();
    // 24 rows, not 10: the band is a quarter of the screen, and since the bar
    // became ranked a ten-row terminal affords one bar row, which holds the
    // primary run and the head of the demoted one. `N headless` is at the
    // demoted run's tail by design — `tui-actionables.md`'s measured priority
    // — so the width that used to reach it no longer does, and the property
    // this test is about is that a normal terminal still names it.
    assert!(
        footer(&state, 120, 24).contains("N headless"),
        "control mode must offer the headless key"
    );

    state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    let row = footer(&state, 120, 10);
    assert!(row.contains("m send text"), "got {row:?}");
    assert!(row.contains("c interrupt"), "got {row:?}");
}

/// Ratatui will happily panic on a zero-sized or one-cell area if any
/// layout maths underflows. None here does, and this proves it.
#[test]
fn renders_without_panicking_at_absurd_sizes() {
    let mut state = sample();
    for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
        rendered(&state, w, h);
    }
    state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
        rendered(&state, w, h);
    }
    // The send field adds rows under the table, which is exactly the kind
    // of growth that overruns a popup nobody re-measured.
    state.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
    for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
        rendered(&state, w, h);
    }

    // A session's own layout with a note showing: the badge over the
    // viewport's top row takes a row out of an area that may not have one.
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    state.set_status("resuming `abc123` …");
    assert_eq!(state.chrome(), Chrome::Header);
    for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
        rendered(&state, w, h);
    }
}

#[test]
fn truncate_start_keeps_the_tail_and_counts_characters() {
    assert_eq!(truncate_start("/a/b/c", 10), "/a/b/c");
    assert_eq!(truncate_start("/a/b/c", 6), "/a/b/c");
    assert_eq!(truncate_start("/a/b/c", 4), "…b/c");
    assert_eq!(truncate_start("/a/b/c", 1), "…");
    assert_eq!(truncate_start("/a/b/c", 0), "");
    // Multi-byte characters must not be cut in half.
    let text = truncate_start("/päth/tö/pröject", 8);
    assert_eq!(text.chars().count(), 8, "counted characters, not bytes");
    assert!(text.ends_with("öject"));
}

#[test]
fn wrapped_row_count_matches_simple_word_wrap() {
    assert_eq!(wrapped_row_count("hello world", 20), 1);
    assert_eq!(
        wrapped_row_count("hello world", 5),
        2,
        "each word wraps to its own row"
    );
    assert_eq!(
        wrapped_row_count("", 20),
        1,
        "an empty line still occupies one row"
    );
    assert_eq!(
        wrapped_row_count("a b c d", 3),
        2,
        "\"a b\" then \"c d\" at width 3"
    );
    assert_eq!(
        wrapped_row_count("a b c d", 1000),
        1,
        "a generous width never wraps"
    );
    // Never panics or loops forever even at the narrowest width.
    assert!(wrapped_row_count("a fairly long sentence indeed", 0) > 0);
    assert!(wrapped_row_count("a fairly long sentence indeed", 1) > 0);
}

#[test]
fn wrapped_height_sums_every_lines_own_row_count() {
    let lines = vec![
        Line::from("hello world"),
        Line::from("a somewhat longer second line here"),
    ];
    let narrow = wrapped_height(&lines, 5);
    let wide = wrapped_height(&lines, 200);
    assert_eq!(wide, 2, "a generous width needs exactly one row per line");
    assert!(
        narrow > wide,
        "a narrower width must never need fewer rows: narrow={narrow} wide={wide}"
    );
    assert_eq!(wrapped_height(&[], 80), 0, "no lines needs no height");
}

/// The regression this function exists to prevent: a long label that
/// wraps by itself must not leave the error line beneath it with no
/// room, the way a fixed `2` did before `wrapped_height` replaced it —
/// found by driving the real binary, not by a unit test.
#[test]
fn wrapped_height_leaves_room_for_a_wrapped_label_and_its_error_line() {
    let label_line = Line::from(
        "Harness for `custom` (claude-code, codex, antigravity, opencode, cursor, pi, \
             hermes): not-a-real-harness_",
    );
    let error_line = Line::from(
        "`not-a-real-harness` is not a harness Glasshouse knows; known harnesses are: \
             claude-code, codex, antigravity, opencode, cursor, pi, hermes",
    );
    let width = 88;
    let height = wrapped_height(&[label_line.clone(), error_line.clone()], width);
    let label_rows = wrapped_row_count(&String::from(label_line), width);
    let error_rows = wrapped_row_count(&String::from(error_line), width);
    assert!(label_rows > 1, "the label alone must wrap in this scenario");
    assert_eq!(
        height as usize,
        label_rows + error_rows,
        "the error line's rows must be included, not clipped by a fixed height"
    );
}

/// The bottom bar carries the key bindings on every screen, including from
/// inside an overlay where the bindings change.
#[test]
fn the_status_bar_always_shows_the_key_bindings() {
    let mut state = sample();
    // 132, not 120: Phase 25's `k knowledge` binding took the row past a
    // hundred columns, the same trade recorded on
    // `the_status_bar_shows_a_note_next_to_the_bindings` below; map line
    // 234's `M memory` pushed it past 120 in turn, so this became 132.
    // Phase 47 line 1765's `h health` took the whole row to exactly 140
    // columns, so this was 142; `d decisions` adds fourteen more, taking
    // it to exactly 154, so this was 156; the last toggle in the bar adds
    // fourteen more again, taking it to exactly 168 — measured against the
    // row, not guessed.
    let bottom = footer(&state, 170, 24);
    assert!(bottom.contains("tab"), "bindings missing: `{bottom}`");
    assert!(bottom.contains("overview"), "bindings missing: `{bottom}`");
    assert!(bottom.contains("quit"), "bindings missing: `{bottom}`");

    state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    let bottom = footer(&state, 170, 24);
    assert!(
        bottom.contains("esc") && bottom.contains("quit"),
        "the overlay's bindings must be shown too: `{bottom}`"
    );
}

/// Feedback and the primary navigation keys both fit a normal terminal.
///
/// Beside each other once, under each other now: the note has a full-width
/// row at the foot of the band and the bar has every column above it. The
/// assertion is unchanged and the reason it holds is stronger — the note
/// cannot displace a binding because it is not on the same row as one.
#[test]
fn the_status_bar_shows_a_note_next_to_the_bindings() {
    let mut state = ShellState::new("p", "/p", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let bottom = footer(&state, 80, 24);
    assert!(
        bottom.contains("this project has only one session"),
        "the note must reach the status bar, whole: `{bottom}`"
    );
    assert!(
        bottom.contains("tab"),
        "the note must not displace the bindings: `{bottom}`"
    );
}

/// On a short terminal the bindings win: they are needed permanently, the
/// note only once.
///
/// The rule that decides it moved from the width to the height, because the
/// note moved from beside the bindings to under them. A band with a single
/// row is all `hotspot::control_band_rows`' quarter-of-the-screen clamp
/// allows below eight rows, and `split_band` spends that row on the bar. The
/// old form of this test asserted the same thing about a thirty-column
/// terminal, where the note is now merely clipped by the width — which would
/// have kept passing while proving nothing.
#[test]
fn a_note_is_dropped_rather_than_crowding_out_the_bindings() {
    let mut state = ShellState::new("p", "/p", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let band = regions(Rect::new(0, 0, 80, 6))[4];
    assert_eq!(band.height, 1, "a six-row terminal affords one footer row");
    let bottom = footer(&state, 80, 6);
    assert!(
        !bottom.contains("only one session"),
        "there was no row for the note: `{bottom}`"
    );
    // `new`, not `tab`: the bar is ranked now, so the one row a six-row
    // terminal affords holds the top of the ranking rather than its head in
    // declaration order. What this test is about is unchanged — the note is
    // dropped and the bindings are not — and asserting it against the pill a
    // user in this state most needs is strictly the better claim.
    assert!(
        bottom.contains("n new"),
        "bindings must survive: `{bottom}`"
    );
}

// -----------------------------------------------------------------
// Buttons: what is drawn, and what clicking it does.
// -----------------------------------------------------------------

/// Render one frame and hand back both the text and what was clickable.
fn recorded(state: &ShellState, width: u16, height: u16) -> (String, Vec<Hotspot>) {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut sink = Vec::new();
    terminal
        .draw(|frame| render_recording(state, frame, &mut sink))
        .expect("draw must not panic");
    let buffer = terminal.backend().buffer();
    let text = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, sink)
}

/// The pill whose painted cells contain `needle`.
///
/// Found by reading the frame rather than by index, so the assertion is about
/// the button the user can see rather than about the table behind it.
fn pill_at(state: &ShellState, width: u16, height: u16, needle: &str) -> Hotspot {
    let (text, spots) = recorded(state, width, height);
    let rows: Vec<&str> = text.lines().collect();
    for spot in &spots {
        let rect = spot.rect();
        let painted: String = rows[usize::from(rect.y)]
            .chars()
            .skip(usize::from(rect.x))
            .take(usize::from(rect.width))
            .collect();
        if painted.contains(needle) {
            return spot.clone();
        }
    }
    panic!("no pill containing {needle:?} was drawn:\n{text}");
}

/// The `Action` `state` answers a run of keys with — the last one's, which is
/// the one the run loop acts on. Takes the state by value because a
/// `ShellState` is deliberately not `Clone`: each caller builds its own.
fn action_of(mut state: ShellState, keys: &[KeyEvent]) -> Action {
    let mut action = Action::None;
    for key in keys {
        action = state.handle_key(*key);
    }
    action
}

/// `sample()` with the harness picker open on two options, cursor on the
/// first — the overlay this file clicks a row inside.
fn picking() -> ShellState {
    let mut state = sample();
    state.open_harness_choice(
        vec![IntegrationId::ClaudeCode, IntegrationId::Codex],
        SessionPresentation::Embedded,
    );
    state
}

/// **The regression the 168-column footer caused, and the one that came back
/// through the note.**
///
/// Fifteen actions were listed in one `Paragraph` with no `.wrap()`, so at 80
/// columns the row stopped after `q quit` and eight of them — settings,
/// memory, project, knowledge, events, routes, health, decisions — were drawn
/// nowhere at all, with no ellipsis and nothing to say they existed. Every one
/// of them is asserted by name, at the width a normal terminal actually is.
///
/// **With a note showing as well, which is the state it silently failed in.**
/// The wrapped bar's rows were reserved from the terminal's full width and
/// then painted into a `keys_area` up to half of it, so a note put the bar
/// back over its rows: measured at 80x24 after `n`, eight actions were drawn
/// and `M memory`, `p project`, `k knowledge`, `e events`, `r routes`,
/// `h health` and `d decisions` were on no screen again. A note now has a row
/// of its own and the bar always has the whole width.
#[test]
fn every_control_mode_action_is_on_screen_at_eighty_columns() {
    for note in [None, Some(MEASURED_NOTES[2])] {
        let mut state = sample();
        if let Some(note) = note {
            state.set_status(note);
        }
        let band = footer(&state, 80, 24);
        for action in CONTROL_ACTIONS {
            assert!(
                band.contains(action),
                "`{action}` must be drawn at 80 columns with note {note:?}:\n{band}"
            );
        }
    }
}

/// The other half of the same defect: an action that is drawn but recorded no
/// hotspot is a pill the user can read and cannot press.
///
/// `hotspot::render_bar` returned before *both* painting a pill and pushing
/// its hotspot, so the seven actions the note pushed off the band were
/// invisible and unclickable together. Asserted both ways round: every action
/// has a hotspot over the cells that spell it — [`pill_at`] reads the frame,
/// so a hotspot somewhere else does not count — and the footer band records
/// exactly as many hotspots as there are actions, so none is recorded twice
/// or left over from a pill that is no longer there.
#[test]
fn every_control_mode_action_is_clickable_at_eighty_columns() {
    for note in [None, Some(MEASURED_NOTES[0])] {
        let mut state = sample();
        if let Some(note) = note {
            state.set_status(note);
        }
        for action in CONTROL_ACTIONS {
            let spot = pill_at(&state, 80, 24, action);
            assert!(
                !spot.keys().is_empty(),
                "`{action}` is drawn as a button, so pressing it must do something"
            );
        }

        let band = regions(Rect::new(0, 0, 80, 24))[4];
        let (text, spots) = recorded(&state, 80, 24);
        let in_band = spots
            .iter()
            .filter(|spot| spot.rect().y >= band.y && spot.rect().y < band.bottom())
            .count();
        assert_eq!(
            in_band,
            CONTROL_ACTIONS.len(),
            "the footer band must record one hotspot per action with note {note:?}:\n{text}"
        );
    }
}

/// **A note is readable to its last word**, which is the word it exists for.
///
/// The note was clamped to `area.width / 2` — forty columns at eighty — and
/// every one of these was cut exactly where its answer began: `exited — back
/// i`, `: i`, `— Enter`. Being returned to control mode with no explanation
/// is the same complaint as not being returned at all.
#[test]
fn a_status_note_is_readable_to_its_last_word_at_eighty_columns() {
    for note in MEASURED_NOTES {
        let mut state = sample();
        state.set_status(note);
        let band = footer(&state, 80, 24);
        assert!(
            band.contains(note),
            "the note must be drawn whole at 80 columns:\n{band}"
        );
    }
}

/// The property that makes the two tests above hold by construction: the note
/// has a row of its own, so it takes nothing from the bar. The pill rows are
/// byte-identical with and without one, and the band is the same height —
/// which is also what stops a note resizing the harness's pseudo-terminal.
///
/// **The note's row is the band's first, not its last.** The pills own the
/// band's foot in every state, because the band's foot is the terminal's last
/// row and a row nothing paints is a row the terminal is never told about —
/// see [`the_footer_bands_last_row_is_painted_in_every_state`], which is the
/// regression, and `hotspot::split_band`, which is where the flip lives.
#[test]
fn a_note_costs_the_action_bar_no_column_and_the_harness_no_row() {
    let plain = footer(&sample(), 80, 24);
    let mut noted = sample();
    noted.set_status(MEASURED_NOTES[1]);
    let with_note = footer(&noted, 80, 24);

    // The reserved band is one row taller than the pills need, in both
    // states — that spare row is the note's, and reserving it whether or not
    // a note is showing is what keeps `viewport_slot` a function of the
    // terminal rather than of the last keystroke.
    let band_rows: Vec<&str> = plain.lines().collect();
    assert_eq!(
        regions(Rect::new(0, 0, 80, 24))[4].height,
        u16::try_from(band_rows.len()).expect("small"),
        "the band is the pills' rows plus the note's:\n{plain}"
    );
    assert_eq!(
        band_rows.first().copied(),
        Some(""),
        "the spare row is the band's first, and holds nothing when there is \
         no note:\n{plain}"
    );

    let noted_rows: Vec<&str> = with_note.lines().collect();
    assert_eq!(
        noted_rows[1..],
        band_rows[1..],
        "the pills must be drawn identically:\n{plain}\n---\n{with_note}"
    );
    assert_eq!(
        noted_rows.first().copied(),
        Some(format!("  {}", MEASURED_NOTES[1]).as_str()),
        "and the note takes the row above them"
    );
}

/// **The terminal's last row is painted in every state the footer has**, which
/// is what makes a resize visible at all.
///
/// The band became the bar's rows plus the note's, and the note's row was the
/// band's last — so with no note showing, which is the ordinary state, nothing
/// wrote the bottom row of the terminal. ratatui's diff emits nothing for a
/// row of default-styled spaces, so that row is not merely blank: it is a row
/// the terminal is never addressed at.
/// `terminal_loss::a_resize_still_arrives_on_a_terminal_that_has_been_silent`
/// reads exactly that — measured, a terminal resized to 100x50 was redrawn
/// with its pills on rows 47–49 and `\x1b[50;` never sent — and calls it,
/// correctly, an interface drawing itself at a size its terminal no longer is.
///
/// Held here as well as there because this is the cheap half: no pty, no
/// child, and it names the state that broke it. Every state of the footer is
/// swept, because the defect was invisible in exactly the one nobody had put
/// a note in.
#[test]
fn the_footer_bands_last_row_is_painted_in_every_state() {
    for (width, height) in [(100u16, 50u16), (80, 24), (120, 30)] {
        for note in [None, Some(MEASURED_NOTES[1])] {
            for overlay in [false, true] {
                let mut state = sample();
                if overlay {
                    state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
                }
                if let Some(note) = note {
                    state.set_status(note);
                }
                let text = rendered(&state, width, height);
                let last = text.lines().last().expect("a frame has rows").to_owned();
                assert!(
                    !last.trim().is_empty(),
                    "nothing painted the last row of a {width}x{height} terminal with note \
                     {note:?} and overlay {overlay}, so the terminal is never told the \
                     interface reaches it:\n{text}"
                );
                assert_eq!(
                    u16::try_from(text.lines().count()).expect("small"),
                    height,
                    "the frame is the terminal's own height"
                );
            }
        }
    }
}

/// **The key that closes the Settings overlay is on the screen at the widths a
/// terminal actually is.**
///
/// The hint was one unwrapped 97-column line: at 80 columns it stopped inside
/// `r setup` and ` esc close` was drawn nowhere — and `esc` is the only key
/// that closes this overlay, so the single way out was the thing the clip
/// hid. Below 70 columns `w save` went with it, which leaves a user who has
/// edited settings unable to discover either the way to keep them or the way
/// out. Asserted on the whole frame rather than on the band, because what
/// matters is that the user can read it.
#[test]
fn the_settings_overlay_shows_the_keys_that_save_and_close_it() {
    let mut state = sample();
    state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for width in [40u16, 60, 70, 80, 100] {
        let text = rendered(&state, width, 24);
        for key in ["esc close", "w save", "tab section"] {
            assert!(
                text.contains(key),
                "`{key}` must be readable at {width} columns:\n{text}"
            );
        }
    }
}

/// The note an overlay's own keys set is readable while that overlay is still
/// open, and costs nothing the hint needed.
///
/// `w` inside Settings answers with `saved to user configuration` and leaves
/// the overlay up, so that note is a settings save's only acknowledgement.
/// The band's first row is under the popup — measured at 80x24, the `Clear`
/// covers the band's first two rows — which is why the note moves to the
/// band's foot for exactly this state, and why the hint moves up a row
/// instead of losing one.
#[test]
fn a_note_set_from_inside_an_overlay_is_not_drawn_under_it() {
    let mut state = sample();
    state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
    state.set_status("saved to user configuration");
    let text = rendered(&state, 80, 24);
    assert!(
        text.contains("saved to user configuration"),
        "the note must be readable with the overlay it was set from still \
         open:\n{text}"
    );
    assert!(
        text.contains("esc close"),
        "and it must not cost the hint the row carrying the way out:\n{text}"
    );
}

/// Session mode keeps Glasshouse chrome clickable without marking any child
/// cell as a Glasshouse hotspot. The run loop changes keyboard ownership
/// before replaying one of these keys, so a tab click cannot become a Tab in
/// the child.
#[test]
fn session_mode_records_only_header_hotspots() {
    let mut state = sample();
    let (_, control) = recorded(&state, 100, 24);
    assert!(!control.is_empty(), "control mode has pills to click");

    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(state.mode(), Mode::Session);
    let (text, spots) = recorded(&state, 200, 24);
    assert!(
        text.contains("claude-code"),
        "the tab strip is still drawn in session mode:\n{text}"
    );
    assert!(
        !spots.is_empty(),
        "the session header keeps its tab actions"
    );
    assert!(
        spots.iter().all(|spot| spot.rect().bottom() <= 1),
        "no child viewport cell may become Glasshouse chrome:\n{text}"
    );
}

/// **The strip pans to the selected tab, and says what it is hiding.**
///
/// A tab pill is 28 columns and the strip is one row, so at eighty columns
/// two of five sessions are on it. Rewritten onto the wrapping bar, the strip
/// laid out from index 0 every time and wrapped the rest onto a row that does
/// not exist: the cursor moved to the third session, the viewport switched to
/// it, and the strip still showed the first two with the focus marker on
/// neither and nothing marking the overflow.
#[test]
fn the_tab_strip_pans_to_keep_the_selected_session_on_screen() {
    let mut state = five_sessions();
    let row = strip(&state, 80, 24);
    assert!(
        row.contains("1 claude-code"),
        "the strip starts at the first tab: `{row}`"
    );
    assert!(
        row.contains('›'),
        "the three tabs past the edge must be marked: `{row}`"
    );

    // `▸` is the focus marker, so this asserts the strip shows the tab the
    // viewport switched to — not merely that its number is somewhere.
    state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.selected_index(), 1);
    let row = strip(&state, 80, 24);
    assert!(
        row.contains("▸ 2 claude-code"),
        "the selected tab must be on screen: `{row}`"
    );

    for _ in 0..3 {
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }
    assert_eq!(state.selected_index(), 4);
    let row = strip(&state, 80, 24);
    assert!(
        row.contains("▸ 5 claude-code"),
        "the strip must pan to the last tab: `{row}`"
    );
    assert!(
        row.contains('‹'),
        "and say the tabs it panned past are still there: `{row}`"
    );
}

/// A panned tab is still the keyboard path: clicking the one the strip
/// scrolled to must be the `Tab` presses that reach it, not the presses that
/// would reach whatever used to be drawn in those cells.
#[test]
fn clicking_a_panned_tab_reaches_the_session_it_names() {
    let mut state = five_sessions();
    for _ in 0..4 {
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }
    assert_eq!(state.selected_index(), 4);

    let fourth = pill_at(&state, 80, 24, "4 claude-code");
    assert_eq!(
        fourth.keys(),
        [KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE)],
        "the tab before the selected one is one BackTab away"
    );
    for key in fourth.keys() {
        state.handle_key(*key);
    }
    assert_eq!(
        state.selected_index(),
        3,
        "clicking it must present the session it names"
    );
}

/// A toggle in the bar states the value it toggles, so the state is readable
/// without pressing the key and watching a status note that the next keystroke
/// erases.
///
/// `f fullscreen` was that toggle and is gone with the mode it armed — user
/// ruling 2026-09-09, a session keeps its header — so the rule is asserted on
/// the toggle that took its place. **The bar must offer no key that does
/// nothing**, which is the second half: a pill for a removed binding would be
/// a button that lies.
#[test]
fn the_footer_states_the_value_of_the_toggle_it_draws() {
    let mut state = sample();
    assert!(
        footer(&state, 100, 24).contains("t theme: neon"),
        "the palette in force must be readable from the bar:\n{}",
        footer(&state, 100, 24)
    );

    state.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(
        footer(&state, 100, 24).contains("t theme: amber"),
        "and pressing it must move the value on screen:\n{}",
        footer(&state, 100, 24)
    );

    assert!(
        !footer(&state, 170, 24).contains("fullscreen"),
        "no pill may advertise a key the shell no longer answers:\n{}",
        footer(&state, 170, 24)
    );
}

/// **A click does exactly what the key does, because it *is* the key.**
///
/// Three pills in three different surfaces — the footer bar, the session tab
/// strip, and a row inside an overlay — each clicked at the middle of the
/// rectangle it was drawn into. The assertion is not "something happened": it
/// is that the `Action` matches the one the advertised keyboard path produces
/// on an identical state.
#[test]
fn clicking_a_pill_produces_the_action_its_key_produces() {
    // --- the footer bar --------------------------------------------------
    let settings = pill_at(&sample(), 100, 24, "s settings");
    assert_eq!(
        action_of(sample(), settings.keys()),
        action_of(
            sample(),
            &[KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)]
        ),
        "clicking `s settings` must be pressing `s`"
    );
    assert_eq!(
        action_of(sample(), settings.keys()),
        Action::OpenSettings,
        "and that is what `s` does"
    );

    // --- the session tab strip -------------------------------------------
    // `sample()` has two sessions and starts on the first; clicking the
    // second is `Tab`, and it must land on the second session.
    let second = pill_at(&sample(), 100, 24, "2 codex");
    assert_eq!(
        second.keys(),
        [KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)],
        "the strip's second tab is one Tab away"
    );
    let mut clicked = sample();
    for key in second.keys() {
        clicked.handle_key(*key);
    }
    assert_eq!(
        clicked.selected_index(),
        1,
        "clicking the second tab must present the second session"
    );

    // --- a row inside an overlay -----------------------------------------
    let row = pill_at(&picking(), 100, 24, "Codex");
    let by_keyboard = [
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    ];
    assert_eq!(row.keys(), by_keyboard, "the row is down-then-enter");
    assert_eq!(
        action_of(picking(), row.keys()),
        action_of(picking(), &by_keyboard),
        "and clicking it starts the harness the cursor would have started"
    );
    assert!(
        matches!(
            action_of(picking(), row.keys()),
            Action::StartSessionWith { .. }
        ),
        "which is a start, not a redraw"
    );
}

/// The bound check, which is the whole of `hotspot::hit`.
///
/// A click on the project root band, on the wordmark, and on the viewport
/// must all do nothing — the alternative, answering with whichever pill is
/// nearest, turns unrelated parts of the screen into buttons.
#[test]
fn a_click_outside_every_pill_does_nothing() {
    let state = sample();
    let (_, spots) = recorded(&state, 100, 24);
    assert!(!spots.is_empty(), "there are pills to miss");

    for (column, row) in [(0u16, 0u16), (60, 1), (50, 10), (99, 23), (99, 0)] {
        assert!(
            super::super::hotspot::hit(&spots, column, row).is_none(),
            "({column},{row}) is not on a pill, so it must not act"
        );
    }
}

/// Every recorded rectangle is one row tall, inside the frame, and does not
/// overlap its neighbours — the properties `hit` returning the *first* match
/// is only correct under.
#[test]
fn recorded_pills_never_overlap_and_never_leave_the_frame() {
    let state = sample();
    for (width, height) in [(40u16, 12u16), (80, 24), (100, 24), (200, 40)] {
        let (_, spots) = recorded(&state, width, height);
        for spot in &spots {
            let rect = spot.rect();
            assert_eq!(rect.height, 1, "a pill is one row: {rect:?}");
            assert!(
                rect.right() <= width && rect.bottom() <= height,
                "{rect:?} left the {width}x{height} frame"
            );
        }
        for (i, a) in spots.iter().enumerate() {
            for b in spots.iter().skip(i + 1) {
                let (a, b) = (a.rect(), b.rect());
                assert!(
                    a.y != b.y || a.right() <= b.x || b.right() <= a.x,
                    "{a:?} and {b:?} overlap at {width}x{height}"
                );
            }
        }
    }
}

/// "Keep the visual design text-first and avoid decorative graph
/// visualizations that do not expose actionable state." — map line 1771,
/// which this test proved for the shell and the session overview before
/// Phase 47's two overlays existed to add.
///
/// Enforced mechanically rather than by inspection: Ratatui's decorative
/// widgets — `Gauge`, `Sparkline`, `BarChart` — all draw with the Unicode
/// block elements, so a frame that contains none of those, on any screen,
/// cannot be rendering one. Box-drawing characters used for borders are a
/// different range and stay allowed.
#[test]
fn compact_views_and_diagnostic_overlays_keep_readable_text() {
    let mut state = sample();
    let mut screens = vec![rendered(&state, 100, 30)];
    state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    screens.push(rendered(&state, 100, 30));
    state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    screens.push(rendered(&state, 100, 30));

    // The two diagnostic overlays map line 1771 names by name — a
    // "knowledge-graph visualization" is exactly what a `Gauge` or
    // `Sparkline` would be reaching for here.
    state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    state.open_project_overview(Vec::new(), Vec::new(), 0, Vec::new(), String::new(), None);
    screens.push(rendered(&state, 100, 30));
    state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
    screens.push(rendered(&state, 100, 30));
    // Phase 25's own diagnostic overlay, map line 1107: a "decorative
    // node graph" is exactly the shape this sweep already guards
    // against for every other overlay.
    state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        None,
    );
    screens.push(rendered(&state, 100, 30));
    // Phase 47 line 1771 is a standing property, so every diagnostic
    // surface added afterwards has to re-prove it. This one is line
    // 1765's, and a "route health" display is exactly where somebody
    // would reach for a `Gauge`.
    state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    state.open_route_health(vec![crate::shell::state::RouteHealthRow {
        provider: "anyrouter".to_owned(),
        credential_label: "anyrouter/API_KEY".to_owned(),
        model: "claude-opus-4-1".to_owned(),
        consecutive_failures: 2,
        credential_rejected: false,
        available_now: true,
        cooling_down_until_unix: None,
        stated_limit: Some(300),
        stated_window_seconds: Some(60),
        quota_resets_at_unix: None,
        failure_domain: "unknown".to_owned(),
        failure_domain_peers: 0,
    }]);
    screens.push(rendered(&state, 100, 30));

    for screen in screens {
        if let Some(found) = screen.chars().find(|c| {
            // U+2580..U+259F: block elements, which is what gauges,
            // sparklines and bar charts are made of.
            ('\u{2580}'..='\u{259F}').contains(c)
        }) {
            panic!("a decorative block element ({found:?}) was drawn:\n{screen}");
        }
    }
}

// -----------------------------------------------------------------
// The viewport and the session-mode footer.
// -----------------------------------------------------------------

/// Build a plain, unstyled grid from lines of ASCII text, padding every
/// row to the widest one with blanks — good enough for rendering tests,
/// which are about placement and clipping, not about `vt100` conversion
/// (that lives in `shell::mod`'s own tests, against a real parser).
fn grid_from_lines(lines: &[&str]) -> ViewportGrid {
    let rows = lines.len() as u16;
    let cols = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0) as u16;
    let mut cells = Vec::with_capacity(usize::from(rows) * usize::from(cols));
    for line in lines {
        let chars: Vec<char> = line.chars().collect();
        for col in 0..usize::from(cols) {
            let ch = chars.get(col).copied().unwrap_or(' ');
            cells.push((ch.to_string(), Style::default()));
        }
    }
    ViewportGrid::new(rows, cols, cells, None)
}

#[test]
fn the_viewport_shows_the_set_grid_once_non_empty() {
    let mut state = sample();
    state.set_viewport_grid(grid_from_lines(&[
        "first line",
        "second line",
        "third line",
    ]));
    let text = rendered(&state, 100, 24);
    assert!(text.contains("first line"), "got:\n{text}");
    assert!(text.contains("second line"), "got:\n{text}");
    assert!(text.contains("third line"), "got:\n{text}");
    assert!(
        !text.contains("This viewport is reserved"),
        "the placeholder must not show once there is a live screen:\n{text}"
    );
}

/// A screen taller than the render area is clipped, not overflowed — the
/// top of the screen stays visible, since a `vt100` screen has no notion
/// of "the most recent lines" the way raw scrollback text did.
#[test]
fn a_grid_taller_than_the_area_is_clipped_not_overflowed() {
    let mut state = sample();
    let lines: Vec<String> = (0..50).map(|i| format!("line-{i}")).collect();
    let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
    state.set_viewport_grid(grid_from_lines(&borrowed));

    let text = rendered(&state, 100, 10);
    assert!(
        text.contains("line-0"),
        "the top of the screen must stay visible:\n{text}"
    );
}

/// "Keep the existing placeholder for the no-session case" — and for the
/// active-but-silent-so-far case too: the placeholder is only replaced
/// once a live session has a screen to show.
#[test]
fn an_empty_viewport_keeps_the_existing_placeholder() {
    let state = sample();
    assert!(state.viewport_grid().is_empty());
    let text = rendered(&state, 100, 24);
    assert!(text.contains("This viewport is reserved"), "got:\n{text}");
}

/// A live grid gets the whole viewport area with no border, so the
/// harness it belongs to is the dominant thing on screen; the
/// placeholder keeps its border since there is no harness screen yet to
/// compete with it.
#[test]
fn landing_and_live_viewport_leave_the_terminal_unboxed() {
    let mut state = sample();
    let placeholder = rendered(&state, 40, 10);
    assert!(
        !placeholder.contains('┌') && placeholder.contains("YOUR WORK, IN VIEW"),
        "the landing stays transparent and unboxed:\n{placeholder}"
    );

    state.set_viewport_grid(grid_from_lines(&["hello"]));
    let live = rendered(&state, 40, 10);
    assert!(
        !live.contains('┌'),
        "a live session's screen must not be boxed in:\n{live}"
    );
    assert!(live.contains("hello"), "got:\n{live}");
}

/// A grid full of real content, at sizes much smaller than the content,
/// must not panic the clipping that keeps it bounded to the area.
#[test]
fn the_viewport_does_not_panic_with_a_real_grid_at_absurd_sizes() {
    let mut state = sample();
    let lines: Vec<String> = (0..500)
        .map(|i| format!("line-{i}-{}", "x".repeat(i % 50)))
        .collect();
    let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
    state.set_viewport_grid(grid_from_lines(&borrowed));
    for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
        rendered(&state, w, h);
    }
}

/// A cursor position the emulator reports but that the render area
/// cannot contain (a resize race) must be ignored, not panic.
#[test]
fn a_cursor_outside_the_render_area_does_not_panic() {
    let mut state = sample();
    state.set_viewport_grid(ViewportGrid::new(
        1,
        1,
        vec![("x".to_owned(), Style::default())],
        Some((99, 99)),
    ));
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(state.mode(), Mode::Session);
    rendered(&state, 1, 1);
}

/// The design note: "A user who cannot see how to get out is the
/// failure this design exists to prevent" — so the escape chord is on
/// screen in session mode at all times. The *header* carries it now: the
/// status bar is one of the three rows the collapse hands the harness.
#[test]
fn the_header_names_the_escape_chord_in_session_mode() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(state.mode(), Mode::Session);

    let header = first_row(&state, 100, 24);
    assert!(
        header.contains(ESCAPE_CHORD),
        "the escape chord must always be on screen in session mode: `{header}`"
    );
}

/// Session mode's chrome is one line, and the row under it is the
/// harness's. Asserted on where the session's own first row lands, not on
/// the absence of the bands, because "the harness starts three rows
/// higher" is the whole of what this mode buys.
#[test]
fn session_mode_collapses_the_chrome_to_a_single_header() {
    let mut state = sample();
    state.set_viewport_grid(grid_from_lines(&["harness first row"]));

    let control = rendered(&state, 100, 24);
    let control_rows: Vec<&str> = control.lines().collect();
    assert!(
        control_rows[1].starts_with("root "),
        "control mode keeps its bands:\n{control}"
    );
    assert!(
        control_rows[3].starts_with("harness first row"),
        "control mode puts the session under three bands:\n{control}"
    );

    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(state.mode(), Mode::Session);
    let session = rendered(&state, 100, 24);
    let session_rows: Vec<&str> = session.lines().collect();
    assert!(
        session_rows[1].starts_with("harness first row"),
        "session mode puts the session directly under the header:\n{session}"
    );
    assert!(
        !session.contains("SESSION MODE"),
        "the status bar is gone with the rest of the chrome:\n{session}"
    );
}

/// The point of the collapse, in the one function the run loop asks: the
/// harness is told about the rows the chrome stopped taking. Nothing else
/// in the shell may compute that size — `shell::viewport_terminal_size`
/// funnels through here — so this is where it is pinned.
///
/// Not "three more rows" any more, and the number is derived rather than
/// written down: the collapse hands back the title, the root band, the tab
/// strip *and* however many rows the wrapped action bar took, which is a
/// function of the terminal's width. Writing a literal here would pin the
/// bar's shape rather than the collapse's promise.
#[test]
fn session_mode_hands_the_viewport_the_bands_control_mode_reserved() {
    let area = Rect::new(0, 0, 100, 24);
    let control = viewport_slot(area, Chrome::Full);
    let session = viewport_slot(area, Chrome::Header);
    let bands = 3 + super::super::hotspot::control_band_rows(area);
    assert!(bands >= 4, "control mode has at least four rows of chrome");
    assert_eq!(
        session.height,
        control.height + bands - 1,
        "control {control:?} against session {session:?}"
    );
    assert_eq!(
        session.width, control.width,
        "the harness already had the full width"
    );
    assert_eq!(
        viewport_terminal_size(TerminalSize::new(24, 100), Chrome::Header).rows,
        session.height,
        "the size handed to the pseudo-terminal must be the slot itself"
    );
}

/// A session's own layout reserves exactly one row, and it is the header.
///
/// The row is the whole trade the 2026-09-09 ruling makes: the mode that
/// reclaimed it (`Chrome::None`, armed by `f`) bought one line of harness and
/// took away the only thing on screen naming the way back. One row is what a
/// way out costs.
#[test]
fn a_session_layout_reserves_exactly_the_header() {
    let area = Rect::new(0, 0, 100, 24);
    let header = viewport_slot(area, Chrome::Header);
    assert_eq!(
        header.height,
        area.height - 1,
        "one row, and only one: {header:?} out of {area:?}"
    );
    assert_eq!(header.y, 1, "and it is the row at the top: {header:?}");
    assert_eq!(
        viewport_terminal_size(TerminalSize::new(24, 100), Chrome::Header),
        TerminalSize::new(23, 100),
        "the pseudo-terminal is told the slot, never the terminal's own size"
    );
}

/// **The regression the user hit.** *"while in fullscreen for claude code or
/// pane — session header up top should still be there."*
///
/// Asserted against the rendered buffer at every width a terminal can be,
/// because "the header is there" is a statement about what a user sees. The
/// header sheds fields as it narrows (`chrome::DROP_ORDER`) but never sheds
/// itself: at every width the frame's first row is Glasshouse's, and the
/// harness's own first row is the second.
#[test]
fn a_session_always_draws_the_header_at_every_width() {
    let mut state = sample();
    state.set_viewport_grid(grid_from_lines(&[
        "harness first row",
        "harness second row",
    ]));
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(state.chrome(), Chrome::Header);

    for width in [1, 8, 20, 40, 60, 80, 100, 170, 400] {
        // What the harness's own row looks like at this width — the row is
        // clipped, not wrapped, so a one-column terminal shows one character
        // of it.
        let harness: String = "harness first row"
            .chars()
            .take(usize::from(width))
            .collect();
        let text = rendered(&state, width, 24);
        let rows: Vec<&str> = text.lines().collect();
        assert!(
            !rows[0].starts_with(&harness),
            "at {width} columns the harness took the header's row:\n{text}"
        );
        assert!(
            rows[1].starts_with(&harness),
            "the harness must start on the row under the header at {width} columns:\n{text}"
        );
    }

    // And what the header keeps when it can keep only one thing is the way
    // out — never the branding.
    let narrow = first_row(&state, 20, 24);
    assert!(
        narrow.contains(ESCAPE_CHORD),
        "a narrow header still names the way out: `{narrow}`"
    );
}

/// The decision this layout makes: a session on screen always tells the user
/// how to leave it, and how to move the keyboard without leaving.
///
/// Both halves are asserted, because either alone is a trap — a chord nothing
/// names, or a name for a chord that does not work. The escape lands in the
/// fleet view with its five bands back; the focus chord lands on the header
/// with the session still drawn under it.
#[test]
fn a_user_inside_a_session_can_leave_it_or_focus_the_header() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(state.chrome(), Chrome::Header);

    let entered = first_row(&state, 100, 24);
    assert!(
        entered.contains(ESCAPE_CHORD),
        "the header must name the way out: `{entered}`"
    );
    assert!(
        entered.contains(FOCUS_CHORD),
        "and the chord that moves the keyboard to it: `{entered}`"
    );

    // The focus chord, spelled the way a real terminal delivers `0x1E`.
    state.handle_key(KeyEvent::new(KeyCode::Char('6'), KeyModifiers::CONTROL));
    assert_eq!(state.mode(), Mode::Control, "the header has the keyboard");
    assert_eq!(
        state.chrome(),
        Chrome::Header,
        "and the session is still on screen under it"
    );
    let focused = first_row(&state, 100, 24);
    assert!(
        focused.contains("session"),
        "a focused header names the way back into the session: `{focused}`"
    );

    // The escape chord, spelled the way a real terminal delivers `0x1D` —
    // `Ctrl-]` arrives as Ctrl plus `'5'`; see `is_session_escape`.
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(state.mode(), Mode::Session);
    state.handle_key(KeyEvent::new(KeyCode::Char('5'), KeyModifiers::CONTROL));
    assert_eq!(
        state.chrome(),
        Chrome::Full,
        "the fleet view comes back with the keyboard"
    );
    let text = rendered(&state, 170, 24);
    assert!(text.contains("GLASSHOUSE"), "the bands are back:\n{text}");
}

/// **The way out is chrome, so no keystroke can take it off the screen.**
///
/// This began as a fullscreen badge painted over the harness's own top row,
/// added after a user typed in a frame-free session and lost the one-shot note
/// that had named the escape — `handle_key` clears a status on the very next
/// key. The header is that guarantee now, and it is stronger: it is a band
/// rather than an overpaint, so nothing the harness draws can cover it either.
#[test]
fn the_way_out_survives_every_keystroke_in_a_session() {
    let mut state = sample();
    state.set_viewport_grid(grid_from_lines(&[
        "harness first row that is long enough to reach across the whole width",
    ]));
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(first_row(&state, 100, 24).contains(ESCAPE_CHORD));

    for key in ['x', 'y', 'z', 'q'] {
        state.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
    }
    let typed = first_row(&state, 100, 24);
    assert!(
        typed.contains(ESCAPE_CHORD),
        "the way out must still be on screen after typing: `{typed}`"
    );
    assert!(
        !typed.contains("harness first row"),
        "and the harness cannot draw over it: `{typed}`"
    );
}

/// The decision the header makes: its drop order.
///
/// At a width that cannot hold every field, the two a focused session
/// cannot be worked without survive — the tab strip is how another session
/// is reached, `ctrl-] back` is the only way out — and the branding is
/// what goes. Reorder `chrome::DROP_ORDER` so the wordmark outlives the
/// chord and this fails on the row it renders.
#[test]
fn a_narrow_header_keeps_the_way_out_and_drops_the_wordmark() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(state.mode(), Mode::Session);

    // 56, not 44: the exit field names two chords now — `ctrl-5` for the
    // keyboards `ctrl-]` is unreachable on — so it reserves nine more
    // columns, and the width at which the tab strip is still affordable
    // moved with it. The order it drops fields in is what this pins.
    let header = first_row(&state, 56, 12);
    assert!(
        header.contains(ESCAPE_CHORD),
        "the way out is never dropped: `{header}`"
    );
    assert!(
        header.contains("claude-code"),
        "the session tabs are never dropped: `{header}`"
    );
    assert!(
        !header.contains("GLASSHOUSE"),
        "the wordmark is the first field to go: `{header}`"
    );
}

/// **A note set while a session is on screen is readable**, or the key that
/// set it said nothing.
///
/// This layout has one line of chrome and it is spoken for, so a note has
/// nowhere of its own — and the note this matters most for is the one `Enter`
/// on a stopped session sets while its harness starts. It is painted over the
/// viewport's own top row, which costs no band: `viewport_slot` has already
/// handed the session every row below the header.
#[test]
fn a_note_is_readable_over_a_session_and_costs_it_no_row() {
    let mut state = sample();
    state.set_viewport_grid(grid_from_lines(&["harness first row"]));
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(state.chrome(), Chrome::Header);
    state.set_status("resuming `aaaaaaaaaaaa1` …");

    let text = rendered(&state, 80, 12);
    let rows: Vec<&str> = text.lines().collect();
    assert!(
        rows[1].contains("resuming `aaaaaaaaaaaa1`"),
        "the note must be on the row under the header:\n{text}"
    );
    assert!(
        rows[0].contains(ESCAPE_CHORD),
        "and it must not have taken the header's row:\n{text}"
    );
    assert_eq!(
        viewport_slot(Rect::new(0, 0, 80, 12), Chrome::Header).height,
        11,
        "the badge is painted over the session's rows, not given one"
    );
}

/// **The focus state is visible at every width, because it is the row's own
/// ground and not a word on it.**
///
/// The label says which way the chord would move the keyboard, and that is
/// the readable half — but the header sheds fields as it narrows and the
/// label is one of them, so at sixty columns a text-only signal would say
/// nothing at all. Asserted on the buffer's style rather than its symbols:
/// every cell of the header row changes ground when Glasshouse holds the
/// keyboard, and no cell of the viewport under it does.
#[test]
fn a_focused_header_is_visibly_focused_at_every_width() {
    let backgrounds = |state: &ShellState, width: u16| -> Vec<Option<Color>> {
        let backend = TestBackend::new(width, 6);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(state, frame))
            .expect("draw must not panic");
        let buffer = terminal.backend().buffer();
        (0..width).map(|x| buffer[(x, 0)].style().bg).collect()
    };

    for width in [40u16, 60, 80, 120] {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let resting = backgrounds(&state, width);

        state.handle_key(KeyEvent::new(KeyCode::Char('6'), KeyModifiers::CONTROL));
        assert!(state.header_focused());
        let focused = backgrounds(&state, width);

        assert_ne!(
            resting, focused,
            "a focused header must not look like a resting one at {width} columns"
        );
        assert!(
            focused.iter().all(|bg| bg.is_some()),
            "the whole row is lit, not the part a word happens to cover, \
             at {width} columns: {focused:?}"
        );
    }
}

/// The other half of the drop order: given the room, every field is drawn,
/// so the narrow case above is a header that dropped fields rather than a
/// header that never had them.
#[test]
fn a_wide_header_carries_the_wordmark_the_root_and_the_way_out() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let header = first_row(&state, 120, 12);
    for expected in [
        "GLASSHOUSE",
        "claude-code",
        "…/glasshouse",
        "ctrl-5 back",
        // The alias is the first field dropped and a 120-column header has
        // room for it — see `chrome::HeaderField::ExitAlias`.
        "ctrl-] back",
        // And the chord that moves the keyboard without leaving.
        "ctrl-6 header",
    ] {
        assert!(
            header.contains(expected),
            "a 120-column header has room for `{expected}`: `{header}`"
        );
    }
}

/// Control mode's own footer must not claim to be session mode.
#[test]
fn the_status_bar_shows_control_mode_bindings_by_default() {
    let state = sample();
    assert_eq!(state.mode(), Mode::Control);
    // 170, not 100 — see `the_status_bar_always_shows_the_key_bindings`.
    let bottom = footer(&state, 170, 24).to_lowercase();
    assert!(!bottom.contains("session mode"), "got: `{bottom}`");
    assert!(bottom.contains("quit"), "got: `{bottom}`");
}

/// The negative from practice §17: an empty activity list draws no
/// `ACTIVITY` heading at all, not an empty section — because an empty
/// section reads as "nothing has happened" rather than "nothing has been
/// observed yet".
#[test]
fn the_overview_draws_no_activity_heading_with_no_events() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    assert!(state.activity().is_empty());

    for (width, height) in [(100, 24), (400, 24)] {
        let text = rendered(&state, width, height);
        assert!(
            !text.contains("ACTIVITY"),
            "no events were recorded, so no heading should draw at width {width}:\n{text}"
        );
    }
}

/// Practice §17: a row can be truncated off-screen at a narrow width,
/// which makes a `contains` assertion pass or fail for reasons that have
/// nothing to do with the code — so this asserts at a realistic width
/// *and* at 400 columns.
#[test]
fn the_overview_shows_recorded_activity_at_a_realistic_and_a_wide_width() {
    use crate::events::{EventBus, LifecycleEvent, MessageOrigin, TurnOutcome};
    use crate::session::SessionId;
    use crate::shell::Action;

    let mut state = sample();
    let bus = EventBus::new();
    let session = SessionId::new("aaaaaaaaaaaa1");
    let recorded = vec![
        bus.publish(&session, LifecycleEvent::SessionStarted),
        bus.publish(
            &session,
            LifecycleEvent::TextDelivered {
                origin: MessageOrigin::Machine,
                bytes: 41,
            },
        ),
        bus.publish(
            &session,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
        ),
    ];
    assert_eq!(state.note_events(&recorded), Action::Redraw);
    state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));

    for (width, height) in [(100, 24), (400, 24)] {
        let text = rendered(&state, width, height);
        assert!(text.contains("ACTIVITY"), "width {width}:\n{text}");
        assert!(text.contains("session started"), "width {width}:\n{text}");
        assert!(
            text.contains("sent 41 bytes (machine)"),
            "width {width}:\n{text}"
        );
        assert!(
            text.contains("turn ended (completed)"),
            "width {width}:\n{text}"
        );
        assert!(
            text.contains(&super::super::state::short_session_id(&session)),
            "the session must be named beside its event, width {width}:\n{text}"
        );
    }
}

/// Phase 11 lines 682 and 684: a session's name/purpose and its last
/// activity time, both already on [`SessionRecord`] since Phase 10, shown
/// in the overview table rather than nowhere.
///
/// Asserted at a realistic width and a wide one — see §17. "Realistic"
/// here is wider than the 100 columns the activity-feed test above uses:
/// with `NAME` and `ACTIVE` written last in the row (see `render_overview`'s
/// comment on why), a 100-column terminal clips them before the identifier
/// the interrupt/send/resume keys act on, which is the trade this makes on
/// purpose. 160 is where they first survive on this fixture — recorded
/// here as a measurement, not assumed.
#[test]
fn the_new_overview_columns_survive_a_realistic_and_a_wide_width() {
    use crate::session::{SessionName, SessionPurpose};

    let now = crate::provider::cache::now_unix_seconds();
    let named = SessionRecord {
        display_name: Some(SessionName::parse("payment retries").expect("valid name")),
        last_activity_at: now - 90,
        ..record("aaaaaaaaaaaa1", "claude-code", SessionLifecycle::Running)
    };
    let purposed = SessionRecord {
        purpose: Some(SessionPurpose::parse("tests").expect("valid purpose")),
        last_activity_at: now - 3 * 86_400,
        ..record("bbbbbbbbbbbb2", "codex", SessionLifecycle::Stopped)
    };
    let mut state = ShellState::new(
        "glasshouse",
        "/Users/someone/projects/glasshouse",
        "0.1.0",
        vec![named, purposed],
    );
    state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));

    for (width, height) in [(160, 30), (400, 30)] {
        let text = rendered(&state, width, height);
        assert!(text.contains("payment retries"), "width {width}:\n{text}");
        assert!(text.contains("tests"), "width {width}:\n{text}");
        assert!(text.contains("1 minute ago"), "width {width}:\n{text}");
        assert!(text.contains("3 days ago"), "width {width}:\n{text}");
        // Line 683: a `Running` session's fine state, not only its
        // coarse `active` disposition — and a `Stopped`-with-no-native-id
        // one's, which `Closed` alone does not distinguish from a
        // session that was explicitly closed.
        assert!(text.contains("active/running"), "width {width}:\n{text}");
        assert!(text.contains("closed/stopped"), "width {width}:\n{text}");
    }

    // A session with neither is a spoken sentinel, not a blank cell — an
    // empty cell here would be indistinguishable from one truncated away.
    let mut unnamed = sample();
    unnamed.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    let no_name = rendered(&unnamed, 400, 30);
    assert!(
        no_name.contains("(unnamed)"),
        "an absent name must say so, not render nothing:\n{no_name}"
    );
}

/// Map lines 1651-1654: the orchestrator, running workers, waiting
/// workers and recently completed workers must each be distinguishable
/// in the project overview — not collapsed into one undifferentiated
/// session list, which a reader could not act on.
#[test]
fn the_project_overview_separates_orchestrator_and_workers_by_role_and_lifecycle() {
    // Exactly 12 characters each — `short_session_id` truncates there,
    // and this test asserts on the truncated identifier a real overlay
    // would show, not the untruncated fixture id.
    let mut orchestrator = record("orchestrator", "claude-code", SessionLifecycle::Running);
    orchestrator.role = SessionRole::Orchestrator;

    let mut running_worker = record("run-worker01", "codex", SessionLifecycle::Running);
    running_worker.role = SessionRole::Worker;

    let mut waiting_worker = record("wait-worker1", "codex", SessionLifecycle::WaitingForUser);
    waiting_worker.role = SessionRole::Worker;

    let mut completed_worker = record("done-worker1", "codex", SessionLifecycle::Stopped);
    completed_worker.role = SessionRole::Worker;
    completed_worker.native_session_id = Some("native-completed".to_owned());

    let mut state = ShellState::new(
        "glasshouse",
        "/work",
        "0.1.0",
        vec![
            orchestrator,
            running_worker,
            waiting_worker,
            completed_worker,
        ],
    );
    assert_eq!(
        state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)),
        crate::shell::state::Action::OpenProjectOverview
    );
    state.open_project_overview(Vec::new(), Vec::new(), 0, Vec::new(), String::new(), None);

    let text = rendered(&state, 120, 40);
    assert!(text.contains("orchestrator"), "orchestrator row:\n{text}");
    assert!(text.contains("run-worker01"), "running row:\n{text}");
    assert!(text.contains("wait-worker1"), "waiting row:\n{text}");
    assert!(text.contains("done-worker1"), "completed row:\n{text}");
    assert!(
        !text.contains("no session is designated"),
        "an orchestrator was designated; the overlay must not say otherwise:\n{text}"
    );
}

/// The same box, with no orchestrator and no workers at all: every
/// section says so honestly instead of rendering an empty heading — the
/// same rule `render_overview` follows for its own activity section.
#[test]
fn the_project_overview_says_so_when_a_section_has_nothing() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    state.open_project_overview(Vec::new(), Vec::new(), 0, Vec::new(), String::new(), None);

    let text = rendered(&state, 120, 40);
    assert!(text.contains("no session is designated"), "{text}");
    assert!(text.contains("no workers running"), "{text}");
    assert!(text.contains("no workers waiting for input"), "{text}");
    assert!(text.contains("no completed workers recorded"), "{text}");
    assert!(text.contains("no current binding decisions"), "{text}");
    assert!(text.contains("no open todos"), "{text}");
    assert!(
        text.contains("no resources configured for this project"),
        "{text}"
    );
}

/// Map lines 1657-1660 and 1663: a resource line the run loop handed in
/// reaches the popup — through [`ShellState::open_project_overview`], the
/// same call the real run loop makes, not a value the view invents.
/// Practice §17: asserted at a realistic width and a wide one, because a
/// row truncated off-screen at 120 columns would make a later `!contains`
/// absence assertion pass for the wrong reason.
#[test]
fn the_project_overview_shows_resource_capacity_the_run_loop_handed_it() {
    for (width, height) in [(120, 40), (400, 40)] {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        state.open_project_overview(
            Vec::new(),
            Vec::new(),
            0,
            vec!["  openrouter (remote)  plenty 82% [measured], reset in 3600s".to_owned()],
            String::new(),
            None,
        );

        let text = rendered(&state, width, height);
        assert!(
            text.contains("openrouter (remote)"),
            "resource label, width {width}:\n{text}"
        );
        assert!(text.contains("82% [measured]"), "width {width}:\n{text}");
        assert!(text.contains("reset in 3600s"), "width {width}:\n{text}");
    }
}

/// Map lines 1658 and 1659: a resource with no telemetry renders
/// `"unknown"` and no number at all — asserted at both widths so a
/// truncated row cannot make the absence trivially true (practice §17).
#[test]
fn an_unknown_resource_never_shows_a_number_at_a_realistic_and_a_wide_width() {
    for (width, height) in [(120, 40), (400, 40)] {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        state.open_project_overview(
            Vec::new(),
            Vec::new(),
            0,
            vec!["  some-provider (remote)  capacity unknown".to_owned()],
            String::new(),
            None,
        );

        let text = rendered(&state, width, height);
        assert!(text.contains("capacity unknown"), "width {width}:\n{text}");
        assert!(!text.contains('%'), "width {width}:\n{text}");
    }
}

/// Map line 1661: the run loop's routing line reaches the screen as its
/// own labelled section, at a realistic width and a wide one (practice
/// §17), and fits an 80-column terminal without corrupting the rest of
/// the overview.
#[test]
fn the_project_overview_shows_the_routing_line_the_run_loop_handed_it() {
    for (width, height) in [(80, 40), (120, 40), (400, 40)] {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        state.open_project_overview(
            Vec::new(),
            Vec::new(),
            0,
            Vec::new(),
            "  routing model  anyrouter:claude-opus-4-1, recent latency median 340ms, \
                 p95 410ms (12 sample(s))"
                .to_owned(),
            None,
        );

        let text = rendered(&state, width, height);
        assert!(text.contains("ROUTING MODEL"), "width {width}:\n{text}");
        assert!(
            flattened(&text).contains("anyrouter:claude-opus-4-1"),
            "width {width}:\n{text}"
        );
        assert!(
            flattened(&text).contains("median 340ms"),
            "width {width}:\n{text}"
        );
    }
}

/// Ruling 1, at the view: an unknown latency reads `unknown`, never a
/// fabricated `0ms` — the same honesty rule
/// [`an_unknown_resource_never_shows_a_number_at_a_realistic_and_a_wide_width`]
/// proves for resources, proven here for the routing line specifically
/// because it is a different builder and a different render branch.
#[test]
fn the_routing_lines_unknown_latency_never_reads_as_zero() {
    for (width, height) in [(80, 40), (120, 40), (400, 40)] {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
        state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        state.open_project_overview(
            Vec::new(),
            Vec::new(),
            0,
            Vec::new(),
            "  routing model  anyrouter:claude-opus-4-1, recent latency unknown — not \
                 enough observations yet"
                .to_owned(),
            None,
        );

        let text = rendered(&state, width, height);
        assert!(
            flattened(&text).contains("recent latency unknown"),
            "width {width}:\n{text}"
        );
        assert!(!text.contains("0ms"), "width {width}:\n{text}");
        assert!(!text.contains("0 ms"), "width {width}:\n{text}");
    }
}

/// The routing section is absent when the run loop never set anything —
/// the fixtures elsewhere in this module that pass `String::new()`
/// because the routing line is not what they are testing, distinct from
/// a real overview the run loop opened.
#[test]
fn the_project_overview_omits_the_routing_section_when_nothing_was_handed_to_it() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    state.open_project_overview(Vec::new(), Vec::new(), 0, Vec::new(), String::new(), None);

    let text = rendered(&state, 120, 40);
    assert!(!text.contains("ROUTING MODEL"), "{text}");
}

/// Map lines 1655 and 1656: decisions/constraints and unresolved todos
/// the run loop read from project memory must actually reach the
/// screen — through [`ShellState::open_project_overview`], the same call
/// the real run loop makes, not a value the view invents.
#[test]
fn the_project_overview_shows_memory_the_run_loop_handed_it() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    state.open_project_overview(
        vec!["constraint: never run ci-local beside cargo".to_owned()],
        vec!["todo: wire the shell into main".to_owned()],
        3,
        Vec::new(),
        String::new(),
        None,
    );

    let text = rendered(&state, 120, 40);
    assert!(
        text.contains("never run ci-local beside cargo"),
        "decision/constraint line:\n{text}"
    );
    assert!(
        text.contains("wire the shell into main"),
        "todo line:\n{text}"
    );
    assert!(
        text.contains("...and 3 more"),
        "omitted todos must be counted, not silently dropped:\n{text}"
    );
}

/// A memory read failure still opens the overlay — sessions are still
/// worth showing — and says plainly that memory could not be read,
/// rather than presenting empty sections as if there were nothing to
/// show.
#[test]
fn a_project_memory_read_failure_still_opens_with_an_honest_note() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    state.open_project_overview(
        Vec::new(),
        Vec::new(),
        0,
        Vec::new(),
        String::new(),
        Some("project memory unavailable: disk full".to_owned()),
    );

    let text = rendered(&state, 120, 40);
    assert!(
        text.contains("project memory unavailable: disk full"),
        "{text}"
    );
}

/// A [`MemoryDetail`] fixture with something recorded in every field —
/// paired with [`one_entry`] so `lines` and `details` stay index-aligned
/// the way production [`super::super::build_project_knowledge_memory`]
/// keeps them.
fn fixture_detail() -> MemoryDetail {
    MemoryDetail {
        rationale: Some("fixture rationale".to_owned()),
        source_session: Some("sess_fixture".to_owned()),
        source_commit: Some("abc1234".to_owned()),
        lifecycle: "active".to_owned(),
    }
}

/// One [`KnowledgeSection`] with a single fixture line, for the five
/// section-presence tests below — each supplies this to exactly one of
/// the project-knowledge view's five sections and leaves the other four
/// at their `Default`, so a mutation that deletes only one section's
/// heading fails only that section's test.
fn one_entry(text: &str) -> KnowledgeSection {
    KnowledgeSection {
        lines: vec![text.to_owned()],
        details: vec![fixture_detail()],
        omitted: 0,
    }
}

/// Map lines 1098 and 1100: the project-knowledge view opens and shows
/// active decisions in their own labelled section.
#[test]
fn the_project_knowledge_view_shows_active_decisions_in_their_own_section() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        one_entry("decision: adopt the grouped-text project-knowledge view"),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        None,
    );

    let text = rendered(&state, 120, 40);
    assert!(text.contains("ACTIVE DECISIONS"), "{text}");
    assert!(
        text.contains("adopt the grouped-text project-knowledge view"),
        "{text}"
    );
}

/// Map line 1101: known constraints show in their own labelled section.
#[test]
fn the_project_knowledge_view_shows_known_constraints_in_their_own_section() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        KnowledgeSection::default(),
        one_entry("constraint: never run ci-local beside cargo"),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        None,
    );

    let text = rendered(&state, 120, 40);
    assert!(text.contains("KNOWN CONSTRAINTS"), "{text}");
    assert!(text.contains("never run ci-local beside cargo"), "{text}");
}

/// Map line 1102: implemented-or-planned features show in their own
/// labelled section.
#[test]
fn the_project_knowledge_view_shows_features_in_their_own_section() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        one_entry("feature: the project-knowledge overlay"),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        None,
    );

    let text = rendered(&state, 120, 40);
    assert!(text.contains("FEATURES"), "{text}");
    assert!(text.contains("the project-knowledge overlay"), "{text}");
}

/// Map line 1103: failed approaches show in their own, dedicated
/// historical section.
#[test]
fn the_project_knowledge_view_shows_failed_approaches_in_a_historical_section() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        one_entry("failed_attempt: a single global lock deadlocked"),
        KnowledgeSection::default(),
        None,
    );

    let text = rendered(&state, 120, 40);
    assert!(text.contains("FAILED APPROACHES"), "{text}");
    assert!(text.contains("HISTORICAL"), "{text}");
    assert!(text.contains("a single global lock deadlocked"), "{text}");
}

/// Map line 1104: unresolved todos show in their own labelled section.
#[test]
fn the_project_knowledge_view_shows_unresolved_todos_in_their_own_section() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        one_entry("todo: wire the knowledge view into main"),
        None,
    );

    let text = rendered(&state, 120, 40);
    assert!(text.contains("UNRESOLVED TODOS"), "{text}");
    assert!(text.contains("wire the knowledge view into main"), "{text}");
}

/// Map line 1098's empty-state half: with nothing recorded in any kind,
/// every one of the five sections says so honestly rather than
/// rendering an empty heading — the same rule
/// `the_project_overview_says_so_when_a_section_has_nothing` proves for
/// the project overview.
#[test]
fn the_project_knowledge_view_says_so_when_every_section_is_empty() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        None,
    );

    let text = rendered(&state, 120, 40);
    assert!(text.contains("no active decisions recorded"), "{text}");
    assert!(text.contains("no known constraints recorded"), "{text}");
    assert!(text.contains("no features recorded"), "{text}");
    assert!(text.contains("no failed approaches recorded"), "{text}");
    assert!(text.contains("no unresolved todos"), "{text}");
}

/// Map line 1106, at the render layer: whatever line
/// `shell::knowledge_line` hands the overlay reaches the screen
/// unchanged — a supersession note included when it names one, and no
/// note appended when it does not. The query-layer proof that the note
/// is only ever added when a real `superseded_by` exists lives in
/// `shell::project_knowledge_tests::
/// failed_approaches_are_shown_regardless_of_status_and_name_their_successor`;
/// this proves the text that function produces is not lost or altered
/// on the way to the terminal.
#[test]
fn the_project_knowledge_view_shows_a_supersession_note_when_present_and_omits_it_when_absent() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection {
            lines: vec![
                "failed_attempt: a global lock — superseded by mem_01AAAAAAAAAAAAAAAAAAAAAAAA"
                    .to_owned(),
                "failed_attempt: a distinct approach, still true".to_owned(),
            ],
            details: vec![fixture_detail(), fixture_detail()],
            omitted: 0,
        },
        KnowledgeSection::default(),
        None,
    );

    let text = rendered(&state, 120, 40);
    assert!(
        text.contains("superseded by mem_01AAAAAAAAAAAAAAAAAAAAAAAA"),
        "{text}"
    );
    assert_eq!(
        text.matches("superseded by").count(),
        1,
        "the line with no successor must not also carry a note:\n{text}"
    );
}

/// Map line 1107, and practice §17: the project-knowledge view draws
/// plain text and never a decorative node graph — proved by scanning the
/// rendered output for the glyphs a node-and-edge visualization would
/// need (node markers, arrows/connectors), at a realistic width *and* a
/// wide one, so a value truncated off a narrow render cannot make this
/// pass for the wrong reason.
///
/// Deliberately **not** scanning for ordinary box-drawing border
/// characters (`┌│└` and friends): every overlay in this shell,
/// including this one, draws inside a single bordered `Block` — see
/// `nothing_draws_with_block_elements_so_the_design_stays_text_first`'s
/// own comment, "Box-drawing characters used for borders are a
/// different range and stay allowed." What line 1107 rules out is a
/// graph of scattered, connected nodes, not this popup's own frame.
#[test]
fn the_project_knowledge_view_renders_no_decorative_graph_glyphs() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        one_entry("decision: adopt the grouped-text project-knowledge view"),
        one_entry("constraint: never run ci-local beside cargo"),
        one_entry("feature: the project-knowledge overlay"),
        KnowledgeSection {
            lines: vec![
                "failed_attempt: a global lock — superseded by mem_01AAAAAAAAAAAAAAAAAAAAAAAA"
                    .to_owned(),
            ],
            details: vec![fixture_detail()],
            omitted: 0,
        },
        one_entry("todo: wire the knowledge view into main"),
        None,
    );

    let graph_glyphs = [
        '●', '○', '◆', '◇', '■', '□', '▲', '▼', '→', '←', '↑', '↓', '↔', '↕',
    ];
    for (width, height) in [(120, 40), (400, 60)] {
        let text = rendered(&state, width, height);
        for glyph in graph_glyphs {
            assert!(
                !text.contains(glyph),
                "map line 1107: no decorative graph glyph `{glyph}`, width {width}:\n{text}"
            );
        }
    }
}

/// A project-knowledge read failure still opens the overlay — the same
/// contract `a_project_memory_read_failure_still_opens_with_an_honest_note`
/// proves for the project overview — and says plainly that memory could
/// not be read rather than presenting empty sections as if there were
/// nothing to show.
#[test]
fn a_project_knowledge_read_failure_still_opens_with_an_honest_note() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        Some("project memory unavailable: disk full".to_owned()),
    );

    let text = rendered(&state, 120, 40);
    assert!(
        text.contains("project memory unavailable: disk full"),
        "{text}"
    );
}

/// Map line 1105, through the production key path: pressing Enter on
/// the cursor's entry opens a detail popup showing its rationale,
/// source session, source commit and lifecycle state.
#[test]
fn opening_a_memory_from_the_knowledge_view_shows_its_full_provenance() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        KnowledgeSection {
            lines: vec!["decision: adopt the drill-down view".to_owned()],
            details: vec![MemoryDetail {
                rationale: Some("keeps the popup answering one question at a time".to_owned()),
                source_session: Some("sess_01AAAAAAAAAAAAAAAAAAAAAAAA".to_owned()),
                source_commit: Some("d34db33f".to_owned()),
                lifecycle: "active".to_owned(),
            }],
            omitted: 0,
        },
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        None,
    );

    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let text = rendered(&state, 120, 40);
    assert!(text.contains("adopt the drill-down view"), "{text}");
    assert!(
        text.contains("rationale: keeps the popup answering one question at a time"),
        "{text}"
    );
    assert!(
        text.contains("source session: sess_01AAAAAAAAAAAAAAAAAAAAAAAA"),
        "{text}"
    );
    assert!(text.contains("source commit: d34db33f"), "{text}");
    assert!(text.contains("lifecycle: active"), "{text}");
}

/// Map line 1105's honesty half: a memory recorded with no rationale, no
/// source session and no source commit says so in the detail popup
/// rather than rendering an empty field or fabricating one — the same
/// rule `knowledge_line`'s absent-supersession case follows for line
/// 1106.
#[test]
fn the_memory_detail_view_says_so_honestly_when_a_field_is_absent() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        KnowledgeSection {
            lines: vec!["todo: wire the knowledge view into main".to_owned()],
            details: vec![MemoryDetail {
                rationale: None,
                source_session: None,
                source_commit: None,
                lifecycle: "active".to_owned(),
            }],
            omitted: 0,
        },
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        None,
    );

    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let text = rendered(&state, 120, 40);
    assert!(text.contains("rationale: not recorded"), "{text}");
    assert!(text.contains("source session: not recorded"), "{text}");
    assert!(text.contains("source commit: not recorded"), "{text}");
    assert!(
        !text.contains("rationale: \n") && !text.contains("rationale: source"),
        "an absent rationale must never render as an empty or run-together field:\n{text}"
    );
}

/// Esc closes the detail popup and returns to the entry list, leaving
/// the cursor where it was rather than closing the whole overlay — the
/// same "close the innermost thing first" shape
/// `handle_overview_entry_key`'s own Esc arm follows for the send
/// field.
#[test]
fn esc_closes_the_memory_detail_popup_without_closing_the_knowledge_view() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    state.open_project_knowledge(
        one_entry("decision: adopt the drill-down view"),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        KnowledgeSection::default(),
        None,
    );

    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(rendered(&state, 120, 40).contains("memory detail"));

    state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let text = rendered(&state, 120, 40);
    assert!(text.contains("project knowledge"), "{text}");
    assert!(text.contains("adopt the drill-down view"), "{text}");
}

/// Map line 234, through the production key path: pressing `M` opens the
/// project-memory view — the same assert-the-premise-first shape
/// (practice §17) `the_project_knowledge_view_shows_active_decisions_in_their_own_section`
/// uses, checked here first as "not open before the press" — and it
/// shows a record's kind and status on the line, the one thing
/// `ProjectKnowledge`'s curated sections never have to say because their
/// section membership already implies both.
#[test]
fn the_m_key_opens_the_project_memory_view_showing_kind_and_status() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    assert_eq!(
        state.overlay(),
        None,
        "must not already be open before the key is pressed"
    );

    state.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE));
    state.open_project_memory(
        one_entry("[active] finding: the local gate must run alone"),
        None,
    );

    assert_eq!(state.overlay(), Some(Overlay::ProjectMemory));
    let text = rendered(&state, 120, 40);
    assert!(text.contains("finding:"), "{text}");
    assert!(text.contains("[active]"), "{text}");
    assert!(text.contains("the local gate must run alone"), "{text}");
}

/// Map line 234's empty-state half: a project with nothing recorded says
/// so honestly rather than rendering an empty list — the same rule
/// `the_project_knowledge_view_says_so_when_every_section_is_empty`
/// proves for its sibling.
#[test]
fn the_project_memory_view_says_so_when_there_is_nothing_recorded() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE));
    state.open_project_memory(KnowledgeSection::default(), None);

    let text = rendered(&state, 120, 40);
    assert!(
        text.contains("no memory recorded for this project"),
        "{text}"
    );
}

/// A project-memory-view read failure still opens the overlay with an
/// honest note — the same contract
/// `a_project_knowledge_read_failure_still_opens_with_an_honest_note`
/// keeps for its sibling, and never fails the shell.
#[test]
fn a_project_memory_view_read_failure_still_opens_with_an_honest_note() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE));
    state.open_project_memory(
        KnowledgeSection::default(),
        Some("project memory unavailable: disk full".to_owned()),
    );

    let text = rendered(&state, 120, 40);
    assert!(
        text.contains("project memory unavailable: disk full"),
        "{text}"
    );
}

/// Esc closes the project-memory detail popup and returns to the entry
/// list without closing the overlay — the same shape
/// `esc_closes_the_memory_detail_popup_without_closing_the_knowledge_view`
/// proves for `ProjectKnowledge`.
#[test]
fn esc_closes_the_project_memory_detail_popup_without_closing_the_view() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE));
    state.open_project_memory(one_entry("[active] decision: adopt the memory view"), None);

    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(rendered(&state, 120, 40).contains("memory detail"));

    state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let text = rendered(&state, 120, 40);
    assert!(text.contains("project memory"), "{text}");
    assert!(text.contains("adopt the memory view"), "{text}");
}

/// Map line 1107, the same rule
/// `the_project_knowledge_view_renders_no_decorative_graph_glyphs`
/// proves for its sibling: plain text, no decorative node graph, at a
/// realistic width and a wide one (practice §17).
#[test]
fn the_project_memory_view_renders_no_decorative_graph_glyphs() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE));
    state.open_project_memory(one_entry("[active] finding: no cycles here"), None);

    let graph_glyphs = [
        '●', '○', '◆', '◇', '■', '□', '▲', '▼', '→', '←', '↑', '↓', '↔', '↕',
    ];
    for (width, height) in [(120, 40), (400, 60)] {
        let text = rendered(&state, width, height);
        for glyph in graph_glyphs {
            assert!(
                !text.contains(glyph),
                "map line 1107: no decorative graph glyph `{glyph}`, width {width}:\n{text}"
            );
        }
    }
}

/// Map line 234's keyboard-reachability half: a keyboard-reachable view
/// nobody is told about is not reachable in the sense the line means, so
/// the footer must advertise `M`.
#[test]
fn the_footer_advertises_the_project_memory_key() {
    let state = sample();
    let bottom = footer(&state, 132, 24);
    assert!(bottom.contains("M memory"), "bindings missing: `{bottom}`");
}

/// Map line 1768: the project overview must never show a lifetime token
/// or spend total, and never present one as an achievement counter.
///
/// Practice §17: a settings test once passed for the wrong reason
/// because a narrow render clipped the very value it asserted was
/// absent, so this is proven at a realistic width *and* a wide one, and
/// mutation-proofed by hand: a temporary line pushed into
/// `render_project_overview` reading `lines.push(Line::from("lifetime
/// tokens: 999999"))` made this fail at both widths before it was
/// reverted (see the evidence ledger for the run).
#[test]
fn the_project_overview_never_shows_a_lifetime_token_or_spend_total() {
    let mut orchestrator = record("orchestrator", "claude-code", SessionLifecycle::Running);
    orchestrator.role = SessionRole::Orchestrator;
    let mut worker = record("run-worker01", "codex", SessionLifecycle::Running);
    worker.role = SessionRole::Worker;

    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![orchestrator, worker]);
    state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    state.open_project_overview(
        vec!["decision: ship the six closeable lines".to_owned()],
        vec!["todo: close the rest next round".to_owned()],
        2,
        Vec::new(),
        String::new(),
        None,
    );

    for (width, height) in [(120, 40), (400, 40)] {
        let text = rendered(&state, width, height).to_lowercase();
        for forbidden in ["token", "spend", "achievement", "lifetime"] {
            assert!(
                !text.contains(forbidden),
                "the project overview must never show `{forbidden}`, width {width}:\n{text}"
            );
        }
    }
}

/// Map line 1768's "never" as a property of the source, not only of one
/// fixture's render: a future PR could add a lifetime counter under
/// session-state most existing fixtures never populate. Scoped to
/// `render_project_overview` specifically — `session_detail`'s
/// per-session model/backend line and Settings' own per-decision
/// "Maximum marginal cost" knob are both legitimate and must not trip
/// this.
///
/// # Scanned by lines, deliberately
///
/// Same idiom as `shell::mod::run_loop_passes_the_default_timeouts`: a
/// multi-line literal search breaks on a CRLF checkout because Git
/// converts line endings and the literal no longer matches, and that has
/// already taken Windows CI red once on this project. [`str::lines`]
/// strips the carriage return, so this is CRLF-agnostic by construction.
fn project_overview_never_names_a_lifetime_total(source: &str) -> bool {
    let production = source.split("#[cfg(test)]").next().unwrap_or(source);
    let lines: Vec<&str> = production.lines().collect();
    let Some(start) = lines
        .iter()
        .position(|line| line.trim_start().starts_with("fn render_project_overview("))
    else {
        return false;
    };
    let body: Vec<&str> = lines[start + 1..]
        .iter()
        .take_while(|line| !line.trim_start().starts_with("fn "))
        .copied()
        .collect();

    const FORBIDDEN: [&str; 4] = ["token", "spend", "achievement", "lifetime"];
    !body.iter().any(|line| {
        let lower = line.to_lowercase();
        FORBIDDEN.iter().any(|term| lower.contains(term))
    })
}

/// The control that keeps the scan above honest — both that it says yes
/// on the real file and that it is capable of saying no, in both an LF
/// and a CRLF checkout.
#[test]
fn the_lifetime_total_scan_is_crlf_agnostic_and_can_say_no() {
    let normalised = include_str!("../view.rs").replace("\r\n", "\n");
    let crlf = normalised.replace('\n', "\r\n");
    assert!(
        project_overview_never_names_a_lifetime_total(&normalised),
        "the real file must pass its own guard"
    );
    assert!(
        project_overview_never_names_a_lifetime_total(&crlf),
        "the scan must still pass in a CRLF checkout"
    );

    let tainted = "fn render_project_overview(x: i32) {\n    let lifetime_tokens = 1;\n}\n";
    assert!(
        !project_overview_never_names_a_lifetime_total(tainted),
        "a source naming a lifetime total inside the function must be rejected"
    );

    // And a term appearing *outside* the function — Settings' own "cost"
    // knob, say — must not trip it, or the guard is too broad to survive
    // the rest of this file.
    let outside = "fn render_project_overview(x: i32) {\n    let y = 1;\n}\n\
                        fn render_settings() {\n    let cost = 1;\n}\n";
    assert!(
        project_overview_never_names_a_lifetime_total(outside),
        "a term outside render_project_overview must not trip the guard"
    );
}

/// Map line 1664: the overview must be derived state, never generated
/// commentary — proven the same way `nothing_draws_with_block_elements`
/// proves the rest of the shell stays text-first: every character in the
/// popup traces to a session field or a memory string the run loop
/// handed in, never a phrase invented at render time beyond the fixed,
/// literal section headings and empty-state notes already asserted
/// above.
#[test]
fn the_project_overview_footer_names_its_own_key() {
    let mut state = ShellState::new("glasshouse", "/work", "0.1.0", vec![lone_session()]);
    state.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    state.open_project_overview(Vec::new(), Vec::new(), 0, Vec::new(), String::new(), None);
    let text = rendered(&state, 120, 40);
    assert!(
        text.contains("esc back to session"),
        "project overview footer:\n{text}"
    );

    let control_text = rendered(&sample(), 120, 24);
    assert!(
        control_text.contains("p project"),
        "control-mode footer must advertise the key:\n{control_text}"
    );
}

/// Map line 1758. Practice §17: asserted at a realistic width and at 400
/// columns, exactly like `the_overview_shows_recorded_activity_...`,
/// because a row clipped off-screen at 100 columns would make the
/// `contains` assertions below pass or fail for reasons that have
/// nothing to do with the code.
///
/// Also proves the filter: `sample()`'s two sessions each get an event,
/// and only the *presented* one's (`aaaaaaaaaaaa1`, `sample()`'s default
/// `selected_index`) text must appear — the other session's event text
/// must not, which is what distinguishes this overlay from
/// `render_overview`'s cross-session ACTIVITY feed.
#[test]
fn session_events_shows_only_the_presented_sessions_events_at_a_realistic_and_a_wide_width() {
    use crate::events::{EventBus, LifecycleEvent, MessageOrigin, TurnOutcome};
    use crate::session::SessionId;
    use crate::shell::Action;

    let mut state = sample();
    let bus = EventBus::new();
    let presented = SessionId::new("aaaaaaaaaaaa1");
    let other = SessionId::new("bbbbbbbbbbbb2");
    let recorded = vec![
        bus.publish(&presented, LifecycleEvent::SessionStarted),
        bus.publish(
            &other,
            LifecycleEvent::TextDelivered {
                origin: MessageOrigin::Machine,
                bytes: 99,
            },
        ),
        bus.publish(
            &presented,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
        ),
    ];
    assert_eq!(state.note_events(&recorded), Action::Redraw);
    assert_eq!(
        state.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)),
        Action::Redraw
    );

    for (width, height) in [(100, 24), (400, 24)] {
        let text = rendered(&state, width, height);
        assert!(text.contains("session events"), "width {width}:\n{text}");
        assert!(text.contains("session started"), "width {width}:\n{text}");
        assert!(
            text.contains("turn ended (completed)"),
            "width {width}:\n{text}"
        );
        assert!(
            !text.contains("sent 99 bytes (machine)"),
            "the other session's event must not appear, width {width}:\n{text}"
        );
    }
}

/// The honest empty state, matching `render_overview`'s own convention
/// for a section with nothing to show.
#[test]
fn session_events_says_so_when_the_presented_session_has_none() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));

    let text = rendered(&state, 120, 24);
    assert!(
        text.contains("no recent lifecycle events recorded for this session"),
        "{text}"
    );
}

/// Map line 1770 for this overlay specifically: reached only by its own
/// key, never present on the screen a user sees without asking for it.
/// Asserted at both widths for the same reason as the test above — an
/// absence assertion is only as strong as the viewport it renders into
/// (practice §17).
#[test]
fn session_events_is_absent_from_the_default_screen_at_a_realistic_and_a_wide_width() {
    let state = sample();
    for (width, height) in [(100, 24), (400, 24)] {
        let text = rendered(&state, width, height);
        assert!(
            !text.contains("session events"),
            "the default screen must not show the session-events overlay, \
                 width {width}:\n{text}"
        );
    }
}

/// The overlay's own footer, and the control-mode footer advertising the
/// key that opens it — the same pair `the_project_overview_footer_...`
/// proves for `p`/`Overlay::ProjectOverview`.
#[test]
fn the_session_events_footer_names_its_own_key() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
    let text = rendered(&state, 120, 24);
    assert!(
        text.contains("esc back to session"),
        "session events footer:\n{text}"
    );

    let control_text = rendered(&sample(), 120, 24);
    assert!(
        control_text.contains("e events"),
        "control-mode footer must advertise the key:\n{control_text}"
    );
}

/// `e` toggles like every other overlay key: pressing it again while open
/// closes it, exactly as `o`/`Overlay::Overview` and `p`/
/// `Overlay::ProjectOverview` already do. A regression here would leave
/// the key un-openable a second time in the same session.
#[test]
fn e_opens_and_esc_closes_the_session_events_overlay() {
    use crate::shell::Action;

    let mut state = sample();
    assert_eq!(
        state.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)),
        Action::Redraw
    );
    assert_eq!(
        state.overlay(),
        Some(crate::shell::state::Overlay::SessionEvents)
    );
    assert_eq!(
        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Action::Redraw
    );
    assert_eq!(state.overlay(), None);
}

fn route_row(
    provider: &str,
    model: &str,
    route: Option<&str>,
    context_state: &str,
    sample_count: usize,
    window_start_unix: i64,
    window_end_unix: i64,
) -> crate::shell::state::RouteEvidenceRow {
    crate::shell::state::RouteEvidenceRow {
        provider: provider.to_owned(),
        model: model.to_owned(),
        route: route.map(str::to_owned),
        context_state: context_state.to_owned(),
        sample_count,
        window_start_unix,
        window_end_unix,
    }
}

/// Acceptance test 4: the table renders sample count and window from
/// real recorded data, and two identities with different counts render
/// differently.
#[test]
fn the_route_evidence_table_renders_sample_count_and_window_and_distinguishes_identities() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    let now = crate::provider::cache::now_unix_seconds();
    state.open_route_evidence(
        vec![
            route_row(
                "anyrouter",
                "claude-opus-4-1",
                Some("anthropic-messages"),
                "unknown",
                5,
                now - 3_600,
                now - 60,
            ),
            route_row(
                "openai-router",
                "gpt-5",
                None,
                "unknown",
                1,
                now - 30,
                now - 30,
            ),
        ],
        None,
    );

    let text = rendered(&state, 120, 24);
    assert!(text.contains("anyrouter"), "{text}");
    assert!(text.contains("claude-opus-4-1"), "{text}");
    assert!(text.contains("anthropic-messages"), "{text}");
    assert!(text.contains('5'), "{text}");
    assert!(text.contains("openai-router"), "{text}");
    assert!(text.contains("gpt-5"), "{text}");
    assert!(
        text.contains("(no route)"),
        "an identity with no recorded route must say so honestly:\n{text}"
    );

    let rows: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("anyrouter") || line.contains("openai-router"))
        .collect();
    assert_eq!(rows.len(), 2);
    assert_ne!(
        rows[0], rows[1],
        "two identities with different counts and windows must render differently:\n{text}"
    );
}

/// Acceptance test 5, capability map line 1764: an `Unknown` row renders
/// as `unknown`, neither omitted nor dressed up as a measurement.
#[test]
fn the_route_evidence_table_renders_unknown_context_state_plainly() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    state.open_route_evidence(
        vec![route_row(
            "anyrouter",
            "m",
            Some("anthropic-messages"),
            "unknown",
            5,
            1_000,
            1_000,
        )],
        None,
    );

    let text = rendered(&state, 120, 24);
    assert!(text.contains("unknown"), "{text}");
}

/// Acceptance test 6, and practice §17: the rendered table names no
/// TTFC/TTFT/throughput/rounds-per-minute column, at a viewport wide
/// enough that such a column *would* have been visible rather than
/// clipped off-screen for the wrong reason.
#[test]
fn no_fabricated_columns_appear_in_the_route_evidence_table() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    state.open_route_evidence(
        vec![route_row(
            "anyrouter",
            "claude-opus-4-1",
            Some("anthropic-messages"),
            "unknown",
            5,
            1_000,
            1_050,
        )],
        None,
    );

    for (width, height) in [(120, 24), (400, 30)] {
        let text = rendered(&state, width, height).to_lowercase();
        for forbidden in [
            "ttfc",
            "ttft",
            "throughput",
            "rounds per minute",
            "rounds/min",
            "decode",
        ] {
            assert!(
                !text.contains(forbidden),
                "map line 1762: no fabricated `{forbidden}` column, width {width}:\n{text}"
            );
        }
    }
}

/// Acceptance test 7, empty half: an empty ledger renders an honest
/// empty state, the same convention every other overlay section here
/// uses.
#[test]
fn the_route_evidence_table_says_so_when_there_is_no_evidence_yet() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    state.open_route_evidence(Vec::new(), None);

    let text = rendered(&state, 120, 24);
    assert!(text.contains("no routing evidence recorded yet"), "{text}");
}

// -----------------------------------------------------------------
// The routing-decisions overlay — the reader half of the disposable
// routing sink. Every test here hands the view a row it invented, so
// none of them says anything about whether the run loop reads the
// ledger; that is `tests/disposable_route_sink.rs`'s job, and practice
// §35 is why the split is deliberate rather than an omission.
// -----------------------------------------------------------------

fn decision_row(
    job: &str,
    session: Option<&str>,
    rationale: Option<&str>,
    observed_at_unix: i64,
) -> crate::shell::state::RouteDecisionRow {
    crate::shell::state::RouteDecisionRow {
        observed_at_unix,
        job: job.to_owned(),
        session_id: session.map(str::to_owned),
        rationale: rationale.map(str::to_owned),
    }
}

/// The whole stored rationale reaches the screen — the heading *and* the
/// named contributions under it.
///
/// The heading alone would be a view that says which resource won and
/// not one reason it did, which is the shape map line 1766 asks this not
/// to be. Asserted at a wide viewport too, per practice §17: a
/// contribution line is long, and a match that only survives at 400
/// columns is a layout finding rather than a rendering one.
#[test]
fn the_routing_decisions_view_draws_the_whole_stored_rationale() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    state.open_route_decisions(
        vec![decision_row(
            "memory extraction",
            Some("session-abc"),
            Some(
                "a-free-model on a-provider — free, used by user preference\n  \
                     +1.000  cost — free — line 530 prefers free capacity\n  \
                     +0.000  user pin — the user pinned this exact free resource",
            ),
            1_000,
        )],
        None,
    );

    for (width, height) in [(120, 30), (400, 30)] {
        let text = rendered(&state, width, height);
        assert!(text.contains("memory extraction"), "width {width}:\n{text}");
        assert!(text.contains("session-abc"), "width {width}:\n{text}");
        assert!(text.contains("a-free-model"), "width {width}:\n{text}");
        assert!(
            text.contains("user preference"),
            "the reason the policy gave must be on screen, width {width}:\n{text}"
        );
        assert!(
            text.contains("line 530 prefers free capacity"),
            "a decision drawn without its contributions is an outcome, not a \
                 rationale, width {width}:\n{text}"
        );
        assert!(
            text.contains("user pin"),
            "every contribution is drawn, not only the first, width {width}:\n{text}"
        );
    }
}

/// A row the producer could not fill says so, at both widths — practice
/// §17, and map line 1294's rule that an absent value is drawn as absent
/// rather than as an empty column a reader would take for a value.
#[test]
fn a_routing_decision_with_nothing_recorded_says_so_rather_than_drawing_a_blank() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    state.open_route_decisions(
        vec![decision_row("memory extraction", None, None, 1_000)],
        None,
    );

    for (width, height) in [(120, 24), (400, 24)] {
        let text = rendered(&state, width, height);
        assert!(
            text.contains("(no session recorded)"),
            "width {width}:\n{text}"
        );
        assert!(
            text.contains("(no rationale recorded)"),
            "width {width}:\n{text}"
        );
    }
}

/// The empty half: a project that has recorded no decision is told so,
/// which is the honest and most common answer rather than a failure.
#[test]
fn the_routing_decisions_view_says_so_when_nothing_is_recorded() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    state.open_route_decisions(Vec::new(), None);

    let text = rendered(&state, 120, 24);
    assert!(
        text.contains("no routing decision has been recorded yet"),
        "{text}"
    );
}

/// The failure half: an unreadable ledger still opens the overlay with an
/// honest note, the same contract
/// `a_route_evidence_read_failure_still_opens_with_an_honest_note` proves
/// for its own view.
#[test]
fn a_routing_decisions_read_failure_still_opens_with_an_honest_note() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    state.open_route_decisions(
        Vec::new(),
        Some("routing decisions unavailable: disk full".to_owned()),
    );

    let text = rendered(&state, 120, 24);
    assert!(
        text.contains("routing decisions unavailable: disk full"),
        "{text}"
    );
}

/// Reached only by its own key, never on the screen a user sees without
/// asking — asserted at both widths per practice §17.
#[test]
fn routing_decisions_are_absent_from_the_default_screen_at_a_realistic_and_a_wide_width() {
    let state = sample();
    for (width, height) in [(100, 24), (400, 24)] {
        let text = rendered(&state, width, height);
        assert!(
            !text.contains("routing decisions"),
            "the default screen must not show the routing-decisions overlay, \
                 width {width}:\n{text}"
        );
    }
}

/// The overlay's own footer, and the control-mode footer advertising `d`
/// — the same pair `the_route_evidence_footer_names_its_own_key` proves.
#[test]
fn the_routing_decisions_footer_names_its_own_key() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    state.open_route_decisions(Vec::new(), None);
    let text = rendered(&state, 120, 24);
    assert!(
        text.contains("esc back to session"),
        "routing decisions footer:\n{text}"
    );

    // 170 for the reason `the_status_bar_always_shows_the_key_bindings`
    // records: the control row is exactly 168 columns now.
    let control_text = rendered(&sample(), 170, 24);
    assert!(
        control_text.contains("d decisions"),
        "control-mode footer must advertise the key:\n{control_text}"
    );
}

/// Acceptance test 7, failure half: a read failure still opens the
/// overlay with an honest note — the same contract
/// `a_project_knowledge_read_failure_still_opens_with_an_honest_note`
/// proves for the project-knowledge view.
#[test]
fn a_route_evidence_read_failure_still_opens_with_an_honest_note() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    state.open_route_evidence(
        Vec::new(),
        Some("routing evidence unavailable: disk full".to_owned()),
    );

    let text = rendered(&state, 120, 24);
    assert!(
        text.contains("routing evidence unavailable: disk full"),
        "{text}"
    );
}

/// Map line 1770 for this overlay specifically: reached only by its own
/// key, never present on the screen a user sees without asking for it —
/// asserted at both widths per practice §17.
#[test]
fn route_evidence_is_absent_from_the_default_screen_at_a_realistic_and_a_wide_width() {
    let state = sample();
    for (width, height) in [(100, 24), (400, 24)] {
        let text = rendered(&state, width, height);
        assert!(
            !text.contains("route evidence"),
            "the default screen must not show the route-evidence overlay, \
                 width {width}:\n{text}"
        );
    }
}

/// The overlay's own footer, and the control-mode footer advertising the
/// key that opens it — the same pair `the_session_events_footer_...`
/// proves for `e`/`Overlay::SessionEvents`.
#[test]
fn the_route_evidence_footer_names_its_own_key() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    state.open_route_evidence(Vec::new(), None);
    let text = rendered(&state, 120, 24);
    assert!(
        text.contains("esc back to session"),
        "route evidence footer:\n{text}"
    );

    let control_text = rendered(&sample(), 120, 24);
    assert!(
        control_text.contains("r routes"),
        "control-mode footer must advertise the key:\n{control_text}"
    );
}

/// `r` toggles like every other overlay key: pressing it again while open
/// closes it, exactly as `e`/`Overlay::SessionEvents` already does.
#[test]
fn r_opens_and_esc_closes_the_route_evidence_overlay() {
    use crate::shell::Action;

    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    state.open_route_evidence(Vec::new(), None);
    assert_eq!(
        state.overlay(),
        Some(crate::shell::state::Overlay::RouteEvidence)
    );

    assert_eq!(
        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Action::Redraw
    );
    assert_eq!(state.overlay(), None);
}

// -----------------------------------------------------------------
// Phase 47 line 1765 — route health, immediate availability, cadence,
// quota reset and failure-domain evidence as SEPARATE concepts.
// -----------------------------------------------------------------

/// The rendered screen with the popup's own border columns dropped and
/// runs of whitespace collapsed, so a phrase the `Wrap` broke across two
/// rows can still be asserted on.
///
/// Needed because line 1765 wants five *labelled* concepts and the labels
/// plus their evidence are longer than a realistic popup is wide. The
/// per-line assertions below deliberately use the **raw** text instead —
/// "these two concepts are not on the same line" is a claim about lines,
/// and flattening would make it unfalsifiable.
fn flattened(text: &str) -> String {
    text.replace('│', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A healthy, available, entirely unstated resource — the state a fresh
/// installation actually observes. Every test below starts here and
/// overrides only the fields its own case is about, with struct-update
/// syntax, so what a case is *testing* is visible at its call site.
fn health_row(provider: &str, model: &str) -> crate::shell::state::RouteHealthRow {
    crate::shell::state::RouteHealthRow {
        provider: provider.to_owned(),
        credential_label: format!("{provider}/API_KEY"),
        model: model.to_owned(),
        consecutive_failures: 0,
        credential_rejected: false,
        available_now: true,
        cooling_down_until_unix: None,
        stated_limit: None,
        stated_window_seconds: None,
        quota_resets_at_unix: None,
        failure_domain: "unknown".to_owned(),
        failure_domain_peers: 0,
    }
}

/// **Line 1765's whole content.** Five labelled concepts, five separate
/// lines, for a resource where they genuinely disagree: healthy (zero
/// failures) yet unavailable (credential refused), paced by Glasshouse
/// while the provider's own reset is at a different time.
///
/// Rendered at a realistic width and a wide one (practice §17), because a
/// label that happened to clip off-screen would make this pass for the
/// wrong reason.
#[test]
fn route_health_keeps_line_1765s_five_concepts_on_separate_lines() {
    let now = crate::provider::cache::now_unix_seconds();
    for (width, height) in [(120, 40), (400, 40)] {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        // Never failed, yet unavailable; paced by Glasshouse until a
        // different instant from the provider's own reset. 330 s, not 300:
        // `describe_deadline` floors to whole minutes against its own clock
        // read, so at exactly 300 s one slow second rendered "4 minutes" —
        // three times in one week on hosted runners (waves 137–138). The
        // 30 s of slack keeps the assertion below at "5 minutes".
        state.open_route_health(vec![crate::shell::state::RouteHealthRow {
            credential_rejected: true,
            available_now: false,
            cooling_down_until_unix: Some(now + 330),
            stated_limit: Some(300),
            stated_window_seconds: Some(60),
            quota_resets_at_unix: Some(now + 1_800),
            failure_domain: "shared".to_owned(),
            failure_domain_peers: 1,
            ..health_row("anyrouter", "claude-opus-4-1")
        }]);
        let text = rendered(&state, width, height);

        for concept in [
            "route health",
            "immediate availability",
            "cadence",
            "quota reset",
            "failure domain",
        ] {
            assert!(
                text.contains(concept),
                "line 1765 names `{concept}` and it must be its own labelled \
                     concept, width {width}:\n{text}"
            );
        }

        // Each concept on its own line: no line may carry two of the five
        // labels, which is exactly what collapsing them would produce.
        for line in text.lines() {
            let found = [
                "route health",
                "immediate availability",
                "cadence",
                "quota reset",
                "failure domain",
            ]
            .iter()
            .filter(|concept| line.contains(*concept))
            .count();
            assert!(
                found <= 1,
                "two of line 1765's concepts were folded onto one line, \
                     width {width}:\n{line}"
            );
        }

        // And the five really do disagree here, which is the point: a
        // single status word could not have carried all of this.
        let flat = flattened(&text);
        assert!(
            flat.contains("0 consecutive failure(s)"),
            "the failure streak must be shown as a streak, width {width}:\n{text}"
        );
        assert!(
            flat.contains("credential rejected: yes"),
            "a refused credential is a health fact of its own, \
                 width {width}:\n{text}"
        );
        assert!(
            flat.contains("not schedulable right now"),
            "availability must be its own answer, width {width}:\n{text}"
        );
        assert!(
            flat.contains("300 request(s) per 60s"),
            "the provider-stated cadence must be shown, width {width}:\n{text}"
        );
        assert!(
            flat.contains("cooling down, ends in 5 minutes"),
            "glasshouse's own pacing is a separate clock from the \
                 provider's, width {width}:\n{text}"
        );
    }
}

/// **The honesty half.** A provider that has stated no rate-limit headers
/// at all — which is most of them — must read `unknown`, never `0` and
/// never an invented reset. Line 1765 sits under a phase whose whole
/// heading is about not presenting a number the evidence does not carry.
#[test]
fn route_health_says_unknown_rather_than_zero_for_what_no_provider_stated() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    // The default is the entirely-unstated case, which is the point.
    state.open_route_health(vec![health_row("openrouter", "some-free-model")]);
    let text = rendered(&state, 120, 40);
    let flat = flattened(&text);

    assert!(
        flat.contains("quota reset unknown — no response has stated one"),
        "an unstated quota reset must read `unknown`:\n{text}"
    );
    assert!(
        flat.contains("provider stated: unknown"),
        "an unstated cadence must read `unknown`:\n{text}"
    );
    assert!(
        flat.contains("glasshouse pacing: none"),
        "no cooldown must read `none`, not an elapsed deadline:\n{text}"
    );
    // The three unstated concepts must not have been filled in with a
    // number: `0` would read as a measurement, which is the whole of what
    // this phase is named after not doing.
    for invented in ["quota reset in", "per 0s", "0 request(s)"] {
        assert!(
            !flat.contains(invented),
            "an unstated value was rendered as `{invented}`:\n{text}"
        );
    }
}

/// `crate::routing::domain::FailureDomain::Independent` is a state this
/// build cannot earn — nothing does the temporal correlation it would
/// need — so no fixture, and no future edit, may make this view print it.
/// Proved at a wide viewport per practice §17, because an absence
/// assertion is only as strong as the screen it renders into.
#[test]
fn route_health_never_claims_two_resources_are_independent() {
    for (width, height) in [(120, 40), (400, 40)] {
        let mut state = sample();
        state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        state.open_route_health(vec![
            crate::shell::state::RouteHealthRow {
                consecutive_failures: 2,
                failure_domain: "shared".to_owned(),
                failure_domain_peers: 1,
                ..health_row("anyrouter", "model-a")
            },
            health_row("openrouter", "model-b"),
        ]);
        let flat = flattened(&rendered(&state, width, height));
        assert!(
            flat.contains("never `independent`"),
            "the view must say what the absence of evidence does not mean, \
                 width {width}:\n{flat}"
        );
        // The only permitted occurrence is inside that refusal.
        assert_eq!(
            flat.matches("independent").count(),
            flat.matches("never `independent`").count(),
            "`independent` may appear only inside the sentence refusing it, \
                 width {width}:\n{flat}"
        );
    }
}

/// The empty state, which is what a fresh installation actually shows:
/// honest words, not a table of zeroes.
#[test]
fn route_health_says_so_when_no_gateway_exchange_has_been_observed() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    state.open_route_health(Vec::new());
    let text = rendered(&state, 120, 30);
    assert!(
        text.contains("no gateway exchange has been observed"),
        "an empty cache must say so:\n{text}"
    );
}

/// The scope label, which is a fact about these caches and not decoration:
/// they live under the installation's data directory, keyed by provider,
/// so a reading written while a gateway served another project is visible
/// here. A view that let a reader assume otherwise would be the spectacle
/// this phase is named after.
#[test]
fn route_health_labels_its_own_installation_wide_scope() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    state.open_route_health(Vec::new());
    let text = rendered(&state, 120, 30);
    assert!(
        text.contains("not scoped to this project"),
        "the view must name its own scope:\n{text}"
    );
}

/// Map line 1770 for this overlay: reached only by its own key, never on
/// the default screen — at both widths, per practice §17.
#[test]
fn route_health_is_absent_from_the_default_screen_at_a_realistic_and_a_wide_width() {
    let state = sample();
    for (width, height) in [(100, 24), (400, 24)] {
        let text = rendered(&state, width, height);
        assert!(
            !text.contains("immediate availability"),
            "the default screen must not show the route-health overlay, \
                 width {width}:\n{text}"
        );
    }
}

/// The overlay's own footer, and the control-mode footer advertising `h`.
#[test]
fn the_route_health_footer_names_its_own_key() {
    let mut state = sample();
    state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    state.open_route_health(Vec::new());
    let text = rendered(&state, 132, 24);
    assert!(
        text.contains("esc back to session"),
        "route health footer:\n{text}"
    );

    let control_text = rendered(&sample(), 132, 24);
    assert!(
        control_text.contains("h health"),
        "control-mode footer must advertise the key:\n{control_text}"
    );
}

/// `h` opens it and `esc` closes it, the same toggle every other overlay
/// key already has.
#[test]
fn h_opens_and_esc_closes_the_route_health_overlay() {
    use crate::shell::Action;

    let mut state = sample();
    assert_eq!(
        state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
        Action::OpenRouteHealth
    );
    state.open_route_health(Vec::new());
    assert_eq!(
        state.overlay(),
        Some(crate::shell::state::Overlay::RouteHealth)
    );

    assert_eq!(
        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Action::Redraw
    );
    assert_eq!(state.overlay(), None);
}

#[cfg(test)]
mod settings_tests {
    use crate::config::{
        Layered, PremiumReservePercent, ProfileConfig, ProviderConfig, RouterCostMicroUsd,
        RouterLatencyMs, RoutingModelChoice,
    };
    use crate::integrations::{IntegrationId, IntegrationStatus};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::super::super::state::{
        HarnessRow, IntegrationRow, MemoryRow, ProfileRow, ProviderRow, RoutingRow,
    };
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn harness_rows() -> Vec<HarnessRow> {
        vec![
            HarnessRow {
                id: IntegrationId::ClaudeCode,
                detected: true,
                enabled: true,
                enabled_layer: Layer::Project,
                executable: Some("/opt/bin/claude".into()),
                executable_layer: Some(Layer::User),
            },
            HarnessRow {
                id: IntegrationId::Codex,
                detected: false,
                enabled: false,
                enabled_layer: Layer::Default,
                executable: None,
                executable_layer: None,
            },
        ]
    }

    fn integration_rows() -> Vec<IntegrationRow> {
        vec![IntegrationRow {
            id: IntegrationId::Ollama,
            detected: false,
            status: IntegrationStatus::NotFound,
        }]
    }

    fn state_with_settings_open() -> ShellState {
        let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
        state.open_settings(harness_rows(), integration_rows(), Vec::new(), Vec::new());
        state
    }

    /// Tab until `target` has the cursor, rather than a fixed count — a count
    /// is a hidden dependency on how many sections precede this test's own,
    /// and inserting Subscriptions into `ORDER` silently re-pointed four of
    /// these tests at the wrong tab.
    fn tab_to(state: &mut ShellState, target: SettingsSection) {
        for _ in 0..SettingsSection::ORDER.len() {
            if state.settings().unwrap().section() == target {
                return;
            }
            state.handle_key(press(KeyCode::Tab));
        }
        assert_eq!(
            state.settings().unwrap().section(),
            target,
            "Tab never reached the section this test is about"
        );
    }

    fn rendered(state: &ShellState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(state, frame))
            .expect("draw must not panic");
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn settings_shows_both_section_labels_and_the_harness_rows() {
        let state = state_with_settings_open();
        let text = rendered(&state, 100, 30);
        assert!(text.contains("Harnesses"), "got:\n{text}");
        assert!(text.contains("Integrations"), "got:\n{text}");
        assert!(text.contains("Claude Code"), "got:\n{text}");
    }

    /// The design decision: "provenance is shown, not inferred" — every
    /// displayed value must carry a layer tag.
    #[test]
    fn every_harness_value_shown_carries_its_layer() {
        let state = state_with_settings_open();
        let text = rendered(&state, 100, 30);
        assert!(text.contains("(project)"), "enabled layer missing:\n{text}");
        assert!(text.contains("(user)"), "executable layer missing:\n{text}");
        assert!(
            text.contains("(default)"),
            "the never-configured row's layer must still show:\n{text}"
        );
    }

    #[test]
    fn switching_to_the_integrations_section_shows_its_rows() {
        let mut state = state_with_settings_open();
        state.handle_key(press(KeyCode::Tab));
        let text = rendered(&state, 100, 30);
        assert!(text.contains("Ollama"), "got:\n{text}");
        assert!(text.contains("not found"), "got:\n{text}");
    }

    #[test]
    fn the_path_editor_renders_the_typed_buffer() {
        let mut state = state_with_settings_open();
        state.handle_key(press(KeyCode::Enter));
        for c in "/opt/bin".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        let text = rendered(&state, 100, 30);
        assert!(text.contains("/opt/bin"), "got:\n{text}");
    }

    /// The confirmation must name the exact path the design decision
    /// requires, not a generic "are you sure".
    #[test]
    fn the_project_write_confirmation_names_the_exact_path() {
        let mut state = state_with_settings_open();
        state.handle_key(press(KeyCode::Char('W')));
        let text = rendered(&state, 100, 30);

        // Built with `join`, exactly as `render_project_write_confirmation`
        // builds it. A hard-coded `/work/glasshouse/.glasshouse/config.toml`
        // passed everywhere except Windows, where `join` yields backslashes —
        // a test-portability fault, not a product one, but it turned CI red.
        let expected = std::path::Path::new("/work/glasshouse")
            .join(".glasshouse")
            .join("config.toml");
        let expected = expected.display().to_string();
        assert!(
            text.contains(&expected),
            "the exact path `{expected}` must be shown:\n{text}"
        );
        assert!(
            text.to_lowercase().contains("repository"),
            "must say the file is inside the repository:\n{text}"
        );
    }

    #[test]
    fn the_footer_names_the_settings_bindings() {
        let state = state_with_settings_open();
        let bottom = footer(&state, 100, 30);
        assert!(bottom.contains("save"), "got: `{bottom}`");
        assert!(bottom.contains("toggle"), "got: `{bottom}`");
    }

    #[test]
    fn settings_renders_without_panicking_at_absurd_sizes() {
        let mut state = state_with_settings_open();
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
        state.handle_key(press(KeyCode::Enter));
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3)] {
            rendered(&state, w, h);
        }
        state.handle_key(press(KeyCode::Esc));
        state.handle_key(press(KeyCode::Char('W')));
        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3)] {
            rendered(&state, w, h);
        }
    }

    /// Same design-first guarantee as the rest of the shell: no decorative
    /// block-element glyphs anywhere in the Settings overlay.
    #[test]
    fn settings_draws_with_no_decorative_block_elements() {
        let mut state = state_with_settings_open();
        let mut screens = vec![rendered(&state, 100, 30)];
        state.handle_key(press(KeyCode::Tab));
        screens.push(rendered(&state, 100, 30));
        state.handle_key(press(KeyCode::BackTab));
        state.handle_key(press(KeyCode::Enter));
        screens.push(rendered(&state, 100, 30));

        for screen in screens {
            if let Some(found) = screen
                .chars()
                .find(|c| ('\u{2580}'..='\u{259F}').contains(c))
            {
                panic!("a decorative block element ({found:?}) was drawn:\n{screen}");
            }
        }
    }

    // -----------------------------------------------------------------
    // Phase 2D: Providers and Launch Profiles.
    // -----------------------------------------------------------------

    fn provider_rows() -> Vec<ProviderRow> {
        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some("https://mirror.example.com/v1".to_owned()));
        vec![ProviderRow::new("my-router", config, Layer::User)]
    }

    fn profile_rows() -> Vec<ProfileRow> {
        vec![ProfileRow {
            name: "fast".to_owned(),
            config: ProfileConfig::new(IntegrationId::ClaudeCode),
            layer: Layer::User,
        }]
    }

    fn state_with_full_settings_open() -> ShellState {
        let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
        state.open_settings(
            harness_rows(),
            integration_rows(),
            provider_rows(),
            profile_rows(),
        );
        state
    }

    /// Routing shows every policy with its own provenance and explains the
    /// conditions on the free-resource preference. Memory shows its one real
    /// setting and layer the same way, and a toggle both changes the value
    /// and promotes its layer to `(user)` — the placeholder "not available"
    /// text this test used to require is the defect Phase 2D line 190 closes.
    #[test]
    fn routing_and_memory_sections_render_their_complete_honest_states() {
        let routing = RoutingRow::new(
            Layered::new(
                RoutingModelChoice::Pinned {
                    provider: "my-router".to_owned(),
                    model: "openai/gpt-5-mini".to_owned(),
                },
                Layer::Project,
            ),
            Layered::new(RouterLatencyMs::try_from(800).unwrap(), Layer::User),
            Layered::new(RouterCostMicroUsd::try_from(2_500).unwrap(), Layer::Default),
            Layered::new(false, Layer::Project),
            Layered::new(PremiumReservePercent::try_from(12).unwrap(), Layer::User),
            vec!["my-router".to_owned()],
        );
        // Premise, per §17: the memory row starts disabled at the project
        // layer, so a later assertion that a toggle changed it to "yes" and
        // `(user)` actually proves the toggle did something.
        let memory = MemoryRow::new(Layered::new(false, Layer::Project));
        let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
        state.open_settings_with_routing(
            harness_rows(),
            integration_rows(),
            provider_rows(),
            profile_rows(),
            routing,
            memory,
        );
        tab_to(&mut state, SettingsSection::Routing);
        let routing_text = rendered(&state, 120, 32);
        for expected in [
            "my-router:openai/gpt-5-mini",
            "800 ms",
            "$0.002500",
            "health, rate-limit, and latency",
            "below 12%",
            "(project)",
            "(user)",
            "(default)",
        ] {
            assert!(
                routing_text.contains(expected),
                "missing {expected:?}:\n{routing_text}"
            );
        }

        state.handle_key(press(KeyCode::Tab));
        let memory_text = rendered(&state, 120, 32);
        assert!(memory_text.contains("Memory"), "{memory_text}");
        assert!(memory_text.contains("no (project)"), "{memory_text}");

        state.handle_key(press(KeyCode::Char(' ')));
        let toggled_text = rendered(&state, 120, 32);
        assert!(toggled_text.contains("yes (user)"), "{toggled_text}");
    }

    /// Acceptance 1 (the render half): an empty Providers section shows an
    /// explanatory empty state rather than a blank list, and nothing panics.
    #[test]
    fn an_empty_providers_section_renders_an_empty_state() {
        let mut state = state_with_settings_open();
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        let text = rendered(&state, 100, 30);
        assert!(text.contains("No providers configured."), "got:\n{text}");
        assert!(
            text.contains("Providers"),
            "the tab label must show:\n{text}"
        );

        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
    }

    /// The Launch Profiles counterpart to the test above.
    #[test]
    fn an_empty_launch_profiles_section_renders_an_empty_state() {
        let mut state = state_with_settings_open();
        tab_to(&mut state, SettingsSection::LaunchProfiles);
        // 140, not 100: the strip spells `Launch Profiles` out only where all
        // seven long labels fit on its one row, and this assertion is about
        // that label. Below it the strip pans the short spellings instead —
        // `the_settings_tab_strip_never_hides_a_section_silently` is the test
        // for that half.
        let text = rendered(&state, 140, 30);
        assert!(
            text.contains("No launch profiles configured."),
            "got:\n{text}"
        );
        assert!(text.contains("Launch Profiles"), "got:\n{text}");

        for (w, h) in [(1, 1), (1, 40), (40, 1), (3, 3), (200, 60)] {
            rendered(&state, w, h);
        }
    }

    #[test]
    fn providers_and_profiles_appear_in_their_sections() {
        let mut state = state_with_full_settings_open();
        tab_to(&mut state, SettingsSection::Providers);
        let text = rendered(&state, 100, 30);
        assert!(text.contains("my-router"), "got:\n{text}");
        assert!(text.contains("openrouter"), "got:\n{text}");
        assert!(
            text.contains("https://mirror.example.com/v1"),
            "got:\n{text}"
        );

        tab_to(&mut state, SettingsSection::LaunchProfiles);
        let text = rendered(&state, 100, 30);
        assert!(text.contains("fast"), "got:\n{text}");
        assert!(text.contains("claude-code"), "got:\n{text}");
    }

    /// Acceptance 7 — the one test this file exists to make pass for
    /// providers and launch profiles: a credential variable set to an
    /// unmistakable secret-shaped value must never appear on any Settings
    /// screen this phase adds, across every section and every inline editor,
    /// while its NAME must. Asserted with `!contains`, never `assert_eq!` on
    /// the secret material itself — see `integrations`'s
    /// `the_doctor_report_names_variable_names_and_never_values`, which this
    /// mirrors for the TUI.
    #[test]
    fn no_credential_value_is_ever_rendered_across_every_settings_screen() {
        const VAR: &str = "GLASSHOUSE_VIEW_TEST_ONLY_SECRET_VAR";
        const SECRET_VALUE: &str = "sk-view-test-totally-real-looking-secret-xyz123";
        /// A second credential, typed straight into the Settings overlay
        /// rather than read from the environment — the Phase 9E path, and
        /// the one that would echo on screen if the field were not masked.
        const TYPED_VALUE: &str = "sk-view-test-typed-into-the-field-abc789";

        // SAFETY: `VAR` is unique to this test and is removed again below,
        // including before every early return this test has (it has none).
        unsafe {
            std::env::set_var(VAR, SECRET_VALUE);
        }

        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow::new("secret-test", config, Layer::User)];

        let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
        state.open_settings(harness_rows(), integration_rows(), rows, profile_rows());

        // Every screen is captured at a realistic width AND at a wide one.
        // At 100 columns the providers row is truncated, so a rendering that
        // leaked the credential's value would be clipped off-screen and this
        // test would pass for the wrong reason — verified by mutation: render
        // the value instead of set/not-set and only the wide capture fails.
        let mut screens = Vec::new();

        // Harnesses (the default section).
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));

        // Integrations.
        state.handle_key(press(KeyCode::Tab));
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));

        // Providers — the row itself must name the variable.
        state.handle_key(press(KeyCode::Tab));
        let providers_screen = rendered(&state, 100, 30);
        screens.push(rendered(&state, 400, 60));
        assert!(
            providers_screen.contains(VAR),
            "the variable NAME must be shown: {providers_screen}"
        );
        screens.push(providers_screen);

        // The "add a provider" wizard, both steps.
        state.handle_key(press(KeyCode::Char('a')));
        for c in "another".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));
        state.handle_key(press(KeyCode::Enter));
        for c in "openrouter".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));
        state.handle_key(press(KeyCode::Enter));

        // Editing the credential variable names of the original row ("another"
        // now sorts first, so one Down reaches "secret-test").
        state.handle_key(press(KeyCode::Down));
        state.handle_key(press(KeyCode::Char('c')));
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));
        state.handle_key(press(KeyCode::Esc));

        // The connectivity test — its result names the provider, the
        // protocol and the exact URL, but must never carry the credential's
        // value. The environment variable above is set, so this plans a real
        // request and renders the in-flight line; no socket is opened here,
        // because opening one is the run loop's job and this test has no run
        // loop.
        state.handle_key(press(KeyCode::Char('t')));
        let test_screen = rendered(&state, 400, 60);
        screens.push(rendered(&state, 100, 30));
        assert!(
            test_screen.contains("request in flight"),
            "the test result must say a request is running: {test_screen}"
        );
        screens.push(test_screen);

        // A finished probe's rendered outcome, one per shape, since each
        // formats a different set of fields. The first also clears the row's
        // in-flight marker, which is what lets `m` below start a second
        // request rather than being refused as a duplicate.
        for outcome in [
            crate::provider::discovery::ProbeOutcome::Rejected { status: 401 },
            crate::provider::discovery::ProbeOutcome::TimedOut { waited_ms: 10_000 },
            crate::provider::discovery::ProbeOutcome::Reached { status: 200 },
        ] {
            state.apply_provider_probe_result(crate::shell::state::ProviderProbeResult {
                provider: "secret-test".to_owned(),
                notice: crate::shell::state::ProviderNotice::Reachability(
                    crate::shell::state::ReachabilityCheck::Answered {
                        protocol: "openai-chat",
                        base_url: "https://openrouter.ai/api/v1".to_owned(),
                        endpoint: "https://openrouter.ai/api/v1/models".to_owned(),
                        outcome,
                    },
                ),
                catalogue: None,
            });
            screens.push(rendered(&state, 400, 60));
            screens.push(rendered(&state, 100, 30));
        }

        // A model refresh, on a provider whose model-list endpoint is
        // verified, and the same rule applies to its rendering.
        state.handle_key(press(KeyCode::Char('m')));
        let models_screen = rendered(&state, 400, 60);
        assert!(
            models_screen.contains("refreshing the model list"),
            "a running refresh must say so: {models_screen}"
        );
        screens.push(rendered(&state, 100, 30));
        screens.push(models_screen);

        // A cached catalogue on the row, timestamp and all — the one place a
        // model list is rendered, and therefore the one place a credential
        // could ride along with it.
        state.apply_provider_probe_result(crate::shell::state::ProviderProbeResult {
            provider: "secret-test".to_owned(),
            notice: crate::shell::state::ProviderNotice::Models(
                crate::shell::state::ModelRefresh::Refreshed {
                    count: 2,
                    fetched_at: 1_787_336_476,
                    endpoint: "https://openrouter.ai/api/v1/models".to_owned(),
                },
            ),
            catalogue: Some(crate::provider::cache::ModelCatalogue::new(
                "secret-test",
                "https://openrouter.ai/api/v1",
                "https://openrouter.ai/api/v1/models",
                1_787_336_476,
                vec![
                    crate::provider::cache::ModelEntry::new("vendor/one"),
                    crate::provider::cache::ModelEntry::new("vendor/two"),
                ],
            )),
        });
        let cached_screen = rendered(&state, 400, 60);
        assert!(
            cached_screen.contains("2 cached, fetched 2026-08-21 18:21:16Z"),
            "a cached model list must be rendered with the instant it was fetched: \
             {cached_screen}"
        );
        screens.push(cached_screen);
        screens.push(rendered(&state, 100, 30));

        // Typing a credential into the OS-secure-store field. The masked
        // rendering is asserted positively as well as with `!contains`, so
        // a field that rendered nothing at all could not pass this by
        // accident.
        state.handle_key(press(KeyCode::Char('s')));
        for c in TYPED_VALUE.chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        let credential_screen = rendered(&state, 100, 30);
        screens.push(rendered(&state, 400, 60));
        assert!(
            credential_screen.contains(&"*".repeat(16)),
            "a credential field must render as mask characters: {credential_screen}"
        );
        screens.push(credential_screen);
        state.handle_key(press(KeyCode::Esc));

        // The delete-a-stored-credential confirmation.
        state.handle_key(press(KeyCode::Char('x')));
        let confirm_screen = rendered(&state, 100, 30);
        screens.push(rendered(&state, 400, 60));
        assert!(
            confirm_screen.contains("secure store"),
            "the confirmation must say what it is about to do: {confirm_screen}"
        );
        screens.push(confirm_screen);
        state.handle_key(press(KeyCode::Esc));

        // A provider whose credential is recorded as living in the OS store
        // says so on its row — line 2 at the row level — and still without
        // a value anywhere near it.
        state.record_provider_credential_stored(
            "secret-test",
            crate::config::StoredCredentialRef::new("glasshouse", VAR),
        );
        let stored_screen = rendered(&state, 400, 60);
        assert!(
            stored_screen.contains("stored in the OS secure store"),
            "the row must say where the credential is kept: {stored_screen}"
        );
        screens.push(stored_screen);
        screens.push(rendered(&state, 100, 30));

        // Launch Profiles.
        state.handle_key(press(KeyCode::Tab));
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));

        // The project-write confirmation.
        state.handle_key(press(KeyCode::Char('W')));
        screens.push(rendered(&state, 100, 30));
        screens.push(rendered(&state, 400, 60));

        unsafe {
            std::env::remove_var(VAR);
        }

        for screen in &screens {
            for value in [SECRET_VALUE, TYPED_VALUE] {
                assert!(
                    !screen.contains(value),
                    "a credential value was rendered on screen:\n{screen}"
                );
            }
        }
    }

    /// Every line's text, joined — for asserting on what a helper produced
    /// without going through a `Buffer`, which pads and clips.
    fn lines_to_string(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **Acceptance test 1.** The result names what it actually reached —
    /// the protocol, the base URL, and the exact URL requested — and the
    /// disclaimer that used to sit under it is gone.
    ///
    /// The `!contains` half is the load-bearing one. The old line said
    /// "Glasshouse has no HTTP client yet", which stopped being true the
    /// moment `ureq` arrived with the gateway; a line that apologises for a
    /// check it is no longer failing to make is worse than no line, because
    /// it teaches the user to disbelieve a result that is now real.
    #[test]
    fn the_connectivity_result_names_what_it_reached_and_carries_no_disclaimer() {
        let lines = provider_test_result_lines(
            "router",
            &crate::shell::state::ReachabilityCheck::Answered {
                protocol: "openai-chat",
                base_url: "https://openrouter.ai/api/v1".to_owned(),
                endpoint: "https://openrouter.ai/api/v1/models".to_owned(),
                outcome: ProbeOutcome::Reached { status: 200 },
            },
        );
        let text = lines_to_string(&lines);

        assert!(text.contains("openai-chat"), "{text}");
        assert!(text.contains("https://openrouter.ai/api/v1"), "{text}");
        assert!(
            text.contains("GET https://openrouter.ai/api/v1/models"),
            "the exact URL requested must be named, or `reached` is unverifiable: {text}"
        );
        assert!(text.contains("answered 200"), "{text}");

        for disclaimer in [
            "Precondition check only",
            "not a real network request",
            "no HTTP client",
            "preconditions met",
        ] {
            assert!(
                !text.contains(disclaimer),
                "the disclaimer {disclaimer:?} must be gone — the request is real now: {text}"
            );
        }
    }

    /// **Acceptance test 2, on screen.** A rejected credential and an
    /// unreachable host must read as different problems, because they are:
    /// one is fixed with a key, the other with a URL or a network.
    #[test]
    fn a_rejection_and_an_unreachable_host_read_as_different_problems() {
        let rejected = describe_probe_outcome(&ProbeOutcome::Rejected { status: 401 });
        let unreachable = describe_probe_outcome(&ProbeOutcome::Unreachable {
            reason: "the connection was refused".to_owned(),
        });
        let timed_out = describe_probe_outcome(&ProbeOutcome::TimedOut { waited_ms: 10_000 });

        assert!(
            rejected.contains("reachable"),
            "a 401 must say the provider is there: {rejected}"
        );
        assert!(
            rejected.contains("401"),
            "and must name the status: {rejected}"
        );
        assert!(
            unreachable.contains("unreachable"),
            "and an unreachable host must say so: {unreachable}"
        );
        assert!(
            !unreachable.contains("did not accept the credential"),
            "an unreachable host says nothing about the credential: {unreachable}"
        );
        assert!(
            timed_out.contains("nothing came back"),
            "a timeout is its own third thing: {timed_out}"
        );
        assert_ne!(rejected, unreachable);
        assert_ne!(rejected, timed_out);

        // And they are coloured differently, so the difference survives being
        // skim-read: a provider that is there is not a red failure.
        assert_ne!(
            probe_outcome_color(&ProbeOutcome::Rejected { status: 401 }),
            probe_outcome_color(&ProbeOutcome::Unreachable {
                reason: String::new()
            })
        );
    }

    /// **Found running the real binary.** With the verb hard-coded, a
    /// refused connection rendered as "reached openai-chat at ... —
    /// unreachable — the connection was refused": a sentence that
    /// contradicts itself, and the same shape of defect as the "(not set) —
    /// stored in the OS secure store" row an earlier batch found the same
    /// way.
    ///
    /// Both directions are asserted. Checking only that "could not reach"
    /// appears would pass on a renderer that said it for a `200` too.
    #[test]
    fn a_result_that_never_reached_the_provider_does_not_claim_it_did() {
        let render = |outcome| {
            lines_to_string(&provider_test_result_lines(
                "p",
                &crate::shell::state::ReachabilityCheck::Answered {
                    protocol: "openai-chat",
                    base_url: "http://127.0.0.1:1/v1".to_owned(),
                    endpoint: "http://127.0.0.1:1/v1".to_owned(),
                    outcome,
                },
            ))
        };

        for outcome in [
            ProbeOutcome::Unreachable {
                reason: "the connection was refused".to_owned(),
            },
            ProbeOutcome::TimedOut { waited_ms: 10_003 },
        ] {
            let text = render(outcome.clone());
            assert!(
                text.contains("could not reach"),
                "a probe that got no answer must not say it reached anything: {text}"
            );
            assert!(
                !text.contains(": reached "),
                "and it must not contradict itself inside one sentence: {text}"
            );
        }

        for outcome in [
            ProbeOutcome::Reached { status: 200 },
            ProbeOutcome::Rejected { status: 401 },
            ProbeOutcome::Unexpected { status: 404 },
        ] {
            let text = render(outcome.clone());
            assert!(
                text.contains(": reached "),
                "an endpoint that answered {outcome:?} really was reached: {text}"
            );
            assert!(!text.contains("could not reach"), "{text}");
        }
    }

    /// **Found running the real binary.** A provider with no established
    /// model-list endpoint used to render "press m to fetch" — advertising
    /// a key that cannot ever fetch anything for it.
    #[test]
    fn a_row_never_advertises_a_refresh_key_for_a_provider_that_cannot_refresh() {
        // `ollama`'s model list is Unverified; `openrouter`'s is Verified.
        let row = ProviderRow::new(
            "local",
            crate::config::ProviderConfig::new("ollama"),
            Layer::User,
        );
        let text = lines_to_string(&[provider_models_line(&row, 1_000)]);
        assert!(
            !text.contains("press m"),
            "a provider that cannot refresh must not be told to press m: {text}"
        );
        assert!(
            text.contains("no model-discovery endpoint established"),
            "and it must say why instead of leaving a dead control unexplained: {text}"
        );

        // The counterpart, so this cannot pass by never offering the key.
        let text = lines_to_string(&[provider_models_line(&provider_row_with(None), 1_000)]);
        assert!(
            text.contains("press m"),
            "a provider that CAN refresh must still say so: {text}"
        );
    }

    /// A running request says it is running, and names the URL it is waiting
    /// on. This is the line that separates "slow" from "frozen".
    #[test]
    fn a_request_in_flight_says_so_on_screen() {
        let text = lines_to_string(&provider_test_result_lines(
            "router",
            &crate::shell::state::ReachabilityCheck::InFlight {
                protocol: "openai-chat",
                base_url: "https://openrouter.ai/api/v1".to_owned(),
                endpoint: "https://openrouter.ai/api/v1/models".to_owned(),
            },
        ));
        assert!(text.contains("in flight"), "{text}");
        assert!(text.contains("stays usable"), "{text}");
    }

    /// **Acceptance test 6, on screen.** A provider with no model discovery
    /// gets a sentence, and it is not styled as a failure.
    #[test]
    fn a_provider_without_model_discovery_is_explained_rather_than_reported_as_an_error() {
        let lines = provider_models_result_lines(
            "local",
            &crate::shell::state::ModelRefresh::NotOffered(
                "no model-discovery endpoint has been established for `local`".to_owned(),
            ),
        );
        let text = lines_to_string(&lines);
        assert!(text.contains("has been established"), "{text}");
        assert!(
            !text.contains("could not"),
            "not phrased as a failure: {text}"
        );
        assert_eq!(
            lines[0].spans[0].style.fg,
            Some(Color::DarkGray),
            "an explanation must not be coloured like an error, or the user goes looking \
             for a problem they do not have"
        );

        // ... where a genuine failure is.
        let failed = provider_models_result_lines(
            "router",
            &crate::shell::state::ModelRefresh::Failed("the connection was refused".to_owned()),
        );
        assert_eq!(failed[0].spans[0].style.fg, Some(Color::Red));
    }

    // --- the timestamp ----------------------------------------------------

    /// **Phase 9D line 3's "with a timestamp", rendered.**
    #[test]
    fn a_unix_timestamp_renders_as_an_unambiguous_utc_instant() {
        // Checked against `date -u -r <seconds>`.
        assert_eq!(format_unix_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(format_unix_utc(1_787_336_476), "2026-08-21 18:21:16Z");
        assert_eq!(format_unix_utc(1_000_000_000), "2001-09-09 01:46:40Z");
        // A leap day, which is where a hand-rolled calendar goes wrong.
        assert_eq!(format_unix_utc(1_709_164_800), "2024-02-29 00:00:00Z");
        // The day after, so the leap day is not merely being clamped.
        assert_eq!(format_unix_utc(1_709_251_200), "2024-03-01 00:00:00Z");
        // 2100 is not a leap year, which is the century rule most
        // hand-rolled conversions get wrong.
        assert_eq!(format_unix_utc(4_107_542_400), "2100-03-01 00:00:00Z");
        // Before the epoch, because `div_euclid` is what makes that work and
        // a plain `/` would not.
        assert_eq!(format_unix_utc(-1), "1969-12-31 23:59:59Z");
    }

    /// The age is what makes a stale cache *visibly* stale. The instant says
    /// when; this says how long ago, which is the question a user has.
    #[test]
    fn an_age_is_rendered_in_the_largest_unit_that_still_says_something() {
        assert_eq!(describe_age(1_000, 1_000), "just now");
        assert_eq!(describe_age(1_059, 1_000), "just now");
        assert_eq!(describe_age(1_060, 1_000), "1 minute ago");
        assert_eq!(describe_age(1_120, 1_000), "2 minutes ago");
        assert_eq!(describe_age(4_600, 1_000), "1 hour ago");
        assert_eq!(describe_age(87_400, 1_000), "1 day ago");
        assert_eq!(describe_age(1_729_000, 1_000), "20 days ago");
        assert_eq!(describe_age(20_000_000, 1_000), "7 months ago");
    }

    /// A clock that moved is said out loud rather than rendered as a negative
    /// age or quietly clamped to "just now" — the second would make a cache
    /// from the future look freshly fetched.
    #[test]
    fn a_timestamp_in_the_future_is_reported_rather_than_clamped() {
        let text = describe_age(1_000, 9_000);
        assert!(text.contains("future"), "{text}");
        assert!(text.contains("clock"), "{text}");
        assert_ne!(text, "just now");
    }

    // --- the model line on a provider row ---------------------------------

    fn provider_row_with(models: Option<crate::provider::cache::ModelCatalogue>) -> ProviderRow {
        let mut config = crate::config::ProviderConfig::new("openrouter");
        config.set_base_url(Some("https://openrouter.ai/api/v1".to_owned()));
        ProviderRow::new("router", config, Layer::User).with_models(models)
    }

    fn catalogue_at(
        base_url: &str,
        fetched_at: i64,
        count: usize,
    ) -> crate::provider::cache::ModelCatalogue {
        crate::provider::cache::ModelCatalogue::new(
            "router",
            base_url,
            format!("{base_url}/models"),
            fetched_at,
            (0..count)
                .map(|i| crate::provider::cache::ModelEntry::new(format!("vendor/model-{i}")))
                .collect(),
        )
    }

    #[test]
    fn a_cached_model_list_is_never_shown_without_when_it_was_fetched() {
        let row = provider_row_with(Some(catalogue_at(
            "https://openrouter.ai/api/v1",
            1_787_336_476,
            417,
        )));
        let text = lines_to_string(&[provider_models_line(&row, 1_787_336_476 + 86_400)]);
        assert!(text.contains("417 cached"), "{text}");
        assert!(
            text.contains("2026-08-21 18:21:16Z"),
            "the instant must be there: {text}"
        );
        assert!(
            text.contains("1 day ago"),
            "and so must the age, which is the half a user acts on: {text}"
        );
    }

    #[test]
    fn a_provider_with_no_cached_models_says_how_to_get_some() {
        let text = lines_to_string(&[provider_models_line(&provider_row_with(None), 1_000)]);
        assert!(text.contains("none cached"), "{text}");
        assert!(
            text.contains("press m"),
            "an empty state must name the key that fills it: {text}"
        );
    }

    /// A catalogue fetched from a base URL the provider no longer uses is a
    /// *wrong* list, not merely a stale one, and the row says which.
    #[test]
    fn a_catalogue_from_a_base_url_the_provider_no_longer_uses_is_flagged() {
        let row = provider_row_with(Some(catalogue_at("https://old.example/v1", 1_000, 9)));
        let line = provider_models_line(&row, 2_000);
        let text = lines_to_string(std::slice::from_ref(&line));
        assert!(text.contains("https://old.example/v1"), "{text}");
        assert!(
            text.contains("no longer this provider's base URL"),
            "{text}"
        );
        assert_eq!(line.spans[0].style.fg, Some(Color::Yellow));
    }

    #[test]
    fn a_request_in_flight_outranks_the_cached_list_on_the_row() {
        let mut row =
            provider_row_with(Some(catalogue_at("https://openrouter.ai/api/v1", 1_000, 9)));
        row.activity = Some(ProbeKind::ModelRefresh);
        let text = lines_to_string(&[provider_models_line(&row, 2_000)]);
        assert!(text.contains("refreshing the model list"), "{text}");

        row.activity = Some(ProbeKind::Connectivity);
        let text = lines_to_string(&[provider_models_line(&row, 2_000)]);
        assert!(text.contains("testing connectivity"), "{text}");
    }

    /// **The two-orders-of-magnitude range the packet named**, rendered at a
    /// realistic width and a narrow one.
    ///
    /// Nine models and four hundred and seventeen must both produce exactly
    /// one line: a renderer that grew with the catalogue would push every row
    /// below it off a short terminal, and the count is what makes the line
    /// length independent of the list length.
    #[test]
    fn a_catalogue_of_nine_and_of_four_hundred_and_seventeen_both_render_as_one_short_line() {
        let mut lengths = Vec::new();
        for count in [9usize, 417] {
            let row = provider_row_with(Some(catalogue_at(
                "https://openrouter.ai/api/v1",
                1_787_336_476,
                count,
            )));
            let line = provider_models_line(&row, 1_787_336_476);
            let text = lines_to_string(&[line]);
            assert_eq!(text.lines().count(), 1, "{text}");
            assert!(
                !text.contains("vendor/model-0"),
                "a row summarises a catalogue; it must never list it: {text}"
            );
            lengths.push(text.chars().count());
        }
        assert!(
            lengths[1] - lengths[0] <= 2,
            "the line's length must follow the count's digits and nothing else, got {lengths:?}"
        );
    }

    /// The whole Providers section, with a large catalogue cached, at a
    /// realistic terminal width and an absurdly narrow one. Ratatui clips;
    /// the assertion is that nothing panics and the rows are still rows.
    #[test]
    fn the_providers_section_survives_a_large_catalogue_at_every_width() {
        let rows = vec![provider_row_with(Some(catalogue_at(
            "https://openrouter.ai/api/v1",
            1_787_336_476,
            417,
        )))];
        let mut state = ShellState::new("glasshouse", "/work/glasshouse", "0.1.0", Vec::new());
        state.open_settings(harness_rows(), integration_rows(), rows, profile_rows());
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));

        for (w, h) in [(1, 1), (20, 5), (80, 24), (100, 30), (400, 60)] {
            let text = rendered(&state, w, h);
            assert!(
                !text.contains("vendor/model-200"),
                "a four-hundred-model catalogue must never be enumerated on screen"
            );
        }
        assert!(rendered(&state, 400, 60).contains("417 cached"));
    }

    /// Found running the real binary: refusing an unknown harness produces a
    /// long label (a name-enumerating prompt) that wraps by itself on a
    /// realistic terminal width, and a fixed bottom-panel height clipped the
    /// error line beneath it into invisibility. This renders the exact
    /// scenario and asserts the error text is actually on screen.
    #[test]
    fn an_unknown_harness_error_is_visible_not_clipped_by_a_wrapped_label() {
        let mut state = state_with_settings_open();
        tab_to(&mut state, SettingsSection::LaunchProfiles);
        state.handle_key(press(KeyCode::Char('a')));
        for c in "custom".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        state.handle_key(press(KeyCode::Enter));
        for c in "not-a-real-harness".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        state.handle_key(press(KeyCode::Enter));

        let text = rendered(&state, 100, 30);
        assert!(
            text.contains("not a harness Glasshouse knows"),
            "the refusal error must actually be visible on screen, not clipped:\n{text}"
        );
    }
}

#[test]
fn appearance_controls_stay_local_and_motion_can_pause() {
    let mut state = sample();
    let first = state.theme().name();
    state.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_ne!(state.theme().name(), first);
    assert!(rendered(&state, 140, 30).contains(state.theme().name()));
    assert!((0..6).any(|_| state.advance_artwork()));
    state.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    let paused = state.artwork_frame();
    assert!(!(0..30).any(|_| state.advance_artwork()));
    assert_eq!(state.artwork_frame(), paused);
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let selected_theme = state.theme().name();
    let action = state.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_ne!(action, super::super::state::Action::Redraw);
    assert_eq!(
        state.theme().name(),
        selected_theme,
        "session input belongs to its harness"
    );
}

#[test]
fn wide_landing_is_expressive_without_covering_a_native_terminal() {
    let mut state = sample();
    let landing = rendered(&state, 140, 32);
    assert!(landing.contains("YOUR WORK, IN VIEW"));
    assert!(landing.contains("waiting for you"));
    assert!(landing.chars().any(|c| c == '▀' || c == '▄'));
    state.set_viewport_grid(grid_from_lines(&["NATIVE SESSION CONTENT"]));
    let live = rendered(&state, 140, 32);
    assert!(live.contains("NATIVE SESSION CONTENT"));
    assert!(!live.contains("YOUR WORK, IN VIEW"));
    assert!(!state.advance_artwork());
}
