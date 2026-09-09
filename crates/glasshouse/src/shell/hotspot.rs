//! Pills — the shell's actionables drawn as buttons — and the hit test that
//! turns a mouse report back into the key press one advertises.
//!
//! **The invariant: a hotspot is recorded only by the code that painted the
//! pill it belongs to, in the same pass, from the same rectangle.** Nothing
//! here is retained between frames; `super::run` clears the sink before every
//! draw and answers a click against the frame the user is actually looking at,
//! so the thing drawn and the thing clicked cannot drift apart.
//!
//! A hotspot carries **key presses**, not a [`super::state::Action`]. An
//! `Action` is only half of what a key does — `t` also cycles the theme on the
//! way past, and `Enter` also arms the fullscreen note — so replaying the key
//! through `ShellState::handle_key` is the only way a click can be a second
//! door to the same behaviour rather than a parallel path that can disagree
//! with it. See `super::run`'s pending queue, which is where they are replayed.
//!
//! Design: `docs/product/tui-actionables.md`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::appearance::Theme;

/// The pill's end caps.
///
/// **Brackets, not the half blocks `tui-actionables.md` drew.** Map line 1771
/// keeps this design text-first, and `view_tests`'s sweep enforces it by
/// refusing every character in U+2580..U+259F — the range `Gauge`, `Sparkline`
/// and `BarChart` are made of. `▌` and `▐` are in it. Weakening a map-line
/// guard to buy a nicer cap is the wrong trade, and the doc had already priced
/// this swap as its fallback for a second reason: both half blocks are East
/// Asian *Ambiguous* and a CJK-configured terminal may draw them two cells
/// wide, which would land a click on the neighbouring pill.
const CAP_LEFT: &str = "[";
const CAP_RIGHT: &str = "]";

/// The focus marker, and the cell reserved for it when nothing is focused.
///
/// Both are one cell, which is why a pill never changes width as focus moves.
/// A bar that reflows under the cursor is a defect, not a style.
///
/// A space follows the marker, which `tui-actionables.md`'s anatomy does not
/// draw. It buys the mnemonic a blank on both sides in every state, so a tab's
/// number reads as ` 2 ` whether or not it is the focused one — the shape the
/// pty tests match the bar by, and the shape a reader scans for.
const MARK_FOCUSED: &str = "▸";
const MARK_RESTING: &str = " ";

/// Widest value in the palette-name domain — `violet` and `cobalt`, six
/// cells. A toggle's value slot is padded to the widest value its domain
/// holds, so changing the value never resizes the pill and never moves the
/// pills to its right out from under the pointer.
const THEME_SLOT: usize = 6;

/// What a bar leaves where pills it could not draw would have been.
///
/// `‹` and `›` (U+2039/U+203A), not the half blocks: they are outside the
/// U+2580..U+259F range `view_tests`'s text-first sweep refuses, for the same
/// reason [`CAP_LEFT`] is a bracket. Each is drawn into a reserved slot two
/// columns wide rather than into a single cell, so a terminal that treats
/// them as East Asian *Ambiguous* and paints them two cells wide still lands
/// inside the slot instead of over the pill beside it.
const MARK_HIDDEN_BEFORE: &str = "‹";
const MARK_HIDDEN_AFTER: &str = "›";
const MARK_SLOT: u16 = 2;

/// What a piece of a pill is, which is the only thing its colour depends on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Cap,
    Mnemonic,
    Label,
    Value,
}

/// One actionable, drawn as a button and clickable at the rectangle it lands on.
///
/// `keys` is what the pill *is*: the exact key presses a user could have typed
/// instead. A pill whose keys are empty draws but does nothing, which is how a
/// legend and a control stay distinguishable in one row.
pub(super) struct Pill {
    mnemonic: String,
    label: String,
    value: Option<String>,
    focused: bool,
    /// Whether this pill is drawn as a *subordinate* one: the same width, the
    /// same hotspot, the same key — one shade quieter. See
    /// [`Pill::subordinate`].
    subordinate: bool,
    keys: Vec<KeyEvent>,
}

