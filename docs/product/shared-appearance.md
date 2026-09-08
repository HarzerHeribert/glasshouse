# Shared appearance — one design language across pane and Glasshouse

Status: **plan only.** Nothing here is implemented. Written 2026-09-08 from a
reading of `crates/pane/src/tui.rs`, `crates/pane/src/tui/controls.rs`,
`crates/glasshouse/src/shell/appearance.rs` and
`crates/glasshouse/src/shell/state/settings/`.

## What is true today

**Both products already have the same eight themes, by name.**

| | pane | Glasshouse |
| --- | --- | --- |
| type | `enum Theme` with `parse(&str)` | `struct Theme(u8)` — an index |
| names | neon amber ice **mono** violet cobalt mint rose | neon amber ice violet cobalt mint rose **mono** |
| chosen by | `/theme <name>` slash command | `t`, which cycles |
| roles | `accent` · `dock` · `backlight` | `accent` · `secondary` · `quiet` |
| persisted | **no** | **no** |

Three findings follow, and each one changes the plan.

**1. Neither product remembers a theme.** Both start on `neon` every launch.
The question "can pane's theme carry into Glasshouse" cannot be answered as
posed, because there is nothing to carry — the first work is to persist one at
all, and that is most of the value on its own.

**2. The orders differ, so the index is not the identity.** `mono` is index 7
in Glasshouse and index 3 in pane. Any exchange must pass the **name**. Passing
`Theme(u8)` between them would silently map `mono` to `violet`. This is the one
mistake that would look like it worked.

**3. The palettes disagree, and pane's is the better one.**

| name | pane accent | Glasshouse accent | |
| --- | --- | --- | --- |
| neon | `rgb(223,255,0)` | `rgb(176,255,94)` | far apart |
| amber | `LightYellow` | `rgb(255,197,91)` | pane follows the terminal |
| ice | `LightCyan` | `rgb(98,223,250)` | pane follows the terminal |
| mono | `White` | `rgb(225,230,232)` | pane follows the terminal |
| violet | `rgb(191,154,255)` | `rgb(190,148,255)` | ~equal |
| cobalt | `rgb(114,155,255)` | `rgb(108,161,255)` | ~equal |
| mint | `rgb(100,231,187)` | `rgb(102,239,187)` | ~equal |
| rose | `rgb(242,156,218)` | `rgb(255,147,183)` | apart |

pane uses **ANSI named colours** for amber, ice and mono, so those three
inherit whatever palette the user has configured in their terminal;
Glasshouse pins RGB and overrides it. pane's header says this is deliberate —
*"Accent-only themes inherit the terminal background and its transparency."*
Converge on pane's table.

## The plan

**The theme belongs to a session, not to a project or a person.** That is the
correction that makes it useful: the point is not "I like mint", it is *"the
mint one is the refactor, the rose one is the benchmark"* — an identity you can
read while scrolling the session bar, the way Claude Code's `/color` works.

An earlier draft of this document put the theme in
`<root>/.glasshouse/pane.toml`. **That was wrong** and is retracted: that file
is project configuration, and every session in a project would share one
colour, which is exactly the case the feature exists to distinguish.

### Three layers

| layer | lives in | worn when |
| --- | --- | --- |
| Glasshouse standard | Glasshouse settings, saved with `w` | control mode, and any session with no theme |
| session theme | `SessionRecord.theme` | while that session is focused |
| `/theme` in pane | the running pane's own state | immediately, in that pane |

Glasshouse starts in its standard and is **overwritten by the focused
session's theme** the moment one is focused — leaving a session returns it to
the standard. The standard is what you see when you are looking at the fleet;
the session colour is what you see when you are looking at one job.

### Step 1 — assign at launch. This delivers the whole goal.

Glasshouse already owns the launch and already owns the session record. It can
therefore do all of this without pane ever talking back:

1. `SessionRecord` gains `theme: Option<String>`. It already carries
   `presentation`, `presentation_ref` and `native_session_id`, so this is a
   column, not a concept.
2. Glasshouse **assigns** a theme to each new session and passes it as
   `pane session --theme <name>`. pane has no `--theme` flag today — its args
   are root, task, model, context-window, rollout, session, glasshouse, yolo —
   so this is the one addition on pane's side.
3. The shell wears the focused session's theme; the tab strip renders each tab
   in its own accent, so the bar itself becomes the legend.

Nothing new is invented: one column, one flag, one lookup.

**The assignment policy is the decision this step makes**, and the one a
mutation should attack. Cycling from the standard at creation guarantees that
*adjacent* sessions differ, which is the property you actually want while
scrolling. Hashing the session id gives stability across restarts but can hand
two live sessions the same colour. Recommendation: **cycle at creation, then
persist the result** — adjacent sessions differ, and the colour is stable
afterwards because it is stored rather than recomputed.

### Step 2 — `/theme` writes back. This needs one thing that does not exist.

