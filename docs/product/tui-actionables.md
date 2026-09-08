# Actionables as buttons — one pill vocabulary for Glasshouse and pane

Status: **plan only.** Nothing implemented. Written 2026-09-09 from a reading of
`crates/glasshouse/src/shell/view/chrome.rs`,
`crates/glasshouse/src/shell/view.rs`,
`crates/glasshouse/src/shell/state/mod.rs`,
`crates/pane/src/tui.rs`, `crates/pane/src/tui/controls.rs` and
`crates/pane/src/session/ui.rs`, plus one live measurement of the shipped
binary under a fixed-size pseudo-terminal (below).

The request: *"Move away from single character selectors and use actual button
styled things for actionables in the tui. Same for pane."*

---

## 1. What already exists, named

**Some of the button vocabulary is here.** Four surfaces already draw a
selected thing with padding, a background fill and a marker glyph. They are
inconsistent with each other, and none of them is reusable.

| surface | file | padding | fill | marker | reusable? |
| --- | --- | --- | --- | --- | --- |
| session tab strip | `view/chrome.rs:77 render_session_bar` | `" {i} {harness} · {state} "` — a leading and trailing space (`:137 tab_label`) | selected: `fg Black`, `bg accent`, `BOLD` | none | no — layout and paint are one loop |
| harness picker rows | `view.rs:261 render_harness_choice` | `" {mark} {name} "` | selected: `fg Black`, `bg accent`, `BOLD` | `▸` / space | no |
| pane provider cards | `tui/controls.rs:363 render_providers` | 3-row card, `card_width` computed from `available / 22` | selected: `bg accent fg Black`; else `bg dock fg accent` | `◆` / `·`, plus a `━━━━━` underline on the selected card | no |
| pane panel rows | `tui/controls.rs:272 render_panel` | `"{mark} {text}"` | selected: `fg accent` only, no fill | `›` / space | no |

So the codebase already agrees on three things and has never written them
down: **a selected actionable gets a background fill, an accent colour, and a
marker character.** pane's provider card is the closest to a real button —
it is the only one with both a resting fill (`dock`) and a selected fill
(`accent`), which is exactly the pair a button needs.

**What does not exist anywhere:** a resting affordance. Every one of the four
draws *selection*; none draws *"this is a thing you can press."* An
unselected pane panel row and a line of prose are the same pixels.

### The key legend already separates key from description

`view/chrome.rs:338 render_footer` splits its hint on three spaces, then
splits each item on its first space, styling the key `accent + BOLD` and the
description `quiet`:

```rust
let (key, description) = item.split_once(' ').unwrap_or((item, ""));
```

That is the mnemonic/label split a button needs, already implemented. What is
missing is the container around it.

### Measured: the footer silently loses eight of its fifteen actions

`render_footer`'s control-mode hint lists fifteen items. Rendered — items
joined by two spaces — it is **168 columns wide**, and it is drawn as a
`Paragraph` with no `.wrap()`, so it **clips**.

Run against the shipped binary, `target/release/glasshouse` at `efdf3ea`+, in
a `pty.fork()` with `TIOCSWINSZ` set, ANSI stripped, tail of the frame:

```
$ python3 scratchpad/footer_probe2.py
===== cols=80 =====
'... t theme: neon   a motion: ontab sessionenter sessionf fullscreenn newN
headlesso overviewq quit'
   quit         True
   settings     False
   memory       False
   knowledge    False
===== cols=200 =====
'... tab sessionenter sessionf fullscreenn newN headlesso overviewq quits
settingsM memoryp projectk knowledgee eventsr routesh healthd decisions'
   settings     True
   memory       True
   knowledge    True
```

**At 80 columns the footer stops after `q quit`.** `s settings`, `M memory`,
`p project`, `k knowledge`, `e events`, `r routes`, `h health` and
`d decisions` are not drawn at all — 8 of 15 actions, invisible, with no
ellipsis and no overflow indicator. At 120 columns it clips mid-word: the row
ends `... k know`.

That is the defect this document fixes. It is not a styling preference.

### pane's chords are worse documented than Glasshouse's letters

pane's inventory, from `session/ui.rs:594-870` and `session/ui.rs:291-363`:

