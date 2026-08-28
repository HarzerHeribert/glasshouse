"""Tests for scripts/validate_round.py and scripts/discover.py.

Run with:
    python3 -m unittest discover -s scripts/tests -v

Most tests build small fixture files under a TemporaryDirectory so they do
not depend on any other checkout being present. A handful of integration
tests additionally exercise the three real acceptance cases named in
`.agent-runtime/packet-round-tools.md` against the sibling `glasshouse`
checkout, and skip themselves if that checkout is not on this machine.
"""
from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SIBLING_GLASSHOUSE = Path("/Users/eneas/projects/glasshouse")


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


vr = _load("validate_round", REPO_ROOT / "scripts" / "validate_round.py")
disc = _load("discover", REPO_ROOT / "scripts" / "discover.py")


def write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    return path


class ParsingTests(unittest.TestCase):
    def test_parse_pattern_strips_new_marker(self):
        pattern, is_new, exc = vr.parse_pattern("scripts/foo.py          (new)")
        self.assertEqual(pattern, "scripts/foo.py")
        self.assertTrue(is_new)
        self.assertIsNone(exc)

    def test_parse_pattern_extracts_except_clause(self):
        pattern, is_new, exc = vr.parse_pattern(
            "crates/glasshouse/src/session/** except session/lifecycle.rs"
        )
        self.assertEqual(pattern, "crates/glasshouse/src/session/**")
        self.assertFalse(is_new)
        self.assertEqual(exc, "session/lifecycle.rs")

    def test_parse_pattern_expands_root_glob_prose(self):
        pattern, _is_new, _exc = vr.parse_pattern("every *.md at the repository root")
        self.assertEqual(pattern, "*.md")

    def test_parse_indented_block_stops_at_dedent(self):
        lines = [
            "**YOURS**",
            "",
            "    a.rs",
            "    b.rs",
            "",
            "**FORBIDDEN**",
        ]
        items = vr.parse_indented_block(lines, 0)
        self.assertEqual([text for _ln, text in items], ["a.rs", "b.rs"])

    def test_parse_indented_block_tolerates_blank_line_inside_block(self):
        lines = [
            "**YOURS**",
            "",
            "    a.rs",
            "",
            "    b.rs",
            "not indented",
        ]
        items = vr.parse_indented_block(lines, 0)
        self.assertEqual([text for _ln, text in items], ["a.rs", "b.rs"])

    def test_parse_box_lines_joins_wrapped_continuation(self):
        lines = [
            "    ☐ Prefer free models for bounded work such as",
            "      classification and reranking.",
            "    ☑ A second, unwrapped box.",
        ]
        boxes = vr.parse_box_lines(lines)
        self.assertEqual(len(boxes), 2)
        self.assertEqual(
            boxes[0][2],
            "Prefer free models for bounded work such as classification and reranking.",
        )
        self.assertEqual(boxes[1][2], "A second, unwrapped box.")

    def test_parse_box_lines_sees_a_box_dressed_as_a_numbered_list_item(self):
        """The shape real packets actually use, which the parser once missed.

        Packets quote a box as a list item labelled with its map line number.
        Matching only the first character saw NOTHING in those packets, so the
        verbatim check silently verified zero lines and still reported PASSED --
        practice §68's defect inside the gate built to catch it. On 2026-08-28 a
        worker found five of its packet's nine quoted lines wrong at the same
        line numbers, two with their topics swapped, after this validator passed
        the round.
        """
        lines = [
            "- **1311** ☐ Track whether each configured resource is currently available.",
            "* 1315 ☐ Track known quota reset time when it is exposed.",
            "1. ☑ A plain numbered box.",
            "    ☐ And the bare form the map itself uses.",
        ]
        boxes = vr.parse_box_lines(lines)
        self.assertEqual(len(boxes), 4, "a dressed-up box quote must still be seen")
        self.assertEqual(
            boxes[0][2], "Track whether each configured resource is currently available."
        )
        self.assertEqual(boxes[1][2], "Track known quota reset time when it is exposed.")
        self.assertEqual(boxes[2][1], "☑")
        self.assertEqual(boxes[3][2], "And the bare form the map itself uses.")

    def test_parse_box_lines_does_not_invent_a_box_from_ordinary_prose(self):
        """The prefix tolerance must not turn any list item into a box quote."""
        lines = [
            "- **1311** Track whether each configured resource is currently available.",
            "- a plain bullet with no marker at all",
            "1. a numbered item that quotes nothing",
        ]
        self.assertEqual(vr.parse_box_lines(lines), [])


