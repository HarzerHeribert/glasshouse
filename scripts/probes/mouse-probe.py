#!/usr/bin/env python3
"""What does THIS terminal actually deliver under each mouse mode?

The design question this settles: mouse capture costs drag-to-select, and the
usual claim is "hold Shift or Option to select anyway". That is a claim about
the TERMINAL, not the app -- so it is measurable. If a Shift-drag arrives here
as a mouse report, the terminal forwarded it and selection is gone. If it does
NOT arrive, the terminal kept it for itself, and that modifier is the escape
hatch.

It also separates the three DECSET modes, because crossterm's EnableMouseCapture
turns on all of them at once and a narrower request may cost less:

    ?1000h  press and release only
    ?1002h  ... plus drag while a button is held
    ?1003h  ... plus ALL motion, button or not
    ?1006h  SGR encoding (unambiguous, no 223-column limit)

Nothing is written to your terminal settings. Raw mode and every mode enabled
here are restored on exit, including on Ctrl-C and on a crash.

Run it in a pane you do not mind clicking around in:

    python3 .agent-runtime/probes/mouse-probe.py
"""

from __future__ import annotations

import atexit
import json
import os
import re
import select
import signal
import sys
import termios
import time
import tty
from datetime import datetime, timezone
from pathlib import Path

def out_path(device: str) -> Path:
    """One file per device. "both" is not a measurement -- run it twice."""
    return Path(__file__).with_name(f"mouse-probe-results-{device}.json")

# Each step: (key, prompt, seconds). ENTER advances early.
STEPS = [
    ("wheel_up", "Scroll the wheel UP a few times", 20),
    ("click", "LEFT-CLICK once, anywhere in this pane", 20),
    ("drag_plain", "Click and DRAG across some text (no modifier)", 25),
    ("drag_shift", "Hold SHIFT and drag across some text", 25),
    ("drag_alt", "Hold OPTION (alt) and drag across some text", 25),
    ("drag_cmd", "Hold COMMAND and drag across some text", 25),
]

MODES = [
    ("baseline", []),
    ("1000+1006 (press/release, SGR)", ["?1000h", "?1006h"]),
    ("1002+1006 (+ drag)", ["?1000h", "?1002h", "?1006h"]),
    ("1003+1006 (+ all motion)", ["?1000h", "?1002h", "?1003h", "?1006h"]),
]

ALL_OFF = ["?1006l", "?1003l", "?1002l", "?1000l", "?1015l"]

SGR = re.compile(rb"\x1b\[<(\d+);(\d+);(\d+)([Mm])")
X10 = re.compile(rb"\x1b\[M(...)", re.S)

_original: list | None = None


def terminal_size() -> dict:
    """`os.terminal_size` is a struct sequence, not a namedtuple -- no `_asdict`."""
    try:
        size = os.get_terminal_size()
        return {"columns": size.columns, "lines": size.lines}
    except OSError:
        return {"columns": None, "lines": None}


def selection() -> None:
    """Hold the shippable mode open so a human can try to SELECT text in it.

    The probe measures what reaches the application. That is not the same
    question as whether the terminal still selects: "not delivered" is equally
    consistent with the terminal keeping the gesture for selection (an escape
    hatch) and with it swallowing the gesture and doing nothing (a dead key).
    Only a person looking at the screen can tell those apart.
    """
    print("Mode ?1000h + ?1006h is now ON -- the mode we would actually ship.")
    print("It is the one that delivers wheel and click but withholds shift-drag.")
    print()
    print("Try each of these and watch whether text actually HIGHLIGHTS:")
    print("  1. plain drag        (expected: no highlight -- the app ate it)")
    print("  2. SHIFT + drag      (the question: does it highlight?)")
    print("  3. COMMAND + drag    (the question: does it highlight?)")
    print("  4. and if 2 or 3 highlighted, press Cmd-C and paste somewhere")
    print()
    print("Press ENTER here when you are done and the mode will be turned off.")
    try:
        sys.stdout.write("\x1b[?1000h\x1b[?1006h")
        sys.stdout.flush()
        input()
    finally:
        sys.stdout.write("\x1b[?1006l\x1b[?1000l")
        sys.stdout.flush()
    print("\nMode off. Which of shift-drag / command-drag actually highlighted text?")
    sys.exit(0)


