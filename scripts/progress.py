#!/usr/bin/env python3
"""Deterministically regenerate the progress block in README.md.

Reads docs/product/capability-map.md, counts ticked/total boxes in the mandatory
(Implementation Order) slice per phase, and rewrites only the block in
README.md between the `<!-- progress:start -->` / `<!-- progress:end -->`
markers.

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
MAP_PATH = REPO_ROOT / "docs" / "product" / "capability-map.md"
README_PATH = REPO_ROOT / "README.md"

START_MARKER = "<!-- progress:start -->"
END_MARKER = "<!-- progress:end -->"

# The mandatory slice is bounded by these two exact heading lines. Boxes
# outside this slice (Maybe / Experimental, Explicit Non-Goals, Product
# Rules) are not pending work: see .agent-runtime/task.md for why.
SLICE_START_HEADING = "Implementation Order"
SLICE_END_HEADING = "Maybe / Experimental Capabilities"

# The map's own words for a phase the user deferred; see render_block.
GATE_MARKER = "deferred experiment gate"
MAYBE_HEADING_RE = re.compile(r"^Maybe [A-Z] — ")

PHASE_HEADING_RE = re.compile(r"^Phase\s+\S+\s+—\s+.+$")
TICKED = "☑"  # ☑
UNTICKED = "☐"  # ☐
CHECKMARK = "✅"  # ✅
FILLED = "█"  # █
EMPTY = "░"  # ░

BAR_WIDTH = 40


def count_parked(map_text):
    """Unticked boxes after the Maybe / Experimental heading.

    Parked lines are neither done nor queued; naming them keeps the headline
    from implying they are either.
    """
    lines = map_text.split("\n")
    try:
        start = next(i for i, l in enumerate(lines) if l.strip() == SLICE_END_HEADING)
    except StopIteration:
        return 0
    # Same heading rule as parse_phases: a `Phase N` appended after the
    # experimental section is committed work, not a parked line.
    parked, experimental = 0, True
    for l in lines[start:]:
        if PHASE_HEADING_RE.match(l):
            experimental = False
        elif MAYBE_HEADING_RE.match(l):
            experimental = True
        if experimental and l.startswith(UNTICKED):
            parked += 1
    return parked


def parse_phases(map_text):
    lines = map_text.split("\n")
    try:
        start = next(i for i, l in enumerate(lines) if l.strip() == SLICE_START_HEADING)
    except StopIteration:
        raise SystemExit(
            "progress.py: heading {!r} not found in capability map".format(SLICE_START_HEADING)
        )
    # The slice used to end at the experimental heading, which made file
    # POSITION decide whether a phase was mandatory. A capability's ID is its
    # line number (scripts/map-index.py), so a new phase must be appended at the
    # END of the map or every ID below it silently renames -- and an appended
    # `Phase N` heading then fell outside the slice and went uncounted. Read the
    # heading kind instead: `Phase N` is mandatory wherever it sits, `Maybe X` is
    # not. That makes appending both safe and countable.
    phases = []
    current = None
    experimental = False
    for line in lines[start + 1 :]:
        if line.strip() == SLICE_END_HEADING:
            experimental = True
        if PHASE_HEADING_RE.match(line):
            experimental = False
        elif MAYBE_HEADING_RE.match(line):
            experimental = True
        if experimental:
            continue
        if PHASE_HEADING_RE.match(line):
            current = {"label": line.strip(), "done": 0, "total": 0,
                       "gate": GATE_MARKER in line}
            phases.append(current)
            continue
        # A phase the user has deferred as an experiment gate carries this
        # marker in the map itself (user decision 2026-09-03). Its unchecked
        # criteria are the terms of a decision not yet taken, not release work,
        # so they are counted and shown apart from the active queue rather than
        # inflating a "mandatory" headline. No separate status file: the map is
        # still the only source.
        if current is not None and GATE_MARKER in line:
            current["gate"] = True
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


def render_block(phases, parked=0):
    active = [p for p in phases if not p.get("gate")]
    gates = [p for p in phases if p.get("gate")]
    done, total = totals(active)
    done_all = sum(p["done"] for p in phases)
    active_open = total - done
    # The ratio is closed against closed-plus-active-open: one percentage over
    # one queue. Deferred and parked lines are reported beside it, never inside.
    denom = done_all + active_open
    pct = (done_all * 100) // denom if denom else 0
    bar = render_bar(done_all, denom)
    gate_open = sum(p["total"] - p["done"] for p in gates)
    gate_names = ", ".join(p["label"].split(" — ")[0] for p in gates)

    lines = [
        START_MARKER,
        "## Progress",
        "",
        "`{}` **{} closed** · **{} active committed open** ({}%)".format(
            bar, done_all, active_open, pct
        ),
        "",
        # One percentage, over one status. The deferred gates and the parked
        # experimental lines are real and stay visible, but folding them into
        # the same figure would state that undecided and unbuilt are the same
        # thing.
        "Separately tracked, and not release-blocking: **{} deferred gate criteria** "
        "({}) awaiting a decision, and **{} parked experimental lines** under "
        "Maybe / Experimental.".format(gate_open, gate_names or "none", parked),
        "",
        # The per-phase table is 100+ rows, which would bury the rest of a
        # short README. `<details>` is plain GitHub Markdown -- no network, no
        # image, no script -- so the breakdown stays one click away and the
        # rendered bytes stay a pure function of the map.
        "<details>",
        "<summary>Per-phase breakdown ({} of {} active phases complete)</summary>".format(
            sum(1 for x in active if x["total"] > 0 and x["done"] == x["total"]),
            len(active),
        ),
        "",
        "| Phase | Done |",
        "|---|---|",
    ]
    for p in phases:
        complete = p["total"] > 0 and p["done"] == p["total"]
        mark = " " + CHECKMARK if complete else ""
        lines.append("| {} | {}/{}{} |".format(p["label"], p["done"], p["total"], mark))
    for p in gates:
        lines.append(
            "| {} | {}/{} — deferred gate |".format(p["label"], p["done"], p["total"])
        )
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
    new_block = render_block(phases, count_parked(map_text))

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
    active = [p for p in phases if not p.get("gate")]
    done_all = sum(p["done"] for p in phases)
    active_open = sum(p["total"] - p["done"] for p in active)
    gate_open = sum(p["total"] - p["done"] for p in phases if p.get("gate"))
    print(
        "progress.py: updated README.md progress block "
        "({} closed, {} active open, {} deferred gate criteria, {} parked)".format(
            done_all, active_open, gate_open, count_parked(map_text)
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
