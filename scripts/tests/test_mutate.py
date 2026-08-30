#!/usr/bin/env python3
"""Acceptance tests for `mutate.sh`, the §16 mutation-ritual runner.

WHY THIS EXISTS
---------------
§16 is six manual steps, and the step most likely to be skipped under time
pressure is the last one: confirm the restored file is byte-identical, not
just "looks the same". These tests exist to prove that refusal paths and
KILLED/SURVIVED paths alike leave the file untouched — asserted on a hash,
never on a diff appearing empty, since an empty diff can hide a change diff
itself failed to detect (e.g. a byte-identical-looking file with a moved
mtime, which is the exact failure §16 records).

These use `--test-cmd` with a python one-liner instead of `cargo test`, so
this file runs in under a second and needs no build — `/bin/true` and
`/bin/false` are not portable (macOS ships them at `/usr/bin/`, not `/bin/`),
so `sys.executable -c "..."` is used instead.
"""
import hashlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

MUTATE = Path(__file__).resolve().parents[1] / "mutate.sh"

PASS_CMD = [sys.executable, "-c", "import sys; sys.exit(0)"]
FAIL_CMD = [sys.executable, "-c", "import sys; sys.exit(1)"]

# --- tree-resolution fixtures ------------------------------------------------
# GH-MUTATE-TREE: mutate.sh derives its own tree from its own on-disk location
# (BASH_SOURCE), not from the caller. scripts/ is tracked, so every worktree
# has its own copy, and running the MAIN CHECKOUT's copy from inside a
# worktree used to mutate the main checkout's file while the caller believed
# it was testing its own tree. These fixtures build a real main checkout plus
# a real `git worktree`, so the tests exercise actual git kinship resolution
# rather than a stand-in.


def _git(*args: str, cwd: Path) -> None:
    subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True)


def _build_repo_and_worktree(base: Path) -> tuple[Path, Path]:
    """Return (main_checkout, worktree), each with its own copy of mutate.sh
    and a committed target.rs containing SOURCE."""
    main_dir = base / "main"
    main_dir.mkdir(parents=True)
    _git("init", "-q", cwd=main_dir)
    _git("config", "user.email", "test@test.invalid", cwd=main_dir)
    _git("config", "user.name", "test", cwd=main_dir)
    scripts_dir = main_dir / "scripts"
    scripts_dir.mkdir()
    shutil.copy(MUTATE, scripts_dir / "mutate.sh")
    (main_dir / "target.rs").write_text(SOURCE)
    _git("add", "-A", cwd=main_dir)
    _git("commit", "-q", "-m", "init", cwd=main_dir)
    worktree_dir = base / "worktree"
    _git("worktree", "add", "-q", "-b", "wt-test-branch", str(worktree_dir), cwd=main_dir)
    return main_dir, worktree_dir


def _run_from(cwd: Path, main_dir: Path, *extra_args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["bash", str(main_dir / "scripts" / "mutate.sh"), *extra_args],
        cwd=str(cwd),
        capture_output=True,
        text=True,
        timeout=10,
    )


