# Dogfooding — the lane the Decompression ruling opened

> Process, not product. Capability requirements live only in `docs/product/capability-map.md`.

**Why this file exists.** Every real defect this project found was found by
running the shipped binary in a real terminal, never by a unit test
(measured, and the user's ruling of 2026-09-03 says the same: mutation tests
prove a test watches a line, not that the Claude/Codex/provider chain works).
The ruling makes it a lane: **one real session per working day**, the shipped
binary driving a real harness on a real project for at least an hour, with
the orchestrator watching memory extraction, routing, the context firewall
and the shell — and findings filed here, then packaged **by risk**.

## The protocol, short

1. Build the binary from `main` (`cargo build -p glasshouse`), run it from a
   scratch checkout of a real project (this repository is fine), in its own
   cmux pane, through the normal `glasshouse` entry point — never a test
   harness, never a fake provider.
2. Launch a real Claude Code session through it; give it an hour of ordinary
   work (a small task with edits, a search, a failing test). Watch: the
   launch briefing (file-observed and file-referenced sections), the firewall
   hook's reductions and its `file_touched` rows, the memory extraction at the
   session's end, `glasshouse memory search --path <a file it edited>`,
   `glasshouse route`, `resources`, `entitlements`, and the shell's activity
   view.
3. Record below: date, commit, harness and version, what was done, **every
   surprise** — a wrong number, a stale label, a slow step, a message that
   did not help, a crash — with the exact command and output. No prose about
   what worked beyond one line.
4. Each surprise becomes a packet sized by its mechanism and tiered by risk,
   or a register row saying why not. The ledger entry for the packet links
   back to the session here.

## Sessions

### 2026-09-03 — `2d1dc4d`, claude-code, read-only surfaces only

Scratch project: a worktree of this repository. Covered: `sessions`, `route`,
`route --help`, `resources`, `entitlements`, `doctor`, `sessions show`, and
`launch` up to the point it refuses.

**No defect found.** Three things were checked against a suspicion and the
suspicion lost each time — recorded because "I looked and it was fine" is the
result, not an absence of one:

- `route` and every `launch` print a 21-line *why* of which 19 are `+0.000`.
  It reads as noise on a first run. It is **deliberate**: `commands/route.rs`
  states that a reader must be able to tell "provider health was equal" from
  "provider health was never read", so an inert term is named rather than
  dropped. Settled decision, no contradictory evidence — **curiosity, not a
  package.**
- A launch that fails records `state failed` and `sessions show` gives no
  reason; `SessionLifecycle::Failed` carries no payload. But the CLI **does**
  say why at the time, and says it well: *"a harness session needs a terminal
  on both standard input and standard output; run this from an interactive
  terminal rather than through a pipe or a redirect."* The user is told. Later
  diagnosis from the record alone is weaker — **debt**, successor below.
- `resources` prints a raw epoch beside the useful relative age
  (`last observed unix 1788430539 (0s old; provider limit 900s)`) —
  **curiosity.**

**Successor (debt, do not schedule on its own):** if a second session's record
ever has to be diagnosed after the fact, give `SessionLifecycle::Failed` a
reason and surface it in `sessions show`. One occurrence, and the live path
already reports correctly.

**The interactive half, run properly.** A real `claude-code` session was
launched by the shipped binary into a cmux pane, given a real task (read
`events/mod.rs`, describe `LifecycleEvent`), and exited. It answered
correctly; 69k tokens, ~$0.91. Glasshouse tracked it end to end: `active` /
`idle` while waiting, `task continuity 1 task completed`, the native session
id captured, `resumable` after exit, and the resume line printed. That part of
the product works.

**The finding, and it is ours not Glasshouse's.** On exit, Claude Code warned:
*"Permission allow rule (.claude/settings.json): `Write(.../report-*.md)` is
not matched by file permission checks — only `Edit(path)` rules are."* Both
worker-report allow-rules in this repo are written as `Write(...)` and have
therefore never applied. **That is why workers stall on a permission prompt
when they write their report** — a stall this orchestrator hand-approved
several times on 2026-09-03 while believing it was a `cd` classifier issue.
One demonstrated cause, repeated effect: **defect**, fix is `Write(` →
`Edit(` in `.claude/settings.json`, and it is the user's call because it
widens what an agent may do unattended.

**Recorded uncertainties, not defects.** `model` stays *"the harness's own
default"* and pairing class `unknown` although the harness's own status line
names the model — honest by design (Glasshouse never invents a reading), but a
visible gap. `memory extract` is a command rather than something this bare
launch ran, so a one-turn session left no memories; whether that is expected
without the firewall hook installed was not established and did not block
anything.

**Method note.** An earlier attempt at this session was abandoned on a false
conclusion — that this context could not drive an interactive TUI or read a
pane back. Both are wrong. `cmux read-screen --surface <s>` reads the pane,
and `cmux send` takes its text **positionally**; the `--text` flag does not
exist, so cmux typed the literal string `--text …` into the shell. The
recovery is the one `scripts/dev/new-worker.sh` already implements: wait for a
prompt, send, read the line back and confirm it is intact, `C-u` and retry if
not, and only then press Enter.

### 2026-09-05 — `149240c`, claude-code 2.1.261 (Fable 5.1), a real task with edits, a search and a test run

