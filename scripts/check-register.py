#!/usr/bin/env python3
"""Flag refusal-register rows that name a capability line already ticked.

    scripts/check-register.py            # report, exit 0
    scripts/check-register.py --check    # exit 1 if any row is stale

WHY THIS EXISTS
---------------
A register row records why a line cannot be closed. When the blocker goes away
— usually because some *other* package landed the missing piece — nothing
rechecks the row, and it keeps telling orchestrators not to package work that
is already done or already possible.

Measured 2026-08-29/30: an audit of the register found **seven stale rows**
(1288, 1291, 1319, 930, 934, 748, 1681), every one naming a line that was
already ☑ in the map. Two of them had been written the same day they went
stale, by the orchestrator who then relied on them. That audit cost a worker
slot. This check is the mechanical part of it and costs a second.

WHAT IT DOES NOT DO
-------------------
It only catches the *ticked-line* case. A row whose blocker is wrong while the
line is still open — Cluster K framing 745 as an unmade design decision when
the read path existed, or Cluster D saying "nothing selects among launch
profiles" when it does — is **not** detectable this way and still needs a human
or a recon. Do not read a clean run as "the register is accurate".
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path


def _main_checkout(script_dir: Path) -> Path:
    """The one real docs/ owner, regardless of which worktree's copy of this
    script is executing.

    scripts/ and docs/ are both tracked, so every worktree carries its own
    (possibly stale) copy of refusal-register.md and capability-map.md.
    Resolving REPO from __file__ alone answers about whichever tree happens
    to be running, not necessarily the one the caller means to check.
    Reproduced 2026-08-30 (script-tree-audit): invoked via a relative path
    from a worker's own worktree, this checked 75 register ids against a
    refusal-register.md 116 lines behind the main checkout's 884, and
    reported "clean" either way, with no indication which tree it read.
    git's own worktree metadata names the one real answer.
    """
    try:
        common = subprocess.run(
            ["git", "-C", str(script_dir), "rev-parse", "--git-common-dir"],
            capture_output=True, text=True, timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired):
        return script_dir
    if common.returncode != 0:
        return script_dir
    common_dir = Path(common.stdout.strip())
    if not common_dir.is_absolute():
        common_dir = (script_dir / common_dir).resolve()
    return common_dir.parent if common_dir.name == ".git" else script_dir


REPO = _main_checkout(Path(__file__).resolve().parents[1])
REGISTER = REPO / "docs" / "process" / "refusal-register.md"
MAP = REPO / "docs" / "product" / "capability-map.md"

# A row that already announces itself as closed is not a finding. Struck-through
# ids (~~1288~~) and rows whose text says CLOSED are the retirement convention.
RETIRED = re.compile(r"~~|\bCLOSED\b")


def ticked_lines(map_path: Path) -> set[int]:
    ticked = set()
    for n, line in enumerate(map_path.read_text().splitlines(), start=1):
        if line.startswith("☑ "):
            ticked.add(n)
    return ticked


def register_rows(register_path: Path) -> list[tuple[int, str, list[int]]]:
    """(register line number, row text, capability ids the row names)."""
    rows = []
    for n, line in enumerate(register_path.read_text().splitlines(), start=1):
        stripped = line.strip()
        # Table rows and the bulleted standing-refusal entries both name ids.
        if not (stripped.startswith("|") or stripped.startswith("- **")):
            continue
        if RETIRED.search(stripped):
            continue
        # Ids are 3-4 digit numbers in the row's first cell or bold marker.
        head = stripped.split("|")[1] if stripped.startswith("|") else stripped[:40]
        # 1-5 digits: real capability ids are 3-4, but a short id must not be
        # invisible, and the range filter below is what actually bounds this.
        ids = [int(m) for m in re.findall(r"\b(\d{1,5})\b", head)]
        ids = [i for i in ids if 1 <= i <= 20000]
        if ids:
            rows.append((n, stripped, ids))
    return rows


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true",
                    help="exit 1 if any live row names a ticked line")
    ap.add_argument("--register", default=str(REGISTER))
    ap.add_argument("--map", default=str(MAP))
    args = ap.parse_args()

    ticked = ticked_lines(Path(args.map))
    stale = []
    checked = 0
    for lineno, text, ids in register_rows(Path(args.register)):
        for cap in ids:
            checked += 1
            if cap in ticked:
                stale.append((lineno, cap, text[:110]))

    if not stale:
        print(f"check-register: clean — {checked} id(s) checked, "
              f"no live row names a ticked line")
        print("  (this catches only the ticked-line case; a row whose blocker "
              "is wrong while its line is open still needs a human)")
        return 0

    print(f"check-register: {len(stale)} STALE row(s) — each names a line that "
          f"is already ☑ in the map:")
    for lineno, cap, text in stale:
        print(f"  refusal-register.md:{lineno}: line {cap} is CLOSED — {text}")
    print("\nRetire them: strike the id through (~~1288~~) and say CLOSED, or "
          "delete the row. A register that lists finished work tells the next "
          "orchestrator not to package something that is already done.")
    return 1 if args.check else 0


if __name__ == "__main__":
    sys.exit(main())
