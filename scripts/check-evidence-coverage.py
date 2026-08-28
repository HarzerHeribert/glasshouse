#!/usr/bin/env python3
"""Every phase with a ticked box should have an evidence file behind it.

CLAUDE.md's rule is `Do not check a box until its evidence-ledger entry is
COMPLETE`. Nothing enforced that, so this does — at the phase level, which is
the granularity the ledger actually writes at (entries cover a group of related
lines, not one box each).

**Now enforced.** It was warn-only while a backlog existed — a gate that starts
red teaches everyone to override it — and the backlog is gone: Phase 0 was the
last uncovered phase, and writing its entry cost two of its own ticked boxes,
which is exactly what the check is for. `ci-local.sh` runs it with `--strict`,
so a box ticked without an evidence entry now fails the gate.

Phase-level granularity is deliberate: the ledger writes entries covering a
group of related lines, not one per box.

**Second check: the state vocabulary.** `agent-sdlc.md` defines exactly six
evidence states, and `CLAUDE.md`'s one rule about the ledger — *do not check a
box until its entry is `COMPLETE`* — is a claim about that vocabulary. Nothing
read what an entry actually declared, so entries accumulated states the SDLC
never defined: `VERIFIED`, `CLOSED`, `NOT ATTEMPTED`, `BLOCKED`, `REFERRED UP`.
Each is a reasonable sentence and none of them is a state, which means the rule
could not be checked mechanically for those entries at all.

This is **warn-only** unless `--strict-vocabulary` is passed, deliberately and
per practice §51: a backlog exists right now, and a gate that starts red is a
gate everyone learns to override. `--strict` (the coverage half) is already
enforced in `ci-local.sh`; promote the vocabulary half the same way once the
backlog is cleared, which is the same path the coverage check itself took.

A state line may carry prose after the state — `LOCALLY VERIFIED, by
construction`, `COMPLETE for eleven of the phase's twelve lines` — and that is
fine and useful. Only the leading token is checked, and it is matched
longest-first so `LOCALLY VERIFIED` is never read as a bare `VERIFIED`.
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


def evidence_phases(evidence_dir):
    """Every phase an evidence entry claims to be about.

    Filenames are not enough, and trusting them overstated the gap. Entries live
    under headings like `### Phase 19 — portable checkpoints`, and some sit in
    `unfiled.md` or under a heading that names the phase mid-sentence
    (`### Migration 7 — …`). Matching filenames alone reported Phases 13 and 19
    as uncovered when both have entries — a false alarm that would have sent a
    worker to write evidence that already exists.

    So read the headings, and keep the filename as a second source.
    """
    found = set()
    for path in glob.glob(os.path.join(evidence_dir, "*.md")):
        base = os.path.basename(path)
        if base.startswith("phase-"):
            # A filename may name several phases: `phase-12-18-and-19.md`,
            # `phase-9f-preflight.md`. Reading the stem as one opaque key
            # matched none of them and reported three covered phases as bare —
            # which is how this check twice overstated the gap. Split it.
            stem = base[len("phase-"):-len(".md")].lower()
            for part in re.split(r"-and-|-", stem):
                if re.fullmatch(r"[0-9]+[a-z]?", part):
                    found.add(part)
        for line in open(path, encoding="utf-8"):
            if not line.startswith("#"):
                continue
            for m in re.finditer(r"Phase ([0-9]+[A-Z]?)", line):
                found.add(m.group(1).lower())
    return found


# The six states `docs/process/agent-sdlc.md` defines, longest first so that
# `LOCALLY VERIFIED` is matched before a bare `VERIFIED` could be. Order is
# load-bearing; do not sort this alphabetically.
SDLC_STATES = [
    "PARTIALLY VERIFIED",
    "LOCALLY VERIFIED",
    "CI VERIFIED",
    "NOT STARTED",
    "SCAFFOLDED",
    "COMPLETE",
]


def declared_states(evidence_dir):
    """Every `State:` declaration in the ledger, as (file, line_no, raw value).

    Entries write the state with varying markdown emphasis (`State:`,
    `**State:**`, `State: **COMPLETE**`) and usually continue into prose on the
    same line. This returns the value verbatim; `state_token` normalizes it.
    """
    out = []
    for path in sorted(glob.glob(os.path.join(evidence_dir, "*.md"))):
        for line_no, line in enumerate(open(path, encoding="utf-8"), 1):
            m = re.match(r"^\s*\**State\**\s*:\s*(.*)$", line)
            if m:
                out.append((path, line_no, m.group(1).rstrip()))
    return out


def state_token(raw):
    """The SDLC state a declaration leads with, or None.

    Strips markdown emphasis and leading punctuation, then matches the longest
    known state as a prefix. Anything after it is prose and is not this
    check's business.
    """
    text = re.sub(r"[*_`]", "", raw).strip()
    for state in SDLC_STATES:
        if text.upper().startswith(state):
            return state
    return None


def check_vocabulary(evidence_dir, strict):
    """Warn on entries whose declared state is not one the SDLC defines."""
    declarations = declared_states(evidence_dir)
    offenders = [(p, n, raw) for p, n, raw in declarations if state_token(raw) is None]

    print(f"evidence vocabulary: {len(declarations) - len(offenders)}/"
          f"{len(declarations)} State: declarations use an SDLC-defined state")

    if not offenders:
        return 0

    print(f"\n  {len(offenders)} declaration(s) use a state "
          f"`docs/process/agent-sdlc.md` does not define:")
    for path, line_no, raw in offenders:
        shown = raw if len(raw) <= 72 else raw[:69] + "..."
        print(f"    {path}:{line_no}: {shown}")
    print("\n  The six defined states are: " + ", ".join(sorted(SDLC_STATES)) + ".")
    print("  CLAUDE.md's rule is about COMPLETE; a state outside the vocabulary")
    print("  cannot be checked against it. Reword the entry, or add the state to")
    print("  the SDLC deliberately.")
    return 1 if strict else 0


def normalize_ws(text):
    """Collapse every run of whitespace to one space.

    Evidence files hard-wrap at ~76 columns; the map stores each box as one
    long unwrapped line. Comparing them needs this on both sides -
    `scripts/validate_round.py` carries the same helper for the same reason.
    """
    return re.sub(r"\s+", " ", text).strip()


# The verdicts an entry uses for a line it is NOT closing. A ticked box whose
# own entry says one of these is the defect this check exists for.
OPEN_VERDICTS = ("PARTIAL", "OPEN", "BLOCKED", "NOT STARTED")


def map_box_texts(map_path):
    """Every box line's normalized text -> (line number, is_ticked)."""
    boxes = {}
    with open(map_path, encoding="utf-8") as handle:
        for line_no, raw in enumerate(handle, start=1):
            if not raw.startswith(("\u2611", "\u2610")):
                continue
            boxes[normalize_ws(raw[1:])] = (line_no, raw.startswith("\u2611"))
    return boxes


