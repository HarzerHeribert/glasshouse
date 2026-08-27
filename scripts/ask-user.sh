#!/usr/bin/env bash
# Park a question with the user WITHOUT the orchestrator stopping to wait.
#
# WHY THIS EXISTS
# ---------------
# An orchestrator idling because it needs a decision is the fleet stopped. Most
# decisions do not deserve that: they block one package, not the project, and
# the answer is just as good in twenty minutes as now.
#
# So the question goes to a cheap Haiku session in its own pane. It asks the
# user, waits as long as it takes, and writes the answer to a file. The
# orchestrator keeps working and picks the answer up when it lands.
#
# **Only stop for a question that blocks the PROJECT.** If it blocks only the
# package in hand, park it here and dispatch something else.
#
# USAGE
#   scripts/ask-user.sh <slug> "<question>" "<option>" ["<option>" ...]
#   scripts/ask-user.sh --check <slug>      # answered yet? prints it if so
#   scripts/ask-user.sh --list              # everything outstanding
#   scripts/ask-user.sh --watch             # notify when any answer lands
#
# The answer lands in .agent-runtime/answers/<slug>.txt
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANS_DIR="$REPO/.agent-runtime/answers"
mkdir -p "$ANS_DIR"

case "${1:-}" in
  --watch)
    # THE DEFECT THIS FIXES
    # ---------------------
    # The first version of this script parked a question and wrote the answer to
    # a file, and **nothing told the orchestrator the answer had arrived**. A
    # question was answered two minutes after it was asked and sat unread for an
    # hour; the orchestrator only found it because the user said so. It had
    # built a way to park a question with no way to be told it was answered —
    # the same gap `worker-watch.sh` exists to close for reports.
    #
    # Drive it with Monitor, persistent, ONE per orchestrator:
    #   Monitor(command: "scripts/ask-user.sh --watch", persistent: true)
    seen_dir="$ANS_DIR/.seen"
    mkdir -p "$seen_dir"
    # Anything already answered before the watch started is not news.
    for a in "$ANS_DIR"/*.txt; do
      [ -e "$a" ] || continue
      touch "$seen_dir/$(basename "$a" .txt)"
    done
    while true; do
      sleep 30
      for a in "$ANS_DIR"/*.txt; do
        [ -e "$a" ] || continue
        slug="$(basename "$a" .txt)"
        [ -e "$seen_dir/$slug" ] && continue
        touch "$seen_dir/$slug"
        echo "ANSWER RECEIVED for '$slug': $(head -c 300 "$a" | tr '\n' ' ')"
      done
    done
    ;;
  --list)
    shopt -s nullglob
    open=("$ANS_DIR"/*.question)
    if [ ${#open[@]} -eq 0 ]; then echo "no questions outstanding"; exit 0; fi
    for q in "${open[@]}"; do
      slug="$(basename "$q" .question)"
      if [ -f "$ANS_DIR/$slug.txt" ]; then
        echo "  ANSWERED  $slug -> $(head -1 "$ANS_DIR/$slug.txt")"
      else
        echo "  waiting   $slug: $(head -1 "$q")"
      fi
    done
    exit 0;;
  --check)
    slug="${2:?usage: --check <slug>}"
    if [ -f "$ANS_DIR/$slug.txt" ]; then cat "$ANS_DIR/$slug.txt"; exit 0; fi
    echo "not answered yet"; exit 1;;
esac

SLUG="${1:?usage: ask-user.sh <slug> \"<question>\" \"<option>\" ...}"
QUESTION="${2:?missing question}"
shift 2
OPTIONS=("$@")
[ ${#OPTIONS[@]} -ge 2 ] || { echo "give at least two options" >&2; exit 2; }

# The slug becomes a filename, so it must actually be one. Passing the question
# text here — easy to do, since both are prose — used to produce three
# "File name too long" errors and then a cheerful line saying the answer would
# appear at a path that could not be created. A question this script says it
# parked must really be parked.
if ! printf '%s' "$SLUG" | grep -Eq '^[a-z0-9][a-z0-9-]{0,47}$'; then
  echo "ask-user.sh: '$SLUG' is not a slug." >&2
  echo "  A slug is lower-case letters, digits and hyphens, 48 characters or fewer." >&2
  echo "  The QUESTION is the second argument, not the first:" >&2
  echo "    ask-user.sh phase0-box2 \"Which reading do you want?\" \"option a\" \"option b\"" >&2
  exit 2
fi

ANSWER_FILE="$ANS_DIR/$SLUG.txt"
rm -f "$ANSWER_FILE"
if ! { printf '%s\n' "$QUESTION" > "$ANS_DIR/$SLUG.question" &&
       printf '%s\n' "${OPTIONS[@]}" >> "$ANS_DIR/$SLUG.question"; }; then
  echo "ask-user.sh: could not write $ANS_DIR/$SLUG.question — question NOT parked." >&2
  exit 1
fi

# Options are passed through a file, never interpolated into the prompt string:
# a question containing a quote would otherwise rewrite the command.
OPTS_RENDERED="$(printf -- '- %s\n' "${OPTIONS[@]}")"

PROMPT="You have exactly one job and it is not coding.

Ask the user this question using the AskUserQuestion tool, with these options,
worded exactly as given:

QUESTION: ${QUESTION}

OPTIONS:
${OPTS_RENDERED}

Then write ONLY their chosen answer — the option text, plus any note they added
— to ${ANSWER_FILE}, and say one sentence confirming it was recorded.

Do not edit any other file. Do not explore the repository. Do not offer an
opinion on the question unless the user asks you for one. If the user asks a
clarifying question you cannot answer from this prompt, write what they said to
${ANSWER_FILE} instead and say so."

CMUX_QUIET=1 cmux new-workspace --name "ask: $SLUG" \
  --cwd "$REPO" \
  --command "claude --model haiku --permission-mode auto --remote-control ask-$SLUG '$(printf '%s' "$PROMPT" | sed "s/'/'\\\\''/g")'" \
  >/dev/null 2>&1 \
  && echo "parked question '$SLUG' with a Haiku session; answer will appear at $ANSWER_FILE" \
  || { echo "failed to open the asking pane" >&2; exit 1; }
