#!/usr/bin/env python3
"""The tag after the given one: `v0.1.0-pre.3` -> `v0.1.0-pre.4`, and a
plain `v0.1.0` -> `v0.1.1-pre.1`. No argument (no tag yet) -> `v0.1.0-pre.1`."""
import re
import sys

last = sys.argv[1] if len(sys.argv) > 1 and sys.argv[1] else ""
if not last:
    print("v0.1.0-pre.1")
elif m := re.fullmatch(r"v(\d+)\.(\d+)\.(\d+)-pre\.(\d+)", last):
    major, minor, patch, pre = map(int, m.groups())
    print(f"v{major}.{minor}.{patch}-pre.{pre + 1}")
elif m := re.fullmatch(r"v(\d+)\.(\d+)\.(\d+)", last):
    major, minor, patch = map(int, m.groups())
    print(f"v{major}.{minor}.{patch + 1}-pre.1")
else:
    sys.exit(f"cannot read a version from tag {last!r}")
