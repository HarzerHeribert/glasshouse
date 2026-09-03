#!/usr/bin/env bash
# scripts/prompt-watch.sh -- arm with Monitor(command: "scripts/prompt-watch.sh", persistent: true)
# A pane waiting on "Do you want to proceed?" moves no tokens, so every other
# watch reads it as thinking -- three workers sat that way for half an hour on
# 2026-09-03. This watch reads every pane and PRESSES ENTER FOR IT: user ruling
# the same night, "I want you to work unattended so this is a blocker -- make
# sure everyone has the permissions or just click accept using cmux". The
# PreToolUse hooks (guard-worktree-boundary, guard-destructive-git) still run
# on the approved command, so approval widens nothing they bound. The one
# prompt it leaves for a human is a command naming the provider-keys file --
# the thing the Read deny rule exists to protect.
#   PROMPT_WATCH_APPROVE=0        report only, never press Enter
#   PROMPT_WATCH_SELF=workspace:N skip that pane (the default watches every pane,
#                                 the orchestrator's included -- it prompts too)
#   PROMPT_WATCH_INTERVAL=20      seconds between sweeps
# Report-only mode names each prompt once (the orchestrator answers it by hand,
# and a screen keeps showing the prompt for a sweep after Enter).
declare -A seen
while true; do
  for ws in $(cmux workspace list 2>/dev/null | /usr/bin/grep -oE 'workspace:[0-9]+'); do
    [ "$ws" = "${PROMPT_WATCH_SELF:-none}" ] && continue
    screen=$(cmux read-screen --workspace "$ws" 2>/dev/null | tail -n 16)
    if echo "$screen" | /usr/bin/grep -q "Do you want to proceed?"; then
      cmd=$(echo "$screen" | /usr/bin/grep -m1 -oE '[│$] .{0,120}')
      if echo "$screen" | /usr/bin/grep -q -E 'provider-keys|\.env\b'; then
        echo "PROMPT $ws names the secrets file and is left for a human: $cmd"
      elif [ "${PROMPT_WATCH_APPROVE:-1}" = "1" ]; then
        cmux send-key --workspace "$ws" Enter >/dev/null 2>&1
        echo "APPROVED $ws permission prompt: $cmd"
      else
        key="$ws:$(printf '%s' "$cmd" | md5 | cut -c1-8)"
        if [ -z "${seen[$key]:-}" ]; then
          seen[$key]=1
          echo "PROMPT $ws is waiting on a permission prompt: $cmd -- approve with: cmux send-key --workspace $ws Enter"
        fi
      fi
    fi
  done
  sleep "${PROMPT_WATCH_INTERVAL:-20}"
done
