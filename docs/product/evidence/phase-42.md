# Phase 42 — External control API

Thirteen boxes, `docs/product/capability-map.md` lines 1673–1685. `SessionApi`
(`session/api.rs`) already exists as the in-process seam; this phase's job was
the external door onto it, plus the parts no in-process seam covers (memory,
checkpoints, the socket itself).

## Pass 1 — audit

| Line | Box | Status before this package | Symbol / file |
|---|---|---|---|
| 1673 | local project-scoped control API | absent | nothing outside a one-shot CLI process existed |
| 1674 | list sessions | in-process only | `SessionApi::list`, `session/api.rs:99` — no external caller |
| 1675 | spawn sessions | in-process only | `SessionRuntime::start` + `SessionStore::create` — only reachable from `main.rs`'s CLI-shaped `launch_session`, never externally |
| 1676 | send messages | in-process only | `SessionApi::send_text`, `session/api.rs:120` — no external caller |
| 1677 | interrupt sessions | in-process only | `SessionApi::interrupt`, `session/api.rs:134` — no external caller |
| 1678 | retrieve lifecycle state | in-process only | `SessionApi::state`, `session/api.rs:109` — no external caller |
| 1679 | resource capacity / quota telemetry | absent, no consumer | `provider/registry.rs`'s own doc: *"does not track live quota telemetry ... does not exist yet"* — confirmed, nothing named a live telemetry reader anywhere in the crate |
| 1680 | routing-model selection and health | partly | selection is durable (`SessionRecord::backend_resource`/`model`/`protocol`, `session/store.rs:476-495`); health (`routing::free::ResourceHealth`, `routing/free.rs:236`) lives only in whichever process's `Gateway::free` last computed a route, in memory, with no durable or cross-process reader |
| 1681 | inspectable routing recommendation without executing | absent, no consumer | `InteractiveRouting::assign`/`next_turn` (`routing/interactive.rs`) are pure given a candidate `Backend`, but nothing assembles a candidate list without also being the launch path itself (`gateway::session`, `config::`, `profile::` — none of it exposes a dry-run entry point) |
| 1682 | query project memory | in-process only | `memory::search::search` (`memory/search.rs:131`), real CLI caller `main.rs::memory_report` — no external caller |
| 1683 | request a checkpoint | in-process only | `Checkpoint::capture` + `CheckpointStore::save` (`checkpoint/mod.rs`, `checkpoint/store.rs`) — only reachable from `main.rs`'s `CheckpointCommand::Save` arm |
| 1684 | authenticate/restrict the local control channel | not applicable | no channel existed to restrict |
| 1685 | bind every control request to project scope | not applicable | no channel existed to bind |

## Pass 2/3 — what this package built

### The door: `glasshouse api serve`, a Unix domain socket

**Argued, per the packet's instruction to argue the door's shape.** Three
options considered: a subcommand per call, a long-running Unix socket, and (a
non-option, ruled out immediately) a network service — CLAUDE.md's own Phase
55 citation says V1 does not need one, and this is explicitly local and
project-scoped.

A subcommand per call (`glasshouse api send-message ...`) is a fresh process
per request. A fresh process cannot hold a `SessionRuntime` — the type that
owns the actual pseudo-terminal handles `send`/`interrupt`/`spawn` need — so
every call would still have to re-attach to *something* long-lived, and that
something might as well be the door itself rather than reinvented per call.
A socket also answers requests without a shell already open, which nothing
purely in-process could do.