class PatternOverlapTests(unittest.TestCase):
    def test_exact_paths_collide_only_when_equal(self):
        self.assertTrue(vr.patterns_overlap("a/b.rs", "a/b.rs"))
        self.assertFalse(vr.patterns_overlap("a/b.rs", "a/c.rs"))

    def test_dirglob_covers_anything_beneath_it(self):
        self.assertTrue(vr.patterns_overlap("crates/glasshouse/src/memory/**", "crates/glasshouse/src/memory/mod.rs"))
        self.assertFalse(vr.patterns_overlap("crates/glasshouse/src/memory/**", "crates/glasshouse/src/profile/mod.rs"))

    def test_nested_dirglobs_overlap(self):
        self.assertTrue(vr.patterns_overlap("crates/glasshouse/src/**", "crates/glasshouse/src/memory/**"))

    def test_root_glob_matches_only_root_files(self):
        self.assertTrue(vr.patterns_overlap("*.md", "README.md"))
        self.assertFalse(vr.patterns_overlap("*.md", "docs/README.md"))

    def test_matches_exception_by_suffix(self):
        self.assertTrue(vr.matches_exception("crates/glasshouse/src/session/lifecycle.rs", "session/lifecycle.rs"))
        self.assertFalse(vr.matches_exception("crates/glasshouse/src/session/store.rs", "session/lifecycle.rs"))


class PacketFixture:
    """A minimal packet file built from YOURS/FORBIDDEN path lists."""

    def __init__(self, tmp: Path, name: str, yours: list[str], forbidden: list[str] | None = None, boxes: list[str] | None = None):
        # These fixtures exercise the partition and box-quoting checks, not
        # Phase -1, so they take the exemption the feasibility check offers —
        # which also keeps that escape hatch exercised on every run.
        lines = [
            "# PACKET",
            "",
            "FEASIBILITY: not applicable — fixture for the partition checks",
            "",
            "## FILE PARTITION",
            "",
            "**YOURS**",
            "",
        ]
        lines += [f"    {p}" for p in yours]
        if forbidden:
            lines += ["", "**FORBIDDEN**", ""]
            lines += [f"    {p}" for p in forbidden]
        if boxes:
            lines += ["", "Quoted from the map:", ""]
            lines += [f"    {b}" for b in boxes]
        self.path = write(tmp / name, "\n".join(lines) + "\n")


