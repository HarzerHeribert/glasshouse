#!/usr/bin/env python3
"""Refuse a round of packets that cannot be right.

    scripts/validate_round.py .agent-runtime/packet-a.md .agent-runtime/packet-b.md

Read-only. Never edits a packet or the map. Exits non-zero with a reason that
names the offending file and line the first time any of the following fails:

 1. every pair of packets partitions its files disjointly (YOURS vs YOURS,
    YOURS vs FORBIDDEN);
 2. every YOURS path exists on disk, or is marked `(new)`;
 3. every quoted `☐`/`☑` box line matches the capability map verbatim;
 4. a packet's own FORBIDDEN list does not also claim a path in its own YOURS;
 5. every YOURS block is non-empty.

Exits 0, printing a clean summary, only when every packet passes every check.
"""
from __future__ import annotations

import argparse
import difflib
import fnmatch
import glob
import re
import sys
from pathlib import Path

BOX_OPEN = "☐"
BOX_DONE = "☑"
BOX_CHARS = (BOX_OPEN, BOX_DONE)

DEFAULT_MAP = "docs/product/capability-map.md"


class Finding:
    def __init__(self, check: str, message: str):
        self.check = check
        self.message = message

    def __str__(self) -> str:
        return f"[{self.check}] {self.message}"


# ---------------------------------------------------------------- parsing --


def parse_indented_block(lines: list[str], header_idx: int) -> list[tuple[int, str]]:
    """Collect the indented lines following a `**HEADER**` line.

    Stops at the first non-blank, non-indented line (a dedent), tolerating
    blank lines inside the block as long as more indentation follows them.
    Returns a list of (1-indexed line number, stripped text).
    """
    items: list[tuple[int, str]] = []
    n = len(lines)
    i = header_idx + 1
    while i < n and lines[i].strip() == "":
        i += 1
    while i < n:
        line = lines[i]
        if line.strip() == "":
            j = i + 1
            while j < n and lines[j].strip() == "":
                j += 1
            if j < n and (lines[j].startswith(" ") or lines[j].startswith("\t")):
                i = j
                continue
            break
        if not (line.startswith(" ") or line.startswith("\t")):
            break
        items.append((i + 1, line.strip()))
        i += 1
    return items


def parse_box_lines(lines: list[str]) -> list[tuple[int, str, str]]:
    """Find every `☐`/`☑` line, joining any indented continuation
    lines a wrapped quote leaves without their own marker.

    Returns (1-indexed start line, marker, joined text).
    """
    boxes: list[tuple[int, str, str]] = []
    n = len(lines)
    i = 0
    while i < n:
        stripped = lines[i].strip()
        if stripped[:1] in BOX_CHARS:
            marker = stripped[0]
            parts = [stripped[1:].strip()]
            start = i + 1
            j = i + 1
            while j < n:
                nxt = lines[j]
                nxt_stripped = nxt.strip()
                if nxt_stripped == "" or nxt_stripped[:1] in BOX_CHARS:
                    break
                if not (nxt.startswith("    ") or nxt.startswith("\t")):
                    break
                parts.append(nxt_stripped)
                j += 1
            boxes.append((start, marker, " ".join(parts)))
            i = j
        else:
            i += 1
    return boxes