def compare() -> None:
    """Print the two device runs side by side, and flag where they disagree."""
    runs = {}
    for device in ("mouse", "trackpad"):
        path = out_path(device)
        if path.exists():
            runs[device] = json.loads(path.read_text())
    if len(runs) < 2:
        have = ", ".join(runs) or "nothing"
        sys.exit(f"need both runs to compare; have: {have}")

    modes = list(runs["mouse"]["modes"])
    print(f"{'mode':32}{'action':13}{'mouse':12}{'trackpad':12}  note")
    print("-" * 92)
    for mode in modes:
        for key, _, _ in STEPS:
            cells = {}
            for device in ("mouse", "trackpad"):
                got = runs[device]["modes"][mode][key]
                n = got["sgr_reports"] + got["x10_reports"]
                cells[device] = (got["delivered"], n)
            note = ""
            if cells["mouse"][0] != cells["trackpad"][0]:
                note = "DIFFERS — the device changes the answer"
            elif cells["trackpad"][1] > cells["mouse"][1] * 3 and cells["mouse"][1] > 0:
                note = f"trackpad emits {cells['trackpad'][1] // max(cells['mouse'][1], 1)}x more"
            print(f"{mode:32}{key:13}"
                  + "".join(f"{('yes' if c[0] else 'no') + '/' + str(c[1]):12}"
                            for c in (cells['mouse'], cells['trackpad']))
                  + f"  {note}")
    print()
    print("Read it like this: a row marked DIFFERS on a drag means the gesture, not")
    print("the terminal, decided the outcome -- trust the MOUSE row for the selection")
    print("question. A large trackpad multiple on wheel_up is the burst any forwarded")
    print("wheel implementation has to coalesce.")
    sys.exit(0)


def selftest() -> None:
    """Exercise the decode path on synthetic reports. No terminal required."""
    cases = [
        (b"\x1b[<64;10;5M", "wheel-up", "press"),
        (b"\x1b[<65;10;5M", "wheel-down", "press"),
        (b"\x1b[<0;40;12M", "left", "press"),
        (b"\x1b[<0;40;12m", "left", "release"),
        (b"\x1b[<35;41;12M", "motion", "press"),
        (b"\x1b[<3;40;12M", "release", "press"),
        (b"\x1b[<39;41;12M", "motion+SHIFT", "press"),
        (b"\x1b[<32;41;12M", "left-drag", "press"),
        (b"\x1b[<36;41;12M", "left-drag+SHIFT", "press"),
        (b"\x1b[<40;41;12M", "left-drag+ALT", "press"),
    ]
    failures = 0
    for raw, expect_decoded, expect_kind in cases:
        got = analyse(raw)
        if got["sgr_reports"] != 1:
            print(f"FAIL {raw!r}: parsed {got['sgr_reports']} reports, expected 1")
            failures += 1
            continue
        first = got["sample"][0]
        if first["decoded"] != expect_decoded or first["kind"] != expect_kind:
            print(f"FAIL {raw!r}: got {first['decoded']}/{first['kind']}, "
                  f"expected {expect_decoded}/{expect_kind}")
            failures += 1
        else:
            print(f"ok   {raw.decode('latin-1'):22} -> {first['decoded']:18} {first['kind']}")

    # A stream with no mouse report at all must read as "not delivered" -- this is
    # the assertion the whole experiment turns on, so it is worth pinning.
    empty = analyse(b"some typed text")
    if empty["delivered"]:
        print("FAIL: plain text was reported as a delivered mouse event")
        failures += 1
    else:
        print("ok   plain text                -> not delivered")

    print(f"\nterminal_size(): {terminal_size()}")
    print("SELFTEST FAILED" if failures else "\nselftest: all decode cases pass")
    sys.exit(1 if failures else 0)


def restore() -> None:
    """Put the terminal back. Registered with atexit AND called in finally."""
    try:
        sys.stdout.write("".join(f"\x1b[{m}" for m in ALL_OFF))
        sys.stdout.flush()
    except Exception:
        pass
    if _original is not None:
        try:
            termios.tcsetattr(sys.stdin.fileno(), termios.TCSADRAIN, _original)
        except Exception:
            pass


def describe(button: int) -> str:
    """Decode an SGR button code.

    The trap: bit 5 (32) means motion and the low two bits are the button, but
    base 3 means NO button -- so `?1003h` bare motion arrives as 35, and
    indexing a three-name list with 3 is an IndexError. That mode is one of the
    four this probe enables, so it would have crashed mid-run.
    """
    base = button & 0b11
    names = ["left", "middle", "right"]
    parts = []
    if button & 64:
        parts.append("wheel-up" if base == 0 else "wheel-down")
    elif button & 32:
        parts.append("motion" if base == 3 else names[base] + "-drag")
    else:
        # In SGR the `m` terminator carries release; base 3 is the X10 spelling.
        parts.append("release" if base == 3 else names[base])
    for bit, name in ((4, "SHIFT"), (8, "ALT"), (16, "CTRL")):
        if button & bit:
            parts.append(name)
    return "+".join(parts)


def capture(seconds: float) -> bytes:
    """Read raw bytes until ENTER or the deadline."""
    deadline = time.monotonic() + seconds
    buf = b""
    while time.monotonic() < deadline:
        ready, _, _ = select.select([sys.stdin], [], [], 0.1)
        if not ready:
            continue
        chunk = os.read(sys.stdin.fileno(), 4096)
        if not chunk:
            break
        if b"\r" in chunk or b"\n" in chunk:
            buf += chunk.replace(b"\r", b"").replace(b"\n", b"")
            break
        buf += chunk
    return buf


def analyse(raw: bytes) -> dict:
    sgr = [
        {"button": int(b), "x": int(x), "y": int(y),
         "kind": "press" if k == b"M" else "release", "decoded": describe(int(b))}
        for b, x, y, k in SGR.findall(raw)
    ]
    x10 = len(X10.findall(raw))
    return {
        "bytes": len(raw),
        "sgr_reports": len(sgr),
        "x10_reports": x10,
        "delivered": bool(sgr or x10),
        "sample": sgr[:6],
        "raw_head": raw[:120].decode("latin-1"),
    }


