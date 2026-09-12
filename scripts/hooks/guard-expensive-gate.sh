#!/usr/bin/env bash
# PreToolUse guard: make a direct full blast-radius sweep explicit.
#
# Worker packets already require `blast-radius.sh --targeted`, but a stale
# launch reminder caused workers to run the bare full sweep after minor edits.
# The full sweep includes process/PTY families and belongs once per integration
# wave. This guard blocks only direct Bash invocations; integrate.sh and
# ci-local.sh remain free to run the tier they explicitly select.
set -uo pipefail

payload="$(cat)"
tool="$(printf '%s' "$payload" | /usr/bin/python3 -c \
  'import json,sys;print(json.load(sys.stdin).get("tool_name",""))' 2>/dev/null || true)"
[ "$tool" = "Bash" ] || exit 0

command="$(printf '%s' "$payload" | /usr/bin/python3 -c \
  'import json,sys;print(json.load(sys.stdin).get("tool_input",{}).get("command",""))' 2>/dev/null || true)"
[ -n "$command" ] || exit 0

decision="$(printf '%s' "$command" | /usr/bin/python3 -c '
import re, shlex, sys

command = sys.stdin.read()
for segment in re.split(r"(?:\|\||&&|;|\||&|\n)", command):
    try:
        tokens = shlex.split(segment)
    except ValueError:
        tokens = segment.split()
    for index, token in enumerate(tokens):
        if token.endswith("blast-radius.sh"):
            args = tokens[index + 1:]
            read_only = {"--status", "--dry-run", "--list"}
            explicit_full = {"--full", "--serial"}
            if "--targeted" in args and not read_only.intersection(args):
                if not any(arg.endswith(".rs") for arg in args):
                    print("targeted-needs-files")
                    raise SystemExit
            elif not read_only.intersection(args) and not explicit_full.intersection(args):
                print("full-needs-opt-in")
                raise SystemExit
' 2>/dev/null || true)"

[ -n "$decision" ] || exit 0
if [ "$decision" = "targeted-needs-files" ]; then
  cat >&2 <<'EOF'
BLOCKED by scripts/hooks/guard-expensive-gate.sh

`scripts/blast-radius.sh --targeted` without filenames means every dirty Rust
file in this checkout. That silently pulled unrelated work into this worker's
gate.

Pass this worker's files explicitly, as its packet requires:

    scripts/blast-radius.sh --targeted <every .rs file this worker changed>

During the edit loop, run the specific cargo test first. The targeted command
above is the worker's final check.
EOF
  exit 2
fi
cat >&2 <<'EOF'
BLOCKED by scripts/hooks/guard-expensive-gate.sh

The bare `scripts/blast-radius.sh` is the full transitive sweep. It runs the
process- and PTY-sensitive families serially and belongs once per integration
wave, not after each worker edit.

Use the worker-loop gate named by the task packet:

    scripts/blast-radius.sh --targeted <every changed .rs file>

For a deliberate integration/pre-push sweep, make that cost explicit:

    scripts/blast-radius.sh --full

For the tight edit loop, run the specific cargo test first; the targeted gate
is the final worker check. The full sweep discovers transitive consumers and
runs their tests, but it does not decide which test files a worker may edit.
EOF
exit 2
