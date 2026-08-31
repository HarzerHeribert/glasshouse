# Capability evidence — phase 34D

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 34D — router request schema, 11 of 11 (lines 1447–1459), and the classifier on the acting path

Package `GH-LAUNCH-CLASSIFIER`, 2026-08-31, Fable specialist at xhigh.
**Twenty-one lines against one mechanism — classification wired into the
decision `glasshouse launch`/`run` acts on — 18 closed, 3 returned open with the
gate built, mutation-killed at the library level, and the missing producer
named.** This entry holds Phase 34D; the other lines are recorded in their own
phases' entries and cross-reference this one.

Contract: Given automatic routing is enabled and a person starts work with a
stated task (`launch --task` / `run --task`), when Glasshouse decides where the
work goes, it classifies the task through a small structured router request —
the bounded task text, minimal session metadata, one capacity band per
candidate provider, and the person's stated constraints; never a file, a
transcript, an environment value or a credential — turns the structured answer
into requirements the destination ranking honours, and measures the latency
the decision added — while preserving today's behaviour byte for byte when no
task is stated, and falling back to the deterministic heuristic whenever the
routing model is unconfigured, unavailable, or low-confidence.

State: **COMPLETE** for 1447, 1448, 1449, 1450, 1451, 1454, 1455, 1456, 1457,
1458, 1459.

Production evidence:
- `crates/glasshouse/src/routing/request.rs` (new) — `RouterRequest`: the task
  (bounded to `TASK_TEXT_CEILING_BYTES = 2048` with a visible
  `TRUNCATION_MARKER`), the `RoutingMoment`, `WarmSessionFact::among` (whether a
  relevant warm session exists among the candidates), one `ProviderBand` per
  candidate provider (a `CapacityBand` word, never a reading), and
  `UserConstraints` (the harness the person named, `--to`, `--fresh`, providers
  with `enabled = false`). 1451 and 1454 come from `classify_heuristically`'s own
  signal fields (`expects_code_modification`, `expects_long_running`). **The type
  has no field that could carry a file, a transcript, an environment value or a
  credential** — 1455/1456/1426 are structural — and `render()` is bounded by
  `REQUEST_CEILING_BYTES = 6144` by construction (capped lists, clipped names).
  `RouterAnswer` wraps a `TaskClassification` with an `AnswerProvenance` (model /
  heuristic-with-reason / reused): `required_tier()` is the **conservative**
  tier, `expected_duration()` and `execution_shape()` apply 1459, `task_class()`
  derives from the signal fields the way `hard_capabilities()` already does;
  `requirements()` is what the session router is handed; `explain()` names the
  escalation when confidence was low.
