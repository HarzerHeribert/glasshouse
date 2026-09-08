# Fullscreen and mouse — plan

Status: **plan only.** Nothing implemented. Written 2026-09-08 from five verified
investigations of `crates/pane/src/tui.rs`, `crates/pane/src/session/ui.rs`,
`crates/glasshouse/src/shell/`, `crates/glasshouse/src/session/runtime.rs`, and the
`vt100 0.16.2` and `crossterm 0.29.0` sources in the registry.

## Lead with the hard part

**Nested mouse routing — Glasshouse decoding a mouse report from the outer terminal and
re-encoding it into a hosted harness's pseudo-terminal — splits into two jobs with wildly
different costs, and the plan below never lets the expensive one block the cheap one.**

- **Wheel-only routing: about two days.** The negotiation half is free — `vt100` already
  parses the child's `?1000h/?1002h/?1003h/?1006h` into private state
  (`vt100-0.16.2/src/screen.rs:1139-1172`) and exposes it as `mouse_protocol_mode()` /
  `mouse_protocol_encoding()` (`screen.rs:578,584`), and Glasshouse already hands the shell
  a `&vt100::Screen` through `LiveSession::with_screen`
  (`crates/glasshouse/src/session/runtime.rs:501`, already called at
  `crates/glasshouse/src/shell/mod.rs:507`). The encoder is ~25 lines beside the existing
  key encoder (`crates/glasshouse/src/shell/state/overview.rs:596-620`), the hit test is
  the `viewport_slot()` call that already exists (`crates/glasshouse/src/shell/view.rs:58`),
  and there is a working precedent for synthesising escape bytes into a child
  (`runtime.rs:1481-1530`).
- **Click / drag / motion routing: a fortnight, and it is Red tier.** It needs a second
  delivery path that is neither user-precedence-marking nor event-logged, because today
  every forwarded byte starts a 10-second window in which machine-sent API messages to that
  session are refused (`runtime.rs:63`, `:1005-1026`, `:1163-1195`) and writes a durable
  event-log row (`runtime.rs:1019-1025` → `crates/glasshouse/src/events/log.rs:472`), while
  delivery itself is single-flight and *refuses* a concurrent write rather than queueing it
  (`runtime.rs:599-650`). It needs a tmux-shaped arbitration layer deciding whether a click
  belongs to Glasshouse's chrome or the harness. And it would actively break the hosted
  harness: pane's fragmented-escape repair reconstructs **wheel presses only** — it does
  `strip_suffix('M')` at `crates/pane/src/session/ui/terminal_input.rs:92` and accepts only
  codes 64/65 at `:99-101` — so a split click report reaches pane as a bare Escape plus the
  literal text `[<0;40;12M` typed into its composer.

**Recommendation: build the fortnight-scale half never, or at least not until something
concrete asks for it.** Steps 1 and 2 below deliver the fullscreen render the user asked
for in about two days, need no mouse capture at all, and carry no terminal-state risk.

## What is true today

### Both TUIs are already "fullscreen" in the alternate-screen sense

pane: alternate screen, raw mode, bracketed paste and mouse capture, all at
`crates/pane/src/session/ui.rs:411-418`. Glasshouse: alternate screen and raw mode at
`crates/glasshouse/src/shutdown.rs:256-264`, bracketed paste at
`crates/glasshouse/src/tui/mod.rs:57-63`. Neither draws a border around the embedded
harness — `view.rs:154-155` says "a live grid gets the *whole* area, with no border".

**So "fullscreen" here means chrome, not the alternate screen.** It is the mode where the
product's own furniture gets out of the way.

### pane already has two thirds of the mechanism

`crates/pane/src/tui.rs:230 pub fn screen_regions(area, &ScreenState) -> ScreenRegions` is a
pure function returning eight `Rect`s (`tui.rs:218-228`), `#[derive(Debug, Clone, Copy)]`,
and its doc comment already calls it "Disjoint hard bounds shared by the renderer and its
structural tests". Three of its inputs are already user-controlled presentation state:

| knob | field | set by |
| --- | --- | --- |
| status bar | `status_line: StatusLine::{Full,Compact,Hidden}` | `/statusline` — `ui.rs:990-998` |
| sidebar | `sidebar: SidebarVisibility::{Auto,Shown,Hidden}` | `/sidebar`, and Ctrl-B at `ui.rs:817-823` |
| activity ribbon | suppressed when idle or `telemetry_open` | `tui.rs:270-279` |

What has **no** knob is the header, which is unconditionally `min(2)` rows
(`tui.rs:264-265`). There is no single command that clears all of it at once.

Measured from that arithmetic (idle, no notice, no completions):

| terminal | transcript today | in fullscreen | gain |
| --- | --- | --- | --- |
| 80×24, no sidebar | 16 rows × 80 = 1280 cells | 21 × 80 = 1680 | **+31%** |
| 160×48, sidebar auto-shown | 41 rows × 124 = 5084 cells | 45 × 160 = 7200 | **+42%** |

### Glasshouse's chrome is four lines, and the collapse plan already recovers three

`crates/glasshouse/src/shell/view.rs:38 fn regions()` splits the terminal into title(1),
root(1), session bar(1), viewport(`Min 0`), footer(1). `view.rs:58 fn viewport_slot()`
returns band 4 and is the **single source of truth** for the size handed to a session's
pseudo-terminal and `vt100` emulator — its consumers are `shell/mod.rs:413` (resize) and
`shell/mod.rs:804 fn viewport_terminal_size`.

