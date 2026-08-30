#!/usr/bin/env python3
"""Every launch prompt must arm a continuity watch, and the watch must fire.

WHY THIS EXISTS
---------------
On 2026-08-29 three Opus workers ran for two hours with no continuity watch
between them, and the user noticed before any mechanism did. Four separate
things had to be true at once, and each is checked below:

 1. `scripts/dev/new-worker.sh`'s prompt never armed anything. The instruction
    had been added to the ORCHESTRATOR's relaunch prompt only — a fix that
    could not bootstrap itself, because it reached a session only if a previous
    session already had a watch.

 2. The documented path was relative (`.agent-runtime/continuity-watch.sh`) and
    `.agent-runtime/` exists only in the main checkout, so it resolved in 1 of
    64 worktrees and failed with exit 127 in the other 63 — while the pane
    looked armed, because a Monitor whose script dies reports it as a task
    notification that reads like noise.

 3. The session id was hand-copied, so a wrong one produced a watch that was
    confidently reading a DIFFERENT session and never said so.

 4. Nothing enforced any of it. This project's own repeated finding is that a
    rule nobody enforces is decoration (§20, §57), and the arming rule had been
    written into CLAUDE.md, ORIENT.md and CONTINUATION.md without ever being
    checked by anything.

So this file checks the mechanism rather than the prose, and it checks it by
RUNNING it: the threshold assertions below drive the real script against a
synthetic statusline file, so a watch that stopped firing would fail here
rather than in six months of silence.
"""
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
REPO = SCRIPTS.parent
WATCH = SCRIPTS / "continuity-watch.sh"
NEW_WORKER = SCRIPTS / "dev" / "new-worker.sh"
# Gitignored, so it is absent in CI and in any fresh clone. Checked when it is
# there and reported as unchecked when it is not — never silently skipped.
SELF_CONTINUE = REPO / ".agent-runtime" / "self-continue.sh"


def run_watch(sessid: str, tmp: Path, role: str = "worker", timeout: int = 6) -> str:
    """Run the watch for a couple of poll intervals and return what it said."""
    env = dict(os.environ, TMPDIR=str(tmp))
    try:
        done = subprocess.run(
            ["bash", str(WATCH), "--role", role, "--session", sessid, "--poll", "1"],
            capture_output=True, text=True, timeout=timeout, env=env,
        )
        return done.stdout
    except subprocess.TimeoutExpired as expired:
        # The watch never exits on its own — that is the point (§54). Whatever
        # it printed before the timeout is the answer.
        return (expired.stdout or b"").decode() if isinstance(expired.stdout, bytes) else (expired.stdout or "")


def statusline(tmp: Path, sessid: str, ctx: str, rl5: str, rl7: str, branch: str = "fake") -> None:
    (tmp / f"ccsl-data-{sessid}").write_text(
        f"CTX_PCT={ctx}\nRL5={rl5}\nRL7={rl7}\nSESSID={sessid}\nBRANCH={branch}\n"
    )


