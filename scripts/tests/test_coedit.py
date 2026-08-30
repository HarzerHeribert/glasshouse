"""Tests for scripts/coedit.sh — convergent co-editing (practice §77, map Maybe L).

The protocol lets two workers share a file instead of serializing behind a lock.
Its correctness rests on three properties, and each has a test here:

- **the barrier holds** until every claimant has declared done, because
  reconciling early is exactly the post-hoc merge Maybe E already describes and
  this is supposed to be better than that;
- **a peer's version is readable across worktrees**, since that visibility is the
  entire difference from queueing;
- **nothing is written outside the caller's own tree.** `test_reading_a_peer_never_writes_to_its_worktree`
  is the load-bearing one: a tool that could mutate a peer's tree would be able to
  destroy the only copy of a worker's deliverable (workers never commit), which is
  practice §22's 161-lost-lines defect with a new mouth.
"""
from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
COEDIT = REPO / "scripts" / "coedit.sh"


def sh(*args, cwd=None, env=None):
    return subprocess.run([str(COEDIT), *args], cwd=cwd or REPO, env=env,
                          capture_output=True, text=True)


class CoEdit(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        # A throwaway git repo standing in for the project, so no test can ever
        # touch a real worker's worktree.
        self.repo = root / "repo"
        self.repo.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=self.repo, check=True)
        (self.repo / "f.txt").write_text("base\n")
        subprocess.run(["git", "add", "-A"], cwd=self.repo, check=True)
        subprocess.run(["git", "-c", "user.email=t@t", "-c", "user.name=t",
                        "commit", "-qm", "base"], cwd=self.repo, check=True)
        self.a = root / "a"
        self.b = root / "b"
        for wt in (self.a, self.b):
            subprocess.run(["git", "worktree", "add", "--detach", "-f", str(wt), "HEAD"],
                           cwd=self.repo, capture_output=True, check=True)
        self.env = dict(os.environ)

    def tearDown(self):
        self.tmp.cleanup()

    def claim_both(self):
        sh("claim", "f.txt", "wa", str(self.a), cwd=self.repo)
        sh("claim", "f.txt", "wb", str(self.b), cwd=self.repo)

    def test_the_barrier_stays_shut_until_every_claimant_declares(self):
        self.claim_both()
        self.assertEqual(sh("ready", "f.txt", cwd=self.repo).returncode, 1)
        sh("done", "f.txt", "wa", cwd=self.repo)
        self.assertEqual(sh("ready", "f.txt", cwd=self.repo).returncode, 1,
                         "barrier opened with one claimant still working")
        sh("done", "f.txt", "wb", cwd=self.repo)
        self.assertEqual(sh("ready", "f.txt", cwd=self.repo).returncode, 0)

    def test_a_peers_in_progress_change_is_visible(self):
        self.claim_both()
        (self.a / "f.txt").write_text("base\nfrom A\n")
        out = sh("diff", "f.txt", "wb", cwd=self.repo).stdout
        self.assertIn("from A", out)
        self.assertIn("UNFINISHED PROPOSAL", out,
                      "the peer's diff must carry its own not-truth warning")

    def test_a_worker_does_not_see_itself_as_a_peer(self):
        self.claim_both()
        (self.a / "f.txt").write_text("base\nfrom A\n")
        self.assertNotIn("from A", sh("diff", "f.txt", "wa", cwd=self.repo).stdout)

    def test_reading_a_peer_never_writes_to_its_worktree(self):
        """The load-bearing one. A peer's tree holds the only copy of its work."""
        self.claim_both()
        (self.a / "f.txt").write_text("base\nfrom A\n")
        before = (self.a / "f.txt").read_text()
        sh("diff", "f.txt", "wb", cwd=self.repo)
        sh("status", "f.txt", cwd=self.repo)
        sh("done", "f.txt", "wb", cwd=self.repo)
        self.assertEqual((self.a / "f.txt").read_text(), before,
                         "coedit.sh mutated a peer's worktree")

    def test_done_is_refused_for_a_worker_that_never_claimed(self):
        self.claim_both()
        self.assertNotEqual(sh("done", "f.txt", "ghost", cwd=self.repo).returncode, 0)

    def test_an_unclaimed_file_reports_no_peers_and_does_not_fail(self):
        r = sh("peers", "other.txt", cwd=self.repo)
        self.assertEqual(r.returncode, 0)
        self.assertIn("no claims", r.stdout)

    def test_state_is_shared_when_invoked_from_inside_a_worktree(self):
        """The load-bearing test, and the one the first version did not have.

        Every worker runs in its own worktree, and `git rev-parse --show-toplevel`
        answers "this worktree" from each of them. The first version anchored on
        that, so each worker wrote its claims into its OWN tree and no worker
        could ever see another's — the mechanism was silently a no-op across the
        exact boundary it exists to cross.

        A synthetic test invoking the script from the main checkout cannot catch
        that. A live two-agent trial caught it in one run. This asserts the
        property directly: a claim made from inside worktree A is visible to a
        call made from inside worktree B.
        """
        r = sh("claim", "f.txt", "wa", str(self.a), cwd=self.a)
        self.assertEqual(r.returncode, 0, r.stderr)
        out = sh("peers", "f.txt", "wb", cwd=self.b).stdout
        self.assertIn("wa", out,
                      "a claim made inside one worktree was invisible from another")
        self.assertNotIn("no claims", out)
        # and the state must live in the main checkout, not either worktree
        self.assertFalse((self.a / ".agent-runtime" / "coedit").exists(),
                         "state leaked into the caller's worktree")

    def test_release_clears_the_contention(self):
        self.claim_both()
        sh("release", "f.txt", cwd=self.repo)
        self.assertIn("no claims", sh("peers", "f.txt", cwd=self.repo).stdout)