impl Pill {
    /// A pill for a single unmodified key.
    pub(super) fn key(mnemonic: &str, label: &str, code: KeyCode) -> Self {
        Self {
            mnemonic: mnemonic.to_owned(),
            label: label.to_owned(),
            value: None,
            focused: false,
            subordinate: false,
            keys: vec![KeyEvent::new(code, KeyModifiers::NONE)],
        }
    }

    /// A pill that shows the state it changes, in a slot padded to the widest
    /// value its domain can hold.
    fn toggle(mnemonic: &str, label: &str, value: &str, slot: usize, code: KeyCode) -> Self {
        let mut pill = Self::key(mnemonic, label, code);
        pill.value = Some(format!("{value:<slot$}"));
        pill
    }

    /// A pill reached by a run of key presses rather than one — a tab strip
    /// entry or an overlay row, where the keyboard path is "move the cursor,
    /// then act" and the click has to be exactly that path.
    pub(super) fn run(mnemonic: &str, label: &str, keys: Vec<KeyEvent>) -> Self {
        Self {
            mnemonic: mnemonic.to_owned(),
            label: label.to_owned(),
            value: None,
            focused: false,
            subordinate: false,
            keys,
        }
    }

    /// Mark this pill as the focused one — the selected tab, the cursor's row.
    pub(super) fn focused(mut self, yes: bool) -> Self {
        self.focused = yes;
        self
    }

    /// Draw this pill one shade quieter than its neighbours.
    ///
    /// **Hierarchy, not removal.** A subordinate pill keeps its width, its
    /// key, its hotspot and its place in the wrap — only its colour changes,
    /// so nothing it could do before becomes unreachable. That is the whole
    /// difference between demoting an action and deleting one, and it is the
    /// distinction [`control_pills`] rests on: fifteen equally-loud buttons
    /// and four quiet ones behind three loud ones contain the same fifteen
    /// actions.
    pub(super) fn subordinate(mut self, yes: bool) -> Self {
        self.subordinate = yes;
        self
    }

    /// The pill's pieces, in draw order. The single source both [`Self::width`]
    /// and [`Self::spans`] read, so a measured width is always the width drawn.
    ///
    /// **The marker and the separators travel with the text they precede**, in
    /// one span, rather than as pieces of their own. Cosmetically identical —
    /// they are spaces — but a terminal receives a style change as an escape
    /// sequence between two runs of text, and a pill whose body is chopped
    /// into three styled runs cannot be matched on by anything reading the
    /// raw stream. A pill with no mnemonic is a single run for the same
    /// reason: a session tab was one styled span before it was a pill, and
    /// `2 claude-code · running` has to stay one afterwards.
    fn pieces(&self) -> Vec<(String, Role)> {
        let marker = if self.focused {
            MARK_FOCUSED
        } else {
            MARK_RESTING
        };
        let mut pieces = vec![(CAP_LEFT.to_owned(), Role::Cap)];
        if self.mnemonic.is_empty() {
            pieces.push((format!("{marker} {}", self.label), Role::Label));
        } else {
            pieces.push((format!("{marker} {} ", self.mnemonic), Role::Mnemonic));
            pieces.push((self.label.clone(), Role::Label));
        }
        if let Some(value) = &self.value {
            pieces.push((" ".to_owned(), Role::Label));
            pieces.push((value.clone(), Role::Value));
        }
        pieces.push((" ".to_owned(), Role::Label));
        pieces.push((CAP_RIGHT.to_owned(), Role::Cap));
        pieces
    }

    /// The pill's mnemonic and label, spelled as they are drawn — `n new`.
    ///
    /// Test-only: production never needs a pill's text apart from the spans it
    /// paints, and an accessor a caller could lay out from would be a second
    /// place the bar's contents lived.
    #[cfg(test)]
    pub(super) fn text(&self) -> String {
        if self.mnemonic.is_empty() {
            self.label.clone()
        } else {
            format!("{} {}", self.mnemonic, self.label)
        }
    }