`docs/product/shell-chrome.md` (plan only, written 2026-09-08) makes `regions()` mode-aware:
control mode keeps five bands, session mode becomes `[header 1, viewport Min(0)]`. Its own
honest number is 36 → 39 rows on a 40-row terminal.

**Therefore the incremental prize of true zero-chrome fullscreen in Glasshouse is one row.**
39 → 40. That is worth saying plainly, because it means Glasshouse's fullscreen is not a
space feature — it is a *frame-free render*: the harness looks like it is running natively,
which is what matters for demos, screenshots, and harnesses that draw their own full-width
furniture. It is also the mode in which mouse routing is unambiguous, because there is no
Glasshouse chrome competing for the click.

### Mouse: what is actually wired

- Glasshouse **never** calls `EnableMouseCapture` — `grep -rn 'EnableMouseCapture' crates/`
  matches only `crates/pane/src/session/ui.rs:14,47,417`. Its `Event::Mouse` arm at
  `crates/glasshouse/src/shell/mod.rs:527` (`Event::Paste(_) | Event::Mouse(_) => {}`) is
  therefore dead code that has never fired in production, even though the translation exists
  at `crates/glasshouse/src/tui/event.rs:878`.
- pane **does** capture, and uses ~2% of it. There is exactly one `Event::Mouse` site in the
  crate (`ui.rs:611-632`) and it matches only `ScrollUp`/`ScrollDown`. `MouseButton` is never
  imported anywhere in pane; `mouse.column`/`mouse.row` appear in production code nowhere —
  the only occurrences under `crates/pane/src/` are the parser's own unit-test assertions
  (`terminal_input.rs:135,138`). There is provably no hit-testing.
- pane's wheel is **inert** exactly where it is most wanted: `ui.rs:620` guards the
  scrollback branch with `state.panel.is_none() && !state.telemetry_open`, and the expanded
  telemetry view has no scroll state at all (`grep -c scroll crates/pane/src/tui/telemetry.rs`
  → 0).
- pane running *inside* Glasshouse gets zero mouse input today. Its capture sequence is
  swallowed by the emulator: the child's bytes go only to a `Scrollback` and a `vt100::Parser`
  (`runtime.rs:1676`, `:1686-1688`) and are never relayed to the real terminal.

### Mouse capture and text selection — the corrected version

Two claims that a first pass got wrong, and the plan uses the corrected form:

1. **A narrower mode *is* selectable.** crossterm's `EnableMouseCapture` emits
   `?1000h ?1002h ?1003h ?1015h ?1006h` in one call
   (`crossterm-0.29.0/src/event.rs:322-335`), but crossterm's *decoding* is byte-driven and
   mode-agnostic — `parse.rs:168-169` dispatches on `b'M'`/`b'<'` and
   `parse_csi_sgr_mouse(buffer: &[u8])` (`parse.rs:718`) takes no mode argument. Writing
   `\x1b[?1000h\x1b[?1006h` by hand yields press/release events and no motion. Glasshouse
   already writes raw CSI in production (`runtime.rs:1506`, `:1512`). The one place "cannot"
   holds is the legacy Windows console path, where `is_ansi_code_supported()` is false
   (`event.rs:341-344`) and it becomes a single all-or-nothing `ENABLE_MOUSE_MODE` console
   flag (`crossterm-0.29.0/src/event/sys/windows.rs:39`) with no mode granularity.
2. **Narrowing does not buy back text selection.** Losing unmodified drag-select follows
   from enabling mouse reporting *at all*, not from `?1003h`. Ghostty attributes it
   mode-agnostically: `mouse-reporting = false` "allows you to always use the mouse for
   selection … without applications capturing mouse input"
   (`/Applications/Ghostty.app/Contents/Resources/man/man5/ghostty.5:1265-1277`). What
   narrowing to `?1000h`+`?1006h` actually buys is **event volume and fragmentation
   surface** — no wake-up per pointer move, and fewer reports to split across a read
   boundary. Both are worth having; neither is selection.
   What survives capture is *modified* drag-select: Ghostty's default
   `mouse-shift-capture = false` means shift-drag still extends the selection
   (`ghostty.5:1229-1251`), and a program can override that with XTSHIFTESCAPE. iTerm2 uses
   Option and ships an `iTermMouseReportingFrustrationDetector`. **Terminal.app's modifier I
   could not verify** and am not asserting.

### Why selection costs more in Glasshouse than in pane

- The `vt100` parser is constructed with **zero scrollback rows** at every production site:
  `runtime.rs:865` and `:1432`, both `vt100::Parser::new(size.rows, size.cols, 0)`.
- There is no clipboard, copy-mode or OSC 52 anywhere in either crate — a grep for
  `clipboard|copy_mode|osc52|arboard|pbcopy` over `crates/` returns zero matches.
- The 256 KiB raw scrollback that does exist (`runtime.rs:38`) is reachable only through the
  unix-socket/MCP API (`crates/glasshouse/src/api/mcp.rs:545`, tool
  `glasshouse_recent_output`); `crates/glasshouse/src/cli.rs`'s `Command` enum has no dump
  verb.

**So today, terminal drag-select is the only user-facing way to copy an error message out of
an embedded harness.** That single fact is why mouse capture is staged last and defaults off.

### Restoration is one line, and there is already a leak of the same shape

`crates/glasshouse/src/shutdown.rs:60-68 restore_terminal()` emits only `LeaveAlternateScreen`
and `cursor::Show`. It is what the panic hook (`:83-89`) and `force_exit` (`:179-183`, which
calls `std::process::exit` and runs no destructor) both go through. Bracketed paste is
enabled at `tui/mod.rs:61` and disabled only in `impl Drop for Screen` (`tui/mod.rs:118`) —
so it already leaks on the forced-exit path and nobody has noticed. A leaked mouse grab would
not go unnoticed.