def normalize_ws(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip()


def parse_pattern(raw: str) -> tuple[str, bool, str | None]:
    """Split a partition-list line into (pattern, is_new, except_pattern)."""
    text = raw.strip()
    is_new = False
    m = re.search(r"\(([^)]*)\)\s*$", text)
    if m:
        note = m.group(1).strip().lower()
        if "new" in note:
            is_new = True
        text = text[: m.start()].strip()
    except_pattern = None
    m = re.search(r"\bexcept\b", text)
    if m:
        base, exc = text[: m.start()], text[m.end():]
        text = base.strip().rstrip(",")
        except_pattern = exc.strip().rstrip(".")
    m = re.match(r"^every\s+(\S+)\s+at the repository root$", text)
    if m:
        text = m.group(1)
    return text, is_new, except_pattern


def pattern_kind(pattern: str) -> tuple[str, str]:
    if pattern.endswith("/**"):
        return "dirglob", pattern[:-3]
    if "*" in pattern and "/" not in pattern:
        return "rootglob", pattern
    if "*" in pattern:
        return "glob", pattern
    return "exact", pattern


def matches_exception(path: str, exc: str) -> bool:
    """An `except` clause is written short (`session/lifecycle.rs`) against a
    YOURS entry that is a full repo-relative path — match by suffix."""
    return path == exc or path.endswith("/" + exc) or exc.endswith("/" + path)


def patterns_overlap(a: str, b: str) -> bool:
    ka, pa = pattern_kind(a)
    kb, pb = pattern_kind(b)
    if ka == "exact" and kb == "exact":
        return pa == pb
    if ka == "dirglob" or kb == "dirglob":
        dirp, other, other_kind = (pa, pb, kb) if ka == "dirglob" else (pb, pa, ka)
        if other_kind == "dirglob":
            return dirp == other or dirp.startswith(other + "/") or other.startswith(dirp + "/")
        if other_kind == "rootglob":
            return False
        if other_kind == "glob":
            return other.startswith(dirp + "/") or fnmatch.fnmatch(dirp, other)
        return other == dirp or other.startswith(dirp + "/")
    if ka == "rootglob" or kb == "rootglob":
        rootp, other, other_kind = (pa, pb, kb) if ka == "rootglob" else (pb, pa, ka)
        if other_kind != "exact" or "/" in other:
            return other_kind == "rootglob" and pa == pb
        return fnmatch.fnmatch(other, rootp)
    if ka == "glob" or kb == "glob":
        globp, other = (pa, pb) if ka == "glob" else (pb, pa)
        return fnmatch.fnmatch(other, globp) or other == globp
    return pa == pb


def path_exists(pattern: str) -> bool:
    kind, base = pattern_kind(pattern)
    if kind == "dirglob":
        return Path(base).is_dir()
    if kind in ("rootglob", "glob"):
        return len(glob.glob(pattern, recursive=True)) > 0
    return Path(pattern).exists()


class Packet:
    def __init__(self, path: str):
        self.path = path
        text = Path(path).read_text()
        self.lines = text.splitlines()
        self.yours: list[tuple[int, str, bool, str | None]] = []
        self.forbidden: list[tuple[int, str, bool, str | None]] = []
        for idx, line in enumerate(self.lines):
            stripped = line.strip()
            if re.match(r"^\*\*YOURS\*\*", stripped):
                for line_no, raw in parse_indented_block(self.lines, idx):
                    pattern, is_new, exc = parse_pattern(raw)
                    self.yours.append((line_no, pattern, is_new, exc))
            elif re.match(r"^\*\*FORBIDDEN", stripped):
                for line_no, raw in parse_indented_block(self.lines, idx):
                    pattern, is_new, exc = parse_pattern(raw)
                    self.forbidden.append((line_no, pattern, is_new, exc))
        self.boxes = parse_box_lines(self.lines)


# ------------------------------------------------------------------ checks --


def check_partitions_disjoint(packets: list[Packet], findings: list[Finding]) -> None:
    """Two packets both listing a path in YOURS is the real failure (the same
    file handed to two live workers). A path in A's YOURS also appearing in
    B's FORBIDDEN is the *correct*, expected shape of a disjoint partition —
    not checked here — so only YOURS is compared against YOURS."""
    for i in range(len(packets)):
        for j in range(i + 1, len(packets)):
            a, b = packets[i], packets[j]
            for a_line, a_pat, _a_new, _a_exc in a.yours:
                for b_line, b_pat, _b_new, b_exc in b.yours:
                    if b_exc is not None and matches_exception(a_pat, b_exc):
                        continue
                    if patterns_overlap(a_pat, b_pat):
                        findings.append(
                            Finding(
                                "partitions-disjoint",
                                f"{a.path}:{a_line} claims `{a_pat}` and "
                                f"{b.path}:{b_line} also claims `{b_pat}` — "
                                f"colliding path is `{a_pat}`.",
                            )
                        )


def check_yours_paths_exist(packets: list[Packet], findings: list[Finding]) -> None:
    for p in packets:
        for line_no, pattern, is_new, _exc in p.yours:
            if is_new:
                continue
            if not path_exists(pattern):
                findings.append(
                    Finding(
                        "yours-paths-exist",
                        f"{p.path}:{line_no} claims `{pattern}`, which does not "
                        f"exist on disk and is not marked `(new)`.",
                    )
                )


def check_box_lines_match_map(
    packets: list[Packet], map_path: str, findings: list[Finding]
) -> None:
    map_text = Path(map_path).read_text()
    map_lines = map_text.splitlines()
    map_boxes = parse_box_lines(map_lines)
    by_text: dict[str, tuple[int, str, str]] = {}
    for line_no, marker, text in map_boxes:
        by_text[normalize_ws(text)] = (line_no, marker, text)
    for p in packets:
        for line_no, marker, text in p.boxes:
            key = normalize_ws(text)
            if key in by_text:
                continue
            close = difflib.get_close_matches(key, by_text.keys(), n=1, cutoff=0.6)
            if close:
                map_line_no, map_marker, map_text_raw = by_text[close[0]]
                findings.append(
                    Finding(
                        "box-lines-match-map",
                        f"{p.path}:{line_no} quotes `{marker} {text}` which does "
                        f"not match {map_path}:{map_line_no} verbatim — map has "
                        f"`{map_marker} {map_text_raw}`.",
                    )
                )
            else:
                findings.append(
                    Finding(
                        "box-lines-match-map",
                        f"{p.path}:{line_no} quotes `{marker} {text}` which does "
                        f"not appear anywhere in {map_path}.",
                    )
                )


def check_no_self_contradiction(packets: list[Packet], findings: list[Finding]) -> None:
    for p in packets:
        for f_line, f_pat, _f_new, f_exc in p.forbidden:
            for y_line, y_pat, _y_new, _y_exc in p.yours:
                if f_exc is not None and matches_exception(y_pat, f_exc):
                    continue
                if patterns_overlap(f_pat, y_pat):
                    findings.append(
                        Finding(
                            "no-self-contradiction",
                            f"{p.path}:{f_line} forbids `{f_pat}` but "
                            f"{p.path}:{y_line} claims `{y_pat}` in the same "
                            f"packet's YOURS.",
                        )
                    )


def check_yours_non_empty(packets: list[Packet], findings: list[Finding]) -> None:
    for p in packets:
        if not p.yours:
            findings.append(
                Finding(
                    "yours-non-empty",
                    f"{p.path} has an empty or missing YOURS block.",
                )
            )


def validate(packet_paths: list[str], map_path: str) -> list[Finding]:
    packets = [Packet(path) for path in packet_paths]
    findings: list[Finding] = []
    check_yours_non_empty(packets, findings)
    check_partitions_disjoint(packets, findings)
    check_no_self_contradiction(packets, findings)
    check_yours_paths_exist(packets, findings)
    check_box_lines_match_map(packets, map_path, findings)
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packets", nargs="+", help="packet markdown files")
    parser.add_argument(
        "--map",
        default=DEFAULT_MAP,
        help=f"capability map path (default: {DEFAULT_MAP})",
    )
    args = parser.parse_args(argv)

    for path in args.packets:
        if not Path(path).exists():
            print(f"validate_round.py: {path} does not exist", file=sys.stderr)
            return 2
    if not Path(args.map).exists():
        print(f"validate_round.py: map {args.map} does not exist", file=sys.stderr)
        return 2

    findings = validate(args.packets, args.map)
    if findings:
        print(f"validate_round.py: REFUSED — {len(findings)} problem(s):", file=sys.stderr)
        for f in findings:
            print(f"  {f}", file=sys.stderr)
        return 1

    print(f"validate_round.py: PASSED — {len(args.packets)} packet(s), no conflicts:")
    for path in args.packets:
        print(f"  {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
