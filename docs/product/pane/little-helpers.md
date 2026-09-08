# pane — little helpers

A **little helper** is a cheap model given a narrow toolset to answer one
question on demand. A secretary, not a delegate: you send it an errand, it
returns a value, and it is gone.

**This document is a spec, and its point is that adding a helper is data, not
code.** One runtime executes every helper; a helper is a `HelperSpec` literal in
a const table. The guardrails live in the runtime, so a new helper cannot
introduce a new failure mode — it can only choose within a boundary already
enforced.

## Status — what is built, measured 2026-09-08

This section is the truth about the code; everything below it is the design.

| Part | State |
|---|---|
| The contract: `HelperSpec`, `HELPERS`, `check_spec`, one `run` dispatcher | **built** |
| REDUCER, SCOUT and CHECKER, all callable from a cell | **built, all three measured** |
| The capability boundary — a helper binds only the tools its spec named | **built, mutation-pinned** |
| Pure search tools `rg`, `fd`, `jq` | **built** |
| Generic install and declaration, both gating on `call_sites` | **built** |
| `validate()` at startup | **built** |
| The lane, the `/cell` HELPERS section, and the `looked` trajectory | **built** |
| Preflight · PostResult · CompletionGate producers | **built** |
| A lane while a helper is still running | **built, not proven end to end** |

**What is NOT done, plainly.** Preflight's gate is a four-word floor rather than
a classification — `glasshouse classify` is the intended producer and is not
wired. The preflight scout's `HelperRecord` is discarded, so that one helper gets
no lane and no `/cell` row. The `## Evidence` section of the preflight block is
not emitted. The recap has rendering but no producer. `OutputKind::Spans` is
still returned as text rather than a structured array. And a helper's context
still binds `helper` and `agent`, closed by refusal rather than by absence.

### What was measured

**The Reducer helps, and the measurement corrected the spec twice.** Four
fixtures with planted oracles, two of them negative controls, the preamble
extracted byte-identical from `helpers.rs`, reducer `gpt-5.6-luna`, downstream
`deepseek-v4-flash-0731` (a different model, so the answerer does not mark its
own homework):

| fixture | raw input | reduced input | both arms correct |
|---|---|---|---|
| decisive error buried at line ~1200 of 1308 | 11,390 | 154 | yes |
| 2 failures among 212 passes | 2,055 | 119 | yes |
| a clean build (control) | 3,676 | 45 | yes, no invented failures |
| a tiny log (control) | 109 | 71 | yes, no harm |

Two defects were found **in the preamble, not in the idea**: warnings were
reported as failures, and identity was dropped in favour of a `file:line` —
useless when the question is *which test failed*. Both are fixed and both
reasons are now stated in the preamble so they are not trimmed later.

**Not claimed:** the reduced arm does not use fewer TOTAL tokens on a single
question — the reduction costs them. The win is the task model's context window,
permanently, and moving those tokens to a cheap model once instead of re-sending
them every turn. Correctness was neutral, not improved.

**SCOUT is the one that buys correctness.** On a real 38,718-line codebase,
asked where a per-cell ceiling is enforced and where its default is set
(`gpt-5.6-sol` as the task model, `gpt-5.6-luna` as the helper):

| arm | sites found | context | spend |
|---|---|---|---|
| SCOUT | **4** | 4.1k | **4.2k** |
| the task model alone | 2 | 11.2k | 50.2k |

It also closed with what it had not inspected, unprompted. **CHECKER** caught a
planted off-by-one in a diff (`> ceiling` where `>= ceiling` was asked for),
naming the line and the reasoning, and returned *holds* on the corrected
control — so it does not simply cry wolf. N=1 per question; a question needing
inference rather than search is untested.

**A helper computes rather than estimates.** Asked to total fifteen two-decimal
durations it answered 39.84s on four separate runs, matching ground truth
exactly. Its `looked` trajectory shows it read the file; it does not show the
arithmetic, which happens as plain JS inside its own cell.

**The preflight format question is answered, weakly.** Three arms, same request
and served files, `deepseek-v4-flash-0731`, N=5, scored on whether the first
action pursued the meta-material instead of the task:

| arm | drifted |
|---|---|
| no preflight | 0/5 |
| selection record behind a handle | 0/5 |
| record inline as sections | **2/5** |

So the handle rule survives — but N=5 on one model, and three of five inline
runs were fine. This is evidence for keeping the rule, not proof, and the first
run of this experiment had to be thrown away: asking for *"your first action in
one sentence, then stop"* suppresses exploration by itself and produced a false
0/5 everywhere.