class FeasibilityCheckTests(unittest.TestCase):
    """Phase -1: a packet must show its mechanism can connect end to end.

    The rule exists because two packets on 2026-08-28 were dispatched on
    structurally impossible premises and cost roughly $30 of worker compute that
    no downstream assurance optimization could recover. See
    `docs/process/assurance-economics.md`.
    """

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())

    def _findings(self, body: str):
        path = write(self.tmp / "p.md", body)
        findings = []
        vr.check_feasibility_block([vr.Packet(str(path))], findings)
        return findings

    def test_a_packet_with_no_feasibility_block_is_refused(self):
        self.assertEqual(len(self._findings("# PACKET\n\n**YOURS**\n\n    a.rs\n")), 1)

    def test_all_four_links_named_passes(self):
        body = (
            "# PACKET\n\n## FEASIBILITY\n\n"
            "Input producer: src/a.rs::Thing\n"
            "Caller field: Thing.harness\n"
            "Propagation path: Thing.harness -> Req.harness\n"
            "Consumer: src/b.rs::score\n"
        )
        self.assertEqual(self._findings(body), [])

    def test_a_missing_link_is_named_in_the_finding(self):
        body = (
            "# PACKET\n\n## FEASIBILITY\n\n"
            "Input producer: src/a.rs::Thing\n"
            "Caller field: Thing.harness\n"
            "Consumer: src/b.rs::score\n"
        )
        findings = self._findings(body)
        self.assertEqual(len(findings), 1)
        self.assertIn("propagation", findings[0].message)

    def test_the_heading_spelling_this_project_actually_uses_is_accepted(self):
        """The first version of this check refused the packet it was written for.

        Packets head the block `## FEASIBILITY — Phase -1, verified before
        dispatch`; a matcher accepting only `FEASIBILITY:` and `**FEASIBILITY**`
        rejected it. Pinned so the matcher cannot narrow again.
        """
        body = (
            "# PACKET\n\n## FEASIBILITY — Phase -1, verified before dispatch\n\n"
            "Input producer: x\nCaller field: y\nPropagation path: z\nConsumer: w\n"
        )
        self.assertEqual(self._findings(body), [])

    def test_an_exemption_must_state_a_reason(self):
        bare = "# PACKET\n\nFEASIBILITY: not applicable\n"
        with_reason = "# PACKET\n\nFEASIBILITY: not applicable — an inventory package\n"
        self.assertEqual(len(self._findings(bare)), 1)
        self.assertEqual(self._findings(with_reason), [])


class ValidateRoundChecksTests(unittest.TestCase):
    def setUp(self):
        # YOURS paths are resolved against the current working directory,
        # the same way `scripts/validate_round.py` is meant to be invoked
        # from a repo root — so these fixtures need cwd inside their tmpdir.
        self.tmp = Path(tempfile.mkdtemp())
        self._old_cwd = Path.cwd()
        import os
        os.chdir(self.tmp)
        write(self.tmp / "owned.rs", "fn owned() {}\n")
        write(self.tmp / "also_owned.rs", "fn also_owned() {}\n")
        self.map_path = write(
            self.tmp / "MAP.md",
            "☐ Do the thing exactly as described.\n☑ Already done.\n",
        )

    def tearDown(self):
        import os
        os.chdir(self._old_cwd)

    def test_shared_yours_path_is_a_failure(self):
        a = PacketFixture(self.tmp, "a.md", yours=["owned.rs"])
        b = PacketFixture(self.tmp, "b.md", yours=["owned.rs"])
        findings = vr.validate([str(a.path), str(b.path)], str(self.map_path))
        self.assertTrue(any(f.check == "partitions-disjoint" for f in findings))

    def test_yours_vs_others_forbidden_is_not_a_failure(self):
        """The correct, expected shape: A owns a file, B forbids the same
        file. This must NOT be flagged — see the real wire-disposable /
        migration-7 packets, which do this correctly for most of their files."""
        a = PacketFixture(self.tmp, "a.md", yours=["owned.rs"])
        b = PacketFixture(self.tmp, "b.md", yours=["also_owned.rs"], forbidden=["owned.rs"])
        findings = vr.validate([str(a.path), str(b.path)], str(self.map_path))
        self.assertEqual([], findings)

    def test_missing_unmarked_path_is_a_failure(self):
        a = PacketFixture(self.tmp, "a.md", yours=["does_not_exist.rs"])
        findings = vr.validate([str(a.path)], str(self.map_path))
        self.assertTrue(any(f.check == "yours-paths-exist" for f in findings))

    def test_new_marked_path_is_not_required_to_exist(self):
        a = PacketFixture(self.tmp, "a.md", yours=["does_not_exist.rs          (new)"])
        findings = vr.validate([str(a.path)], str(self.map_path))
        self.assertFalse(any(f.check == "yours-paths-exist" for f in findings))

    def test_self_contradiction_is_a_failure(self):
        a = PacketFixture(self.tmp, "a.md", yours=["owned.rs"], forbidden=["owned.rs"])
        findings = vr.validate([str(a.path)], str(self.map_path))
        self.assertTrue(any(f.check == "no-self-contradiction" for f in findings))

    def test_self_contradiction_respects_except_clause(self):
        a = PacketFixture(
            self.tmp,
            "a.md",
            yours=["owned.rs"],
            forbidden=["*.rs except owned.rs"],
        )
        findings = vr.validate([str(a.path)], str(self.map_path))
        self.assertFalse(any(f.check == "no-self-contradiction" for f in findings))

    def test_empty_yours_is_a_failure(self):
        path = write(self.tmp / "empty.md", "# PACKET\n\n## FILE PARTITION\n\n**FORBIDDEN**\n\n    owned.rs\n")
        findings = vr.validate([str(path)], str(self.map_path))
        self.assertTrue(any(f.check == "yours-non-empty" for f in findings))

    def test_box_line_matching_the_map_verbatim_passes(self):
        a = PacketFixture(self.tmp, "a.md", yours=["owned.rs"], boxes=["☐ Do the thing exactly as described."])
        findings = vr.validate([str(a.path)], str(self.map_path))
        self.assertFalse(any(f.check == "box-lines-match-map" for f in findings))

    def test_box_line_drifted_by_one_character_fails(self):
        a = PacketFixture(self.tmp, "a.md", yours=["owned.rs"], boxes=["☐ Do the thing exactly as describde."])
        findings = vr.validate([str(a.path)], str(self.map_path))
        drift = [f for f in findings if f.check == "box-lines-match-map"]
        self.assertEqual(len(drift), 1)
        self.assertIn("does not match", drift[0].message)

    def test_box_line_apostrophe_drift_fails(self):
        map_path = write(self.tmp / "MAP2.md", "☐ Preserve the harness’s required protocol.\n")
        a = PacketFixture(self.tmp, "a.md", yours=["owned.rs"], boxes=["☐ Preserve the harness's required protocol."])
        findings = vr.validate([str(a.path)], str(map_path))
        self.assertTrue(any(f.check == "box-lines-match-map" for f in findings))

    def test_clean_round_passes(self):
        a = PacketFixture(self.tmp, "a.md", yours=["owned.rs"], boxes=["☐ Do the thing exactly as described."])
        b = PacketFixture(self.tmp, "b.md", yours=["also_owned.rs"], forbidden=["owned.rs"])
        findings = vr.validate([str(a.path), str(b.path)], str(self.map_path))
        self.assertEqual([], findings)


