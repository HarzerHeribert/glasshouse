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
import subprocess
import sys
import tempfile
from pathlib import Path

MUTATE = Path(__file__).resolve().parents[1] / "mutate.sh"

PASS_CMD = [sys.executable, "-c", "import sys; sys.exit(0)"]
FAIL_CMD = [sys.executable, "-c", "import sys; sys.exit(1)"]

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

    if failures:
        print(f"test_mutate: {len(failures)} failure(s)", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1

    print("test_mutate: ok — refusal, KILLED, SURVIVED and --expect-survive all leave the file byte-identical")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