    /// Display columns, measured the way ratatui positions spans.
    ///
    /// `Line::width`, never `chars().count()`: a pill's label can carry a
    /// harness name, and the two metrics disagree on a wide glyph — by exactly
    /// enough to land a click on the neighbouring pill.
    pub(super) fn width(&self) -> u16 {
        let text: String = self.pieces().into_iter().map(|(text, _)| text).collect();
        u16::try_from(Line::from(text).width()).unwrap_or(u16::MAX)
    }

    fn spans(&self, theme: Theme) -> Vec<Span<'static>> {
        self.pieces()
            .into_iter()
            .map(|(text, role)| {
                Span::styled(text, style_for(role, self.focused, self.subordinate, theme))
            })
            .collect()
    }
}

/// A pill's colour, by the role of the piece and whether it is focused.
///
/// Resting is not bare prose: the caps carry the accent's quiet neighbour and
/// the mnemonic keeps the accent, so an unpressed button still looks like one
/// — the resting affordance `tui-actionables.md` found missing everywhere.
fn style_for(role: Role, focused: bool, subordinate: bool, theme: Theme) -> Style {
    if focused {
        return match role {
            Role::Cap => Style::default().fg(theme.accent()),
            _ => Style::default()
                .fg(Color::Black)
                .bg(theme.accent())
                .add_modifier(Modifier::BOLD),
        };
    }
    // A subordinate pill drops the accent and the bold and keeps everything
    // else. Colour is the only thing that changes, so the pill's width — and
    // therefore the hotspot the click lands in — is identical to a primary
    // one's; see `Pill::subordinate`.
    if subordinate {
        return Style::default().fg(theme.quiet());
    }
    match role {
        Role::Cap => Style::default().fg(theme.quiet()),
        Role::Mnemonic => Style::default()
            .fg(theme.accent())
            .add_modifier(Modifier::BOLD),
        Role::Label => Style::default().fg(theme.secondary()),
        Role::Value => Style::default().fg(theme.accent()),
    }
}

/// A pill's extent on the frame just drawn, and the keys clicking it replays.
#[derive(Clone)]
pub(crate) struct Hotspot {
    rect: Rect,
    keys: Vec<KeyEvent>,
}

impl Hotspot {
    /// The keys this hotspot stands for, oldest first.
    pub(crate) fn keys(&self) -> &[KeyEvent] {
        &self.keys
    }

    /// The cells this pill was painted into, which is exactly what [`hit`]
    /// tests against.
    ///
    /// Test-only on purpose: production never reads a hotspot's geometry, it
    /// asks [`hit`] about a click. An accessor a caller could lay out from
    /// would be a second place the frame's geometry lived.
    #[cfg(test)]
    pub(crate) fn rect(&self) -> Rect {
        self.rect
    }
}

/// The pill under a cell, or `None` if the click landed on nothing.
///
/// The bound check is the whole function: a click outside every pill must do
/// nothing at all, because the alternative — answering with whichever pill is
/// nearest — makes an unrelated part of the screen act like a button.
pub(crate) fn hit(hotspots: &[Hotspot], column: u16, row: u16) -> Option<&Hotspot> {
    hotspots.iter().find(|spot| {
        column >= spot.rect.x
            && column < spot.rect.right()
            && row >= spot.rect.y
            && row < spot.rect.bottom()
    })
}