def self_inconsistent(map_path, evidence_dir):
    """Ticked boxes whose own evidence entry calls them PARTIAL/OPEN/BLOCKED.

    Map line 1330 was ticked from its entry's *summary* line while that same
    entry's per-line disposition read `PARTIAL. ... open on purpose alone`.
    Both sentences were in the same file. Nothing compared them.

    The entry quotes the box text in bold and follows it with a bolded verdict,
    so the pair is findable. Whitespace is normalized across the whole document
    first because evidence files hard-wrap at ~76 columns and the map stores
    each box as one long line - practice section 49's join-then-normalize, and
    section 42's rule that prose a file has wrapped will not match a search for it.
    """
    boxes = map_box_texts(map_path)
    pair = re.compile(
        r"\*\*(?P<quote>[^*]{20,}?)\*\*\s*\*\*(?P<verdict>"
        + "|".join(OPEN_VERDICTS)
        + r")\b"
    )
    findings = []
    for path in sorted(glob.glob(os.path.join(evidence_dir, "*.md"))):
        with open(path, encoding="utf-8") as handle:
            flat = normalize_ws(handle.read())
        for match in pair.finditer(flat):
            quote = normalize_ws(match.group("quote"))
            entry = boxes.get(quote)
            if entry is None:
                continue
            line_no, is_ticked = entry
            if is_ticked:
                findings.append((path, line_no, match.group("verdict"), quote))
    return findings


def check_self_consistency(map_path, evidence_dir, strict):
    findings = self_inconsistent(map_path, evidence_dir)
    if not findings:
        print("evidence self-consistency: clean - no ticked box is called "
              "PARTIAL/OPEN/BLOCKED by its own entry")
        return 0
    print(f"\n  {len(findings)} ticked box(es) contradicted by their own evidence entry:")
    for path, line_no, verdict, quote in findings:
        shown = quote if len(quote) <= 66 else quote[:63] + "..."
        print(f"    map:{line_no} is ticked, {os.path.basename(path)} says {verdict}")
        print(f"      {shown}")
    print("\n  An entry's summary and its own per-line disposition must agree.")
    print("  Either the box is not COMPLETE and should be unticked, or the")
    print("  disposition is stale and should be updated. Do not leave both.")
    return 1 if strict else 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--map", default=MAP)
    ap.add_argument("--evidence", default=EVIDENCE)
    ap.add_argument("--strict", action="store_true",
                    help="exit non-zero on any uncovered phase")
    ap.add_argument("--strict-vocabulary", action="store_true",
                    help="exit non-zero on any State: outside the SDLC's six "
                         "(warn-only by default while a backlog exists — §51)")
    ap.add_argument("--strict-consistency", action="store_true",
                    help="exit non-zero when a ticked box is called "
                         "PARTIAL/OPEN/BLOCKED by its own evidence entry")
    args = ap.parse_args()

    if not os.path.exists(args.map):
        print(f"check-evidence-coverage: no map at {args.map}", file=sys.stderr)
        return 2

    vocabulary_rc = check_vocabulary(args.evidence, args.strict_vocabulary)
    print()
    consistency_rc = check_self_consistency(args.map, args.evidence,
                                            args.strict_consistency)
    print()

    ticked = ticked_by_phase(args.map)
    have = evidence_phases(args.evidence)

    uncovered = []
    for phase, count in sorted(ticked.items(), key=lambda kv: kv[0]):
        key = phase.replace("Phase ", "").lower()
        # `phase-9f-preflight.md` covers Phase 9F; match the stem or a
        # hyphenated extension of it, never a longer number (`9` vs `9a`).
        if not any(h == key or h.startswith(key + "-") for h in have):
            uncovered.append((phase, count))

    covered = len(ticked) - len(uncovered)
    print(f"evidence coverage: {covered}/{len(ticked)} phases with ticked boxes "
          f"have evidence ({len(have)} phases referenced in the ledger)")

    if not uncovered:
        return vocabulary_rc or consistency_rc

    boxes = sum(c for _, c in uncovered)
    print(f"\n  {len(uncovered)} phase(s), {boxes} ticked box(es), with no evidence entry:")
    for phase, count in uncovered:
        print(f"    {phase}: {count} ticked")
    print("\n  A ticked box without an evidence entry is a claim with nothing behind it.")
    print("  Either write the entry, or untick the box.")
    return 1 if args.strict else (vocabulary_rc or consistency_rc)


if __name__ == "__main__":
    sys.exit(main())
