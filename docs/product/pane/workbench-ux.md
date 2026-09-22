# The workbench as an application — roadmap and the principles under it

**Status: the plan of record for Pane's interface, 2026-09-21.** It supersedes
nothing; `pane-workbench.html` stays the drawing, and this is the argument for
what the drawing is *for*. Every stage below names the principle it serves and
the observation that made it necessary.

## The complaint, and what is actually wrong

> "This is not your usual TUI — this is working like a clickable UI application
> UI. So what does make an application actually enjoyable to use and explore →
> it's already laid out what the user most likely wants. And the settings is
> alright but it's not really explained how this would affect the user's work.
> Most settings which are always used need to be always visible and immediately
> accessible and not behind some cryptic sandbox mode keys which the user does
> not understand. … My statusline is great, while being a bit densely packed
> with stuff I don't care about. … I would start with something like instant
> updates."

Four defects, and they are not taste:

1. **Nothing a setting does happens now.** 66 of the 71 keys in
   `settings/registry.rs` carry `restart: true`, and `workbench/settings.rs::save`
   applies exactly four (`ui.theme`, `ui.reduced_motion`, `ui.statusline`,
   `ui.sidebar`) to the running session. Everything else prints *"Saved for a new
   session."* `permissions.mode` is declared `restart: false` and is still not
   applied — that one is a plain bug.
2. **The screen names mechanisms.** `sandbox 3p/1c YOLO unconfined · net:off` is
   built at `session.rs:740` from a path-rule count, a command-pattern count and
   a boolean; nothing on screen says so. `Ask:`, `Access:`, `session.effort ·
   Saved: inherited · Effective: default (built-in)` are the same failure.
3. **The everyday settings are not on the everyday surface.** The settings
   surface's first category shows three of seventy-one keys, one description at
   a time, at the foot, for the selected row only.
4. **The status line is a fact dump where the drawing specifies a control
   strip.** The mockup's own `renderStatus()` renders buttons, a live notice and
   an Undo. What was built renders read-only text, half of it counters.

## The principles, and where they come from

Ten, from the research pass; the full citations are in the evidence entry.

| # | Principle | Source |
|---|---|---|
| P1 | A state change the **user** caused renders inside **100 ms**, with no spinner. That is the whole of "instant updates". | Miller 1968; Card/Robertson/Mackinlay CHI'91; Nielsen, *Response Times: The 3 Important Limits* |
| P2 | Make that architecturally true. ratatui is immediate-mode and rebuilds from state every frame; a mutation that waits on the agent's work is a frozen UI. | ratatui.rs/concepts/rendering |
| P3 | Label by **consequence**, not by mechanism. A label with no information scent costs a click and a disappointment. | Pirolli & Card, information foraging; NN/g, *Information Scent* |
| P4 | Keep the consequence text **persistently visible beside the control** — not on hover, not behind a key, not in docs. | NN/g, *Placeholders in Form Fields Are Harmful* |
| P5 | Split settings by **frequency of use**, and put the frequent few on the primary surface. | NN/g, *Progressive Disclosure* |
| P6 | Prefer fixing the default to adding an option; a preference is a decision handed to the user. | Spolsky, *Choices*; GNOME HIG |
| P7 | A mode is **continuously visible in the locus of attention**, or it is a quasimode. Invisible modes cause mode errors by construction. | Raskin, *The Humane Interface*; Norman, *DOET* |
| P8 | **Reversibility over confirmation.** Undo relieves anxiety; a dialog only delays. | Shneiderman, rules 5 and 6 |
| P9 | Direct manipulation has three requirements, and the third is the one apps fail: rapid, incremental, **reversible** operations whose impact is **immediately visible**. A clickable TUI that fails it is a menu, not an application. | Shneiderman, IEEE Computer 16(8), 1983 |
| P10 | Teach **at the moment of use**, never with a tour. | NN/g, *Instructional Overlays and Coach Marks* |

Two the evidence tells us **not** to do: no skeleton loaders for perceived speed
(the one controlled comparison, Viget n=136, rated them slowest of three), and no
animated spinner as the *default* long-work indicator (static changing text is as
informative and does not break speech synthesis). Pane's braille art stays what it
already is — decoration that reduced motion freezes.

## What the two neighbours do, read from the shipped binaries