/// Draw a row of pills over `area`, wrapping onto further rows, and record
/// where each one landed.
///
/// Whole pills only: a pill that does not fit on the row it reached moves to
/// the next, and one that does not fit on a row of its own is not drawn. A
/// half-drawn pill is a lie about where a click will land.
///
/// **What it could not draw, it says.** The rows can run out — the height
/// clamp in [`control_bar_rows`] is deliberate on a short terminal — and a
/// bar that quietly stops is the 168-column clip in a new place: the action
/// is on no screen and has no hotspot, so it is neither readable nor
/// clickable and nothing says it exists. The `›` costs one cell of the last
/// row it did reach, and is drawn only where no pill was.
pub(super) fn render_bar(
    frame: &mut Frame,
    area: Rect,
    pills: &[Pill],
    theme: Theme,
    sink: &mut Vec<Hotspot>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let walk = draw_pills(frame, area, pills, theme, sink);
    if walk.painted < pills.len() {
        let x = area
            .right()
            .saturating_sub(MARK_SLOT)
            .max(walk.tail.0.saturating_add(1));
        mark(frame, area, x, walk.tail.1, MARK_HIDDEN_AFTER, theme);
    }
}

/// Draw a strip that **pans** rather than wraps, so the pill at `focus` is
/// always on screen, and say with `‹`/`›` what that cost.
///
/// The session tab strip's shape, and why it is not [`render_bar`]:
/// `view::regions` gives the strip exactly one row, so a wrap puts every tab
/// past the second on a row that does not exist. The cursor moves to the
/// third session, the viewport switches to it, and the strip still shows the
/// first two — the focus marker on neither of them, and nothing saying the
/// rest are there. `header_fields`' own doc comment says the strip pans; this
/// is that, restored.
///
/// The same walk and the same hotspots as the wrapping bar — only the first
/// pill drawn differs — so a click on a panned tab lands where it looks.
pub(super) fn render_panned_bar(
    frame: &mut Frame,
    area: Rect,
    pills: &[Pill],
    focus: usize,
    theme: Theme,
    sink: &mut Vec<Hotspot>,
) {
    if area.width == 0 || area.height == 0 || pills.is_empty() {
        return;
    }
    // Nothing is hidden, so nothing needs marking or panning: the plain bar
    // is the same walk with `start` at zero.
    if rows_for(pills, area.width) <= area.height {
        render_bar(frame, area, pills, theme, sink);
        return;
    }

    let focus = focus.min(pills.len() - 1);
    let focused_width = pills[focus].width();
    // A marker is only worth its slot while the pill it is about still fits
    // beside it. On a strip too narrow for both, the session the user is
    // actually looking at wins — being shown the wrong tab is the defect,
    // and being told tabs are hidden is the consolation.
    let after = if area.width >= focused_width.saturating_add(MARK_SLOT) {
        MARK_SLOT
    } else {
        0
    };
    let mut start = pan_start(pills, area.width - after, focus);
    let mut before = 0;
    if start > 0 && area.width >= focused_width.saturating_add(after + MARK_SLOT) {
        before = MARK_SLOT;
        start = pan_start(pills, area.width - after - before, focus);
    }

    let content = Rect::new(
        area.x + before,
        area.y,
        area.width - before - after,
        area.height,
    );
    let walk = draw_pills(frame, content, &pills[start..], theme, sink);
    if start > 0 && before > 0 {
        mark(frame, area, area.x, area.y, MARK_HIDDEN_BEFORE, theme);
    }
    if start + walk.painted < pills.len() && after > 0 {
        mark(
            frame,
            area,
            area.right() - after,
            walk.tail.1,
            MARK_HIDDEN_AFTER,
            theme,
        );
    }
}

/// What a walk over the pills left behind.
struct Walk {
    /// How many of the pills handed in were painted — fewer than were handed
    /// in means some are on no screen, which is the only thing the markers
    /// are drawn from.
    painted: usize,
    /// The cell just past the last pill painted, which is where a marker can
    /// go without covering one.
    tail: (u16, u16),
}

