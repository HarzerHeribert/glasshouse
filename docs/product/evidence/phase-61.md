# Capability evidence — phase 61

Phase 61 — pane, the first-party harness (map lines 2419–2481), approved by the user on 2026-09-05. Design of record: the session artifact *The Glasshouse Native Harness*, recorded in `design-decisions.md` (*pane, the first-party harness*). Hand-off draft: `.agent-runtime/pane/phase-61-draft.md`.

Entries are bounded by the *Decompression* ruling: the contract, the tests by name, the mutation on each decision, the limits, and the worker's report by path.

**This phase is built in its own lane.** A team lead owns `crates/pane/**` on branch `pane/integration` and pays review out of its own context; the primary orchestrator owns `main`, this file, and everything outside `crates/pane/`. Reports live under `.agent-runtime/pane/`. Merges are per sub-phase, not per packet — six across the whole build.

**Its gate is not the glasshouse gate.** `pane` is excluded from every `--workspace` invocation in `ci-local.sh` and the GitHub matrix and has its own job; `cargo test -p pane` plus that job is this phase's acceptance. That exclusion is part of the user's decision, not housekeeping: without it all twelve cells and the local gate would compile V8 on every run.

**Two orderings are not negotiable, and a reviewer should refuse a package that breaks either.** 61A comes first, because every claim below it is unfalsifiable without a ruler and the interesting one — that code-mode beats tool-calls on real work — is exactly the kind that feels true and often isn't. **61D comes before any model-authored code executes**; the alternative is a window in which generated code runs beside a keyring with nothing between them.

---

## 61A — The ruler — lines 2430–2432

State: **2431 and 2432 COMPLETE, 2430 PARTIALLY VERIFIED** (2026-09-05, `pane/integration` at `c3fa1d3` merged into `main` as `aa16f6b`; `cargo test -p pane` on the merged tree: 11 + 9 + 11 passed, 0 failed; `cargo tree -p glasshouse -e normal` names no `pane`). Lead's report **`.agent-runtime/pane/report-61A.md`**, review notes `.agent-runtime/pane/review-61a.md`; sub-workers `GH-PANE-61A-SCORE` and `GH-PANE-61A-RUN` (Sonnet high, Amber), reports `.agent-runtime/report-pane-61a-{score,run}.md`.

**2431.** Contract: given a slice of attempts, the ruler reports tokens per *completed* task per workload tier with failed attempts' tokens in their own column, and the aggregate never replaces the per-tier rows. Production `crates/pane/src/ruler/score.rs::Score::of`, `::build_row`; tests `ruler::the_score_excludes_failed_attempts_and_names_the_tier`, `ruler::an_errored_attempt_counts_in_neither_token_column`, `ruler::an_unmetered_completed_attempt_is_excluded_from_the_average_denominator`.

| mutation | change | result | killed by |
|---|---|---|---|
| collapse-every-tier-into-one | `.entry(attempt.tier)` → `.entry(Tier::Leaf)` | **SURVIVED against a single-tier fixture, KILLED after the fixture gained two tiers** | `the_score_excludes_failed_attempts_and_names_the_tier` (ruler.rs:122) |
| fold-failed-attempts-into-the-headline | `filter(\|a\| a.outcome.completed())` → `filter(\|_a\| true)` | KILLED | the same test (ruler.rs:107) and `an_errored_attempt_counts_in_neither_token_column` |
| divide-by-all-completed-not-the-metered-ones | `sum / metered_completed` → `sum / attempts_completed` | KILLED | `an_unmetered_completed_attempt_is_excluded_from_the_average_denominator` |

**2432.** Contract: no tokens-per-turn figure is computed or presented; `turns` is carried and never a divisor. Production `crates/pane/src/ruler/report.rs::{HEADERS, JSONL_KEYS, render_table, render_jsonl}`; tests `ruler::the_rendered_table_header_is_exactly_the_declared_columns`, `ruler::the_rendered_jsonl_keys_are_exactly_the_declared_keys`.

