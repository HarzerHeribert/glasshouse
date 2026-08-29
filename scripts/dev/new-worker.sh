#!/usr/bin/env bash
# Launch a visible worker and PROVE the prompt actually landed.
#
# WHY THIS EXISTS
# ---------------
# Dispatch had no delivery confirmation; it was fire-and-hope. Measured: three
# workers were launched with their prompt passed as a command-line argument,
# `claude` started in the right worktree, and all three then sat at an EMPTY
# PROMPT — 0/1M tokens, $0.00 — for five minutes. The prompt never arrived.
# Worse, `worker-watch.sh` reported "pane went quiet, NO report" and the
# orchestrator dismissed it three times as the known false-idle-on-startup case.
#
# Two separate bugs, both fixed here:
#
#  1. THE PROMPT MUST BE TYPED INTO THE RUNNING TUI, not passed as argv. Sending
#     it as an argument through `cmux send` did not survive; typing it into the
#     started session does. This script starts the harness, waits for it, then
#     sends the prompt as text.
#
#  2. `cmux identify --workspace <ws>` DOES NOT ANSWER "that workspace's
#     surface". It reports the APP's focused surface, whatever workspace that is
#     in. Using it to address a new pane sent a launch command into the user's
#     own orchestrator pane. `cmux new-pane` returns the surface ref directly and
#     is the only trustworthy source; this script uses that, or selects the
#     workspace first and reads the focused surface back.
#
# It then polls the pane until the token counter moves, and FAILS LOUDLY if the
# prompt did not take, instead of leaving a silent idle worker burning nothing.
#
# USAGE
#   scripts/dev/new-worker.sh <name> <cwd> <packet-path> [--model sonnet]
set -uo pipefail

NAME="${1:?worker name}"; CWD="${2:?working directory}"; PACKET="${3:?packet path}"
MODEL="sonnet"; [ "${4:-}" = "--model" ] && MODEL="${5:-sonnet}"

[ -d "$CWD" ]    || { echo "new-worker: $CWD does not exist"; exit 1; }
[ -f "$PACKET" ] || { echo "new-worker: packet $PACKET does not exist"; exit 1; }

# The prompt is typed into a harness whose cwd is the WORKER'S WORKTREE, and
# `.agent-runtime/` is gitignored -- so it does not exist there at all. A
# relative packet path therefore resolves to nothing on the other side, and the
# worker stops on its first turn asking where its packet is. Measured
# 2026-08-29: all three editing workers of batch 47 hit this at once; two of
# them burned ~100k tokens exploring before the correction arrived. The
# delivery proof below cannot catch it, because the prompt *did* land -- it was
# the path inside the prompt that was unusable. Resolve it here, once.
PACKET="$(cd "$(dirname "$PACKET")" && pwd)/$(basename "$PACKET")"

ws="$(cmux workspace create --name "$NAME" --cwd "$CWD" 2>&1 | grep -oE 'workspace:[0-9]+' | head -1)"
[ -n "$ws" ] || { echo "new-worker: could not create a workspace"; exit 1; }
cmux workspace select "$ws" >/dev/null 2>&1
# Focus does not switch synchronously, and `identify` reports the APP's focused
# surface -- so a single read races and can hand back the PREVIOUS workspace's
# surface. That is the bug that typed a launch command into the user's own pane.
# Poll until the focused surface actually belongs to the workspace we created,
# and never accept one that does not.
surface=""
for _ in $(seq 1 15); do
  sleep 1
  surface="$(cmux identify 2>/dev/null | python3 -c 'import sys,json;d=json.load(sys.stdin);f=d["focused"];print(f["surface_ref"] if f["workspace_ref"]=="'"$ws"'" else "")' 2>/dev/null)"
  [ -n "$surface" ] && break
  cmux workspace select "$ws" >/dev/null 2>&1
done
[ -n "$surface" ] || { echo "new-worker: could not resolve a surface for $ws (focus never moved)"; exit 1; }

echo "new-worker: $NAME -> $ws $surface  (cwd $CWD)"

# 1. start the harness with NO prompt argument
cmux send --surface "$surface" "claude --model $MODEL --permission-mode auto --remote-control" >/dev/null
cmux send-key --surface "$surface" Enter >/dev/null

# 2. wait for the TUI to come up
for _ in $(seq 1 30); do
  sleep 1
  cmux read-screen --surface "$surface" 2>/dev/null | grep -q 'auto mode on\|⏵⏵' && break
done

# 3. type the prompt in, then submit
PROMPT="Read ${PACKET} now and follow it exactly. Read ONLY the files its 'READ ONLY THIS' section names - reading more is the documented way workers waste context. Do not commit. Write your report to the path the packet's REPORT TO section gives."
cmux send --surface "$surface" "$PROMPT" >/dev/null
sleep 1
cmux send-key --surface "$surface" Enter >/dev/null

# 4. PROVE it landed: the token counter must move off zero.
delivered=0
for _ in $(seq 1 20); do
  sleep 2
  screen="$(cmux read-screen --surface "$surface" 2>/dev/null)"
  # status line looks like: `  ██▏░░░ 5% · 52k/1M    5m · ~$0.21`
  if ! grep -qE '0% · 0/1M' <<<"$screen" && grep -qE '·[[:space:]]*[0-9]+(\.[0-9]+)?k?/1M' <<<"$screen"; then
    delivered=1; break
  fi
done

if [ "$delivered" -eq 1 ]; then
  printf '\033[32mnew-worker: %s ACCEPTED the prompt\033[0m — %s %s\n' "$NAME" "$ws" "$surface"
  printf '  arm the watch:  scripts/worker-watch.sh %s %s <report-path>\n' "$NAME" "$surface"
  exit 0
fi

printf '\033[31mnew-worker: %s DID NOT ACCEPT the prompt\033[0m — still at 0 tokens.\n' "$NAME"
printf '  This is the documented failure: the harness started but the prompt did not land.\n'
printf '  Re-send it by hand:  cmux send --surface %s "<prompt>" && cmux send-key --surface %s Enter\n' "$surface" "$surface"
exit 1
