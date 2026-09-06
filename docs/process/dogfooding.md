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

- **A configured provider is not a configured account, and the extraction's message points at the wrong one.** After task 2 (edits to `tests/gateway_failure_taxonomy.rs`, `7 passed`), `memory search --path crates/glasshouse/tests/gateway_failure_taxonomy.rs` answered *"No current memories"* again and `evaluation_observations` row 10 read *"none configured (Phase 39 supplies the provider): no provider is configured for Glasshouse's own support work — no model was called"* — with `[providers.groq]` present. `resources` listed no Groq resource; `entitlements` said the true thing: *"No `[entitlements]` entries are configured … Add one under `[entitlements.<name>]` to name an account it may charge."* The unit Glasshouse charges is the entitlement (Phase 56A), and `NoResource::NothingConfigured`'s sentence names the provider instead. Adding `[entitlements.groq] provider = "groq"` did not change it either. The real condition is `commands/routing_classification.rs :: disposable_candidates`: a provider contributes no candidate unless it is enabled **and names at least one model** in `free_models` or `metered_models` — a bare `[providers.groq]` with a template and a credential is invisible to support work. **Debt, one line:** `NoResource::NothingConfigured`'s sentence should say *no configured provider names a model for Glasshouse's own work — add `free_models` or `metered_models` to a `[providers.<name>]` entry*; a Green fix beside the next routing packet. Added `free_models = ["llama-3.1-8b-instant"]` on this machine (decision 8) for the rest of the session.

- **The record's task count is not a turn count, and the session read it as one.** After three turns the session's own `sessions show` said *1 task completed* and *no checkpoint*, and the session called that wrong. `session/store/context.rs:215` says what the number is: completed **task boundaries** observed in the event log, which a turn ending is not (`commands/sessions.rs:241`). Not a defect; the wording invites the misreading. Debt, one line: say *task boundaries*.

- **Extraction routed at last, and then nothing is recorded.** With `[entitlements.groq] provider = "groq"` and `free_models = ["llama-3.1-8b-instant"]` in place, `evaluation_observations` row 16 reads *`llama-3.1-8b-instant` on groq — free, used by user preference*. After it: no row of any kind, `memories` empty, `memory_files` empty, `routing_observations` empty; `memory search --path …` on the file the session edited twice still answers *No current memories*. Whether the model answered, timed out at the hook's 5-second bound (`memory_extraction.rs :: EXTRACTION_BOUND`), or returned nothing worth storing is not decidable from anything durable — the hook's one stderr line is the only trace, and the harness swallows it. `glasshouse memory extract` cannot reproduce it either: it is an evaluation harness that takes the model's reply from a file. **Defect (diagnosability)** — successor `GH-EXTRACTION-OUTCOME-OBSERVED` (Amber): one observation per hook extraction naming the model, the duration, memories stored and rejected, and the failure class, so the next session can read what happened. **Landed 2026-09-06 (wave 143):** `memory_extraction_observed` rows — `subject` the trigger, `detail` the model's description with the counts or the failure phrase and the elapsed milliseconds, or *no outcome (preparation failed | the 5 s bound expired)* — one per hook extraction, `NothingToExtract` included; the next session's `sqlite3` query on `evaluation_observations` reads it.

- **The 488 fix has a consequence the packet did not name, found by its verifier:** the hooks above are children of the harness, so once every configured provider's credential is withheld from the harness, an environment-only key stops reaching them; a Keychain key still does. Ruling and the notice that ships with the fix: `docs/product/evidence/phase-9g.md`, 488. The question of policy is parked with the user (`provider-keys-and-helpers`).

Session closed 03:05; the taxonomy test fix landed on `main` as `af63252`.

### 2026-09-06, second session — `74ff128`, claude-code 2.1.263 (Fable 5.1), the night's packages under a real harness

Scratch project `.worktrees/dogfood-20260906b` (detached at `74ff128`), the binary built from `main` and run by path, `glasshouse launch claude-code --response-role interactive` in cmux workspace 149, driven from the orchestrator's pane; session `6bad2a429d5e`. Before the launch the Groq key was moved from the shell into the macOS Keychain (service `glasshouse`, account `GROQ_API_KEY`) exactly as decision 11 and the new notices instruct, and the launching shell carried no provider variable.

**Surprises, each with the exact reading:**

- **The strip holds in a real session.** Task 1, `env | grep -c GROQ_API_KEY` from inside the session: **0** (the first session of the day answered 1); `env | grep -c ANTHROPIC_`: 0. Map line 488 as ticked.

