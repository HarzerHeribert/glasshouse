# Capability evidence — phase 21K

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21K — assumption-aware implementation guardrails, 42 of 43 (lines 996–1053); 1044 refused

Package `GH-ASSUMPTION-GUARDRAILS`, 2026-08-31, Fable specialist at xhigh.
**Forty-three lines against one mechanism, built on the Phase 43 shape — over
the existing API door and its MCP twin.** Forty-two closed, one refused. The
ruling it implements is `design-decisions.md`, *"Phase 21K: assumptions are
stated by the agent through the door, never inferred."* Phase 51's RC-D lines
(1838–1844) now have their subject.

Contract: Given an agent working in a Glasshouse session, when it is about to
make a substantial change, Glasshouse asks it — through the door,
harness-independently — for the few critical assumptions the change rests on,
records each as a concise claim with its evidence, source class, uncertainty,
affected scope and cheapest verification, tracks it through proposed → probing
→ supported | refuted | unresolved | waived-by-user, surfaces the set to the
person, notifies when a premise is refuted or a budget is exceeded, and offers
explicit responses — while preserving that trivial edits pass with no gate,
that nothing is inferred from the agent's output or stored from its reasoning,
and that every automatic pause is attributable and overridable.

State: **COMPLETE** for 996–1000, 1004–1009, 1013–1020, 1024–1032, 1036–1043,
1048–1053. **REFUSED** for 1044.