### Settings have a home, and it needs no migration

Persisted settings are two layered TOML files — `<config_dir>/config.toml`
(`crates/glasshouse/src/paths.rs:97-99`) and `<root>/.glasshouse/config.toml`
(`config/loading.rs:24`) — never the database (schema 29, `database/schema.rs:16`; none of
its thirteen tables is a settings store). Neither `UserConfig` (`loading.rs:201-202`) nor
`ProjectConfig` (`:528-529`) uses `deny_unknown_fields`, and `CURRENT_SCHEMA_VERSION = 1`
(`loading.rs:19`) is compared only on **write** (`:513`, `:822`). The exact end-to-end
precedent for a persisted boolean is `memory_extraction`: `Option<bool>` on both layers
(`loading.rs:248-249`, `:562-563`), resolved project→user→default at `config/effective.rs:242-250`,
toggled with space at `settings/keys.rs:524-529`, written by both `w` and `W`
(`shell/mod.rs:2441`, `:2488`).

Two caveats to honour: `deny_unknown_fields` *does* exist on nested types reachable from
both layers — `EntitlementConfig` (`config/entitlement.rs:506`) and `ModelCapabilityRecord`
(`config/capability.rs:252`) — so a new setting must be **top-level**, not nested inside
`[entitlements.*]` or a provider's `model_capabilities`. And because we do not bump the
version, an older build will load a newer file, pass the guard and rewrite it without the new
key; `UserConfig` has no `#[serde(flatten)]` catch-all. That is a real round-trip drop, but
it is identical for every existing optional field, so it is a property of the design rather
than a new risk.

### pane cannot persist anything

`crates/pane/src/config.rs:104` states it and the code holds: nothing in that module opens a
path for writing, and `PaneConfig::load` (`config.rs:163`) is the only filesystem touch.
`docs/product/shared-appearance.md:58-61` separately **retracts** putting presentation in
`pane.toml`, and lists "No per-user or per-project theme file" as a non-goal (`:134`). So a
pane-side fullscreen preference is either session-lifetime or a startup flag — never a file.

## Measured, 2026-09-08 — the mouse probe, and what it changed

Run with `scripts/probes/mouse-probe.py` on the development machine, twice: once with
a mouse, once with a trackpad. Baseline (no mode enabled) delivered nothing in every cell,
so the experiment is controlled. Cells are `delivered / report-count`.

| mode | wheel | click | drag | **shift-drag** | alt-drag | **cmd-drag** |
| --- | --- | --- | --- | --- | --- | --- |
| baseline | no | no | no | no | no | no |
| `?1000h ?1006h` | yes/640 | yes/2 | yes/2 | **no** | yes/2 | **no** |
| `?1002h ?1006h` | yes/716 | yes/2 | yes/70 | **no** | yes/42 | **no** |
| `?1003h ?1006h` | yes/796 | yes/46 | yes/67 | **yes/1** | yes/31 | **yes/5** |

Three findings, each of which changes something.

**1. `?1003h` is the mode that destroys the escape hatch, and crossterm turns it on.**
Under `?1000h` and `?1002h` a Shift-drag and a Command-drag are **withheld from the
application** — they never arrive. Under `?1003h` both leak through.

**Confirmed by eye, same day.** "Not delivered" was equally consistent with the terminal
*keeping* the gesture for selection and with it *swallowing* the gesture entirely, so the
operator checked on screen. Results, in cmux:

| gesture | probe says | on screen | reading |
| --- | --- | --- | --- |
| plain drag, baseline | not delivered | **highlights** | control is valid |
| plain drag, capture | delivered | does not highlight | the app took it |
| **shift-drag, capture** | **not delivered** | **highlights** | **escape hatch is real** |
| **cmd-drag, capture** | **not delivered** | **highlights** | **second escape hatch** |
| wheel, capture | delivered | terminal no longer scrolls | the app took it |

**Shift-drag and Command-drag survive mouse capture and still select text.** That is the
fact the whole cost argument rested on, and it holds: enabling `?1000h ?1006h` costs plain
drag-select and nothing else.

Two caveats to carry, neither of them blocking:

- **Alt-drag is anomalous.** The probe measured it *delivered* in every capture mode, and the
  operator also reported it highlighting. Both being true would mean cmux forwards it and
  selects. It is not decision-relevant — document **Shift**, which is unambiguous on both
  sides — but do not claim anything about Option without re-measuring.
- **This is cmux, and only cmux.** Terminal identity was recorded in the results file.
  Glasshouse and pane run anywhere, and a different terminal may withhold different
  modifiers. Ship the escape hatch as "your terminal's selection modifier, commonly Shift",
  not as a promise, and keep `mouse-probe.py` so any user can settle it on their own machine
  in three minutes.
- Whether a Shift-selection then *copies* needs no test on this machine: cmux runs on
  Ghostty, and `~/.config/ghostty/config:33` already sets `copy-on-select = clipboard`,
  which Ghostty documents as "always copy text to the selection clipboard as well as the
  system clipboard". **A Shift-drag under mouse capture therefore selects and copies in one
  gesture, with no Cmd-C.**

  That lowers the cost of capture further than the table above suggests: what a user loses
  is plain-drag, and what they keep is a single-modifier gesture that both selects and
  copies. It is a *terminal* setting, not something either product provides — so the
  user-facing guidance is "turn on your terminal's copy-on-select" (iTerm2 calls it *Copy to
  pasteboard on selection*; Ghostty, WezTerm and Alacritty all have one), never a promise
  that Glasshouse or pane will do it. Neither has any clipboard path at all: `grep -rn
  'OSC\|clipboard\|pbcopy' crates/pane/src/` returns nothing, and adding OSC 52 would only
  be needed if pane ever owned selection itself, which the plan explicitly declines. crossterm's `EnableMouseCapture` emits `?1000h ?1002h ?1003h`
