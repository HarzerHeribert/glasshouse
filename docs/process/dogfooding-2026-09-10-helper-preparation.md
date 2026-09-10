# Deterministic helper preparation and usability repairs

Status: implementation in progress. Primary integration, installation and real
subscription evidence are still required. No capability boxes are closed.

## User objective and target state

Complete tasks correctly without repeated human correction, then reduce the
model/cache-weighted cost of that complete work. Total tokens include helpers;
time is secondary. The previous installed expense task passed nine local and
16 independent cases without intervention, but its Scout consumed four model
turns and the parent repeated successful tests. This is an observed harness
improvement target, not evidence that Pane has reached established harnesses'
reliability or cost at equal quality.

Before each helper's first model request, the host should prepare relevant,
bounded evidence. It must not make the model discover basic project structure
or pay another model to extract mechanically recognizable failure locations.
Preparation respects the compiled parent profile, cancellation, ignore rules,
resource bounds and the helper's existing read-only capability boundary.

Proposed starting packets:

| Role | Deterministic candidates | First implementation target |
|---|---|---|
| Scout | Pruned tree; language/test manifests; changed paths; task-term matches | Bounded project map and relevant entrypoints, with explicit omissions |
| Checker | Named tests; optional separately configured lint/type checks; changed-file evidence; original acceptance criteria | Actual named verification observations plus bounded original-contract/change evidence before the model request |
| Reducer | Exit status and command; failing-test identities; error windows; repetition counts | Bounded mechanical failure extraction from the supplied real output; no repository walk |

A passing test is not an automatic checker approval. The checker still judges
whether the implementation satisfies the original request and names unchecked
areas. Prepared content is evidence, never instructions that replace the task.
Build outputs, vendor trees, dependency directories and very large inputs must
not become broad context dumps.

## Named verification primitive

The primary implemented `.glasshouse/checks.toml` and `checks.list()` /
`checks.run(name, force?)`. A configured name identifies a fixed command; this
configuration confers no command or filesystem permission. Execution uses the
existing confined Bash invocation and caller cancellation token. Helpers do
not receive the effectful `checks` global: checker preparation is host-owned
and uses the parent profile before handing observations to the read-only model.

Example configuration for the existing Python fixture:

```toml
checker = ["tests"]

[checks.tests]
command = "/opt/homebrew/bin/python3 -m unittest discover -s tests -v"
inputs = ["expense_report", "tests", "fixtures", "README.md"]
reuse = true
```

Reuse is opt-in, request-local, and limited to successful checks whose declared
input snapshot and process environment remain unchanged before and after the
run. File contents and path identities contribute to the fingerprint. Missing,
denied, symlinked, oversized or incomplete inputs cannot establish reuse.
The snapshot work itself is bounded; declaring the whole repository may simply
disable reuse. Tests dependent on undeclared external state or nondeterminism
must use `force=true` or leave reuse disabled. A reused observation retains its
original time and says `executed=false, reused=true`; it never claims a fresh
execution. Failures and task boundaries invalidate cached evidence.

## Parallel packets

- Sol editor: profile-aware shell quick-open using actual shared CLI serving
  behavior, including backend/gateway lifetime and credential isolation.
- Sol editor: parent plus helper usage accounting, with model/cache scope and
  unknown usage coverage, never a token stopping rule.
- Sol editor: deterministic helper-context module, with pruned and bounded
  traversal and role-specific evidence.
- Primary: named verification, helper wiring, integration and independent
  installed-task acceptance.

Workers use isolated worktrees with disjoint file ownership. Their reports,
review corrections, tests and real trial outcomes will be recorded here before
completion. UI desktop validation remains dependent on controller access;
terminal execution and fixture tests do not substitute for physical mouse proof.

## cmux input evidence, 2026-09-10

The user explicitly authorized cmux API testing. The primary created a separate
window (`F8A5F1BF-9C0A-4039-A0C4-1E6E1F157621`), workspace
`3827ADF0-DBA6-4880-89BF-C572A3174E76`, surface
`BAF2717B-6498-4A1A-AD58-4E503EA48E22`, titled Pane API dogfood.
The user's original working window was not used.

Installed b2c38db observations:

- Native quick-open Pane immediately accepted `FOCUS_PROBE_äλ`.
- Ctrl-6 as an API control byte switched the header/session focus hint and
  restored typing with the draft intact. Ctrl-5 returned to host controls.
- Desktop-control click on the embedded composer from host controls restored
  session focus; `_CLICK` appended to the draft.