Production evidence:
- `crates/glasshouse/src/guardrails/mod.rs` (new) — the vocabulary (six
  states, six evidence-source classes, seven responses, three modes, four
  blocking categories, three overrides); the deterministic classification
  ladder `classify` (1004; trivial: footprint ≤ 2, reversible, local blast
  radius, no flags, premise not stated as inference; ≥ 8 files is a broad
  refactor whatever it calls itself); the verdict `decide` (1005: trivial never
  gates → per-task override → mode); the ≤ 3-prompt template (1007, 1013); the
  guidance page keyed by capability-map line (997, 1009, 1024–1032, 1038,
  1043); budget review (1037, 1039, 1040); the untrusted-text sanitizer
  (`sanitize` for storage, `quote` for anything rendered into a block an agent
  reads — `memory/inject.rs`'s three rules).
- `guardrails/store.rs` (new) — `task_assumptions` + `assumption_transitions`
  over **migration 19**; current state = latest transition; **no `UPDATE`
  anywhere in production code (source-scanned) and refused by trigger
  (schema)**; project-scoped by migration 15's two triggers; prunable with the
  evaluation ledger's bounds, trimmed in the writer's transaction. Gates,
  overrides and budgets are session-level rows (`assumption_id IS NULL`, `kind ∈
  gate | override | budget_exceeded`) with two table `CHECK`s — 1049 and 1008
  need persistence a stateless preflight cannot give.
- `api/protocol.rs`, `api/unix.rs`, `api/mcp.rs` — **five** `Request` variants
  (`Preflight`, `RecordAssumption`, `UpdateAssumption`, `ListAssumptions`,
  `PromoteAssumption`) and their five MCP tools; `Events` carries a second
  cursor (`assumptions_after`/`assumptions_head`) with `refuted` and
  `budget_exceeded` notifications (1050); `WatchWorker`'s completion line names
  them; `SpawnSession { guardrail }` records the override with origin `agent`
  (a spawn is a program's request — `RequestOrigin`'s attribution boundary),
  and a spawn whose override cannot be recorded is refused rather than started
  without it (1053). Wire names are `affected` and `footprint`, because
  `tests/mcp_project_scope.rs` forbids any tool argument named like `scope`,
  `file` or `path` — the scope-smuggling rule Phase 43 shipped with.
- `config/mod.rs` — `[guardrails] mode = off | advisory | risk_gated` (default
  advisory), `blocking = [...]`, layered project > user > default (1052).
- `cli.rs`, `main.rs` — `glasshouse assumptions [--session <id>] [--limit N]`;
  `--guardrail force|skip|lower` on `launch`/`run` (1008; records
  `waived_by_user` with origin `user`); an `assumptions` section in `glasshouse
  sessions show` bounded to three open premises, the last gate and the override
  in force (1048).
- A substantial preflight for a known session goes through
  `request_checkpoint` and says so in the answer (1036); `off` disables the
  mechanism whole, while `--guardrail skip` waives the gate and not the
  recovery, so it still checkpoints.
- `PromoteAssumption` requires state `supported` (1017's *"until they have been
  supported and accepted"* — promotion is the acceptance); `finding` promotes
  with no authority, `decision`/`constraint` carry their class (1020).

Two bends of the ruling, both accepted by the orchestrator: **four blocking
categories, not three** — `data_integrity` beside security, destructive and
migration, because map line 1052 names *"security, destructive-action, or
data-integrity policies"* itself; `DEFAULT_BLOCKING` stays the ruling's three.
**1031 closed on guidance carried through the door** with its limit stated —
the same reading 1030 and 1032 close on; a fresh-session verifier is spawned,
if at all, through the existing `SpawnSession`.

Regression evidence (`tests/assumption_guardrails.rs`, 7 tests over the shipped
binary — 5 cross-platform through `glasshouse mcp serve`, 2 Unix through
`glasshouse api serve` with the fake harness; `mcp_server` 5, `mcp_project_scope`
4, `api_event_log` 8, `session_context` 18, `worker_wakeup` 9; lib 1599; bin 47):
`a_trivial_edit_passes_with_no_gate_and_a_migration_triggers_a_short_preflight_naming_the_factor`,
`an_assumption_is_recorded_with_its_six_fields_and_never_its_reasoning`,
`transitions_append_and_the_current_state_is_the_latest`,
`the_mode_and_the_per_task_override_decide_the_verdict_and_are_attributable`,
`a_refuted_premise_can_become_a_failed_approach_memory_and_promotion_is_explicit`,
`with_a_harness::a_refutation_reaches_the_watcher_and_the_person`,
`another_projects_server_sees_none_of_it`;
`database::tests::migration_19_adds_the_assumption_tables_and_undoes_cleanly`
(applied to an 18 fixture, undone to a byte-identical schema).

Failure / isolation evidence — 33 mutations (`scripts/mutate.sh`), **31 KILLED
on the first pass and both survivors acted on**: `MAX_PROMPTS` 3 → 5 survived
because the unit test derived its expectation from the constant (§80 case 6) —
both tests now pin the literal three and the door test fires every rung at
once; re-run KILLED twice. The door's own duplicate `waived_by_user`-needs-`user`
check survived because the store's identical check refused first — the door
copy protected nothing and was removed. Every run restored byte-identical;
M03a/M03b verified as behavioural kills (the trigger refused the in-place
`UPDATE`). Named kills include: mode ignored (1052), factor mis-attributed
(1049), trivial gated (1005), guidance line unlabelled (997), scope check
disabled (another project's server sees a planted row), transition `UPDATE`d in
place, override origin forged, promotion without `supported`, budget comparison
inverted.

Adversarial section (three attacks on the boundary, each with the test that
covers it) is in the report: a foreign session id through any assumption tool;
a claim body carrying `[`/control characters aimed at forging a block boundary
when rendered back to an agent; a `scope`/`path`-named argument smuggled onto
a tool.

Gates: fmt, clippy `-D warnings` clean; targets above green; blast radius 69
targets — 66 green, 3 red on schema-version pins in test files outside the
packet (`evaluation_observations`, `memory_provenance`, `memory_store`) fixed
and re-run green. Not run: Linux/Windows legs (no `#[cfg]` added; the two
Unix-only tests are `#[cfg(unix)]` for the fake-harness reason every neighbour's
are). **Windows exercises the migration and the door only through the
cross-platform MCP tests — the same coverage Phase 43 shipped with.**

## 1044 — REFUSED

*"Preserve user changes and unrelated worker changes when rolling back or
isolating an invalidated experiment."* Glasshouse performs no rollback and no
isolation of code; it records the agent's choice (1041) as a transition. The
preservation the line asks for is the agent's — or the version control's —
act, and a guardrail that promised it would be promising something it cannot
observe. The guidance page carries the instruction; the line stays open.

Limits: the failure mode is countered only for agents that call the preflight —
nothing forces the call (1000's harness independence is the reason); guidance
lines are carried in the map's own words and the agent's compliance is not
observable by Glasshouse (Phase 51 RC-D's measurement); budget "materially
exceeded" is any stated axis strictly over its bound, no margin.