in one call (`crossterm-0.29.0/src/event.rs:322-335`), so **the convenience API is precisely
the mode that costs the most.** Writing `\x1b[?1000h\x1b[?1006h` directly keeps the hatch,
and crossterm still decodes it because its parser is byte-driven and mode-agnostic
(`parse.rs:168-169`, `parse_csi_sgr_mouse` at `:718` takes no mode argument).

**Rule: never enable `?1003h`. Neither product needs any-motion tracking.**

**2. Shift and Command are the escape hatches; Option is not to be relied on.** Earlier
drafts said "most terminals keep Option or Shift". Shift is confirmed on both sides —
withheld from the application AND highlighting on screen. Option is measured as delivered in
every capture mode (`yes/2`, `yes/42`, `yes/31`) and is therefore not something to promise.
Document **Shift**.

**3. The wheel burst is two to three orders of magnitude larger than a click, and it is the
real cost of step 4.** One scroll gesture produced **640 reports on the mouse and 2369 on the
trackpad** in `?1000h`, against 2 for a click. The trackpad runs ~3x the mouse throughout,
which is momentum scrolling.

That number lands on the delivery path this document already identified as single-flight and
*refusing* a concurrent write rather than queueing it (`runtime.rs:599-650`). **Forwarding
wheel reports one-for-one is not viable**; step 4 needs coalescing — collapse a burst into a
line-count delta and send that — which is design work its "~2 days" estimate did not include.
Step 3 (click a session tab) is unaffected: a click is 2 reports.

## The staged plan

Each step is independently shippable. Steps 1 and 2 answer the user's literal request. Steps
3 and 4 are the mouse, and each is optional in the strict sense: not building it leaves a
coherent product.

| step | what it delivers | tier | rough cost | needs mouse capture? |
| --- | --- | --- | --- | --- |
| 1 | pane fullscreen | Amber | ~1 day | no |
| 2 | Glasshouse fullscreen | Amber | ~1 day | no |
| 3 | click a session tab in Glasshouse | Amber | ~1–2 days | **yes** |
| 4 | wheel scroll into the hosted harness | Amber | ~2 days | **yes** |
| 5 | click/drag/motion into the harness | Red | ~2 weeks | yes — **refused** |

### Step 1 — pane fullscreen

**After this step the user can:** press Ctrl-F (or type `/fullscreen`) in pane and get the
transcript alone — no header, no status bar, no sidebar, no activity ribbon, composer still
there — gaining 31% more visible transcript on an 80×24 terminal and 42% more on a 160×48 one
with the sidebar showing. Pressing it again restores exactly what was there before. Nothing
about the terminal's mouse or selection changes.

**Files touched**

- `crates/pane/src/tui.rs` — one field on `ScreenState` (`tui.rs:32-33`, which derives
  `Default`, and every test literal spreads `..ScreenState::default()`), and the arithmetic in
  `screen_regions` (`tui.rs:230-321`): `status_h`, `header_h` and `activity_h` become 0 and
  `sidebar_visible` becomes false when the flag is set.
- `crates/pane/src/session/ui.rs` — `KeyCode::Char('f')` in the CONTROL block at `:796-840`
  (`f` is free: the block claims c, t, o, b, d, Home, End, and the editor claims p, n, a, e,
  u, k at `:292-305`), plus the `/fullscreen` branch beside `/sidebar` at `:1002-1010`.
- `crates/pane/src/tui.rs:325 slash_matches` — one entry, beside `/sidebar` and `/statusline`.
- `crates/pane/src/session/controls.rs:215` — the `/config` "Presentation: /theme · /sidebar ·
  /statusline" line gains `· /fullscreen`.
- Optional and separable: `--fullscreen` on `SessionArgs` (`crates/pane/src/session.rs:401-403`),
  which is how Glasshouse could later launch a pane already in it — the same one-flag shape
  `shared-appearance.md:80-89` proposes for `--theme`.

**The seam.** `screen_regions` is already pure, already `Copy`, and already the shared
contract between the renderer and its structural tests (`tui.rs:218-230`). Fullscreen is one
more input to it. No render function changes: a zero-height `Rect` renders nothing, and the
one place that indexes into the header already guards `regions.header.height > 0`
(`tui.rs:654`).

**The single decision: the hide-set — what fullscreen removes and what it keeps.** The
composer stays; everything else goes. A mutation should attack exactly that: make fullscreen
also zero `input_h`, and a test that types a character in fullscreen and asserts it is on
screen must fail. (Test home: `crates/pane/tests/tui.rs`, which already exercises
`screen_regions` structurally, plus one live-PTY case in `crates/pane/tests/tui_live.rs`
alongside the existing `fragmented_mouse_reports_do_not_become_prompt_text` at `:504`.)

**Not in this step:** persisting it. pane cannot (`config.rs:104`), and
`shared-appearance.md:58-61` has already ruled out the file that would otherwise be the home.

### Step 2 — Glasshouse fullscreen