## A helper is not a subagent

They share one implementation and are different kinds. Confusing them is how a
helper acquires authority it must never have.

| | **subagent** | **little helper** |
|---|---|---|
| owns | a goal | a question |
| returns | work done — effects in the world | a value |
| decides its own approach | yes | no — the caller states what it needs |
| lifetime | long, independent | atomic; internally iterative, externally atomic |
| state after returning | its effects persist | none |
| may write | within its grant | **never — it has no write tool** |
| may call a helper | **yes** | no |
| may call a subagent | no | no |

**The call graph is a bounded tree of depth three.** The task model may call a
subagent or a helper; a subagent may call a helper; a helper is a leaf.
`agent.rs:17` already forbids a subagent starting a subagent — helpers relax
that by one edge and close it again at the leaf.

A Scout loops to explore because exploring requires it, but the loop dies with
the call and leaves no conversation. That is what keeps it atomic from the
caller's side.

## The contract

```rust
/// One helper, entirely as data. Adding a helper is adding one of these.
pub struct HelperSpec {
    /// Called as `helper.<name>(…)`; also the key in the generated declarations.
    pub name: &'static str,
    /// One line, rendered into the declarations the caller sees.
    pub summary: &'static str,
    /// Its fixed instruction preamble — the only prose a helper carries.
    pub preamble: &'static str,
    /// Tool names, a subset of `registry::ALL`. Empty is legal and is the
    /// safest possible helper.
    pub tools: &'static [&'static str],
    /// Turns it may take inside one call. `1` is one-shot.
    pub max_turns: u32,
    pub input: InputKind,
    pub output: OutputKind,
    /// Where it may be invoked from.
    pub call_sites: &'static [CallSite],
}

pub enum InputKind  { Request, Text, Handle, Diff }
pub enum OutputKind { Spans, Reduction, Verdict }
pub enum CallSite   { Preflight, PostResult, CompletionGate, Cell }

/// The roster. This array is the whole extension point.
pub const HELPERS: &[HelperSpec] = &[SCOUT, REDUCER, CHECKER];

/// One runtime for every helper: `agent::run` with the spec's toolset,
/// turn cap and tier model.
pub fn run(spec: &HelperSpec, profile: &Profile, input: Input, …) -> HelperResult;
```

### Invariants the runtime enforces, so a spec cannot break them

| Enforced | How |
|---|---|
| A helper never writes, edits or executes | `spec.tools` is rejected at startup if it names `write`, `edit` or `bash`. Not a rule a model might ignore — a list it does not have |
| A helper is a leaf | It runs with the subagent guard already in `agent.rs`, plus the helper binding absent from its own declarations |
| Every call is explicable | The call is recorded as a `CallRecord`, so §9.4's trajectory explains the cell that made it |
| Output never injects itself | `run` returns a handle; only the caller may `keep` it. Context growth stays the caller's decision and stays in the ledger |
| Cost is bounded per cell | A per-cell call ceiling in `[limits]`, beside `cell_wall_clock_s`. Exceeding it is a refusal the model can catch |
| Turns are bounded | `spec.max_turns` clamped against `HELPER_MAX_TURNS` |

Two contracts no toolset can express, so they stay prose and stay in each
helper's `preamble`:

- **Evidence, never conclusions.** A Scout holding only `read` and `grep` can
  still answer *"the bug is X"*. It must not. A wrong diagnosis the caller
  trusts is worse than no diagnosis.
- **Say what you did not look at.** A helper that ran out of turns says so.

## How to add a helper

Three steps, no new mechanism:

1. Write a `HelperSpec` const — name, summary, preamble, toolset, turns, input,
   output, call sites.
2. Append it to `HELPERS`.
3. Add one fixture test: given a known input, the helper's output names the
   known answer, **and** a spec naming a mutating tool is refused at startup.

The declaration the caller sees is generated from the spec, so nothing is
written into the stable preamble — the improvement register's migration table
already puts tool names and argument types in generated declarations.

If a proposed helper needs anything outside `HelperSpec` — a new tool, an
effect, a second model call, a longer life — **it is not a helper.** It is a
subagent, or it is a change to the runtime that every helper then inherits, and
it gets reviewed as such.

## The three we start with

