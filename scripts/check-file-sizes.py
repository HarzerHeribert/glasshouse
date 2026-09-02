#!/usr/bin/env python3
"""Phase 59's size ratchet: no production file grows, and the large ones shrink.

Counts *production* lines per Rust source file under `crates/glasshouse/src`
— everything before the file's inline `mod tests` — and fails when a file is
over the ceiling unless the baseline lists it at a size it has not exceeded.
The baseline is the ratchet: a decomposition package lowers its entries (or
removes them once the file is under the ceiling), and nothing can quietly
grow a file back. `--update` rewrites the baseline from the tree, which is
how a package records the shrink it made; the reviewer diffs that file.

    python3 scripts/check-file-sizes.py            # exit 1 on any violation
    python3 scripts/check-file-sizes.py --update   # rewrite the baseline
    python3 scripts/check-file-sizes.py --report   # every file over the ceiling, largest first

Why production lines and not the whole file: a 3,000-line inline test module
is its own problem (map line 59.3 moves it beside the module), and counting it
here would let a file "shrink" by deleting tests. Why a ratchet and not a hard
ceiling: twelve files were over 2,500 on 2026-09-03 and the point is to move
them down one package at a time without blocking every other package while
they are.
"""
import os
import re
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(REPO, "crates", "glasshouse", "src")
BASELINE = os.path.join(REPO, "scripts", "file-size-baseline.txt")
CEILING = 2500
TESTS_RE = re.compile(r"^(pub(\(crate\))?\s+)?mod tests\b")


def production_lines(path):
    with open(path, encoding="utf-8") as fh:
        for index, line in enumerate(fh):
            if TESTS_RE.match(line):
                return index
    with open(path, encoding="utf-8") as fh:
        return sum(1 for _ in fh)


def measure():
    sizes = {}
    for root, _, files in os.walk(SRC):
        for name in files:
            if name.endswith(".rs"):
                path = os.path.join(root, name)
                sizes[os.path.relpath(path, REPO)] = production_lines(path)
    return sizes


def read_baseline():
    baseline = {}
    if not os.path.exists(BASELINE):
        return baseline
    with open(BASELINE, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            size, path = line.split(None, 1)
            baseline[path] = int(size)
    return baseline


def write_baseline(sizes):
    over = sorted(((s, p) for p, s in sizes.items() if s > CEILING), reverse=True)
    with open(BASELINE, "w", encoding="utf-8") as fh:
        fh.write("# Phase 59 size ratchet — production lines of every file over the ceiling.\n")
        fh.write("# Regenerate with scripts/check-file-sizes.py --update after a decomposition.\n")
        fh.write("# ceiling %d\n" % CEILING)
        for size, path in over:
            fh.write("%6d %s\n" % (size, path))
    return over


def main(argv):
    sizes = measure()
    if "--update" in argv:
        over = write_baseline(sizes)
        print("check-file-sizes: baseline rewritten, %d file(s) over %d" % (len(over), CEILING))
        return 0
    if "--report" in argv:
        for size, path in sorted(((s, p) for p, s in sizes.items() if s > CEILING), reverse=True):
            print("%6d %s" % (size, path))
        return 0
    baseline = read_baseline()
    failures = []
    for path, size in sorted(sizes.items()):
        if size <= CEILING:
            continue
        allowed = baseline.get(path)
        if allowed is None:
            failures.append("%s: %d production lines, over the %d ceiling and not in the baseline" % (path, size, CEILING))
        elif size > allowed:
            failures.append("%s: %d production lines, grew past its baseline of %d" % (path, size, allowed))
    for path, allowed in baseline.items():
        if path in sizes and sizes[path] <= CEILING:
            failures.append("%s: now %d lines, under the ceiling — remove it from the baseline (--update)" % (path, sizes[path]))
    if failures:
        print("check-file-sizes: FAILED")
        for failure in failures:
            print("  " + failure)
        print("  a decomposition package lowers a baseline entry; nothing raises one")
        return 1
    print("check-file-sizes: ok (%d file(s) over %d, none grown)" % (
        sum(1 for s in sizes.values() if s > CEILING), CEILING))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
