#!/usr/bin/env python3
"""The capability-ID drift canary, and the live index it guards.

A capability's ID is its line number in `capability-map.md`. Insert a mandatory
line mid-map and every ID below it silently means a different capability. These
tests pin the three behaviours that decide whether the canary is usable at all:

  * ticking a box is NOT drift        — the most common map edit there is; a
                                        canary that fires on it gets silenced
  * a mid-map insertion IS drift      — the actual hazard, reported with the
                                        earliest affected ID and the shift
  * appending at the end is NOT drift — where every insertion has so far landed

The last test is the one that gives this gate teeth: it asserts the checked-in
index still matches the checked-in map, so `lint / script tests` fails the whole
gate the moment someone renumbers the map without reconciling the references.
"""
from __future__ import annotations

import importlib.util
import pathlib
import subprocess
import sys
import tempfile
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = REPO / "scripts" / "map-index.py"

spec = importlib.util.spec_from_file_location("map_index", SCRIPT)
mi = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mi)


class Normalisation(unittest.TestCase):
    def test_tick_glyph_is_not_part_of_a_capabilitys_identity(self):
        self.assertEqual(mi.normalise("☐ Do the thing."),
                         mi.normalise("☑ Do the thing."))

    def test_whitespace_is_collapsed_not_significant(self):
        self.assertEqual(mi.normalise("☐ Do   the  thing."),
                         mi.normalise("☑ Do the thing."))

    def test_different_capabilities_are_different(self):
        self.assertNotEqual(mi.normalise("☐ Do the thing."),
                            mi.normalise("☐ Do the other thing."))


class DriftDetection(unittest.TestCase):
    """Exercised through the real script against a temporary map."""

    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.map = self.tmp / "capability-map.md"
        self.index = self.tmp / "capability-map.index"
        self.map.write_text(
            "Phase 1 — Something\n\n"
            "☐ First capability.\n"
            "☐ Second capability.\n"
            "☐ Third capability.\n"
        )
        mi.MAP, mi.INDEX = str(self.map), str(self.index)
        mi.write_index(mi.boxes(str(self.map)))

    def test_a_clean_map_is_stable(self):
        self.assertEqual(mi.check(), 0)

    def test_ticking_a_box_is_not_drift(self):
        self.map.write_text(self.map.read_text().replace(
            "☐ Second capability.", "☑ Second capability."))
        self.assertEqual(mi.check(), 0, "ticking must not read as drift")

    def test_a_mid_map_insertion_is_drift(self):
        lines = self.map.read_text().split("\n")
        lines[2:2] = ["☐ An inserted capability."]
        self.map.write_text("\n".join(lines))
        self.assertEqual(mi.check(), 1, "a mid-map insertion must be caught")

    def test_appending_at_the_end_is_not_drift(self):
        self.map.write_text(self.map.read_text() + "☐ An appended capability.\n")
        self.assertEqual(mi.check(), 0, "appending shifts nothing and must stay quiet")

    def test_deleting_a_capability_is_drift(self):
        self.map.write_text(self.map.read_text().replace(
            "☐ Second capability.\n", ""))
        self.assertEqual(mi.check(), 1, "a removed capability must be caught")


class TheLiveIndexIsCurrent(unittest.TestCase):
    """The teeth. Fails the gate if the real map has drifted from the real index.

    Deliberately shells out to the script rather than reusing the module state
    the tests above rewrote — this must assert about the repository, not about
    whatever globals a previous test left behind.
    """

    def test_checked_in_map_matches_checked_in_index(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--check"],
            capture_output=True, text=True, cwd=str(REPO))
        self.assertEqual(
            result.returncode, 0,
            "capability IDs have drifted from the recorded index.\n"
            "Every reference to an affected ID now names a different capability.\n"
            "Reconcile the references, THEN run scripts/map-index.py --update.\n\n"
            + result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=1)