def main() -> None:
    global _original
    if "--selftest" in sys.argv:
        selftest()
    if "--compare" in sys.argv:
        compare()
    if "--selection" in sys.argv:
        selection()
    if not sys.stdin.isatty():
        sys.exit("run this in a real terminal, not through a pipe")

    identity = {
        "when": datetime.now(timezone.utc).isoformat(),
        "TERM": os.environ.get("TERM"),
        "TERM_PROGRAM": os.environ.get("TERM_PROGRAM"),
        "TERM_PROGRAM_VERSION": os.environ.get("TERM_PROGRAM_VERSION"),
        "COLORTERM": os.environ.get("COLORTERM"),
        "cmux": {k: v for k, v in os.environ.items() if k.startswith("CMUX_")},
        "size": terminal_size(),
    }
    print("Terminal identity")
    for k, v in identity.items():
        print(f"  {k:22} {v}")
    print()
    print("For each mode you will be asked to do six things. After each one press ENTER.")
    print("What matters is whether a modifier-drag is DELIVERED here (capture wins,")
    print("selection is gone) or NOT delivered (the terminal kept it -- that modifier")
    print("is your escape hatch).")
    print()
    print("Which device are you using for THIS run? Use one only -- results are")
    print("written per device, so run it again with the other and nothing is lost.")
    print("  m = mouse (wheel detents, real button-drag)")
    print("  t = trackpad (momentum scroll; a 'drag' may never leave macOS)")
    device = ""
    while device not in ("m", "t"):
        device = input("  device [m/t]: ").strip().lower()[:1]
    identity["device"] = {"m": "mouse", "t": "trackpad"}[device]
    done = [d for d in ("mouse", "trackpad") if out_path(d).exists()]
    if done:
        print(f"\n  (already recorded: {', '.join(done)})")
    print()
    print("Use ONE device for the whole run. Scroll with a deliberate flick if you")
    print("are on a trackpad -- momentum is part of what is being measured.")
    print()
    input("Press ENTER to begin. ")

    _original = termios.tcgetattr(sys.stdin.fileno())
    atexit.register(restore)
    signal.signal(signal.SIGINT, lambda *_: sys.exit(130))

    results = {"identity": identity, "modes": {}}
    try:
        tty.setraw(sys.stdin.fileno())
        for mode_name, seqs in MODES:
            sys.stdout.write("".join(f"\x1b[{s}" for s in seqs))
            sys.stdout.flush()
            per_step = {}
            sys.stdout.write(f"\r\n=== MODE: {mode_name} ===\r\n")
            for key, prompt, secs in STEPS:
                sys.stdout.write(f"  {prompt} ... then ENTER\r\n")
                sys.stdout.flush()
                raw = capture(secs)
                got = analyse(raw)
                per_step[key] = got
                mark = "DELIVERED" if got["delivered"] else "not delivered"
                detail = got["sample"][0]["decoded"] if got["sample"] else ""
                sys.stdout.write(f"     -> {mark:14} {got['sgr_reports']} reports  {detail}\r\n")
                sys.stdout.flush()
            results["modes"][mode_name] = per_step
            sys.stdout.write("".join(f"\x1b[{m}" for m in ALL_OFF))
            sys.stdout.flush()
    finally:
        restore()

    out = out_path(identity["device"])
    out.write_text(json.dumps(results, indent=2) + "\n")

    print(f"\n\nSUMMARY — device: {identity['device']}")
    print("Each cell is `delivered / report-count`.\n")
    head = f"{'mode':32}" + "".join(f"{k:13}" for k, _, _ in STEPS)
    print(head)
    print("-" * len(head))
    for mode_name, steps in results["modes"].items():
        row = f"{mode_name:32}"
        for key, _, _ in STEPS:
            got = steps[key]
            n = got["sgr_reports"] + got["x10_reports"]
            row += f"{('yes' if got['delivered'] else 'no') + '/' + str(n):13}"
        print(row)
    peak = max(
        (s["sgr_reports"], m, k)
        for m, steps in results["modes"].items()
        for k, s in steps.items()
    )
    print(f"\nBusiest single gesture: {peak[0]} reports ({peak[2]}, {peak[1]}).")
    print("That is the burst a forwarded-wheel implementation would have to survive.")
    print(f"\nWritten to {out}")
    other = "trackpad" if identity["device"] == "mouse" else "mouse"
    if out_path(other).exists():
        print(f"Both devices recorded. Compare them with:\n"
              f"  python3 {Path(__file__).name} --compare")
    else:
        print(f"Now run it again with the {other} to complete the pair.")
    print("\nRead it like this: a 'no' on drag_shift or drag_alt means that modifier")
    print("still selects text, and is the escape hatch to document. A 'yes' on")
    print("drag_plain in a mode you would ship means plain selection is gone in it.")


if __name__ == "__main__":
    main()