| binding | does | mentioned on screen? |
| --- | --- | --- |
| `Shift-Tab` | toggle execute/plan (`ui.rs:786`) | **nowhere** |
| `Ctrl-B` | sidebar (`ui.rs:817`) | only inside the notice `/sidebar` itself prints (`ui.rs:1023`) |
| `Ctrl-O` | compact (`ui.rs:813`) | 8 times, always as *"Ctrl-O expands"* next to a fold — never as a control |
| `Ctrl-T` | telemetry (`ui.rs:807`) | twice, in the `/telemetry` slash description |
| `Ctrl-F` | fullscreen (`ui.rs:824`) | once, in the `/fullscreen` slash description |
| `Ctrl-C` `Ctrl-D` `Ctrl-Home` `Ctrl-End` | interrupt, exit, scroll ends | nowhere |

`grep -rno "Ctrl-[A-Z]" crates/pane/src/tui.rs crates/pane/src/tui/` returns
`Ctrl-F` ×1, `Ctrl-O` ×8, `Ctrl-T` ×2, `Ctrl-U` ×2. And the one hint row pane
has — `"PgUp/PgDn chat · /cells inspect"` at `tui.rs:875` — is **replaced by
the token spend line as soon as `width >= 78` and a token count exists**
(`tui.rs:836`). On a normal terminal in a live session, pane shows no control
hints at all.

---

## 2. The button

A **pill**: one row, four cells of chrome around a label, state carried by a
character *and* by colour so the `mono` theme and a colourless terminal still
show focus.

```
▌ n new ▐          ▌▸n new ▐          ▌·n new ▐
 resting            focused            disabled
```

### Anatomy

```
▌  ·  n   new    ▐
│  │  │   │      └─ right cap,  1 cell
│  │  │   └──────── label,      n cells   ("new", "telemetry")
│  │  └──────────── mnemonic,   m cells   ("n", "^T", "S-Tab") + 1 separator
│  └─────────────── marker,     1 cell    ' ' resting · '▸' focused · '·' disabled
└────────────────── left cap,   1 cell
```

`width = 1 + 1 + m + 1 + n + 1 + 1` = **label length + mnemonic length + 5**.
A pill with a value slot (*A toggle pill shows its value*, below) adds
`1 + value_width`.

**The marker slot is why width never changes between states.** ` `, `▸` and
`·` are all one cell. A bar whose pills change width as focus moves reflows
under the cursor, which is a defect, not a style.

The end caps `▌` (U+258C) and `▐` (U+2590) are drawn with `fg` = the fill
colour on the page background, so they read as the pill's rounded ends rather
than as a border. This is the same half-block trick `appearance.rs:74` and
`tui/ribbon.rs:60` already use for the activity artwork.

### Colours, by role

Glasshouse's `Theme` has `accent · secondary · quiet` (`shell/appearance.rs`);
pane's has `accent · dock · backlight` (`tui.rs:137-152`). The pill needs a
fill that is neither the page nor the accent, which is exactly pane's `dock`.

**Step 1 adds `Theme::dock()` to `shell/appearance.rs`, copied row-for-row
from pane's table** (`tui.rs:140-151`). `shared-appearance.md` already rules
that the shared vocabulary is `accent · dock · quiet` and that pane's table is
the one to converge on; this is that line, taken early because a button needs
it.

| state | left/right cap | body bg | mnemonic | label | marker |
| --- | --- | --- | --- | --- | --- |
| resting | `fg dock`, bg Reset | `dock` | `accent` + BOLD | `quiet` | `' '` |
| focused | `fg accent`, bg Reset | `accent` | `Black` + BOLD | `Black` + BOLD | `'▸'` |
| disabled | `fg quiet`, bg Reset | **Reset** — hollow | `quiet` | `quiet` | `'·'` |

Disabled is the only hollow one. A pill with no fill does not look pressable,
which is the point.

### A toggle pill shows its value, and pads it

`t theme: neon`, `a motion: on`, pane's `execute`/`plan`, pane's `/effort` —
these are not commands, they are *states you change*. The pill carries the
value in its own slot:

```
▌ t neon   ▐        ▌ a motion on  ▐        ▌ S-Tab execute ▐
▌ t cobalt ▐        ▌ a motion off ▐        ▌ S-Tab plan    ▐
```

