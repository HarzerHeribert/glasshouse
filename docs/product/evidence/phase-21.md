# Capability evidence — phase 21

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21 — the five extraction lines that stay open, and why

Recorded so a later session does not re-derive them.

- *Allow a configurable cheap or local model to perform memory extraction.* —
  **Phase 39.** There is no way to call a model in this codebase.
  `ExtractionModel` is the seam and `ExtractionOutcome::model` is where
  Phase 39's *"record which resource performed important memory extraction"*
  lands. The seam is deliberately **synchronous** (this codebase has no async
  runtime, and extraction runs on a thread so it never blocks a PTY drain),
  `Send + Sync`, and its error type takes a `&'static str` so a provider's
  error body — which can echo the request, and the request is a prompt —
  cannot be routed into a Glasshouse diagnostic.
- *Require the extractor to omit speculative claims that were not established.*
  — **half-enforced, and the gap is real.** A memory marked
  `support: speculative` is dropped and counted (M11 killed). A memory
  *wrongly* marked `established` is stored, and no code here can catch it:
  whether a claim was established is a judgment about the session, which is
  the same thing `memory/policy.rs` already declined to fake at the storage
  layer. Needs extraction evaluation against a real model.
- *Require the extractor to preserve concise rationale when a decision's
  rationale is important.* — enforced where importance is decidable (a
  `decision` declared `invariant`, `constraint` or `decision` **must** carry a
  rationale or it is refused, M10 killed; capped at 400 characters for
  "concise"), but "important" is approximated by "binding", and there is no
  `memories.rationale` column, so it is folded into the body behind
  `RATIONALE_MARKER` and renders as `Why:` in a search. Findable, but a
  consumer cannot ask for a decision *without* its rationale.
- *Store the originating session and event references.* — the session half is
  done and proved; the **event** half has nowhere to go. The DDL is written
  and reviewed, below.
- *Allow memory extraction to run after task completion* / *before or around
  native prompt compaction.* — no caller exists for either. See the migration
  note and Phase 7/8 note below.

Migration ready to apply, deliberately not applied this batch:
- Three columns on `memories` — `source_event_first`, `source_event_last`
  (the `RecordedEvent::seq` range of the chunk; a range and not a single id,
  because extraction reads a slice and a memory is rarely traceable to one
  event; nullable, because a hand-written memory having no event range is a
  different fact from an empty one) and `rationale`.
- **It is migration 6, not 5** — `lead-record` took 5 for `lifecycle_events`
  and `checkpoints` in the batch immediately before this one. The DDL as
  written in that report says 5.
- **`rationale` can hold a credential, exactly like `subject` and `body`, and
  must not be certified otherwise.** The control stays on the producer:
  `judge` screens the whole element before reading any field, so coverage is
  automatic as long as `rationale` stays inside the element.
- `memories_fts` is an external-content index over `subject` and `body` only.
  Making `rationale` searchable is a **rebuild** of the index and its three
  triggers, not an `ALTER` — which is why this is a real migration rather than
  three column additions, and why it was not squeezed into this batch.

Blocked two phases deep:
- Extraction around compaction cannot be observed today from either harness.
  Codex's hook catalogue *has* `PreCompact`/`PostCompact` and
  `harness/codex.rs`'s `REPORTED_EVENTS` deliberately does not ask for them;
  Claude Code's observed catalogue does not list them at all. Phase 7 line 307
  and Phase 8 line 324 are the boxes that unblock it.
  `ExtractionTrigger::BeforeCompaction` exists and waits.

---

### Phase 21 — migration 6, provenance, and a failure the coding session survives (lines 814, 816, 820)

Contract: An extracted memory carries the range of the event log it came from
and the rationale behind it; a person can run extraction over a session's own
recorded events; and extraction failing never costs the user their turn.

State: COMPLETE

Production evidence:
- **Migration 6** adds `source_event_first`, `source_event_last` and
  `rationale` to `memories`, and **rebuilds `memories_fts`** with `rationale`
  as a third indexed column plus its three triggers — the rebuild the previous
  batch flagged as the real work. `RATIONALE_MARKER`'s fold into the body is
  gone: the rationale is now its own column and its own line in a search.
- `memory/extract/lifecycle.rs` — `describe(&LoggedEvent)` and
  `chunk_for_session`, which builds a bounded, scrubbed chunk **that knows the
  range of the log it covers**.
- **`glasshouse memory extract --session <id> --from-events`** — a caller a
  person can run. `--activity` and `--from-events` are mutually exclusive and
  one is required, so extraction is never run over activity nobody chose.
- **The chunk's event range is narrowed to what survived the budget.**
  `SessionChunk::build` keeps the newest entries when the budget binds, so a
  chunk that dropped the first sixteen events must not claim a memory came from
  them. Tested independently by both subcontractors — unit and end-to-end — and
  the mutation widening the range back to the whole input slice is killed by
  both.