/// The greedy walk both bars share: whole pills, left to right, onto the next
/// row when one does not fit, stopping when the rows run out.
///
/// One walk, so the wrapping bar and the panning strip cannot disagree about
/// where a pill lands, and so [`rows_for`] has exactly one arithmetic to
/// mirror.
fn draw_pills(
    frame: &mut Frame,
    area: Rect,
    pills: &[Pill],
    theme: Theme,
    sink: &mut Vec<Hotspot>,
) -> Walk {
    let mut walk = Walk {
        painted: 0,
        tail: (area.x, area.y),
    };
    let mut x = area.x;
    let mut y = area.y;
    for pill in pills {
        let width = pill.width();
        if width > area.width {
            continue;
        }
        if x > area.x && x + width > area.right() {
            y += 1;
            x = area.x;
        }
        if y >= area.bottom() {
            return walk;
        }
        let rect = Rect::new(x, y, width, 1);
        frame.render_widget(Paragraph::new(Line::from(pill.spans(theme))), rect);
        if !pill.keys.is_empty() {
            sink.push(Hotspot {
                rect,
                keys: pill.keys.clone(),
            });
        }
        walk.painted += 1;
        walk.tail = (rect.right(), rect.y);
        x = rect.right() + 1;
    }
    walk
}

/// Paint one overflow marker, or nothing if the row has no cell to spare.
///
/// Given room to the area's edge rather than a single cell, so a terminal
/// that draws the glyph two cells wide has somewhere to put the second one.
fn mark(frame: &mut Frame, area: Rect, x: u16, y: u16, glyph: &str, theme: Theme) {
    if x >= area.right() || y >= area.bottom() {
        return;
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            glyph.to_owned(),
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        )),
        Rect::new(x, y, area.right() - x, 1),
    );
}

/// The first pill a panned strip starts at so that `focus` is drawn: the
/// smallest one that leaves room for the run up to it, so the strip moves as
/// little as the selection makes it.
fn pan_start(pills: &[Pill], width: u16, focus: usize) -> usize {
    let mut start = 0;
    while start < focus && !fits(&pills[start..=focus], width) {
        start += 1;
    }
    start
}

/// Whether a run of pills fits one row `width` wide, the single blank between
/// neighbours included — the arithmetic [`draw_pills`] positions by, asked
/// ahead of time.
fn fits(pills: &[Pill], width: u16) -> bool {
    let mut used = 0u16;
    for (index, pill) in pills.iter().enumerate() {
        if index > 0 {
            used = used.saturating_add(1);
        }
        used = used.saturating_add(pill.width());
    }
    used <= width
}

/// How many rows [`render_bar`] would need for `pills` at `width`.
///
/// The same greedy walk `render_bar` makes, so the band reserved and the band
/// painted are the same height — see [`control_bar_rows`], which is what
/// `view::regions` asks before it splits the screen.
pub(super) fn rows_for(pills: &[Pill], width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    let mut rows = 1u16;
    let mut x = 0u16;
    for pill in pills {
        let pill_width = pill.width();
        if pill_width > width {
            continue;
        }
        if x > 0 && x + pill_width > width {
            rows = rows.saturating_add(1);
            x = 0;
        }
        x = x.saturating_add(pill_width).saturating_add(1);
    }
    rows
}

/// How many of [`control_pills`]'s entries are **primary** — drawn loud, on a
/// row of their own, before anything else.
///
/// Three, and they are the three acts a person who has just installed
/// Glasshouse has to perform: start a session, connect an account, open
/// settings. Everything else is something you do once you are already
/// running.
pub(super) const CONTROL_PRIMARY: usize = 3;

