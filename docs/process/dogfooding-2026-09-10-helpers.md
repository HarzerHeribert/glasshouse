# Sol task model and Luna helpers — installed dogfood

Date: 2026-09-10 (Europe/Berlin). Status: **REPAIRS INSTALLED; FUNCTIONAL TRIALS PASS; DESKTOP RETEST BLOCKED**.

## Target state

The installed Glasshouse/Pane pair must complete a real repair in a separate
cmux window: Sol owns edits and verification, while Luna performs read-only
scouting, failure reduction and checking. The person can see activity, interrupt
an in-flight helper, type afterward, and inspect the completed helper evidence.
Explicitly admitted test commands must execute inside the existing OS sandbox.
The primary runs independent black-box checks after Pane reports completion.
No capability checkbox is closed by a partial demonstration.

The fixture is `/Users/eneas/projects/glasshouse-helper-demo`, a fresh Git repo
with a Python expense-report CLI, quoted CSV, exact cents, month/currency/status
filtering, refunds, clean errors and a five-test baseline. All five baseline
tests fail. An independent GPT-5.6 Sol reviewer supplied 15 black-box acceptance
cases; the unmodified fixture fails those too (18 assertions including subcases).
The oracle does not impose an unrequested top-level JSON key order.

Project configuration selects `gpt-5.6-luna` for all three helper roles, with a
four-call per-cell ceiling and silent completion. The task model is
`gpt-5.6-sol` through the existing OpenAI subscription profile. Pane currently
has one helper-model setting, not a numerical model-ratio setting. Configuration
alone is not evidence that the gateway used that model.

## First run: findings, not a completed repair

Build `f912bb9` / installed `v0.1.0-pre.1-126-gf912bb9`.

- Operator setup error: an explicit Bash-only settings document removed the
  default writable-file grant. The demo settings were corrected to read the
  repo and edit only source/tests. This requires a new session to take effect.
