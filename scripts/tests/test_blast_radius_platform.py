"""Tests for scripts/blast-radius.sh's platform-conditional warning.

Run with:
    python3 -m unittest discover -s scripts/tests -v

WHY THIS FILE EXISTS
--------------------
On 2026-08-30 the full gate returned 13 PASS / 3 FAIL on a tree `blast-radius.sh`
had called green. All three failures were in platform-conditional code: a
Windows build error (`-D warnings` on a constant that is `None` there) and a
Linux test failure (a hazard real on macOS/BSD and absent on Linux, so a ceiling
taken from the documented constant rather than measured protected nothing).

`blast-radius.sh` runs on one platform. That is what it is for — but it means a
green result is never evidence about the other two, and nineteen commits piled
up before anyone ran the gate that could see them.

The warning must fire when it matters **and stay quiet otherwise**: a banner on
every run is a banner nobody reads, which is how the pipeline nag failed
earlier the same day.
"""
from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "scripts" / "blast-radius.sh"
BANNER = "PLATFORM-CONDITIONAL CODE CHANGED"


class PlatformWarningTests(unittest.TestCase):
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
        r = subprocess.run(
            ["bash", str(self.tmp / "scripts" / "blast-radius.sh"),
             "--dry-run", "--since", since],
            cwd=self.tmp, capture_output=True, text=True,
        )
        return r.stdout + r.stderr

    def test_it_warns_when_a_changed_file_is_platform_conditional(self):
        self.commit_rs("src/base.rs", "pub fn a() {}\n")
        self.commit_rs("src/plat.rs",
                       "#[cfg(unix)]\npub fn only_unix() {}\n")
        out = self.dry_run_since("HEAD~1")
        self.assertIn(BANNER, out)
        self.assertIn("plat.rs", out)
        self.assertIn("ci-local.sh", out,
                      "the warning must name the gate, not just complain")

    def test_it_stays_quiet_when_nothing_platform_conditional_changed(self):
        """A banner on every run is a banner nobody reads."""
        self.commit_rs("src/base.rs", "pub fn a() {}\n")
        self.commit_rs("src/plain.rs", "pub fn b() -> u8 { 1 }\n")
        self.assertNotIn(BANNER, self.dry_run_since("HEAD~1"))

    def test_cfg_windows_and_target_os_also_trigger_it(self):
        self.commit_rs("src/base.rs", "pub fn a() {}\n")
        self.commit_rs("src/w.rs", '#[cfg(target_os = "windows")]\npub fn w() {}\n')
        self.assertIn(BANNER, self.dry_run_since("HEAD~1"))

    def test_the_cfg_macro_form_triggers_it_too(self):
        """`if cfg!(windows)` is platform-conditional without an attribute."""
        self.commit_rs("src/base.rs", "pub fn a() {}\n")
        self.commit_rs("src/m.rs",
                       "pub fn m() -> bool { cfg!(windows) }\n")
        self.assertIn(BANNER, self.dry_run_since("HEAD~1"))

    def test_a_mere_mention_in_prose_does_not_trigger_it(self):
        """A doc comment saying the word must not raise the banner."""
        self.commit_rs("src/base.rs", "pub fn a() {}\n")
        self.commit_rs("src/doc.rs",
                       "/// Behaves the same on unix and windows.\npub fn d() {}\n")
        self.assertNotIn(BANNER, self.dry_run_since("HEAD~1"))


if __name__ == "__main__":
    unittest.main()