**Prerequisite:** the header-collapse package (`docs/product/shell-chrome.md`) lands first,
because it is what makes `regions()` mode-aware. Fullscreen is a third arm of that same
function, not a second layout mechanism.

**After this step the user can:** press `f` in control mode to arm fullscreen, then Enter/`i`
to focus a session and see the harness occupy the entire terminal with no Glasshouse
furniture at all — the harness is told it has the whole screen, so it lays itself out for it.
Ctrl-] still returns to control mode. Over today that is 36 → 40 rows on a 40-row terminal
(+11%); over the collapsed header it is one row, and the real deliverable is the frame-free
render.

**Files touched**

- `crates/glasshouse/src/shell/view.rs` — `regions()` (`:38`) gains a fullscreen arm returning
  `[Min(0)]`; `viewport_slot()` (`:58`) picks the right index per mode; `render()` (`:63-70`)
  skips the chrome calls.
- `crates/glasshouse/src/shell/state/mod.rs` — one `bool` on `ShellState` (`:477-531`), and
  `KeyCode::Char('f')` in `handle_control_key` (`:840-900`); `f` is free — the table claims
  Ctrl-C, q, Esc, Tab/Right, BackTab/Left, o, s, t, a, p, k, e, r, h, d, M, Enter/i, n, N.
- `crates/glasshouse/src/shell/mod.rs` — nothing, if `viewport_terminal_size` (`:804`) keeps
  calling `viewport_slot`. That is the point of the seam.

**The seam.** `viewport_slot()` is the single source of truth for the PTY and emulator size
(`view.rs:47-57`, consumers at `shell/mod.rs:413` and `:804`). Changing the bands changes what
the harness is told, automatically and in one place.

**The single decision: fullscreen changes the PTY size, not just the paint.** The mutation is
to make `viewport_slot()` return the pre-fullscreen band while `render()` paints fullscreen —
a test asserting `viewport_terminal_size` equals the terminal's own size in fullscreen mode
must fail. This is the failure that is invisible on screen and catastrophic in the harness:
it draws for space it does not have.

**Deliberate constraint honoured:** no new reserved chord in session mode. The toggle lives in
control mode. `crates/glasshouse/src/shell/state/overview.rs:524-536` documents that
"everything is forwarded to the focused PTY untouched — including `q`, Tab, and Ctrl-C —
except the one reserved escape chord", and adding a second one takes a key away from every
harness forever.

**One honest wart:** in fullscreen there is no on-screen "ctrl-] back". Mitigate with a
one-shot status line on entry — the `status` field already exists on `ShellState` and every
keystroke clears it (`state/mod.rs:770`). Do not mitigate by keeping a line of chrome; that
is the mode we already have.

### Step 3 — click a session tab (the first step that needs mouse capture)

**After this step the user can:** click a tab in the session bar to select and focus that
session, instead of pressing Tab N times. **And loses** unmodified drag-select across the
whole terminal, recovering it only with Shift (Ghostty) or Option (iTerm2).

**Do not ship this until the text-recovery prerequisite is met** — see *The mouse-capture
trade* below. It is listed here because it is the correct *shape*, not because it is the
correct next thing to build.

**Files touched**

- `crates/glasshouse/src/tui/mod.rs:61` — `EnableMouseCapture` beside `EnableBracketedPaste`,
  same tolerant `tracing::debug!` on failure. Prefer writing `\x1b[?1000h\x1b[?1006h` by hand
  over crossterm's bundle: press/release plus SGR, no `?1002h`, no `?1003h`, no `?1015h`. That
  removes the wake-up per pointer move and shrinks the fragmentation surface, and it costs
  nothing, because crossterm's decoder is mode-agnostic (`parse.rs:168-169`, `:718`).
- `crates/glasshouse/src/shutdown.rs:60-68` — `DisableMouseCapture` in `restore_terminal()`,
  **not** in a `Drop`, because `force_exit` (`:179-183`) runs no destructor. Move
  `DisableBracketedPaste` there too while in the file; it leaks on the same path today.
- `crates/glasshouse/src/shell/view/chrome.rs` — lift the layout half of `render_session_bar`
  (`:77-131`) into a pure `pub(super) fn session_bar_layout(state, area) -> Vec<(usize, u16, u16)>`
  that the renderer consumes.
- `crates/glasshouse/src/shell/view.rs` — a `session_bar_hit(state, area, x, y) -> Option<usize>`
  beside `viewport_slot`, same file, same idiom, same doc-comment argument.
- `crates/glasshouse/src/shell/state/mod.rs` — `select_session(index)` beside `next_session`
  (`:656`) and `previous_session` (`:661`); `selected` is written in only two places today
  (`:678`, `:722`), and the new one must clear `status` the way `handle_key` does at `:770`.
- `crates/glasshouse/src/shell/mod.rs:527` — split the discard arm; on `Down(Left)` in control
  mode with no overlay, hit-test, `select_session`, then `sync_focus` (`:782`, the free
  function the run loop already calls at `:403`).

**Three corrections a naïve extraction would get wrong**

1. `render_session_bar`'s layout block also reads `state.theme()` (`chrome.rs:110,118,119`).
   It returns `Color` (`shell/appearance.rs:18`, `:38`) so it is geometry-neutral — but the
   read-set is not "sessions, selection and width" alone.
2. The extracted function must use **two width metrics**, because the render pass itself is
   inconsistent: the pan loop measures `s.chars().count() + 1` (`chrome.rs:100`) while ratatui
   positions the spans by `UnicodeWidthStr` (`ratatui-core-0.1.2/src/text/span.rs:271-273`),
   and `SessionRecord.harness` is a free-form `String` (`session/store/record.rs:414`). Use
   `chars().count()` to reproduce `start` and display width to reproduce the x offsets.
   Picking one for both silently mismatches the drawn frame.
