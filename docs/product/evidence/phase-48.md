# Phase 48 — CLI ergonomics

Eight boxes, `docs/product/capability-map.md` lines 1775–1782. An audit
package before a building package: six of eight were already real; one
(`status`) was genuinely absent and built this round; two are a naming
question, argued below and left unrenamed.

## Pass 1 — audit

| Line | Status before this package | Symbol / file |
|---|---|---|
| 1775 | already satisfied, undertested at the dispatch level | `Command::None` → `glasshouse::shell::run`, `main.rs:290–293` |
| 1776 | satisfied under a different name | `Command::Sessions` (bare), `main.rs:143–146` → `session_report` |
| 1777 | satisfied under a different name | `Command::Launch` / `Command::Run`, `main.rs:183–218` → `launch_session` |
| 1778 | already satisfied, undertested at the dispatch level | `Command::Memory{Search}`, `main.rs:229–238` → `memory_report` |
| 1779 | absent | built this round: `Command::Status`, `main.rs:82–84` → `status_report` |
| 1780 | already satisfied, undertested at the dispatch level | `Command::Doctor`, `main.rs:85–87` → `integrations::doctor_report` |
| 1781 | already satisfied | every project-scoped command reads `runtime.project()`/`runtime.state_dir()`; `Shim` is the one command that does not, and is the map's own named exception — see below |
| 1782 | already satisfied | `cli.rs` declares no `init` subcommand; `bootstrap` resolves the project root from Git discovery on every invocation, argued below |

**The dispatch-level gap, found by this audit.** Doctor and memory search
each had a production caller in `main.rs`'s match — line 1780 and 1778 both
"looked satisfied" on that basis alone, which is what the orchestrator's own
`--help` read concluded. But neither had a test that entered through it:
`doctor_report_includes_project_identity_and_never_panics`
(`integrations/mod.rs`) calls `doctor_report` directly, and every test in
`tests/memory_search.rs` calls `memory::search` in-process. This is §35's
shape exactly — "a caller every test bypasses is not a caller" — proved by
mutation below, not assumed. Both are closed properly now: a new binary-level
test each in `tests/session_model.rs`, mutation-confirmed against the
`main.rs` dispatch arm.

## Pass 2/3 — what this package did

### 1779 — built: `glasshouse status`

`Command::Status` (`cli.rs`) → `status_report` (`main.rs`). One screenful:
project identity, harness usable/total count (`Discovery::run`,
`DetectedIntegration::is_usable`), session count and the most recently
active session's identifier/state/age (`ProjectSessions`, the same
`disposition_word`/`format_age`/`short_id` helpers `sessions` uses), and a
count of resources Glasshouse can describe
(`glasshouse::provider::registry::registry`). Deliberately a composition of
counts, not a re-render of any of `doctor`/`sessions`/`resources`' own
detail — each points a reader at the fuller command for depth, per the
packet's "keep it small."

Contract: Given a project with N recorded sessions and a set of installed
harnesses, when `glasshouse status` runs, Glasshouse prints the project's
identity, how many harnesses are usable, how many sessions are recorded and
which was active most recently, and how many resources it can describe,
without making a probe request or re-deriving `doctor`/`resources`'
rendering.