- The primary entered native fullscreen through the exposed window button,
  typed `_FULLSCREEN`, returned to host controls, then clicked the composer
  and typed `_REFOCUSED`. The complete draft was visible in the screenshot
  and cmux read-screen capture. No model task was submitted in this probe.

Artifacts: `.agent-runtime/input-fixes/cmux-api-launch-focus.txt`,
`cmux-api-control-byte6.txt`, and `cmux-api-fullscreen-focus.txt`.
The API rejects `ctrl+6`, `ctrl-6` and `F12` key names in this installed cmux;
raw control-byte delivery worked. This is not a Glasshouse key-routing failure
and does not prove a physical keyboard layout. Terminal, Ghostty and Warp
remained unavailable because automatic UI approval review rejected app access;
no alternative access path to those apps was attempted. Shift-drag/clipboard
behavior has not been independently verified.

## Integration review findings

The Sol named-check review found no required production correction, but
reproduced a flaky local-provider test: accepted sockets inherited nonblocking
mode. The primary reset accepted sockets to blocking with a read timeout.
Production cancellation is also rechecked before returning reused evidence
and after execution. The complete integrated Pane suite passes.

Primary review tightened seed reads to bounded ordinary-file reads and
conservative omission when ignore semantics cannot be read completely. The
supported ignore subset remains intentionally limited; unsupported rules
omit their scope and are reported. It is not a complete Git ignore engine.

The next installed trial uses a fresh clone of the same broken expense fixture,
with the original test suite and frozen external 16-case oracle unchanged.
Its oracle baseline fails as expected. Six hundred ignored generated files
and an oversized ignored file exercise pruning. The prompt uses named checks
in place of reconstructing a verification script. This is a controlled
follow-up, not a new independent benchmark or proof of price parity.

Additional integration review caught unbounded term extraction from a very large
helper request. Scout and Checker now derive terms from the bounded prefix; a
1 MiB tail-sentinel regression proves later terms are not scanned. Sol re-review
ACCEPT and all 11 focused context/first-request tests pass. The provider socket
correction also passes 20 consecutive first-request test runs.

The first combined Glasshouse library gate passed 2,332 tests, ignored one,
and failed the existing 64-caller bootstrap stress test with SQLite CANTOPEN.
Its exact isolated rerun passes. The process soft descriptor limit is 256;
resource-limit attribution is being checked before the final gate. This first
run is not reported as green. The file-size gate also rejected shell/mod.rs at
2,538 production lines, prompting a dispatch extraction before installation.

The 4096-descriptor full macOS gate passes the 2,333-test Glasshouse library
and 116-test command target, supporting the resource-load diagnosis. Its PTY
target then passes 79 of 80 tests and exposes a real integration regression:
`resizing_the_shell_reaches_the_harness_terminal` starts a bare `/bin/sh` as
Native Codex, but the new profile resolver adds approval argv and the process
exits with usage. The requested correction preserves the synthesized Native
quick-open argv rather than weakening the resize test. Explicit configured
profiles retain their shared CLI approval behavior. This gate is not green;
correction and follow-through are pending.

The Native argv correction is integrated: only the reserved synthesized Native
profile skips the configured overlay; named profiles still use the complete
shared resolution path. The worker passes the exact resize regression, all
80 PTY tests, 415 shell tests, check, Clippy and formatting. Primary formatting
and file-size checks pass after integration; the corrected full Glasshouse
workspace test run is in progress. The initial failed CI log is retained.

## Final pre-install gate

The corrected full `cargo test --locked --workspace --exclude pane` run exits
0 under the declared Rust 1.98.0 and descriptor limit4096, with inherited
provider variables scrubbed. All 80 PTY cases pass, including the unchanged
resize regression; every integration target skipped by the first failure now
runs. The full Pane CI step also passes (922 summed passing test executions,
including nested fixture probes, one ignored). Formatting and file-size checks
pass. The initial macOS CI invocation remains recorded as failed; its other
steps passed, including documentation, script tests and Rust 1.88 MSRV.
Artifacts: `helper-preparation-ci-macos.log`,
`helper-preparation-corrected-workspace.log`. No Windows/Linux claim is made.

Final integrated workspace Clippy (both crates, all targets, warnings denied)
passes. The corrected Glasshouse workspace log sums to 4,244 passed and six
ignored across 168 test summaries; nested probes are included in that sum.

## Installed v3 trial and observed completion correction