- **Codex** collapses approval policy × sandbox mode into **three named profiles**
  with one sentence of consequence each — *Read Only*, *Default*, *Full Access* —
  and cycles them on Shift-Tab.
- **Codex prints the affordance beside the fact**: `model: gpt-5   /model to change`.
- **Both** offer `No, and tell <agent> what to do differently` as a first-class
  denial, and Codex offers scope-graded grants at the moment of the prompt
  ("don't ask again for commands that start with …", "allow this host for this
  conversation").
- **Both** shout the dangerous mode rather than spelling it: `BYPASS PERMISSIONS`,
  `permissions: YOLO mode`.
- **Both** collapse expensive output with the hidden quantity printed next to the
  expand key.
- **Claude Code** derives its opening examples from the user's own repository, and
  keeps exactly one permanent advertisement — ` ? for shortcuts` — which erases
  itself the moment the input is not empty.

## The stages

### 1 — Instant updates (P1, P2, P9)

A saved setting that has a live equivalent applies **now**, and the live
equivalent already exists: `session/controls.rs::command` mutates the running
session for `effort`, `mode`, `permissions`, `model`, `subagents` and more. The
settings surface simply never called it.

- `Preferences::save` returns the control command a key maps to, and the reducer
  emits it as an `Effect`.
- A key with no live equivalent says **`next session`** on its own row, and the
  notice for it names what the current session keeps.
- Fix `permissions.mode`, which claims `restart: false` and is not applied.

### 2 — Name the thing by what it does to your work (P3, P4)

- `sandbox 3p/1c YOLO unconfined · net:off` becomes one **named access profile**
  derived from the real posture, with one sentence of consequence. The counts,
  the confinement applier and the network posture stay — under the sentence, on
  the Access surface, where someone who wants the mechanism will look.
- The dangerous profile is shouted, not spelled.
- `Work:` / `Ask:` / `Access:` prefixes go; the value alone carries the scent and
  buys back the columns a narrow terminal needs.
- The settings foot names the **winning layer in words** rather than printing a
  dotted key and a three-valued provenance.

### 3 — The always-visible strip (P5, P7, and the content-to-chrome ratio)

The status line stops being a fact dump and becomes what the drawing specifies: a
**control strip**. It carries a live notice, an **Undo** whenever something is
undoable (P8), and the handful of toggles that are genuinely always-used. The
counters that are nobody's decision move to the sidebar and the telemetry
surface. A row earns its line only by carrying state the user would otherwise
have to recall, or a mode they could otherwise forget.

### 4 — Cycle the everyday four; do not open a surface for them (P9)

Shift-Tab opens a surface and then wants three cursor moves and an Enter. Both
neighbours cycle in place. Shift-Tab cycles the rung and says the new name and
its one sentence; the surface stays reachable by clicking the same control, so
the visible path and the fast path are one path (C7 — layered interface).

### 5 — The settings surface as an application (P4, P5, P6)

- The first category becomes the **genuinely everyday** set, chosen by frequency
  of use, not by table name.
- **Every visible row carries its consequence**, not only the selected one.
- The descriptions are rewritten in consequence language.
- `next session` is a mark on the row, not a sentence attached to all of them.

### 6 — An opening that is already laid out (P10)

The card stays. What it gains: the affordance printed beside the fact, and one
rotating just-in-time hint on the composer's own line — never a tour.

## What this roadmap deliberately does not do

- It adds **no new preference**. P6 says a preference is a decision handed to the
  user; this work removes screens-full of decision, and the only keys it adds are
  ones that replace two cryptic ones with one plain one.
- It does not touch the cell, the diff, the navigator or the telemetry
  instruments beyond relabelling. Those were measured against the mockup in the
  previous pass and are not what the complaint is about.
- It does not add a tour, a skeleton loader, or a second spinner.

## What shipped, against the stages above

All six, in `3af277ff..bb4290ad`. Two deliberate departures from the plan as
written:

- **The rotating hint sits on the status line's second row, not on the
  composer's own line.** The composer's line already carries the activity
  ribbon and a standing notice; a third thing there would have been the
  crowding this pass is trying to undo.
- **The status strip has no notice and no Undo**, though the drawing's
  `renderStatus` has both. A notice already rides the ribbon one row up, where
  the eye is, and Undo belongs to the surface that can undo something. The
  strip is the drawing's other half: buttons rather than readings.

Two things found while doing it that were not in the plan and were fixed
anyway, because both were the same defect the plan is about — a control that
does not say what it is:

- **A picker did not mark the option the session was on.** Work and Ask
  highlighted the row under the cursor and nothing else.
- **Shift-Tab was dead.** The workbench consumed `BackTab` and opened a
  surface, so the live cycler in `session/ui.rs` never ran. Making it reachable
  raised a question the plan had not: it would have walked into the rung that
  never asks again in one unconfirmed keystroke from the default. It steps over
  that rung now.

Two things deliberately **not** done, and why:

- **Denial with redirection** (`No, and tell Pane what to do differently`),
  which both neighbours ship as a first-class approval option, along with
  Codex's scope-graded grants at the moment of the prompt. It is the best
  remaining idea from the research and it needs a change to the approval
  protocol — the denial has to carry text back as the tool result — which is
  not a presentation change and does not belong in a presentation pass.
- **Deleting the legacy renderer.** `tui.rs` and six submodules, roughly 2,300
  lines, are dead in the interactive path and alive only for non-TTY output and
  `#[cfg(test)]`. Real debt, no user-visible benefit, and its own piece of work.

  **Measured on 2026-09-22, because "dead" was about to be read as
  "deletable".** It is neither. `session/startup.rs::render_as_lines` is
  production: it is what `pane` prints when stdout is a pipe, and it draws
  through `tui::render` into an in-memory backend, so the conversation column
  and the sidebar reach a redirected run. Both `session/ui.rs` call sites of
  `render_screen_with_geometry` are inside `#[cfg(test)]` helpers, and of the
  eleven entry points only three — `render_screen`, `conversation_rows`,
  `anchor_scrollback` — have no caller outside `tests/`. So deleting this is
  not a removal, it is **replacing pane's non-TTY output**, and it owes a
  decision about what a piped run should print before it owes a line of code.
  Every acceptance test takes that same path, which is what makes the debt
  cheap to leave and expensive to pay.

## The application pass — 2026-09-22

The user, on the result of the six stages above: *"Pane's frontend is bad — I
need it upgraded asap, clear sections and clear distinctions. Think of it as a
SaaS app become chatbot in a TUI with a fun interactive personality."* And on
the proposal: *"I love it, please make sure to build it. Can we do something
bird themed as well, I love birds."* The proposal page, with the real screens
before and after at 140×40, is the artifact linked from the checkpoint of that
day; what it diagnosed was not taste but texture: six kinds of content in one
foreground, eight regions with no enclosure, four controls that looked like
words, a composer indistinguishable from the transcript, and a manual's voice.

Five moves, all shipped in one pass, all inside `workbench/`:

1. **Frames.** `chrome.rs` draws the shapes: a rule with joints, a frame, a
   chip. The header rule meets the session card's gutter at `┬`, the dock's top
   edge meets it at `┴`; the dock is `╭ … ╯` with the prompt mark inside.
2. **A grammar.** `Row::kind` (`document.rs`) says what a row is -- your turn,
   Pane's turn, card top, card body, card bottom, helper, note, answer -- and
   the view draws the shape. Copy and selection read the words alone.
3. **The chip.** One control shape everywhere: the bar, the dock, a cell's tabs,
   the answer's quick actions, the pickers and the settings rows. The current
   choice is filled in the accent with dark ink; mono reverses. That filled chip
   is the one thing the workbench paints, and `tests/workbench.rs` bounds it.
4. **The bird and the voice.** `voice.rs` holds the six braille states (idle
   blinks, thinking is a pose, working pecks inside a running cell, done, asking,
   oops) and every line Pane says, in two voices side by side. `ui.voice` is the
   pass's one new preference, and it changes words, never structure. Clicking
   the bird gets a remark; the plain voice gets a pointer instead.
5. **Feedback.** A pressed chip fills for the frames the finger is down; a notice
   rides the dock's edge for four seconds with `⟨ undo … ⟩` beside it when a dock
   chip made the change; `?` opens the key sheet; the opening offers the
   project's own suggestions as chips that type the message.

Held from the principles: no paint under a region (borders only, transparency
untouched), reduced motion freezes every decoration, every click keeps its
keyboard route, no tour. The three-cell motion budget on an idle screen is kept
by design: the dock's flap is three cells and the bird's blink is two. Still
deliberately not done: denial with redirection (needs the approval reply to
carry text), the side-by-side diff, and deleting the legacy renderer.