class ValidateRoundCliTests(unittest.TestCase):
    def test_exit_code_and_stderr_on_failure(self):
        tmp = Path(tempfile.mkdtemp())
        write(tmp / "owned.rs", "fn owned() {}\n")
        map_path = write(tmp / "MAP.md", "")
        a = PacketFixture(tmp, "a.md", yours=["owned.rs"])
        b = PacketFixture(tmp, "b.md", yours=["owned.rs"])
        result = subprocess.run(
            [sys.executable, str(REPO_ROOT / "scripts" / "validate_round.py"),
             "--map", str(map_path), str(a.path), str(b.path)],
            capture_output=True, text=True, cwd=str(tmp),
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("owned.rs", result.stderr)

    def test_exit_code_on_success(self):
        tmp = Path(tempfile.mkdtemp())
        write(tmp / "owned.rs", "fn owned() {}\n")
        map_path = write(tmp / "MAP.md", "")
        a = PacketFixture(tmp, "a.md", yours=["owned.rs"])
        result = subprocess.run(
            [sys.executable, str(REPO_ROOT / "scripts" / "validate_round.py"),
             "--map", str(map_path), str(a.path)],
            capture_output=True, text=True, cwd=str(tmp),
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("PASSED", result.stdout)


class DiscoverSeamTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())

    # --- a definition is not a call site (added 2026-08-28) ----------------
    #
    # `discover.py --seam evaluate_reserve_spend` reported "3 non-test call
    # sites ... a box depending on this seam can close" when all three lines
    # were inside its own module: two `///` intra-doc links and the `pub fn`
    # itself. The function was reachable from tests only, and the verdict
    # nearly ticked seven capability-map boxes with no production caller.

    def test_a_functions_own_definition_is_not_counted_as_a_caller(self):
        write(self.tmp / "src" / "lib.rs", "pub fn seam(x: u8) -> u8 { x }\n")
        hits = disc.find_call_sites("seam", str(self.tmp))
        self.assertEqual(hits["literal"], [], "a definition is not a call site")
        self.assertEqual(len(hits["definition"]), 1)

    def test_doc_comments_naming_the_seam_are_not_counted_as_callers(self):
        write(
            self.tmp / "src" / "lib.rs",
            "/// See [`seam`] for the rule this applies.\n"
            "//! Module doc mentioning seam as well.\n"
            "// An ordinary comment about seam.\n"
            "pub fn other() {}\n",
        )
        hits = disc.find_call_sites("seam", str(self.tmp))
        self.assertEqual(hits["literal"], [])
        self.assertEqual(len(hits["definition"]), 3)

    def test_a_real_caller_beside_the_definition_still_counts(self):
        """The whole point: definition excluded, caller kept."""
        write(
            self.tmp / "src" / "lib.rs",
            "/// Doc naming seam.\n"
            "pub fn seam() {}\n"
            "pub fn caller() { seam(); }\n",
        )
        hits = disc.find_call_sites("seam", str(self.tmp))
        self.assertEqual(len(hits["literal"]), 1)
        self.assertIn("caller()", hits["literal"][0][2])

    def test_a_one_line_function_body_containing_a_call_is_still_a_call(self):
        """The narrowing that the first version of this fix got wrong.

        `fn real() { Thing::seam(); }` both declares `real` and calls the
        seam. A predicate testing "does this line start with `fn`" swallowed
        it and lost a genuine call site -- so the rule is *declares the
        seam*, not *declares anything*.
        """
        write(self.tmp / "src" / "lib.rs", "fn real() { Thing::seam(); }\n")
        hits = disc.find_call_sites("Thing::seam", str(self.tmp))
        self.assertEqual(len(hits["literal"]), 1)
        self.assertEqual(hits["definition"], [])

    def test_an_impl_block_for_the_seam_type_is_a_definition(self):
        write(
            self.tmp / "src" / "lib.rs",
            "pub struct Widget;\n"
            "impl Widget {\n"
            "    pub fn go(&self) {}\n"
            "}\n"
            "fn use_it(w: &Widget) { w.go(); }\n",
        )
        hits = disc.find_call_sites("Widget", str(self.tmp))
        defined = {text for _rel, _line, text in hits["definition"]}
        self.assertTrue(any(d.startswith("pub struct Widget") for d in defined))
        self.assertTrue(any(d.startswith("impl Widget") for d in defined))
        self.assertTrue(any("use_it" in text for _r, _l, text in hits["literal"]))

    def test_excludes_tests_directory(self):
        write(self.tmp / "src" / "lib.rs", "fn f() { Thing::seam(); }\n")
        write(self.tmp / "tests" / "it.rs", "fn t() { Thing::seam(); }\n")
        hits = disc.find_call_sites("Thing::seam", str(self.tmp))
        files = {rel for rel, _line, _text in hits["literal"]}
        self.assertTrue(any("src/lib.rs" in f for f in files))
        self.assertFalse(any("tests/it.rs" in f for f in files))

    def test_excludes_cfg_test_module(self):
        write(
            self.tmp / "src" / "lib.rs",
            "fn real() { Thing::seam(); }\n\n"
            "#[cfg(test)]\nmod tests {\n"
            "    fn also_calls() { Thing::seam(); }\n"
            "}\n",
        )
        hits = disc.find_call_sites("Thing::seam", str(self.tmp))
        self.assertEqual(len(hits["literal"]), 1)
        self.assertIn("real()", hits["literal"][0][2])

    def test_excludes_attribute_test_function(self):
        write(
            self.tmp / "src" / "lib.rs",
            "fn real() { Thing::seam(); }\n\n"
            "#[test]\nfn a_test() {\n"
            "    Thing::seam();\n"
            "}\n",
        )
        hits = disc.find_call_sites("Thing::seam", str(self.tmp))
        self.assertEqual(len(hits["literal"]), 1)

    def test_zero_call_sites_reported_as_headline(self):
        write(self.tmp / "src" / "lib.rs", "fn unrelated() {}\n")
        hits = disc.find_call_sites("Nothing::calls_this", str(self.tmp))
        self.assertEqual(hits["literal"], [])
        self.assertEqual(hits["method"], [])

    def test_method_call_fallback_when_no_literal_match(self):
        write(self.tmp / "src" / "lib.rs", "fn f(x: Box<dyn Thing>) { x.seam(); }\n")
        hits = disc.find_call_sites("Thing::seam", str(self.tmp))
        self.assertEqual(hits["literal"], [])
        self.assertEqual(len(hits["method"]), 1)


class DiscoverPhaseTests(unittest.TestCase):
    def setUp(self):
        self.map_lines = (
            "Phase 9A — Something else\n"
            "\n"
            "☑ An unrelated closed box.\n"
            "\n"
            "Phase 9I — Free-pool routing\n"
            "\n"
            "☐ An open box in scope.\n"
            "☑ A closed box in scope.\n"
            "\n"
            "Phase 9J — Next phase\n"
            "\n"
            "☐ A box that must not be included.\n"
        ).splitlines()

    def test_phase_span_stops_before_next_phase(self):
        start, end, heading = disc.phase_span(self.map_lines, "9I")
        self.assertEqual(heading, "Phase 9I — Free-pool routing")
        boxes = disc.parse_box_lines(self.map_lines[start:end])
        texts = [t for _ln, _m, t in boxes]
        self.assertIn("An open box in scope.", texts)
        self.assertNotIn("A box that must not be included.", texts)

    def test_phase_span_missing_phase_returns_none(self):
        self.assertIsNone(disc.phase_span(self.map_lines, "9Z"))

    def test_phase_evidence_paths_scoped_to_heading(self):
        evidence_lines = (
            "### Phase 9I — something\n"
            "\n"
            "- `crates/glasshouse/src/routing/free.rs` is the seam.\n"
            "\n"
            "### Phase 9J — something else\n"
            "\n"
            "- `crates/glasshouse/src/other.rs` must not appear.\n"
        ).splitlines()
        paths = disc.phase_evidence_paths(evidence_lines, "9I")
        self.assertEqual(paths, ["crates/glasshouse/src/routing/free.rs"])


def _packets_present(*names: str) -> bool:
    """The packet fixtures live in `.agent-runtime/`, which is gitignored.

    These three cases were stale from the day the documents moved — the map
    they named became `docs/product/capability-map.md` and nothing re-ran them,
    because nothing ran these tests at all until they were wired into
    `ci-local.sh`. Guarding on the fixtures rather than only on the checkout
    keeps them honest on a machine where the runtime directory is empty:
    skipped says "not checked here", where a hard failure would say
    "validate_round.py is broken" about a tool that is fine.
    """
    return all((SIBLING_GLASSHOUSE / ".agent-runtime" / n).is_file() for n in names)


@unittest.skipUnless(SIBLING_GLASSHOUSE.is_dir(), "sibling glasshouse checkout not present on this machine")
class RealAcceptanceTests(unittest.TestCase):
    """The three acceptance cases named in packet-round-tools.md, run for
    real against the sibling glasshouse checkout when it is available."""

    @unittest.skipUnless(
        _packets_present("packet-wire-disposable.md", "packet-migration-7.md"),
        "packet fixtures are not on this machine (.agent-runtime is gitignored)")
    def test_real_colliding_packets_fail_and_name_state_rs(self):
        result = subprocess.run(
            [sys.executable, str(REPO_ROOT / "scripts" / "validate_round.py"),
             "--map", str(SIBLING_GLASSHOUSE / "docs" / "product" / "capability-map.md"),
             ".agent-runtime/packet-wire-disposable.md",
             ".agent-runtime/packet-migration-7.md"],
            capture_output=True, text=True, cwd=str(SIBLING_GLASSHOUSE),
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("crates/glasshouse/src/shell/state.rs", result.stderr)

    @unittest.skipUnless(
        _packets_present("packet-round-tools.md"),
        "packet fixture is not on this machine (.agent-runtime is gitignored)")
    def test_a_real_dispatched_packet_passes(self):
        """A packet this project actually dispatches must clear every check.

        Pointed at the current exemplar rather than `packet-round-tools.md`,
        which predates Phase -1 and carries no FEASIBILITY block. That older
        packet failing is the rule working, not a regression — but an
        acceptance test asserting "a real packet passes" has to name one that
        is real *today*.
        """
        packet = SIBLING_GLASSHOUSE / ".agent-runtime" / "packet-gateway-evidence.md"
        if not packet.exists():
            self.skipTest("the current exemplar packet is not on this machine")
        result = subprocess.run(
            [sys.executable, str(REPO_ROOT / "scripts" / "validate_round.py"),
             "--map", str(SIBLING_GLASSHOUSE / "docs" / "product" / "capability-map.md"),
             ".agent-runtime/packet-gateway-evidence.md"],
            capture_output=True, text=True, cwd=str(SIBLING_GLASSHOUSE),
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_real_extraction_model_seam_reported(self):
        result = subprocess.run(
            [sys.executable, str(REPO_ROOT / "scripts" / "discover.py"),
             "--seam", "ExtractionModel::complete",
             "--src-root", str(REPO_ROOT / "crates")],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("complete", result.stdout)



class CitedSeams(unittest.TestCase):
    """The cited-seam check, added after batch 45 made the same mistake twice.

    A packet's FEASIBILITY block names the symbols its premise rests on. Twice in
    one round one of those had ZERO non-test call sites — a function called from
    nowhere but tests, cited as proof that behaviour reaches production. Practice
    §76 records that writing the rule down did not stop it recurring ninety
    minutes later, which is why it is mechanical.

    `test_a_symbol_that_is_the_gap_is_still_flagged` is the load-bearing one and
    the reason this is REVIEW rather than a refusal: a package whose whole job is
    to give a symbol its first caller legitimately cites a zero-call-site symbol,
    and a gate that refused it would be red on a correct packet.
    """

    def symbols(self, text):
        return vr.feasibility_symbols(text)

    def test_snake_case_call_is_extracted(self):
        self.assertEqual(self.symbols("cites `Foo::bar_baz()` here"), ["Foo::bar_baz"])

    def test_a_bare_function_is_extracted(self):
        self.assertEqual(self.symbols("`observe_exchange()`"), ["observe_exchange"])

    def test_a_type_is_not_extracted(self):
        """A struct has no call sites; flagging one would be pure noise."""
        self.assertEqual(self.symbols("`ResourceHealth()` and `Foo::Bar()`"), [])

    def test_prose_without_parens_is_not_extracted(self):
        """Only an explicit call form counts, so ordinary mentions stay quiet."""
        self.assertEqual(self.symbols("`free_pool` is mentioned"), [])

    def test_duplicates_are_reported_once(self):
        self.assertEqual(self.symbols("`a_b()` and again `a_b()`"), ["a_b"])

    def test_a_symbol_that_is_the_gap_is_still_flagged(self):
        """Both readings are zero-call-site; only a reader can tell them apart.

        `health-cache` cites `free_pool()` precisely because nothing calls it —
        adding the caller is the package. The check must still surface it, and
        must not refuse it.
        """
        self.assertEqual(self.symbols("propagation: `free_pool()` is the gap"),
                         ["free_pool"])

if __name__ == "__main__":
    unittest.main()