**The value slot is padded to the widest value in its domain**, so the pill
does not resize when the value changes. Domains, measured:

| pill | domain | widest | slot |
| --- | --- | --- | --- |
| `t` theme | neon amber ice violet cobalt mint rose mono | `violet`/`cobalt` | 6 |
| `a` motion | on, off | `off` | 3 |
| pane `S-Tab` mode | execute, plan (`tui/controls.rs:19-20`) | `execute` | 7 |
| pane `/effort` | auto low medium high xhigh max (`wire.rs:34-42`) | `medium` | 6 |

### At 80 and at 120 columns

Glasshouse control-mode footer, 80 columns, nothing focused:

```
▌ n new ▐ ▌ o sessions ▐ ▌ s settings ▐ ▌ ? all ▐                     ▌ q quit ▐
```

Same bar with `o sessions` focused, and with `n new` disabled (no harness is
enabled):

```
▌ n new ▐ ▌▸o sessions ▐ ▌ s settings ▐ ▌ ? all ▐                     ▌ q quit ▐
▌·n new ▐ ▌ o sessions ▐ ▌ s settings ▐ ▌ ? all ▐                     ▌ q quit ▐
```

120 columns — three more pills appear; nothing moves, nothing clips:

```
▌ n new ▐ ▌ o sessions ▐ ▌ s settings ▐ ▌ p project ▐ ▌ e events ▐ ▌ f full ▐ ▌ ? all ▐                       ▌ q quit ▐
```

pane's status bar, last row, 80 columns — resting, then with `^T` focused and
the mode toggled to `plan`:

```
▌ S-Tab execute ▐ ▌ ^T telemetry ▐ ▌ ^B sidebar ▐             spent 4.2k · exact
▌ S-Tab plan    ▐ ▌▸^T telemetry ▐ ▌ ^B sidebar ▐             spent 4.2k · exact
```

pane at 120:

```
▌ S-Tab execute ▐ ▌ ^T telemetry ▐ ▌ ^B sidebar ▐ ▌ ^F full ▐ ▌ /effort high   ▐ ▌ / commands ▐       spent 4.2k · exact
```

Glasshouse's title-row accessory, which today is the string
`t neon  ·  a motion on  ·  v0.1.0 ` (`chrome.rs:31-37`):

```
▌ t neon   ▐ ▌ a motion on  ▐  v0.1.0
```

Collapsed session-mode header at 80 columns, tabs as pills, exit as a pill,
root and model already dropped by `DROP_ORDER`:

```
GLASSHOUSE   ▌▸1 pane · running ▐▌ 2 claude · waiting ▐              ▌ ^] back ▐
```

### Mnemonic spelling

**The mnemonic is spelled as the user must type it, in the shortest
unambiguous form.** A letter binding is the bare letter (`n`, `o`, `s`); a
control chord is caret notation (`^T`, `^]`) because it is single-width,
ASCII, and what `stty` already prints; `Shift-Tab` is `S-Tab`; a slash command
is its slash form (`/effort`). Prose elsewhere keeps the spelled form
("ctrl-] for glasshouse") — the abbreviation is for bars where cells are
scarce, and `^] back` at 11 cells against `ctrl-] back` at 15 is four columns
of a header that must never drop its exit.

---

## 3. Where they go — and no button ever adds a row

**The rule: if a surface has no row already, it gets no buttons.** Every
placement below reuses a band that exists today, so the harness's viewport is
byte-for-byte the same size afterwards. `view.rs:42 regions()` and
`view.rs:60 session_regions()` are unchanged; `view.rs:77 viewport_slot()`
returns the same rectangle; `tui.rs:238 screen_regions()` returns the same
`ScreenRegions`.

