#!/usr/bin/env python3
"""No test binary may carry a name Windows treats as an installer.

Windows' installer-detection heuristic looks at an executable's file name:
one containing ``install``, ``setup``, ``update`` or ``patch`` is assumed to
need elevation and refuses to start under a standard user (``The requested
operation requires elevation. (os error 740)``). Cargo names each integration
test binary after its source file, so ``tests/dispatch_reservation.rs``
(dis-*patch*) and ``tests/v1_criteria_setup.rs`` never executed on the
Windows VM leg — found on 2026-09-02, after an unknown number of runs in
which both showed as ``error: test failed`` with no test output at all. Both
were renamed; this keeps the next one out.

Only the file's own basename matters: a ``mod`` inside the library crate is
not a binary, and the binary's own name (``glasshouse``) is checked too.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
INSTALLER_WORDS = ("install", "setup", "update", "patch")


def offenders() -> list[str]:
    found: list[str] = []
    for crate in (REPO / "crates").iterdir():
        for tests_dir in (crate / "tests", crate / "src" / "bin"):
            if not tests_dir.is_dir():
                continue
            for path in sorted(tests_dir.glob("*.rs")):
                stem = path.stem.lower()
                hit = [w for w in INSTALLER_WORDS if w in stem]
                if hit:
                    found.append(f"{path.relative_to(REPO)} (contains {', '.join(hit)})")
        manifest = crate / "Cargo.toml"
        if manifest.is_file():
            for m in re.finditer(r'^name\s*=\s*"([^"]+)"', manifest.read_text(), re.M):
                name = m.group(1).lower()
                hit = [w for w in INSTALLER_WORDS if w in name]
                if hit:
                    found.append(f"{manifest.relative_to(REPO)}: name {name!r} (contains {', '.join(hit)})")
    return found


def main() -> int:
    bad = offenders()
    if bad:
        print("test binaries Windows will refuse to start (installer detection, os error 740):")
        for line in bad:
            print(f"  {line}")
        print("rename the file; see the docstring of this test for why")
        return 1
    print("test_windows_test_binary_names: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
