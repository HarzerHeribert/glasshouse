#!/usr/bin/env python3
"""Freeze a ccusage snapshot, and report Tue->Mon cycles around the plan upgrade.

THIS IS BUILD TOOLING, NOT PRODUCT CODE. It measures what building Glasshouse
costs us — our account, our plan, our workers. It is not Phase 51, which is the
product's own evaluation hooks and needs a table inside the product. Do not cite
this script to satisfy a capability-map line, and do not build plan-usage
tracking into the product because this exists.

WHY THIS EXISTS
---------------
On 2026-08-29 the account moved from a 5x to a 20x Max plan, and the question
that upgrade has to answer is not "did we use more" — of course we did — but
**did the extra capacity turn into more verified capability**. That comparison
needs three things this script provides:

1. a *frozen* pre-upgrade baseline, because ccusage can only read as far back as
   the underlying agent logs are retained, and those rotate;
2. the same capture repeated later, so before and after are the same measurement
   rather than two different hand-rolled ones;
3. cycle arithmetic on the account's real boundary — Tue 00:00, which is what
   `RL7_RESET` lands on — instead of calendar weeks that straddle it.

THE ONE RULE FOR READING IT
---------------------------
**Do not judge the upgrade from the cycle it landed in.** 2026-08-25..08-31
contains both plans and also hit the old ceiling (the 5x weekly window read 98%
hours before the upgrade). The first honest post-upgrade cycle is 2026-09-01.

USAGE
    scripts/usage-snapshot.py --capture <label>   # freeze a new snapshot
    scripts/usage-snapshot.py --report            # cycle table from every snapshot
"""
from __future__ import annotations

import argparse, collections, datetime, json, pathlib, subprocess, sys


