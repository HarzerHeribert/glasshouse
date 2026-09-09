# Glasshouse / Pane input dogfooding — 2026-09-09

Scope: the user's reported inability to type or click in a newly launched
harness, including fullscreen. This is a bug report, not capability closure
or capability proof. The initial investigation changed no product code or
account settings and submitted no model task. The user subsequently authorized
fixing the findings with GPT-5.6 Sol subagents.

## Target state — approved repair scope

1. Given a newly launched embedded harness, its composer accepts typing at
   once, while headless launches, failures and external sessions retain their
   distinct behavior. Clicking a visible harness viewport enters it; Ctrl+5
   returns to Glasshouse and Ctrl+6 can focus its header.
2. Given a focused embedded harness, paste arrives intact as input, including
   multiline and Unicode text, without invoking Glasshouse shortcuts or
   silently submitting a task. Respect the child's bracketed-paste mode.
3. Given an embedded child requesting mouse input, viewport events reach it
   in its requested encoding and relative coordinates; Glasshouse controls
   and overlays retain their events. Events outside the child never activate
   an unrelated child control. Native Shift-drag selection remains available.
4. Given Pane's model picker, clicking a provider selects that provider;
   clicking a visible model selects its row. Enter applies the selected
   model. A click alone never changes the serving model. Keyboard behavior
   and narrow-screen hit targets remain consistent with what is drawn.
5. Given a shell in a project, bare `pane` starts the ordinary session UI.
   `pane --help` gives useful usage; unknown arguments give a clear error.
   Existing `session`, `ruler` and version entry points continue working.
6. Explain terminal text-selection modifiers concisely where useful rather
   than disabling mouse support globally. Shift selection is expected in
   mouse-aware TUIs, not a defect that requires removing their mouse controls.

Implementation ownership: one isolated GPT-5.6 Sol worker for
`crates/glasshouse/**`, one for `crates/pane/**`; primary owns integration,
records, the integrated checks and live replay. No new capability checkboxes
are needed for these corrections. Native Apple Terminal/Ghostty GUI proof
remains unavailable unless the computer-control restriction changes.

## Environment and method

- Installed Glasshouse and Pane both print `0.1.0`.
- macOS 26.5; cmux 0.64.22 (installed app bundle version).
- Both PATH entries point through
  `~/.local/lib/glasshouse/current`, whose target was
  `versions/v0.1.0-pre.1-125-gea495f6`.
- Source checkout HEAD: `ea495f6fd7c02f8aae74d383793ad48133e299a6`;
  working tree clean before this report.
- Fresh Git projects:
  `/private/tmp/glasshouse-pane-input-20260909/repo` and
  `/private/tmp/glasshouse-pane-input-20260909/repo-second-window`.
- Actual keyboard and mouse interaction through the computer-control tool,
  with screenshots checked after each significant action. Screenshots are
  in the conversation's tool results, not saved as report attachments.
- First tested in a new workspace of the existing cmux window. At the user's
  request, continued in their separate test window, leaving their working
  window separate.
- Native-terminal comparison could not be run: the computer-control tool
  refused both `com.apple.Terminal` and `com.mitchellh.ghostty` with
  "Computer Use is not allowed to use the app ... for safety reasons."
  Standalone Pane below still runs in cmux; it is not a non-cmux test.

## Findings

### INPUT-01 — Launch looks ready to type, but keyboard remains with Glasshouse

Priority: high usability blocker. Reproduced in both fresh projects.

1. Run `glasshouse` in a fresh Git repo.
2. Press `n`, select `pane`, and start it. Both keyboard selection and clicking
   Pane in Glasshouse's harness picker successfully launched it.
3. Pane visibly shows READY and a `message or / for commands` input area.
4. Type without first pressing another Enter, or click that input and type.

Observed: input does not enter Pane. Typing `cmux-before-enter` eventually
opened Glasshouse's new-session picker: letters were interpreted as control
shortcuts. After Escape, clicking the composer and typing `xyz` left it empty.
The session was alive throughout.

Expected: a newly launched, visibly selected interactive harness is ready for
input, or focus ownership is unmistakable and clicking its composer enters it.

