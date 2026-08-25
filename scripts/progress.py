#!/usr/bin/env python3
"""Deterministically regenerate the progress block in README.md.

Reads GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md, counts ticked/total boxes
in the mandatory (Implementation Order) slice per phase, and rewrites only the
block in README.md between the `<!-- progress:start -->` /
`<!-- progress:end -->` markers.

    python3 scripts/progress.py            # rewrite README.md in place
    python3 scripts/progress.py --check    # exit non-zero if README.md is stale

No third-party imports. No timestamps, dates, or run counters: the same map
always produces the same bytes.
"""

import argparse
import difflib
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MAP_PATH = REPO_ROOT / "GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md"
README_PATH = REPO_ROOT / "README.md"

START_MARKER = "<!-- progress:start -->"
END_MARKER = "<!-- progress:end -->"

# The mandatory slice is bounded by these two exact heading lines. Boxes
# outside this slice (Maybe / Experimental, Explicit Non-Goals, Product
# Rules) are not pending work: see .agent-runtime/task.md for why.
SLICE_START_HEADING = "Implementation Order"
SLICE_END_HEADING = "Maybe / Experimental Capabilities"

PHASE_HEADING_RE = re.compile(r"^Phase\s+\S+\s+—\s+.+$")
TICKED = "☑"  # ☑
UNTICKED = "☐"  # ☐
CHECKMARK = "✅"  # ✅
FILLED = "█"  # █
EMPTY = "░"  # ░

BAR_WIDTH = 40


def parse_phases(map_text):
    lines = map_text.split("\n")
    try:
        start = next(i for i, l in enumerate(lines) if l.strip() == SLICE_START_HEADING)
    except StopIteration:
        raise SystemExit(
            "progress.py: heading {!r} not found in capability map".format(SLICE_START_HEADING)
        )
    try:
        end = next(
            i for i, l in enumerate(lines) if l.strip() == SLICE_END_HEADING and i > start
        )
    except StopIteration:
        raise SystemExit(
            "progress.py: heading {!r} not found after {!r} in capability map".format(
                SLICE_END_HEADING, SLICE_START_HEADING
            )
        )

    phases = []
    current = None
    for line in lines[start + 1 : end]:
        if PHASE_HEADING_RE.match(line):
            current = {"label": line.strip(), "done": 0, "total": 0}
            phases.append(current)
            continue
        if line.startswith(TICKED):
            if current is None:
                raise SystemExit("progress.py: mandatory box found before first phase heading")
            current["done"] += 1
            current["total"] += 1
        elif line.startswith(UNTICKED):
            if current is None:
                raise SystemExit("progress.py: mandatory box found before first phase heading")
            current["total"] += 1

    if not phases:
        raise SystemExit("progress.py: no phase headings found in mandatory slice")
    return phases


def totals(phases):
    done = sum(p["done"] for p in phases)
    total = sum(p["total"] for p in phases)
    return done, total


def render_bar(done, total, width=BAR_WIDTH):
    filled = (done * width) // total if total else 0
    return FILLED * filled + EMPTY * (width - filled)


def render_block(phases):
    done, total = totals(phases)
    pct = (done * 100) // total if total else 0
    bar = render_bar(done, total)

    lines = [
        START_MARKER,
        "## Progress",
        "",
        "`{}` {} / {} mandatory capabilities ({}%)".format(bar, done, total, pct),
        "",
        # The per-phase table is 100+ rows, which would bury the rest of a
        # short README. `<details>` is plain GitHub Markdown -- no network, no
        # image, no script -- so the breakdown stays one click away and the
        # rendered bytes stay a pure function of the map.
        "<details>",
        "<summary>Per-phase breakdown ({} of {} phases complete)</summary>".format(
            sum(1 for x in phases if x["total"] > 0 and x["done"] == x["total"]),
            len(phases),
        ),
        "",
        "| Phase | Done |",
        "|---|---|",
    ]
    for p in phases:
        complete = p["total"] > 0 and p["done"] == p["total"]
        mark = " " + CHECKMARK if complete else ""
        lines.append("| {} | {}/{}{} |".format(p["label"], p["done"], p["total"], mark))
    lines.append("")
    lines.append("</details>")
    lines.append(END_MARKER)
    return "\n".join(lines)


def find_block(readme_text):
    start_idx = readme_text.find(START_MARKER)
    end_idx = readme_text.find(END_MARKER)
    if start_idx == -1 or end_idx == -1:
        raise SystemExit(
            "progress.py: markers {!r} / {!r} not found in {}".format(
                START_MARKER, END_MARKER, README_PATH
            )
        )
    end_idx += len(END_MARKER)
    if end_idx <= start_idx:
        raise SystemExit("progress.py: progress:end marker precedes progress:start in README.md")
    return start_idx, end_idx


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit non-zero if README.md's progress block is out of date; write nothing",
    )
    args = parser.parse_args(argv)

    map_text = MAP_PATH.read_text(encoding="utf-8")
    phases = parse_phases(map_text)
    new_block = render_block(phases)

    readme_text = README_PATH.read_text(encoding="utf-8")
    start_idx, end_idx = find_block(readme_text)
    current_block = readme_text[start_idx:end_idx]

    if current_block == new_block:
        return 0

    if args.check:
        diff = difflib.unified_diff(
            current_block.splitlines(keepends=True),
            new_block.splitlines(keepends=True),
            fromfile="README.md (current)",
            tofile="README.md (expected)",
        )
        sys.stderr.write("progress.py --check: README.md progress block is stale\n")
        sys.stderr.writelines(diff)
        return 1

    new_readme_text = readme_text[:start_idx] + new_block + readme_text[end_idx:]
    README_PATH.write_text(new_readme_text, encoding="utf-8")
    done, total = totals(phases)
    pct = (done * 100) // total if total else 0
    print(
        "progress.py: updated README.md progress block ({} / {} mandatory, {}%)".format(
            done, total, pct
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
