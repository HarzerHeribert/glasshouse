#!/usr/bin/env python3
"""The decisive test for scripts/new-packet.sh.

Generate one of each variant and run validate_round.py's own checks over
them in-process; assert PASSED. If this test passes, new-packet.sh's whole
job — a packet skeleton that is valid by construction — is proven. If it
does not, nothing else about the tool matters.

Runs against the real repository (not a synthetic sandbox) so the
capability-map.md line numbers it exercises are real, current box lines —
the same file validate_round.py itself reads by default. Every generated
packet is removed at the end, on success or failure.
"""
from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPTS_DIR = REPO_ROOT / "scripts"
NEW_PACKET = SCRIPTS_DIR / "new-packet.sh"
MAP_PATH = REPO_ROOT / "docs" / "product" / "capability-map.md"
AGENT_RUNTIME = REPO_ROOT / ".agent-runtime"

# Distinctive prefix so this test can never collide with a real packet name,
# and so cleanup can find everything it created even after a failure.
PREFIX = "npt-selftest"

GENERATED: list[Path] = []


def _load_validate_round():
    spec = importlib.util.spec_from_file_location(
        "validate_round", SCRIPTS_DIR / "validate_round.py"
    )
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def _run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["bash", str(NEW_PACKET), *args],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )


def _packet_path(name: str) -> Path:
    p = AGENT_RUNTIME / f"packet-{name}.md"
    GENERATED.append(p)
    return p


def _generate(name: str, *flags: str) -> Path:
    result = _run(name, *flags)
    assert result.returncode == 0, (
        f"new-packet.sh {name} {' '.join(flags)} failed:\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    path = _packet_path(name)
    assert path.exists(), f"new-packet.sh reported success but {path} was not written"
    return path


def _first_line_no(predicate) -> int:
    with open(MAP_PATH, encoding="utf-8") as f:
        for i, line in enumerate(f, start=1):
            if predicate(line):
                return i
    raise AssertionError("no matching line found in capability-map.md")


def main() -> int:
    validate_round = _load_validate_round()
    failures: list[str] = []

    def check(label: str, paths: list[Path]) -> None:
        findings = validate_round.validate([str(p) for p in paths], str(MAP_PATH))
        if findings:
            failures.append(f"{label}: " + "; ".join(str(f) for f in findings))
        else:
            print(f"PASSED — {label}")

    # 1 & 2: default and --recon variants each pass alone.
    default_pkt = _generate(f"{PREFIX}-demo")
    recon_pkt = _generate(f"{PREFIX}-demo-recon", "--recon")
    check("default variant alone", [default_pkt])
    check("recon variant alone", [recon_pkt])

    # 2b: the default skeleton's gate line is pre-filled --targeted. The
    # validator refuses the bare form (gate-is-targeted), so drift would
    # already fail check 1 -- this asserts the line is PRESENT, because a
    # skeleton with no gate line at all would pass that check vacuously.
    skeleton = default_pkt.read_text(encoding="utf-8")
    if "scripts/blast-radius.sh --targeted" not in skeleton:
        failures.append("default skeleton does not pre-fill `scripts/blast-radius.sh --targeted`")
    elif "scripts/blast-radius.sh --targeted" in recon_pkt.read_text(encoding="utf-8"):
        failures.append("recon skeleton names a gate; recon runs no build or test")
    else:
        print("PASSED — default skeleton pre-fills the targeted gate; recon does not")

    # 3: both together — must not collide on YOURS paths.
    check("default + recon together", [default_pkt, recon_pkt])

    # 4a: --lines with a real box number quotes the map text verbatim.
    box_line_no = _first_line_no(lambda line: line.startswith(("☐", "☑")))
    lines_pkt = _generate(f"{PREFIX}-lines", "--lines", str(box_line_no))
    check("--lines variant", [lines_pkt])
    with open(MAP_PATH, encoding="utf-8") as f:
        map_line = f.readlines()[box_line_no - 1].strip()
    quoted = lines_pkt.read_text(encoding="utf-8")
    box_text = map_line[1:].strip()
    if box_text not in quoted:
        failures.append(
            f"--lines: box text from map line {box_line_no} not found verbatim "
            f"in {lines_pkt}"
        )
    else:
        print("PASSED — --lines quotes the map line verbatim")

    # 4b: --lines with a non-box line number fails loudly (and writes nothing).
    non_box_line_no = _first_line_no(
        lambda line: line.strip() and not line.startswith(("☐", "☑"))
    )
    bad_name = f"{PREFIX}-bad-lines"
    result = _run(bad_name, "--lines", str(non_box_line_no))
    bad_path = AGENT_RUNTIME / f"packet-{bad_name}.md"
    if result.returncode == 0:
        failures.append(
            f"--lines {non_box_line_no} (not a box line) should fail loudly, "
            f"exited 0 instead: {result.stdout}"
        )
    elif bad_path.exists():
        GENERATED.append(bad_path)
        failures.append(f"--lines failure still wrote {bad_path}")
    else:
        print("PASSED — --lines fails loudly on a non-box line number, writes nothing")

    # 5: refuses to overwrite without --force, allows it with --force.
    result = _run(f"{PREFIX}-demo")
    if result.returncode == 0:
        failures.append("re-running new-packet.sh on an existing packet should refuse")
    else:
        print("PASSED — refuses to overwrite without --force")

    result = _run(f"{PREFIX}-demo", "--force")
    if result.returncode != 0:
        failures.append(f"--force should allow overwrite: {result.stderr}")
    else:
        print("PASSED — --force allows overwrite")

    for p in GENERATED:
        p.unlink(missing_ok=True)
    try:
        next(AGENT_RUNTIME.iterdir())
    except (StopIteration, FileNotFoundError):
        if AGENT_RUNTIME.exists():
            AGENT_RUNTIME.rmdir()

    if failures:
        print("FAILED:")
        for f in failures:
            print(f"  {f}")
        return 1
    print("ALL PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