Verified recovery: press **Enter once** from Glasshouse control mode. Then
`cmux-typing-123` appeared in Pane, and `/model` opened its native model picker.
The launch status does say `Enter to type in it`, but the user independently
reported never discovering how to focus the harness.

Source: `crates/glasshouse/src/shell/mod.rs:199` deliberately separates start
from focus; it selects the session and sets the Enter hint without entering
session mode. `state/mod.rs:1251` maps Enter or `i` to entering the session.
This is an observed usability failure of an intentional interaction, not a
claim that the child process hung.

### INPUT-02 — Glasshouse consumes mouse events instead of delivering them to the harness

Priority: high for native harness mouse interactions. Source-confirmed;
composer click failure observed in INPUT-01.

The production `Event::Mouse` arm in
`crates/glasshouse/src/shell/mod.rs:654` handles only left-button-down on a
Glasshouse hotspot, rewriting it into Glasshouse key presses. All other mouse
events are discarded. There is no PTY mouse forwarding in that arm.

This explains why clicking inside the viewport cannot transfer focus and
prevents a child harness from receiving its own mouse events. Glasshouse's
own picker clicks worked in the same window, so the observed failure is not
that cmux cannot report a click at all.

Do not use Pane's model-tab failure alone as proof of forwarding failure:
INPUT-03 shows that Pane itself also lacks that click behavior.

Acceptance for a future fix: clicking a harness viewport should follow a
defined focus policy; when the child requests mouse reporting, forward
supported events with viewport-relative coordinates and the child's encoding.
Verify clicks and scrolling in both standalone and embedded paths without
letting Glasshouse chrome clicks leak into the child.

### INPUT-03 — Pane's provider tabs look interactive but ignore clicks, even standalone

Priority: medium usability gap. Reproduced embedded and standalone in cmux.

1. Enter Pane and submit `/model`.
2. Click another provider tab (OpenRouter in the embedded run; Google in the
   standalone run).

Observed: selected provider does not change. In the embedded run, pressing
Right immediately changed Claude to Google, proving the picker was responsive.

Source: `crates/pane/src/session/ui.rs:640` processes only ScrollUp/ScrollDown
in its mouse arm. Button-down events have no action. This is separate from
Glasshouse swallowing mouse input.

Workaround: Left/Right selects a provider; Up/Down selects a model; Escape
closes the picker. No model selection was applied during this test.

### INPUT-04 — Clipboard paste disappears in an explicitly focused embedded Pane

Priority: high input/data-entry defect. Paired GUI observation plus source.

Control: run
`pane session --root . --rollout ../standalone.jsonl` in the first test repo.
Paste `paste-control-012`: it visibly appears in the input. Clear with Ctrl+U.

Embedded: launch Pane from Glasshouse in `repo-second-window`, then press
Enter to enter the harness. Paste `embedded-paste-345`: the composer stays
empty. Type `typed-after-paste-678`: it immediately appears, establishing
that keyboard focus and the child process are still working.

Source: `crates/glasshouse/src/shell/mod.rs:663` has `Event::Paste(_) => {}`.
Pane's own `ui.rs` handles `Event::Paste(text)` by inserting it into the
editor or model-search field.

Measurement caveat: the automation paste call reported clipboard-read timeout
in both cases. Screenshots establish the different outcomes despite that
shared tool error. The source independently establishes the dropped event;
a human Cmd+V repeat is useful additional confirmation.

Acceptance for a future fix: paste must reach the focused child with the
appropriate bracketed-paste semantics, preserving multiline text as input
without unexpectedly submitting it or treating it as Glasshouse shortcuts.

### INPUT-05 — Bare `pane` and top-level `--help` do not discoverably start the harness

Priority: medium onboarding blocker. Bare-command GUI observation and source.

Running `pane` in a shell leaves an apparently empty terminal waiting for input.
It does not display the composer, usage, or a hint about `pane session`.
`pane --help` in the noninteractive inspection returned no help text.

Source: `crates/pane/src/main.rs:3` recognizes version, `ruler`, and `session`;
every other invocation falls through to `pane::echo_line`, which reads and
echoes one line and exits. This includes no arguments and `--help`.

Verified standalone launch:

```sh
pane session --root .
```

