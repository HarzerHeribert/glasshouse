# Glasshouse capability evidence ledger — index

This ledger supports—but never replaces—the authoritative [`docs/product/capability-map.md`](../capability-map.md). It maps requirements to observable product contracts, production paths, and non-vacuous regression evidence.

Populate entries incrementally as a capability becomes active or as previously checked work is reconciled. Do not spend a whole implementation cycle filling hundreds of future entries speculatively.

The ledger used to be one file, `GLASSHOUSE_CAPABILITY_EVIDENCE.md`, at 5,851 lines — the single largest tax on every worker in this project, since `CLAUDE.md` told each of them to read it completely. It is now split by phase below; read only the phase file(s) your packet names.

## Entry template

```markdown
### <phase and stable short name> — <exact capability text>

Contract: Given <context>, when <trigger>, Glasshouse <observable behavior>,
while preserving <invariant or failure behavior>.

State: NOT STARTED | SCAFFOLDED | PARTIALLY VERIFIED | LOCALLY VERIFIED |
CI VERIFIED | COMPLETE

Production evidence:
- `<file>: <symbol/path>` — why this is a real reachable production path

Regression evidence:
- `<test name>` — behavior proved and platforms actually executed

Failure/isolation evidence:
- `<test or probe>` — negative, fail-closed, cleanup, or boundary behavior

Platform/external evidence:
- `<CI run or runtime probe>` — commit and platforms covered

Missing evidence:
- exact remaining proof or implementation
```

## Evidence rules

- Quote the capability exactly enough to find it in the map.
- Keep the contract to one product sentence.
- Cite symbols and test names, not merely directories.
- State which platform actually executed a test.
- A test-only type or fake caller is not production evidence.
- A checked box requires **COMPLETE**.
- If later evidence contradicts an entry, downgrade it immediately and reopen the map checkbox if necessary.

## Phase files

38 phase files, 87 entries total.