**Why the event log is the right source and not a consolation.** A hook payload
carries the user's prompt and the model's last message; Glasshouse's handler
drains that stream unread, and `lifecycle_events` has no column a conversation
could reach. **A chunk built from the event log cannot contain conversation
text because there is none to contain** — the credential and privacy properties
hold by construction rather than by a screen.

Regression evidence (box 820, and the failure path is the one that actually
runs in production today):
- `a_failing_extraction_model_costs_the_coding_session_nothing` — a refusing
  model and a panicking model; asserts the lifecycle still moved to `idle` and
  the event was still recorded.
- `an_extraction_model_that_never_answers_is_abandoned_at_its_bound` — a model
  that sleeps a minute; asserts the hook returned at `EXTRACTION_BOUND` (5s)
  and not after the sleep, **in both directions**.
- Four failures are absorbed and none reaches the hook: the database not
  opening, the model refusing, the model **panicking** (`catch_unwind`), and
  the model **hanging** (the work is on its own thread; the hook waits on a
  channel and leaves it behind).

**Why 820 is load-bearing rather than defensive.** `glasshouse hook` runs
*inside* the user's session, and Claude Code treats a hook's non-zero exit as a
veto on the turn — the user's own words echoed back at them with nothing sent.
This project observed that directly. `report_hook` may never fail.

**The panic hook, decided rather than deferred.** `memory/extract` recorded a
caveat it could not fix: `catch_unwind` catches the panic but the default hook
has already printed to stderr. `install_quiet_panic_hook()` is installed **only
in the `Command::Hook` arm** — not process-wide from a library module — and
routes the payload and location to `tracing::error!`. A Rust backtrace in the
middle of someone's coding session because a support job fell over is the same
defect as the hook failing. The panic is logged, not swallowed.

Verified against the shipped binary, and reproduced independently by the
orchestrator: a real `Stop` hook exits `0`, records `turn_ended`, moves the
session to `idle`, and logs
`memory extraction after a completed task produced nothing … reason=no
extraction model is available`.

---

### Phase 21 — "Allow memory extraction to run after task completion" stays OPEN, and the criterion that decides it

State: SCAFFOLDED — the wiring is complete, proven, and reachable; the
capability does not complete.

What exists:
- `report_hook` is a two-line wrapper over `report_hook_with(runtime, session,
  event, model_factory)`. On a translated `TurnEnded { outcome: Completed }` —
  and on nothing else — `run_extraction_after_turn` runs, **after** the event is
  recorded, so the turn's own closing event is in the material extraction reads.
- `an_event_that_is_not_a_completed_task_asks_no_model` is the discriminating
  half: `StopFailure`, `UserPromptSubmit` and `PermissionRequest` ask no model.
  Without it, *"runs after task completion"* would be satisfied by *"runs
  always"*.
- Production passes `NoExtractionModel`, whose `describe()` is
  `none configured (Phase 39 supplies the provider)`; that string is on every
  outcome and in every log line, and a mutation renaming it to
  `phase-39/cheap-model` is killed.

**Why it is not closed, and the sharpened criterion.** The lead argued for
closing it: the map states the trigger (817) and the model (809) as two separate
lines, and practice §33 closed *"run manually"* on a caller that calls no model.
It also stated the counter-argument and left the decision here. The counter is
right, and the reason is a criterion worth stating once:

**The test is not whether a model is called. It is whether the capability
completes and produces its result in the shipped binary.**

- *Run manually* completes: `--reply-from` supplies the model half at the user's
  direction, the whole pipeline runs, and **memories are stored** — verified by
  the orchestrator running the binary.
- *Run after task completion* cannot complete: the trigger fires on every
  completed task and dead-ends, always, because nothing can supply the model
  half on a turn boundary. Independently reproduced by the orchestrator: a real
  `Stop` hook exits `0` and stores nothing, and `memory search` finds nothing,
  ever.

Asked plainly — *can Glasshouse run memory extraction after task completion?* —
today's honest answer is "it tries, every time, and reports it has no model."
That is not the line. It closes the moment any model exists, at one line in
`main.rs` passing a different `Box<dyn ExtractionModel>`, and 809 and 817 will
close together for a reason that is about Phase 39 rather than about this box.

---

### Phase 21 line 837 — speculative claims are omitted, and this was already shipped

Contract: Given a session in which a model proposed something that was never
established, when memory extraction runs, Glasshouse drops that element rather
than storing it as project knowledge.

State: **COMPLETE** — already shipped before this package; verified and ticked
rather than built.

