# Capability evidence — phase 35

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 35 — the classifier now exists, and nothing calls it yet

**Audit first (Pass 1).** Nothing in the shipped binary classified a request
along any of the map's fourteen dimensions before this batch.
`routing::disposable::JobKind::Classification` (Phase 9I) is only a *label*
a disposable job can carry — grepped across the crate, it is constructed by
tests and by `RoutedNoModel`'s own fixtures, never by `main.rs`, so the "job
before spending premium agent capacity" the map's preamble describes has no
job to name yet. `config::RoutingModelChoice` /
`RoutingModelResolution` (Phase 2C, `crates/glasshouse/src/config/mod.rs`,
read-only for this package) record *which model, if any, would classify a
request* and resolve that against configured providers — genuinely adjacent,
and cited below per box — but that machinery answers "who classifies," never
"what did the request need," which is this phase's actual content. Nothing
else in the crate (`harness::pairing::classify`, `memory::store::Classifier`,
`launch::classify_launch_kind`) classifies a *request*; each classifies
something else entirely (a model/harness pairing, a memory's authority, a
launch script's file type) and the name is the only thing they share.
**Absent**, for all fourteen lines, before this batch.

**Built (Pass 3, smallest first).**
`crates/glasshouse/src/routing/classify.rs` (new, 717 lines including tests)
adds `TaskClassification` — the eleven-field, `Copy`-heavy struct that
answers every classification box directly — plus `classify_heuristically`
(a pure, deterministic keyword-matching function: the required "fall back to
deterministic heuristics when no cheap model is available") and `classify`
(prefers an already-produced model answer, falls through to the heuristic
otherwise — the "run on a cheap, free, or local model" half, kept lightweight
by never calling one itself; see the module's own doc comment for why a
network call would disqualify it from the box). `WorkloadTier::escalate`,
`TaskClassification::conservative_workload_tier`, and
`::conservative_safe_for_disposable_model` are the confidence-escalation
mechanism, applied only at `Confidence::Low`.

**Nothing calls it (Pass 1 finding restated as the reason Pass 2 could not
run).** The map's own line for the phase that would consume this — Phase
34F's model-capability calibration and Phase 35B's candidate generation — are
both at zero, exactly as the packet said going in. What the packet did not
settle in advance is that **this package cannot add a caller even for the
mechanism it itself built**: a production caller would have to sit on the
request-handling path, in `main.rs`, `cli.rs`, or `shell/**`, and all three
are outside `YOURS` (the last two are the shipped binary's only entry points;
`shell/**` is a live worker's file this round per the packet's own forbidden
list). `routing::disposable::DisposableRouting::choose` — the one function in
this package's own files with a real production caller
(`memory::extract::disposable::route`, itself called from `main.rs`) — could
not be extended to consume a `TaskClassification` without changing its
signature, which would break that caller's file, itself outside `YOURS`.
There is no door in this package's own file set that reaches the shipped
binary. Per practice §5 and §35 — a mechanism with no production caller does
not get its box, and a caller only tests can reach is not a caller — **every
box stays open**, named against Phase 35B (the router) and, secondarily,
against whichever future package is allowed to touch `main.rs`/`cli.rs` to
wire a manual entry point the way Phase 21's `--reply-from` did for manual
memory extraction (practice §33).

One exception, argued explicitly rather than assumed: **line 1502 ("keep
classification output structured and small") is a property of the type, not
of a caller**, the same way Phase 32's evidence closed "secrets are never
plaintext" on the type's own invariant rather than on a caller reaching it.
`tests::the_classification_stays_small` pins `size_of::<TaskClassification>()
<= 64` bytes directly against the type; nothing about that claim needs the
shipped binary to exercise it, because there is no "wrong behind a bypassed
caller" failure mode for a struct's own shape. **CLOSED** on that basis; see
the per-line disposition for the counter-argument considered and rejected.

State: **PARTIALLY VERIFIED** — one line (1502) closed on a type-level
argument; the remaining thirteen mechanism-level lines are **OPEN**,
consuming-phase named, mechanism built and independently tested.

Production evidence:
- None. See the "nothing calls it" section above for why, and what would
  close this.

Regression evidence (all reachable only through tests — noted, not hidden):
- `routing::classify::tests::*` (12 tests, `crates/glasshouse/src/routing/classify.rs`)
  — the author's own unit tests: field-by-field behaviour for a shell
  request, a browser request, a generic question, a file-referencing
  question, an ambiguous/empty request, escalation at `Confidence::Low`,
  non-escalation at `Confidence::Medium`, `classify`'s model-preferred /
  heuristic-fallback behaviour, and the size bound for line 1502.
- `classification::*` (4 tests, `crates/glasshouse/tests/routing_policy.rs`)
  — independent, outside-in tests against the module's own doc-comment
  claims rather than its unit tests (same split this file's header already
  states for the two routing-policy modules): `classify_heuristically` is
  deterministic on five representative inputs; the module makes no network
  call (same source-scan idiom as
  `no_routing_policy_can_make_a_request`, extended to `classify.rs`); an
  empty request degrades to `Confidence::Low` rather than panicking or
  guessing confidently; `classify()` returns a supplied model answer
  unmodified even when it actively disagrees with what the heuristic alone
  would say, proving the two paths are not blended.

Mutation evidence (practice §41 — production logic, since there is no
production *call site* per §35 to mutate instead):
- `WorkloadTier::escalate` mutated to the identity function: `ok` before,
  `FAILED` at `an_ambiguous_request_gets_low_confidence_and_escalates` and
  `workload_tier_escalation_never_goes_past_heavy`, `ok` after restore.
- `conservative_safe_for_disposable_model` mutated to return the raw field
  unconditionally: `ok` before, `FAILED` at
  `low_confidence_withdraws_disposable_safety_even_when_the_raw_fields_say_safe`,
  `ok` after restore.
- `needs_repo_context`'s `references_repo` term deleted (so a question naming
  a repository file would read as a generic question): `ok` before, `FAILED`
  at `a_question_about_a_named_file_still_needs_repo_context`, `ok` after
  restore.
- `classify`'s body replaced with an unconditional call to
  `classify_heuristically`, discarding `model_output`: `ok` before, `FAILED`
  at `classify_returns_the_model_answer_unmodified_even_when_it_disagrees_with_the_heuristic`
  (`tests/routing_policy.rs`, the outside-in test, not the unit test) — the
  §36 form of the check, since no production call site exists to mutate.
  `ok` after restore.

Platform/external evidence:
- `cargo test -p glasshouse` (macOS, this worktree, run alone per §40): every
  lib and integration target passed, 0 failed — 1150 lib tests and 22 in
  `routing_policy` among them (see the exact `--test routing_policy` run
  below).
- `cargo test -p glasshouse --test routing_policy`: 22 passed, 0 failed (18
  pre-existing + the new `classification` module's 4).
- `cargo clippy -p glasshouse --all-targets -- -D warnings`: clean.
- `cargo fmt -p glasshouse -- --check`: clean.
- `cargo doc -p glasshouse --no-deps`: clean.
- Not run: `scripts/ci-local.sh` and the Linux/Windows legs (forbidden file;
  §40 — never beside another cargo invocation regardless). No other cargo
  process was running during this package's own gate (`ps aux` checked
  before each run).

Missing evidence:
- No production caller for `routing::classify::{classify, classify_heuristically}`
  anywhere in the shipped binary. This is not a gap this package can close —
  see "nothing calls it" above. Phase 35B (candidate generation, reading a
  classification) and Phase 34F (workload-tier calibration, the other half
  of what a router would compare a classification against) are both at
  zero. A manual entry point (a `glasshouse classify <text>`-shaped command,
  the Phase 21 `--reply-from` precedent) would close it without either
  phase existing, but needs `cli.rs`/`main.rs`, both outside `YOURS` this
  round.
- The heuristic's keyword lists are hand-written and untuned against real
  request text; nothing in this batch claims they generalize past the
  representative inputs the tests use. That is expected for "deterministic
  heuristics," not a defect, but worth stating so a later phase does not
  assume the heuristic was validated against a corpus it never saw.

---

## Per-line disposition

**1489 — Add a lightweight task classifier that can run on a cheap, free, or
local model.**
Built: `routing::classify::{TaskClassification, classify, classify_heuristically}`.
`classify` takes an already-produced classification as an `Option` and never
calls a model itself, which is what keeps it "lightweight" per the module's
own doc comment rather than a claim resting on which model happens to be
configured. OPEN — no production caller; see above. Names Phase 35B.

**1490 — Classify whether a request requires repository context.**
**1491 — Classify whether a request requires code modification.**
**1492 — Classify whether a request requires shell execution.**
**1493 — Classify whether a request requires browser interaction.**
Built: `TaskClassification::{needs_repo_context, needs_code_modification,
needs_shell_execution, needs_browser_interaction}`, computed by
`classify_heuristically` from keyword signals
(`tests::a_shell_command_request_is_heavy_and_multi_turn`,
`::a_browser_task_needs_browser_interaction_and_no_repo_context_is_assumed_needed`,
`::a_generic_question_needs_no_repo_context_and_is_leaf_tier`,
`::a_question_about_a_named_file_still_needs_repo_context`). All four OPEN —
no production caller. Names Phase 35B.

**1494 — Estimate task complexity on a coarse scale.**
Built: `Complexity::{Trivial, Moderate, Complex}`, an ordered three-band
enum, deliberately no finer. OPEN. Names Phase 35B.

**1495 — Estimate whether the task is likely to require multiple turns.**
Built: `TaskClassification::likely_multi_turn`. OPEN. Names Phase 35B.

**1496 — Assign a required workload tier to the task.**
Built: `WorkloadTier::{Leaf, Standard, Heavy}` plus
`TaskClassification::workload_tier` /
`::conservative_workload_tier`. Deliberately not unified with any future
Phase 34F model-capability ceiling — see the type's own doc comment for why
a requirement and a capability sharing one scale would let a router compare
a task's tier against itself and believe that proved something. OPEN — no
production caller, and no Phase 34F ceiling yet to compare against even if
there were one. Names Phase 34F and 35B.

**1497 — Identify hard capability requirements that cannot be satisfied
merely by choosing a stronger text model.**
Built: `HardCapability::{RepositoryAccess, ShellExecution,
BrowserInteraction}` and `TaskClassification::hard_capabilities`, derived
from the signal fields rather than stored a second time
(`tests::hard_capabilities_are_derived_not_stored_separately`). OPEN. Names
Phase 35B.

**1498 — Estimate whether the task is safe for a disposable free or local
model.**
Built: `TaskClassification::safe_for_disposable_model` /
`::conservative_safe_for_disposable_model` (the latter withdrawn at
`Confidence::Low`, proven by
`tests::low_confidence_withdraws_disposable_safety_even_when_the_raw_fields_say_safe`
and its mutation above). OPEN — no production caller; note this is the box
most directly adjacent to
`routing::disposable::DisposableRouting`/`JobKind::Classification`, and the
nearest future consumer if `choose` is ever extended by a package that also
holds `memory/extract/disposable.rs`. Names Phase 35B.

**1499 — Estimate whether existing warm context is likely more valuable
than a stronger cold model.**
Built: `WarmContextValue::{PreferWarm, PreferStrongerCold}` and
`TaskClassification::warm_context`. OPEN. Names Phase 35B.

**1500 — Return classification confidence so uncertain tier assignments can
be escalated conservatively.**
Built: `Confidence::{Low, Medium, High}` plus the two `conservative_*`
accessors that escalate only at `Low`, proven by both mutation tests above
and `tests::medium_confidence_does_not_escalate` (the negative case — Medium
must not escalate, or every heuristic answer would be maximally
conservative and the tier would never actually inform anything). OPEN — no
production caller. Names Phase 35B.

**1501 — Allow classification to fall back to deterministic heuristics when
no cheap model is available.**
Built: `classify(text, None)` — `classify_heuristically(text)`, proven by
`tests::classify_falls_back_to_heuristics_when_no_model_output_is_supplied`.
Related, not the same mechanism: `config::RoutingModelChoice::Deterministic`
/ `RoutingModelResolution::Heuristics` (Phase 2C,
`crates/glasshouse/src/config/mod.rs`, read-only) already record and resolve
*whether a routing model is configured at all* and degrade visibly when a
pinned one disappears — genuinely the box's *surrounding* infrastructure,
already shipped and already reachable from configuration, but it answers
"who classifies," never "what did the request need," so it does not by
itself satisfy this line. OPEN for the classification half; the
"which entity classifies" half it depends on is already CLOSED (Phase 2C,
not re-verified by this package). Names Phase 34B (actually asking a model)
and 35B.

**1502 — Keep classification output structured and small.**
**CLOSED.** Argued as a type-level property rather than a mechanism with a
caller, the same way Phase 32's evidence closed "secrets are never
plaintext" on `SecretRef`'s own shape. `TaskClassification` has eleven
fields, all `Copy` except an optional diagnostic label on
`ClassificationSource::Model`, and `tests::the_classification_stays_small`
pins `size_of::<TaskClassification>() <= 64` bytes directly against the
type. The counter-argument — that §5 should still apply because nothing
*uses* the small shape yet — was considered and rejected: §5's concern is a
caller-dependent behaviour silently regressing behind a bypassed entry
point, and a struct's own field layout has no such caller to bypass; the
size assertion tests the type directly, the same way
`cost_has_no_third_state` (`routing/mod.rs`) closes `Cost`'s two-state
guarantee without invoking a caller. If a later reviewer disagrees with
this argument, treat this line as OPEN alongside the other thirteen — the
mechanism and its test exist either way, and nothing else in this ledger
depends on the answer.
