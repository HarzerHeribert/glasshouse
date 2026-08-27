"""Tests for the state-vocabulary half of scripts/check-evidence-coverage.py.

Run with:
    python3 scripts/tests/test_evidence_vocabulary.py
    python3 -m unittest discover -s scripts/tests -v

`docs/process/agent-sdlc.md` defines exactly six evidence states, and
`CLAUDE.md`'s one rule about the ledger — *do not check a box until its entry is
`COMPLETE`* — is a claim about that vocabulary. The check exists because entries
had accumulated states the SDLC never defined (`VERIFIED`, `CLOSED`,
`NOT ATTEMPTED`, `BLOCKED`, `REFERRED UP`), each a reasonable sentence and none
of them a state.

The load-bearing case is `test_locally_verified_is_not_read_as_bare_verified`:
`VERIFIED` is **not** a defined state but `LOCALLY VERIFIED` is, so a matcher
that tested states in the wrong order would accept every bare `VERIFIED` in the
ledger and the check would pass while proving nothing. That ordering is the one
thing here that can silently rot, so it is asserted directly rather than only
implied by the table.
"""
from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
CHECKER = REPO_ROOT / "scripts" / "check-evidence-coverage.py"


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


chk = _load("check_evidence_coverage", CHECKER)


class StateTokenTests(unittest.TestCase):
    """Which leading token a `State:` declaration is read as."""

    ACCEPTED = [
        ("COMPLETE", "COMPLETE"),
        ("**COMPLETE**", "COMPLETE"),
        ("COMPLETE. Phase 9G is nineteen of nineteen.", "COMPLETE"),
        ("COMPLETE (Unix only — see Missing evidence).", "COMPLETE"),
        ("LOCALLY VERIFIED, by construction", "LOCALLY VERIFIED"),
        ("**LOCALLY VERIFIED** (macOS/arm64 only)", "LOCALLY VERIFIED"),
        ("PARTIALLY VERIFIED — box deliberately unchecked.", "PARTIALLY VERIFIED"),
        ("CI VERIFIED on three platforms", "CI VERIFIED"),
        ("NOT STARTED, blocked on a file this package does not hold",
         "NOT STARTED"),
        ("SCAFFOLDED — the wiring is complete, proven, and reachable",
         "SCAFFOLDED"),
    ]

    # Every one of these appeared in the real ledger when the check was
    # written. They are reasonable sentences and none of them is a state.
    REJECTED = [
        "VERIFIED — all thirteen lines carry production callers",
        "**CLOSED.**",
        "**CLOSED**, with one gap and one production fix",
        "**was already CLOSED before this package; unchanged.**",
        "NOT ATTEMPTED — no test written, deliberately.",
        "NOT ATTEMPTED AS A GROUP — assessed",
        "**BLOCKED on Phase 30, and deliberately not forced.**",
        "**REFERRED UP, not ticked — the orchestrator's call**",
        "**the mechanism is COMPLETE and proven; its production caller is two",
        "DEFINITELY DONE",
        "",
    ]

    def test_defined_states_are_accepted_with_prose_after_them(self):
        for raw, expected in self.ACCEPTED:
            with self.subTest(raw=raw):
                self.assertEqual(chk.state_token(raw), expected)

    def test_states_the_sdlc_does_not_define_are_rejected(self):
        for raw in self.REJECTED:
            with self.subTest(raw=raw):
                self.assertIsNone(chk.state_token(raw))

    def test_locally_verified_is_not_read_as_bare_verified(self):
        """The ordering in SDLC_STATES is load-bearing, so assert it directly.

        Sorting SDLC_STATES alphabetically would put `COMPLETE` first and leave
        the two-word states matchable, but any ordering that tested a shorter
        state before a longer one containing it would make this check vacuous.
        """
        self.assertIsNone(chk.state_token("VERIFIED — anything at all"))
        self.assertEqual(
            chk.state_token("LOCALLY VERIFIED — anything at all"),
            "LOCALLY VERIFIED",
        )
        self.assertEqual(
            chk.state_token("PARTIALLY VERIFIED — anything at all"),
            "PARTIALLY VERIFIED",
        )

    def test_every_state_the_sdlc_defines_is_in_the_list(self):
        """The list must not drift from the document it claims to mirror."""
        sdlc = (REPO_ROOT / "docs" / "process" / "agent-sdlc.md").read_text(
            encoding="utf-8")
        for state in chk.SDLC_STATES:
            with self.subTest(state=state):
                self.assertIn(state, sdlc)


class DeclarationScanTests(unittest.TestCase):
    """Finding `State:` lines in a ledger directory."""

    def test_finds_declarations_through_markdown_emphasis(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "phase-1.md").write_text(
                "# heading\n"
                "State: COMPLETE\n"
                "some prose\n"
                "**State:** LOCALLY VERIFIED\n"
                "State:   **SCAFFOLDED** — with a note\n",
                encoding="utf-8",
            )
            found = chk.declared_states(str(d))
            self.assertEqual([n for _, n, _ in found], [2, 4, 5])
            self.assertEqual(
                [chk.state_token(raw) for _, _, raw in found],
                ["COMPLETE", "LOCALLY VERIFIED", "SCAFFOLDED"],
            )

    def test_prose_mentioning_a_state_is_not_a_declaration(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "phase-2.md").write_text(
                "The entry was COMPLETE before this package.\n"
                "See State of the art, which is not a declaration.\n",
                encoding="utf-8",
            )
            self.assertEqual(chk.declared_states(str(d)), [])


class ExitCodeTests(unittest.TestCase):
    """Warn-only by default — practice §51, because a backlog exists."""

    def _run(self, contents, strict):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "phase-1.md").write_text(contents, encoding="utf-8")
            return chk.check_vocabulary(str(d), strict)

    def test_clean_ledger_passes_either_way(self):
        self.assertEqual(self._run("State: COMPLETE\n", False), 0)
        self.assertEqual(self._run("State: COMPLETE\n", True), 0)

    def test_offender_warns_by_default_and_fails_under_strict(self):
        self.assertEqual(self._run("State: VERIFIED\n", False), 0)
        self.assertEqual(self._run("State: VERIFIED\n", True), 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
