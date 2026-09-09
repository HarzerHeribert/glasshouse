//! The pill vocabulary's own geometry, proven without a terminal.
//!
//! What is *not* here: the tests that a click reaches the right key through a
//! real frame. Those need `view::render_recording`, so they live in
//! `view_tests.rs` beside the renderer they drive.

use super::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// The pill this file measures, in every state.
fn sample() -> Pill {
    Pill::key("s", "settings", KeyCode::Char('s'))
}

/// The marker slot is the whole reason a pill has one: a bar whose buttons
/// change width as focus moves reflows under the cursor.
#[test]
fn a_pill_is_the_same_width_focused_and_resting() {
    assert_eq!(sample().width(), sample().focused(true).width());
}

/// The other constant-width promise: a toggle pads its value slot to the
/// widest value its domain holds, so changing the value never moves the pills
/// to its right — which is what would make a click land on the wrong one.
///
/// Proven over the whole palette rather than over one pair, because the bar's
/// only toggle now carries the palette name and `violet`/`cobalt` are two
/// cells wider than `neon`: an unpadded slot would move every pill after
/// `t theme:` by two columns on a keystroke.
#[test]
fn a_toggle_pill_is_the_same_width_in_every_value_it_can_show() {
    let mut theme = Theme::default();
    let mut widths = Vec::new();
    let mut names = Vec::new();
    for _ in 0..8 {
        names.push(theme.name());
        widths.push(control_pills(theme).iter().map(Pill::width).sum::<u16>());
        theme = theme.next();
    }
    assert_eq!(
        theme.name(),
        Theme::default().name(),
        "eight palettes and back to the first: {names:?}"
    );
    assert!(
        widths.windows(2).all(|pair| pair[0] == pair[1]),
        "every palette name must fit one slot: {names:?} -> {widths:?}"
    );
    // The slot the bar reserves, stated where a new palette name would break
    // it: `Pill::toggle` pads, it does not clip.
    assert_eq!(names.iter().map(|name| name.len()).max(), Some(6));
}