Production evidence:
- `memory/extract/schema.rs`: `Support::Speculative` is a **required** field
  that `judge()` checks before any other field. A speculative element returns
  `Verdict::Speculative` and is dropped, counted, and never stored.

Regression evidence:
- `tests/memory_extract_schema.rs::a_speculative_memory_is_dropped_rather_than_rejected_or_stored`
  — note the distinction the test name carries: *dropped*, not *rejected*. A
  rejection would be an error the caller has to handle; a drop is the honest
  outcome for a model volunteering something it was not asked for.

**Third time in three batches that a box closed on already-shipped code** —
Phase 48 closed six that way, Phase 35 fourteen. The pattern is consistent
enough to plan around: before sizing a package, ask what is already built and
merely unticked.

### Phase 21A line 862 — still open, and looked at hard before being left

State: NOT STARTED, blocked on the same unverifiable judgement as Phase 20's
828/829.

`PROMPT_CONTRACT` rule 7 states *"distinguish a hard requirement from a
convenient implementation choice"* almost verbatim, and the `authority` enum
already gives the distinction a place to land (`constraint` versus
`decision`/`preference`). But nothing validates that a model's `constraint` call
was an externally-imposed requirement rather than a convenient choice it
labelled wrong — rule 7 is not one of the three fields `judge()` enforces.

The worker searched for a way to close it and reported the gap instead of
inventing a check. Recorded so the next package starts from here.

---

### Phase 21 — lines 834, 842 and 843 (batch 51). All three COMPLETE.

**The packet's ordering was wrong and the worker inverted it, correctly.** It
said 842 probably already held and 843 was probably premise-invalid. Neither
was true, and both errors came from reasoning off documents that had gone stale
against current source:

- **842 was blocked on 834**, not already-holding. `phase-21.md` had already
  argued this line and left it open on a sharpened criterion — *"the test is not
  whether a model is called; it is whether the capability completes and produces
  its result in the shipped binary"* — and recorded that it "closes the moment
  any model exists".
- **843's blocker did not apply.** A compaction trigger needs no
  `LIFECYCLE_EVENT_KINDS` value, because a compaction is not a lifecycle event
  and nothing needs to record it. Codex reports `PreCompact` today.
- **834's premise — "there is no way to call a model here" — was false.**
  `ureq` is a declared dependency and `provider/discovery.rs` has been making
  real authenticated provider requests since the gateway landed. The missing
  half was never an architecture; it was a transport.

Production: `memory/extract/model.rs` (new) — a configured model reached over
HTTP; extraction triggers for task completion and pre-compaction wired through
`main.rs` and `session/lifecycle.rs`.

**A credential leak found by the worker in its own new code, and fixed at the
source.** The first `ConfiguredModel` accepted `https://key@host/v1` and
asserted only that `describe()` omitted it — the credential then appeared in
full in the type's own `Debug`, which redacted the `credential` field and had no
reason to suspect the `endpoint` built from the base URL. **Redacting that
second exit would have left a third**, so a base URL carrying userinfo is now
refused outright, which is what `config`'s "No secrets here" rule already
required. Mutation `accept-a-credential-in-the-base-url` re-run by the
orchestrator: KILLED by
`a_base_url_carrying_a_credential_is_refused_rather_than_redacted`.

*Carried forward, outside that packet's scope:* `provider/discovery.rs::ProbeRequest`
has the same latent shape — a hand-written `Debug` redacting a credential field
beside a `base_url` printed in full. Not currently reachable with a userinfo URL
by any path checked, but worth a look.

**An absence assertion that was one edit away from going vacuous.**
`session_hook.rs` holds a matched pair keyed off a literal log string: one
asserting the line is present with extraction on, one asserting it absent with
it off. The message changed. The positive test failed loudly in the blast
radius; **the negative one would have passed silently forever**, asserting the
absence of a string production no longer emits. Both were updated together and
the positive one strengthened to require `trigger=task_completed`.

#### Limits — the 5s bound is now load-bearing in a way it never was

`EXTRACTION_BOUND` abandons extraction after five seconds and the hook process
exits moments later. Until now that could not bite, because no model existed. It
can now: **a model slower than five seconds produces nothing, silently, on every
turn**, and the user is not told — the log says only "did not finish within its
bound". A loopback runner answers in milliseconds and a fast hosted model
usually does; a large local model on a busy machine will not. This is the
existing design and the worker did not change it, but it is the number most
likely to need revisiting.

Also: **Codex clamps hook timeouts to 3 seconds**, below `EXTRACTION_BOUND`.
**No real provider was ever called** — every test is a loopback fixture
asserting the documented OpenAI chat-completions shape byte-for-byte off the
wire. **Nothing ran on Windows.** And **843 is live for Codex only**: Claude
Code's observed catalogue lists no compaction event, which is map line 310's
business and remains open.
