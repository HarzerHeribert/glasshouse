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
#   scripts/dev/new-worker.sh <name> <cwd> <packet-path> [--model sonnet] [--effort <level>]
#
# --effort is optional. Omitted, the launch is byte-identical to before this
# flag existed -- no existing caller or test changes behaviour. Given, it must
# be one of the levels the harness itself accepts (`claude --help`): low,
# medium, high, xhigh, max. The orchestrator chooses the level per worker
# tier; this script only carries it.
set -uo pipefail

# The checkout this script lives in, resolved from its own location so the
# paths it hands a worker are right from any worktree. See the prompt below.
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# The TRUE main checkout -- NOT $REPO. Every worktree carries its own copy of
# this script, so BASH_SOURCE-derived $REPO resolves to whichever tree the
# invoked copy happens to live in: running this script's own worktree copy
# against the worktree itself as CWD makes $REPO == $CWD and a same-tree
# guard below would never fire. Measured live during this packet's own
# testing: that exact call actually created a workspace and typed a prompt
# into a real Claude session running IN the main checkout with --permission-
# mode auto, before this fix existed. git's own worktree metadata names the
# one real main checkout regardless of which copy is running -- same
# technique as scripts/reap-worktrees.sh's REPO resolution.
MAIN_COMMON="$(git -C "$REPO" rev-parse --git-common-dir 2>/dev/null)"
case "$MAIN_COMMON" in
  /*) : ;;
  *)  MAIN_COMMON="$(cd "$REPO/$MAIN_COMMON" 2>/dev/null && pwd -P)" ;;
esac
if [ -n "$MAIN_COMMON" ] && [ "$(basename "$MAIN_COMMON")" = ".git" ]; then
  MAIN_CHECKOUT="$(dirname "$MAIN_COMMON")"
else
  MAIN_CHECKOUT="$REPO"
fi

NAME="${1:?worker name}"; CWD="${2:?working directory}"; PACKET="${3:?packet path}"
MODEL="sonnet"
EFFORT=""
PRINT_PROMPT_FLAG=0
shift 3
while [ "$#" -gt 0 ]; do
  case "$1" in
    --model)
      MODEL="${2:-sonnet}"; shift "$(( $# >= 2 ? 2 : 1 ))" ;;
    --effort)
      [ "$#" -ge 2 ] || { echo "new-worker: --effort requires a level"; exit 1; }
      EFFORT="$2"; shift 2 ;;
    --print-prompt)
      PRINT_PROMPT_FLAG=1; shift ;;
    *)
      echo "new-worker: unknown argument: $1"; exit 1 ;;
  esac
done

case "$EFFORT" in
  ""|low|medium|high|xhigh|max) ;;
  *)
    echo "new-worker: unknown --effort level: $EFFORT (expected one of: low, medium, high, xhigh, max)"
    exit 1 ;;
esac

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

ARM="FIRST, before reading anything: arm your continuity watch, or you will run out of context mid-package with nothing to show for it. Run Monitor(command: \"${REPO}/scripts/continuity-watch.sh --role worker\", persistent: true). It finds your session itself; if it prints CONTINUITY NOT ARMED, pass --session with the last path component of your scratchpad directory and run it again. Never \`cd\` inside a shell command: use absolute paths and \`git -C\`. A \`cd\` followed by a read makes the permission classifier stop and ask, and nobody is watching your pane to answer (2026-09-03: three workers sat on that prompt for half an hour). THEN:"
# Two build facts every worker hits and none can discover cheaply: this pane
# inherits ANTHROPIC_BASE_URL from Claude Code, which fails exactly one
# pty_smoke assertion for a reason that is not the tree; and RUSTFLAGS is
# never set in this workspace (CLAUDE.md, *Verification*, rules 1 and 3).
# Said here, once, because a worker that meets the red first and the rule
# never will "fix" the test or re-add the flag.
BUILD_NOTE="Two build facts: (1) if pty_smoke fails on 'overlay's own environment variable leaked', that is ANTHROPIC_BASE_URL inherited from this Claude Code pane, not your change - confirm once with 'env -u ANTHROPIC_BASE_URL cargo test --test pty_smoke', say so in the report, do not alter the test. (2) Never set RUSTFLAGS; warnings come from [workspace.lints.rust]. Use 'scripts/ci-local.sh --scoped' for the fast tier and 'scripts/blast-radius.sh' before reporting."
PROMPT="${ARM} Read ${PACKET} now and follow it exactly. Read ONLY the files its 'READ ONLY THIS' section names - reading more is the documented way workers waste context. Do not commit. Write your report to the path the packet's REPORT TO section gives. ${BUILD_NOTE}"

# Built here, before the --print-prompt short-circuit, so that path can also
# expose the launch command a worker actually receives -- not just the prompt
# it is typed. That is what `--effort` needs proven: the flag composes with
# `--model` regardless of order, and an omitted flag leaves this byte-identical
# to before the flag existed.
LAUNCH="claude --model $MODEL --permission-mode auto --remote-control"
[ -n "$EFFORT" ] && LAUNCH="$LAUNCH --effort $EFFORT"

# `--print-prompt` builds the prompt and the launch command and exits,
# launching nothing.
#
# It exists so `scripts/tests/test_launch_prompts.py` can assert on the prompt
# a worker would ACTUALLY receive rather than on how this file spells it. The
# first version of that test read these lines as text and was fooled by a
# mutation that emptied ARM while leaving the words in a trailing comment —
# SURVIVED, on a check that looked thorough. Assert on the product, not the
# source.
if [ "${PRINT_PROMPT:-}" = 1 ] || [ "$PRINT_PROMPT_FLAG" = 1 ]; then
  printf 'LAUNCH: %s\n' "$LAUNCH"
  printf '%s\n' "$PROMPT"
  exit 0
fi

# Persistent per-worker build cache: a worker's target-dir survives under its
# NAME rather than living inside its worktree, so a second-ever dispatch under
# the same name -- and every rebuild inside one dispatch -- reuses artifacts
# instead of cold-building (today's default: ~10+ min per fresh worktree).
# `close-worker.sh` removes it with the pane (since 2026-09-02: 59 of them
# held 156 GB and the Linux gate leg failed with ENOSPC); a cache that
# outlived its pane is reclaimed with `scripts/reap-worktrees.sh
# --reap-caches`. Only reached past
# the --print-prompt short-circuit above, so a prompt-only invocation stays
# side-effect-free.
#
# CWD_REAL, not CWD: a worker's cwd can be passed with a trailing slash or a
# non-canonical relative form, and comparing that literally against
# $MAIN_CHECKOUT (always canonical, resolved via `cd ... && pwd`) would let a
# spelling difference slip a worker's cache config past this guard.
CWD_REAL="$(cd "$CWD" && pwd)"
if [ "$CWD_REAL" = "$MAIN_CHECKOUT" ]; then
  echo "new-worker: refusing to write a build-cache config into the MAIN checkout ($MAIN_CHECKOUT)"
  exit 1
fi
WORKER_CACHE_DIR="$HOME/.cache/glasshouse-worker-targets/$NAME"
mkdir -p "$WORKER_CACHE_DIR"
mkdir -p "$CWD/.cargo"
# Keep the cache config out of the worktree's untracked view: integrate.sh
# copies every untracked file as a deliverable (ls-files --exclude-standard),
# and without this exclude the config rode into the main checkout on
# 2026-09-01 and silently redirected main builds at a worker's cache.
EXCLUDE_FILE="$(git -C "$CWD" rev-parse --git-path info/exclude)"
case "$EXCLUDE_FILE" in /*) : ;; *) EXCLUDE_FILE="$CWD/$EXCLUDE_FILE" ;; esac
mkdir -p "$(dirname "$EXCLUDE_FILE")"
grep -qx '\.cargo/' "$EXCLUDE_FILE" 2>/dev/null || echo '.cargo/' >> "$EXCLUDE_FILE"
cat > "$CWD/.cargo/config.toml" <<CARGOCFG
# Written by scripts/dev/new-worker.sh. Points this worktree's cargo builds at
# a target-dir that outlives the worktree, keyed by worker NAME ($NAME), so a
# later dispatch under the same name reuses build output instead of cold-
# building. Removing this worktree (scripts/close-worker.sh) does NOT remove
# this cache -- see scripts/reap-worktrees.sh --reap-caches.
[build]
target-dir = "$WORKER_CACHE_DIR"
CARGOCFG
echo "new-worker: build cache -> $WORKER_CACHE_DIR"

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

# 1. start the harness with NO prompt argument.
#
# WAIT FOR THE SHELL FIRST, AND VERIFY WHAT LANDED.
# ------------------------------------------------
# A resolved surface is not a ready shell. The pane is created, then zsh runs
# the user's profile and prints a login banner and a prompt, and text sent
# during that window interleaves with the terminal's own output. Measured
# 2026-08-29: a launch arrived as `e ofclaude --model sonnet ...`, zsh answered
# `command not found: e`, and the packet prompt was then typed at a shell
# prompt instead of into the harness. The delivery proof at the end of this
# script cannot catch that -- the harness never started, so there is no token
# counter to move, and it reports the generic "prompt did not land".
#
# So: wait for a prompt character, send, then READ THE LINE BACK before
# committing to it with Enter. A mangled line is recoverable with ctrl-u; a
# mangled line that has already been executed is a zombie pane.
shell_ready=0
for _ in $(seq 1 20); do
  screen="$(cmux read-screen --surface "$surface" 2>/dev/null)"
  # A prompt, and nothing still being written after it.
  if printf '%s' "$screen" | tail -3 | grep -qE '(❯|\$|%) *$'; then
    shell_ready=1; break
  fi
  sleep 1
done
[ "$shell_ready" -eq 1 ] || echo "new-worker: WARNING — no shell prompt seen on $surface; sending anyway"

landed=0
for attempt in 1 2 3; do
  cmux send --surface "$surface" "$LAUNCH" >/dev/null
  sleep 1
  # Verify the command is on screen INTACT before executing it.
  if cmux read-screen --surface "$surface" 2>/dev/null | grep -qF -- "$LAUNCH"; then
    landed=1; break
  fi
  echo "new-worker: launch line garbled on attempt $attempt (send raced the shell); clearing and retrying"
  cmux send-key --surface "$surface" C-u >/dev/null 2>&1
  sleep 2
done
if [ "$landed" -ne 1 ]; then
  echo "new-worker: could not get a clean launch line onto $surface after 3 attempts." >&2
  echo "  The pane is at a shell prompt; nothing was executed. Send it by hand." >&2
  exit 1
fi
cmux send-key --surface "$surface" Enter >/dev/null

# 2. wait for the TUI to come up
for _ in $(seq 1 30); do
  sleep 1
  cmux read-screen --surface "$surface" 2>/dev/null | grep -q 'auto mode on\|⏵⏵' && break
done

# 3. type the prompt in (built above), then submit
# THE FIRST INSTRUCTION IS THE WATCH, and that is deliberate.
#
# Until 2026-08-29 this prompt did not arm one, and nothing else did either:
# the arming instruction had been added to the ORCHESTRATOR's relaunch prompt
# only, so it reached a session only if a previous session already had a watch
# — a fix that could not bootstrap itself and never reached a worker at all.
# Measured that day: three Opus workers, two hours in, 33% context each, and
# not one watch between them. The orchestrator's own `worker-watch.sh` watches
# the PANE from the orchestrator's session; it reports that a worker went
# quiet, which is exactly what a worker that died of context looks like, and
# it cannot tell the worker anything.
#
# ABSOLUTE, not relative. The documented recipe was
# `.agent-runtime/continuity-watch.sh`, and `.agent-runtime/` exists only in
# the main checkout — so it resolved in 1 of 64 worktrees and failed with exit
# 127 in the other 63, while the pane looked armed. $REPO is the checkout this
# script lives in, so the path is right from any worktree.
#
# `scripts/tests/test_launch_prompts.py` fails the gate if this line loses the
# instruction again.
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

  # Record the dispatch so `pipeline.sh` can count a worker that has no worktree.
  # A read-only recon runs in the main checkout by design, so worktree-counting
  # reported it as never dispatched and the board nag fired at an orchestrator
  # who had just dispatched it. Written only here, after delivery is proven, so
  # the marker means "this really started" rather than "this was attempted".
  mkdir -p "$REPO/.agent-runtime/dispatched"
  printf '%s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$ws" "$surface" \
    > "$REPO/.agent-runtime/dispatched/$NAME"
  printf '  arm the watch:  scripts/worker-watch.sh %s %s <report-path>\n' "$NAME" "$surface"
  exit 0
fi

# Zero tokens has TWO causes and they need opposite responses. Re-sending into
# the second one queues a duplicate prompt behind a turn that is about to
# succeed, and the worker then runs its packet twice. 2026-09-03's provider 529
# outage produced the false negative three times in one session, and the
# documented "just send Enter" remedy made it worse each time.
screen="$(cmux read-screen --surface "$surface" 2>/dev/null)"
if grep -qE 'API Error: 5[0-9][0-9]|Retrying in [0-9]+s · attempt|attempt [0-9]+/[0-9]+' <<<"$screen"; then
  printf '\033[33mnew-worker: %s LANDED the prompt but the provider is refusing it\033[0m — 0 tokens, and the pane shows an API error or a retry ladder.\n' "$NAME"
  printf '  DO NOT re-send: the prompt is the active turn, and a second copy queues behind it.\n'
  printf '  Wait for the ladder to finish. Re-submit only once the pane is idle with an EMPTY input box,\n'
  printf '  and press Escape first to drop anything already queued.\n'
  printf '  Recover the exact text with:  PRINT_PROMPT=1 %s <same args>\n' "$0"
  exit 2
fi

printf '\033[31mnew-worker: %s DID NOT ACCEPT the prompt\033[0m — still at 0 tokens, and the pane shows no API error.\n' "$NAME"
printf '  This is the documented failure: the harness started but the prompt did not land.\n'
printf '  Re-send it by hand:  cmux send --surface %s "<prompt>" && cmux send-key --surface %s Enter\n' "$surface" "$surface"
printf '  (cmux send takes its text POSITIONALLY; there is no --text flag.)\n'
exit 1