- **The extraction-outcome row landed on the first turn and was decisive.** `evaluation_observations` row 11, `memory_extraction_observed`: *none configured … no configured provider names a model for Glasshouse's own support work — no model was called* — with `[providers.groq]` naming `free_models` and `[entitlements.groq]` present. So the new row (finding 4's successor, `ea5a349`) did its job on its first outing: it said what the first session could not.

- **Decision 11's keychain path does not work as the notices describe it — a Defect, found because the row above said where to look.** `doctor` reads *credential env: GROQ_API_KEY (not set)* with the item in the Keychain, and the candidate set (`routing_classification.rs:500`, `is_present`) agrees. Measured in four steps: (1) an item created by `security add-generic-password -s glasshouse -a GROQ_API_KEY -w` — not set; (2) the same with `-T <the glasshouse binary>` — not set; (3) the same with `-A` (every application) — not set; (4) an item created through the `keyring` crate by a scratch program reads back for that program (`set_password` Ok, `get_password` Ok, 56 bytes) and is still *not set* for Glasshouse. The mechanism: `keyring`'s presence check reads the secret (`credential.rs:74–78`), macOS gates any application that did not create an item behind an *Allow* dialog (the partition list, since Sierra), and Glasshouse forbids that dialog (`SecKeychainSetUserInteractionAllowed(0)`, `secret/native.rs:486`) — so the read errors and reads as absent. A scratch program without the flag simply blocked on the dialog for ten minutes. **Only an item the Glasshouse binary itself creates is readable by Glasshouse, and no command creates one** (`NativeSecretStore::store` has no caller). Successor `GH-CREDENTIALS-STORE-VERB` (Red — secrets): `glasshouse credentials store <VAR>` reading the value from a prompt with echo off or `--stdin`, never argv; `credentials remove <VAR>`; `credentials list` (names and sources only); the three notices and `doctor` name that verb instead of `security …`; `doctor` tells *present but not readable by Glasshouse* from *absent* where the OS error allows. Until it lands, the keychain instruction shipped in `3b9a0b0` is wrong on macOS and a user who follows it loses extraction silently except for the row above. **Landed the same morning (wave 144):** `glasshouse credentials store|remove|list`; `store` refuses a foreign item before prompting and names the one command that removes it; `doctor` tells *present but not readable by Glasshouse* from absent; the demonstration line `GROQ_API_KEY (set in the macOS Keychain, value hidden)` printed by the worker's build and re-proven through `main`'s binary at integration. Two more facts from its report: `security find-generic-password -w` on a Glasshouse-created item *hangs* on the Allow dialog (so no outside tool can read back what Glasshouse stored), and every differently-built binary is a different program to the Keychain — a debug rebuild, or an unsigned release upgrade, must re-store its keys — a debug rebuild included, which is why the third session ran the 07:10 build untouched (Debt: a signing identity for the macOS release binary, the user's certificate; parked as `macos-signing-identity`). A release binary carries an ad-hoc signature, so each upgrade is a new program to the Keychain and `cargo install` produces yet another.


Session closed 06:30; its Display-wording pin test landed on `main` with this record (the rule it wrote: every user-visible `Display` arm gets one `to_string()` assertion against the literal text, singular and plural separately).

### 2026-09-06, third session — `2507deb`, claude-code 2.1.263 (Fable 5.1), decision 11's path proven end to end, and what stood in its way

Scratch project `.worktrees/dogfood-20260906c` (detached at `2507deb`), `main`'s binary run by path (built 07:10, the build that stored the Groq key — a rebuild is a new program to the Keychain), `glasshouse launch claude-code --response-role interactive` in cmux workspace 157, driven from the orchestrator's pane; session `2d559ae4a1cc`. The project config carried `[context_firewall] mode = "shadow"` from the start. Two tasks: the README's command examples checked against `--help` (consistent, no edit), then a *Provider keys* subsection under *Environment* written from `credentials --help` (nine lines, ported to `main` with this record).

**Surprises, each with the exact reading:**

- **Extraction silently called no model, and the outcome row blamed the wrong thing.** After turn 1, `evaluation_observations` row 11 (`memory_extraction_observed`, trigger `task_completed`) read *llama-3.1-8b-instant on groq — free, used by user preference … — no model was called; Glasshouse withholds provider credentials … provider `anyrouter`'s credential … provider `openrouter`'s credential … resolves from neither this hook's environment nor the native secure store …: no extraction model is available; 3 ms*. Groq's key was in the Keychain and `doctor` read it. The cause was none of that: **no `memory_extraction_model` was configured**, on this machine or in any project — `routing_classification.rs :: disposable_extraction_model` decides and records the route without consent but builds no client (step 4, *only with consent*), `memory/extract/disposable.rs` renders that as *no model was called* with no reason, and `noting_withheld_credentials` then appends a credential notice for every provider (`only = None` without consent). The first session's row 16 on 2026-09-06 (*routed at last, and then nothing is recorded*) had the same cause and was read the same wrong way. **Defect (diagnosability)** — `GH-EXTRACTION-CONSENT-NOTICE` (Amber, live): the unconsented row names the missing key and the routed model, and the notice names only the routed provider. Not a product-behaviour defect: consent before any outbound request is the design.

- **Consent given, the Keychain path works — and the model was dead.** With `memory_extraction_model = { provider = "groq", model = "llama-3.1-8b-instant" }` in the scratch project's `.glasshouse/config.toml`, turn 2's row 15 read *asked as groq/llama-3.1-8b-instant via openai-chat: the extraction model failed: the extraction model's endpoint answered 404; check the provider's base URL; 115 ms*. The hook — a child of the harness with the stripped environment — resolved the key from the Keychain item the same binary had stored and made an authenticated request: that is decision 11's path, proven. The 404 is Groq's `model_not_found`: `llama-3.1-8b-instant` no longer exists there (probed with the key: fourteen models, none of the Llama 3.1 family; `openai/gpt-oss-20b` is the small general model now). The machine config's `free_models` and the scratch consent moved to it. **Curiosity, one line:** the row's hint *check the provider's base URL* is wrong for a 404 that carries `model_not_found`; the error class is the provider's and the hint could quote it. Not a package.

- **Two launchers from the earlier sessions were still alive with no terminal.** `glasshouse launch claude-code` processes started 02:02 and 06:08 sat at 0.8 % CPU with fds 0–2 `(revoked)`, each holding a harness child in state `?Es` (exiting, never reaped); their panes had been closed hours before. One `SIGHUP` to each ended launcher, child and shell within four seconds, so the signal path works and the gap is the revoked-without-signal path: `session/attach.rs :: pump_output` ends on a stdout write failure with `break` and stops draining the pty (so the harness blocks on its next write and its exit blocks in the tty drain), and `pump_input`'s `stdin_hung_up()` reports no hangup for a revoked descriptor on macOS, so nothing requests shutdown. **Defect (lifecycle, Red)** — `GH-ATTACH-REVOKED-TERMINAL` (Opus, live). A clean `/exit` in this session ended the launcher at once — the normal path holds.

- **The firewall in `shadow` wrote no `file_touched` row** across a `Read` of `README.md` and an `Edit` of it; `lifecycle_events` holds only `session_started`, `turn_started` ×2, `turn_ended` ×2. The hook was registered (`context-firewall hook --session … --mode shadow` in the harness's settings). Probed after the session with a hand run of `context-firewall hook --session … --mode shadow` and a synthetic `Edit` payload — which `GH-FIREWALL-FILE-TOUCHED`'s worker showed was malformed (no `tool_use_id`, so the event never parsed) and mis-read (the hook *does* log: `GLASSHOUSE_LOG` writes to `<project state dir>/logs/glasshouse.log`, never to stderr unless `--log-stderr`). The worker's own reproduction found and fixed a real dropping gate: `record_file_touches` compares the canonicalized project root against the harness's raw path, so a project under a symlinked directory (`/tmp`, `$TMPDIR` under `/var/folders`) never matches — pinned by `a_writing_tools_post_tool_use_records_one_file_touched_row_for_a_project_path`. **The third session's own missing row is not that gate** (its worktree path had no symlink) and stays open: **next session, after one `Edit`, read the scratch project's `logs/glasshouse.log` for the hook's WARN line** before anything else.

- **The second cmux pane created with `--command` never ran its command** (`cmux workspace create` timed out; the pane existed, no launcher process, `read-screen` failed); the first pane's shell relaunched fine. A driver's note, not Glasshouse's.

- **Third turn, live model: the call completed.** With the consent pointed at `openai/gpt-oss-20b`, row 25 read *asked as groq/openai/gpt-oss-20b via openai-chat: recorded 0, lowered 0, speculative 0, duplicates 0, rejected 0; 422 ms* — the hook's request went out with the Keychain key, the model answered inside the bound, and a three-line documentation fold yielded nothing worth storing. `memories` stayed at 0 for that reason, not for a failure; the next session's task should be one that produces a durable fact (a design decision, an invariant) so the store side is exercised too.

Session closed 08:00 after the third turn; findings packaged by risk above.