def test_tree_resolution(failures: list) -> None:
    # 1 & 2: the defect, reproduced then fixed, and the announcement. Invoke
    # the MAIN CHECKOUT's mutate.sh from inside the WORKTREE against a file
    # that exists, identically, in both trees. Assert the file that actually
    # changed DURING the run — not after, since the ritual always restores —
    # is the caller's (worktree's), not the main checkout's.
    with tempfile.TemporaryDirectory() as tmp:
        main_dir, worktree_dir = _build_repo_and_worktree(Path(tmp))
        sentinel = Path(tmp) / "sentinel.txt"
        main_target = main_dir / "target.rs"
        wt_target = worktree_dir / "target.rs"
        capture_code = (
            "import pathlib, sys\n"
            "sentinel, a, b = sys.argv[1], sys.argv[2], sys.argv[3]\n"
            "pathlib.Path(sentinel).write_text("
            "pathlib.Path(a).read_text() + chr(30) + pathlib.Path(b).read_text())\n"
        )
        result = _run_from(
            worktree_dir, main_dir,
            "--file", "target.rs", "--find", "a + b", "--replace", "a - b",
            "--allow-dirty", "--test-cmd", sys.executable, "-c", capture_code,
            str(sentinel), str(main_target), str(wt_target),
        )
        if not sentinel.exists():
            failures.append(
                f"tree-resolution: --test-cmd never ran\nstdout={result.stdout}\nstderr={result.stderr}"
            )
        else:
            parts = sentinel.read_text().split(chr(30))
            main_seen, wt_seen = (parts + ["", ""])[:2]
            if main_seen != SOURCE:
                failures.append(
                    f"mutate.sh mutated the MAIN CHECKOUT's file while invoked from a "
                    f"worktree — it must mutate the caller's tree instead: {main_seen!r}"
                )
            if "a - b" not in wt_seen:
                failures.append(
                    f"mutate.sh did not mutate the CALLER's (worktree) file: {wt_seen!r}"
                )
        combined = result.stdout + result.stderr
        if "operating on the caller's worktree" not in combined or str(worktree_dir) not in combined:
            failures.append(f"mutate.sh did not announce the tree it operated on:\n{combined}")

    # 3: the refusal, from a tree unrelated to the repo mutate.sh lives in —
    # both a fresh, unrelated `git init` and a plain non-git directory.
    with tempfile.TemporaryDirectory() as tmp:
        base = Path(tmp)
        main_dir, _worktree_dir = _build_repo_and_worktree(base / "repo")

        other_git = base / "other-git"
        other_git.mkdir()
        _git("init", "-q", cwd=other_git)
        result = _run_from(
            other_git, main_dir,
            "--file", "target.rs", "--find", "a + b", "--replace", "a - b",
            "--test-cmd", sys.executable, "-c", "import sys; sys.exit(0)",
        )
        combined = result.stdout + result.stderr
        if result.returncode == 0:
            failures.append(f"mutate.sh did not refuse an unrelated git tree (exit 0)\n{combined}")
        if str(other_git) not in combined or str(main_dir) not in combined:
            failures.append(f"unrelated-tree refusal did not name both paths:\n{combined}")

        non_git = base / "non-git"
        non_git.mkdir()
        result = _run_from(
            non_git, main_dir,
            "--file", "target.rs", "--find", "a + b", "--replace", "a - b",
            "--test-cmd", sys.executable, "-c", "import sys; sys.exit(0)",
        )
        combined = result.stdout + result.stderr
        if result.returncode == 0:
            failures.append(f"mutate.sh did not refuse a non-git caller directory (exit 0)\n{combined}")
        if str(non_git) not in combined or str(main_dir) not in combined:
            failures.append(f"non-git refusal did not name both paths:\n{combined}")

    # 4: the dirty guard must fire under the PREVIOUSLY-SLIPPING form — a
    # dirty file in the caller's worktree, invoked via the main checkout's
    # script, refused without --allow-dirty.
    with tempfile.TemporaryDirectory() as tmp:
        main_dir, worktree_dir = _build_repo_and_worktree(Path(tmp))
        wt_target = worktree_dir / "target.rs"
        wt_target.write_text(SOURCE + "// dirty, uncommitted\n")
        before = sha(wt_target)
        result = _run_from(
            worktree_dir, main_dir,
            "--file", "target.rs", "--find", "a + b", "--replace", "a - b",
            "--test-cmd", sys.executable, "-c", "import sys; sys.exit(0)",
        )
        if result.returncode == 0:
            failures.append(
                "mutate.sh did not refuse a dirty caller-worktree file under the "
                f"main-checkout invocation form (exit 0)\n{result.stdout}\n{result.stderr}"
            )
        if "uncommitted changes" not in result.stderr:
            failures.append(f"dirty-guard refusal message missing expected text:\n{result.stderr}")
        if sha(wt_target) != before:
            failures.append("dirty caller-worktree file was modified despite the guard refusing")

SOURCE = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n"
TWICE_SOURCE = "let x = a + b;\nlet y = a + b;\n"


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run_mutate(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["bash", str(MUTATE), *args],
        capture_output=True,
        text=True,
        timeout=10,
    )


