# Native workbench cutover — evidence, 2026-09-20

**State: implemented, targeted native checks pass; broader/platform acceptance is
not complete.** Base: `17904082d91a08b3d3da585f606ec62965c4cd25`. This is not a claim
that the browser mockup or the whole workspace has been fully implemented/verified.
The current contract is [workbench.md](../pane/workbench.md).

## Production path

The ordinary session renderer/event path now uses `workbench`, not the legacy TUI.
The document consumes real Conversation/Notebook/CellView/HelperRecord data. The
settings editor saves through Store; model changes use the existing session
control channel. `RuntimeState::agent_assignment` enforces explicit pinned/roster
model and roster effort before `agent.run` creates a background job. UI filtering
alone is not the policy. Existing approval/question/credential owners retain
precedence. The live model controls cannot activate unrelated saved-for-restart
configuration; the native settings boundary is exercised explicitly.

## Reproduction and observed results

Rust 1.98.0, Linux x86_64 root container, locked vendored dependencies and the
repository-pinned V8 library. No paid/model subscription calls. PTY tests use a
loopback scripted provider and spawn the real `pane` executable.

| Command / target | Observed |
|---|---|
| `cargo check -p pane --tests --locked` | pass |
| `cargo test -p pane --test workbench --test delegation_policy --test custom_agents --test settings_store --test config --locked` | 106 pass: 40 + 9 + 6 + 33 + 18 |
| `cargo test -p pane --test tui_live workbench_ --locked -- --test-threads=1` | 3 pass; 27 other legacy PTY tests not included |
| `cargo test -p pane --lib session::controls --locked` | 11 pass |
| `cargo test -p pane --test runtime_cells subagent --locked` | 4 pass |
| `cargo test -p pane --test tools the_agent_declaration_names_the_models_this_session_can_delegate_to --locked -- --exact` | 1 pass |
| `cargo fmt --all -- --check` / `git diff --check` | pass |
| `python3 scripts/check-file-sizes.py` | pass; the three pre-existing over-limit files did not grow |
| `cargo test -p pane --lib --locked` | 660 pass, 4 fail; same four failures reproduced on unchanged base (656 pass, 4 fail) |
| `cargo clippy -p pane --lib --tests --locked -- -D warnings` | not green; same baseline six production / ten including library-test findings reproduced; new module findings fixed |

The four reproduced library failures are
`session::tests::a_direct_provider_call_runs_and_answers_with_a_typed_result`,
`session::tests::independent_direct_calls_answer_individually_from_one_frame`,
`tools::invoke::search::tests::finding_5_an_unreadable_directory_keeps_the_matches_beside_the_error`,
and `tools::invoke::tests::a_bypassed_child_reads_what_the_sandbox_would_refuse`.
The host cannot provide the tested Landlock/seccomp boundary; root also bypasses
the unreadable-directory fixture. No sandbox code was weakened to make these pass.

A broader integration run was also attempted and was **not green**. Some fixtures
asserted superseded default inheritance, model overrides or old panel strings;
others require unavailable sandbox/build environment behavior. The explicit
custom-agent and selected runtime/declaration fixtures above were migrated and
retested. Remaining old UI and broader session fixtures still need review; they
are not all being attributed to the environment without a baseline comparison.

## Decisive mutation

Removed the roster's explicit model-versus-selected-slot mismatch check. The
native `roster_resolves_only_configured_slots_and_exact_models` test failed with
an unauthorized assignment, exit 101. Restored the original bytes, then ran all
nine delegation tests successfully. The actual V8 tests additionally assert
that forbidden model/effort choices leave the background-job count at zero.

## Boundaries still open

Exact provider/account/entitlement pinning and an enforced subscription-only or
no-API-spend fallback are **not** implemented. Catalogue account rows are labeled
availability evidence; routing stays gateway-owned. Favorites currently bind
model and effort. Side-by-side and working-tree/HEAD diffs are not implemented;
the native diff is a cell's captured before/after unified diff. The final answer
is the real model return, not a new answer-quality evaluator. Native Ghostty,
macOS and Windows acceptance and the remaining legacy regression migration are
required before treating this cutover as merge-ready. Existing legacy renderer
code is retained for DTO/test compatibility, not selected by the live UI.