3. It must reproduce the **empty-sessions branch** (`chrome.rs:78-87`, which draws a hint and
   no tabs), and it must know that the pan budget at `:97-105` never accounts for the 2-column
   `"‹ "` it later pushes at `:107-112`, so drawn extents already exceed `area.width` by 2
   whenever `start > 0`. That is faithfully reproducible — but "cannot drift" must not be read
   as "is correct".

**The single decision: the x→index mapping**, specifically the lead-indicator offset and the
clamp to `area.width`. The pan loop bounds only up to the selected tab (`chrome.rs:97-105`),
so tabs after it can run past the band and be clipped by the `Paragraph`. Mutation: remove the
clamp; a test at a width that forces the pan, asserting the hit test agrees with the drawn
`TestBackend` buffer (the existing idiom at `shell/tests/view_tests.rs:67-82`), must fail.

**Interaction with step 2:** in fullscreen there is no session bar to click. The hit test must
derive its band from the same mode-aware `regions()` the PTY size comes from, or clicks land
one to four rows off in exactly one mode — which is why step 2 lands first.

### Step 4 — wheel scroll into the hosted harness

**After this step the user can:** scroll the wheel over the viewport and have the *harness*
scroll — pane's scrollback and cell inspection (`crates/pane/src/session/ui.rs:611-630`),
Claude Code's own scrollback, whatever the focused harness does with a wheel notch. This is
the only way a wheel can ever do anything useful in Glasshouse's viewport, because the
emulator has zero scrollback of its own to scroll (`runtime.rs:865`, `:1432`).

**Files touched**

- `crates/glasshouse/src/shell/state/overview.rs:596` — `fn encode_mouse(event, cell) -> Option<Vec<u8>>`
  beside `encode`, returning `Some` for `ScrollUp` (SGR code 64) and `ScrollDown` (65) and
  `None` for every other kind, emitting `\x1b[<{code};{col+1};{row+1}M`. There is no SGR
  encoder anywhere in either crate today (`grep -rn '\x1b\[<' crates/*/src` is empty).
- `crates/glasshouse/src/shell/mod.rs:527` — the arm fires only when the mode is session, no
  overlay is open, the point is inside `view::viewport_slot(...)`, and the focused session's
  screen reports a mouse protocol.
- No change to `runtime.rs`: read through the existing `with_screen` (`:501`), deliver through
  the existing `write_input` (`:1030`).

**Read vt100's answer correctly.** It keeps a **single last-writer-wins slot**, not a faithful
DECSET flag set: `clear_mouse_mode` resets only when the current mode equals the one being
cleared (`screen.rs:687-691`), and `?1015`/`?1016` fall to the no-op `unhandled` callback
(`screen.rs:1170`, `callbacks.rs:55-63`). So consume it as two booleans — *does this child want
the mouse*, and *is the encoding SGR* — never as a mirror of the child's flags. For
crossterm-driven children like pane the answer is exact in both directions, because crossterm's
fixed enable/disable orders (`event.rs:321-334`, `:353-364`) are precisely the case a single
slot models correctly.

**The single decision: wheel bytes go through `write_input` (`runtime.rs:1030`), not
`write_to_focused` (`:1005`).** A wheel notch is not a person typing. Routing it through the
keyboard path would call `note_user_input` and start a 10-second window in which machine-sent
API messages to that session are refused (`runtime.rs:63`, `:1163-1195`) — so a user idly
scrolling would silently block an orchestrator — and would publish a `TextDelivered` lifecycle
row per notch (`runtime.rs:1019-1025` → `events/log.rs:472`). Mutation: flip it back to
`write_to_focused`; a test that sends a notch and then an API `send_text` must fail with the
precedence refusal.

**Test home:** `crates/glasshouse/tests/pane_launch.rs`, which already launches a real pane
inside Glasshouse over a real PTY and builds the pane binary itself — note it is `#![cfg(unix)]`
(line 31), so Windows is not exercised there. Plus a restore assertion in `pty_smoke.rs`
modelled on `:2250-2255`, checking `?1000l`/`?1006l` are emitted on exit.

### Step 5 — click, drag and motion into the harness: refused

Not built, and the plan recommends never building it as scoped. Four reasons, each with
evidence, and each independently sufficient:

1. **It requires a new delivery path.** Delivery is single-flight and *refuses* a concurrent
   write rather than queueing it (`runtime.rs:599-650`), every write marks user precedence
   (`:1005-1026`) and logs a durable row (`events/log.rs:472`). A motion stream needs a
   non-user, non-logged path through the one funnel Phase 10A protects — Red tier.
2. **It would break the hosted harness.** pane's fragmentation repair reconstructs wheel
   presses only (`terminal_input.rs:92` `strip_suffix('M')`, `:99-101` codes 64/65, deliberate
   per its own test at `:139-151`). A split click report becomes a stray Escape — closing
   pane's panel — plus literal `[<0;40;12M` typed into its composer. Fixing that is pane-side
   work that must land *before* any click forwarding, not after.
3. **There is nothing to click.** pane consumes no click, drag or release anywhere
   (`ui.rs:611-632`), never imports `MouseButton`, and never reads `mouse.column`/`mouse.row`
   in production. Forwarding clicks to pane today delivers them to a handler that drops them.
