"""The co-edit claim guard: a declared COEDIT file is not editable before it is claimed.

validate_round.py enforces that both packets DECLARE a shared file (mutual
`COEDIT:` lines). Nothing enforced that the worker then CLAIMED it — and the
claim is the half the peer, the Stop hook and integrate.sh's release nudge
all key on. This hook refuses the first edit until the claim exists, and is
silent everywhere else: the orchestrator's checkout, undeclared files, and a
worker with no packet.

Runs against a throwaway worktree of the real repo so the git-common-dir
resolution the hook relies on is the real one.
"""

import json
import pathlib
import subprocess
import unittest
import uuid

HOOK = pathlib.Path(__file__).resolve().parents[1] / "hooks" / "coedit-claim-guard.sh"
REPO = HOOK.parents[2]

# The hook never trusts `parents[...]` for "the main checkout" — it resolves
# through `git rev-parse --path-format=absolute --git-common-dir` (shared by
# every worktree of this repo) and takes that path's parent. `REPO` above is
# just "the tree this test file happens to live in"; from the main checkout
# the two coincide, but from a worker's worktree they do not, so fixtures
# written under `REPO/.agent-runtime` land somewhere the hook never looks.
# Resolve the fixture root the same way the hook resolves its own.
_COMMON = subprocess.run(
    ["git", "-C", str(REPO), "rev-parse", "--path-format=absolute", "--git-common-dir"],
    check=True, capture_output=True, text=True,
).stdout.strip()
MAIN = pathlib.Path(_COMMON).parent
FILE = "crates/glasshouse/src/main.rs"


def hook(cwd, file_path, tool="Edit"):
    payload = json.dumps({"tool_name": tool, "cwd": str(cwd), "tool_input": {"file_path": str(file_path)}})
    r = subprocess.run([str(HOOK)], input=payload, capture_output=True, text=True)
    return r.returncode, r.stderr


class ClaimGuard(unittest.TestCase):
    def setUp(self):
        self.name = f"zz-claim-{uuid.uuid4().hex[:6]}"
        self.wt = MAIN / ".worktrees" / self.name
        self.branch = f"claude/{self.name}"
        subprocess.run(
            ["git", "worktree", "add", "-q", str(self.wt), "-b", self.branch, "HEAD"],
            cwd=MAIN, check=True, capture_output=True,
        )
        self.packet = MAIN / ".agent-runtime" / f"packet-{self.name}.md"
        self.packet.write_text(f"# PACKET\n\nCOEDIT: {FILE}\n")

    def tearDown(self):
        subprocess.run(["scripts/coedit.sh", "release", FILE], cwd=MAIN, capture_output=True)
        self.packet.unlink(missing_ok=True)
        subprocess.run(["git", "worktree", "remove", "--force", str(self.wt)], cwd=MAIN, capture_output=True)
        subprocess.run(["git", "branch", "-D", self.branch], cwd=MAIN, capture_output=True)

    def claim(self):
        subprocess.run(
            ["scripts/coedit.sh", "claim", FILE, self.name], cwd=self.wt, check=True, capture_output=True
        )

    def test_declared_and_unclaimed_is_blocked_and_the_message_gives_the_command(self):
        rc, err = hook(self.wt, self.wt / FILE)
        self.assertEqual(rc, 2, err)
        self.assertIn(f"scripts/coedit.sh claim {FILE} {self.name}", err)
        rc, _ = hook(self.wt, FILE)          # relative form, same answer
        self.assertEqual(rc, 2)

    def test_once_claimed_the_edit_is_allowed(self):
        self.claim()
        rc, err = hook(self.wt, self.wt / FILE)
        self.assertEqual(rc, 0, err)

    def test_an_undeclared_file_is_never_gated(self):
        rc, err = hook(self.wt, self.wt / "crates/glasshouse/src/lib.rs")
        self.assertEqual(rc, 0, err)

    def test_the_orchestrator_in_the_main_checkout_is_never_gated(self):
        rc, err = hook(MAIN, MAIN / FILE)
        self.assertEqual(rc, 0, err)

    def test_a_worktree_with_no_packet_is_never_gated(self):
        self.packet.unlink()
        rc, err = hook(self.wt, self.wt / FILE)
        self.assertEqual(rc, 0, err)

    def test_a_subpacket_governs_a_subcontractor_the_same_way(self):
        self.packet.unlink()
        sub = MAIN / ".agent-runtime" / f"subpacket-{self.name}.md"
        sub.write_text(f"# SUBPACKET\n\nCOEDIT: `{FILE}`\n")
        try:
            rc, _ = hook(self.wt, self.wt / FILE)
            self.assertEqual(rc, 2)
        finally:
            sub.unlink()


if __name__ == "__main__":
    unittest.main()
