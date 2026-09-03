#!/usr/bin/env bash
# scripts/prompt-watch.sh -- arm with Monitor(command: "PROMPT_WATCH_SELF=workspace:<yours> scripts/prompt-watch.sh", persistent: true)
# Emit one line per worker pane that is sitting on a permission prompt. The
# worker watches read token movement, and a pane waiting for "Do you want to
# proceed?" moves nothing -- three workers sat that way for half an hour.
declare -A seen
while true; do
  for ws in $(cmux workspace list 2>/dev/null | /usr/bin/grep -oE 'workspace:[0-9]+'); do
    [ "$ws" = "${PROMPT_WATCH_SELF:-none}" ] && continue   # the orchestrator's own pane: PROMPT_WATCH_SELF=workspace:N
    screen=$(cmux read-screen --workspace "$ws" 2>/dev/null | tail -n 12)
    if echo "$screen" | /usr/bin/grep -q "Do you want to proceed?"; then
      key="$ws:$(echo "$screen" | /usr/bin/grep -m1 -oE '\$ .{0,60}' | md5 | cut -c1-8)"
      if [ -z "${seen[$key]:-}" ]; then
        seen[$key]=1
        echo "PROMPT $ws is waiting on a permission prompt: $(echo "$screen" | /usr/bin/grep -m1 -oE '\$ .{0,100}') -- approve with: cmux send-key --workspace $ws Enter"
      fi
    fi
  done
  sleep 60
done