| | **SCOUT** | **REDUCER** | **CHECKER** |
|---|---|---|---|
| question | where is it · what is relevant | what does this say, shorter | does this hold |
| `tools` | `read` `glob` `grep` `context` | **`[]`** | `read` `grep` |
| `max_turns` | 8 | 1 | 3 |
| `input` | `Request` | `Text` \| `Handle` | `Diff` |
| `output` | `Spans` | `Reduction` | `Verdict` |
| `call_sites` | `Preflight`, `Cell` | `PostResult`, `Cell` | `CompletionGate`, `Cell` |
| absorbs | selection, oracle scouting, boundary scouting, conventions, semantic search, index answers | log reduction, evidence compaction | diff review, claim verification |

Everything the earlier draft listed as twelve helpers is one of these three
asking a different question. A new question is a new *call*, not a new helper;
a new **toolset** is a new helper.

## Not helpers

| job | mechanism |
|---|---|
| Structural elision of code — signatures and doc comments, bodies elided *and marked elided* | `oxc`, already in the process. `project::source_context::pack` is most of it |
| Repeated-failure detection | A fingerprint over the cell's source head and call trajectory. Instant, exact, free |
| Staleness of an edit target or a served file | The SHA-256 binding already in `tools/exact_edit.rs` |

**Prefer a parser to a model, and a hash to a parser.** A cheap model is for
ranking, selecting and describing. A parser's elision is reproducible, marked,
and never drops a branch.

## Two ways in

### Pulled — the callable primitive

A cell is code, so a helper call sits inside an `if`, a `catch` or a loop and
**costs no turn**. Every other harness spends a round-trip to ask a question;
pane asks inside the cell already running.

    try { await edit({ ... }) }
    catch (e) { keep("trace", await helper.trace(e)) }

*Attempt the work; pay for the diagnosis only when it fails.*

Shaped like `mcp`, not like `bash` — a host binding making one metered wire
call, not a program exec'd in the sandbox. It joins `NON_TOOL_HOST_FUNCTIONS`;
`registry::ALL` stays the set of sandboxed programs and `sandbox-grants.md` is
untouched, because nothing new runs on the machine.

### Pushed — the preflight hook

`build_system_prompt` (`session.rs:483`) is called from `run_task_inner`
(`session.rs:1139`) with the request text in scope and does not receive it. It
appends `project::orientation::collect` — *"one bounded, read-only snapshot that
orients a task before its first cell"*. Preflight is that function made
task-aware: same call site, same consumer, one Scout.

Gated on a classification that the request needs repository context at all
(`glasshouse classify`). **The gate keeps irrelevant files out of context; it
is not about time.**

    ## Request (verbatim, authoritative)
    ## Reading (advisory, at most three lines — the request above governs)
    ## Served in full (N)   — file contents, each under one line of why
    ## Evidence (summarised; refresh with `context(…)`)
    ## Selection record     — one line: counts, and the handle holding the rest

**A section that reads as an open question will be answered.** The first turn
should be the task. *"Could not determine"* invites determination and a list of
rejected candidates is a menu to browse, so neither is a section — the record
goes behind a handle. **Recoverable, not present.**

**The verbatim request is never replaced.** A paraphrase that adds a clause
becomes the criterion the model solves for, and the wrong criterion is solved
perfectly.

## In the TUI

The user must be able to see that something is happening on their behalf, and
must get the payoff when it lands. Pane already has the machinery: `Activity`
frames are **four ASCII characters, four frames** (`tui.rs:176`),
`completion_tick` already drives a completion animation (`tui.rs:612-640`), and
`/motion on|off` already exists.

### The lane — one line per running helper, under the cell header

```
  cell 3                                             executing  |==>
    scout     ---.  "where is rate limiting handled"        3.2s
    reducer   --.   cargo build log · 4118 lines            0.9s
```

Frames `-.  ` → `--. ` → `---.` → `.---`: a line reaching out, and the dot
coming back on the fourth. One frame set for every helper; the role and its
`verb` carry the difference, so a new spec renders correctly with no UI change.

On resolve the glyph flips to the existing `Complete`/`Failed` frames and the
text becomes what the caller *got*, then the lane folds into the header:

```
    scout     OK    6 of 29 served · sel-1                  3.4s
  cell 3  · 2 helpers · +6 files, 3 errors           executing  |==>
```

Four rules:

| Rule | Why |
|---|---|
| **No lane under ~300ms** — show only the folded summary | A lane that appears and vanishes on every cheap call is the banner nobody reads |
| **A failed helper shows ` !! ` with its reason and does not fold** | The supervisor shipped for weeks rendering a permanently failing look as a healthy one. Do not rebuild that |
| **Elapsed is text, not animation** | Under `/motion off` the glyph freezes and the seconds keep counting, because *is it alive* is the question being answered |
| **Past three lanes, collapse to `3 helpers · 4.1s`** | Helpers run in parallel; the cell must not be pushed off screen |

### `/cell` — what they actually did

The inspector gains one section, in its existing vocabulary:

```
HELPERS · what was asked and what came back
  scout · 3.4s · 4 turns · 12.4k tokens · read glob grep context
    asked    "where is rate limiting handled"
    looked   glob **/*.rs → 412 · grep ratelimit → 38 · read ×9
    served   6 files in full
               provider/telemetry/mod.rs
               gateway/ingress.rs
               … 4 more
    record   sel-1 · 29 considered, 23 not served, with reasons

  reducer · 1.1s · 1 turn · 3.1k tokens · no tools
    asked    cargo build log, 4118 lines
    gave     3 distinct root failures
    record   log-7 · full 4118-line output
```

`looked` is what earns the section: it is the helper's own trajectory, so a bad
selection is debuggable instead of mysterious. `record` names the handle, so
*"why was my file not served?"* is one retrieval away. Turns, tokens and the
toolset actually held are shown, so a Scout that burned eight turns to serve two
files is visible as a bad call rather than hidden behind an `OK`.

**The folded summary persists in scrollback.** It is one line and it is
evidence.

### The one field this adds to the contract

```rust
pub struct HelperSpec {
    …
    /// Present participle for the lane: "scanning", "reducing", "checking".
    pub verb: &'static str,
}
```

Everything else — glyph, timing, fold, failure rendering, the inspector
section — is shared. **A new helper gets a working UI for free**, which is the
extension property the rest of this spec is built on.

## Cost rules

**Deflation is a context-window mechanism, never a cost saving.** At Luna's
rates, keeping a cached 20k-token file costs $0.0004; deflating it invalidates
everything after it and costs about $0.0135 — **34× more to save four
hundredths of a cent.** Deflate only under window pressure or measured
attention harm, at a compaction boundary you are already paying for. Deflate to
a *marker*, never to nothing, so a prior turn's reference does not dangle.

**A per-helper-type cache already exists and needs no design.** `agent::run`
builds its system block from `render_system(instructions, tools, facts)` plus
`orientation::collect(profile)` — deterministic given the same profile. Two
Scouts in a session share a byte-identical prefix; a Scout and a Reducer differ
because their tool declarations differ, so they land in separate entries
automatically.

**Lifting a helper's findings into a shared prefix is deliberately not
specified.** It needs a session store, deterministic ordering, digest
invalidation and eviction, and none of the three questions that decide whether
it pays — how often helpers fire, whether the ~5-minute TTL survives between
them, whether Scouts re-read the same files — can be answered before Scouts
exist. Pane already meters `cache_read` and `cache_creation` separately, so the
data will be there. Revisit then.

## The helper tier

Not a pinned model but **any model meeting seven criteria**, so a model swaps
without touching a spec.

| # | Criterion | Why |
|---|---|---|
| 1 | Context ≥ 1M tokens | A Scout must consider every candidate at once |
| 2 | Declares structured output | Helpers emit records, not prose |
| 3 | `output_modalities == ["text"]` | Also what makes a stated zero price meaningful rather than a per-second billing artefact |
| 4 | ≤ $0.25 per M input | The tier is defined against a subscription window, not against zero |
| 5 | Publishes a cached-input rate | Helpers re-send the same system block constantly |
| 6 | Coding index ≥ 65 where published | The one quality floor; helpers neither plan nor act |
| 7 | **Proven available by a live call** | A catalogue entry is not availability |

**Criterion 7 outranks the rest.** A helper that throttles mid-session does not
relieve its caller, it strands it. Admission requires a measured `200`, revoked
by sustained refusal.

### Measured 2026-09-08 — probed, not read from documentation

| model | ctx | $/M in | cached | code | measured |
|---|---|---|---|---|---|
| `gpt-5.6-luna` (Experiential) | 1.05M | 0.20 list | 0.02 | 71.4 | `200`, `cost: 0.0`, 80 tok/s on a 242-token reply |
| `glm-5.3-flash` (Z.ai) | 1.31M | 0.075 | 0.015 | 71.5 | `200` on a live key |
| `deepseek-v4-flash-0731` (Experiential) | 1.31M | 0.065 | 0.018 | 69.1 | `200`, `cost: 4e-06` |
| `gemma-4-26b-a4b-it-free` | 262k | 0 | – | 39.3 | **`429` on the first request — fails 7** |
| `minimax-m3-free` | 1.05M | 0 | – | 58.6 | **`502` — fails 7** |

