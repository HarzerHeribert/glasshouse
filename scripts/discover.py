#!/usr/bin/env python3
"""Read-only evidence gathering, run before a packet is written.

    scripts/discover.py --seam 'ExtractionModel::complete'
    scripts/discover.py --phase 9I

Never edits anything. Both modes read `crates/**` and the capability
documents under `docs/` relative to the current working directory.
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

BOX_OPEN = "☐"
BOX_DONE = "☑"
BOX_CHARS = (BOX_OPEN, BOX_DONE)

DEFAULT_MAP = "docs/product/capability-map.md"
DEFAULT_EVIDENCE = "docs/product/evidence"
DEFAULT_SRC_ROOT = "crates"


def is_test_path(rel_path: str) -> bool:
    return re.search(r"(^|/)tests/", rel_path) is not None


def test_block_line_numbers(lines: list[str]) -> set[int]:
    """0-indexed line numbers that fall inside a `#[cfg(test)]` module or a
    `#[test]`-attributed function, found by brace-counting from the
    attribute's next `{` to its matching `}`.

    Brace counting ignores string/comment content — a deliberate
    approximation, since this tool only needs to *exclude* test scaffolding
    from a call-site count, not parse Rust exactly.
    """
    skip: set[int] = set()
    n = len(lines)
    i = 0
    while i < n:
        stripped = lines[i].strip()
        if stripped == "#[cfg(test)]" or stripped.startswith("#[test]"):
            j = i
            open_line = None
            while j < n:
                if "{" in lines[j]:
                    open_line = j
                    break
                j += 1
            if open_line is None:
                i += 1
                continue
            depth = 0
            k = open_line
            opened = False
            while k < n:
                depth += lines[k].count("{")
                depth -= lines[k].count("}")
                if lines[k].count("{") > 0:
                    opened = True
                if opened and depth <= 0:
                    break
                k += 1
            for m in range(i, min(k, n - 1) + 1):
                skip.add(m)
            i = k + 1
            continue
        i += 1
    return skip


def find_call_sites(seam: str, src_root: str) -> dict:
    """Find non-test call sites of `seam` (`Type::method` or `method`).

    Prefers a literal, fully-qualified match of `seam` itself; if that finds
    nothing, falls back to method-call syntax (`.method(`) on the trailing
    component, since idiomatic Rust calls a trait method as `x.method(...)`
    rather than `Type::method(...)`. The fallback is reported as a heuristic,
    not asserted as equivalent to the literal form.
    """
    method = seam.rsplit("::", 1)[-1]
    literal_re = re.compile(re.escape(seam))
    method_re = re.compile(r"\." + re.escape(method) + r"\(")

    literal_hits: list[tuple[str, int, str]] = []
    method_hits: list[tuple[str, int, str]] = []

    for path in sorted(Path(src_root).rglob("*.rs")):
        rel = path.as_posix()
        if is_test_path(rel):
            continue
        try:
            text = path.read_text(errors="replace")
        except OSError:
            continue
        lines = text.splitlines()
        skip = test_block_line_numbers(lines)
        for idx, line in enumerate(lines):
            if idx in skip:
                continue
            if literal_re.search(line):
                literal_hits.append((rel, idx + 1, line.strip()))
            elif method_re.search(line):
                method_hits.append((rel, idx + 1, line.strip()))

    return {"literal": literal_hits, "method": method_hits}


def report_seam(seam: str, src_root: str) -> None:
    hits = find_call_sites(seam, src_root)
    literal, method = hits["literal"], hits["method"]

    if literal:
        print(f"discover.py: {len(literal)} non-test call site(s) of `{seam}` "
              f"(literal match):")
        for rel, lineno, text in literal:
            print(f"  {rel}:{lineno}: {text}")
        print(
            "A box that depends on this seam can close: it has a production "
            "caller in the current tree."
        )
        return

    if method:
        print(
            f"discover.py: no literal `{seam}` call site found; "
            f"{len(method)} non-test call(s) of `.{seam.rsplit('::', 1)[-1]}(` "
            f"(method-call heuristic — may include unrelated types):"
        )
        for rel, lineno, text in method:
            print(f"  {rel}:{lineno}: {text}")
        print(
            "Treat this as a lead, not proof — confirm the receiver's type "
            "before crediting a box with this caller."
        )
        return

    print(
        f"discover.py: ZERO non-test call sites of `{seam}` in {src_root}/**/src/**. "
        f"No box depending on this seam can close — it has no production "
        f"caller in the current tree (practice §5, §36)."
    )


def parse_box_lines(lines: list[str]) -> list[tuple[int, str, str]]:
    boxes = []
    for idx, line in enumerate(lines):
        stripped = line.strip()
        if stripped[:1] in BOX_CHARS:
            boxes.append((idx + 1, stripped[0], stripped[1:].strip()))
    return boxes


def phase_span(lines: list[str], phase_id: str) -> tuple[int, int, str] | None:
    heading_re = re.compile(r"^Phase\s+" + re.escape(phase_id) + r"\b")
    next_re = re.compile(r"^Phase\s+\S+")
    start = None
    heading = None
    for i, line in enumerate(lines):
        if heading_re.match(line.strip()):
            start = i
            heading = line.strip()
            break
    if start is None:
        return None
    end = len(lines)
    for j in range(start + 1, len(lines)):
        if next_re.match(lines[j].strip()):
            end = j
            break
    return start, end, heading


def phase_evidence_paths(evidence_lines: list[str], phase_id: str) -> list[str]:
    heading_re = re.compile(r"^###\s+Phase\s+" + re.escape(phase_id) + r"\b")
    next_heading_re = re.compile(r"^###\s+Phase\b")
    path_re = re.compile(r"`([\w./\-]+\.[A-Za-z0-9]+)`")
    paths: list[str] = []
    n = len(evidence_lines)
    i = 0
    while i < n:
        if heading_re.match(evidence_lines[i].strip()):
            j = i + 1
            while j < n and not next_heading_re.match(evidence_lines[j].strip()):
                for m in path_re.finditer(evidence_lines[j]):
                    p = m.group(1)
                    if "/" in p:
                        paths.append(p)
                j += 1
            i = j
            continue
        i += 1
    seen: set[str] = set()
    out: list[str] = []
    for p in paths:
        if p not in seen:
            seen.add(p)
            out.append(p)
    return out


def evidence_file_id(phase_id: str) -> str:
    """`docs/product/evidence/phase-<id>.md`'s <id>: lowercased, non-alnum runs
    hyphenated, matching how the ledger split named those files."""
    slug = re.sub(r"[^a-z0-9]+", "-", phase_id.lower())
    return slug.strip("-")


def phase_evidence_lines(evidence_dir: str, phase_id: str) -> tuple[list[str], str]:
    """The evidence-ledger lines to scan for `phase_id`, and where they came
    from.

    The split ledger gives each phase its own `phase-<id>.md` — read that
    file directly when it exists, which is the point of the split: no need
    to scan the other 30-odd files. A phase_id that names more than one
    split file (e.g. a bare `21` alongside `phase-21-extraction-contract.md`)
    falls back to scanning every split file, the same exhaustive way the
    single ledger used to be scanned.
    """
    direct = Path(evidence_dir) / f"phase-{evidence_file_id(phase_id)}.md"
    if direct.exists():
        return direct.read_text().splitlines(), str(direct)

    lines: list[str] = []
    for f in sorted(Path(evidence_dir).glob("*.md")):
        if f.name == "README.md":
            continue
        lines.extend(f.read_text().splitlines())
    return lines, f"{evidence_dir}/*.md (scanned)"


def report_phase(phase_id: str, map_path: str, evidence_dir: str) -> None:
    map_lines = Path(map_path).read_text().splitlines()
    span = phase_span(map_lines, phase_id)
    if span is None:
        print(f"discover.py: no phase heading matching `Phase {phase_id}` in {map_path}")
        return
    start, end, heading = span
    boxes = parse_box_lines(map_lines[start:end])
    open_boxes = [(ln + start, m, t) for ln, m, t in boxes if m == BOX_OPEN]

    print(f"discover.py: {heading}")
    if open_boxes:
        print(f"  {len(open_boxes)} open box(es):")
        for line_no, _marker, text in open_boxes:
            print(f"    {map_path}:{line_no}: {BOX_OPEN} {text}")
    else:
        print("  no open boxes in this phase.")

    evidence_lines, source = phase_evidence_lines(evidence_dir, phase_id)
    paths = phase_evidence_paths(evidence_lines, phase_id)
    if paths:
        print(f"  {len(paths)} file(s) named in this phase's evidence-ledger entries ({source}):")
        for p in paths:
            print(f"    {p}")
    else:
        print(f"  no file paths found in {source} for this phase.")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--seam", help="symbol to find call sites of, e.g. Type::method")
    group.add_argument("--phase", help="phase id to print open boxes and evidence files for, e.g. 9I")
    parser.add_argument("--src-root", default=DEFAULT_SRC_ROOT)
    parser.add_argument("--map", default=DEFAULT_MAP)
    parser.add_argument("--evidence", default=DEFAULT_EVIDENCE,
                         help="directory of split evidence-ledger files, e.g. docs/product/evidence")
    args = parser.parse_args(argv)

    if args.seam:
        if not Path(args.src_root).is_dir():
            print(f"discover.py: {args.src_root} is not a directory", file=sys.stderr)
            return 2
        report_seam(args.seam, args.src_root)
        return 0

    if not Path(args.map).exists():
        print(f"discover.py: map {args.map} does not exist", file=sys.stderr)
        return 2
    if not Path(args.evidence).is_dir():
        print(f"discover.py: evidence directory {args.evidence} does not exist", file=sys.stderr)
        return 2
    report_phase(args.phase, args.map, args.evidence)
    return 0


if __name__ == "__main__":
    sys.exit(main())
