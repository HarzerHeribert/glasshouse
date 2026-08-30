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


class WorktreeTargetingTests(unittest.TestCase):
    """The script must analyse the CALLER's tree, not its own location.

    WHY THIS CLASS EXISTS
    ----------------------
    `REPO` was derived from the script's own location and the script then
    `cd`'d there unconditionally. Every editing worker runs from a worktree
    under `.worktrees/`, and the script is reachable by absolute path from
    the main checkout — so a worker running the main checkout's copy from
    inside its own worktree made the script diff the MAIN CHECKOUT, print
    "no changed .rs files -- nothing to do", and exit 0. A verification tool
    reported success while looking at the wrong tree.

    These tests set up a "main checkout" (a fresh git repo holding a copy of
    the script) and a real `git worktree` of it, then invoke the main
    checkout's copy of the script from inside the worktree -- exactly the
    scenario that produced the defect.
    """

    NO_CHANGES = "no changed .rs files"

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.main = self.tmp / "main"
        (self.main / "scripts").mkdir(parents=True)
        (self.main / "scripts" / "blast-radius.sh").write_bytes(SCRIPT.read_bytes())
        os.chmod(self.main / "scripts" / "blast-radius.sh", 0o755)
        self.git(self.main, "init", "-q")
        self.git(self.main, "config", "user.email", "t@example.com")
        self.git(self.main, "config", "user.name", "t")
        (self.main / "README.md").write_text("x\n")
        self.git(self.main, "add", "-A")
        self.git(self.main, "commit", "-q", "-m", "init")

    def git(self, cwd: Path, *args):
        return subprocess.run(["git", *args], cwd=cwd,
                              capture_output=True, text=True)

    def make_worktree(self, name: str = "wt") -> Path:
        wt = self.tmp / name
        r = self.git(self.main, "worktree", "add", "-q", "-b", f"{name}-branch", str(wt))
        assert r.returncode == 0, r.stderr
        return wt

    def run_main_script(self, cwd: Path):
        return subprocess.run(
            ["bash", str(self.main / "scripts" / "blast-radius.sh"), "--dry-run"],
            cwd=cwd, capture_output=True, text=True,
        )

    def test_the_defect_reproduced_then_fixed(self):
        """Running the main checkout's copy from inside a worktree with a
        changed .rs file must find that file, not report nothing changed.

        This test fails against the pre-fix script: it prints
        "no changed .rs files -- nothing to do" and exits 0, because the
        unconditional `cd "$REPO"` made it diff the main checkout instead of
        the worktree that actually has the change.
        """
        wt = self.make_worktree()
        (wt / "crates" / "glasshouse" / "src").mkdir(parents=True)
        (wt / "crates" / "glasshouse" / "src" / "foo.rs").write_text(
            "pub fn foo() {}\n")

        r = self.run_main_script(wt)
        out = r.stdout + r.stderr
        self.assertNotIn(self.NO_CHANGES, out,
                          "the script looked at the wrong tree and found nothing")
        self.assertIn("foo.rs", out)
        self.assertEqual(r.returncode, 0)

    def test_the_announcement(self):
        """When the script switches to the caller's tree it must say so,
        naming the directory it is actually analysing, before doing anything
        else."""
        wt = self.make_worktree()
        (wt / "crates" / "glasshouse" / "src").mkdir(parents=True)
        (wt / "crates" / "glasshouse" / "src" / "foo.rs").write_text(
            "pub fn foo() {}\n")

        r = self.run_main_script(wt)
        out = r.stdout + r.stderr
        self.assertIn(str(wt.resolve()), out,
                       "must name the directory it is actually analysing")

    def test_the_refusal_different_repository(self):
        """Invoked from a directory that is a git repo but NOT a worktree of
        this one, the script must refuse (non-zero exit) and name both
        paths, not guess."""
        other = self.tmp / "other"
        other.mkdir()
        self.git(other, "init", "-q")

        r = self.run_main_script(other)
        out = r.stdout + r.stderr
        self.assertNotEqual(r.returncode, 0)
        self.assertIn(str(other.resolve()), out)
        # $REPO is built via an explicit `cd` off BASH_SOURCE (a logical,
        # non-symlink-resolved path), unlike $ORIG_CWD/$CALLER_TOPLEVEL which
        # come from the shell's own getcwd()-based startup pwd -- so it prints
        # unresolved, matching self.main as constructed rather than .resolve().
        self.assertIn(str(self.main), out)

    def test_the_refusal_not_a_git_tree(self):
        """Invoked from a directory that is not a git tree at all, the
        script must refuse (non-zero exit) and name both paths."""
        plain = self.tmp / "plain"
        plain.mkdir()

        r = self.run_main_script(plain)
        out = r.stdout + r.stderr
        self.assertNotEqual(r.returncode, 0)
        self.assertIn(str(plain.resolve()), out)
        self.assertIn(str(self.main), out)

    def test_the_unchanged_ordinary_case(self):
        """From the main checkout itself, behaviour is byte-for-byte as
        before: no announcement line, and changed files are found exactly as
        they always were."""
        (self.main / "crates" / "glasshouse" / "src").mkdir(parents=True)
        (self.main / "crates" / "glasshouse" / "src" / "bar.rs").write_text(
            "pub fn bar() {}\n")

        r = self.run_main_script(self.main)
        out = r.stdout + r.stderr
        self.assertNotIn("analysing the caller's worktree", out)
        self.assertNotIn("refusing", out)
        self.assertIn("bar.rs", out)
        self.assertEqual(r.returncode, 0)

    def test_the_ordinary_case_with_nothing_changed(self):
        """The plain no-worktree, nothing-changed path is untouched."""
        r = self.run_main_script(self.main)
        out = r.stdout + r.stderr
        self.assertNotIn("analysing the caller's worktree", out)
        self.assertIn(self.NO_CHANGES, out)
        self.assertEqual(r.returncode, 0)


if __name__ == "__main__":
    unittest.main()