Both release binaries from `9f3a129` installed as
`v0.1.0-pre.1-130-g9f3a129`; their SHA-256 values match the immutable manifest.
The primary used the new visible quick-open profile picker in the dedicated
cmux window, selecting subscription-openai-pane (gpt-5.6-sol). Embedded Pane
accepted the complete 1,219-byte prompt with no extra focus step and kept the
submitted prompt visible. The real session is `cb726e661965`, Pane `pane-67138`.
The footer reports effort auto; this is a setting, not evidence of hidden reasoning.

The task completed in seven cells with zero operator corrections. Ten project
tests pass, and all 16 unchanged independent oracle cases pass. Baseline hashes
prove only four implementation files and tests/test_report.py changed; AST
comparison proves all five original test methods unchanged. The actual September
EUR CLI returns 1,029 cents, three transactions, food 1,000 and “travel, train” 29.

The Scout used two Luna requests (previous v2 used four), with recorded bounded
preparation excluding build and node_modules. Parent glob still returned 625 paths
before the model filtered them; bounded helper preparation does not fix broad
parent glob selection. The live helper lane, partial-cache coverage and combined
parent/helper spend were visible. Exact ledger reconciliation is pending.

Three recovered cell errors remain recorded: unsupported TypeScript `as const`,
invalid JavaScript string quoting, and denied git diff. The model recovered
through permitted source context without changing permissions or asking the user.

The final answer exposed a real sequencing defect: it appended a checker verdict
`cannot tell` then asserted no unresolved limitations, because the return text
was composed in the same cell before the checker outcome reached the parent
model. Two Sol workers are correcting that handoff and the checker's diff-only
evidence assumptions. Code acceptance passes; harness completion behavior is
not treated as flawless. Final installed replay is required after correction.

### v3 measured accounting and usability

| Scope | Requests | Input | Output | Reported cache read | Known total |
|---|---:|---:|---:|---:|---:|
| Sol parent | 7 | 54,251 | 8,102 | 22,400 | 84,753 |
| Luna helpers | 4 | 21,088 | 1,424 | 11,264 | 33,776 |
| Combined | 11 | 75,339 | 9,526 | 33,664 | 118,529 |

Input/output coverage is 11/11; cached input is reported on 7/11 requests. Helper
cache creation is unreported on all four helper responses. All monetary fields
are null. These are exact known sums with partial overall coverage, not complete
usage or a monetary cost estimate. Two persisted helper calls cover all four
helper ledger requests once. The final UI matches at 118.5k total, 84.8k parent,
33.8k helpers and explicitly reports partial coverage. The final persisted cell
feedback predates adding that cell's helper usage; its 95,853 subtotal plus 22,676
Checker tokens equals the final notebook/ledger 118,529.

The persisted operator-prompt-to-final-answer interval is 309.300 seconds, matching
the UI 309.3 s; the second-resolution provider span is 310 s. There was one exact
operator prompt; the seven other protocol-user rows are runtime cell feedback.
The later external observation timestamp is an upper bound, not task wall time.

The previous v2 trial recorded 201,801 known tokens, 13 cells and 361.7 s. This v3
trial recorded 118,529, 7 cells and 309.3 s. Setup/prompt/cache differences and the
observed completion flaw preclude a controlled savings or parity claim.

Installed 9f3a129 also passed standalone composer launch/Unicode typing through
the cmux API, and actual desktop click-to-focus windowed and fullscreen. A
non-submitted draft survived focus/size transitions and was cleared afterward.
The standalone probe made no provider request and added no rollout session.
Desktop typeText delivered the umlaut but not the requested lambda; Unicode
lambda input is established through cmux API, not that desktop typing call.
Shift-drag/clipboard behavior remains unverified.

The completion correction integrates the shared parent handoff in both agent
and session paths, plus evidence-based checker instructions. Primary review
also relabels non-diff supplied packets as Supplied rather than Diff, and
serializes both environment-sensitive provider fixtures with their existing
lock. Focused evidence tests pass; final scoped gate and installed replay follow.

Final correction gate: `scripts/ci-local.sh --scoped` exits0, all traced targets
and full Pane suite pass (925 summed passing executions including nested probes,
one ignored). Pane all-target Clippy with warnings denied also passes. The
scoped gate includes formatting, docs, size, scripts and Glasshouse Rust1.88
MSRV. Its nested Glasshouse Windows-GNU compile check passed; no Pane Windows
compile or Windows execution is claimed. The earlier full Glasshouse workspace
remains the unmodified-host behavior evidence. No push performed.