/// Control mode's action bar: every action the shell offers, **ranked**.
///
/// The user, on the fifteen-pill version of this bar: *"there are like 5
/// million buttons and i still dont see a way to actually add providers or
/// subscriptions"*. Both halves matter. The bar was not too long — every entry
/// is a real key and dropping one would put it back where the clipped
/// 168-column footer had it, on no screen and in no hotspot — it was **flat**.
/// Fifteen equally-weighted buttons say that fifteen things are equally worth
/// doing, so the one thing a new user must do first was rendered exactly as
/// loudly as `d decisions`.
///
/// **The rule this list applies: the first [`CONTROL_PRIMARY`] entries are the
/// acts a person who has nothing running needs, and they get their own row and
/// the accent; everything after them keeps its key, its click target and its
/// place, one shade quieter.** Demotion, never deletion — see
/// [`Pill::subordinate`], which changes colour and nothing else, and the
/// footer tests, which still assert every action is both drawn and clickable
/// at eighty columns.
///
/// Below the fold the order is `tui-actionables.md`'s measured priority — quit,
/// overview, project, events, theme, health, routes, decisions, knowledge,
/// memory, headless — with `tab`/`enter` last because they are motion rather
/// than actions and both are discoverable by pressing an arrow.
///
/// One table, read both by the footer that draws it and by the geometry that
/// reserves room for it, so an action cannot be added in one place and go
/// missing in the other. Every entry mirrors an arm of
/// `state::ShellState::handle_control_key`; clicking one replays that arm's
/// key, which is what makes the two structurally incapable of disagreeing.
///
/// **`f fullscreen` is not here, and it is not hidden — it is gone.** User
/// ruling 2026-09-09: a session on screen keeps Glasshouse's header whoever
/// holds the keyboard, and with the header staying, the mode `f` armed bought
/// exactly one row and cost the only visible way out. A key that toggles
/// nothing is worse than a missing one in a bar this user has already read as
/// *"5 million buttons"*.
///
/// `t theme` takes the slot it leaves, at the same count and for the reason
/// the fullscreen pill was given a value in the first place: `t` is an arm of
/// `handle_control_key` that this table did not mirror, and the palette it
/// selects was readable only from a status note the next keystroke erased.
pub(super) fn control_pills(theme: Theme) -> Vec<Pill> {
    let mut pills = vec![
        // Primary. Three, and in the order a first run needs them.
        Pill::key("n", "new", KeyCode::Char('n')),
        Pill::key("c", "connect", KeyCode::Char('c')),
        Pill::key("s", "settings", KeyCode::Char('s')),
    ];
    for pill in [
        Pill::key("q", "quit", KeyCode::Char('q')),
        Pill::key("o", "overview", KeyCode::Char('o')),
        Pill::key("p", "project", KeyCode::Char('p')),
        Pill::key("e", "events", KeyCode::Char('e')),
        Pill::toggle("t", "theme:", theme.name(), THEME_SLOT, KeyCode::Char('t')),
        Pill::key("h", "health", KeyCode::Char('h')),
        Pill::key("r", "routes", KeyCode::Char('r')),
        Pill::key("d", "decisions", KeyCode::Char('d')),
        Pill::key("k", "knowledge", KeyCode::Char('k')),
        Pill::key("M", "memory", KeyCode::Char('M')),
        Pill::key("N", "headless", KeyCode::Char('N')),
        Pill::key("tab", "session", KeyCode::Tab),
        Pill::key("enter", "session", KeyCode::Enter),
    ] {
        pills.push(pill.subordinate(true));
    }
    pills
}

/// The rows a tiered bar needs: the primary run on its own rows, then the rest
/// wrapped under it.
///
/// The same greedy walk [`rows_for`] makes, applied twice, so the band
/// reserved and the band painted are the same height for the tiered bar
/// exactly as they were for the flat one.
fn tiered_rows(pills: &[Pill], primary: usize, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    let split = primary.min(pills.len());
    let head = rows_for(&pills[..split], width);
    if split == pills.len() {
        return head;
    }
    head.saturating_add(rows_for(&pills[split..], width))
}

