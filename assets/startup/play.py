#!/usr/bin/env python3
"""Play the startup frame bank in a real terminal.

This is the proof that the asset works, and the reference implementation of the
playback rules the Rust player must follow: centre, centre-crop when the
terminal is smaller than the art, one accent colour in three intensities, and
stop on demand rather than at the end of a loop.

  ./assets/startup/play.py                      # loop until ctrl-c
  ./assets/startup/play.py --loops 2            # two passes, then exit
  ./assets/startup/play.py --accent '#dfff00'   # any theme accent
  ./assets/startup/play.py --plain              # no colour, no alt screen
"""
import argparse
import pathlib
import shutil
import signal
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
DEFAULT_BANK = HERE / "glasshouse.frames"
# Below this the art cannot be cropped into anything readable; the caller
# should print a line of text instead of a picture.
MIN_COLS, MIN_ROWS = 40, 12


def load(path):
    """Return (meta, frames) where a frame is a list of `rows` padded strings."""
    meta, tiers, frames, cur = {}, {}, [], None
    for line in path.read_text().splitlines():
        if cur is None and (line.startswith("#") or not line):
            continue
        if line.startswith("%"):
            key, _, rest = line[1:].partition(" ")
            if key == "tier":
                name, _, glyphs = rest.strip().partition(" ")
                tiers[name] = glyphs.strip()
            else:
                meta[key] = rest.strip()
            continue
        if line.startswith("@"):
            cur = []
            frames.append(cur)
            continue
        if cur is not None:
            cur.append(line)
    cols, rows = (int(v) for v in meta["grid"].split())
    frames = [[(f[r] if r < len(f) else "").ljust(cols)[:cols] for r in range(rows)] for f in frames]
    meta["cols"], meta["rows"], meta["tiers"] = cols, rows, tiers
    return meta, frames


def shade(hex_colour, mul=1.0, toward_white=0.0):
    n = int(hex_colour.lstrip("#"), 16)
    out = []
    for shift in (16, 8, 0):
        c = ((n >> shift) & 255) * mul
        c = c + (255 - c) * toward_white
        out.append(max(0, min(255, int(round(c)))))
    return "\x1b[38;2;{};{};{}m".format(*out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("bank", nargs="?", type=pathlib.Path, default=DEFAULT_BANK)
    ap.add_argument("--loops", type=int, default=0, help="0 = until interrupted")
    ap.add_argument("--accent", default="#dfff00")
    ap.add_argument("--interval", type=int, default=0, help="ms per frame; 0 = the bank's own")
    ap.add_argument("--plain", action="store_true")
    args = ap.parse_args()

    meta, frames = load(args.bank)
    cols, rows, tiers = meta["cols"], meta["rows"], meta["tiers"]
    interval = (args.interval or int(meta["interval"])) / 1000.0

    colour = {}
    if not args.plain:
        for glyph in tiers.get("quiet", ""):
            colour[glyph] = shade(args.accent, mul=0.40)
        for glyph in tiers.get("soft", ""):
            colour[glyph] = shade(args.accent)
        for glyph in tiers.get("bright", ""):
            colour[glyph] = "\x1b[1m" + shade(args.accent, toward_white=0.45)
    reset = "" if args.plain else "\x1b[0m"

    w = shutil.get_terminal_size((80, 24))
    if w.columns < MIN_COLS or w.lines < MIN_ROWS:
        print("glasshouse — starting…")
        return 0

    # Crop, never scale: the art is composed so that the centre survives.
    vis_cols, vis_rows = min(cols, w.columns), min(rows, w.lines - 1)
    cut_x, cut_y = (cols - vis_cols) // 2, (rows - vis_rows) // 2
    pad_x = (w.columns - vis_cols) // 2
    pad_y = max(0, (w.lines - 1 - vis_rows) // 2)

    out = sys.stdout
    tty = out.isatty()
    alt = not args.plain and tty
    stop = {"now": False}
    signal.signal(signal.SIGINT, lambda *_: stop.__setitem__("now", True))

    if alt:
        out.write("\x1b[?1049h\x1b[?25l")
    try:
        loops = 0
        while not stop["now"] and (args.loops == 0 or loops < args.loops):
            for frame in frames:
                if stop["now"]:
                    break
                buf = ["\x1b[H\x1b[2J" if alt else ("\x1b[H" if tty else "")]
                buf.append("\n" * pad_y)
                for r in range(vis_rows):
                    line = frame[cut_y + r][cut_x : cut_x + vis_cols]
                    buf.append(" " * pad_x)
                    if args.plain:
                        buf.append(line.rstrip())
                    else:
                        run, last = [], None
                        for ch in line:
                            c = colour.get(ch)
                            if c != last:
                                run.append(reset if c is None else c)
                                last = c
                            run.append(ch)
                        buf.append("".join(run).rstrip() + reset)
                    buf.append("\n")
                out.write("".join(buf))
                out.flush()
                time.sleep(interval)
            loops += 1
    except BrokenPipeError:
        # Piped into `head`, which is a normal way to look at one frame.
        return 0
    finally:
        if alt:
            try:
                out.write("\x1b[?25h\x1b[?1049l")
                out.flush()
            except BrokenPipeError:
                pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