def main() -> int:
    failures: list[str] = []
    notes: list[str] = []

    # --- 1. the watch exists and is runnable ------------------------------
    if not WATCH.exists():
        print("test_launch_prompts: scripts/continuity-watch.sh is missing", file=sys.stderr)
        return 1
    if not os.access(WATCH, os.X_OK):
        failures.append("scripts/continuity-watch.sh is not executable")

    # A role is not defaulted: the two roles give OPPOSITE instructions at the
    # context threshold (a worker must never relaunch an orchestrator), so
    # guessing would be worse than refusing.
    refused = subprocess.run(["bash", str(WATCH)], capture_output=True, text=True)
    if refused.returncode == 0:
        failures.append("continuity-watch.sh ran without --role; it must refuse rather than guess")

    # --- 2. it fires, for both roles --------------------------------------
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)

        statusline(tmp, "t-worker", "81", "95", "100")
        out = run_watch("t-worker", tmp, role="worker")
        if "CONTEXT 81%" not in out:
            failures.append(f"worker role did not fire the 75% context threshold; said: {out!r}")
        if "do not run self-continue.sh" not in out.lower():
            failures.append("worker role must tell the worker NOT to relaunch an orchestrator")
        # And it must not *also* hand it the invocation. A worker that ran
        # self-continue.sh would spawn a second Opus orchestrator into a tree
        # that already has one, and `worker-capabilities.md` reserves
        # integrating, committing and updating project-status records to the
        # first. Checked as the absence of the invocation forms, because an
        # action string can carry the warning and the instruction at once —
        # a mutation that did exactly that survived the first version of this.
        for forbidden in ("self-continue.sh context", "self-continue.sh ratelimit"):
            if forbidden in out:
                failures.append(
                    f"worker role hands the worker {forbidden!r} — that relaunches an "
                    "orchestrator, which is never a worker's move"
                )
        if "RL5 95%" not in out:
            failures.append(f"worker role did not fire the RL5 threshold; said: {out!r}")
        if "RL7 100%" not in out:
            failures.append(f"worker role did not fire the RL7 threshold; said: {out!r}")

        statusline(tmp, "t-orch", "81", "95", "100")
        out = run_watch("t-orch", tmp, role="orchestrator")
        if "CONTEXT 81%" not in out or "self-continue.sh context" not in out:
            failures.append(f"orchestrator role did not tell itself to hand off; said: {out!r}")

        # RL7 fires at 100, not 90. The standing instruction is "continue until
        # depleted", so a 90 that fired would stop work the user wants done.
        statusline(tmp, "t-rl7", "10", "10", "95")
        out = run_watch("t-rl7", tmp)
        if "RL7" in out:
            failures.append("RL7 fired at 95%; it must fire at 100 (see the script header)")

        # Below every threshold: silence is the correct answer.
        statusline(tmp, "t-quiet", "10", "10", "10")
        out = run_watch("t-quiet", tmp)
        if out.strip():
            failures.append(f"watch spoke while every threshold was clear: {out!r}")

        # --- 3. blind is not silent, and blind does not quit ---------------
        out = run_watch("t-absent", tmp)
        if "CONTINUITY BLIND" not in out:
            failures.append("an unreadable statusline file must announce itself, not look like all-clear")
        if "still watching" not in out:
            failures.append("a blind watch must say it is still watching (§54: it must not quit)")

        # A malformed value must never be read as zero, which would look calm.
        (tmp / "ccsl-data-t-junk").write_text("CTX_PCT=''\nRL5=abc\nRL7=\n")
        out = run_watch("t-junk", tmp)
        if "CONTINUITY BLIND" not in out:
            failures.append("malformed statusline values must read as blind, never as 0%")

    # --- 4. the worker launch prompt arms it ------------------------------
    #
    # Asserted on the prompt the script PRODUCES, not on how the file spells
    # it. The first version of this check read the `ARM=`/`PROMPT=` lines as
    # text, and a mutation that set `ARM=""` while leaving the words in a
    # trailing comment SURVIVED it. A check that inspects the source instead of
    # the product is the defect it is meant to catch, wearing a lab coat.
    if not NEW_WORKER.exists():
        failures.append("scripts/dev/new-worker.sh is missing")
    else:
        with tempfile.TemporaryDirectory() as raw:
            packet = Path(raw) / "packet-test.md"
            packet.write_text("# TASK PACKET — TEST\n")
            built = subprocess.run(
                ["bash", str(NEW_WORKER), "test-worker", raw, str(packet), "--print-prompt"],
                capture_output=True, text=True, timeout=60,
            )
        if built.returncode != 0:
            failures.append(
                f"new-worker.sh --print-prompt failed ({built.returncode}): {built.stderr.strip()!r}. "
                "Without it this test cannot see the prompt a worker actually receives."
            )
        prompt = built.stdout.strip()
        if not prompt:
            failures.append("new-worker.sh --print-prompt produced nothing")
        if "Monitor(" not in prompt:
            failures.append("the worker prompt does not name Monitor(...), the tool that arms the watch")

        # The armed path must be absolute AND must actually exist — a relative
        # `.agent-runtime/...` resolved in 1 of 64 worktrees, and every worker
        # runs in one of the other 63.
        armed = re.search(r'command:\s*"([^"]*continuity-watch\.sh[^"]*)"', prompt)
        if not armed:
            failures.append(
                "the worker prompt does not arm continuity-watch.sh — this is the exact "
                "regression of 2026-08-29, where three Opus workers ran unwatched for two hours"
            )
        else:
            path = armed.group(1).split()[0]
            if not path.startswith("/"):
                failures.append(f"the worker prompt arms a relative path ({path!r}); it must be absolute")
            elif not os.access(path, os.X_OK):
                failures.append(f"the worker prompt arms {path!r}, which is not an executable file")
            if "--role worker" not in armed.group(1):
                failures.append("the worker prompt must arm the watch with --role worker, not the orchestrator role")

    # --- 4b. --effort is optional, composes with --model, and rejects a typo
    #
    # Effort is not dynamic today: every worker inherits whatever the global
    # default is, regardless of tier. `--effort` is the missing sibling of
    # `--model` (already threaded, positional-independent). The one rule that
    # matters: omitted, the launch command must be byte-identical to before
    # this flag existed. `--print-prompt` now emits that command as its first
    # line (`LAUNCH: ...`), so this is checked hermetically -- no real worker
    # is spawned, per this package's own STOP CONDITIONS.
    if not NEW_WORKER.exists():
        failures.append("scripts/dev/new-worker.sh is missing")  # already recorded above
    else:
        def launch_line(*extra_args: str) -> str | None:
            with tempfile.TemporaryDirectory() as raw:
                packet = Path(raw) / "packet-test.md"
                packet.write_text("# TASK PACKET — TEST\n")
                built = subprocess.run(
                    ["bash", str(NEW_WORKER), "test-worker", raw, str(packet),
                     *extra_args, "--print-prompt"],
                    capture_output=True, text=True, timeout=60,
                )
            lines = built.stdout.splitlines()
            if not lines or not lines[0].startswith("LAUNCH: "):
                return None
            return lines[0][len("LAUNCH: "):]

        baseline = launch_line()
        expected_baseline = "claude --model sonnet --permission-mode auto --remote-control"
        if baseline != expected_baseline:
            failures.append(
                f"no --effort must leave the launch command byte-identical to today's "
                f"({expected_baseline!r}); got {baseline!r}"
            )

        with_effort = launch_line("--effort", "medium")
        if with_effort != f"{expected_baseline} --effort medium":
            failures.append(f"--effort medium did not appear at that level: {with_effort!r}")

        order_a = launch_line("--model", "sonnet", "--effort", "medium")
        order_b = launch_line("--effort", "medium", "--model", "sonnet")
        if order_a != order_b or order_a is None:
            failures.append(
                f"--model and --effort must compose regardless of order: {order_a!r} != {order_b!r}"
            )

        # An unknown level must be rejected loudly, before a workspace is
        # created -- checked without --print-prompt: validation happens before
        # any `cmux` call, so a bad level must fail fast rather than hang
        # waiting on a workspace that never gets created.
        with tempfile.TemporaryDirectory() as raw:
            packet = Path(raw) / "packet-test.md"
            packet.write_text("# TASK PACKET — TEST\n")
            bad = subprocess.run(
                ["bash", str(NEW_WORKER), "test-worker", raw, str(packet), "--effort", "bogus"],
                capture_output=True, text=True, timeout=10,
            )
        if bad.returncode == 0:
            failures.append("new-worker.sh accepted an unknown --effort level (exit 0)")
        if "dispatched" in (bad.stdout + bad.stderr).lower():
            failures.append(
                f"an unknown --effort level must create no workspace; script said: {bad.stdout!r}"
            )

    # --- 5. the orchestrator relaunch prompt arms it ----------------------
    if SELF_CONTINUE.exists():
        text = SELF_CONTINUE.read_text()
        if "continuity-watch.sh" not in text:
            failures.append(
                ".agent-runtime/self-continue.sh's relaunch prompt does not arm a continuity watch"
            )
    else:
        # Say so rather than pass quietly. This is the honest limit of the
        # check: the orchestrator's relaunch prompt lives in a gitignored file,
        # so nothing in the repository can protect it on a fresh clone. Moving
        # it under scripts/ would close that, and is recorded as owed work.
        notes.append(
            ".agent-runtime/self-continue.sh absent (gitignored) — the orchestrator's own "
            "relaunch prompt was NOT checked"
        )

    # --- 6. the relaunch recipe carries the session id, and a missing id --
    #        is refused rather than shared ---------------------------------
    #
    # WHY: `self-continue.sh` scopes its fire-once lock as
    # `.relaunch-<sessid>-<mode>.lock` and reads the id from `CCSL_SESSID`.
    # That variable is not exported into the environment the watch's advice is
    # followed in, so `${CCSL_SESSID:-unknown}` collapsed EVERY session onto one
    # shared lock file — reinstating, by a different route, the exact defect
    # self-continue.sh's own header says was fixed on 2026-08-26. Measured
    # 2026-08-30: a handoff reported "already relaunched; nothing to do" against
    # a lock an unrelated session had written, and
    # `.agent-runtime/.relaunch-unknown-context.lock` is still on disk as the
    # evidence.
    #
    # Two halves, so check both: the watch must PASS the id on (it is the only
    # place that knows it), and self-continue.sh must REFUSE a degraded one
    # rather than quietly share. Either half alone leaves the hole open.
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        sessid = "cafe1234-0000-4000-8000-abcdefabcdef"
        statusline(tmp, sessid, ctx="76", rl5="10", rl7="10")
        said = run_watch(sessid, tmp, role="orchestrator")
        if "self-continue.sh" in said and f"CCSL_SESSID={sessid}" not in said:
            failures.append(
                "continuity-watch.sh --role orchestrator advises running self-continue.sh "
                f"WITHOUT CCSL_SESSID={sessid}; every session would then share the lock "
                ".relaunch-unknown-context.lock"
            )

    if SELF_CONTINUE.exists():
        env = {k: v for k, v in os.environ.items() if k != "CCSL_SESSID"}
        done = subprocess.run(
            ["bash", str(SELF_CONTINUE), "context"],
            capture_output=True, text=True, env=env, timeout=30,
        )
        if done.returncode == 0:
            failures.append(
                ".agent-runtime/self-continue.sh accepted an unset CCSL_SESSID (exit 0) "
                "instead of refusing; its fire-once lock is shared with every other session"
            )
        elif "CCSL_SESSID" not in (done.stderr + done.stdout):
            failures.append(
                ".agent-runtime/self-continue.sh refused an unset CCSL_SESSID but never "
                "named the variable, so the message does not say how to fix it"
            )
    else:
        notes.append(
            ".agent-runtime/self-continue.sh absent (gitignored) — the shared-lock refusal "
            "was NOT checked"
        )

    if failures:
        print(f"test_launch_prompts: {len(failures)} failure(s)", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1

    print("test_launch_prompts: ok — both roles fire at 75/90/100, blind announces itself "
          "and keeps watching, the worker launch prompt arms a rooted path, and the "
          "relaunch recipe carries a session id that cannot degrade to a shared lock")
    for n in notes:
        print(f"  note: {n}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