**Cheap and available beats free and throttled.**

## What this costs to build

`agent::run` (`agent.rs:70`) already takes a `&Profile`, clamps `options.turns`
against `MAX_TURNS`, returns `{status, turns, tokens}`, and forbids a subagent
starting a subagent. Two changes make it the helper runtime:

1. `agent.rs:78` — take the tool list from options instead of `registry::ALL`.
2. `AgentOptions` — allow a model override, so a helper runs on the tier.

Everything else in this spec is a const table and a binding.

## Build and measure in this order

1. **REDUCER, with the lane and the inspector section.** `tools: []`, one turn
   — it cannot touch anything, and its request is `[system][blob]`, so no prefix
   or deflation question arises. It proves the spec, the runtime, the handle
   rule and evidence-never-conclusions in the smallest possible package, and
   pays immediately on a 4,000-line log. **The UI ships with it, not after**:
   the lane and the `HELPERS` section are shared machinery, so building them
   against one trivial helper is what proves a later spec renders for free — and
   until they exist, nobody can see whether a helper ran at all.
2. **The preflight format measurement**, before any Scout ships. Three matched
   ruler arms on a frozen prompt: no preflight · record behind a handle · record
   inline as sections. Score **the first act of the first turn** and
   requests-to-first-edit, not tokens. The format above is reasoning about what
   a model attends to, and this project does not let that stand unmeasured. **If
   the inline arm shows no drift, the handle rule is unnecessary complexity and
   should be deleted rather than defended.**
3. **SCOUT**, on whichever format won, with no shared findings prefix.
4. **CHECKER** last — it is the only one whose value depends on the other two.

Helpers are judged on whether the caller ends up **right more often**, never on
tokens saved. The matched benchmark had pane at 247k tokens against a control's
372k–444k while losing 4/6 on correctness; a helper that only makes a wrong
answer cheaper has closed nothing.

## Split subscriptions — the helper tier's real economics

**The problem.** Anthropic has nothing in the helper tier. Measured 2026-09-08:
`claude-haiku-4.5` is 1.00/5.00 per M at coding 43.9 with a 200k window, against
`gpt-5.6-luna` at 0.20/1.20 and coding 71.4 with 1.05M, and `glm-5.3-flash` at
0.075/0.250 and coding 71.5 with 1.31M. So a Claude-subscription session has no
cheap helper worth using, and the tier's first criterion — context ≥ 1M — rules
Haiku out on its own.

**What is needed: the task model on one subscription, the helper on another.**

**Why it does not work today, in two places.**

1. **pane sends one endpoint.** `wire::send_turn_bounded` and `send_turn_with`
   both use the global `base_url()` and `credential_header()`. `[helpers] model`
   changes only the model *name* in the body, so a helper request goes to the
   same host with the same credential as a task turn.
2. **The gateway pins one entitlement per session.** Its own routing output says
   *"entitlement `claude-max` … will serve this session"*, and
   `entitlements --json` marks the others unselectable with *"current gateway
   route is pinned to X"*.

**The hook already exists.** Every helper request is stamped
`x-glasshouse-purpose: helper` — the seam `supervisor.rs` introduced and this
spec reuses. That header is exactly what a per-purpose route keys on: *task
turns to the subscription, helper turns to the cheap tier.* The gateway already
translates protocols and already knows several entitlements; what is missing is
routing on **purpose** rather than pinning per session.

**This is a Glasshouse change, not a pane one**, and it is what makes the
architecture pay: without it, the only pairings that work are ones where the big
model and the cheap model live behind the same endpoint —
`gpt-5.6-sol` + `gpt-5.6-luna`, or any Gemini + `gemini-3.8-flash`.

**Not decided here:** whether the route keys on the purpose header alone, on the
model name, or on an explicit `[helpers] entitlement`; and whether a helper may
run on an entitlement the session itself is not pinned to, which is a spend
question as much as a routing one.

## Open, not decided here

Whether one tier model serves all three specs or each names its own. Whether
CHECKER may run one named command or only read what has already been produced.
Whether SCOUT's budget is a token count or a file count.