/// Draw a tiered bar: the first `primary` pills on the band's own first rows,
/// the rest wrapped beneath them.
///
/// **The break is what makes the ranking readable.** Left to the greedy wrap,
/// the primary row would fill up with whatever subordinate pills happened to
/// fit beside it, and a hierarchy that only holds at some widths is not one.
/// Both halves are drawn by [`render_bar`], so a pill in either tier lands
/// where it looks and records the hotspot that says so.
pub(super) fn render_tiered_bar(
    frame: &mut Frame,
    area: Rect,
    pills: &[Pill],
    primary: usize,
    theme: Theme,
    sink: &mut Vec<Hotspot>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let split = primary.min(pills.len());
    let head_rows = rows_for(&pills[..split], area.width);
    // **A band too short for two tiers gets one.** Reserving the whole of a
    // one-row band for the primary run would drop every other action off the
    // screen and out of the hotspot sink together — the 168-column clip, in a
    // new place. The ranking still holds where it can be seen at all: the
    // primary pills lead the walk and keep the accent, and only the row break
    // is given up.
    if split == pills.len() || head_rows >= area.height {
        render_bar(frame, area, pills, theme, sink);
        return;
    }
    let head = Rect {
        height: head_rows,
        ..area
    };
    render_bar(frame, head, &pills[..split], theme, sink);
    let tail = Rect {
        y: area.y.saturating_add(head_rows),
        // Saturating, though the guard above already makes it non-zero: this
        // module's own doc says nothing here computes a size by subtraction,
        // because that is how a "must not panic on a tiny terminal" promise
        // gets broken. Measured: mutating the guard away panicked here rather
        // than degrading.
        height: area.height.saturating_sub(head_rows),
        ..area
    };
    render_bar(frame, tail, &pills[split..], theme, sink);
}

/// Rows control mode's footer needs at this terminal's shape.
///
/// Width-and-height only, never the shell's state: `view::viewport_slot` hands
/// the harness's pseudo-terminal the rectangle this leaves, and a band whose
/// height moved when an overlay opened would resize a live harness for a
/// Glasshouse popup. The height clamp is what stops the bar eating a short
/// terminal; the width is what decides everything else.
pub(super) fn control_bar_rows(area: Rect) -> u16 {
    // Every toggle pads its value slot to the widest value in its domain, so
    // one call answers for every state the bar can be in — including the
    // palette, which is why this may pass any theme at all.
    let pills = control_pills(Theme::default());
    let ceiling = (area.height / 4).max(1);
    tiered_rows(&pills, CONTROL_PRIMARY, area.width).min(ceiling)
}

/// Rows control mode's whole footer band needs: [`control_bar_rows`] for the
/// pills, plus the one row the status note is given to itself.
///
/// **The note's row is reserved whether or not a note is showing**, and for
/// the same reason [`control_bar_rows`] never reads the shell's state: a band
/// that grew by a row when a note appeared would resize a live harness's
/// pseudo-terminal under the user, because `view::viewport_slot` hands the
/// harness whatever this leaves.
///
/// Giving the note a row of its own is what lets [`split_band`] hand the bar
/// the terminal's full width in every state. Before it, `render_footer` split
/// the note off the *right* — up to half the width — while this reservation
/// was computed from the whole of it, so at eighty columns with a note
/// showing the bar wrapped past the rows it had been given and seven of the
/// fifteen actions were drawn nowhere and recorded no hotspot: neither
/// readable nor clickable, with nothing saying they existed. The clamp is
/// applied to the total so a short terminal is not handed more chrome than it
/// was before the note had a row.
pub(super) fn control_band_rows(area: Rect) -> u16 {
    let ceiling = (area.height / 4).max(1);
    control_bar_rows(area).saturating_add(1).min(ceiling)
}