| File | Entries | Headings |
|---|---|---|
| [`phase-1.md`](phase-1.md) | 3 | Phase 1 — Display the active canonical project root prominently in the TUI<br>Phase 1 — Reject any attempt to resume a Glasshouse-managed session whose project identifier differs from the current project identifier<br>Phase 1 — Ensure every spawned harness process starts with its working directory set to the current project root |
| [`phase-12-13-and-45.md`](phase-12-13-and-45.md) | 1 | Phases 12, 13 and 45 — the lifecycle event bus, the session API, and failure isolation (18 of 24) |
| [`phase-12-18-and-19.md`](phase-12-18-and-19.md) | 1 | Phases 12, 18 and 19 — the event log, portable checkpoints, and the wiring that had no caller (25 of 28) |
| [`phase-2.md`](phase-2.md) | 6 | Phase 2 — Persist Glasshouse session metadata independently from the native harness session files<br>Phase 2 — Persist a mapping between Glasshouse session IDs and native harness session IDs when native IDs are available<br>Phase 2 — Persist the harness type, creation time, last activity time, role, lifecycle state, and project identifier for every session<br>Phase 2 — Persist the process presentation mode for every session<br>Phase 2 — Persist enough metadata to distinguish active, resumable, closed, and failed sessions<br>Phase 2 — Never store provider credentials directly in the project memory database |
| [`phase-20-22-and-23.md`](phase-20-22-and-23.md) | 1 | Phases 20, 22 and 23 — durable project memory, its lifecycle, and FTS5 search (31 of 34) |
| [`phase-21.md`](phase-21.md) | 3 | Phase 21 — the five extraction lines that stay open, and why<br>Phase 21 — migration 6, provenance, and a failure the coding session survives (lines 814, 816, 820)<br>Phase 21 — "Allow memory extraction to run after task completion" stays OPEN, and the criterion that decides it |
| [`phase-21-credential-acceptance-condition.md`](phase-21-credential-acceptance-condition.md) | 1 | Phase 21 credential acceptance condition — the extractor is never shown, and never emits, credential material |
| [`phase-21-extraction-contract.md`](phase-21-extraction-contract.md) | 1 | Phase 21 extraction contract — "Define a structured JSON schema…", "Feed the extractor bounded session/event chunks…", "Require the extractor to classify every emitted memory into one supported memory kind.", "Require the extractor to distinguish failed approaches from accepted decisions.", "Require the extractor to avoid duplicating an existing active memory when nothing materially changed." |
| [`phase-21-manual-extraction.md`](phase-21-manual-extraction.md) | 1 | Phase 21 manual extraction — "Allow memory extraction to run manually for debugging and evaluation." (line 818) |
| [`phase-21a-authority-classes.md`](phase-21a-authority-classes.md) | 1 | Phase 21A authority classes — all seven classes, classification by authority, conservative classification, explicit promotion (lines 828–841) |
| [`phase-21b.md`](phase-21b.md) | 1 | Phase 21B — decision provenance, 11 of 11 (lines 844–854) |
| [`phase-2a.md`](phase-2a.md) | 2 | Phase 2A — Make unsupported platform/harness combinations fail with a clear diagnostic rather than a partial broken session<br>Phase 2A — Support native Windows as a first-class Glasshouse runtime where the selected harness is available |
| [`phase-2b.md`](phase-2b.md) | 4 | Phase 2B — Mark every detected integration as available, configured, unconfigured, unsupported-version, or unknown<br>Phase 2B — Detect Antigravity when a supported Antigravity CLI executable is present<br>Phase 2B — Detect cmux when a usable cmux executable or supported cmux control environment is present<br>Phase 2B — Detect Ollama when a usable ollama executable or configured local endpoint is present |
| [`phase-2c.md`](phase-2c.md) | 2 | Phase 2C — first-run onboarding, and the acknowledgement `setup` had been promising (six lines, plus a 9A gap closed)<br>Phase 2C — the routing-model step, and Phase 2C at nineteen of nineteen |
| [`phase-2d.md`](phase-2d.md) | 3 | Phase 2D — the Providers and Launch Profiles settings sections (four lines)<br>Phase 2D — the Routing settings section and its five policy controls (six of seven)<br>Phase 2D — the settings view (nine of twenty lines) |
| [`phase-3.md`](phase-3.md) | 9 | Phase 3 — return from overlays to the active native session, and propagate resize to it<br>Phase 3 — Build the main interactive interface with Ratatui and Crossterm<br>Phase 3 — Create a persistent top bar that shows the project name, project root, and active session<br>Phase 3 — Create a persistent session bar that lists currently known sessions<br>Phase 3 — Create a central viewport reserved for the active session terminal<br>Phase 3 — Create a compact bottom status bar for Glasshouse-level key bindings and status messages<br>Phase 3 — Allow the user to move to the previous / next session with a keyboard shortcut<br>Phase 3 — Allow the user to open a session overview from the keyboard<br>Phase 3 — Keep the visual design text-first and avoid decorative graph visualizations that do not expose actionable state |
| [`phase-4.md`](phase-4.md) | 3 | Phase 4 — the multi-session PTY runtime (covers seven map lines)<br>Phase 4 — Implement a generic PTY-backed child-process abstraction for interactive harnesses<br>Phase 4 — Detect process exit independently from textual terminal output |
| [`phase-4-unfocused-control.md`](phase-4-unfocused-control.md) | 3 | Phase 4 unfocused control — "Support sending text programmatically to a PTY session without requiring the user to focus it."<br>Phase 4 unfocused control — "Support sending interrupt signals to a PTY session."<br>Phase 4 unfocused control — "Add a headless presentation mode in which a PTY continues running without occupying the visible session viewport." |
| [`phase-45.md`](phase-45.md) | 1 | Phase 45 — the crash report's race, and why the box was right anyway |
| [`phase-5.md`](phase-5.md) | 2 | Phase 5 — native terminal embedding (complete, 8 of 8)<br>Phase 5 — the input half of native terminal embedding |
| [`phase-5-7.md`](phase-5-7.md) | 1 | Phase 5/7 — the terminal handshake, and the defect it was hiding |
| [`phase-6.md`](phase-6.md) | 3 | Phase 6 — Make each adapter declare which native approval/permission modes it supports<br>Phase 6 — the harness adapter interface (eleven of twelve)<br>Phase 6 — Make each adapter declare which native communication-style mechanisms it supports and whether changing them requires a new or cleared native session |
| [`phase-7.md`](phase-7.md) | 6 | Phase 7 — Keep terminal-text parsing only as a fallback for state that cannot be obtained structurally<br>Phase 7 — Record Claude compaction events when they can be observed reliably<br>Phase 7 — Claude Code lifecycle hooks (one line closed, three pending one probe)<br>Phase 7 — Support resuming a known Claude Code session through Claude Code's native resume mechanism<br>Phase 7 — Add a Claude Code adapter that starts the real claude executable inside the current project root<br>Phase 7 — Capture the native Claude Code session identifier when it can be obtained reliably |
| [`phase-8.md`](phase-8.md) | 6 | Phase 8 — Record observed Codex compaction events or compaction-related state when available<br>Phase 8 — Detect Codex waiting-for-user and permission states structurally when possible<br>Phase 8 — Codex lifecycle hooks (three lines: integrate, translate, detect turn completion)<br>Phase 8 — Support resuming a known Codex session through Codex's native resume mechanism<br>Phase 8 — Capture the native Codex thread or session identifier when it can be obtained reliably<br>Phase 8 — the Codex adapter's first three lines |
| [`phase-9.md`](phase-9.md) | 3 | Phase 9 — the Antigravity conversation identifier, from an index rather than a walk (lines 2 and 3)<br>Phase 9 — the Antigravity adapter (three of seven, and one design defect it caught)<br>Phase 9 — the Antigravity adapter (probed, and blocked on authentication) |
| [`phase-9a.md`](phase-9a.md) | 1 | Phase 9A — harness launch profiles (seventeen of twenty-six) |
| [`phase-9b.md`](phase-9b.md) | 2 | Phase 9B — the child's environment, and Phase 9B at nine of nine<br>Phase 9B — scoped harness wrappers and shims (eight of nine) |
| [`phase-9c.md`](phase-9c.md) | 1 | Phase 9C — protocol compatibility as a filter, and Phase 9C at twelve of twelve |
| [`phase-9c-9d.md`](phase-9c-9d.md) | 1 | Phase 9C/9D — the provider protocol model and its built-in templates |
| [`phase-9d.md`](phase-9d.md) | 1 | Phase 9D — a connectivity test that makes a request, a manual model refresh, and a catalogue that survives a restart (three lines, Phase 9D at 14 of 14) |
| [`phase-9d-9a.md`](phase-9d-9a.md) | 1 | Phase 9D/9A — provider templates, header overrides, and the first gateway a harness can actually reach (five lines) |
| [`phase-9e.md`](phase-9e.md) | 2 | Phase 9E — the macOS Keychain, a labelled fallback, and a hang that would have frozen the TUI (three lines)<br>Phase 9E — secret storage (eight of thirteen) |
| [`phase-9f.md`](phase-9f.md) | 1 | Phase 9F — direct provider launch profiles (eleven of thirteen) |
| [`phase-9f-preflight.md`](phase-9f-preflight.md) | 2 | Phase 9F preflight — "Verify the selected harness, model, provider, and protocol combination before starting an interactive session when a cheap capability check is available." (line 465)<br>Phase 9F preflight — "Require the selected coding harness executable to be installed and usable before offering an interactive direct-provider or gateway-backed launch profile." (line 466) |
| [`phase-9g.md`](phase-9g.md) | 3 | Phase 9G — the Anthropic Messages ingress, and a credential the child never sees (ten lines)<br>Phase 9G — the last two ingresses, and Phase 9G at nineteen of nineteen<br>Phase 9G — the local gateway process (seven of nineteen) |
| [`phase-9g-refined.md`](phase-9g-refined.md) | 1 | Phase 9G refined — the several-providers refusal is lifted, and why that is not a reversal |
| [`phase-9h.md`](phase-9h.md) | 1 | Phase 9H — sticky gateway routing, 13 of 14 (lines 505–518) |
| [`phase-9i.md`](phase-9i.md) | 2 | Phase 9I — free-pool routing, 9 of 14 (lines 527–540)<br>Phase 9I — the disposable policy gets its caller (lines 530, 531, 532, 540) |

## Unfiled entries

5 entries whose heading did not name a phase — not guessed into a phase file, filed in [`unfiled.md`](unfiled.md) instead:

- Correction the orchestrator made on review: z.ai's model list stays `Unverified`
- The design defect the worker refused to implement
- A claim Windows would not support, and the box that came back off
- Correction, later the same day: the declaration now carries argv
- Migration 7 — `lifecycle_events` rebuilt, and `seq` proven durable

