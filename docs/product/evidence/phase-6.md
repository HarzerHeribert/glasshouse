# Capability evidence — phase 6

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 6 — Make each adapter declare which native approval/permission modes it supports

Contract: Given a supported harness, when Glasshouse is asked what approval
modes it offers, the adapter answers with what was read from that harness's own
binary — naming its automatic-review mode when it has one, and saying plainly
when nobody established one — while never presenting a blanket bypass as though
it were review.

State: COMPLETE.

Production evidence:
- `harness/mod.rs: ApprovalModes` — `automatic_review`, `bypass` and `sandbox`,
  each a `Declared<&'static str>` like every other harness fact.
- All seven adapters declare it; `HarnessDescription` carries it.
- `integrations/mod.rs: write_adapter_report` — `glasshouse doctor` prints it,
  which is what keeps the declaration from being data nothing reads.

The distinction the type exists for: **a blanket bypass is not automatic
review.** Claude Code's auto mode, Codex's `--approve-for-me` ("automatic review
using the workspace-write sandbox") and Cursor's `--auto-review` ("a server
classifier auto-runs safe tool calls") classify. OpenCode's `--auto`
("auto-approve permissions that are not explicitly denied (dangerous!)"),
Hermes's `--yolo` and Antigravity's `--dangerously-skip-permissions` do not, and
are recorded as bypasses only.

Regression evidence:
- `each_adapter_declares_the_approval_mode_its_binary_documents` — pins the
  whole table, harness by harness, mode string by mode string.
- `every_verified_declaration_cites_its_evidence` covers the new field.
- `the_doctor_report_describes_every_adapter` — the rendered row.

Non-vacuity: **the first version of this test was weak and a mutation proved
it.** It asserted only that an `automatic_review` evidence string avoided the
words "yolo", "dangerously" and "bypass"; a mutation recording OpenCode's
`--auto` as automatic review, with evidence reading "…(dangerous!)", walked
straight through — "dangerous!" is not "dangerously". The fuzzy test was
replaced by the exact table above, and the same mutation now fails, as does
removing Cursor's real `--auto-review`. The lesson is recorded in the test's own
comment: pin *which mode each harness has*, never how a declaration is worded.

Failure/isolation evidence:
- `Unverified` renders as **"automatic review unverified"**, never "no automatic
  review". `Declared` cannot express "verified absent" for a mode name, so
  absence of a declaration means nobody established one — a different claim from
  the harness not having one. Pi makes it concrete: installed, but not on `PATH`
  here, so its `--help` could not be read and everything about it is
  `Unverified`.

Platform/external evidence:
- Every declaration read on 2026-08-25 from the installed binaries: Claude Code
  2.1.245, Codex 0.149.1, Cursor CLI 2026.08.11, OpenCode 1.18.22, Hermes Agent
  0.15.1, Antigravity CLI 1.1.20. Pi 0.73.1 could not be read.
- `glasshouse doctor` run from the built binary and its output read, which is
  how the "no automatic review" overstatement was caught before it shipped.

### Phase 6 — the harness adapter interface (eleven of twelve)

Contract: Given a harness Glasshouse supports, when anything in core needs to
know how that harness is started, resumed, messaged, interrupted, observed or
described, Glasshouse asks that harness's adapter and gets an answer derived
from the installed binary, while core itself stays unable to name any
particular harness.

State: COMPLETE for eleven of the phase's twelve lines. The
communication-style line is PARTIALLY VERIFIED and its box stays unchecked —
see its own entry below.

Production evidence:
- `harness/mod.rs: HarnessAdapter` — the contract, with `id`,
  `executable_candidates`, `start`, `resume`, `describe`, `message` and
  `interrupt`. The six verbs the map names map onto it: starting is
  `start`, resuming `resume`, messaging `message`, interrupting `interrupt`,
  observing is `describe().hooks` plus `describe().session_ids`, and
  describing is `describe` itself.
- `harness/mod.rs: adapter_for` — the registry. Total over
  `IntegrationKind::Harness`, `None` for everything else.
- `integrations/mod.rs: IntegrationId::executable_candidates` — **delegates to
  the adapter for every harness.** This is the production path that makes the
  phase's fixed requirement structural rather than aspirational: there is one
  place a harness's executable name lives, and it is the adapter.
- `session/select.rs: HarnessSelection::adapter` and
  `HarnessSelection::start_args` — the seam both start paths go through.
- `main.rs: launch_session` and `shell/mod.rs: start_session` — the two real
  session producers, both using `start_args`.
- `integrations/mod.rs: doctor_report` / `write_adapter_report` — `glasshouse
  doctor` prints every adapter's declarations. This is what keeps `describe`
  from being a data structure nothing reads; it is generic over the trait and
  cannot tell one harness from another.

Regression evidence (macOS, and Linux/Windows in CI — all are pure logic):
- `every_harness_has_an_adapter_and_nothing_else_does` — a harness added to
  the catalogue without an adapter fails here rather than at a user's launch.
- `the_catalogue_takes_harness_executable_names_from_the_adapter` — proves the
  delegation above; **mutation-checked** by restoring a hard-coded name to the
  catalogue, which fails it.
- `resume_shapes_match_the_installed_binaries` — pins all seven resume
  invocations, which genuinely differ (`--resume`, a `resume` subcommand,
  `--conversation`, `--session`). **Mutation-checked** by giving Codex Claude
  Code's flag.
- `the_executable_names_match_the_installed_binaries` — pins `agy` for
  Antigravity. **Mutation-checked** by removing it.
- `resume_passes_the_identifier_as_one_whole_argument` — an identifier is its
  own `argv` entry and always last, so a value starting with a dash cannot be
  re-read as a flag.
- `every_verified_declaration_cites_its_evidence` — a `Verified` declaration
  with no usable evidence string fails. This is the honesty rule made
  mechanical.
- `no_two_adapters_claim_the_same_executable_name` — two harnesses claiming
  one name would silently resolve as each other.
- `a_sessions_arguments_are_the_adapters_first_then_the_users` — the ordering
  rule, exercised against a test adapter that actually declares start
  arguments, since none of the seven real ones needs any today.
- `the_doctor_report_shows_each_adapters_declarations` — asserts the specific
  rows of one adapter's block, not a `contains` over the whole report.
  **Mutation-checked** by making the report loop print nothing.
- `the_doctor_report_describes_every_harness_adapter` — every adapter gets a
  block whether or not it is installed on the machine running the tests.

Failure/isolation evidence:
- `the_generic_pty_runtime_depends_on_no_adapter` and
  `the_session_model_depends_on_no_adapter` — scan the production source of
  `pty/mod.rs`, `pty/process.rs`, `session/runtime.rs` and `session/store.rs`
  for `HarnessAdapter`, `crate::harness` and `IntegrationId`. Comments are
  stripped first, because `session/store` *documents* that it stores an
  identifier's string form, which is the boundary working rather than
  breaking. **Mutation-checked** by adding a method returning an
  `IntegrationId` to `SessionRuntime`, which fails it.
- `the_dependency_scan_would_catch_a_violation` — the scanner's own
  non-vacuity check: it fires on code, and not on a doc comment or a test.
- `an_unverified_capability_is_not_treated_as_present` — `Declared::Unverified`
  reads as "cannot rely on it", never as present.

Platform/external evidence:
- Declarations derived on 2026-08-25 from binaries installed on this machine:
  Claude Code 2.1.245, Codex 0.149.0, Antigravity CLI 1.1.20, OpenCode
  1.18.22, Cursor CLI 2026.08.11-e8db854, Pi 0.73.1, Hermes Agent 0.15.1.
  Each adapter module names what was read.
- `glasshouse doctor` run from the built binary; its output is what surfaced
  two rendering defects (nested backticks, a source phrase that did not fit
  its sentence) that no unit test would have shown.

Missing evidence:
- `resume`, `message` and `interrupt` are declared and unit-proven but have no
  production caller yet: resuming is Phase 7/8's, messaging and interrupting
  are Phase 13/14's. Line 3 asks an adapter to *expose* the resume command,
  which it does; executing one is a later phase's line and is not claimed
  here.
- No adapter parses harness output yet, so line 12's guard currently protects
  a property nothing is pushing against. That is the point of installing it
  before Phase 7 rather than after.

### Phase 6 — Make each adapter declare which native communication-style mechanisms it supports and whether changing them requires a new or cleared native session

Contract: Given a harness with a native way to control how it talks to the
user, when Glasshouse needs to apply a response profile, the adapter names
that mechanism and says whether changing it costs the running session, while
never presenting an unverified guess as a mechanism.

State: PARTIALLY VERIFIED — **box deliberately unchecked.** The closing bar
recorded here on 2026-08-25 — one verified in-place mechanism, or a second
harness with a verified native mechanism of any kind — was retested on
2026-08-26 against newer installed binaries and is still unmet. A worker
batch strengthened the entry without closing it; that is recorded rather than
rounded up.

Production evidence:
- `harness/mod.rs: CommunicationStyle`, `StyleChange`, and
  `HarnessDescription::communication_style` — the declaration exists and every
  adapter fills it in.
- `harness/claude_code.rs` — declares output styles, supplied through the
  settings document `--settings` reads at startup, as `StyleChange::NewSession`.
- Each adapter's value is now a named `COMMUNICATION_STYLE` constant whose doc
  comment cites the artifact it was read from, so the declaration and its
  provenance cannot drift apart silently.

Regression evidence:
- `every_adapter_declares_its_native_communication_style_and_session_cost` —
  pins the complete seven-adapter table, so neither a new adapter nor a changed
  declaration can pass without being written down here.
- Mutation proof, re-run by the orchestrator on integrated `main` rather than
  taken from the worker's report:

      test harness::tests::every_adapter_declares_..._session_cost ... ok
      (mutate claude_code.rs NewSession -> InPlace)
      test harness::tests::every_adapter_declares_..._session_cost ... FAILED
      error: test failed, to rerun pass `-p glasshouse --lib`
      (restore)
      test harness::tests::every_adapter_declares_..._session_cost ... ok

  Before this test, flipping Claude Code's launch-only mechanism to `InPlace`
  passed every gate in the repository. That is the specific hole it closes.

Platform/external evidence:
- Native artifacts read on macOS on 2026-08-26: `claude` 2.1.246, `codex`
  0.149.1, `agy` 1.1.21, `opencode` 1.18.22, `cursor-agent`
  2026.08.11-e8db854, `hermes` 0.15.1. `pi` is absent from `PATH`.
- Claude Code, Codex and Antigravity are all newer than the versions the
  adapter module headers cite; the newer help output still documents no
  in-place communication-style mechanism for any of them.

Missing evidence:
- Six of seven adapters declare `Unverified`, because their installed binaries
  document no communication-style mechanism at all. Codex is the pointed case:
  the capability map names "Codex personalities" as an example, and Codex
  0.149.1's `--help` still exposes none.
- `StyleChange::InPlace` still has no instance, so the arm the enum exists to
  express is unexercised by any real harness.
- Closing this needs one verified in-place mechanism, or a second harness with
  a verified native mechanism of any kind. Re-reading `--help` has now failed
  to produce either twice; a third attempt should read a different artifact —
  an in-session command list or a settings schema — rather than repeat it.

Known limit, recorded rather than fixed:
- `Declared::Unverified` collapses two different claims: "this version's
  `--help` was read and documents no mechanism" (Codex, Antigravity, OpenCode,
  Cursor, Hermes) and "no artifact could be read at all" (Pi, absent from
  `PATH`). Only the evidence prose distinguishes them. Nothing consumes the
  distinction today; the first thing that will is a re-probe policy, and it
  will need a structural difference rather than a doc comment.

