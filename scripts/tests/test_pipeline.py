"""Tests for scripts/pipeline.sh's board counting.

Run with:
    python3 -m unittest discover -s scripts/tests -v

Each test builds a throwaway repo-shaped directory so it depends on nothing in
the real checkout.

WHY THIS FILE EXISTS
--------------------
`pipeline.sh` counted directories under `.worktrees/`, "one per dispatched
worker". A **read-only recon has no worktree by design** — it works in the main
checkout because it edits nothing — so a running recon was counted as
`ready-to-dispatch`, and `--watch` fired "already written and never dispatched"
at an orchestrator who had just dispatched it. Twice in one session,
2026-08-29.

The miscount is the smaller half. A nag that fires when nothing is wrong trains
the reader to dismiss it, which is the exact failure the watch exists to
prevent — an empty board produces no events and is quiet precisely when
something is wrong.
"""
from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PIPELINE = REPO_ROOT / "scripts" / "pipeline.sh"


class PipelineCountingTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        (self.tmp / ".agent-runtime").mkdir()
        (self.tmp / "scripts").mkdir()
        # pipeline.sh resolves REPO from its own location, so it must live here.
        (self.tmp / "scripts" / "pipeline.sh").write_bytes(PIPELINE.read_bytes())
        os.chmod(self.tmp / "scripts" / "pipeline.sh", 0o755)
        # It shells out to worker-ack.sh --list for the waiting count.
        ack = self.tmp / "scripts" / "worker-ack.sh"
        ack.write_text("#!/usr/bin/env bash\necho 'no workers waiting'\n")
        os.chmod(ack, 0o755)

    def run_pipeline(self, *args):
        return subprocess.run(
            ["bash", str(self.tmp / "scripts" / "pipeline.sh"), *args],
            capture_output=True, text=True, cwd=self.tmp,
        )

    def packet(self, name: str):
        (self.tmp / ".agent-runtime" / f"packet-{name}.md").write_text("# packet\n")

    def worktree(self, name: str):
        (self.tmp / ".worktrees" / name).mkdir(parents=True)

    def marker(self, name: str):
        d = self.tmp / ".agent-runtime" / "dispatched"
        d.mkdir(exist_ok=True)
        (d / name).write_text("2026-08-29T00:00:00Z workspace:1 surface:2\n")

    def report(self, name: str):
        (self.tmp / ".agent-runtime" / f"report-{name}.md").write_text("# report\n")

    # --- the defect this file exists for -------------------------------------

    def test_a_dispatched_recon_with_no_worktree_counts_as_live(self):
        self.packet("recon")
        self.marker("recon")
        out = self.run_pipeline().stdout
        self.assertIn("live=1", out, "a marked, unreported worker is live")
        self.assertIn("ready-to-dispatch=0", out,
                      "and it must NOT also be offered as ready to dispatch")

    def test_watch_style_check_does_not_fire_when_a_recon_fills_the_floor(self):
        for n in ("edit", "recon"):
            self.packet(n)
        self.worktree("edit")
        self.marker("recon")
        self.assertEqual(self.run_pipeline("--check").returncode, 0,
                         "two workers, one worktree-less, is at the floor of 2")

    def test_a_worker_with_both_a_worktree_and_a_marker_is_counted_once(self):
        self.packet("both")
        self.worktree("both")
        self.marker("both")
        self.assertIn("live=1", self.run_pipeline().stdout)

    def test_a_marker_stops_counting_once_the_report_lands(self):
        self.packet("done")
        self.marker("done")
        self.report("done")
        out = self.run_pipeline().stdout
        self.assertIn("live=0", out, "a finished worker is not live")
        self.assertIn("ready-to-dispatch=0", out,
                      "nor is a finished packet ready to dispatch again")

    # --- the behaviour that must not regress ---------------------------------

    def test_an_undispatched_packet_is_still_ready(self):
        self.packet("fresh")
        out = self.run_pipeline().stdout
        self.assertIn("ready-to-dispatch=1", out)
        self.assertIn("live=0", out)

    def test_check_still_fails_below_the_floor(self):
        self.packet("lonely")
        self.worktree("lonely")
        self.assertEqual(self.run_pipeline("--check").returncode, 1,
                         "one worker is below the floor of 2 and must still fail")


if __name__ == "__main__":
    unittest.main()