def main() -> int:
    failures = []

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)

        # 1. A find string occurring twice is refused, and the file is unchanged.
        twice_file = tmp_path / "twice.rs"
        twice_file.write_text(TWICE_SOURCE)
        before = sha(twice_file)
        result = run_mutate(
            "--file", str(twice_file), "--find", "a + b", "--replace", "a - b",
            "--allow-dirty", "--test-cmd", *PASS_CMD,
        )
        if result.returncode == 0:
            failures.append(
                f"twice-occurring find string was not refused (exit {result.returncode})\n{result.stderr}"
            )
        if sha(twice_file) != before:
            failures.append("file changed after a refused (2x-occurrence) mutation")

        # 2. A find string occurring zero times is refused, and the file is unchanged.
        zero_file = tmp_path / "zero.rs"
        zero_file.write_text(SOURCE)
        before = sha(zero_file)
        result = run_mutate(
            "--file", str(zero_file), "--find", "does not appear", "--replace", "x",
            "--allow-dirty", "--test-cmd", *PASS_CMD,
        )
        if result.returncode == 0:
            failures.append(
                f"zero-occurrence find string was not refused (exit {result.returncode})\n{result.stderr}"
            )
        if sha(zero_file) != before:
            failures.append("file changed after a refused (0-occurrence) mutation")

        # 3. A mutation whose test fails reports KILLED and exits 0.
        killed_file = tmp_path / "killed.rs"
        killed_file.write_text(SOURCE)
        before = sha(killed_file)
        result = run_mutate(
            "--file", str(killed_file), "--find", "a + b", "--replace", "a - b",
            "--allow-dirty", "--test-cmd", *FAIL_CMD,
        )
        if result.returncode != 0:
            failures.append(f"KILLED mutation should exit 0, got {result.returncode}\n{result.stdout}\n{result.stderr}")
        if "KILLED" not in result.stdout:
            failures.append(f"KILLED mutation did not report KILLED:\n{result.stdout}")
        # 5. Byte-identical after this run too.
        if sha(killed_file) != before:
            failures.append("file not byte-identical after a KILLED mutation")

        # 4. A mutation whose test passes reports SURVIVED and exits 1.
        survived_file = tmp_path / "survived.rs"
        survived_file.write_text(SOURCE)
        before = sha(survived_file)
        result = run_mutate(
            "--file", str(survived_file), "--find", "a + b", "--replace", "a - b",
            "--allow-dirty", "--test-cmd", *PASS_CMD,
        )
        if result.returncode != 1:
            failures.append(f"SURVIVED mutation should exit 1, got {result.returncode}\n{result.stdout}\n{result.stderr}")
        if "SURVIVED" not in result.stdout:
            failures.append(f"SURVIVED mutation did not report SURVIVED:\n{result.stdout}")
        if sha(survived_file) != before:
            failures.append("file not byte-identical after a SURVIVED mutation")
        # The ordinary case (mutate.sh invoked from within its own tree) must
        # print no new output — the cross-tree announcement is for the case
        # where the caller's tree actually differs from mutate.sh's own.
        combined = result.stdout + result.stderr
        if "operating on the caller's worktree" in combined:
            failures.append(
                f"mutate.sh announced a tree switch for an ordinary same-tree invocation:\n{combined}"
            )

        # Bonus: --expect-survive turns the same SURVIVED result into exit 0.
        expect_file = tmp_path / "expect.rs"
        expect_file.write_text(SOURCE)
        before = sha(expect_file)
        result = run_mutate(
            "--file", str(expect_file), "--find", "a + b", "--replace", "a - b",
            "--allow-dirty", "--expect-survive", "--test-cmd", *PASS_CMD,
        )
        if result.returncode != 0:
            failures.append(f"--expect-survive should exit 0 on SURVIVED, got {result.returncode}")
        if sha(expect_file) != before:
            failures.append("file not byte-identical after an --expect-survive mutation")

    test_tree_resolution(failures)

    if failures:
        print(f"test_mutate: {len(failures)} failure(s)", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1

    print("test_mutate: ok — refusal, KILLED, SURVIVED and --expect-survive all leave the file byte-identical")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
