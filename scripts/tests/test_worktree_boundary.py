"""The worktree-boundary guard: a worker may not edit outside its own worktree.

Batch 47 lost thirteen minutes of a worker's work to the main checkout and
found it by accident. These lock the guard's four decisions: block the escape,
allow the worktree, allow the one report exception, and never restrict the
orchestrator.
"""

import json
import pathlib
import subprocess

HOOK = pathlib.Path(__file__).resolve().parents[1] / "hooks" / "guard-worktree-boundary.sh"
REPO = "/Users/eneas/projects/glasshouse"
WT = f"{REPO}/.worktrees/session-record"


def run(tool, file_path, cwd):
    payload = json.dumps(
        {"tool_name": tool, "cwd": cwd, "tool_input": {"file_path": file_path}}
    )
    return subprocess.run(
        [str(HOOK)], input=payload, capture_output=True, text=True
    ).returncode


def test_worker_editing_the_main_checkout_is_blocked():
    assert run("Edit", f"{REPO}/crates/glasshouse/src/session/store.rs", WT) == 2


def test_worker_editing_its_own_worktree_is_allowed_absolute_and_relative():
    assert run("Edit", f"{WT}/crates/glasshouse/src/session/store.rs", WT) == 0
    assert run("Edit", "crates/glasshouse/src/session/store.rs", WT) == 0


def test_worker_may_write_its_report_to_the_main_checkout():
    """The one exception, and it is load-bearing: the watch reads that path."""
    assert run("Write", f"{REPO}/.agent-runtime/report-session-record.md", WT) == 0


def test_the_report_exception_does_not_extend_to_packets_or_the_checkpoint():
    assert run("Write", f"{REPO}/.agent-runtime/packet-session-record.md", WT) == 2
    assert run("Write", f"{REPO}/.agent-runtime/CONTINUATION.md", WT) == 2


def test_a_worker_may_not_edit_another_workers_worktree():
    assert run("Edit", f"{REPO}/.worktrees/mem-settings/src/x.rs", WT) == 2


def test_the_orchestrator_in_the_main_checkout_is_never_restricted():
    assert run("Edit", f"{REPO}/crates/glasshouse/src/main.rs", REPO) == 0
    assert run("Write", f"{REPO}/docs/product/capability-map.md", REPO) == 0


def test_non_edit_tools_are_ignored():
    payload = json.dumps(
        {"tool_name": "Bash", "cwd": WT, "tool_input": {"command": "ls"}}
    )
    assert (
        subprocess.run([str(HOOK)], input=payload, capture_output=True, text=True).returncode
        == 0
    )


def test_the_block_explains_the_absolute_path_trap_that_caused_it():
    payload = json.dumps(
        {
            "tool_name": "Edit",
            "cwd": WT,
            "tool_input": {"file_path": f"{REPO}/crates/glasshouse/src/main.rs"},
        }
    )
    done = subprocess.run([str(HOOK)], input=payload, capture_output=True, text=True)
    assert "READING ONLY" in done.stderr
    assert "REPORT TO" in done.stderr