| mutation | change | result | killed by |
|---|---|---|---|
| add-a-per-turn-column-to-the-rendered-table | append `tokens/turn` to the rendered header | **SURVIVED while the tests pinned the constants, KILLED once they read the rendered bytes** | `the_rendered_table_header_is_exactly_the_declared_columns` (ruler.rs:359) |
| smuggle-tokens-per-turn-into-the-rendered-jsonl | one extra key in the JSONL line | KILLED | `the_rendered_jsonl_keys_are_exactly_the_declared_keys` (ruler.rs:396) |

**2430 — PARTIALLY VERIFIED, and the ruling (primary, 2026-09-05, `.agent-runtime/pane/ask-primary-2430-bar.md`).** The runner is built and proven against a fake harness: a fresh worktree per attempt at the task's *parent* commit (`start-at-the-commit-not-the-parent` KILLED by `ruler_run::the_attempt_starts_at_the_parent_commit`), the task's own test commands deciding the outcome, wall-clock, and a meter over `routing-cost --json` pinned key-by-key against the twenty-two-key row (`ruler_run::the_meter_parses_the_readouts_full_twenty_two_key_row`). What the line says has not happened: no run through a live gateway with a real harness, and the second column is empty because `pane` cannot carry a statement before 61C. **Ruled:** the line is not ticked on the gloss; **61C is unblocked by this ruling** — what the ordering protects is a ruler that can falsify claims, and that exists with killed mutations; **no unattended live run before 61D** (the user's own ordering: 61D before any model-authored code executes); **the second baseline is Codex**, wired at 61C with its non-interactive mode declared `Verified` on the adapter; **the live two-column run is 61E's line 2464**, and one honest run there closes 2430 and answers 2464 together.

Lead's packet errors, recorded: the two sub-packets read `Task::test` incompatibly (fixed in `fbb6c71` before the merge); the `session_id` join key the lead proposed and the primary accepted does not exist for a harness the ruler launches directly — `--since` is that join. Limits: `turns` is summed across outcomes with no test pinning the aggregation beyond its absence from any divisor; the 2432 guarantee covers `render_table`/`render_jsonl`, the two renderers `cli.rs` calls.

**The Glasshouse-side producer 2430's meter needs landed 2026-09-05**: `GH-ROUTING-COST-JSON` (Sonnet, Amber), report `.agent-runtime/report-routing-cost-json.md`, answering the lead's ask `.agent-runtime/pane/ask-primary-exchange-readout.md` (its refusal of the SQLite shortcut is on the record there). `glasshouse routing-cost --json [--since <UNIX>] [--session <ID>]` prints JSON Lines — one object per observation from `consumption_in_window`, twenty-two fixed keys in a fixed order (`ObservationJson`, a `Serialize` view struct in `commands/routing_cost.rs` so the order is structural), every absent column `null`, ordered by `observed_at`; `--since` conflicts with `--hours`, both filters require `--json`, so the prose path is untouched. Tests (nine, all driving the shipped binary): `routing_cost::a_row_with_no_tokens_prints_null_never_zero_in_json`, `json_lines_are_ordered_by_observed_at_ascending`, `json_empty_window_prints_nothing_and_exits_zero`, `json_session_filters_to_that_sessions_rows_only`, `json_since_bounds_the_window`, `session_flag_without_json_is_a_clap_usage_error`, `since_flag_without_json_is_a_clap_usage_error`, `since_and_hours_together_is_a_clap_usage_error`, `a_row_planted_under_another_projects_id_never_appears_in_json`.

| mutation | change | result | killed by |
|---|---|---|---|
| absent-as-zero | `input_tokens: observation.input_tokens` → `Some(observation.input_tokens.unwrap_or(0))` | KILLED | `routing_cost::a_row_with_no_tokens_prints_null_never_zero_in_json` — `"input_tokens":0` printed where the build prints `null` |

Limits: closes no line itself; whether the meter parses the shape is 2430's own test on the pane side. The stale "the relay never parses a reply body" sentences in `cli.rs`, `commands/routing_cost.rs` and `tests/routing_cost.rs` were rewritten to the true invariant in the same package.

## 61B — The crate and the adapter — lines 2436–2440

State: **COMPLETE** (2026-09-05) — 2436, 2437 and 2440 with `GH-PANE-KICKOFF` below; 2438 and 2439 with `GH-PANE-ADAPTER` (Sonnet, Amber), report **`.agent-runtime/report-pane-adapter.md`**, integrated in wave 118.

**2438 — the adapter.** Contract: given the `pane` binary, when Glasshouse resolves its adapter, every declaration is `Verified` against the built binary, while `glasshouse` gains no compile-time dependency on `crates/pane`. Production: `harness/pane.rs::Pane`; `harness/mod.rs` (`Vendor::Glasshouse`, `adapter_for`, `structured_pre_tool_hook`); `integrations/mod.rs::IntegrationId::Pane`. Tests: `harness::tests::every_verified_declaration_cites_its_evidence`, `harness::tests::all_lists_exactly_the_harness_adapters`, `harness::tests::every_harness_has_an_adapter_and_nothing_else_does`, `harness::tests::every_supported_harness_can_be_resumed_except_pane`, `harness::pairing::tests::a_vendor_with_no_established_model_line_is_never_native`, `integrations::tests::config_evidence_distinguishes_tools_needing_no_config_from_unknown_harness`; `cargo tree -p glasshouse -e normal` names no `pane`.

| mutation | change | result | killed by |
|---|---|---|---|
| false-declaration | `Pane::resume`'s `None` → `Some(Invocation::of(["--resume", native_session]))` | KILLED | `harness::tests::every_supported_harness_can_be_resumed_except_pane` — `assertion failed: adapter.resume("some-id").is_none()` |

Limits: `ConfigEvidence::Available` for pane is a judgment reasoned like `Cmux`/`LlamaCpp`, not read off the binary. Where the binary has no mechanism at all (hooks, session ids, backends, approvals, communication style) the adapter says `Unverified`, and the ruling is that this is the honest state of a declaration whose mechanism does not exist yet — a `Verified` about 61C would be a lie with a citation.

**2439 — launch over a PTY.** Contract: given `pane` on the searched PATH, when a user runs `glasshouse launch pane`, Glasshouse starts it over a PTY, lists it as a session with harness `pane` and accepts typed input, while nothing about the launch path changes for other harnesses. Production: `harness/pane.rs::Pane::start`, `Pane::executable_candidates`. Test: `pane_launch::pane_launches_over_a_pty_and_is_visible_in_the_session_list` (`#![cfg(unix)]`; builds `pane` itself with a child `cargo build -p pane`, so it is self-sufficient run alone). No mutation of its own: the line adds no decision beyond the adapter's. Limits: Unix only from this box, the GitHub Linux cells are the other leg; one line in, one echoed back, a clean exit — not sustained interactive use.

Scope the packet did not foresee, accepted at integration: `harness/pairing/mod.rs::vendor_organisation`'s exhaustive match needed a `Vendor::Glasshouse => None` arm (E0004 otherwise), and `harness/tests.rs` needed two declaration-table rows and the resume exception — the packet's "scan list line only" was wrong and the worker's `packet_errors` says so.

**The successor named below landed in the same wave.** `GH-BLAST-RADIUS-PACKAGE` (Sonnet, Green), report `.agent-runtime/report-blast-radius-package.md`: every classified target carries its owning package (`pkg:label`), `run_target()` passes it to `-p`, and every `cargo test` child runs under `env -u` of the three provider variables. The glasshouse-only plan is byte-identical before and after (diffed), 28/28 existing script tests unchanged; a bare sweep with `crates/pane/src/main.rs` touched now runs `cargo test -p pane --bin pane`. Limits: the bare mode's `--lib` family split and the `cargo check`/`cargo doc` steps still say `-p glasshouse`, unexercised today because pane has no lib submodule.

`GH-PANE-KICKOFF` (Sonnet, Amber), report **`.agent-runtime/report-pane-kickoff.md`**. `crates/pane` is a member with a library (`echo_line`, two tests) and a binary that reads a line, echoes it and exits 0; `default-members = ["crates/glasshouse"]` keeps a bare `cargo build` unchanged; `cargo tree -p pane` names no dependency at all (2440). All thirteen `--workspace` invocations — eight in `ci-local.sh`, five in `ci-extended.yml` — carry `--exclude pane`, `ci-local.sh`'s macOS lane gains a pane step, and the GitHub matrix gains a two-OS `pane` job (2437). **The mutation is the interesting part:** removing one `--exclude pane` **SURVIVED** — nothing in the repository could observe it — so the worker added `scripts/tests/test_pane_workspace_exclusion.py`, which asserts every `--workspace` in both gate files carries the exclusion, and the identical mutation then **KILLED**. A rule nothing enforces is a comment; now it is enforced.

**One finding outside the packet, with a named successor.** `scripts/blast-radius.sh`'s `run_target()` hardcodes `cargo test -p glasshouse`, so the bare sweep's classified `--bin pane` runs against the wrong package and fails — the workspace had exactly one package for the script's whole life. `--targeted`, the worker's gate, does not hit it. Successor: `GH-BLAST-RADIUS-PACKAGE` (Green) — the classifier and `run_target()` learn which package each target belongs to. Until it lands, the local full sweep is red on that one target and nothing else.

Recorded in the same commit by the orchestrator, in the two CI files this packet held: the Codex npm install pinned to `CATALOGUE_OBSERVED_VERSION` read from source (`.agent-runtime/finding-codex-pin-ci.md`), and `libdbus-1-dev` + `pkg-config` on the Linux cells and in the local Linux container, which `GH-SECRET-SERVICE-BACKEND` needs to build.

Kickoff note: the four Glasshouse-side files this sub-phase touches — the workspace manifest, `harness/mod.rs`, `scripts/ci-local.sh` and `.github/workflows/ci-extended.yml` — change exactly once, in the primary's kickoff commit, and never again. `integrate.sh` refuses any file two trees both touched, and after the kickoff there is no such file.

## 61C — The loop and the three seams — lines 2444–2451

State: **NOT STARTED**.

The question this paragraph used to owe — whether `gateway/translate/canonical.rs` round-trips reasoning blocks byte-identically — **is answered: yes**, since `a4d5911` (`Block::Thinking`/`RedactedThinking`, `anthropic.rs::a_thinking_and_redacted_thinking_block_round_trip_the_anthropic_wire_byte_for_byte`); the lead's finding `.agent-runtime/pane/finding-canonical-thinking-already-landed.md` caught both stale records. What remains open is the *live response* thinking block on a translated route, named in `a4d5911` as `GH-CANONICAL-THINKING-STREAM` — not a 61C blocker, since a native harness relays.

## 61D — The sandbox — lines 2455–2457

State: **NOT STARTED**. Red tier: Opus specialist plus an independent verifier, platform code on three operating systems.

## 61E — Code over live objects — lines 2461–2465

State: **NOT STARTED**. Red tier.

This is the sub-phase the whole phase exists for, and line 2464 is the one that can honestly fail: *a measured win on at least one workload tier by 61A's ruler, or record why not*. Recording why not is a complete outcome of this line, not a failure of it.

## 61F — The supervisor — lines 2469–2471

State: **NOT STARTED**.

## 61G — Events in batches, background work, messages — lines 2475–2481

State: **NOT STARTED**. Amber: the batch window and the interrupt class are decisions; the event bus and direct session messaging (Phases 12, 13) already exist and are what this rides on inside Glasshouse.

Recorded 2026-09-05 (evening) from the ended session `glasshouse-9c`'s hand-off (`.agent-runtime/subpacket-pane-phase-61.md`) on the user's instruction; the sixth spec, `docs/product/pane/events-contract.md`, is what makes these seven lines pass Phase −1 and is dispatched as `GH-PANE-EVENTS`. The reason this sub-phase exists is measured on the orchestrator itself: one Monitor per worker plus three standing watches delivers every event as its own turn, and a harness meant to hold the orchestrator role has to coalesce.

