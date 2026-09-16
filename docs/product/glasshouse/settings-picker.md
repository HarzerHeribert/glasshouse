# Settings as a picker — one interaction instead of six sections

Status: **plan only.** Nothing implemented. Written 2026-09-08 from a reading of
`crates/glasshouse/src/shell/state/settings/{mod.rs,keys.rs}`,
`crates/glasshouse/src/shell/view.rs:1337 render_settings`, and
`crates/pane/src/tui/controls.rs`.

## What is true today, measured

Glasshouse's settings are one centred popup (90% × 80%) holding six sections —
Harnesses, Integrations, Providers, LaunchProfiles, Routing, Memory
(`settings/mod.rs:308-315`). Each has its own row renderer, dispatched at
`view.rs:1408-1415`.

The complexity is not a feeling; it counts:

| | |
| --- | --- |
| sections | 6, each with its own renderer and its own keys |
| key bindings in the overlay | **34, across 20 distinct letters** (`settings/keys.rs`) |
| what the footer says about them | `section keys edit` — a non-explanation |
| mutually exclusive bottom panels | **8**, chained in one if/else (`view.rs:1362-1379`) |
| lines | 2,759 across `mod.rs` + `keys.rs` |

The eight bottom states are: path input, credential-delete confirm, project-write
confirm, provider input, profile input, routing input, provider test result,
provider models result. Exactly one may be visible, and which one is decided by
an if/else chain whose ordering carries a real invariant — an active input must
outrank the reachability banner, and the code says so at `view.rs:1352-1358`.

It is a tabbed configuration dialog, which is what every settings screen decays
into.

## pane already has the better shape

`crates/pane/src/tui/controls.rs:37-64` is one generic `Panel { title, rows,
selected, search }` with a `PanelSearch { query, source, matched, providers,
active, choices }`. Its `ModelGroup` carries `selectable` and — the part worth
stealing — **`unavailable_reason`**, so a choice you cannot pick is still *shown,
with the reason*, rather than vanishing.

One interaction serves all of it: type to filter, arrow to move, Enter to take.

## The plan

**Every setting becomes a row in one flat, searchable list.**

A row is a path, a kind, a value and an optional reason:

```
providers/anthropic/key      Secret   ●●●●●●●●   set, 2026-09-07
providers/anthropic/test     Action   →
harnesses/codex/enabled      Toggle   off        not installed
routing/buffer               Text     0.15
appearance/standard          Choice   neon
```

Kinds: `Toggle · Text · Secret · Path · Choice · Action`.

### What this buys, in the order it matters

1. **Eight bottom panels become one editor.** The row's *kind* selects it, so
   the if/else chain at `view.rs:1362-1379` collapses into a table lookup. The
   ordering invariant it currently encodes by hand — an active input outranks
   the banner — becomes structural: an editor is open or it is not.
2. **Twenty letters become one key.** Enter edits whatever is selected.
   `section keys edit` stops needing to be documented because there are no
   section keys.
3. **Hidden actions become visible rows.** "Test this provider", "refresh
   models", "run setup" are letters today. As `Action` rows they are things you
   can *see*, which is the difference between a feature and a secret.
4. **Search becomes the navigation.** Type `anth`, get anthropic's rows. The six
   sections survive as a **filter strip for orientation, not as modes** — you
   can still tab through them, but they are one facet of one list rather than
   six separate screens.
5. **Unavailable stays visible.** Carry pane's `unavailable_reason`. A harness
   that is not installed greys out *with the reason* instead of disappearing and
   leaving the user to wonder.

### The decision this package makes

**Which kind maps to which editor** — and specifically that **`Secret` is its
own kind, never `Text` with a flag.**

That is not tidiness. The current code is careful in a way a uniform editor
could easily lose: masking happens inside `SettingsState::provider_input`, and
`view.rs:1367-1369` says *"`input.buffer` is already masked for a credential
field … This renderer never sees a typed credential."* The renderer being
structurally unable to see a secret is the property. A mutation should attack
exactly that: route `Secret` through the `Text` editor and a test that types
into a credential row and asserts the rendered buffer contains no plaintext must
fail.

### Sizing, honestly

This is a refactor of 2,759 lines and it should come out **smaller**: six key
tables and an eight-way dispatch collapse into one row table. Under the Phase 59
decompression ruling that makes it *simplification*, which is explicitly in
scope, rather than a new feature. Amber tier.

### Non-goals

- **Not a redesign of what the settings are.** Same six areas, same values, same
  persistence. Only the way in changes.
- **No mouse.** Rows become clickable only if `fullscreen-mode.md`'s step 3
  lands; the keyboard path must be complete on its own.
- **No new settings.** Adding rows is a different package.

## Mockup

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

Filtered, with an editor open on a `Choice` row:

```
┌─ settings ──────────────────────────────────────────────────────────────────┐
│ search  theme▌                                             2 of 47 settings │
│                                                                             │
│ ▸ appearance/standard         neon      ◀ neon amber ice mono violet ▶      │
│     what Glasshouse wears in control mode, and any session with no theme    │
│   appearance/assign           cycle     ◀ cycle · fixed · off ▶             │
│     how a new session gets its colour                                       │
│                                                                             │
│ enter apply   ←/→ try   esc close                                           │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Order of work

It shares `crates/glasshouse/src/shell/state/` with the fullscreen package, so
the two cannot run concurrently. This one goes **after** `fullscreen-mode.md`
steps 1–2 integrate, and **before** `shared-appearance.md`, because
`appearance/standard` and `appearance/assign` are the first rows the picker
would carry and they are cheaper to add to a picker than to the tabbed dialog.