Scratch project: `.worktrees/dogfood-20260905`, a detached worktree of this
repository. The binary built from `main` at `149240c`, launched as
`glasshouse launch claude-code --response-role interactive` into its own cmux
pane; three tasks, fifteen minutes of harness time, 149k tokens, about $4.
Driven from the orchestrator's pane over `cmux send`, exactly like a worker.

**What was done.** Task 1: fix the `999` false positive in
`tests/routing_cost.rs`'s cross-project isolation test (the nightly red of run
33985317460) — done correctly in 2m42s, with the header line stripped and its
shape asserted. Task 2: the JSON sibling the session found on its own, plus a
survey of every negative-substring assertion under `tests/` (189 sites; only the
two `routing_cost.rs` ones exposed, the rest safe by needle shape, limits of the
regex stated). Task 3: the memory, sessions and routing-cost readouts from inside
the session. Both fixes landed on `main` as `ad05372`.

**Glasshouse tracked it end to end:** `sessions` showed it `active` then
`resumable`; `sessions show` carried the native session id, `3 tasks completed`,
`prompt cache hot (estimated)`, `compactions 0`; `lifecycle_events` recorded
`session_started`, three `turn_started`/`turn_ended completed` pairs from the
`UserPromptSubmit`/`Stop` hooks, and `process_exited 0`; `route` preferred the
warm session at +2.097 over a fresh start at −1.100 with every inert term named.
The hooks reach the harness on its argv (`--settings` with `hook --event
UserPromptSubmit|PermissionRequest|Stop|StopFailure|PreCompact` and `edit-intent
hook`), merged with cmux's own hooks, both active.

**Surprises, each with the exact reading:**

- **Memory extraction ran after every turn and produced nothing, and said why.**
  `evaluation_observations` holds three `disposable_route_decided` rows, each
  reading *"none configured (Phase 39 supplies the provider): no provider is
  configured for Glasshouse's own support work — no model was called"*; this
  machine's `config.toml` has an empty `[providers]` table (fifteen keys live in
  `provider-keys.env`, none configured for support work). `memory search --path
  crates/glasshouse/tests/routing_cost.rs` therefore answered *"No current
  memories"* after a session that edited that file and found something worth
  remembering. Honest, and a **precondition for the next session**: configure
  one support-work provider (Ollama is enabled as an integration) before
  launching, or extraction cannot be watched at all. Not a package.
- **The context firewall cannot be watched from a launched session until
  `[context_firewall] mode` is set:** the default is `off`, which by design
  registers no `PostToolUse` hook (`config/firewall.rs`), so no reduction and no
  `file_touched` row can appear. Same precondition class as above: set `shadow`
  or `safe` in the scratch project's config next time. Not a package.
- **`entitlements` on a project with no pool prints *"Also served, by entries no
  longer configured: `claude-code`"*** — for a session that had been running for
  a minute under the harness's own sign-in, "no longer configured" is a label
  that did not help. **Curiosity.**
- **`routing-cost --json` printed nothing, exit 0** — correct: a native session
  never crosses the gateway, so it writes no `routing_observations` row. The
  prose readout says as much; the JSON's silence is the documented empty-window
  shape. Not a surprise once read, recorded so nobody re-derives it.
- **On exit, Claude Code still warns** *"Permission allow rule
  (.claude/settings.json): `Write(…/report-*.md)` is not matched by file
  permission checks"* — the 2026-09-03 finding is unchanged; the repo's two
  worker-report allow rules have never applied, and workers write reports under
  auto mode's classifier instead. Recorded once already; still open, still ours.
- **`/exit` typed into the input needs Enter twice** (the slash-command popup
  takes the first) — a driver's note, not Glasshouse's.

**No defect found in the product path exercised.** The two test defects the
session fixed were the sweep's, found by CI and closed here in one session.

### 2026-09-06 — `221e355`→`4b70d72`, claude-code 2.1.261 (Fable 5.1), a real task with edits and a test run

Scratch project: `.worktrees/dogfood-20260906`, a detached worktree of this repository at `221e355`. Binary built from `main`, run by path (the dev shim resolves by the checkout you stand in and wanted a build in the worktree — a shim fact, not a product one), launched as `glasshouse launch claude-code --response-role interactive` in cmux workspace 122, driven from the orchestrator's pane over `cmux send`. Under decision 8 the machine's config gained its first support-work provider (`[providers.groq]`, `credential_env = ["GROQ_API_KEY"]`) so memory extraction has a model to call. `sessions` showed the session `active`, `claude-code`, profile `native`, presented `embedded` within seconds.

**Tasks.** Task 1: `env | grep -c GROQ_API_KEY` from inside the session. Task 2: make `tests/gateway_failure_taxonomy.rs`'s `each_failure_class_is_recorded_from_status_headers_and_framing_alone` match ledger rows to cases by content rather than position (the Windows ordering flake of sweep `33995530968`).

**Surprises, each with the exact reading:**

- **The harness child inherits Glasshouse's own provider credential.** Task 1 answered **1**: `GROQ_API_KEY`, exported in the launching shell because `credential_env` names it as the credential source, is present in the Claude Code session's environment. `launch.rs :: HarnessLaunch::build_command` replays the overlay's `EnvChange`s only; nothing removes a configured `credential_env` name. **Defect against map line 488** — un-ticked, successor `GH-LAUNCH-STRIPS-PROVIDER-CREDENTIALS` (Red).
