#!/usr/bin/env python3
"""The sidecar coverage canary for `docs/product/capability-map.components`.

`scripts/orient.py` groups every `Phase` heading by component using a hand
maintained sidecar (`<phase id>\\t<component>` per line). A phase appended to
the map without a matching sidecar line would silently fall out of every
grouped table unless the loader refuses. These tests pin that refusal, and
that the checked-in sidecar currently covers every checked-in `Phase` heading.
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
import pathlib
import tempfile
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = REPO / "scripts" / "orient.py"

spec = importlib.util.spec_from_file_location("orient", SCRIPT)
orient = importlib.util.module_from_spec(spec)
spec.loader.exec_module(orient)


class SidecarCoverage(unittest.TestCase):
    """Exercised through the real loader against a temporary map + sidecar."""

    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.map = self.tmp / "capability-map.md"
        self.components = self.tmp / "capability-map.components"
        self.map.write_text(
            "Phase 1 — Something\n\n"
            "☐ First capability.\n"
            "☑ Second capability.\n"
            "\n"
            "Phase 2 — Something else\n\n"
            "☐ Third capability.\n"
        )
        self.addCleanup(self._restore)
        self._orig_map = orient.MAP
        self._orig_components = orient.COMPONENTS
        orient.MAP = str(self.map)
        orient.COMPONENTS = str(self.components)

    def _restore(self):
        orient.MAP = self._orig_map
        orient.COMPONENTS = self._orig_components

    def test_a_phase_missing_from_the_sidecar_exits_nonzero_and_is_named(self):
        # Phase 2 has no sidecar line.
        self.components.write_text("Phase 1\tglasshouse\n")
        stderr = io.StringIO()
        with self.assertRaises(SystemExit) as cm, \
                contextlib.redirect_stderr(stderr):
            orient.map_state()
        self.assertEqual(cm.exception.code, 1)
        self.assertIn("Phase 2", stderr.getvalue())

    def test_a_sidecar_line_naming_a_phase_the_map_lacks_exits_nonzero(self):
        self.components.write_text(
            "Phase 1\tglasshouse\n"
            "Phase 2\tglasshouse\n"
            "Phase 3\tglasshouse\n"
        )
        stderr = io.StringIO()
        with self.assertRaises(SystemExit) as cm, \
                contextlib.redirect_stderr(stderr):
            orient.map_state()
        self.assertEqual(cm.exception.code, 1)
        self.assertIn("Phase 3", stderr.getvalue())

    def test_a_complete_sidecar_raises_nothing(self):
        self.components.write_text(
            "Phase 1\tglasshouse\n"
            "Phase 2\tgateway\n"
        )
        phases = orient.map_state()
        self.assertEqual(
            {p["id"]: p["component"] for p in phases},
            {"Phase 1": "glasshouse", "Phase 2": "gateway"})


class TheCheckedInSidecarCoversTheCheckedInMap(unittest.TestCase):
    """The teeth. A phase appended to the map without a sidecar line fails
    the script tests here, not just an interactive `orient.py` run.
    """

    def test_every_phase_heading_has_a_component_line(self):
        # map_state() calls attach_components(), which raises SystemExit(1)
        # if any `Phase` heading in the real map lacks a real sidecar line.
        try:
            phases = orient.map_state()
        except SystemExit as exc:  # pragma: no cover - failure path
            self.fail(f"orient.map_state() refused: exit {exc.code}")
        mandatory = [p for p in phases if not p["experimental"]]
        self.assertTrue(mandatory)
        for p in mandatory:
            self.assertIn(p["component"],
                          {"glasshouse", "gateway", "pane", "boundary", "process"})


if __name__ == "__main__":
    unittest.main(verbosity=1)
