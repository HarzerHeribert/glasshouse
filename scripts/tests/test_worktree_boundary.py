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


def test_a_team_lead_may_write_a_subpacket_to_the_main_checkout():
    """A lead's output is subpackets, not only a report.

    Blocked in batch 49 the first time a lead decomposed a phase. Its
    subcontractors live in their own worktrees and must be able to read the
    subpacket, and the lead's own tree is deleted at close, which would take
    the record of what was delegated with it.
    """
    assert run("Write", f"{REPO}/.agent-runtime/subpacket-ceilings.md", WT) == 0


def test_the_subpacket_exception_does_not_extend_to_the_leads_own_packet():
    """A worker must not rewrite its own instructions."""
    assert run("Write", f"{REPO}/.agent-runtime/packet-lead-capacity.md", WT) == 2


if __name__ == "__main__":
    # Every other file here self-runs under `python3 <file>`, which is how
    # ci-local.sh's "script tests" step invokes them. This one was written in
    # pytest style with no entry point, so the gate ran it, saw exit 0, and
    # had executed nothing — found 2026-09-01 while adding a sibling. Ten
    # tests, green for five days, never once run by the gate.
    import sys

    failed = 0
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok   {name}")
            except AssertionError as e:
                failed += 1
                print(f"FAIL {name}: {e}")
    sys.exit(1 if failed else 0)
