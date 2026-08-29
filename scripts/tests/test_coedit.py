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


if __name__ == "__main__":
    unittest.main()
