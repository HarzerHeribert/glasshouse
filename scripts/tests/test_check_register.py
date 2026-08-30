"""Tests for scripts/check-register.py.

Run with:
    python3 -m unittest discover -s scripts/tests -v

WHY THIS FILE EXISTS
--------------------
A refusal-register row records why a capability line cannot close. When the
blocker goes away — usually because a different package landed the missing
piece — nothing rechecks the row, and it keeps telling orchestrators not to
package work that is already done.

An audit on 2026-08-29 found seven stale rows. It cost a worker slot. The very
next day, this script found **five more** that the audit had not covered,
because all five went stale in the hours after it ran.
"""
from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "scripts" / "check-register.py"


class CheckRegisterTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())

    def write(self, name: str, text: str) -> Path:
        p = self.tmp / name
        p.write_text(text)
        return p

    def run_check(self, register: Path, cap_map: Path, *args):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--register", str(register),
             "--map", str(cap_map), *args],
            capture_output=True, text=True,
        )

    def a_map(self, lines: list[str]) -> Path:
        return self.write("map.md", "\n".join(lines) + "\n")

    def test_a_row_naming_a_ticked_line_is_stale(self):
        cap_map = self.a_map(["☐ one", "☑ two"])          # line 2 is ticked
        reg = self.write("reg.md", "| 2 | the blocker |\n")
        r = self.run_check(reg, cap_map)
        self.assertIn("STALE", r.stdout)
        self.assertIn("line 2 is CLOSED", r.stdout)

    def test_a_row_naming_an_open_line_is_not_flagged(self):
        cap_map = self.a_map(["☐ one", "☑ two"])
        reg = self.write("reg.md", "| 1 | the blocker |\n")
        self.assertIn("clean", self.run_check(reg, cap_map).stdout)

    def test_a_struck_through_row_is_already_retired(self):
        """Retirement is the fix, so a retired row must stop being a finding."""
        cap_map = self.a_map(["☐ one", "☑ two"])
        reg = self.write("reg.md", "| ~~2~~ | CLOSED, kept for the record |\n")
        self.assertIn("clean", self.run_check(reg, cap_map).stdout)

    def test_a_row_saying_CLOSED_is_also_retired(self):
        cap_map = self.a_map(["☐ one", "☑ two"])
        reg = self.write("reg.md", "| 2 | **CLOSED** by some package |\n")
        self.assertIn("clean", self.run_check(reg, cap_map).stdout)

    def test_a_bulleted_standing_refusal_is_checked_too(self):
        """The register's standing refusals are bullets, not table rows."""
        cap_map = self.a_map(["☐ one", "☑ two"])
        reg = self.write("reg.md", "- **2** — no producer exists.\n")
        self.assertIn("line 2 is CLOSED", self.run_check(reg, cap_map).stdout)

    def test_a_row_naming_several_ids_flags_only_the_ticked_ones(self):
        cap_map = self.a_map(["☐ one", "☑ two", "☐ three"])
        reg = self.write("reg.md", "| 1, 2, 3 | a shared blocker |\n")
        out = self.run_check(reg, cap_map).stdout
        self.assertIn("line 2 is CLOSED", out)
        self.assertNotIn("line 1 is CLOSED", out)
        self.assertNotIn("line 3 is CLOSED", out)

    def test_check_flag_sets_the_exit_code(self):
        cap_map = self.a_map(["☐ one", "☑ two"])
        reg = self.write("reg.md", "| 2 | the blocker |\n")
        self.assertEqual(self.run_check(reg, cap_map).returncode, 0,
                         "without --check it reports and exits 0")
        self.assertEqual(self.run_check(reg, cap_map, "--check").returncode, 1,
                         "with --check a stale row fails")

    def test_prose_outside_a_row_is_not_mistaken_for_an_id(self):
        """The file is mostly prose; only rows and bullets name ids."""
        cap_map = self.a_map(["☐ one", "☑ two"])
        reg = self.write("reg.md", "Some prose mentioning 2 in passing.\n")
        self.assertIn("clean", self.run_check(reg, cap_map).stdout)


if __name__ == "__main__":
    unittest.main()