For this test an explicit `--rollout ../standalone.jsonl` kept its rollout
separate from the embedded run. The standalone composer appeared and immediately
accepted `standalone-input-789` without a Glasshouse focus step.
`crates/glasshouse/src/harness/pane.rs:67` supplies `session --root .` on the
user's behalf, explaining why launching from Glasshouse succeeds.

User corroboration: "I have never seen pane starting solo actually."

### INPUT-06 — Text selection requires Shift according to the user

Priority: medium interaction/discoverability finding. User-observed; not
independently reproduced with a modifier-drag by the automation.

The user reports that text can only be marked while holding Shift; Cmd has
not been tested. Preserve that distinction: no claim that Cmd works or fails.

Both programs request terminal mouse capture (`?1000h` and `?1006h`):
Glasshouse in `crates/glasshouse/src/shutdown.rs:66`, Pane in
`crates/pane/src/session/ui.rs:56`. This is consistent with ordinary dragging
being reserved for application mouse handling and a terminal modifier being
needed for text selection. Shift's exact behavior is the user's cmux
observation, not inferred from the Rust constants.

Mouse capture itself may be intentional, but its combination with INPUT-02
is poor: normal mouse interaction neither selects terminal text nor reaches
the harness. A future fix should retain an obvious selection path and verify
native selection, application mouse events and clipboard copy together.

## Fullscreen result and limits

After explicitly entering the harness, toggling macOS fullscreen with
Ctrl+Cmd+F, clicking the composer and typing `fullscreen-input-456` succeeded.
Restored the initial window mode afterward. Thus this run did **not** reproduce
a fullscreen-specific keyboard lock. It did reproduce the launch-focus and
mouse problems that can make the same screen feel locked.

Current shortcuts: Enter enters the selected session from Glasshouse controls;
Ctrl+5 returns to the full Glasshouse view; the header advertises Ctrl+6 for
header focus. Ctrl+6 behavior is source-backed here, not a separately completed
GUI test. Older Glasshouse fullscreen layouts were not tested.

Additional observation, not yet diagnosed: Pane showed `Glasshouse not connected`
even when launched inside Glasshouse. This may describe control-API availability
rather than parentage; do not file it as a confirmed integration defect yet.

The initial pass made no claim about model execution, provider authentication, long-running
stability, other harnesses, or Apple Terminal/Ghostty behavior. These tests isolate
local launch and input behavior without a provider request.

## Repair follow-up: subscription launch gap

Source review found a separate pre-existing spec gap: the TUI quick-open path
in `crates/glasshouse/src/shell/start.rs` constructs a Native launch profile
with the harness default model and no provider. It does not offer the user's
configured subscription profiles. Capability-map line 371 requires manual
profile selection; the CLI provides it through `launch --profile`, while this
TUI path does not. This is outside the input-routing corrections above.

All three configured subscription accounts report present. The authorized
real-task replay will use the existing `subscription-claude-pane`,
`subscription-gemini-pane`, or `subscription-openai-pane` profiles. A successful
CLI-profile task must not be presented as proof of the quick-open route. The
separate `Glasshouse not connected` banner still needs live diagnosis.

## Repair preview — integrated debug build

Before installing, replayed the integrated Glasshouse debug build with the
identical reviewed Pane worker build in the separate cmux window:

- `n` launched Pane and `immediate-input-123` appeared without another Enter.
- Clipboard paste of `paste-line-one\n雪-line-two` appeared as two composer
  lines, with no request sent. The automation still returned a clipboard-read
  timeout; the screenshot, not that return code, established delivery.
- F12 returned to full Glasshouse controls. Clicking the terminal viewport's
  accessibility target restored session mode, and `click-refocus-456` appeared.
- Fullscreen accepted `fullscreen-input-789`; `/exit` returned Glasshouse to
  control mode with the session marked stopped, and `q` restored the shell.
- Ctrl+5 and Ctrl+6 generated by the automation produced no visible transition.
  Ctrl+bracketright entered a printable character. F12 worked in the same run.
  This remains a keyboard-layout/automation observation, not proof of which
  bytes a physical shortcut delivers on this machine.
- Precise model-provider clicks could not be replayed: coordinate clicks were
  rejected by the computer-control tool with `windowNotFoundAtPosition`, even
  after raising the window and toggling fullscreen. The accessibility-target
  viewport click worked. Provider/model hit behavior retains unit and PTY
  evidence; do not call it GUI-verified from this preview.