- `crates/glasshouse/src/routing/classify.rs` — `DurationClass`, `ExecutionShape`
  (`ReuseSession | NewSession | DisposableJob`), two *optional* fields on
  `TaskClassification` (builders, not constructor arguments), `is_low_risk()`, a
  **lenient** parse of the two new schema keys (missing or unknown reads as "not
  stated", never a failure), and `CLASSIFICATION_RESPONSE_SCHEMA`'s new closing
  section, *"The routing request"*.
- `crates/glasshouse/src/main.rs` — `launch`/`run --task`; `classify_for_routing`
  called in `launch_session` after `observed_provider_health` and before
  `RouterInputs` (no task → `None` → `TaskRequirements::default()`, no request, no
  model, no ledger row, the same explanation text as before); `route --task`
  calls the **same** function, so the diagnostic and the decision cannot
  disagree; `classify_with_routing_model` takes `&RouterRequest` and sends
  `request.render()` through `Prompt::for_request`; `glasshouse classify` sends
  `RouterRequest::for_text(text)` with every session fact honestly absent and
  prints the two recommendations, marked *derived* when the model stated none.

Regression evidence (`tests/launch_classification.rs`, 16 tests, through the
shipped binary against a loopback endpoint; `route_command` 36;
`classification_call` 10; `routing::` lib 130):
- `a_launch_without_a_task_routes_exactly_as_before_and_calls_no_model` — a
  canned endpoint fails the test if it is hit.
- `a_stated_task_classifies_through_the_routing_model_and_the_request_carries_bands_not_numbers`
  — the body on the wire names the task, `plenty` (never `947`/`1213`), `warm
  session yes` on the second launch, `harness claude-code, named by the user`,
  `code modification: yes`, `long-running multi-turn: yes`.
- `the_router_request_never_carries_repository_contents_transcripts_or_secrets`
  — a planted repository file, a planted harness transcript
  (`.harness/rollout.jsonl`, named by the hook's `transcript_path`), a planted
  environment value, a memory body and a provider credential are each absent
  from the wire.
- `low_confidence_routes_on_the_conservative_tier_and_says_so`;
  `parse_of_an_answer_missing_new_fields_yields_the_conservative_default`;
  `an_explicit_destination_is_deterministic_and_asks_no_model`;
  `with_no_routing_model_configured_the_heuristic_routes_end_to_end`;
  `classification_call::the_classify_command_sends_the_structured_router_request`;
  `request::tests::a_four_kilobyte_task_does_not_reach_the_model_whole` (a
  literal, not the constant).

Failure / isolation evidence — 24 mutations run, 23 KILLED, one SURVIVED and
honestly explained (`scripts/mutate.sh`, each restored byte-identical; `killed
at` is the test's own assertion line):
- 1447 send-raw-text (`&request.render()` → `request.task_text()`) — KILLED,
  `body.contains("route-probe   plenty")`. 1448 drop-warm-fact — KILLED. 1449
  drop-band — KILLED. 1450 drop-pinned-harness — KILLED. 1451/1454 constant
  expectation — KILLED.
- 1425 raise-ceiling (`TASK_TEXT_CEILING_BYTES` → 1 000 000) — KILLED by the
  literal-bracketed test.
- 1455 leak-repository-file (task text += `read_to_string(planted.rs)`) — KILLED:
  repository sentinel on the wire. 1426 leak-environment — KILLED. 1456
  leak-transcript-file — KILLED: transcript sentinel on the wire.
- **1456 leak-transcript-events — SURVIVED, §80 case 3, and it is the honest
  result**: leaking the session's *events* into the request leaked no transcript
  because Glasshouse persists none — `events/log.rs:89-93` stores `Observation {
  harness, event }` only, and `log.rs:34` says the hook payload's prompt is
  drained, never stored. The only transcript a router could send is the
  harness's own file, which the test now plants and the second mutation kills.
- 1457 mislabel-duration, 1458 mislabel-shape — KILLED by the parse test. 1459
  skip-escalation (`workload_tier()` for `conservative_workload_tier()`) —
  KILLED: `tier heavy (conservative: …)`.

Rulings:
- **1458 — CLOSED on the line's own wording**, which the worker asked to have
  ruled. *"Allow the router output to **recommend** reuse-session, new-session,
  or disposable-job as an execution shape"* is a statement about the output
  schema: the shape is asked for, parsed, withdrawn when unsafe, and rendered
  in the explanation. Acting on it — reuse versus new — is Phase 35/37's
  ranking over candidate facts a request-text classifier cannot see, and those
  lines are evidenced separately. A recommendation that reaches the person is
  the consumer this line names.
- **Latency (1849) is measured decision-start → decision-end**, not → spawn, and
  the worker's reason is accepted: between the decision and the spawn sit
  profile resolution, the pre-flight, the gateway start and the evidence
  ledger's own handle — all of which happen identically with no task, so they
  are the launch's cost and not what routing *added*; recording at spawn would
  also open a second `EvidenceLedger` beside the gateway's (§65).

Limits, stated by the worker:
- Proven against a loopback endpoint; a real provider's tokenisation of the
  request is not measured. Bands reach the wire for direct-provider
  destinations; a native destination's capacity lands as `unknown` unless one is
  planted. `forbidden providers` has a producer and a constructor-level test; no
  binary-level test plants a disabled provider. The task-class and the two
  expectations are the heuristic's; the model may disagree and its answer wins.
- macOS only; the Windows leg has not run.
- The sticky record (`routing-classification.json` under `project_state_dir`) is
  a new file nothing prunes; `reuse_for`'s conditions bound its effect.

Packet errors the worker recorded, all accepted: the launch path never prints
`Routed::render()` (it prints the continuation line and an override refusal —
so the classification note is a zero-weight `Contribution` on `route --task` and
one stderr line on launch); `Destination::with_resource_facts` has **no
production caller** and `ResourceFacts` has no tier field, so there was no
carrier to read a ceiling from (see 1516 in `phase-35a.md`); 1517 is
*deliberately* additive by ruling 4's own test
(`an_unverified_axis_scores_strictly_better_than_an_established_absent_one`) and
no adapter declares any axis `Verified { value: false }`.


---

## From `GH-LAUNCH-CLASSIFIER` (2026-08-31) — lines outside Phase 34D; the mechanism is in `phase-34d.md`

### Phase 34E — 1467, 1468, 1470, 1471 CLOSED

- **1467** ☑ — `StickyClassification` (`routing-classification.json` under
  `project_state_dir`, write-temp-then-rename, every read failure reads as
  absent), written by `main.rs::remember_classification` once the session the
  work landed on has an id, consulted through `reuse_for`: the previous answer
  stands iff it was low-risk (`TaskClassification::is_low_risk`), the
  `RoutingFingerprint` is unchanged, and the sticky session is still offered and
  idle ≤ `STICKY_TURN_WINDOW_SECONDS` (30 min, a named policy constant). Only an
  answer a model actually gave is remembered; a reused answer is attributed
  `AnswerProvenance::Reused`. Killed four ways: never-reuse, reuse-anything,
  **sever-cache-read at the launch site** (§35, the call), sever-cache-write —
  each by `repeated_low_risk_turns_in_the_same_sticky_session_bypass_the_routing_model`
  (`requests.len() == 1`) or `a_classification_that_is_not_low_risk_is_asked_again`.
  Limit: a "turn" here is a Glasshouse-visible task start (`launch --task`);
  in-harness turns never reach this router.
- **1468** ☑ — `RoutingFingerprint` (harness, bands, observed-health labels); a
  health reading appearing re-asks the model. Killed: ignore-conditions →
  `a_material_change_in_resource_conditions_re_asks_the_model` (`requests.len() == 2`).
  Explicit migration (`--to`/`--fresh`) never reuses (1470), which is this line's
  *requests migration* clause by construction.
- **1470** ☑ — `UserConstraints::is_deterministic()` (`--to`/`--fresh`) →
  heuristics classify for the explanation and no model is asked. Killed:
  `an_explicit_destination_is_deterministic_and_asks_no_model` (`requests.is_empty()`).
  Limit: `--profile` and `--from-checkpoint` build router overrides but are not
  treated as deterministic for classification.
- **1471** ☑ — `RoutingModelResolution::Heuristics` → the heuristic answer routes
  end to end, every downstream term working on it. Killed: the `Heuristics` arm
  returning `None` → `with_no_routing_model_configured_the_heuristic_routes_end_to_end`
  (*classified by deterministic heuristics*). Limit: the heuristic path's tier
  gate is inert for 1516's reason.

### Phase 35A — 1516, 1517 OPEN, the gate built

- **1516** ☐ **open — mechanism built, no production producer of a ceiling.**
  `TaskRequirements.minimum_tier: Option<WorkloadTier>`,
  `Destination::with_tier_ceiling`, `HardConstraint::WorkloadTier { required,
  offered }` with a rendered `reason()`, the gate in `hard_constraint` firing
  only on an *established* ceiling strictly below the requirement. Killed at the
  library level (`a_destination_below_the_required_tier_is_excluded_with_a_readable_reason`)
  — **library-level by necessity**: nothing in config, the provider registry,
  the harness adapters or `ResourceFacts` states a resource's tier, so
  `tier_ceiling` is `None` on every path the binary builds and the gate is inert
  there. **Successor**: a producer — Phase 49's line 1796 (*"configure
  workload-tier ceilings for individual models"*) is the natural one — plus one
  `with_tier_ceiling` call in `routing_destinations`.
- **1517** ☐ **open — deliberately additive, and no producer.** `session.rs`
  (pre-package) records ruling 4's one rejecting exception as unwired, and
  `routing_capability.rs::an_unverified_axis_scores_strictly_better_than_an_established_absent_one`
  pins an established-absent destination as *eligible and lower-scored*. Wiring
  the exclusion would fail that test and has no producer: no harness adapter
  declares any axis `Verified { value: false }`.

### Phase 35B — 1531 OPEN, the term built

- **1531** ☐ **open** — `workload_tier_fit` (exact 0.4 > headroom 0.2 >
  not-established 0.0), pushed into `score()` only when a tier is stated; the
  discriminating pair killed at the library level (`workload_tier_fit_decides_between_two_otherwise_equal_destinations`).
  Same missing producer as 1516; on the binary's path the term contributes 0.0
  with *"nothing has established … ceiling — not a no"*.

### Phase 51 — 1849 CLOSED

- **1849** ☑ — `main.rs::record_routing_latency` writes one `routing_observations`
  row per classified launch (`provider = "glasshouse"`, `model =
  "session-router"`, `harness`, `purpose = "routing-latency"`, both timing
  columns), opened, written and dropped at the end of the routing decision, and
  **none** when no classification ran. Measured decision-start → decision-end
  (ruling in `phase-34d.md`). Killed: skip-record →
  `routing_latency_is_recorded_only_when_classification_ran`
  (`routing_latency_rows() == 1`). Limits: the ledger's timing columns are unix
  seconds (migration 11), so a sub-second decision reads back as `0` through
  `duration_ms()` — the millisecond figure goes to the log; the shell's
  `routing_latency_phrase` was deliberately not fed (it reads the pinned model's
  identity, not this row's — not a one-line change).

### Phase 34B — 1425, 1426 CLOSED

- **1425** ☑ — `TASK_TEXT_CEILING_BYTES = 2048` (visible `TRUNCATION_MARKER`) and
  `REQUEST_CEILING_BYTES = 6144` by construction; the request carries no
  repository history by type. Killed: raise-ceiling → the literal-bracketed
  `a_four_kilobyte_task_does_not_reach_the_model_whole`. Limit: the ceiling is a
  chosen number; nothing measures what a routing model needs.
- **1426** ☑ — structural (no field for a secret, a memory body or a history) and
  proven on the wire: planted environment value, memory body and provider
  credential absent. Killed: leak-environment. Limit: memory bodies are asserted
  absent but not mutated (no one-line reader exists).