For a `/theme mint` typed inside a running pane to reach Glasshouse, pane must
know **which** session it is. Today it cannot:

| link | status |
| --- | --- |
| producer — Glasshouse tells the child its session id | **missing.** Glasshouse sets no `GLASSHOUSE_*` variable when spawning a harness |
| carrier — the child's environment | exists |
| consumer — pane calls back | the *mechanism* exists (pane already runs `Command::new(glasshouse)` for `routing-cost --json`, `mcp serve` and memory); the subcommand does not |

So step 2 is honestly two new things — `GLASSHOUSE_SESSION_ID` at spawn, and a
`glasshouse session theme <name>` subcommand — over a shell-out pattern that is
already established. pane reads `GLASSHOUSE_DATA_DIR` and
`GLASSHOUSE_STATE_DIR` today, so the variable is not a new idea, only a new
value.

The shell needs no polling for it: it already re-reads the session store on
its redraw tick (`state.refresh(sessions.store().list()?)`), so a written theme
is picked up by machinery that runs anyway.

### Step 3 — one palette, duplicated on purpose, with a test that binds them

Unchanged from the reading above: copy pane's eight-row table into
`appearance.rs` and add a `#[cfg(test)]` test that `include_str!`s pane's
`tui.rs` and fails when the two drift. Glasshouse must not depend on the `pane`
crate — `Cargo.toml` keeps pane out of `default-members` precisely so V8 and
tokio never ride along — and CLAUDE.md rule 8 forbids inventing a shared crate
for tidiness. One test buys the coupling instead.

### Non-goals

- No shared crate, no trait, no `glasshouse-theme`. Rule 8.
- No per-user or per-project theme file. The session record is the home.
- No new colour roles. The union is `accent · dock · quiet`.

## Mockup

**The session bar is the legend.** Each tab wears its own accent, so scrolling
is how you tell them apart — the header adopts whichever is focused.

```
 GLASSHOUSE · pane a3f9c2          …/glasshouse   sonnet-4-6   ctrl-] back
 ‹ 1 pane · running   2 pane · waiting   3 claude · running ›
   └ mint ────────────┘└ rose ──────────┘└ cobalt ─────────┘
   the header, the border and the accents above follow the focused tab
```

**Settings, themes selected** — the worked example for the picker:

```
┌─ settings ──────────────────────────────────────────────────────────────────┐
│ search  theme▌                                             2 of 47 settings │
│                                                                             │
│ ▸ appearance/standard         neon      ◀ neon amber ice mono violet ▶      │
│     what Glasshouse wears in control mode, and any session with no theme    │
│   appearance/assign           cycle     ◀ cycle · fixed · off ▶             │
│     how a new session gets its colour                                       │
│                                                                             │
│   PREVIEW                                                                   │
│   ┌───────────────────────────────────────────────────────────────────────┐ │
│   │ GLASSHOUSE · pane a3f9c2      …/glasshouse   sonnet-4-6   ctrl-] back │ │
│   │ ‹ 1 pane · running   2 pane · waiting ›                               │ │
│   └───────────────────────────────────────────────────────────────────────┘ │
│                                                                             │
│ enter apply   ←/→ try   esc close                                           │
└─────────────────────────────────────────────────────────────────────────────┘
```

**The full list**, showing sections demoted from modes to filters:

```
┌─ settings ──────────────────────────────────────────────────────────────────┐
│ search  ▌                                                 47 settings       │
│ all · appearance · harnesses · integrations · providers · profiles · routing│
│                                                                             │
│ ▸ providers/anthropic/key          ●●●●●●●●         set, 2026-09-07         │
│   providers/anthropic/test         →                action                  │
│   providers/openai/key             —                not set                 │
│   harnesses/pane/enabled           on                                       │
│   harnesses/codex/enabled          off              not installed           │
│   routing/buffer                   0.15                                     │
│   appearance/standard              neon                                     │
│                                                                             │
│ enter edit   / search   tab filter   w save   esc close                     │
└─────────────────────────────────────────────────────────────────────────────┘
```

`codex/enabled — not installed` is pane's `unavailable_reason` carried across:
an unavailable row stays visible with its reason instead of vanishing.

## Open decisions

1. **Does a session keep its colour across a resume?** The record persists, so
   yes by default. Confirm that is wanted before relying on it.
2. **What does pane do standalone?** Run without `--theme` and without
   Glasshouse, it keeps `/theme` as a session-local choice that persists
   nowhere — which is what it does today. Do not add a directory to hold a
   colour.
3. **Does `t` survive in Glasshouse?** It currently cycles the shell's theme.
   Under this model it should set the *standard*, not the focused session's
   colour, or the two concepts collide on one key.
4. **`secondary` vs `dock`.** Glasshouse uses `secondary` in exactly two
   places. Confirm on reading whether `dock` covers both before adding a role
   to the shared vocabulary.
