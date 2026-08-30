#!/usr/bin/env python3
"""Turn a worker's structured FACTS block into an evidence-ledger draft.

WHY THIS EXISTS
---------------
Measured on batch 45: six workers wrote **92 KB** of prose reports. The
orchestrator read all of it and then hand-wrote **979 lines** of ledger entries
saying the same things in a different order. That is the single largest
orchestrator cost in the process, and `assurance-economics.md`'s Phase 1b
already prescribed the fix ("stop double-authoring evidence") without anyone
building it.

The split this enforces:

    worker    -> structured FACTS   (what it changed, ran, and observed)
    generator -> the mechanical 70% (contract, citations, mutation table)
    ORCHESTRATOR -> the ruling      (accepted? which box? what is still missing?)

The generator deliberately does **not** decide anything. It emits `State: ⟨RULING
REQUIRED⟩` and a REVIEW block listing exactly what the orchestrator must still
answer. A draft that silently claimed COMPLETE would be the "confident fiction"
this project refuses — and worse, it would let a box be ticked by a script.

A worker appends one fenced block to its report:

    ```glasshouse-facts
    task: GH-EXAMPLE
    status: complete            # complete | partial | blocked
    worktree: .worktrees/example
    lines:
      - id: 1641                # the map line number
        verdict: closed         # closed | open | refused
        contract: "Given ..., when ..., Glasshouse ..., while preserving ..."
        production:
          - "src/checkpoint/mod.rs :: Checkpoint::capture"
        regression:
          - "checkpoint_portability::a_checkpoint_records_working_tree_status"
        mutations:
          - vocabulary: skip-state-update
            change: "working_tree: detect(root) -> None"
            result: killed      # killed | survived | not-run
            killed_by: "checkpoint_portability::a_checkpoint_records_..."
            observed: "a checkpoint taken inside a repository must record ..."
        limits:
          - "compares against the index only; untracked files are not detected"
    packet_errors:
      - "the packet said X; current source says Y (src/foo.rs:12)"
    scope_overflow:
      - path: "src/api/unix.rs"
        reason: "signature change forced two call-site updates"
    gates:
      - "cargo clippy --all-targets --all-features -D warnings: clean"
    ```

USAGE
    scripts/evidence_from_report.py .agent-runtime/report-foo.md
    scripts/evidence_from_report.py --check .agent-runtime/report-foo.md
    scripts/evidence_from_report.py report.md --map-check
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys

try:
    import yaml
except ImportError:                                        # pragma: no cover
    print("evidence: PyYAML is required (pip install pyyaml)", file=sys.stderr)
    raise SystemExit(2)


def _main_checkout(script_dir: str) -> str:
    """The one real capability-map.md owner, regardless of which worktree's
    copy of this script is executing.

    scripts/ and docs/ are both tracked, so every worktree carries its own
    (possibly stale) copy of the map. Resolving REPO from __file__ alone
    answers about whichever tree happens to be running, not necessarily the
    one the orchestrator means to check --map-check against. Same shape as
    scripts/check-register.py, which reproduced a stale-doc read 2026-08-30
    (script-tree-audit) from this identical pattern; not independently
    reproduced here because this worktree's capability-map.md happened to be
    byte-identical to the main checkout's at audit time. git's own worktree
    metadata names the one real answer.
    """
    try:
        common = subprocess.run(
            ["git", "-C", script_dir, "rev-parse", "--git-common-dir"],
            capture_output=True, text=True, timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired):
        return script_dir
    if common.returncode != 0:
        return script_dir
    common_dir = common.stdout.strip()
    if not os.path.isabs(common_dir):
        common_dir = os.path.normpath(os.path.join(script_dir, common_dir))
    return os.path.dirname(common_dir) if os.path.basename(common_dir) == ".git" else script_dir


REPO = _main_checkout(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
MAP = os.path.join(REPO, "docs/product/capability-map.md")
FENCE = re.compile(r"```glasshouse-facts\s*\n(.*?)\n```", re.S)

VERDICT_STATE = {
    # A worker's verdict is a *claim*. It maps to a ledger state only as a
    # proposal; the orchestrator overwrites it. Never emit COMPLETE from a
    # script -- COMPLETE is what authorises ticking a box.
    "closed": "⟨RULING REQUIRED⟩ — worker proposes COMPLETE",
    "open": "NOT STARTED — worker reports the line still open",
    "refused": "NOT STARTED — worker refused the line; see its reason",
}


def map_line(n: int) -> str | None:
    try:
        with open(MAP, encoding="utf-8") as fh:
            lines = fh.read().split("\n")
        return lines[n - 1] if 0 < n <= len(lines) else None
    except OSError:
        return None


def extract(path: str) -> dict:
    with open(path, encoding="utf-8") as fh:
        text = fh.read()
    m = FENCE.search(text)
    if not m:
        raise SystemExit(
            f"evidence: {path} has no ```glasshouse-facts block.\n"
            f"          The worker must emit one; see this script's docstring."
        )
    data = yaml.safe_load(m.group(1))
    if not isinstance(data, dict):
        raise SystemExit(f"evidence: {path}'s facts block is not a mapping")
    return data


def validate(data: dict, path: str, map_check: bool) -> list[str]:
    problems: list[str] = []
    for key in ("task", "status", "lines"):
        if key not in data:
            problems.append(f"missing top-level `{key}`")
    for i, ln in enumerate(data.get("lines") or []):
        where = f"lines[{i}]"
        for key in ("id", "verdict"):
            if key not in ln:
                problems.append(f"{where}: missing `{key}`")
        if ln.get("verdict") == "closed":
            if not ln.get("production"):
                problems.append(f"{where}: verdict closed with no `production` evidence")
            if not ln.get("regression"):
                problems.append(f"{where}: verdict closed with no `regression` evidence")
            muts = ln.get("mutations") or []
            if not any(m.get("result") == "killed" for m in muts):
                problems.append(
                    f"{where}: verdict closed with no killed mutation — "
                    f"a closure resting on an existing test is practice §14's trap"
                )
        if map_check and isinstance(ln.get("id"), int):
            actual = map_line(ln["id"])
            if actual is None:
                problems.append(f"{where}: map line {ln['id']} does not exist")
            elif not actual.startswith(("☐", "☑")):
                problems.append(
                    f"{where}: map line {ln['id']} is not a capability box "
                    f"— it reads {actual[:48]!r}"
                )
    return problems


def render(data: dict) -> str:
    task = data.get("task", "UNKNOWN")
    L: list[str] = []
    add = L.append

    add(f"<!-- DRAFT generated by scripts/evidence_from_report.py from {task}'s")
    add("     facts block. The citations and mutation results below are the")
    add("     worker's. THE RULINGS ARE NOT WRITTEN AND MUST NOT BE GUESSED. -->")
    add("")

    for ln in data.get("lines") or []:
        num = ln.get("id")
        box = (map_line(num) or "").lstrip("☐☑ ").strip() if isinstance(num, int) else ""
        add(f"### {box or '⟨map text⟩'} (line {num})")
        add("")
        if ln.get("contract"):
            add(f"Contract: {ln['contract']}")
            add("")
        add(f"State: {VERDICT_STATE.get(ln.get('verdict'), '⟨RULING REQUIRED⟩')}")
        add("")

        if ln.get("production"):
            add("Production evidence:")
            for cite in ln["production"]:
                if "::" in cite:
                    f, sym = [x.strip() for x in cite.split("::", 1)]
                    add(f"- `{f}` — `{sym}`")
                else:
                    add(f"- `{cite}`")
            add("")

        if ln.get("regression"):
            add("Regression evidence:")
            for t in ln["regression"]:
                add(f"- `{t}`")
            add("")

        muts = ln.get("mutations") or []
        if muts:
            add("| mutation | vocabulary | result | killed by |")
            add("|---|---|---|---|")
            for m in muts:
                res = m.get("result", "?")
                mark = {"killed": "**killed**",
                        "survived": "**SURVIVED — investigate**",
                        "not-run": "not run"}.get(res, res)
                add(f"| {m.get('change','?')} | `{m.get('vocabulary','?')}` "
                    f"| {mark} | `{m.get('killed_by','—')}` |")
            add("")
            for m in muts:
                if m.get("observed"):
                    add(f"> {m.get('vocabulary','mutation')} observed: {m['observed']}")
                    add("")
                if m.get("result") == "survived":
                    add("**A SURVIVING MUTATION IS THE MOST VALUABLE OUTCOME HERE** —")
                    add("it names a case where passing tests do not prove the claimed")
                    add("behaviour. Do not tick this box; write down what it means.")
                    add("")

        if ln.get("limits"):
            add("Recorded scope limits — stated by the worker, not discovered later:")
            for lim in ln["limits"]:
                add(f"- {lim}")
            add("")
        add("---")
        add("")

    add("## REVIEW — the orchestrator owes an answer to each of these")
    add("")
    add("This section is the point of the generator. Everything above is the")
    add("worker's facts, transcribed. Nothing below is decided.")
    add("")
    for ln in data.get("lines") or []:
        v = ln.get("verdict")
        add(f"- **{ln.get('id')}** — verdict `{v}`. "
            + ("Re-run one decisive mutation yourself, then rule (§79: a worker's "
               "packet does not bind the integrator)."
               if v == "closed" else
               "Confirm the worker's reason against current source before recording it."))
    if data.get("packet_errors"):
        add("")
        add("**Packet errors the worker reported — read these BEFORE its results.**")
        add("Thirteen consecutive rounds a worker corrected its packet and was right:")
        for e in data["packet_errors"]:
            add(f"- {e}")
    if data.get("scope_overflow"):
        add("")
        add("**Files touched outside EXPECTED FILES** — disclosed, not hidden:")
        for o in data["scope_overflow"]:
            add(f"- `{o.get('path')}` — {o.get('reason')}")
    if data.get("gates"):
        add("")
        add("Gates the worker ran (re-run the decisive ones yourself):")
        for g in data["gates"]:
            add(f"- {g}")
    add("")
    return "\n".join(L) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("report")
    ap.add_argument("--check", action="store_true",
                    help="validate the facts block and exit; emit nothing")
    ap.add_argument("--map-check", action="store_true",
                    help="also verify each cited id is a real capability box")
    args = ap.parse_args()

    data = extract(args.report)
    problems = validate(data, args.report, args.map_check or args.check)
    if problems:
        print(f"evidence: REFUSED — {len(problems)} problem(s) in {args.report}:")
        for p in problems:
            print(f"  {p}")
        return 1
    if args.check:
        n = len(data.get("lines") or [])
        print(f"evidence: {args.report} facts block is well-formed ({n} line(s))")
        return 0
    sys.stdout.write(render(data))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
