# Capability evidence — phase 35

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 35 — the classifier now has a caller: `glasshouse classify <text>`

**Audit first (Pass 1).** Nothing in the shipped binary classified a request
along any of the map's fourteen dimensions before the batch that built
`classify.rs`. `routing::disposable::JobKind::Classification` (Phase 9I) is
only a *label* a disposable job can carry — grepped across the crate, it is
constructed by tests and by `RoutedNoModel`'s own fixtures, never by
`main.rs`, so the "job before spending premium agent capacity" the map's
preamble describes has no job to name yet. `config::RoutingModelChoice` /
`RoutingModelResolution` (Phase 2C, `crates/glasshouse/src/config/mod.rs`,
read-only for this package) record *which model, if any, would classify a
request* and resolve that against configured providers — genuinely adjacent,
and cited below per box — but that machinery answers "who classifies," never
"what did the request need," which is this phase's actual content. Nothing
else in the crate (`harness::pairing::classify`, `memory::store::Classifier`,
`launch::classify_launch_kind`) classifies a *request*; each classifies
something else entirely (a model/harness pairing, a memory's authority, a
launch script's file type) and the name is the only thing they share.
**Absent**, for all fourteen lines, before that batch.

**Built (prior round).**
`crates/glasshouse/src/routing/classify.rs` adds `TaskClassification` — the
eleven-field, `Copy`-heavy struct that answers every classification box
directly — plus `classify_heuristically` (a pure, deterministic
keyword-matching function: the required "fall back to deterministic
heuristics when no cheap model is available") and `classify` (prefers an
already-produced model answer, falls through to the heuristic otherwise — the
"run on a cheap, free, or local model" half, kept lightweight by never
calling one itself). `WorkloadTier::escalate`,
`TaskClassification::conservative_workload_tier`, and
`::conservative_safe_for_disposable_model` are the confidence-escalation
mechanism, applied only at `Confidence::Low`.

**"Nothing calls it" — restated and re-verified at the start of this round,
then closed.** The prior round's blocker was real and precisely diagnosed: a
production caller would have to sit on the request-handling path, in
`main.rs` or `cli.rs`, and neither file was in that package's file set — the
mechanism was sound, integrated, and reachable from nothing. Re-checked
before writing any code this round: `grep`ping for
`TaskClass|classify::classify\b|routing::classify` across
`crates/glasshouse/src` outside `classify.rs` itself still found nothing but
one doc-comment mention in `routing/mod.rs`. This package's entire reason to
exist is that `main.rs` and `cli.rs` are `YOURS` this round, for exactly that
gap (see the packet's own "WHY THIS PACKAGE EXISTS").

**Built this round.** `glasshouse classify <text>` — `cli.rs`'s
`Command::Classify` variant, `main.rs`'s matching arm, and a new
`routing::classify::report` function living in the same module the
classifier already did. Modelled directly on `glasshouse pairing` /
`glasshouse response` / `glasshouse resources`: read-only, one screen, and
the module owns its own `report` function that `main.rs`'s arm calls in one
line — the same shape `config::pairing::report` and `config::response::report`
already use. The text arguments join with a space (`text: Vec<String>`, the
same free-form pattern `glasshouse memory search`'s `query` already uses)
and are passed straight to `classify(text, None)`. `None`, because **no
cheap model is wired up in this build** — the heuristic path is not a
fallback for this caller, it is the only path available, and the report's
`source` line prints `deterministic heuristics` on every run rather than
implying anything else answered. This is the same shape practice §33 settled
for `glasshouse memory extract --reply-from`: the model half is supplied by
hand there, or here is simply absent, and the capability still *completes*
and *produces its result* in the shipped binary either way.

**The caller reaches every field.** `report`'s "Signals" section prints
`needs_repo_context` / `needs_code_modification` / `needs_shell_execution` /
`needs_browser_interaction`; "Estimates" prints `complexity`,
`likely_multi_turn`, and `warm_context`; "Routing" prints `confidence`, both
`workload_tier` and its conservative escalation, both
`safe_for_disposable_model` and its conservative withdrawal, and the derived
`hard_capabilities` list. No field on `TaskClassification` is computed and
left unprinted.

**Proof this is a real caller, not one more test-shaped door (§35).** The
danger `routing_policy.rs`'s own header names for the two routing-policy
modules applies here too: a test built directly against
`classify_heuristically` would keep passing even if `main.rs`'s `Classify`
arm were deleted outright, because nothing would call the module and nothing
would fail — the same shape as `lead-route`'s M18 finding, one layer up. So
the reachability claim is proven by two tests in
`crates/glasshouse/tests/routing_policy.rs::command_dispatch` that spawn the
real compiled executable (`CARGO_BIN_EXE_glasshouse`, the idiom
`launch_overlay.rs` and `pty_smoke.rs` already use for binary-level tests)
and read its real stdout, not by calling any library function directly.
**Mutation-proofed per the packet's own §35 instruction**:
`Command::Classify`'s arm body in `main.rs` was deleted (replaced with a
no-op and an unused-var binding) — `ok` before on both `command_dispatch`
tests, `FAILED` on both after, `ok` after restore, confirmed byte-identical
to the pre-mutation file by `git diff`.

**Disposition.** Per §5/§33's test — "ask the capability as a question a
user would ask, and see whether the honest answer is yes" — *can a person
ask Glasshouse what a request needs and get a structured answer back, in the
shipped binary, today?* Yes: `glasshouse classify <text>`, demonstrated below
with three live runs (a shell-and-code request, a plain question, and the
empty string). Per D4 in the packet, 1496 ("assign a required workload
tier") closes on the tier this module already assigns —
`conservative_workload_tier` — without needing Phase 34A's model-capability
ceiling, which this package deliberately does not build. Per D2/D3, 1489 and
1501 close on the same argument §33 already settled for `--reply-from`: the
model half is absent by design in this build, the heuristic half runs for
real, and the report says exactly what happened rather than implying more.
Line 1502 stays closed on its pre-existing type-level argument, unchanged
this round and restated below.

State: **COMPLETE** — all fourteen lines close this round: thirteen on the
caller built this round reaching every field it prints, plus line 1502 on
the type-level argument carried over unchanged from the prior round.

Production evidence:
- `glasshouse classify run cargo test and fix whatever fails` (built binary,
  this worktree, `--data-dir`/`--config-dir` pointed at a scratch directory,
  a bare `git init` as the project root):
  ```
  Glasshouse task classification
  ===============================

  request                 "run cargo test and fix whatever fails"
  source                  deterministic heuristics

  Signals
    repository context      yes
    code modification       yes
    shell execution         yes
    browser interaction     no

  Estimates
    complexity              complex
    likely multi-turn       yes
    warm context            prefer warm

  Routing
    confidence              medium
    workload tier           heavy (conservative: heavy)
    safe for disposable     no (conservative: no)
    hard capabilities       repository access, shell execution
  ```
- `glasshouse classify what is a mutex` (same binary, same run): `workload
  tier leaf (conservative: leaf)`, `safe for disposable yes (conservative:
  yes)`, `hard capabilities none` — the opposite end of the scale, live.
- `glasshouse classify` with no text at all: `confidence low`, `workload
  tier leaf (conservative: standard)`, `safe for disposable no (conservative:
  no)` — the fail-closed escalation line 1500 requires, firing on a real,
  unparsed-anything input rather than a fixture built to trigger it.
- `glasshouse --help` lists `classify` beside `sessions`, `resources`, and
  the rest — a first-class entry point, not a hidden debug path.

Regression evidence (all reachable only through tests — noted, not hidden):
- `routing::classify::tests::*` (13 tests, `crates/glasshouse/src/routing/classify.rs`)
  — the author's own unit tests: field-by-field behaviour for a shell
  request, a browser request, a generic question, a file-referencing
  question, an ambiguous/empty request, escalation at `Confidence::Low`,
  non-escalation at `Confidence::Medium`, `classify`'s model-preferred /
  heuristic-fallback behaviour, the size bound for line 1502, and (new this
  round) `report_says_no_model_was_consulted_and_shows_the_signals`, which
  pins that `report()`'s text says `deterministic heuristics` — never
  implying a model answered — and shows the shell-execution and
  workload-tier signals a caller would actually read.
- `classification::*` (4 tests, `crates/glasshouse/tests/routing_policy.rs`)
  — independent, outside-in tests against the module's own doc-comment
  claims rather than its unit tests (same split this file's header states
  for the two routing-policy modules): `classify_heuristically` is
  deterministic on five representative inputs; the module makes no network
  call; an empty request degrades to `Confidence::Low` rather than panicking
  or guessing confidently; `classify()` returns a supplied model answer
  unmodified even when it actively disagrees with what the heuristic alone
  would say, proving the two paths are not blended.
- `command_dispatch::*` (2 tests, `crates/glasshouse/tests/routing_policy.rs`,
  new this round) — the §35 proof described above: spawns the real
  `glasshouse` binary and asserts on its real stdout for a shell-shaped
  request and a plain question.

Mutation evidence (practice §41):
- **New this round — the production *call site* itself, per the packet's
  own §35 instruction**: `Command::Classify`'s match-arm body in `main.rs`
  deleted (replaced with a no-op and an unused-var binding). Both
  `command_dispatch` tests: `ok` before, `FAILED` after (`no model is wired
  up in this build, so the source line must say so` /
  `a plain question must reach leaf tier through the real dispatch`), `ok`
  after restore — verified byte-identical to the pre-mutation file by `git
  diff`. Every source file `touch`ed before each build; a private,
  worktree-local `target/` throughout (§16 — this worktree has never shared
  one with another checkout).
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
  (`tests/routing_policy.rs`, the outside-in test) — recorded in the prior
  round when no production call site existed yet to mutate instead; `classify`
  itself is still not on the production call path (`report` calls it with
  `model_output = None` always), so this mutation form remains the right one
  for `classify`'s own model-preference behaviour. `ok` after restore.

Platform/external evidence:
- `cargo build -p glasshouse`: clean (one fixup needed this round —
  `HardCapability::as_str` takes `self` by value, so
  `caps.iter().map(HardCapability::as_str)` did not type-check; fixed with
  `.iter().copied().map(...)`).
- `cargo test -p glasshouse` (macOS, this worktree, run alone per §40, `ps
  aux` checked clean of other cargo processes before starting): every lib
  and integration target passed, 0 failed — 1252 lib tests (up from 1150;
  the new `report` unit test and other work merged into this branch since
  the last round) and 24 in `routing_policy` (up from 22: the two new
  `command_dispatch` tests).
- `cargo test -p glasshouse --test routing_policy`: 24 passed, 0 failed.
- `cargo clippy -p glasshouse --all-targets -- -D warnings`: clean.
- `rustfmt --edition 2024 --check` on the four files this package touched
  (`cli.rs`, `main.rs`, `routing/classify.rs`, `tests/routing_policy.rs` —
  §37: `cargo fmt --all` is not this package's to run): clean.
- `cargo doc -p glasshouse --no-deps`: clean (§60 addendum — run before the
  rest of the gate).
- Not run: `scripts/ci-local.sh` and the Linux/Windows legs (forbidden file;
  §40 — never beside another cargo invocation regardless).

Missing evidence:
- The heuristic's keyword lists are hand-written and untuned against real
  request text; nothing in this batch or the last claims they generalize
  past the representative inputs the tests use. Expected for "deterministic
  heuristics," not a defect — restated so a later phase does not assume the
  heuristic was validated against a corpus it never saw.
- `glasshouse classify` is a manual, on-demand report, exactly like
  `pairing`/`response`/`resources` — nothing in the routing pipeline calls
  `classify()` automatically ahead of a real request yet. That is Phase 35B
  (candidate generation reading a classification), still at zero, and is
  outside this package's scope; the map's fourteen lines describe the
  classifier's own capability, not its automatic invocation, and D1/D2 in
  the packet settle that a manual caller is the intended shape for this
  round, the same way `--reply-from` settled manual memory extraction.

---

## Per-line disposition

**1489 — Add a lightweight task classifier that can run on a cheap, free, or
local model.**
Built: `routing::classify::{TaskClassification, classify, classify_heuristically}`,
now called by `routing::classify::report`, called by `main.rs`'s
`Command::Classify` arm, reached by `glasshouse classify <text>`. `classify`
takes an already-produced classification as an `Option` and never calls a
model itself, which is what keeps it "lightweight" per the module's own doc
comment rather than a claim resting on which model happens to be configured
— per packet D3, this line is about capability, not about a model being
configured. **CLOSED.**

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
`::a_question_about_a_named_file_still_needs_repo_context`), all four printed
in `report`'s "Signals" section and demonstrated live above (shell + code
modification `yes` for a shell request, all four `no` for a plain question).
**CLOSED**, all four.

**1494 — Estimate task complexity on a coarse scale.**
Built: `Complexity::{Trivial, Moderate, Complex}`, an ordered three-band
enum, deliberately no finer, printed in `report`'s "Estimates" section
(`complex` and `trivial` both demonstrated live above). **CLOSED.**

**1495 — Estimate whether the task is likely to require multiple turns.**
Built: `TaskClassification::likely_multi_turn`, printed in "Estimates".
**CLOSED.**

**1496 — Assign a required workload tier to the task.**
Built: `WorkloadTier::{Leaf, Standard, Heavy}` plus
`TaskClassification::workload_tier` / `::conservative_workload_tier`,
printed in "Routing" as both the raw and conservative tier. Deliberately not
unified with any future Phase 34F model-capability ceiling — see the type's
own doc comment for why a requirement and a capability sharing one scale
would let a router compare a task's tier against itself and believe that
proved something. Per packet D4, this box is satisfied by assignment alone;
it does not ask for a comparison against a capability ceiling, and this
package deliberately does not build Phase 34A to manufacture one. **CLOSED.**
Names Phase 34A only as the phase that must not be built to close this line,
not as a blocker.

**1497 — Identify hard capability requirements that cannot be satisfied
merely by choosing a stronger text model.**
Built: `HardCapability::{RepositoryAccess, ShellExecution,
BrowserInteraction}` and `TaskClassification::hard_capabilities`, derived
from the signal fields rather than stored a second time
(`tests::hard_capabilities_are_derived_not_stored_separately`), printed as
`report`'s "hard capabilities" line (`repository access, shell execution`
and `none` both demonstrated live above). **CLOSED.**

**1498 — Estimate whether the task is safe for a disposable free or local
model.**
Built: `TaskClassification::safe_for_disposable_model` /
`::conservative_safe_for_disposable_model` (the latter withdrawn at
`Confidence::Low`, proven by
`tests::low_confidence_withdraws_disposable_safety_even_when_the_raw_fields_say_safe`
and its mutation above), printed in "Routing" as both the raw and
conservative answer. Still the box most directly adjacent to
`routing::disposable::DisposableRouting` / `JobKind::Classification`, and
still the nearest future consumer if `choose` is ever extended by a package
that also holds `memory/extract/disposable.rs` — that extension is not this
package's and is not needed to close this line, which asks only for the
estimate to exist and be reachable. **CLOSED.**

**1499 — Estimate whether existing warm context is likely more valuable
than a stronger cold model.**
Built: `WarmContextValue::{PreferWarm, PreferStrongerCold}` and
`TaskClassification::warm_context`, printed in "Estimates" (`prefer warm`
demonstrated live above for a multi-turn shell request). **CLOSED.**

**1500 — Return classification confidence so uncertain tier assignments can
be escalated conservatively.**
Built: `Confidence::{Low, Medium, High}` plus the two `conservative_*`
accessors that escalate only at `Low`, proven by both mutation tests above
and `tests::medium_confidence_does_not_escalate` (the negative case), printed
in "Routing" as the `confidence` line. The escalation itself is demonstrated
live: `glasshouse classify` with no text prints `confidence low` and
`workload tier leaf (conservative: standard)` — the raw and conservative
tiers actually diverge on a real run, not only inside a unit test.
**CLOSED.**

**1501 — Allow classification to fall back to deterministic heuristics when
no cheap model is available.**
Built: `classify(text, None)` inside `report`, called on every
`glasshouse classify` invocation, proven at the unit level by
`tests::classify_falls_back_to_heuristics_when_no_model_output_is_supplied`
and at the binary level by every live run above, each of which prints
`source deterministic heuristics`. Per packet D2, no cheap model is wired up
in this build, so this is not a fallback exercised occasionally — it is the
only path this caller can take, and it is the path proven above. Related,
not the same mechanism: `config::RoutingModelChoice::Deterministic` /
`RoutingModelResolution::Heuristics` (Phase 2C,
`crates/glasshouse/src/config/mod.rs`, read-only) already record and resolve
*whether a routing model is configured at all* — genuinely the box's
*surrounding* infrastructure, already shipped, but it answers "who
classifies," never "what did the request need," so it does not by itself
satisfy this line; this line is satisfied by `classify`'s own fallback
behaviour, now reachable. **CLOSED**, on the same argument practice §33
settled for `glasshouse memory extract --reply-from`: the model half is
absent by design, the deterministic half runs for real and completes, and
the report says so on every run.

**1502 — Keep classification output structured and small.**
**CLOSED**, unchanged from the prior round. Argued as a type-level property
rather than a mechanism with a caller, the same way Phase 32's evidence
closed "secrets are never plaintext" on `SecretRef`'s own shape.
`TaskClassification` has eleven fields, all `Copy` except an optional
diagnostic label on `ClassificationSource::Model`, and
`tests::the_classification_stays_small` pins
`size_of::<TaskClassification>() <= 64` bytes directly against the type. The
counter-argument — that §5 should still apply because nothing *uses* the
small shape yet — was considered and rejected last round: §5's concern is a
caller-dependent behaviour silently regressing behind a bypassed entry
point, and a struct's own field layout has no such caller to bypass. That
counter-argument is now moot in any case: a caller exists, `report()` prints
the whole small structure on one screen, and the type is exactly as small
under real use as the unit test claimed in isolation.