4. **It is a tmux reimplementation.** Swallow the pane's mode request into per-pane state, keep
   the outer capture as the union of what the panes want, decode each report to absolute x/y,
   resolve to a pane, arbitrate against the host's own bindings first, re-encode into that
   pane's dialect. *(Stated from knowledge of tmux's architecture, **not** verified — no tmux
   source exists on this machine, only the 3.7b binary. Treat the mechanism as a pointer to
   verify, not as evidence.)*

## What I would deliberately not build, and why

- **Mouse inside pane's own UI** — clickable panel rows, provider tabs, the scrollbar thumb.
  The band-level hoist is genuinely cheap and already proven in place: `viewport_height`
  (`ui.rs:442`, assigned at `:565` from `regions.transcript.height`, read inside the mouse arm
  at `:625`) already crosses from the draw block into the mouse handler by exactly the
  hoisted-`let` mechanism a hit test would use, and `ScreenRegions` is `Copy` (`tui.rs:218`) so
  it borrows nothing. But everything *below* the band level needs geometry that private render
  functions compute and discard — the panel's scroll window (`controls.rs:325-327`) over an
  `inner` shrunk by a 3-row carousel (`controls.rs:429`) *and* up to 3 variable search-header
  lines (`:313-322`); the provider card grid (`controls.rs:383-388`); the scrollbar thumb
  (`tui.rs:1283-1301`). Two copies of state-dependent arithmetic will drift. This is Debt, and
  it becomes one successor line, not a package.
- **Clicking a transcript cell in pane.** Structural blocker: `conversation_lines` returns a
  flat `Vec<Line>` (`tui.rs:1131-1137`) with no line→cell map — a repo-wide grep for
  `cell_index|line_to_cell|row_to_cell|cell_at|hit_test` under `crates/pane/src/` returns zero.
  A click resolves to a line index and stops. Cells open only via `/cell N` (`ui.rs:864-873`).
  That is new row provenance, a different kind of change, behind its own contract.
- **Clicking Glasshouse's knowledge and memory overlays.** Their cursor index is a running
  counter across five sections whose headings, blank separators and "…and N more" trailers
  consume screen lines without advancing it (`view.rs:745-776`). A click map means duplicating
  the line builder.
- **Clicking settings rows.** The row list's `Rect` depends on
  `wrapped_height(&bottom_lines, inner.width)` (`view.rs:1392`) — the one place in the shell
  where retention, not extraction, is the honest answer, and section row heights differ (four
  lines per provider at `view.rs:1638-1676`, one per harness/integration/profile). The tab
  strip is cheap (`view.rs:1424-1467`, six literal labels) and can join later; the rows should
  not.
- **DECSET 1007 alternate scroll for pane.** It would give the wheel as arrow keys with no
  capture and no selection loss — but `Up`/`Down` in pane's editor is prompt-history recall
  (`ui.rs:337-338`), so the wheel would walk history instead of the transcript. Wrong tool
  here.
- **Terminal capability detection.** Neither crate reads `TERM`, `TERM_PROGRAM`, `COLORTERM`,
  terminfo, or calls `supports_keyboard_enhancement` — verified by grep over `crates/`. Adding
  a detection subsystem is a new capability nobody has asked for, under the freeze rule.
- **An `[appearance]` table in `pane.toml`.** `shared-appearance.md:58-61` retracts pane.toml as
  a presentation home and `:134` makes it a non-goal; pane's config is read-only by construction
  (`config.rs:104`) so it could never persist a runtime toggle anyway. Adding it would cost five
  edits (`config.rs:177` clause, `:179` error string, a `parse_appearance`, a struct, a field)
  for nothing that asks for it.
- **A shared crate, a `glasshouse-theme`, or an abstract `Surface`/`HitTest` trait.** CLAUDE.md
  rule 8, and `Cargo.toml`'s `default-members = ["crates/glasshouse"]` exists precisely so V8
  and tokio never ride along on a Glasshouse build. The two fullscreen implementations are
  written twice on purpose. If they must be bound, bind them the way `shared-appearance.md`
  binds the palettes: a `#[cfg(test)]` test that `include_str!`s the other crate's file and
  fails on drift — one test buys the coupling.

## The mouse-capture trade, stated plainly

**What capture costs:** unmodified click-drag text selection across the entire terminal,
including the harness viewport, for the whole session. In Glasshouse that is unrecoverable
today — zero emulator scrollback (`runtime.rs:865`, `:1432`), no clipboard or copy mode
anywhere, and the raw scrollback reachable only through an MCP tool. This user's Ghostty is
configured `copy-on-select = clipboard`, so selection is a per-drag reflex, not an occasional
action; and their `cmux` embeds libghostty and reads the same config, so the regression would
be felt in every worker pane on the machine.

**What capture buys:** one clickable line of chrome (step 3) and wheel scroll into the harness
(step 4).

**That trade is not worth making yet.** The recommendation is:

1. **Ship steps 1 and 2 first.** They deliver the fullscreen render the user asked for and
   need no capture, no new terminal state, and no restoration risk.
2. **Before any capture ships, build the text-recovery path** — either give the `vt100` parser
   a non-zero scrollback and a copy/scroll mode, or surface `glasshouse_recent_output` as a real
   user affordance. Until then, selection is load-bearing and taking it away is a defect, not a
   trade.
3. **Gate capture behind one top-level `Option<bool>`, defaulting `false`**, copied field-for-field
   from `memory_extraction` (`loading.rs:248-249`, `:562-563`, `effective.rs:242-250`,
   `settings/keys.rs:524-529`, `shell/mod.rs:2441`/`:2488`) but with
   `Layered::new(false, Layer::Default)`. No migration, no schema bump, no database work — and
   read once at `Screen::acquire`, not toggled at runtime, because a mode you can trip by
   fat-fingering a key is worse than one you turn on deliberately.
