"""Tests for scripts/blast-radius.sh's parent-module filter widening.

WHY THIS FILE EXISTS
---------------------
2026-09-02's trailing sweep: `gateway/session.rs` changed, and both the full
trace and the targeted gate traced `--lib gateway::session` -- but this
crate's source-scanning tests live in `gateway/mod.rs`'s own `mod tests`,
which runs only under the filter `gateway`. That module went red only in the
full sweep, after the gate had already called the change green.

A change nested under a module directory must also carry the parent module
as a `--lib` filter, alongside the child's own path, in both the full-trace
and the `--targeted` mapping.
"""
from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "scripts" / "blast-radius.sh"


class ParentModuleFilterTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        (self.tmp / "scripts").mkdir()
        (self.tmp / "scripts" / "blast-radius.sh").write_bytes(SCRIPT.read_bytes())
        os.chmod(self.tmp / "scripts" / "blast-radius.sh", 0o755)
        self.git("init", "-q")
        self.git("config", "user.email", "t@example.com")
        self.git("config", "user.name", "t")

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.tmp,
                              capture_output=True, text=True)

    def commit_rs(self, name: str, body: str) -> None:
        p = self.tmp / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(body)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", f"add {name}")

    def dry_run_since(self, since: str) -> str:
        # --list (not just --dry-run) is needed to print the "--targeted
        # preview" section, which is where TARGETED_FILTERS is visible.
        r = subprocess.run(
            ["bash", str(self.tmp / "scripts" / "blast-radius.sh"),
             "--dry-run", "--list", "--since", since],
            cwd=self.tmp, capture_output=True, text=True,
        )
        return r.stdout + r.stderr

    @staticmethod
    def targeted_preview(out: str) -> str:
        """Isolate the "--targeted preview" section, so a check of the
        TARGETED_FILTERS-derived line can't be satisfied by the separate
        full-trace filter line instead."""
        marker = "--targeted preview"
        idx = out.index(marker)
        return out[idx:]

    def test_a_nested_module_change_also_traces_its_parent(self):
        self.commit_rs("crates/glasshouse/src/gateway/mod.rs",
                       "pub mod session;\n")
        self.commit_rs("crates/glasshouse/src/gateway/session.rs",
                       "pub fn a() {}\n")
        out = self.dry_run_since("HEAD~1")
        self.assertIn("gateway::session", out)
        targeted = self.targeted_preview(out)
        self.assertIn("gateway::session", targeted)
        # the bare parent filter must appear as its own token, not merely as
        # a substring of "gateway::session"
        self.assertRegex(targeted, r"(?<![:\w])gateway(?![:\w])")

    def test_a_top_level_file_gets_no_stray_parent(self):
        self.commit_rs("crates/glasshouse/src/base.rs", "pub fn a() {}\n")
        self.commit_rs("crates/glasshouse/src/foo.rs", "pub fn foo() {}\n")
        out = self.dry_run_since("HEAD~1")
        self.assertIn("foo", out)
        # no stray parent filter for a top-level file
        self.assertNotRegex(out, r"\bfoo::")

    def test_changing_mod_rs_itself_traces_the_parent_once(self):
        self.commit_rs("crates/glasshouse/src/base.rs", "pub fn a() {}\n")
        self.commit_rs("crates/glasshouse/src/gateway/mod.rs",
                       "pub mod session;\n")
        out = self.dry_run_since("HEAD~1")
        self.assertIn("gateway", out)
        self.assertNotIn("gateway::", out)
        # each filter list (full-trace --lib line, "targets by lane", and the
        # "--targeted preview") is independently de-duped, so "gateway" (with
        # no "::" suffix) appears at most once PER LINE.
        for line in out.splitlines():
            self.assertLessEqual(
                line.count("gateway"), 1,
                f"a filter list line repeated the bare 'gateway' filter: {line!r}")


if __name__ == "__main__":
    unittest.main()