/// The footer band's two parts: the rows the pills get, and the row the note
/// gets.
///
/// **The note takes the band's first row, so whatever is always drawn — the
/// wrapped bar, or an overlay's hint — owns the band's last row, which is the
/// terminal's last row.** The note is the only thing in the footer that is
/// sometimes not there, and the band's foot is the one row that may never go
/// unpainted: ratatui's diff emits nothing for a row of default-styled
/// spaces, so a bottom row nothing writes is a bottom row the terminal never
/// hears about. Measured with the note at the foot instead: a terminal
/// resized to 100x50 was redrawn at the new size with its pills on rows
/// 47–49 and `\x1b[50;` never emitted at all, which
/// `terminal_loss::a_resize_still_arrives_on_a_terminal_that_has_been_silent`
/// reads — correctly — as an interface that never laid itself out for the
/// terminal it now has.
///
/// `under_overlay` puts the note back at the foot for the one state where the
/// first row is not the note's to have. An overlay is centred on the whole
/// terminal and painted after the bands, so at 80x24 its `Clear` covers the
/// band's first two rows: a note set from inside one — `saved to user
/// configuration`, which is a settings save's only acknowledgement — would be
/// drawn under the popup and seen by nobody. The caller passes true only when
/// a note is actually showing, so the row the hint gives up is a row the note
/// then paints and the foot stays written either way.
///
/// Split from the band that was handed in rather than recomputed from the
/// terminal, so what `view::regions` reserved and what
/// `view::chrome::render_footer` paints are the same rectangles by
/// construction — the property whose absence is described on
/// [`control_band_rows`]. Only which end of the band the spare row sits at
/// depends on the shell's state; the band's height does not, which is what
/// keeps a note from resizing a live harness.
///
/// A band with a single row keeps the bindings and drops the note: the
/// bindings are needed permanently and the note once.
pub(super) fn split_band(band: Rect, under_overlay: bool) -> (Rect, Option<Rect>) {
    if band.height <= 1 {
        return (band, None);
    }
    let note = Rect {
        y: if under_overlay {
            band.bottom() - 1
        } else {
            band.y
        },
        height: 1,
        ..band
    };
    let keys = Rect {
        y: if under_overlay { band.y } else { band.y + 1 },
        height: band.height - 1,
        ..band
    };
    (keys, Some(note))
}

/// Wrap a run of styled items onto as many rows as `width` needs, whole items
/// only — the greedy walk [`render_bar`] makes over pills, for text that is
/// read rather than clicked.
///
/// The overlay hints were a single unwrapped `Paragraph`: the Settings hint is
/// 97 columns, so at 80 the row stopped inside `r setup` and ` esc close` was
/// on no screen — and `esc` is the only key that closes that overlay. Below 70
/// columns `w save` went with it, leaving an overlay holding unsaved edits
/// with neither the key that saves them nor the key that leaves on screen.
/// Wrapping is the same answer the action bar already gave to the same defect
/// one row above.
///
/// The gap between two neighbours belongs to the item on its right and is
/// dropped when that item begins a row, so a wrapped row never opens with
/// blank columns.
pub(super) fn wrap_items(
    items: Vec<Vec<Span<'static>>>,
    width: u16,
    gap: u16,
) -> Vec<Line<'static>> {
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0u16;
    for item in items {
        let item_width = u16::try_from(item.iter().map(Span::width).sum::<usize>())
            .unwrap_or(u16::MAX)
            .max(1);
        if !current.is_empty() && used.saturating_add(gap).saturating_add(item_width) > width {
            rows.push(Line::from(std::mem::take(&mut current)));
            used = 0;
        }
        if !current.is_empty() {
            current.push(Span::raw(" ".repeat(usize::from(gap))));
            used = used.saturating_add(gap);
        }
        used = used.saturating_add(item_width);
        current.extend(item);
    }
    if !current.is_empty() {
        rows.push(Line::from(current));
    }
    rows
}

/// The presses that move a cursor from `from` to `to`, then act.
///
/// A click on a row is the keyboard path spelled out, not a shortcut past it:
/// exactly the arrow keys a user would have pressed, then the key that acts.
/// A parallel "select row N" path could disagree with the cursor handler; this
/// cannot, because it *is* the cursor handler.
pub(super) fn walk_to(from: usize, to: usize, back: KeyCode, forward: KeyCode) -> Vec<KeyEvent> {
    let (code, steps) = if to >= from {
        (forward, to - from)
    } else {
        (back, from - to)
    };
    (0..steps)
        .map(|_| KeyEvent::new(code, KeyModifiers::NONE))
        .collect()
}

#[cfg(test)]
#[path = "tests/hotspot_tests.rs"]
mod tests;
