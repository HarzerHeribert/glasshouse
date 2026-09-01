"""The destructive-git guard: what it must refuse and what it must never touch.

Two incidents, two halves. 2026-08-26: `git checkout -- <path>` in a worker's
worktree deleted 161 lines of uncommitted deliverable. 2026-09-01: `git add -A`
in the main checkout, minutes after `scripts/integrate.sh` had applied a
worker's diff, swept a 1005-line implementation into a one-paragraph docs
commit (645d6cf) and pushed it. The first half discards work; the second
commits work under the wrong name. Both are one habitual keystroke.

The allow list matters as much as the deny list: a guard that blocks
`git log --all` or `git commit --amend` teaches the model to route around it.
"""

import json
import pathlib
import subprocess
import unittest

HOOK = pathlib.Path(__file__).resolve().parents[1] / "hooks" / "guard-destructive-git.sh"


def run(command, tool="Bash"):
    payload = json.dumps({"tool_name": tool, "tool_input": {"command": command}})
    r = subprocess.run([str(HOOK)], input=payload, capture_output=True, text=True)
    return r.returncode, r.stderr


BLOCKED_SWEEPS = [
    "git add -A",
    "git add --all",
    "git add .",
    "git add ./",
    "git add :/",
    "git add -- .",
    "git add -u",
    "git add --update",
    "git add -Av",
    "git -C /some/where add -A",
    'git commit -am "x"',
    "git commit -a -m x",
    "git commit --all -m x",
    "git commit -qam x",
    "cargo fmt --all && git add -A && git commit -m x",
    "git add -A; git commit -m x",
]

ALLOWED_STAGES = [
    "git add -- docs/a.md scripts/b.sh",
    "git add docs/product/capability-map.md",
    "git add -u -- docs/",
    "git add -u crates/glasshouse/src/main.rs",
    "git add -p crates/x.rs",
    "git add scripts/hooks/*.sh",
    'git commit -m "x"',
    "git commit --amend --no-edit",
    "git commit -qm x",
    "git commit -F /tmp/msg",
    "git status --short",
    "git diff --stat",
    "git log --all --oneline",
    "git checkout -b claude/foo",
    "git worktree add /abs/path",
    "git stash list",
    "git clean -n",
]

BLOCKED_DISCARDS = [
    "git checkout -- crates/glasshouse/src/provider/mod.rs",
    "git checkout crates/glasshouse/src/provider/mod.rs",
    "git restore crates/x.rs",
    "git stash",
    "git stash push",
    "git clean -fd",
]


class SweepingStage(unittest.TestCase):
    def test_every_sweeping_form_is_blocked(self):
        for cmd in BLOCKED_SWEEPS:
            rc, err = run(cmd)
            self.assertEqual(rc, 2, f"should block: {cmd!r}\n{err}")

    def test_every_pathspec_form_and_read_only_command_is_allowed(self):
        for cmd in ALLOWED_STAGES:
            rc, err = run(cmd)
            self.assertEqual(rc, 0, f"should allow: {cmd!r}\n{err}")

    def test_the_block_names_the_incident_and_the_replacement(self):
        """The message is the teaching; a bare 'blocked' teaches routing around."""
        rc, err = run("git add -A")
        self.assertEqual(rc, 2)
        self.assertIn("645d6cf", err)
        self.assertIn("integrate.sh", err)
        self.assertIn("git add -- <the files this commit is about>", err)

    def test_the_block_shows_the_tree_so_the_pathspec_can_be_chosen(self):
        rc, err = run("git commit -am x")
        self.assertEqual(rc, 2)
        self.assertIn("git status --short", err)


class DiscardingCommands(unittest.TestCase):
    def test_the_original_four_are_still_blocked(self):
        for cmd in BLOCKED_DISCARDS:
            rc, err = run(cmd)
            self.assertEqual(rc, 2, f"should block: {cmd!r}\n{err}")

    def test_non_bash_tools_are_ignored(self):
        rc, _ = run("git add -A", tool="Read")
        self.assertEqual(rc, 0)


if __name__ == "__main__":
    unittest.main()