State: **COMPLETE** — promoted by the orchestrator on real Windows execution.
`status_reports_a_launched_sessions_count_and_the_project_identity` ran and passed
on the ARM64 VM (`--windows-vm`, batch 33), alongside the Linux container leg. The
entry below was written before either had run; both have now. (Original note: macOS/Darwin; not run on Linux or Windows, and this
project's CI is unavailable until September — §27).

Production evidence:
- `cli.rs`: `Command::Status` variant
- `main.rs:82–84`: the dispatch arm
- `main.rs`: `status_report` — the only caller of
  `glasshouse::integrations::Discovery::run`, `ProjectSessions::open`, and
  `glasshouse::provider::registry::registry` in this function's body

Regression evidence:
- `status_reports_a_launched_sessions_count_and_the_project_identity`
  (`tests/session_model.rs`) — runs the real binary against a real project,
  asserts "none recorded" before a launch and "1 recorded" with the launched
  session's identifier present after one. Mutation-confirmed: deleting the
  `Command::Status` dispatch arm's body fails this test
  (`FAILED ... a fresh project should report no recorded sessions`),
  restored and green afterward.

Missing evidence:
- No Linux or Windows run this round.

### 1780 — closed further: `glasshouse doctor`'s dispatch arm

Contract: Given the project resolved from the working directory, when
`glasshouse doctor` runs, Glasshouse prints that project's own identity and
harness/integration state — not a report for some other project.

State: **COMPLETE** — promoted on real Windows execution (`--windows-vm`, batch 33).

Production evidence:
- `main.rs:85–87`: `Some(Command::Doctor) => print!("{}",
  glasshouse::integrations::doctor_report(&runtime))`

Regression evidence:
- `doctor_dispatches_through_the_command_and_reports_this_project`
  (`tests/session_model.rs`, new) — runs `glasshouse doctor` as a subprocess
  and asserts the report names this fixture's own project root. Mutation-
  confirmed: emptying the dispatch arm's body fails this test, restored and
  green afterward.
- `doctor_report_includes_project_identity_and_never_panics`
  (`integrations/mod.rs`, pre-existing) — proves the report's own content,
  in-process; kept as the content-level test, now backed by a dispatch-level
  one instead of standing alone as the box's only evidence.

### 1778 — closed further: `glasshouse memory search`'s dispatch arm

Contract: Given free-form query text on the command line, when `glasshouse
memory search <query>` runs, Glasshouse reaches `memory_report` and prints
its answer — not a query language, matched against subject and body,
current-scope by default.

State: **COMPLETE** — promoted on real Windows execution (`--windows-vm`, batch 33).

Production evidence:
- `main.rs:229–238`: `MemoryCommand::Search { .. } => print!("{}",
  memory_report(&runtime, ...))`

Regression evidence:
- `memory_search_dispatches_through_the_command` (`tests/session_model.rs`,
  new) — runs `glasshouse memory search <term>` as a subprocess and asserts
  the no-match report text that only `memory_report` produces. Mutation-
  confirmed: emptying the `MemoryCommand::Search` arm's body fails this
  test, restored and green afterward.
- `tests/memory_search.rs` (pre-existing, extensive) — proves
  `memory::search`'s own ranking/scope/authority behavior, in-process; kept
  as the content-level suite, now backed by a dispatch-level test for the
  box itself.

Missing evidence:
- Neither test seeds a real stored memory and asserts it comes back through
  the CLI end to end; the new test only proves the dispatch reaches
  `memory_report`. `tests/memory_search.rs`'s in-process coverage of ranking
  and scope is not duplicated here, deliberately — see the packet's "keep
  it small."

### 1775 — closed further: the bare-TUI dispatch, mutation-confirmed

Contract: Given a real terminal on both stdin and stdout and no subcommand,
`glasshouse` opens the interactive shell for the current project; without
one, it prints the plain summary instead.

State: **COMPLETE** — promoted by the orchestrator. The bare-TUI dispatch is proven
through a **real terminal on Windows**: `pty_smoke::the_shell_opens_in_a_real_terminal_and_answers_the_keyboard`
ran and passed on the ARM64 VM. (Original note: not run in this package with a real
terminal via the Bash tool, which cannot supply one (§38) — instead, the
existing pty-backed production test was used and mutation-confirmed.

Production evidence:
- `main.rs:290–293`: `if std::io::stdin().is_terminal() &&
  std::io::stdout().is_terminal() { glasshouse::shell::run(&runtime)?; ...
  }` inside the `Command::None` arm.

Regression evidence:
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard`
  (`tests/pty_smoke.rs`, pre-existing — not owned by this packet, run but
  not edited): spawns the shipped binary with no subcommand inside a real
  pseudo-terminal and drives it with real keystrokes. Mutation-confirmed
  this round: gating the `is_terminal()` branch with `if false && ...` so
  the bare command always falls through to the plain summary fails this
  test (`send_text: ... Input/output error`, because the shell never opens
  to receive the keystroke); restored and green afterward.

### 1776 / 1777 — the naming question, argued

State: **COMPLETE** — both capabilities exist in the shipped binary under names the
map does not use (`glasshouse sessions`, `glasshouse launch`/`run`). Closed on
practice §33's criterion by the orchestrator: *ask the capability as a question a
user would ask.* "Can I get a non-interactive session summary from the shell?" and
"Can I start a project session from the shell?" are both answered yes. **The map's
spelling and the binary's differ, so `glasshouse session list` is an error** — that
is a map rewording and the user's call, not a defect to fix by renaming a command
`glasshouse shim` generates callers into.

The map asks for `glasshouse session list` and `glasshouse session new
<harness>`. The binary has `glasshouse sessions` (bare, or with `show`/
`rename`/`tag`/`close`) and `glasshouse launch <harness>` / `glasshouse run
<harness>`.

**Asked as a user would ask it** (practice §33): *Can I get a
non-interactive project-session summary from the shell?* — yes,
`glasshouse sessions`, already production-tested at the dispatch level
(`a_launched_session_records_seven_facts_and_the_binary_shows_them_apart`
and others in `tests/session_model.rs`, none of them new this round). *Can I
start a project session from the shell?* — yes, `glasshouse launch
<harness>` or `glasshouse run <harness>`, exhaustively tested. Both honest
answers are yes.

**The argument for renaming or aliasing:** the map's literal spelling
implies one `session` namespace with `list`/`new`/(`show`/`rename`/...)
underneath it, which would read as more coherent than two unrelated verbs
(`sessions`, `launch`) for two related ideas.

**The argument against, and what this package found while checking it:** a
single `session` alias cannot serve both lines at once. Aliasing the
existing `Sessions` command to `session` (clap `alias`) would make
`glasshouse session` list — but `glasshouse session new` would still have
nothing under it, because starting a session takes `launch`'s whole flag
surface (`--profile`, `--response-profile`, `--response-role`,
`--from-checkpoint`, `--headless`, trailing harness args), which is not a
small addition to fold under a second command name without duplicating or
re-routing that surface. A `session` command that lists but cannot start
would be a worse, half-built product surface than no alias at all — the
literal wording implies a coherence this architecture does not have
without a genuinely new command, which is outside a Sonnet implementer's
scope to decide unprompted (`worker-capabilities.md`: "Decide ambiguous
product behavior" is a Do-not).

**Decision: both lines stay closed under their existing names.** `run`
exists under its exact name because a generated shim execs into it, and
`sessions` is the name every existing test and printed identifier already
assumes (per the packet's own warning against a silent rename). No CLI
change was made for either line. If the orchestrator judges the coherent
`session` namespace worth building, it is new command surface — Amber-tier
CLI/TUI work per `worker-capabilities.md`'s risk routing, not a rename —
and belongs in its own packet rather than folded into an eight-line audit.

### 1781 — audited, no change

State: **COMPLETE**

Every project-scoped command reads `runtime.project()` and/or
`runtime.state_dir()` inside its `main.rs` arm. The one exception is
`Command::Shim`: `run_shim(harness, profile, dir, name, force)` takes no
`&runtime` argument and writes to an arbitrary `--dir` the caller names —
its doc comment says as much ("Writes exactly one file to `--dir`, which is
required: there is no default system-wide location"). This is the map's own
carve-out ("unless an explicitly administrative command is clearly
global") rather than a gap: a shim generator has no project to be scoped
to. `bootstrap` still resolves a project root as a side effect of `run()`
for every invocation including `shim`, but `run_shim` never reads it — worth
naming for the orchestrator, not a defect this packet found reason to
change.

### 1782 — audited, no change

State: **COMPLETE**

`cli.rs`'s `Command` enum declares no `init`/`initialize` variant, and
`AFTER_HELP` states the root-resolution rule directly ("The root is the
containing Git repository when there is one, otherwise the current
directory"). `bootstrap` resolves the project root by Git discovery on
every invocation, unconditionally — every test in this project's suite,
including every one added this round, creates a bare `.git` directory and
runs commands against it with no initialization step, which is the running
proof rather than a single named one.

## Mutation ledger

| Box | Test | Mutation | Result |
|---|---|---|---|
| 1779 | `status_reports_a_launched_sessions_count_and_the_project_identity` | emptied `Command::Status` dispatch arm | FAILED, restored → ok |
| 1780 | `doctor_dispatches_through_the_command_and_reports_this_project` | emptied `Command::Doctor` dispatch arm | FAILED, restored → ok |
| 1778 | `memory_search_dispatches_through_the_command` | emptied `MemoryCommand::Search` arm body | FAILED, restored → ok |
| 1775 | `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` (pty_smoke.rs, not owned) | `if false && ...` around the `is_terminal()` branch | FAILED, restored → ok |

## Gate

`cargo build -p glasshouse`, `cargo clippy -p glasshouse --all-targets`,
`cargo doc -p glasshouse --no-deps`, `cargo test -p glasshouse --lib
cli::`, `cargo test -p glasshouse --test session_model` (19/19) — all clean
on this machine (macOS/Darwin), private `CARGO_TARGET_DIR`, run one at a
time (§40). `rustfmt --edition 2024` run on the three owned files touched
(`cli.rs`, `main.rs`, `tests/session_model.rs`) — this workspace is edition
2024. `cargo fmt --all` was not run (§37, not this packet's to run).
`ci-local.sh` was not run — that is the pre-commit gate, and this packet
does not commit.

## What this packet got wrong

The packet's own framing ("over half of these eight are already
satisfied") undercounted: six of eight were already real, not "over half."
It also did not anticipate the dispatch-level gap on 1778/1780 — the
`--help` read that motivated the packet is real evidence a command exists,
but not evidence anything would notice it breaking, which is what closing a
box "properly" (Pass 2's own word) turned out to require.
