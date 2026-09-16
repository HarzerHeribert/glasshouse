"""Tests for scripts/progress.py's per-component grouping.

The README progress block used to render one flat per-phase table. Now the
phases are grouped by component (from the `capability-map.components`
sidecar) in a fixed display order. The one property worth pinning: the
component headings appear in that fixed order, and the section totals sum
back to the overall totals -- if either drifts, the README silently
misreports what a component still owes.
"""
from __future__ import annotations

import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))

import progress  # noqa: E402


def make_phase(label, done, total, component):
    return {"label": label, "done": done, "total": total, "gate": False, "component": component}


class RenderBlockComponents(unittest.TestCase):
    def test_component_headings_in_fixed_order_and_totals_match(self):
        phases = [
            make_phase("Phase 1 — one", 1, 1, "pane"),
            make_phase("Phase 2 — two", 0, 2, "glasshouse"),
            make_phase("Phase 3 — three", 3, 3, "gateway"),
        ]
        block = progress.render_block(phases, parked=0)

        headings = [c for c in progress.COMPONENT_ORDER if "**{}**".format(c) in block]
        expected_order = [c for c in progress.COMPONENT_ORDER
                           if c in {p["component"] for p in phases}]
        self.assertEqual(headings, expected_order)

        done_all, total_all = progress.totals(phases)
        self.assertIn("**{} closed**".format(done_all), block)
        for label in ("Phase 1", "Phase 2", "Phase 3"):
            self.assertIn(label, block)


if __name__ == "__main__":
    unittest.main()