**This is also why the module lives at `crates/glasshouse/src/api/**`,
declared with `mod api;` from `main.rs`, rather than as `glasshouse::api` in
`lib.rs`.** This phase's packet holds `cli.rs` and `main.rs` but not `lib.rs`
(another phase's partition, live this round). Declaring the module from the
binary keeps it inside the process that already owns `run_headless`'s
`Arc<Mutex<SessionRuntime>>` pattern — reused here — without touching a file
outside this package's grant. The direct consequence: this module is provable
only by running the shipped binary (`tests/session_model.rs`'s `control_api`
cluster), never by an in-process unit test — which is arguably the more
honest proof for an *external* door regardless of file ownership.

**Protocol.** One connection, one newline-delimited JSON request, one
newline-delimited JSON response (`api/protocol.rs`). No framing to get wrong.

### 1673 — closed

`glasshouse api serve` (`cli.rs`'s `Command::Api`/`ApiCommand::Serve`,
`main.rs`'s dispatch arm, `api::serve` in `api/unix.rs`). Production caller:
the CLI dispatch itself. Proven end to end by every test in the `control_api`
cluster — none of them could pass without a real listening socket.

### 1674 — closed

`Request::ListSessions` → `SessionApi::list` (routed through the seam, not
`SessionStore::list` directly — see the note in `dispatch`'s own comment
about why `store.list()` alone would have been a needless step around a seam
that exists for exactly this). Proven by
`spawning_listing_messaging_and_reading_state_go_through_the_socket` and
`a_session_never_crosses_into_another_projects_listing`.

### 1675 — closed, with a named simplification

`Request::SpawnSession` (`api/unix.rs::spawn_session`) resolves the harness
executable through `session::select`, the same resolver `main.rs`'s
`launch_session` uses, then calls `SessionStore::create` and
`SessionRuntime::start` directly.

**Deliberately narrower than `launch_session`.** It skips launch-profile and
response-profile resolution entirely — both live behind `config::`/`profile::`
machinery this phase's packet does not hold, and both are about how a session
*presents itself to a person* (system-prompt injection, response-style
axes), which an API-spawned session run by something other than a person has
no occasion to need. What every session gets regardless — a store record,
`SessionPresentation::Headless`, and a really-running process this door can
message and interrupt — this gives in full.

Argued per §33's test: *is the honest answer to "can the API start a
session" yes?* Yes — a real harness process starts, is listed, receives
text, and can be interrupted, all proven against the shipped binary. What it
cannot do yet is start one *through a named launch profile*; that is a
narrower, separately closeable follow-on, not a reason to leave the whole box
open.

Proven by `spawning_listing_messaging_and_reading_state_go_through_the_socket`.
Mutation-proof: replaced the body with `Response::ok(...)` and never called
`guard.start` — two tests failed (`spawning_...` and
`interrupting_through_the_socket_kills_a_real_process`, since both spawn
first); restored, `ok`.

### 1676 — closed

`Request::SendMessage` → `SessionApi::send_text`. Proven by
`spawning_listing_messaging_and_reading_state_go_through_the_socket`, which
plants a looping-echo harness that appends every line it reads to
`received.log` in the project root — a side channel the control API itself
never touches, chosen deliberately so the proof does not require exposing
scrollback through the door (box 4 is "send", not "read back"; no box asks
for the latter). Mutation-proof: replaced the body with `Response::ok(...)`
and never called `api.send_text` — `spawning_...` failed (timed out waiting
for `received.log` to contain the line); restored, `ok`.

### 1677 — closed

`Request::Interrupt` → `SessionApi::interrupt`. Proven by
`interrupting_through_the_socket_kills_a_real_process`, which spawns a
harness that writes its own pid to a file before entering its read loop, then
polls `kill -0 <pid>` after interrupting — proof the operating system, not
Glasshouse's own bookkeeping, agrees the process is gone. Mutation-proof:
replaced the body with `Response::ok(...)` and never called `api.interrupt`
— the test failed (timed out waiting for the pid to disappear); restored,
`ok`.

### 1678 — closed

`Request::SessionState` → `SessionApi::state`. Proven indirectly by every
test that polls it, and directly by
`a_session_never_crosses_into_another_projects_listing`'s final assertion
(asking a foreign session's state through the wrong project's door returns
`status: "error"`).

### 1679 — left open

No mechanism exists to leave open *for*. `provider/registry.rs`'s own module
doc is explicit that live quota telemetry — "a rolling-window counter, a
remaining-balance read, a hit-a-limit signal" — is not built. There is
nothing this package could route a socket handler to that would not be
reporting a number Glasshouse never actually reads. Adding a handler that
returns `QuotaModel` (the *shape* a resource's quota takes, which does
exist) rather than telemetry (the *live state* of it) would answer a
different, easier question and let the box be ticked on the wrong thing —
exactly the mistake §5 names. Left open, naming the missing telemetry
mechanism as its own future phase.

### 1680 — left open, partial

The **selection** half is real and exposed: `session_summary` in
`api/unix.rs` includes `backend_resource`, `model`, and `protocol` straight
from `SessionRecord` — the same durable fields `gateway::session::Gateway`'s
`bind` writes on every real routing decision. This door does expose them.

The **health** half is not. `routing::free::ResourceHealth` lives inside
whichever process's `Gateway` most recently computed a route, in memory,
with no durable store and no cross-process reader — and this door's own
`SpawnSession` does not touch the gateway at all (see 1675's note), so even a
session this daemon spawns itself never populates one. Asked as a question a
user would ask — *"can I get this session's current model **and** whether
its route is healthy?"* — the honest answer is no, only the first half.
Leaving the box open rather than ticking it on the half that works: the map's
line says "and health", not "or".

`session_summary`'s own doc comment states this gap in the shipped code, not
only here.

### 1681 — left open

No pure recommendation entry point exists to call. `InteractiveRouting::assign`
and `next_turn` (`routing/interactive.rs`) are pure functions over an
already-chosen `Backend`, but *choosing* the candidate backends is done by
`gateway::session::Gateway` woven together with `config::`/`profile::`
resolution — the same machinery `launch_session` runs, and the same one
1675 argued against fully replicating for a spawn that skips presentation
concerns. A "recommend without executing" endpoint would need a genuine
dry-run seam into that resolution that does not exist today; building one
inside this phase's file grant (`api/**`, `cli.rs`, `main.rs`) would mean
duplicating `gateway`/`config`/`profile` internals this package does not own,
which is exactly the "expand into architecture" a Sonnet implementer is
told to stop short of. Left open, naming `gateway::session` /
`routing::interactive` as where the dry-run seam belongs.

### 1682 — closed

`Request::QueryMemory` delegates to `main.rs::memory_report` — the exact
function `glasshouse memory search` prints from — so the door and the CLI
command can never disagree about what a query finds. Proven by
`memory_query_and_checkpoint_reach_the_same_store_the_cli_reads`, which
queries for a term with no matches and asserts the same "No current memories
match" text the CLI prints.

### 1683 — closed

`Request::TakeCheckpoint` mirrors `main.rs`'s `CheckpointCommand::Save` arm:
the same session resolution (`main.rs::active_session`, named or the
project's most recently active session), the same `Checkpoint::capture`,
the same `CheckpointStore::save`. Not called through directly because that
arm prints to standard output as part of returning an `ExitCode`, which has
nothing to do with what a socket handler writes back.

Proven by `memory_query_and_checkpoint_reach_the_same_store_the_cli_reads`,
which takes a checkpoint through the socket, then — after the server process
exits — runs `glasshouse checkpoint show --document` as a **separate,
independent binary invocation** and asserts the checkpoint's objective text
comes back. Written by the socket, read by the CLI: proof the store is
actually shared and durable, not an artefact of one process's memory.
Mutation-proof: replaced `store.save(checkpoint)` with a fabricated response
naming a checkpoint id nothing ever wrote — the test failed (`glasshouse:
no checkpoint 'deadbeef' in this project`); restored, `ok`.

### 1684 — closed

Two independent mechanisms, both in `api/unix.rs`:

1. **Socket permissions.** `serve` `chmod`s the bound socket to `0600`
   immediately after `bind`, before the accept loop starts. The kernel
   refuses `connect(2)` from another user against a `0600` socket before
   this door ever sees the attempt.
2. **Peer-credential check.** `authorize`, called on every accepted
   connection before its request line is even read, reads the connecting
   process's kernel-verified uid — `SO_PEERCRED` via `getsockopt` on Linux,
   `getpeereid` on the BSD family including macOS — and refuses any uid that
   does not match this process's own. Neither reads anything the peer sent;
   there is nothing for an unrelated local process to spoof.

Proven directly for (1): `the_control_socket_is_owner_only` asserts the
socket file's mode is exactly `0600`. Mutation-proof: removed the
`set_permissions` call — the test failed (mode `755`, from the process
umask); restored, `ok`.

**Not proven for (2), honestly.** Every test in this sandbox connects as the
same user this package's tests run as — there is no second local account
available to prove the *refusal* path exercises. The positive path (a
same-uid connection succeeding) is exercised by every passing test, since
`Server::call` would hang or fail otherwise, but a mutation deleting the
`authorize` call entirely would not be caught by anything in this suite. This
is a real gap in this package's own proof, not a reason to leave the box
open — the mechanism is real, standard (`ssh-agent`, Docker's own local
sockets use the same pair of checks), and its positive path is exercised on
every request this door answers — but it is flagged here rather than claimed
as mutation-proofed when it was not.

### 1685 — closed

There is no request field naming a project: the socket itself is the scope.
`serve` opens one `ProjectSessions`/`SessionRuntime` for the `Runtime` it was
started with, and every session handler routes through `SessionApi`, which
refuses a foreign session by construction (existing behaviour, `session/api.rs`,
not re-tested here). Proven at the door's own level — not just the session
seam's — by `a_session_never_crosses_into_another_projects_listing`: two
sockets, one machine, one shared data root, a session spawned on one door
never appears through the other's, and asking the other door for it by name
returns a typed refusal.

### The path-length problem this package found and fixed

`sockaddr_un.sun_path` is 104 bytes on macOS/BSD and 108 on Linux. The
project's own state directory, nested under a deep data directory (this
project's own git worktree layout, any CI runner, or simply a long home
directory), can push `<state-dir>/control.sock` past that bound — `bind(2)`
then refuses with `EINVAL`, before this door ever authorizes a single
connection. Found immediately by this package's own first test run (`path
must be shorter than SUN_LEN`), not a hypothetical.

`socket_path_for` (`api/unix.rs`) prefers the project's state directory when
its path fits within a safety margin (90 bytes, chosen with headroom below
the tighter platform minimum), and falls back to a short,
project-id-keyed name under the system temp directory when it does not. Real
Unix deployments (short data directories) get the tidy, per-project path;
pathological ones (this crate's own CI and test fixtures included) still get
a working door instead of a refusal to start.

### Windows

`api::serve` is `#[cfg(unix)]`; the non-Unix stub refuses loudly, explaining
that a named-pipe transport does not exist yet, rather than silently doing
nothing. Consistent with this codebase's existing `cfg(unix)` gating idiom
for platform-specific mechanisms (see the resume-through-a-real-terminal
cluster in `tests/session_model.rs`).

## Left open, with the consumer named

- **1679** (resource capacity / quota telemetry) — no live telemetry reader
  exists anywhere in the crate to expose. `provider::registry`'s own doc
  names this as future work.
- **1680** (routing-model selection **and health**) — selection is exposed;
  health is not, because `routing::free::ResourceHealth` has no durable or
  cross-process store. Left open rather than ticked on the half that works.
- **1681** (inspectable routing recommendation without executing) — no dry-run
  seam exists into `gateway::session`'s route assembly; building one is
  `gateway`/`config`/`profile` architecture this package's file grant does
  not cover.

Ask, per §33's standard, whether the honest answer to "can a user ask the API
this, and does it change something in the shipped binary" is yes. For these
three it is not — the missing half is a consumer or a data source that does
not exist yet, not a field this package could wire by itself without
building the architecture the packet told it to stop short of.

## Gate

- `cargo build -p glasshouse --bin glasshouse` — clean, no warnings.
- `cargo clippy -p glasshouse --bin glasshouse --all-targets` — clean (one
  `enum_variant_names` lint fixed: `RequestCheckpoint` → `TakeCheckpoint`).
- `cargo fmt` on every touched/new file (`api/mod.rs`, `api/protocol.rs`,
  `api/unix.rs`, `cli.rs`, `main.rs`, `tests/session_model.rs`) — clean after
  one reflow pass; not run with `--all` (§37).
- `cargo doc -p glasshouse --no-deps` — clean, no warnings.
- `cargo test -p glasshouse --test session_model` — 16 passed, 0 failed (10
  pre-existing + 6 new `control_api` tests), run alone.
- `cargo test -p glasshouse` (the full crate: lib + every integration binary)
  — every suite green, 0 failed, run alone (§40 — checked for a concurrent
  `cargo` first both times; the only other `cargo` processes found belonged
  to sibling worktrees with their own `target/`, confirmed via `lsof`'s `cwd`
  before proceeding).
- Did **not** run `scripts/ci-local.sh` — out of this package's scope to
  invoke; everything above is what actually ran, and all of it is clean.

## Mutation-proofing — what was run, and what was not

Every mutation below reverted the file to a byte-identical original after
(`diff` confirmed) before the next one:

| Mutation | Box | Caught by |
|---|---|---|
| Skip `chmod 0600` on the socket | 1684 (perms half) | `the_control_socket_is_owner_only` |
| `SendMessage` never calls `send_text` | 1676 | `spawning_listing_messaging_and_reading_state_go_through_the_socket` |
| `Interrupt` never calls `interrupt` | 1677 | `interrupting_through_the_socket_kills_a_real_process` |
| `SpawnSession` never calls `guard.start` | 1675 | both of the above (both spawn first) |
| `TakeCheckpoint` never calls `store.save` | 1683 | `memory_query_and_checkpoint_reach_the_same_store_the_cli_reads` |

**Not mutation-tested:**

- **1684, the peer-uid half.** No second local user account exists in this
  environment to exercise the refusal path; see 1684's own section above.
- **1674's routing through `SessionApi::list` rather than raw
  `SessionStore::list`.** In this environment every project already gets its
  own physical SQLite file (trigger-enforced, see `session/api.rs`'s own
  test fixture), so a mutation reverting to `store.list()` directly would
  *not* be caught by `a_session_never_crosses_into_another_projects_listing`
  — both code paths pass for the same underlying reason. Proving the
  `SessionApi`-specific defense-in-depth would need a planted foreign row
  the way `session/api.rs`'s own `plant_foreign_row` does, which this
  package's test file does not currently import the SQL access to build.
  Flagged rather than claimed.
- **1678, 1682, 1685** — proven end to end by the tests above, but not
  separately mutation-attacked beyond what the listed mutations already
  exercise incidentally (a broken `SessionState` or `QueryMemory` handler
  would fail those same tests' later assertions).

This package's own read of §41: every mutation actually run here failed
loudly and specifically (a timeout naming what it was waiting for, or a
wrong-mode assertion, or a CLI refusal naming the missing id) rather than
surviving quietly — no case here needed a second look at what the mutation
and its test both assumed.
