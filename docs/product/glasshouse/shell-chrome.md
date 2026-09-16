# Shell chrome — what Glasshouse shows while a session is focused

Status: **plan only.** Nothing implemented. Written 2026-09-08 from a reading of
`crates/glasshouse/src/shell/view.rs`, `.../view/chrome.rs` and `.../shell/mod.rs`.

## What is true today

Glasshouse's TUI is already full-screen — alternate screen and raw mode, set in
`crates/glasshouse/src/shutdown.rs:256-264`. There is no nested "screen in a
screen": the embedded harness gets the **full width** and no border.

`view.rs:38 fn regions()` splits the terminal into five vertical bands:

| band | height | content |
| --- | --- | --- |
| title | 1 | `GLASSHOUSE · pane a3f9c2` |
| root | 1 | `root /Users/eneas/projects/glasshouse` |
| session bar | 1 | the tab strip |
| viewport | `Min(0)` | the harness |
| footer | 1 | in session mode: `SESSION MODE  ctrl-] for glasshouse  keys go to the session` |

`view.rs:58 fn viewport_slot()` returns band 4 and is the **single source of
truth** for the size handed to a session's pseudo-terminal and its `vt100`
emulator — `shell/mod.rs:804 fn viewport_terminal_size` is its only consumer of
consequence. Anything that changes the bands changes what the harness is told,
automatically and in one place.

The session bar already pans horizontally, keeping the selected tab visible and
drawing a `‹` when it has scrolled (`view/chrome.rs:75 fn render_session_bar`).
It does not wrap. That behaviour is already what a header wants.

## The plan

**When a session is focused, collapse the chrome from four lines to one.**

`regions()` becomes mode-aware:

- **Control mode** — today's five bands, unchanged. When you are looking at the
  fleet rather than at one job, the full view *is* the product.
- **Session mode** — `[header 1, viewport Min(0)]`. Title, root and footer are
  gone; what still matters from them moves into the header.

The header is one line laid out as a ribbon with an explicit **drop order**,
never a wrap:

```
‹ 1 pane · running  2 claude · waiting ›   …/glasshouse   sonnet-4-6   ctrl-] back
```

Widest to narrowest, fields drop in this order: wordmark, then root, then
model. **Session tabs and `ctrl-] back` never drop** — one is the navigation and
the other is the only way out. This is the same treatment the tab strip already
applies to itself, extended to the rest of the line rather than reinvented.

### Why this is small

`viewport_slot()` already funnels the harness's size through one function, so
the harness picks up the recovered lines with no resize logic to touch. The work
is `regions()`, one `render_header()`, and the mode check. Three files.

### The prize, stated honestly

**Three lines.** On a 40-row terminal the harness goes 36 → 39 rows (+8%); on a
24-row split, 20 → 23 (+15%). There is no horizontal win available, because the
harness already has the full width — the "screen in screen" this plan was
originally proposed to fix does not exist.

### The decision this makes

The **drop order**. That is the thing a mutation should attack: reorder it and a
narrow terminal must lose the wordmark before it loses the way out.

### Non-goals

- **Control mode is not touched.** Collapsing it would lose information for no
  gain.
- **No help overlay, slash palette or mouse in this package.** Those decorate
  chrome; fix the chrome first, then ask whether the header wants a `?`.

## Interaction with the other plans

- `shared-appearance.md` — the header wears the **focused session's** theme, and
  each tab renders in its own accent, so the collapsed bar doubles as the legend
  for which colour is which job.
- `fullscreen-mode.md` — if the session bar becomes clickable, the header is the
  hit-test surface, so the header's Rect is the one that must be retained after
  drawing.
