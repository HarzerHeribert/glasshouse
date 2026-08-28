"""Tests for the self-consistency half of scripts/check-evidence-coverage.py.

Run with:
    python3 scripts/tests/test_evidence_consistency.py
    python3 -m unittest discover -s scripts/tests -v

Map line 1330 was ticked from its evidence entry's *summary* line while the
same entry's per-line disposition read `PARTIAL. ... open on purpose alone`.
Both sentences had been in that file for the life of the entry, and nothing
compared them: `check-evidence-coverage.py` verified an entry *existed* for a
phase and that its `State:` used a defined word, never that the entry agreed
with itself.

The load-bearing case here is `test_an_open_box_called_partial_is_not_flagged`.
An entry calling an **unticked** box `PARTIAL` is the ordinary, correct state of
most of this ledger, and a check that flagged it would fire on dozens of honest
entries and be switched off within a day - practice section 20's "a gate that
starts red teaches everyone to override it", from the other side.

`test_a_wrapped_quote_still_matches` guards the join-then-normalize step.
Evidence files hard-wrap at ~76 columns while the map stores each box as one
long line, so a matcher comparing them raw finds nothing and reports clean -
which is precisely how `validate_round.py`'s box check sat inert for four
rounds (practice section 49, and section 42's rule that wrapped prose will not
match a search for it).
"""
from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
CHECKER = REPO_ROOT / "scripts" / "check-evidence-coverage.py"


def load(path, name="checker"):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


checker = load(CHECKER)

BOX = ("Record provider, route, model identity, authenticated quota context, "
       "harness, request purpose, and observation timestamp for each measurable turn.")


class SelfConsistency(unittest.TestCase):
    def run_case(self, map_line, entry_body):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            map_path = root / "map.md"
            map_path.write_text("Phase 33A\n\n" + map_line + "\n")
            evidence = root / "evidence"
            evidence.mkdir()
            (evidence / "phase-33a.md").write_text(entry_body)
            return checker.self_inconsistent(str(map_path), str(evidence))

    def test_a_ticked_box_called_partial_by_its_own_entry_is_flagged(self):
        """The 1330 defect itself."""
        found = self.run_case("☑ " + BOX,
                              f"State: **COMPLETE** for map line 1330.\n\n**{BOX}** **PARTIAL.** open on purpose alone.\n")
        self.assertEqual(len(found), 1, found)
        self.assertEqual(found[0][2], "PARTIAL")

    def test_an_open_box_called_partial_is_not_flagged(self):
        """The ordinary, correct state of most of this ledger.

        If this ever starts failing, the check has become one that fires on
        honest entries, and it will be switched off rather than fixed.
        """
        found = self.run_case("☐ " + BOX,
                              f"**{BOX}** **PARTIAL.** open on purpose alone.\n")
        self.assertEqual(found, [])

    def test_a_ticked_box_its_entry_does_not_contradict_is_not_flagged(self):
        found = self.run_case("☑ " + BOX,
                              f"**{BOX}** **COMPLETE.** every field is written.\n")
        self.assertEqual(found, [])

    def test_a_wrapped_quote_still_matches(self):
        """Evidence hard-wraps; the map does not. Normalize both, or see nothing."""
        wrapped = BOX.replace("authenticated quota context, ",
                              "authenticated quota context,\n")
        wrapped = wrapped.replace("and observation timestamp ",
                                  "and observation\ntimestamp ")
        self.assertIn("\n", wrapped)
        found = self.run_case("☑ " + BOX, f"**{wrapped}** **PARTIAL.** wrapped.\n")
        self.assertEqual(len(found), 1, found)

    def test_every_open_verdict_is_detected(self):
        for verdict in checker.OPEN_VERDICTS:
            with self.subTest(verdict=verdict):
                found = self.run_case("☑ " + BOX,
                                      f"**{BOX}** **{verdict}.** why.\n")
                self.assertEqual(len(found), 1, f"{verdict} not detected")

    def test_the_real_ledger_is_clean(self):
        """The tree must stay in the state the fix put it in."""
        found = checker.self_inconsistent(
            str(REPO_ROOT / "docs" / "product" / "capability-map.md"),
            str(REPO_ROOT / "docs" / "product" / "evidence"),
        )
        self.assertEqual(found, [], f"self-inconsistent ticked boxes: {found}")


if __name__ == "__main__":
    unittest.main()
