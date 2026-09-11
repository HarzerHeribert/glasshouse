"""scripts/ci/rerun-failed-alone.sh: a failed test is rerun alone by its own
binary, a pass alone is a named flaky-pass and exit 0, a red alone is exit 1."""
import os
import pathlib
import stat
import subprocess
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "ci" / "rerun-failed-alone.sh"


def fake_binary(root: pathlib.Path) -> str:
    """A test binary that is red for `always_red` and green for anything else,
    and records every argv it was given."""
    rel = os.path.join("target", "debug", "deps", "fake-0123abcd")
    path = root / rel
    path.parent.mkdir(parents=True)
    path.write_text(
        "#!/bin/sh\n"
        'echo "$@" >> "$0.argv"\n'
        'case "$1" in always_red) echo "thread \'always_red\' panicked at x.rs:1:1:"; '
        "echo still broken; exit 101;; esac\n"
        "echo 'test result: ok. 1 passed'\n"
    )
    path.chmod(path.stat().st_mode | stat.S_IEXEC)
    return rel


def run(root: pathlib.Path, log_text: str):
    log = root / "test.log"
    log.write_text(log_text)
    return subprocess.run(
        ["bash", str(SCRIPT), str(log)],
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )


class RerunFailedAlone(unittest.TestCase):
    def test_a_flaky_test_passes_alone_and_a_real_red_stays_red(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            rel = fake_binary(root)
            log = (
                f"     Running tests/fake.rs ({rel})\n"
                "test flaky_one ... FAILED\n"
                "test steady ... ok\n"
                "test always_red ... FAILED\n"
                "failures:\n    flaky_one\n    always_red\n"
                "     Doc-tests glasshouse\n"
                "test src/lib.rs - doc (line 3) ... FAILED\n"
            )
            result = run(root, log)
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("flaky-pass: flaky_one", result.stdout)
            self.assertIn("still red: always_red", result.stdout)
            self.assertIn("still broken", result.stdout)
            self.assertNotIn("doc (line 3)", result.stdout, "a doctest has no binary to rerun")
            argv = (root / rel).with_name("fake-0123abcd.argv").read_text()
            self.assertIn("flaky_one --exact --test-threads=1 --nocapture", argv)
            self.assertIn("always_red --exact --test-threads=1 --nocapture", argv)
            self.assertNotIn("steady", argv, "a passing test is never rerun")

    def test_every_failure_passing_alone_is_exit_zero_and_named(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            rel = fake_binary(root)
            result = run(root, f"     Running tests/fake.rs ({rel})\ntest flaky_one ... FAILED\n")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("flaky-pass: flaky_one", result.stdout)
            self.assertIn("1 flaky-pass, 0 still red", result.stdout)

    def test_no_failure_is_exit_zero(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            result = run(root, "test steady ... ok\ntest result: ok. 1 passed\n")
            self.assertEqual(result.returncode, 0)
            self.assertIn("no failed test", result.stdout)

    def test_a_windows_shaped_path_is_recognised(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            rel = fake_binary(root)
            windows = rel.replace("/", "\\\\") + ".exe"
            # The binary named in the log does not exist under that spelling,
            # so the rerun fails to start: what matters is that the failure was
            # attributed to a binary at all rather than skipped as unrunnable.
            result = run(root, f"     Running tests\\fake.rs ({windows})\ntest flaky_one ... FAILED\n")
            self.assertIn("rerun-failed-alone: flaky_one (", result.stdout)


if __name__ == "__main__":
    unittest.main()