- `helper.find` (the Scout's actual callable name) completed in 29,150 ms,
  seven turns, with read/search evidence. `helper.reduce` completed in 1,720 ms,
  one turn. Both appear in completed view records. The original prompt called
  the Scout `helper.scout`; the task model found and used `helper.find`.
- **HELPER-01:** the submitted prompt disappears until preflight finishes (also
  reported independently by the person). Preflight spends roughly a minute in generic Thinking with no
  helper lane. Its token is not armed in the session interrupter; explicit cell
  helpers also use private cancellation tokens. An interrupt can surface much
  later as an unrelated read/glob `Cancelled`. This is not a glob permission
  rejection. The existing contract cancels one call and lets the task continue.
- **SANDBOX-01:** the admitted `/usr/bin/python3 -m unittest ...` command exits
  126 with `Operation not permitted`. The narrow grant lets the resolved shell
  execute but omits its Python child from the macOS executable literals.
  Adding `Read(**)` does not repair that executable grant.
- The person also reported no visible effort. The screenshot has a small
  `effort auto` footer; Auto leaves the wire effort unspecified, so an actual
  provider-selected effort cannot honestly be inferred from that label.
- **HELPER-02:** current routing observations stamp the session assignment
  model (Sol), rather than the request model; `helper` is absent from the
  client-purpose allowlist, and tool-holding helper turns omit its header.
  The request serializer and byte relay preserve Luna in the actual request
  body, but the ledger incorrectly says Sol/harness-turn. This is requested
  model evidence, not independent verification of the backend model identity.
- Tests also use temporary files: an in-project TMPDIR is necessary under the
  project-only writable-root policy. The demo uses `.pane/tmp`.
- Two actual SIGINTs 150 ms apart exit 130 and restore the terminal. Two CUA
  Ctrl-C key events issued together did not exit; the boolean input flag can
  coalesce events before its 20 ms poll. This separate edge is not yet repaired.
- Nested strings in console objects are sharply truncated. The first task
  spent repeated cells trying to expose README text. The second prompt asks
  for direct string evidence. This operator assistance is recorded explicitly.

The first task was stopped before it edited source. Raw local evidence is in
`.agent-runtime/input-fixes/helpers-first-run.json` and the baseline logs.

## Second run: project-scoped unattended mode

The initial source remains unchanged at restart. The new run uses `--yolo`,
`TMPDIR=<demo>/.pane/tmp` and `PYTHONDONTWRITEBYTECODE=1`, with its own rollout.
The Scout completed in 27,716 ms (three turns). The reducer then identified a
second platform boundary: `/usr/bin/python3` is an Apple developer shim, whose
`xcrun` cannot load Xcode libraries under `/Applications` in this sandbox. This
is distinct from the original missing child-executable grant. Homebrew Python
is available and will be tested without granting all of `/Applications`. The
person asked to let the run finish, so no corrective prompt was injected during
this attempt. Pane subsequently selected Homebrew Python from its environment
orientation without an operator follow-up, ran all 11 tests successfully, and
truthfully retained the Apple-shim failure. Its first Checker call exhausted
the three-turn limit; the task continued to handle that outcome.

## Observed coding-harness behavior

At 14 cells the task reported 140,756 tokens; by cell 20 it reported 256,537.
This is task telemetry, not a priced bill or a matched cross-harness benchmark.
Repeated context/edit round trips and retained handles make this run expensive.
The person suggested batching related implementation and tests in one cell;
that is a useful follow-up once the needed source has reached the model.

The second Checker completed but produced an unsupported requirement: it treated
“Require all five columns” as “exactly five columns.” The task model had already
introduced that stronger phrase in its question. It then accepted the finding
and added an extra-column rejection. This is a concrete example of correlated
requirements drift between task model, helper and self-written tests. Independent
checks and comparison with the original README remain necessary.

## Unattended operation and later web capability

The person wants ordinary work to proceed without personal approval prompts.
For the demo, existing `--yolo` is an interim supported mode: it grants project
edits and every command line while retaining OS confinement, the protected-path
set, and no network. It is not a fix for SANDBOX-01 and is not a model classifier.

Long-term target, not implemented in this batch: an unattended action policy
that permits routine repo work, records refusal reasons, and uses deterministic
rules before any small-model classification of ambiguous actions. A classifier
must choose among pre-authorized bounded capabilities; it is not authority to
invent new grants. Web search, fetching documentation and bounded crawling are
wanted capabilities without per-search prompts. A dedicated search/fetch route
(SearXNG or a CLI backend are candidates) can provide that separately from shell
networking. Backend selection and implementation are deferred until this repair,
build, installation and demo are complete, as the person requested.

## Evaluation priority clarified by the person

Correctness and completeness without repeated user corrections come first.
Second is cost per successful task, split by task/helper model and cache usage,
not total token count alone. Elapsed time matters after those; a 50% slowdown
can be acceptable if the result is complete and correct. The raw cumulative token figure
is insufficient to establish poor economics. This subscription run exposes no
per-token invoice, and the pre-fix model labels cannot support a weighted-cost
claim. Further trials must distinguish model-specific work and compact evidence
returned to the task model, then record how much user/operator correction was
needed. The person authorized changes to workflow ordering and behavior to make
Pane a dependable coding harness.


## Completed old-build repair and independent review

The initial repair ended after 23 cells, 565.7 seconds and approximately 348.2k
reported task tokens (323,426 was an interim cell reading). Its 12 self-tests
passed, and the primary's 15 independent black-box cases passed. These totals
are not a combined priced task/helper bill.

A primary follow-up corrected the unsupported extra-header prohibition by
quoting the original requirement. Pane removed it, kept duplicate/missing-header
validation and all five original tests, and finished with 13 passing self-tests.
This assisted correction took three cells and 77,493 reported tokens. The
primary reran the independent 15-case oracle afterward: all passed. The real
September fixture also yields 1,029 cents across three transactions (food 1,000;
travel,train 29). This is one operator-corrected result, not an autonomous
first-pass success.

## Reusable cell work

The person suggested named primitives for repeated code and test scripts.
Existing request-local bindings are the first implementation to evaluate;
reusing a runner must still execute the real tools and expose exit status and
output. A remembered successful result cannot stand in for a new run. Cross-
request scripts should remain inspectable project artifacts, and evidence must
continue to track the source version actually observed. No new primitive API
or token-saving claim is made before a working demonstration.


The existing async-function binding path passed a direct runtime test: define
`verify` once, invoke it from a later cell after changing an input, and observe
fresh tool output. Calling an unadmitted command through the same runner still
throws `PermissionDenied`. No new execution primitive was needed. The contract
now includes this recipe; live model adoption and token economics remain to be
measured separately.


## Integrated repair validation before installation

Three GPT-5.6 Sol workers supplied isolated sandbox, helper lifecycle and gateway
patches. The primary integrated them with the independently reviewed context
batching change. The full integrated Pane suite passes 890 tests with one
ignored; all-target Pane Clippy, formatting, file-size, document-boundary,
progress, secrets and evidence-consistency checks pass. The complete macOS gate
is still running before the installation commit.

Gateway review rejected the original malformed-JSON/string-retention scanner,
then caught JSON whitespace, incomplete-body attribution and escaped-name bound
issues. The final seven scanner tests include physical one-byte reads, exact
byte preservation, partial/erroring bodies, maximum escaped names and over 500
invalid mutations checked against a reference parser. Independent review accepts
the corrected patch. It records requested identity, not backend verification.

A Windows GNU Pane test compile was attempted but blocked before Pane compiled:
`v8 152.2.0` has no published `x86_64-pc-windows-gnu` binding artifact at the
requested URL (HTTP 404). This round therefore has no new Windows compile or
runtime proof. No stub or unconfined substitute was used.

The fresh v2 fixture uses the same broken source, but its setup and prompt are
improved: the README names the admitted Homebrew interpreter, project writes
and TMPDIR are available, preflight is explicitly off, and the prompt asks for
named-runner reuse and review against the original contract. This is a repaired-
path dogfood trial, not a matched timing or cost comparison with the old run.
Both its five original tests and the independent oracle fail before the trial.


The v2 independent oracle adds a sixteenth case for the observed extra-header
requirement drift. It is frozen before the new task: the broken v2 baseline
fails it, while the independently corrected first demo passes all 16. The
original 15-case oracle remains preserved separately.

Named runners reduce repeated code generation, but their original definitions
can remain in later request history and cached input. Savings must therefore
be measured across generated output, uncached/cached input and model tier;
source reuse is not automatically an elimination of all repeated input tokens.


The first complete macOS gate passed all static/script/build/Pane/MSRV steps,
but its Glasshouse test step stopped at five stale `gateway_first_events`
lookups. Those requests name `claude-x`; their queries still expected the session
assignment `fixture-model`. The worker audited gateway observation queries and
corrected six test files, preserving every timing, usage, route and outcome
assertion and the deliberate assignment fallback for an unsent body. Seven
focused targets pass all 43 tests. The complete Glasshouse workspace test step
is rerunning with `--no-fail-fast`; the original failed gate log is preserved.


### Final pre-install verdict

All required macOS validation components now pass. The complete non-Pane
workspace sweep covered 168 targets: 4,226 passed and 11 failed solely in
`relay_usage` and `routing_session_column`, which still had the old lookups in
the binaries that sweep started. The integrated expectation corrections then
passed all 17 active tests across those two targets and `task_class_cost_join`
(one measurement ignored). Thus all 4,237 active workspace tests have passing
final evidence; six workspace measurements remain ignored. Pane separately
passes all 890 tests with one ignored. Static/script/build/MSRV steps passed;
final formatting, staged-diff and secret checks passed too. This is a full
sweep plus exact corrected-target reruns, not a claim that the original
`ci-local` invocation exited successfully.

Desktop control later began returning `cgWindowNotFound` for cmux, including
after a controller reset. The primary requested the second window be brought
forward and continued independent work. Installation and a real scripted CLI
trial can proceed; a new live desktop observation remains pending availability.


### Installed rerun: autonomous contract completion

Installed `c010eea` (`v0.1.0-pre.1-127-gc010eea`) completed the fresh v2
expense repair in a real PTY through `subscription-openai-pane`, without
`--yolo` or an operator correction. The frozen task ran from 23:50:22 to
23:56:24 UTC on September 9 (361.704 seconds including launcher lifecycle),
using 13 cells. All five original tests remain intact; four meaningful cases
were added. The primary reran all nine tests and the separately frozen 16-case
black-box oracle: all pass. README, permission/helper configuration and launch
wrapper are unchanged. The actual September EUR fixture returns 1,029 cents,
three transactions, food 1,000 and `travel, train` 29.

The live ledger now separates requested identities: Sol has 13 harness-turn
requests, 118,751 input / 8,773 output tokens, with 46,336 cached input reported;
Luna has six helper requests, 20,329 input / 2,492 output, with 5,120 cached
input reported. All monetary cost fields are unavailable. These are provider
reports and requested model labels, not verified backend identities or an
invoice. They establish that helper attribution works; they do not establish
a two-thirds delegation ratio or a cost advantage from an unmatched trial.

Observed behavior: no automatic preflight preceded the task. Explicit find,
reduce (on a real syntax failure), and check executed; the checker received the
original contract and actual implementation. Five independent source contexts
were batched in one cell. A later cell applied the related implementation and
regression edits and ran the reusable verifier. After a self-corrected missing
comma in a new test, later cells called the same `verifyActual` identifier for
fresh commands, rather than regenerating its body.

There were recoverable harness/model inefficiencies. The 96-character nested
console string cap hid the README command and most helper evidence, causing
several recovery cells and an unadmitted `python3` guess before the model printed
the original contract directly. A block-local reducer binding was incorrectly
referenced in a later cell and recovered; verification was repeated after no
new edits. An unadmitted final Git status command was refused and honestly
reported. The model still completed without operator steering. The nested
console evidence defect is being fixed in a separate bounded Sol packet.

Artifacts: `.agent-runtime/input-fixes/helpers-v2-{run,ledger}.json`,
`helpers-v2-oracle-final.log`, and the demo's `.pane/installed-repair.jsonl`.
This is installed functional PTY evidence. The desktop controller still returns
`cgWindowNotFound`, so it does not prove new cmux mouse/focus behavior.


Independent Sol review accepts the completed v2 artifact and confirms the
original tests were preserved byte-for-byte. It also bounds the review claim:
`helper.check` received the behavioral contract but its README slice omitted
the configuration-preservation line and documented command. Independent Git
inspection and actual command records supply that evidence. The orientation
helper used four provider turns and nine inspections for this small repository;
this run does not establish that every delegation was economically worthwhile.


### Follow-up formatter and telemetry audit

A Sol high editor and independent Sol reviewer replace the nested 96-character
cap with the existing shared 24,576-character argument inspection budget. The
actual structured excerpt shape preserves all README text/metadata alongside
helper and command output. Traversal stops with an unvisited-member count;
a review-found exhausted-key recursion path was fixed before acceptance. The
final rendered cap marks its omission and true suffix instead of silently
cutting a prefix. All 13 bounded-excerpt tests and the aggregate console-cap
regression pass; independent review accepts. The complete integrated Pane suite
passes all 894 tests across 54 targets, with one ignored measurement. Formatting
and diff checks pass; focused Clippy passes with warnings denied. An inherited limit remains: V8 enumerates
ordinary-object property names before applying the display key limit, so this
is not a claim that every enumeration allocation is bounded.

A separate Sol read-only audit reconciles usage: 173,860 is the parent Sol
UI meter, including its separately reported cached input; 27,941 belongs to
Luna helpers. The full run is 201,801 reported tokens across 19 requests. The
Anthropic-compatible route normalizes input as excluding cached input, so this
addition does not double-count cache. The UI currently excludes helper spend:
one-shot helper usage is discarded and multi-turn helper totals are not
propagated into `TaskSpend`. This is an existing open Phase 64 telemetry item,
now backed by a concrete trial; it is not fixed in the console patch. No dollar
cost or billing savings can be established from these fields.


### Final installed formatter replay

`b2c38db` is installed as `v0.1.0-pre.1-128-gb2c38db` for both Glasshouse and
Pane. The installer built release artifacts, smoke-tested both version commands
and switched the current-version symlink. Both installed binary hashes were
checked against the manifest before the real subscription replay.

The read-only replay completed in 25.797 seconds, four cells, no thrown cells,
no `--yolo`, and no operator correction. Its first cell printed the actual
`{contract: readme.excerpt(...)}` shape. Primary JSON assertions compare every
numbered README line and metadata exactly: no omission, no truncated lines.
The next cell chose the documented absolute interpreter correctly, defined
`async verify()` once and executed it. A later cell called the same identifier
again. Both real commands exited zero and returned complete structured stderr
with nine tests and `OK`; the final cell asserted both results before returning.
The deliberate second execution was a binding/freshness test, not a recommended
extra verification step for ordinary coding tasks. Actual calls were read, bash,
bash; the model made no edit calls.

Artifacts: `.agent-runtime/input-fixes/console-{install.log,replay-run.json,
replay-verdict.json,replay-terminal.log}`, and the demo's
`.pane/console-replay.jsonl`. Full integrated Pane verification is 894 passed,
one ignored; all 13 focused excerpt regressions also passed independent review.
The earlier full workspace evidence remains applicable to unchanged Glasshouse
source. The working launch example is `glasshouse-helper-demo-v2/run-pane.sh`.

Remaining findings are explicit: no new desktop/cmux mouse-focus proof while
the controller returns `cgWindowNotFound`; helper usage is missing from the UI
task meter; quick-open still lacks the CLI profile-selection behavior; this
round has no Windows proof because the upstream V8 GNU artifact was unavailable.
An automatic permission classifier and web crawling remain future work, as
requested while prioritizing this harness repair. No capability box is closed.
