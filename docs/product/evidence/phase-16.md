# Phase 16 — Worker transparency

Written by the PACKET-ORCHESTRATOR-ROLE Sonnet-implementer package. Per
`docs/process/worker-capabilities.md`, a worker does not tick capability-map
boxes, edit the map, or edit this file's status into the map — this entry is
handed to the orchestrator, who decides each box against the evidence below.

State: **COMPLETE** for map lines 744, 749 and 750. **NOT STARTED, blocked**
for 745, 746, 747 and 748.

> **The finding is sharper than "it needs the TUI", and it is a design question
> the next round must answer before it can write code.** Phase 11 already built
> the entry mechanism — `Enter` on the session overview — and **gated it to
> `SessionPresentation::Embedded`. Every worker this door spawns is
> `Headless`.** So the existing mechanism does not merely fail to cover these
> sessions; it *actively refuses exactly the sessions this phase is about*.
>
> Lines 746 and 747 are blocked behind 745 for the same reason. Line 748 has its
> raw material — `MessageOrigin::Machine` / `UserKeystroke` are recorded — and
> no read path that surfaces an intervention to an orchestrator.
>
> **The decision to take first: does an orchestrated worker become `Embedded`,
> or does the overview learn to attach to a `Headless` one?** That is a product
> question about what a "real session the user can inspect" means for a session
> nobody is looking at, and it is not a Sonnet's to settle.
>
> **Integrator's follow-up, and it makes the question bigger rather than
> smaller.** Two facts, both checked rather than assumed:
>
> 1. **`glasshouse api serve` is a separate process from the TUI.** It is its
>    own top-level command (`main.rs`'s `Command::Api` arm), and nothing in
>    `shell/**` starts or reaches it. A worker spawned through the socket lives
>    in the API server's `SessionRuntime`; the TUI is a different process and
>    cannot render a pty it does not own.
> 2. **`session/attach.rs` cannot be reused for this.** Its own module doc says
>    so in as many words: *"Nothing here is reusable from inside a longer-lived
>    interface; the session runtime that multiplexes several harnesses needs a
>    different input path, not this one."* It is a whole-process transparent
>    bridge that owns the terminal for the life of the process, deliberately.
>
> So the overview's refusal — *"there is no viewport to focus into"* — is not a
> gate somebody forgot to relax. **It is an accurate statement about where the
> worker's pty actually lives**, and neither option is small:
>
> - **(a) an orchestrated worker becomes `Embedded`** — which means the socket
>   door must spawn into a runtime the TUI owns, i.e. the two processes become
>   one, or learn to share;
> - **(b) the TUI learns to attach to a running headless session** — which means
>   a pty handed between processes, and `attach.rs` says the input path for that
>   does not exist.
>
> **This is Red tier** (`worker-capabilities.md`: PTY lifecycle, process
> ownership, cross-process state) and it is a product decision before it is an
> implementation one: *what does "a real session the user can inspect" mean for
> a session whose process nobody is attached to?* Recorded here rather than
> guessed, because a Sonnet package sent at these four boxes would either invent
> an architecture or stall, and both cost a round.
>
> Lines 749 and 750 close on audit — no LLM client is reachable from
> `api::unix` or `session::api`; every request routes to a real
> `SessionRuntime` and a real pty. Line 744 closes on the stronger evidence: a
> **wholly separate `glasshouse sessions` process** reads what the socket
> spawned.

Seven boxes. Hypothesis 2 in the packet asked, before anything else, whether
these boxes have a production surface at all from this package's file grant
— `shell/**` is forbidden this round — and whether the answer changes box by
box. It does.

**Headline finding: box 1 does not need the TUI, and closes this package.
Boxes 2–5 do need it, and one of them (box 2) needs more than access — Phase
11 already built a mechanism that actively excludes exactly the sessions box
2 is about. Boxes 6–7 are structural claims already true, checked and cited
below, not built.**

## Box 1 — CLOSED (this package)

*"Ensure every worker created by an orchestrator appears immediately in the
normal Glasshouse session list."*

"The normal Glasshouse session list" is not the orchestrator's own view
(Phase 14 box 4, `list_sessions` through the socket) — it is the surface a
person runs: `glasshouse sessions`. Both read `SessionStore::list` against
the same project database file (`session/store.rs`, unowned by either
listing surface), so nothing structurally *could* separate them — but that
was an inference until tested against two genuinely separate processes.

Proven by
`a_worker_spawned_through_the_socket_appears_in_the_ordinary_sessions_listing`
(`tests/orchestrator_role.rs`): a worker is spawned through `glasshouse api
serve`'s socket in one process, and a **wholly separate** `glasshouse
sessions` invocation — a fresh process, same `--scope`/`--data-dir`/
`--config-dir`, no socket involved — is run afterward and asserted to list
it, role and all. "Immediately" is proven by there being no synchronization
step between the two calls beyond the spawn's own response: the CLI process
did not exist yet when the socket call returned.

Not mutation-tested beyond what box 1 of `phase-14.md` already covers: the
worker's role appearing correctly in this same listing is the identical
production path (`SessionStore::create` → `record.role.to_string()` in
`main.rs`'s row-printing, unowned and unedited by this package) that mutation
M1/M2 in `phase-14.md` already kills.

## Box 2 — NOT CLOSED, and not merely "needs the TUI"

*"Allow the user to enter any orchestrated worker while it is running."*

This needs `shell/**` (forbidden), confirming hypothesis 2's premise — but
reading `shell/**` (permitted; only editing is forbidden) found something
sharper than "no mechanism exists yet." One already does, and it refuses
exactly the sessions this box is about.

Phase 11 (`phase-11.md`, line 687) built "focus" — the overview's `Enter`
key, `ShellState::focus_overview_target` — and gated it, deliberately, on
`session.presentation == SessionPresentation::Embedded`. Every session this
package's door spawns is `SessionPresentation::Headless`
(`api::unix::spawn_session`, unchanged by this package, matches Phase 42's
own deliberate choice). **A worker spawned through the orchestrator's door is
therefore refused by name if a person tries to focus it from the overview
today** — not because nothing tries, but because Phase 11's own refusal,
built for a different reason (distinguishing "bring into the viewport" from
"this session has nowhere to be presented"), fires on it.

This is not a defect in Phase 11 — box 687 never claimed to cover a
headless session, and headless-vs-embedded is a real, load-bearing
distinction elsewhere in the codebase. It is a scoping fact the next package
that owns `shell/**` needs before touching `focus_overview_target`: closing
box 2 is not "add a key", it is "decide what focusing a *headless* session
even means" (there is no embedded viewport slot such a session already
occupies — focusing it would have to *change* its presentation, which is a
real design question, not a one-line gate change) — genuinely `gateway`/
gateway-shaped architecture-adjacent, in the sense practice §36 means: a
consumer being built by someone else is not automatically a consumer of your
policy, and here the existing consumer is actively the opposite of one.

**Left open.** The patch this needs is not named under `PATCHES ANOTHER
PACKAGE MUST APPLY` because it is not a small, mechanical addition — it is a
design decision (per worker-capabilities.md's own boundary: a Sonnet
implementer stops short of new architecture) that belongs to whichever
package next owns `shell/**` with the authority to make it.

## Boxes 3, 4, 5 — NOT CLOSED, need the TUI, no sharper finding

*"Allow direct user input to an orchestrated worker without requiring the
orchestrator as an intermediary." / "Allow the user to interrupt an
orchestrated worker directly." / "Record user intervention so the
orchestrator can be informed that the worker state may have changed."*

All three presuppose box 2 (a person is already looking at the worker's own
viewport) or a comparable TUI-side mechanism this package's file grant does
not reach. `shell/state.rs`'s existing `c` (interrupt) and message-sending
paths (Phase 11's `enter_session_mode`/send-text machinery) already operate
on *whichever session is presented* — but "presented" is exactly what box 2
cannot yet do for a headless worker, so these three inherit that same
blocker rather than adding one of their own. Box 5 in particular
("record user intervention") has no event-log shape decision made anywhere
in this package's file grant to build against; `events::MessageOrigin`
(`session/api.rs`'s own seam, Phase 14 box 2's evidence) already
distinguishes `Machine` from `UserKeystroke` at the point of delivery, so the
raw material for "was this a person or the orchestrator" exists — but
*surfacing* that to the orchestrator (a read path, not the write path this
package owns) is undesigned.

**Left open**, for the same package that resolves box 2.

## Box 6 — CLOSED (audit only, no code changed)

*"Never implement orchestration workers as hidden in-process LLM calls when
a native harness session was requested."*

Structural, per design decision 3: proven the same shape
`routing/interactive.rs` already uses — a test that scans source for a
forbidden pattern and fails the build if it appears, rather than a runtime
assertion. `api::unix::spawn_session` (this package's own file) has exactly
one production path to a running worker: `SessionRuntime::start`, which
spawns a real OS process through `HarnessLaunch` (`launch.rs`, unowned,
re-verified by reading it: `spawn()` calls through to `portable-pty`'s
`CommandBuilder`/`PtySystem`, never an in-process model call). There is no
branch anywhere in `api/unix.rs` or `session::api::SessionApi` that answers a
request with generated text instead of routing to a live session — every
`Request` variant either reads `SessionStore`/`CheckpointStore` state or
writes to a `SessionRuntime`-held process.

Grepped the whole crate for anything an in-process LLM call would need
(an HTTP client constructed anywhere near `session::api` or `api::unix`, an
`anthropic`/`openai`-shaped request builder reachable from either): nothing
found. `provider::` clients exist for routing/gateway purposes elsewhere in
the crate, entirely unreferenced from this package's files.

## Box 7 — CLOSED (audit only, no code changed)

*"Preserve the rule that every worker remains a real session the user can
inspect."*

Same evidence as box 6, from the other direction: every session
`api::unix::spawn_session` creates goes through `SessionStore::create` (the
one door a session record can come through, Phase 10's unchanged guarantee)
and is therefore listed, showable, and closeable through every ordinary
surface — proven directly by box 1's own test above, which is exactly "the
user can inspect it" made concrete: a wholly separate `glasshouse sessions`
process sees it. Box 2's gap (above) means a person cannot yet *enter* it
interactively, but "inspectable" (listed, its state queryable, its harness
and role visible) does not require that — box 685's own reading in
`phase-11.md` draws the same distinction between a fact being *shown* and a
session being *actionable* from a given surface, and applies it the same
way here: box 7 asks for visibility, not for the interactive-entry
capability box 2 is separately about.

## Gate

Same run as `phase-14.md` — this entry adds no new tests of its own beyond
the one cited for box 1, already counted there. No file this entry's claims
depend on was edited by this package; boxes 6 and 7's evidence is entirely
read-only inspection of `api/unix.rs` (owned, unchanged in the relevant
paths) and `launch.rs`/`session::api` (read-only, per the packet's grant).

## Missing evidence

- **Boxes 2–5 need a `shell/**`-owning package with room to make a design
  decision**, not merely file access — see box 2 above for exactly what
  that decision is.
- **Box 5's event-log shape for "the orchestrator learns a worker's state
  may have changed"** is undesigned; `MessageOrigin` gives raw material,
  nothing surfaces it as a read path today.
- **Windows/Linux execution.** Not run for this entry, same caveat as
  `phase-14.md`.