class IntegrateReleaseNudge(unittest.TestCase):
    """scripts/integrate.sh names an unreleased co-edit barrier instead of
    silently leaving it for a later round to rediscover (packet
    GH-INTEGRATE-RELEASE-NUDGE, CLAUDE.md, practice §77).

    `integrate.sh` resolves its own repo root from its OWN script location
    (`dirname "${BASH_SOURCE[0]}"/..`), not from cwd, so it cannot be pointed
    at a throwaway repo the way `coedit.sh` can. Exercising it means copying
    the real script — and the real `coedit.sh`, whose interface these tests
    also pin — into a fake project tree, with stub `cargo` and
    `blast-radius.sh` so the run needs no Rust toolchain. This never invokes
    the real `scripts/integrate.sh` against a live `.worktrees/` entry, which
    the packet forbids.
    """

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.fake = Path(self.tmp.name) / "fake_repo"
        self.fake.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=self.fake, check=True)
        (self.fake / "README.md").write_text("fake\n")
        # .agent-runtime/ and .worktrees/ are gitignored in the real repo —
        # coedit.sh's claim state and every worker worktree live there, and
        # neither should make integrate.sh's own dirty-tree check trip on
        # state that was never meant to be committed to the main checkout.
        (self.fake / ".gitignore").write_text(".agent-runtime/\n.worktrees/\n")

        scripts = self.fake / "scripts"
        scripts.mkdir()
        for name in ("integrate.sh", "coedit.sh"):
            dst = scripts / name
            dst.write_text((REPO / "scripts" / name).read_text())
            dst.chmod(0o755)
        blast = scripts / "blast-radius.sh"
        blast.write_text("#!/usr/bin/env bash\necho 'blast-radius: (stub)'\nexit 0\n")
        blast.chmod(0o755)

        subprocess.run(["git", "add", "-A"], cwd=self.fake, check=True)
        subprocess.run(["git", "-c", "user.email=t@t", "-c", "user.name=t",
                        "commit", "-qm", "base"], cwd=self.fake, check=True)

        # `cargo fmt --all` is unconditional in integrate.sh and this fake repo
        # has no Cargo.toml — stub cargo on PATH rather than touch that line.
        bindir = Path(self.tmp.name) / "bin"
        bindir.mkdir()
        cargo_stub = bindir / "cargo"
        cargo_stub.write_text("#!/usr/bin/env bash\nexit 0\n")
        cargo_stub.chmod(0o755)
        self.env = dict(os.environ)
        self.env["PATH"] = f"{bindir}:{self.env['PATH']}"

        self.worktrees = self.fake / ".worktrees"
        self.worktrees.mkdir()

    def tearDown(self):
        self.tmp.cleanup()

    def _make_worktree(self, name):
        wt = self.worktrees / name
        subprocess.run(["git", "worktree", "add", "--detach", "-f", str(wt), "HEAD"],
                       cwd=self.fake, capture_output=True, check=True)
        (wt / "README.md").write_text(f"fake\nedited by {name}\n")
        return wt

    def _integrate(self, *names):
        return subprocess.run(
            [str(self.fake / "scripts" / "integrate.sh"), *names],
            cwd=self.fake, env=self.env, capture_output=True, text=True,
        )

    def _coedit(self, *args):
        return subprocess.run(
            [str(self.fake / "scripts" / "coedit.sh"), *args],
            cwd=self.fake, env=self.env, capture_output=True, text=True,
        )

    def test_the_nudge_fires_when_the_integrated_worker_holds_a_claim(self):
        wt = self._make_worktree("wa")
        self._coedit("claim", "shared.txt", "wa", str(wt))
        r = self._integrate("wa")
        self.assertIn("scripts/coedit.sh release shared.txt", r.stdout,
                      r.stdout + r.stderr)

    def test_silence_when_the_integrated_worker_holds_nothing(self):
        self._make_worktree("wb")
        r = self._integrate("wb")
        self.assertNotIn("coedit.sh release", r.stdout, r.stdout)

    def test_integrate_does_not_release_the_barrier_itself(self):
        wt = self._make_worktree("wa")
        self._coedit("claim", "shared.txt", "wa", str(wt))
        self._integrate("wa")
        status = self._coedit("status", "shared.txt").stdout
        self.assertIn("peer wa", status)
        self.assertNotIn("no claims", status)

    def test_exit_code_and_existing_output_survive_the_nudge(self):
        wt = self._make_worktree("wa")
        self._coedit("claim", "shared.txt", "wa", str(wt))
        r = self._integrate("wa")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("cargo fmt --all: done", r.stdout)
        self.assertIn("Still yours, and not delegable:", r.stdout)
        self.assertIn('rule on every box, write the evidence, commit, push',
                      r.stdout)


if __name__ == "__main__":
    unittest.main()