def _main_checkout(script_dir: pathlib.Path) -> pathlib.Path:
    """The one real .agent-runtime/usage-baseline owner, regardless of which
    worktree's copy of this script is executing.

    scripts/ is tracked, so every worktree carries its own copy, and
    .agent-runtime/ is gitignored -- it exists only in the main checkout
    (continuity-watch.sh's header measured this). Resolving ROOT from
    __file__ alone silently forks the snapshot store per worktree. Reproduced
    2026-08-30 (script-tree-audit): `--report` run via a relative path from a
    worker's own worktree said "no snapshots" while the real baseline sat in
    the main checkout. git's own worktree metadata names the one real answer.
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
    common_dir = pathlib.Path(common.stdout.strip())
    if not common_dir.is_absolute():
        common_dir = (script_dir / common_dir).resolve()
    return common_dir.parent if common_dir.name == ".git" else script_dir


ROOT = _main_checkout(pathlib.Path(__file__).resolve().parents[1])
STORE = ROOT / ".agent-runtime" / "usage-baseline"

# The account's real weekly boundary. RL7_RESET lands on Tuesday 00:00 local.
CYCLE_ANCHOR_WEEKDAY = 1  # Monday=0, so Tuesday=1
UPGRADE_DATE = "2026-08-29"
UPGRADE_CYCLE = "2026-08-25"


def cycle_start(day: str) -> datetime.date:
    dt = datetime.date.fromisoformat(day)
    return dt - datetime.timedelta(days=(dt.weekday() - CYCLE_ANCHOR_WEEKDAY) % 7)


def load_rows(path: pathlib.Path) -> list[dict]:
    data = json.loads(path.read_text())
    return data.get("daily") or data.get("data") or (data if isinstance(data, list) else [])


def capture(label: str) -> int:
    STORE.mkdir(parents=True, exist_ok=True)
    stamp = datetime.datetime.now().strftime("%Y-%m-%d")
    out = STORE / f"{label}-{stamp}.json"
    try:
        proc = subprocess.run(["ccusage", "daily", "--json"],
                              capture_output=True, text=True, timeout=600)
    except FileNotFoundError:
        print("usage-snapshot: ccusage is not on PATH", file=sys.stderr)
        return 2
    except subprocess.TimeoutExpired:
        print("usage-snapshot: ccusage timed out after 600s", file=sys.stderr)
        return 2
    if proc.returncode != 0 or not proc.stdout.strip():
        # An empty file that looks like a capture is worse than no capture.
        print(f"usage-snapshot: ccusage failed (rc={proc.returncode}); wrote nothing",
              file=sys.stderr)
        return 1
    out.write_text(proc.stdout)
    rows = load_rows(out)
    if not rows:
        out.unlink()
        print("usage-snapshot: ccusage returned no rows; wrote nothing", file=sys.stderr)
        return 1
    print(f"usage-snapshot: {out.relative_to(ROOT)} — {len(rows)} days")
    return 0


def report() -> int:
    snaps = sorted(STORE.glob("*.json"))
    if not snaps:
        print(f"usage-snapshot: no snapshots in {STORE.relative_to(ROOT)}")
        return 1
    merged: dict[str, dict] = {}
    for s in snaps:
        for r in load_rows(s):
            merged[r["period"]] = r          # later snapshots win
    cyc = collections.defaultdict(lambda: {"cost": 0.0, "tok": 0, "days": 0})
    for day, r in merged.items():
        k = cycle_start(day).isoformat()
        cyc[k]["cost"] += r.get("totalCost") or 0
        cyc[k]["tok"] += r.get("totalTokens") or 0
        cyc[k]["days"] += 1
    print(f"snapshots: {', '.join(s.name for s in snaps)}")
    print(f"upgrade to 20x: {UPGRADE_DATE} (5x weekly window read 98% hours before)\n")
    print("  Tue->Mon cycle       days       cost      tokens")
    for k in sorted(cyc):
        c = cyc[k]
        end = (datetime.date.fromisoformat(k) + datetime.timedelta(days=6)).isoformat()
        if k == UPGRADE_CYCLE:
            note = "  <- BOTH PLANS: do not judge the upgrade from this cycle"
        elif k > UPGRADE_CYCLE:
            note = "  <- post-upgrade"
        elif k == max(x for x in cyc if x < UPGRADE_CYCLE):
            note = "  <- last clean pre-upgrade cycle (the baseline)"
        else:
            note = ""
        print(f"  {k}..{end[5:]}   {c['days']:>2}   ${c['cost']:>9,.0f}   {c['tok']/1e9:>5.2f}B{note}")
    print("\n  Cost is API-equivalent value, not billed spend: a Max plan is a flat fee.")
    print("  The question the upgrade has to answer is boxes closed per cycle, not")
    print("  dollars consumed — see docs/process/orchestration-measurements.md.")
    return 0


# ---------------------------------------------------------------------------
# Glasshouse-only attribution, which ccusage cannot do.
#
# `ccusage daily` is ACCOUNT-WIDE. Measured 2026-08-29: ~/.claude/projects holds
# 151 project directories, only 114 of them Glasshouse, and neither `daily` nor
# `session` carries a project path in its JSON — `session` groups by session
# UUID. So the account totals include every other repository on this machine,
# and for a five-day-old project that is most of them: ccusage's history starts
# 2026-06-02 while Glasshouse's first commit is 2026-08-24.
#
# The raw logs do carry it. Each `~/.claude/projects/<slugged-path>/<uuid>.jsonl`
# is one session, its directory names the working directory it ran in, and each
# assistant line carries `message.usage` with a timestamp. Summing those over the
# directories whose slug contains "glasshouse" is exact.
#
# CAVEAT, and it matters for a precise comparison: the raw logs timestamp in
# **UTC** (`2026-08-29T17:21:55.054Z`) while `ccusage daily` groups in **local
# time**. Around midnight Europe/Berlin the two disagree by a day, so a
# `--glasshouse` day and a `ccusage daily` day are not the same bucket. Compare
# whole cycles, never single days, or pass `-z UTC` to ccusage.
#
# Report output and cache-creation tokens, not the total. Cache reads dominate
# this workload — measured 4.94B of 4.99B in one window — and are not equivalent
# to fresh reasoning; `docs/process/assurance-economics.md` says output and
# cache-create are the signals.

PROJECTS = pathlib.Path.home() / ".claude" / "projects"


def glasshouse_usage(since: str | None) -> int:
    if not PROJECTS.is_dir():
        print(f"usage-snapshot: no {PROJECTS}", file=sys.stderr)
        return 2
    dirs = [d for d in PROJECTS.iterdir() if d.is_dir() and "glasshouse" in d.name]
    if not dirs:
        print("usage-snapshot: no glasshouse project directories", file=sys.stderr)
        return 1
    by_day = collections.defaultdict(lambda: collections.Counter())
    by_project = collections.Counter()
    sessions = 0
    for d in dirs:
        for f in d.glob("*.jsonl"):
            sessions += 1
            for line in f.open(errors="replace"):
                try:
                    o = json.loads(line)
                except Exception:
                    continue
                msg = o.get("message")
                if not isinstance(msg, dict):
                    continue
                u = msg.get("usage")
                ts = o.get("timestamp")
                if not isinstance(u, dict) or not ts:
                    continue
                day = ts[:10]
                if since and day < since:
                    continue
                c = by_day[day]
                c["out"] += u.get("output_tokens") or 0
                c["cc"] += u.get("cache_creation_input_tokens") or 0
                c["cr"] += u.get("cache_read_input_tokens") or 0
                c["in"] += u.get("input_tokens") or 0
                c["msgs"] += 1
                by_project[d.name] += u.get("output_tokens") or 0

    if not by_day:
        print("usage-snapshot: no usage rows found", file=sys.stderr)
        return 1

    print(f"Glasshouse-only usage — {len(dirs)} project dirs, {sessions} session logs")
    print("(ccusage totals are account-wide and include every other repo on this machine)\n")
    print("  day           output    cache-create     cache-read    messages")
    tot = collections.Counter()
    for day in sorted(by_day):
        c = by_day[day]
        tot.update(c)
        print(f"  {day}   {c['out']/1e6:>8.2f}M   {c['cc']/1e6:>9.1f}M   {c['cr']/1e9:>9.2f}B   {c['msgs']:>9,}")
    print(f"  {'TOTAL':<10}   {tot['out']/1e6:>8.2f}M   {tot['cc']/1e6:>9.1f}M   {tot['cr']/1e9:>9.2f}B   {tot['msgs']:>9,}")

    print("\n  busiest worker directories, by output tokens:")
    for name, out in by_project.most_common(8):
        short = name.replace("-Users-eneas-projects-", "").replace("-private-tmp-claude-501-", "tmp:")
        print(f"    {out/1e6:>7.2f}M  {short[:70]}")
    print("\n  Output and cache-create are the signals; cache reads dominate volume")
    print("  and are not equivalent to fresh reasoning (assurance-economics.md).")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--capture", metavar="LABEL")
    g.add_argument("--report", action="store_true")
    g.add_argument("--glasshouse", action="store_true",
                   help="Glasshouse-only token usage from the raw logs, which "
                        "ccusage cannot attribute")
    ap.add_argument("--since", metavar="YYYY-MM-DD",
                    help="with --glasshouse, ignore days before this")
    args = ap.parse_args()
    if args.capture:
        return capture(args.capture)
    if args.glasshouse:
        return glasshouse_usage(args.since)
    return report()


if __name__ == "__main__":
    sys.exit(main())