---

### Phase 6 — Make each adapter declare which native communication-style mechanisms it supports and whether changing them requires a new or cleared native session (line 290)

State: **COMPLETE** — orchestrator ruling, batch 51. **Phase 6 is now fully closed.**

Contract: Given any installed harness, when a user runs `glasshouse doctor`,
Glasshouse shows which native communication-style mechanism that harness
supports and whether changing it costs a new session — distinguishing "nothing
established it" from "there is none", and never claiming a mechanism it cannot
cite.

**The declaration slot already existed and every adapter filled it with a
placeholder.** All seven declared `Declared::Unverified`. `Declared`'s own
documentation is the law this package worked under: `Verified` carries
`evidence` naming a source *"concrete enough to re-check"*, and `Unverified`
means *"nothing available in this environment established it. Not 'no', and
never a guess."* So the deliverable was **establishing**, not filling in.

Result per adapter: Claude Code already `Verified{NewSession}`; **Hermes newly
`Verified{InPlace}`** — its mechanism found outside `--help`; Codex, Antigravity,
OpenCode, Cursor and Pi rewritten and **left `Unverified`**, each recording what
was ruled out rather than merely that nothing was found. Five of seven staying
`Unverified` is the correct outcome, not a shortfall.

**The consumer was the whole gap, and the orchestrator closed it rather than
the lead.** `communication_style` was written by all seven adapters and read in
production by **nothing**: `harness/mod.rs`'s readers are behind `#[cfg(test)]`,
and every construction in `profile/` and `session/select.rs` is a fixture past
those files' `#[cfg(test)]` markers. `write_adapter_report` printed vendor,
resume, session ids, hooks, approvals, capabilities, protocols and model — and
not this. The lead reported that and stopped rather than build a consumer it had
not been authorised to design, which was correct.

Production: `integrations/mod.rs::write_adapter_report` now renders a
`comm style:` row carrying **both** of the line's clauses — the mechanism and
its session cost — with `Unverified` printing as `unverified` rather than
collapsing to "none".

Regression: `tests/harness_declarations.rs`, five tests. Three hold properties a
table cannot: that the dimension cannot silently go dead, that a verified style
cites something re-checkable, and that an unverified one yields no value. The
fourth is the consumer test, and it runs the **shipped binary** rather than
calling `doctor_report` in-process. It deliberately does not assert any
harness's mechanism text — that would break whenever an adapter learned
something, which is the opposite of what this dimension is for.

Mutation: `drop-the-comm-style-row` — KILLED, run by the orchestrator, on
*"every adapter's doctor entry must carry a communication-style row, so a
declaration nobody reads cannot masquerade as a capability."*

Limit: the `StyleChange` half has no consumer that *acts* on it — everything
applies styles at launch. The line asks for declaration, which is met; a router
that avoids giving up a warm session is a routing-phase capability and inventing
one here would be §35 pointed the other way.