| surface | rows today | buttons | rows after |
| --- | --- | --- | --- |
| Glasshouse control, footer (`regions()[4]`) | 1 | the action bar | **1** |
| Glasshouse control, title accessory (`chrome.rs:31`) | shares row 1 | `t theme`, `a motion` | **1** |
| Glasshouse session, header (`session_regions()[0]`) | 1 | tabs, `^] back` | **1** |
| Glasshouse fullscreen (`Chrome::None`) | 0 | **none** | **0** |
| Glasshouse overlays, hint line | 1 (inside the popup) | the overlay's own actions | **1** |
| pane status, last row (`regions.status`) | 3 at `<140`, 2 at `≥140` | the toggle bar | **3 / 2** |
| pane `StatusLine::Compact` | 1 | **none** | **1** |
| pane `StatusLine::Hidden`, pane fullscreen | 0 | **none** | **0** |

Two consequences worth stating because they are the hard cases:

- **Glasshouse fullscreen has no buttons at all.** `Chrome::None` exists to
  hand the harness every row; the only mark left is the transient note
  `render_fullscreen_hint` paints over the top row. A button bar there would
  undo the mode.
- **pane at `width >= 140` has only two status rows.** The toggle bar takes
  the *left of the last row*, which pushes `posture · connection` up to row
  one's right — where `mode · effort` sits today, and which the `S-Tab` and
  `/effort` pills have just vacated. The pills pay for their own space.

### What the bar drops as the terminal narrows

**Whole pills, from the end of a priority list. Never a partial pill.**

Control-mode priority, highest first: `? all` · `q quit` · `n new` ·
`o sessions` · `s settings` · `p project` · `e events` · `f full` ·
`h health` · `r routes` · `d decisions` · `k knowledge` · `M memory` ·
`N headless`.

`tab session` and `enter session` are **not** pills — they are motion, not
actions, and both are already discoverable by pressing an arrow. They live in
the palette as rows.

| available | drawn |
| --- | --- |
| ≥ 120 | 7 action pills + `? all` + `q quit` |
| ≥ 80 | 4 action pills + `? all` + `q quit` |
| ≥ 20 | `▌ ? all ▐ ▌ q quit ▐` |
| ≥ 13 | `? all  q quit` — pills degrade to bare labels |
| ≥ 6 | `q quit` |
| ≥ 1 | `q` |