4. **Write the narrow mode by hand** (`\x1b[?1000h\x1b[?1006h`) rather than calling crossterm's
   `EnableMouseCapture`. It halves the report population and stops hover from generating traffic
   — which matters because `crates/glasshouse/src/tui/event.rs:56` `QUIET_TICKS` exists
   specifically to keep an idle process out of `crossterm::event::poll`. It does **not** buy back
   selection; do not claim that it does.
5. **Emit XTSHIFTESCAPE `CSI > 0 s`** so Ghostty's shift-to-select default is explicitly
   preserved rather than left to a default the program is permitted to override
   (`ghostty.5:1235-1239`).
6. **Disable in `restore_terminal()`** (`shutdown.rs:60-68`), never only in a `Drop`.
7. **Note the Windows tier.** `EnableMouseCapture` there is a WinAPI console-mode change, not an
   escape sequence (`crossterm-0.29.0/src/event.rs:337-345` → `sys/windows.rs:39`), which puts
   the capture half in CLAUDE.md's `#[cfg(...)] platform code` **Red** tier and makes the
   `pane (windows-latest)` CI cell the only thing that verifies it — `pane_launch.rs` is
   `#![cfg(unix)]`.

**Separately, and worth a small package of its own regardless of everything above:** pane's
own capture is over-broad. It requests `?1000h ?1002h ?1003h ?1015h ?1006h` (`ui.rs:411-418`)
to read `ScrollUp`/`ScrollDown` only (`ui.rs:611-613`), a duplicate of PageUp/PageDown against
the same two fields (`ui.rs:667-679`, `:840-853`), and it has already paid for the excess with
a whole repair module (`terminal_input.rs`, a 20 ms escape grace and a 64-event lookahead, and
a live-PTY regression test at `tui_live.rs:504`). Narrowing to `?1000h`+`?1006h` keeps the
wheel, drops all motion traffic, and shrinks the surface `terminal_input.rs` exists to repair.
While there: re-enable the wheel when a panel or telemetry is open (`ui.rs:620` currently
disables it in exactly the two views where users reach for it), and add the one-line note pane
has never had — that the wheel costs unmodified drag-select, and the override is Shift in
Ghostty and Option in iTerm2. pane documents its mouse capture **nowhere**: the only
user-facing mention in either product is a footer hint at `crates/pane/src/tui/inspection.rs:306`,
shown only when `area.width >= 100`.

## How this fits the other plans

- **`shell-chrome.md`** — fullscreen is a third arm of the same mode-aware `regions()`, not a
  second layout mechanism. That document's decision stays the header's drop order; this one's
  is the PTY size. It also asks a question at `:85-87` — *"if the session bar becomes
  clickable, the header is the hit-test surface, so the header's Rect is the one that must be
  retained after drawing."* **The answer is that nothing is retained.** `ShellState` has no
  geometry field at all (`state/mod.rs:477-531`) and `render` takes `&ShellState`, so it
  could not store one; the Rect is *recomputed* by an extracted pure function, exactly as
  `viewport_slot()` already recomputes `regions(area)[3]` outside the render pass for two
  production callers. `shell-chrome.md:77`'s "no mouse in this package" stays correct — this
  document is the successor it points at.
- **`shared-appearance.md`** — the collapsed header wears the focused session's theme and each
  tab renders in its own accent, so the bar doubles as the legend. Fullscreen removes that
  legend, which is a reason to keep the one-line header as the default and fullscreen as the
  deliberate extra rather than the new normal. pane's `/fullscreen` and `/theme` stay
  orthogonal: fullscreen touches `ScreenRegions`, theme touches colour, and neither reads the
  other. If a `--fullscreen` startup flag is added to `SessionArgs` (`session.rs:401-403`), it
  is the same one-flag shape as `--theme` in that plan's step 1, and it should ride in the same
  package rather than co-editing the file twice.
- **Rule 8, no new coupling debt.** Nothing here introduces a shared crate, a trait, or a new
  module boundary. Every new function stays in the module that owns the thing it computes:
  `session_bar_layout` in `chrome.rs` next to the renderer that draws it, `session_bar_hit` in
  `view.rs` next to `viewport_slot`, `encode_mouse` in `overview.rs` next to `encode`,
  `select_session` in `state/mod.rs` next to `next_session`, and pane's fullscreen arithmetic
  inside `screen_regions` where the rest of the layout already lives.

## Open questions for the user

1. **Is the Glasshouse fullscreen worth it for one row?** Its value is a frame-free render, not
   space. If the collapsed header is enough, step 2 can be dropped and steps 3–4 re-based on
   `shell-chrome.md`'s two-band layout with no other change.
2. **Is losing unmodified drag-select acceptable, and when?** Steps 3 and 4 are the only ones
   that need it. The recommendation is not before a text-recovery path exists.
3. **Should `glasshouse attach` be priced first?** `crates/glasshouse/src/session/attach.rs:1-19`
   is already a byte-transparent bridge — raw mode only, no emulator, `stdout.write_all` on the
   output pump (`:223-246`) — where pane's mouse works today with zero new code. If the real
   need is "I want to scroll pane's history with the wheel", *attach to that session* already
   answers it. The nested case earns its cost only if the wheel must work while Glasshouse's
   chrome and tab strip stay on screen — which fullscreen, by definition, removes.