## Integrated verification before installation

The first `RUST_TEST_THREADS=1 scripts/ci-local.sh --macos` run passed
formatting, Clippy, rustdoc, progress, secrets, documentation boundaries,
evidence checks, script tests, build and MSRV. Its failures were the shell
production-size ceiling and legacy tests requiring the previous launch/echo
behavior. These were corrected rather than weakening the assertions.

Follow-up evidence on the integrated checkout:

- File-size gate: no file over 2,500 production lines; formatting and diff
  checks clean; all-target workspace Clippy clean.
- Glasshouse library: 2,318 passed in the initial full run; final shell subset
  after extraction: 409 passed, one existing ignored test.
- Glasshouse PTY suite: 80/80 passed after correcting three old launch
  sequences. The remaining 58 integration targets were swept with
  `--no-fail-fast`; one analogous V1 session sequence was corrected and its
  complete seven-test target passed. Combined remaining-target outcome:
  669 passed, two existing ignored tests.
- Pane integrated entrypoint/session/live-TUI suites: 103/103 passed.
  Its worker's remaining-target sweep had one handler-panel PTY timeout;
  the exact rerun passed, as did the integrated 16-test live-TUI suite.
- Optimized release build of both binaries passed.

The initial gate log remains a failed run; the corrective checks above are
separate evidence, not a rewritten all-green gate result. No Windows or Linux
runtime claim is made. No capability checkbox was added or promoted.

## Installed release and real subscription tasks — 2026-09-10

Installed commit `f912bb9ef2ab6954bf1e0bac1be9bcae4ec75459` with
`scripts/install-local.sh`, release profile, as
`v0.1.0-pre.1-126-gf912bb9`. Both installed binary hashes match their manifest.
The previous installed version remains available. The installed bare `pane`
opened its composer, accepted `installed-standalone-123`, and exited cleanly
through `/exit`.

Used the user's existing subscription profiles, without changing account
settings or using a direct paid API route. Each task requested native file
tools: read `numbers.txt`, sum its integers, write `result.txt` as
`sum=<number>` plus newline, read it back, then answer. Each task's rollout
records exactly `read`, `write`, `read`, all successful. The primary checked
the actual file bytes independently.

| route/model | task | result | displayed elapsed / task tokens |
|---|---|---|---|
| Claude subscription / `claude-sonnet-4-6` | initial 3, 7, 11 fixture | exact `sum=21` plus newline | 8.2 s / about 11.2k |
| same Claude session | external addition of 13, then follow-up | re-read current file; exact `sum=34` plus newline | 6.1 s / about 12.4k |
| Gemini subscription / `gemini-3.8-flash-high` | fresh fixture | exact `sum=21` plus newline | 4.9 s / about 10.0k |
| OpenAI subscription / `gpt-5.6-sol` | fresh fixture | exact `sum=21` plus newline | 12.8 s / about 4.6k |

Observed against the runtime/model contracts: real tool calls and changes
were visible; structured diagnostic returns in the Claude and Gemini runs
appeared as notebook output and were followed by final prose; all four tasks
reached COMPLETE; usage was labeled reported; no shell/network tool ran;
external file changes were honored on the next task. Claude and Gemini exited
cleanly and Glasshouse lists them closed. The completed OpenAI session remains
open in the separate test window, with an empty composer after a successful
`post-task-input-ok` typing probe. Its active Glasshouse lifecycle describes a
live process, while Pane's task state is COMPLETE.

The launch banner initially said `Glasshouse not connected` on all three
subscription routes, then changed to `Glasshouse connected` during real work.
Thus the initial banner is a transient startup observation here, not proof of
a permanently disconnected parent/control path. Startup-message clarity remains
worth improving. These CLI-profile tasks do not close the separate TUI
quick-open profile-selection gap described above.

Live fixtures and rollouts remain under
`/private/tmp/glasshouse-input-fixed/{claude-task,gemini-task,openai-task}`.
Precise coordinate clicks and wheel replay were blocked by the computer-control
tool's `windowNotFoundAtPosition` error; native Terminal/Ghostty remain blocked.
The successful keyboard, clipboard and accessibility-target focus observations
must not be inflated into proof of those unavailable GUI paths.