/// `width()` is what `render_bar` positions by and what `hit` is measured
/// against, so it has to be the columns ratatui actually paints.
#[test]
fn a_pill_measures_the_columns_it_draws() {
    let pill = sample();
    let backend = TestBackend::new(40, 1);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut sink = Vec::new();
    terminal
        .draw(|frame| {
            render_bar(
                frame,
                Rect::new(0, 0, 40, 1),
                std::slice::from_ref(&pill),
                Theme::default(),
                &mut sink,
            );
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let row: String = (0..40).map(|x| buffer[(x, 0)].symbol()).collect();
    let drawn = row.trim_end();
    assert_eq!(
        u16::try_from(Line::from(drawn.to_owned()).width()).unwrap(),
        pill.width(),
        "measured and painted must agree: `{drawn}`"
    );
    assert_eq!(
        sink[0].rect.width,
        pill.width(),
        "and the hotspot must cover exactly what was painted"
    );
}

/// The bar wraps rather than clipping, and the reservation `view::regions`
/// makes from [`control_bar_rows`] has to be the height that walk needs — a
/// band one row short is the 168-column clip again, in a different place.
///
/// **Two walks, not one, since the bar became tiered**: the primary run gets
/// its own rows and the subordinate run wraps beneath it, so the reservation
/// has to be the sum. Computed here from `rows_for` over each half rather than
/// from `tiered_rows`, so the test is not the production arithmetic restated.
#[test]
fn the_reserved_height_is_the_height_the_bar_wraps_to() {
    for width in [40u16, 60, 80, 100, 120, 160, 240] {
        let pills = control_pills(Theme::default());
        let needed =
            rows_for(&pills[..CONTROL_PRIMARY], width) + rows_for(&pills[CONTROL_PRIMARY..], width);
        let area = Rect::new(0, 0, width, 40);
        assert_eq!(
            control_bar_rows(area),
            needed,
            "at {width} columns the bar wraps to {needed} rows and must be given them"
        );
    }
}

/// The ranking is real on screen and costs no action.
///
/// The user's complaint was hierarchy, not count — *"5 million buttons"* over
/// a bar where the one act a new install needs looked exactly like `d
/// decisions`. Both halves are asserted here because fixing the first by
/// deleting actions would be the 168-column clip returning: the primary run
/// leads, every primary pill is drawn plainly rather than subordinate, and the
/// list still holds every action.
#[test]
fn the_primary_actions_lead_the_bar_and_nothing_is_dropped_to_make_room() {
    let pills = control_pills(Theme::default());
    assert_eq!(
        CONTROL_PRIMARY, 3,
        "the primary run is the acts a first run needs, and there are three"
    );
    let labels: Vec<String> = pills.iter().map(Pill::text).collect();
    assert_eq!(
        &labels[..CONTROL_PRIMARY],
        &["n new", "c connect", "s settings"],
        "start a session, connect an account, settings — in that order"
    );
    for action in [
        "q quit",
        "o overview",
        "p project",
        "e events",
        "h health",
        "r routes",
        "d decisions",
        "k knowledge",
        "M memory",
        "N headless",
        "tab session",
        "enter session",
    ] {
        assert!(
            labels.iter().any(|label| label == action),
            "`{action}` was demoted, not deleted: it must still be in the bar"
        );
    }
}

/// A short terminal is not handed its whole screen as chrome.
#[test]
fn the_bar_is_clamped_on_a_short_terminal() {
    assert_eq!(control_bar_rows(Rect::new(0, 0, 80, 4)), 1);
    assert!(control_bar_rows(Rect::new(0, 0, 80, 24)) <= 6);
}

/// `walk_to` is the click's whole claim to being the keyboard path: the exact
/// presses, in the right direction, and none at all when the cursor is
/// already there.
#[test]
fn walking_to_a_row_is_the_arrow_presses_a_user_would_have_made() {
    assert!(walk_to(3, 3, KeyCode::Up, KeyCode::Down).is_empty());
    let down = walk_to(1, 4, KeyCode::Up, KeyCode::Down);
    assert_eq!(down.len(), 3);
    assert!(down.iter().all(|key| key.code == KeyCode::Down));
    let up = walk_to(4, 1, KeyCode::Up, KeyCode::Down);
    assert_eq!(up.len(), 3);
    assert!(up.iter().all(|key| key.code == KeyCode::Up));
}

/// **The hit test's positive half.**
///
/// `view_tests`'s `a_click_outside_every_pill_does_nothing` proves the
/// negative half, and on its own it is satisfied by a `hit` that answers
/// `None` to everything — which is the entire clickable-button feature dead.
/// Widening `column >= spot.rect.x` to `spot.rect.right()` does exactly that
/// and nothing anywhere noticed, so this is the test that watches the bound
/// from the inside: every pill, at its middle and at both end cells.
///
/// The four cells that must *not* answer are the ones a one-off error reaches:
/// the blank on either side of the pill, and the rows above and below the bar.
/// The bar is therefore drawn on a single row at a non-zero origin, so all
/// four of those cells exist and none of them belongs to another pill.
#[test]
fn a_click_inside_a_pill_answers_with_that_pills_keys() {
    let pills = control_pills(Theme::default());
    let one_row: u16 = pills.iter().map(Pill::width).sum::<u16>()
        + u16::try_from(pills.len() - 1).expect("one blank between neighbours");
    let area = Rect::new(2, 3, one_row, 1);
    let mut terminal =
        Terminal::new(TestBackend::new(area.right() + 2, area.bottom() + 2)).expect("terminal");
    let mut sink = Vec::new();
    terminal
        .draw(|frame| render_bar(frame, area, &pills, Theme::default(), &mut sink))
        .expect("draw");
    assert_eq!(
        sink.len(),
        pills.len(),
        "the row must be wide enough for every pill, or this proves nothing \
         about the ones it dropped"
    );

    for (index, spot) in sink.iter().enumerate() {
        let rect = spot.rect;
        let keys = spot.keys().to_vec();
        let middle = rect.x + rect.width / 2;
        for column in [middle, rect.x, rect.right() - 1] {
            let found = hit(&sink, column, rect.y).unwrap_or_else(|| {
                panic!("column {column} of pill {index} at {rect:?} must be clickable")
            });
            assert_eq!(
                found.keys(),
                keys.as_slice(),
                "column {column} must answer with pill {index}'s own keys"
            );
        }
        for (column, row, what) in [
            (rect.right(), rect.y, "the blank after the pill"),
            (rect.x - 1, rect.y, "the blank before the pill"),
            (middle, rect.y - 1, "the row above the bar"),
            (middle, rect.y + 1, "the row below the bar"),
        ] {
            assert!(
                hit(&sink, column, row).is_none(),
                "{what} at ({column}, {row}) is outside pill {index} at {rect:?}"
            );
        }
    }
}

/// **The band's spare row is its first, so the bar owns its last.**
///
/// The band's last row is the terminal's last row, and the note is the only
/// thing in the footer that is sometimes not drawn — so a note at the foot
/// leaves the terminal's bottom line unaddressed whenever there is no note,
/// which is the ordinary state. `view_tests`'s
/// `the_footer_bands_last_row_is_painted_in_every_state` is the same property
/// through a frame; this is the arithmetic under it.
///
/// The band's height is the same in both arrangements, which is what stops a
/// note resizing a live harness's pseudo-terminal.
#[test]
fn the_bands_spare_row_is_its_first_unless_an_overlay_covers_it() {
    let band = Rect::new(0, 46, 100, 4);

    let (keys, note) = split_band(band, false);
    assert_eq!(
        keys.bottom(),
        band.bottom(),
        "the bar must reach the band's last row"
    );
    assert_eq!(keys.height, band.height - 1);
    assert_eq!(note, Some(Rect::new(0, 46, 100, 1)), "the note goes first");

    // Under an overlay the first row is not the note's to have: the popup is
    // painted over it. The hint gives up the last row instead, and only when
    // there is a note to paint it.
    let (keys, note) = split_band(band, true);
    assert_eq!(keys.y, band.y);
    assert_eq!(keys.height, band.height - 1);
    assert_eq!(note, Some(Rect::new(0, 49, 100, 1)));

    // A one-row band keeps the bindings and drops the note, either way round.
    for under_overlay in [false, true] {
        let single = Rect::new(0, 23, 80, 1);
        assert_eq!(split_band(single, under_overlay), (single, None));
    }
}

/// The hint wraps between items and never inside one, so a key and the word
/// that says what it does cannot end up on different rows — and a wrapped row
/// never opens with the gap that separated them.
#[test]
fn wrapping_a_hint_breaks_between_items_and_never_inside_one() {
    let items: Vec<Vec<Span<'static>>> = ["tab section", "w save", "esc close"]
        .into_iter()
        .map(|text| vec![Span::raw(text)])
        .collect();

    let one_row = wrap_items(items.clone(), 80, 2);
    assert_eq!(one_row.len(), 1);
    assert_eq!(
        one_row[0].to_string(),
        "tab section  w save  esc close",
        "everything fits, so the gaps are the only thing between them"
    );

    // `tab section  w save` is nineteen columns and `esc close` is nine.
    let two_rows = wrap_items(items.clone(), 20, 2);
    assert_eq!(
        two_rows.iter().map(Line::to_string).collect::<Vec<_>>(),
        vec!["tab section  w save".to_owned(), "esc close".to_owned()],
        "the row that overflows starts at column zero"
    );

    // An item wider than the row still gets a row: dropping it would put a
    // key on no screen, which is the defect wrapping exists to fix.
    assert_eq!(
        wrap_items(items, 4, 2)
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>(),
        vec![
            "tab section".to_owned(),
            "w save".to_owned(),
            "esc close".to_owned()
        ]
    );
}