**A pill degrades to its bare label rather than dropping the field.** That
rule is what protects the collapsed header: `^] back` is 7 cells bare and 11
as a pill, and it must survive every width a terminal can draw
(`chrome.rs:143-158`'s existing design note). `header_width()` at
`chrome.rs:278` must count the pill's four cells, or the header will drop the
root and the model four columns earlier than it does today — which is correct
behaviour, but it must be deliberate and it must be in the test.

---

## 4. Keyboard first — the letters do not move

**Every existing binding keeps working, unchanged, at every step.** A button
is a second way in, never a replacement. Two things are new: one key that
opens the palette, and a focus ring that re-targets three keys *only while the
bar has focus*.

### The key map, enumerated before claiming anything is free

`state/mod.rs:1021 handle_control_key`, complete:

| key | action |
| --- | --- |
| `Ctrl-C` / `Ctrl-Shift-C` | Quit |
| `q`, `Esc` | Quit |
| `Tab`, `Right` | next session |
| `BackTab`, `Left` | previous session |
| `Enter`, `i` | focus the session |
| `n` / `N` | new session / headless |
| `o` `s` `p` `k` `e` `r` `h` `d` `M` | overview, settings, project, knowledge, events, routes, health, decisions, memory |
| `t` (no ctrl) | cycle theme |
| `a` (no ctrl) | toggle motion |
| `f` (no ctrl) | toggle fullscreen |

Overlay handlers run *instead of* this table, never after it
(`state/mod.rs:944-1018`), and each claims only its own close key plus, for
the three with a cursor, `Up`/`Down`/`Enter`. Settings is the exception and
owns every key while open.

**Free, verified by that enumeration:** `?` · `/` · `Up` · `Down` · `Space` ·
digits `0`–`9` · `b g j l m u v w x y z` · every capital but `M` and `N`.

`Up` and `Down` falling through to `_ => Action::None` is the load-bearing
one — it is what makes a focus ring possible without rebinding anything.

### The palette: `?`

`?` opens `Overlay::Actions`: every control-mode action as a pill row with its
description, cursor + `Enter` to run it. `/` is deliberately **not** used —
`settings-picker.md` reserves it for search, and the palette gains search
later without a collision.

```
┌─ actions ───────────────────────────────────────────────────────────┐
│                                                    15 actions       │
│  ▸ ▌ n new         ▐   start a session in an installed harness      │
│    ▌ N headless    ▐   start one with no viewport                   │
│    ▌ o sessions    ▐   every session in this project, with detail   │
│    ▌ s settings    ▐   providers, harnesses, routing, memory        │
│    ▌ p project     ▐   what this project is doing right now         │
│    ▌ k knowledge   ▐   decisions, constraints, failed approaches    │
│    ▌ M memory      ▐   durable project memory                       │
│    ▌ e events      ▐   this session's recent lifecycle events       │
│    ▌ r routes      ▐   routing evidence                             │
│    ▌ h health      ▐   route health                                 │
│    ▌ d decisions   ▐   routing decisions already made               │
│    ▌ f full        ▐   fullscreen: a focused session, no chrome     │
│    ▌ t theme   neon▐   cycle the palette                            │
│    ▌ a motion  on  ▐   pause the artwork                            │
│    ▌ q quit        ▐   leave Glasshouse; sessions keep running      │
│                                                                     │
│  ▌ ↑↓ pick ▐ ▌ ⏎ do ▐ ▌ esc close ▐                                 │
└─────────────────────────────────────────────────────────────────────┘
```

Built exactly like `Overlay::HarnessChoice`, the worked example: an `Overlay`
variant (`state/mod.rs:76`), an `ActionsState { cursor }` beside
`HarnessChoice`, an `open_actions()` and a `handle_actions_key()` mirroring
`:725` and `:749`, a `render_actions()` in a new `view/palette.rs`, and one
arm in `render_footer`'s hint match.

### The focus ring, and the one collision it resolves

`ShellState` gains `focus: Focus`, either `Focus::Body` (everything today) or
`Focus::Bar(usize)`.

| key | in `Body` | in `Bar` |
| --- | --- | --- |
| `Up` | **enter the bar** at index 0 | — |
| `Down`, `Esc` | unbound / Quit | **leave the bar** back to `Body` |
| `Left` / `Right` / `Tab` / `BackTab` | previous / next session | move within the bar, wrapping |
| `Enter` | focus the session | activate the focused pill |
| any bound letter | its binding | **its binding, then focus returns to `Body`** |

**The single collision is `Left`/`Right`/`Tab`, and it is resolved by focus
location rather than by rebinding.** While the bar has focus the arrows are
not pointing at the session strip, so they cannot mean "next session"; the
moment focus leaves, they do again. This is the same arrangement Settings
already uses for `Tab` (`state/mod.rs:952-959`) — a key reused in a sub-state,
never shared.

`Esc` is the second, smaller one: it quits from `Body` and only leaves the bar
from `Bar`. The bar itself shows `▌ q quit ▐`, so the way out stays on screen.

**Session mode gets no focus ring.** `handle_key` returns
`handle_session_key` before any binding is consulted (`state/mod.rs:949`) —
every key belongs to the harness, and taking one back would be a regression
for every harness that uses it. Session-mode buttons are mouse targets only.

**pane gets no focus ring either, and that is a finding, not an omission.**
Its composer claims every printable character, both arrows (history recall and
completion selection, `session/ui.rs:329-338`), `Tab` (completion, `:339`) and
`Enter` (`:353`). The only free surfaces are `Esc` with nothing open —
`editor.key` does not match it, so it falls to `_ => {}` — and `Alt+<letter>`,
which `:360` explicitly excludes from insertion. Neither is a good focus
gesture: `Esc` is the universal "get me out" and `Alt` is a macOS
option-as-meta configuration question. **pane's keyboard path to its
actionables stays the slash palette it already has**, and the pills' job there
is to make the chords learnable and the state readable. A `▌ / commands ▐`
pill makes that path visible.

---

## 5. Clickability

Mouse mode is **`?1000h` + `?1006h`, written by hand, never crossterm's
`EnableMouseCapture` bundle, and never `?1003h`** — the measured rule from
`fullscreen-mode.md`, whose probe showed `?1003h` is the mode that leaks
Shift-drag and Command-drag away from the terminal and destroys text
selection. Nothing in this document needs motion tracking: a click is 2
reports, and there is no hover state anywhere in the design precisely because
hover is not available under `?1000h`. **That is why the resting/focused
distinction is keyboard- and marker-driven rather than hover-driven.**

`Event::Mouse` is translated today (`tui/event.rs:878`) and discarded today
(`shell/mod.rs:535`: `Event::Paste(_) | Event::Mouse(_) => {}`). Four things
are needed:

1. **Enable**, in `tui/mod.rs Screen::acquire`, beside `EnableBracketedPaste`,
   behind one top-level `Option<bool>` defaulting `false`, copied
   field-for-field from `memory_extraction` — the precedent
   `fullscreen-mode.md` already priced. Emit XTSHIFTESCAPE `CSI > 0 s` with
   it so the terminal's shift-to-select default is explicitly preserved.
2. **Disable** in `shutdown.rs restore_terminal()`, not in a `Drop` —
   `force_exit` calls `std::process::exit` and runs no destructor.
3. **One layout function per bar, producing the extents both the renderer and
   the hit test consume.** This is the whole reason buttons make clicking
   tractable: today's footer is a single `Paragraph` of spans and hit-testing
   it means re-deriving per-span offsets from the format strings. A pill bar
   is naturally `Vec<Placed { index, x, width }>` computed *before* drawing.
4. **Recompute, retain nothing.** `ShellState` has no geometry field and
   `render` takes `&ShellState`. The click handler calls `regions(area)` — the
   same function `viewport_slot` already calls outside the render pass — then
   `lay_out(...)`, then `hit(...)`.

**Measure with `Line::width()`, never `chars().count()`.** ratatui positions
spans by display width; pane's `abbreviate` (`tui.rs:1000-1015`) already uses
`Line::from(text).width()` and is correct, while `chrome.rs:100`'s pan loop
and `view.rs:2393 truncate_start` count characters. A pill label can contain
`SessionRecord.harness`, a free-form `String`, so the two metrics can disagree
and a click would land on the neighbouring pill. `Line::width()` needs no new
dependency.

**The cap glyphs are the one residual risk.** `▌` U+258C and `▐` U+2590 are
East Asian *Ambiguous*; `unicode-width` reports 1, and a CJK-configured
terminal may draw 2. pane already ships `▌` in `context_bar`'s parts array
(`tui.rs:951`), so the risk is pre-existing rather than new. Put both caps
behind one `const` in the button module: switching to `[` and `]` is then a
two-line change if a width report ever arrives.

**Scope of the hit test, initially:** `MouseEventKind::Down(MouseButton::Left)`
in control mode with no overlay, against the footer bar and the session bar.
Every other mouse event stays discarded. The wheel is **not** designed here —
`fullscreen-mode.md` step 4 owns it, and its measured 640–2369 reports per
gesture against a single-flight write path is a coalescing problem, not a
button problem.

---

## 6. The stages

Each step ships alone and improves the product alone. Each states the one
decision it makes and the test that fails if the decision is wrong.

### Step 1 — the pill, and `?`

**Ships:** `Theme::dock()`; a new `crates/glasshouse/src/shell/view/button.rs`
holding `Chip`, `ChipState`, `width()`, `spans()`, `lay_out()`, `render_bar()`,
`hit()`; a new `crates/glasshouse/src/shell/state/actions.rs` and
`crates/glasshouse/src/shell/view/palette.rs`; `?` bound in
`handle_control_key`; one arm added to `render_footer`'s hint match. The footer
itself is untouched.

**Improves alone:** all fifteen actions become visible and reachable at every
terminal width, including the eight the 80-column footer does not draw today.

**The decision: the palette re-dispatches the row's key through
`handle_control_key` rather than carrying its own copy of the actions.**

```rust
KeyCode::Enter => {
    let key = ACTIONS[cursor].key;
    self.overlay = None;
    self.actions = None;
    self.handle_control_key(KeyEvent::from(key), false)
}
```

That is what makes the palette structurally incapable of disagreeing with the
key map — which is the whole discoverability problem restated.

**Test:** `the_palette_runs_exactly_what_the_key_runs` — for every row *i*,
assert `open_actions(); cursor_to(i); Enter` returns the same `Action` as
`handle_key(row.key)` on an identical state. **Mutation:** give the palette its
own `Action` per row; the equality test must fail.

**Second test, the coverage half:** `every_control_binding_is_in_the_table` —
`include_str!("mod.rs")`, grep the `KeyCode::Char('x')` arms out of
`handle_control_key`, assert each appears in `ACTIONS`. This is the
source-scanning idiom `routing/mod.rs`, `harness/mod.rs`, `shell/mod.rs` and
`shell/view.rs` already use. Note for whoever splits `state/mod.rs` later: the
scan reads that one file and must follow the arms if they move.

**Caution for the implementer, not a package:** `check-file-sizes.py` reports
`state/mod.rs` as 42 production lines because it slices at the first
`#[cfg(test)]`, which is the `mod tests;` declaration on line 41. The file is
1,110 lines. The ratchet does not guard it. `view.rs` at **2,408** production
lines against the 2,500 ceiling is guarded and has 92 lines of headroom — which
is why every renderer above goes in a new file.

### Step 2 — the footer becomes a bar

**Ships:** `render_footer` builds `Chip`s from the same `ACTIONS` table and
calls `lay_out` + `render_bar`. The status note keeps its right-hand half
(`chrome.rs:378-390`); the bar lays out in what is left.

**The decision: whole pills are dropped by priority, and a partial pill is
never drawn.**

**Test:** `no_width_ever_draws_a_partial_button` — render the footer at every
width `1..=200` and assert the row's `▌` and `▐` counts are equal and
correctly interleaved, and that the row always contains either a way to the
palette or a way to quit. Today's footer fails this by construction: at 120
columns it draws `... k know`. **Mutation:** clamp by cells instead of by whole
pills; the test must fail at 120.

### Step 3 — pane's status bar becomes a toggle bar

**Ships:** `crates/pane/src/tui/button.rs`, a twin of Glasshouse's, and the
last `footer_row` in `render_screen` replaced by a pill bar for `S-Tab mode`,
`^T telemetry`, `^B sidebar`, `^F full`, `/effort` and `/ commands`.

**Improves alone, and it is the largest single discoverability win in either
product:** three chords with no on-screen documentation anywhere become
visible, and four toggles start showing their state instead of their name.

**The decision: a toggle pill carries state in its fill and pads its value slot
to the widest value in its domain.**

**Test:** `a_toggle_pill_is_the_same_width_in_every_state` — render the mode
pill as `execute` and as `plan`, the effort pill across all six values, and
assert identical `Line::width()`. **Second test, the disabled path:** with
`busy = true`, the mode pill renders the `·` marker and hollow styling, and
activating it produces the existing refusal notice *"Change mode after the
current task finishes."* (`session/ui.rs:792`) rather than a mode change.

**Rule 8 — the twin, not a shared crate.** `pane` is not in `default-members`
and must stay out of Glasshouse's dependency graph. The two button modules are
duplicated on purpose, and one `#[cfg(test)]` test in Glasshouse's binds them:

```rust
const PANE_BUTTON: &str =
    include_str!("../../../../pane/src/tui/button.rs");
```

It asserts that five declarations — `CAP_LEFT`, `CAP_RIGHT`, `MARK_RESTING`,
`MARK_FOCUSED`, `MARK_DISABLED` — are byte-identical in both files. Not the
whole file: the two crates have different `Theme` types and different callers.
`include_str!` reads a file and does not build the crate, so no dependency is
added. **Mutation:** change `MARK_FOCUSED` in pane alone; the drift test must
fail. This is the mechanism `shared-appearance.md` step 3 already chose for the
palette, used for the same reason.

### Step 4 — the collapsed header and the overlay hints

**Ships:** session tabs as pills in `render_session_bar`; `^] back` as a pill
in `render_header`; every overlay's hint line as a pill row, including
Settings, whose footer currently says `section keys edit`.

**The decision: a pill degrades to its bare label rather than dropping the
field, and `DROP_ORDER` is unchanged.**

**Test:** `the_way_out_survives_every_width` — render session mode at widths
`1..=200` and assert `back` is on screen at every one of them. **Mutation:**
make the exit field drop when its pill does not fit; the test must fail at
width 12, where `^] back` fits bare and `▌ ^] back ▐` does not.

### Step 5 — the focus ring

**Ships:** `Focus` on `ShellState`, `Up` to enter, `Down`/`Esc` to leave,
`Left`/`Right`/`Tab` re-targeted while focused, `Enter` to activate.

**The decision: `Left`/`Right`/`Tab` are re-targeted by focus location, not
rebound.**

**Test:** `arrows_move_the_bar_not_the_session_while_the_bar_has_focus` — with
`Focus::Bar(0)`, `Left` leaves `selected_index()` unchanged and moves the
focused pill; with `Focus::Body`, `Left` changes `selected_index()`.
**Second test, the promise this document makes:**
`a_letter_still_acts_while_the_bar_has_focus` — press `s` in `Focus::Bar(3)`,
assert `Action::OpenSettings` and `Focus::Body`. **Mutation:** let `Left` in
`Bar` fall through to `previous_session`; the first test must fail.

### Step 6 — clicking

**Ships:** the `?1000h ?1006h` enable behind the opt-in flag, the
`restore_terminal` disable, and the split of `shell/mod.rs:535`'s discard arm.

**The decision: one layout function produces the extents that both the
renderer and the hit test read.**

**Test:** `a_click_lands_on_the_button_that_was_drawn` — for every width
`20..=200`, for every column of the footer row, assert `hit()` returns `Some(i)`
exactly when the `TestBackend` buffer at that column belongs to pill *i*, using
the existing `rendered()` idiom at `shell/tests/view_tests.rs:77-90`.
**Mutation:** have the hit test recompute widths with `chars().count()`; the
test must fail on a harness name containing a wide glyph.

**Do not ship step 6 before the text-recovery prerequisite** that
`fullscreen-mode.md` names: Glasshouse's `vt100::Parser` is built with zero
scrollback rows at both production sites, and there is no clipboard or copy
mode anywhere, so terminal drag-select is currently the only way to get an
error message out of an embedded harness. Steps 1–5 need no capture at all.

---

## 7. What this deliberately does not do

- **No single-character selector is removed.** Every letter and every chord
  keeps working, at every step, in every mode. The buttons are a second door.
- **No new row anywhere.** If a surface has no row today, it gets no buttons —
  which is why fullscreen, pane's compact status and pane's hidden status all
  get nothing.
- **No hover.** `?1003h` is refused, so there is no hover event to style, and
  the design does not assume one.
- **No shared crate, no trait, no `glasshouse-ui`.** Rule 8. Two twin modules
  and one drift test.
- **No wheel.** `fullscreen-mode.md` step 4 owns it and its coalescing problem.
- **No search in the palette.** Fifteen rows do not need one, and `/` is
  reserved for `settings-picker.md`.
- **No redesign of what the actions are.** Same fifteen bindings, same
  behaviour, same order of consequence.

## 8. How this fits the other plans

- **`shell-chrome.md`** — its `DROP_ORDER` decision stands unchanged. This
  document only adds four cells per field to `header_width()`, and step 4's
  degrade-to-label rule is what keeps its "a user who cannot see how to get out
  is the failure this design exists to prevent" true at every width.
- **`fullscreen-mode.md`** — step 6 here *is* its step 3, generalised: it
  extracts one layout function per bar instead of one for the session strip.
  Its answer that nothing is retained in state is the answer used here. Its
  ruling that capture waits for a text-recovery path is carried, not relitigated.
- **`settings-picker.md`** — the picker's rows are pill rows, and `Action`
  rows become literally pressable. Its `Secret`-is-its-own-kind decision is
  untouched: a pill renders a value slot, and a `Secret` row's value is already
  masked before the renderer sees it.
- **`shared-appearance.md`** — step 1 here takes its `dock` role early,
  copied from pane's table, which is the direction that document already
  chose. The pill's accent is whatever the focused session wears, so a
  tab-as-pill is the legend that document wants.
- **Rule 8, no new coupling debt.** Every new function stays in the module
  that owns what it computes: `Chip` and `lay_out` in `view/button.rs` beside
  the bars that draw them; `ACTIONS` and `handle_actions_key` in
  `state/actions.rs` beside the key table they mirror; `hit` called from the
  event arm in `shell/mod.rs` that already owns input dispatch. `mod.rs` files
  stay dispatch and composition.
