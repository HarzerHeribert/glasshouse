#!/usr/bin/env python3
"""The baked snapshot's shape, and the two limits it must be able to carry.

`scripts/bake-model-index.py` writes the model measurements the gateway ships,
so an install or an update carries current figures with no key and no network.
These tests pin the parts that are easy to break silently: which fields survive
the bake, and — since 2026-09-17, when a second source began supplying them —
how the two real limits are matched to our own model names.

**A wrong limit is worse than an absent one.** A window attached to the wrong
model makes a session compact against a figure that was never true, which is
exactly the failure the whole change set exists to end. So the matching rules
are conservative by construction and tested that way: the vendor's own row
decides, a disagreement between providers is skipped rather than resolved by
preference, and an unmatched model keeps both fields absent.

No test here reaches the network: every bake passes `--no-limits` or a local
limits file.
"""
from __future__ import annotations

import importlib.util
import json
import pathlib
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
SCRIPT = ROOT / "scripts" / "bake-model-index.py"


def load():
    spec = importlib.util.spec_from_file_location("bake_model_index", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class BakeModelIndex(unittest.TestCase):
    def bake(self, catalogue, limits=None):
        """One bake into a temporary file. `limits` absent means `--no-limits`."""
        module = load()
        with tempfile.TemporaryDirectory() as directory:
            out = pathlib.Path(directory) / "model-index.json"
            module.OUT = out
            source = pathlib.Path(directory) / "catalogue.json"
            source.write_text(json.dumps(catalogue))
            argv = [str(source)]
            if limits is None:
                argv.append("--no-limits")
            else:
                where = pathlib.Path(directory) / "limits.json"
                where.write_text(limits if isinstance(limits, str) else json.dumps(limits))
                argv += ["--limits", str(where)]
            module.main(argv)
            return json.loads(out.read_text())

    # --- the field list, which is what stands between a source publishing a
    # --- figure and a session reading it ------------------------------------

    def test_the_two_real_limits_survive_the_bake(self):
        baked = self.bake(
            {
                "index_version": 4.3,
                "models": {
                    "m": {
                        "name": "M",
                        "intelligence": 50.0,
                        "context_window_tokens": 400000,
                        "max_output_tokens": 128000,
                    }
                },
            }
        )
        self.assertEqual(baked["models"]["m"]["context_window_tokens"], 400000)
        self.assertEqual(baked["models"]["m"]["max_output_tokens"], 128000)

    def test_an_absent_limit_stays_absent_rather_than_becoming_a_zero(self):
        baked = self.bake({"models": {"m": {"name": "M", "intelligence": 50.0}}})
        self.assertNotIn("context_window_tokens", baked["models"]["m"])
        self.assertNotIn("max_output_tokens", baked["models"]["m"])

    def test_a_row_carrying_only_a_window_is_still_worth_shipping(self):
        baked = self.bake({"models": {"m": {"context_window_tokens": 200000}}})
        self.assertEqual(baked["models"]["m"]["context_window_tokens"], 200000)

    def test_the_shipped_snapshot_is_what_this_script_produces(self):
        shipped = json.loads(
            (ROOT / "crates" / "inference-gateway" / "data" / "model-index.json").read_text()
        )
        self.assertIn("models", shipped)
        self.assertGreater(len(shipped["models"]), 100)
        self.assertEqual(shipped["source"], "Artificial Analysis (artificialanalysis.ai)")
        # The second source is named in the file itself, with the fields it is
        # allowed to have filled -- provenance a reader can check.
        self.assertIn("LiteLLM", shipped["limits_source"])
        self.assertEqual(
            shipped["limits_fields"], ["context_window_tokens", "max_output_tokens"]
        )
        carried = [
            slug
            for slug, facts in shipped["models"].items()
            if "context_window_tokens" in facts
        ]
        self.assertGreater(len(carried), 100)
        self.assertFalse(
            [
                slug
                for slug, facts in shipped["models"].items()
                if facts.get("context_window_tokens") == 0
                or facts.get("max_output_tokens") == 0
            ],
            "an unknown limit is absent, never a zero",
        )

    # --- the merge ----------------------------------------------------------

    def test_the_second_source_fills_only_what_the_primary_lacks(self):
        baked = self.bake(
            {"models": {"m": {"name": "M", "intelligence": 50.0}}},
            limits={"m": {"mode": "chat", "max_input_tokens": 200000, "max_output_tokens": 64000}},
        )
        self.assertEqual(baked["models"]["m"]["context_window_tokens"], 200000)
        self.assertEqual(baked["models"]["m"]["max_output_tokens"], 64000)
        self.assertEqual(baked["models"]["m"]["intelligence"], 50.0)

    def test_the_primary_wins_where_both_carry_a_figure(self):
        baked = self.bake(
            {"models": {"m": {"name": "M", "context_window_tokens": 400000}}},
            limits={"m": {"mode": "chat", "max_input_tokens": 200000, "max_output_tokens": 64000}},
        )
        self.assertEqual(baked["models"]["m"]["context_window_tokens"], 400000)
        # The field the primary did not carry is still filled.
        self.assertEqual(baked["models"]["m"]["max_output_tokens"], 64000)

    def test_a_model_the_second_source_does_not_know_keeps_both_absent(self):
        baked = self.bake(
            {"models": {"stranger": {"name": "S", "intelligence": 10.0}}},
            limits={"m": {"mode": "chat", "max_input_tokens": 200000}},
        )
        self.assertNotIn("context_window_tokens", baked["models"]["stranger"])
        self.assertNotIn("max_output_tokens", baked["models"]["stranger"])

    # --- matching, which is where a wrong figure would come from ------------

    def test_the_vendors_own_row_beats_a_rehost_that_caps_it(self):
        """Measured 2026-09-17: Anthropic publishes 200000 for claude-opus-4-5
        and GitHub Copilot re-hosts it at 128000."""
        baked = self.bake(
            {"models": {"claude-opus-4-5": {"name": "C"}}},
            limits={
                "claude-opus-4-5": {"mode": "chat", "max_input_tokens": 200000, "max_output_tokens": 64000},
                "github_copilot/claude-opus-4.5": {
                    "mode": "chat",
                    "max_input_tokens": 128000,
                    "max_output_tokens": 16000,
                },
            },
        )
        self.assertEqual(baked["models"]["claude-opus-4-5"]["context_window_tokens"], 200000)
        self.assertEqual(baked["models"]["claude-opus-4-5"]["max_output_tokens"], 64000)

    def test_providers_that_disagree_with_no_vendor_row_are_skipped(self):
        baked = self.bake(
            {"models": {"m": {"name": "M", "intelligence": 1.0}}},
            limits={
                "rehost_one/m": {"mode": "chat", "max_input_tokens": 128000},
                "rehost_two/m": {"mode": "chat", "max_input_tokens": 32768},
            },
        )
        self.assertNotIn("context_window_tokens", baked["models"]["m"])

    def test_an_effort_variant_inherits_the_models_own_window(self):
        """`-high` is a reasoning effort, not a different model, and a context
        window belongs to the model."""
        baked = self.bake(
            {"models": {"claude-opus-5-high": {"name": "C high"}}},
            limits={"claude-opus-5": {"mode": "chat", "max_input_tokens": 1000000, "max_output_tokens": 128000}},
        )
        self.assertEqual(baked["models"]["claude-opus-5-high"]["context_window_tokens"], 1000000)

    def test_a_suffix_that_is_not_an_effort_is_never_stripped(self):
        """`-mini` and `-codex` name different models; only the listed effort
        words are removed."""
        baked = self.bake(
            {"models": {"gpt-5-1-codex-mini": {"name": "mini", "intelligence": 1.0}}},
            limits={"gpt-5-1-codex": {"mode": "chat", "max_input_tokens": 400000}},
        )
        self.assertNotIn("context_window_tokens", baked["models"]["gpt-5-1-codex-mini"])

    def test_a_row_that_is_not_a_conversational_model_is_ignored(self):
        baked = self.bake(
            {"models": {"m": {"name": "M", "intelligence": 1.0}}},
            limits={"m": {"mode": "image_generation", "max_input_tokens": 2600}},
        )
        self.assertNotIn("context_window_tokens", baked["models"]["m"])

    def test_the_name_is_normalised_the_way_our_own_keys_are(self):
        baked = self.bake(
            {"models": {"gpt-5-6-sol": {"name": "Sol"}}},
            limits={"gpt-5.6-sol": {"mode": "chat", "max_input_tokens": 922000}},
        )
        self.assertEqual(baked["models"]["gpt-5-6-sol"]["context_window_tokens"], 922000)

    # --- the bake must survive a source it cannot reach ---------------------

    def test_an_unreachable_limits_source_leaves_the_primary_figures_intact(self):
        module = load()
        with tempfile.TemporaryDirectory() as directory:
            out = pathlib.Path(directory) / "model-index.json"
            module.OUT = out
            source = pathlib.Path(directory) / "catalogue.json"
            source.write_text(json.dumps({"models": {"m": {"name": "M", "intelligence": 50.0}}}))
            missing = pathlib.Path(directory) / "not-here.json"
            module.main([str(source), "--limits", str(missing)])
            baked = json.loads(out.read_text())
        self.assertEqual(baked["models"]["m"]["intelligence"], 50.0)
        self.assertNotIn("context_window_tokens", baked["models"]["m"])
        # Nothing claims a source that did not contribute.
        self.assertNotIn("limits_source", baked)

    def test_an_unreadable_limits_source_is_reported_not_guessed(self):
        baked = self.bake(
            {"models": {"m": {"name": "M", "intelligence": 50.0}}},
            limits="{ this is not json",
        )
        self.assertEqual(baked["models"]["m"]["intelligence"], 50.0)
        self.assertNotIn("context_window_tokens", baked["models"]["m"])
        self.assertNotIn("limits_source", baked)


if __name__ == "__main__":
    unittest.main()
