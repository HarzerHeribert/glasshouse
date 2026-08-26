#!/usr/bin/env python3
"""Every phase with a ticked box should have an evidence file behind it.

CLAUDE.md's rule is `Do not check a box until its evidence-ledger entry is
COMPLETE`. Nothing enforced that, so this does — at the phase level, which is
the granularity the ledger actually writes at (entries cover a group of related
lines, not one box each).

Warn-only by default, and deliberately so: run today it names six phases with
ticked boxes and no evidence file, most of them predating the ledger discipline.
A gate that starts red teaches everyone to override it. Reconcile the backlog,
then turn on --strict and wire it into ci-local.sh.
"""
import argparse, glob, os, re, sys

MAP = "docs/product/capability-map.md"
EVIDENCE = "docs/product/evidence"


def ticked_by_phase(map_path):
    phase, out = None, {}
    for line in open(map_path, encoding="utf-8"):
        head = re.match(r"^(Phase [0-9]+[A-Z]?) — ", line.strip())
        if head:
            phase = head.group(1)
        if phase and line.startswith("☑"):
            out[phase] = out.get(phase, 0) + 1
    return out


def evidence_keys(evidence_dir):
    keys = set()
    for path in glob.glob(os.path.join(evidence_dir, "phase-*.md")):
        keys.add(os.path.basename(path)[len("phase-"):-len(".md")].lower())
    return keys


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--map", default=MAP)
    ap.add_argument("--evidence", default=EVIDENCE)
    ap.add_argument("--strict", action="store_true",
                    help="exit non-zero on any uncovered phase")
    args = ap.parse_args()

    if not os.path.exists(args.map):
        print(f"check-evidence-coverage: no map at {args.map}", file=sys.stderr)
        return 2

    ticked = ticked_by_phase(args.map)
    have = evidence_keys(args.evidence)

    uncovered = []
    for phase, count in sorted(ticked.items(), key=lambda kv: kv[0]):
        key = phase.replace("Phase ", "").lower()
        # `phase-9f-preflight.md` covers Phase 9F; match the stem or a
        # hyphenated extension of it, never a longer number (`9` vs `9a`).
        if not any(h == key or h.startswith(key + "-") for h in have):
            uncovered.append((phase, count))

    covered = len(ticked) - len(uncovered)
    print(f"evidence coverage: {covered}/{len(ticked)} phases with ticked boxes "
          f"have an evidence file ({len(have)} files present)")

    if not uncovered:
        return 0

    boxes = sum(c for _, c in uncovered)
    print(f"\n  {len(uncovered)} phase(s), {boxes} ticked box(es), with no evidence file:")
    for phase, count in uncovered:
        print(f"    {phase}: {count} ticked")
    print("\n  A ticked box without an evidence entry is a claim with nothing behind it.")
    print("  Either write the entry, or untick the box.")
    return 1 if args.strict else 0


if __name__ == "__main__":
    sys.exit(main())
