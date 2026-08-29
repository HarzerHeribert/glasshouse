#!/usr/bin/env bash
# Emit a task-packet skeleton that passes scripts/validate_round.py with no
# hand editing.
#
# WHY THIS EXISTS
# ----------------
# validate_round.py refused three hand-written packets in a row this session,
# every time on format alone (an empty YOURS block, a missing FEASIBILITY
# block, then a FEASIBILITY heading that shadowed the one-line "not
# applicable" exemption below it). None of those refusals were about
# substance — each cost a full edit-and-revalidate cycle of orchestrator
# context, which is the scarcest thing in this process. A skeleton that is
# valid by construction removes that loop.
#
# It also owes the worker its own scoping (CLAUDE.md's rule, practice §52):
# nine packets in this project's history opened with "read CLAUDE.md and the
# files it names," which hands a Sonnet running a four-box package the
# orchestrator's entire ~175k-token reading list. The generated READ ONLY
# THIS section is the fix.
#
# Usage:
#   scripts/new-packet.sh <name> [--recon] [--lines N,M,...] [--worktree] [--force]
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: scripts/new-packet.sh <name> [--recon] [--lines N,M,...] [--worktree] [--force]

Emits .agent-runtime/packet-<name>.md, pre-filled so that

    python3 scripts/validate_round.py .agent-runtime/packet-<name>.md

passes with no hand editing. Run from a checkout of this repository; the
capability map is read relative to the current directory, matching
validate_round.py's own default.

  --recon      emit the read-only recon variant: FEASIBILITY: not
               applicable on one line, and a YOURS block naming only its
               own report.
  --lines N,M  quote these docs/product/capability-map.md line numbers
               verbatim and unwrapped (practice §49). Each must be a
               currently-☐-or-☑ box line, or the tool fails loudly.
  --worktree   print (do not run) the exact `git worktree add` command for
               this packet, with an absolute path under .worktrees/.
  --force      overwrite an existing packet.
EOF
}

NAME=""
RECON=false
LINES=""
PRINT_WORKTREE=false
FORCE=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --recon) RECON=true; shift ;;
    --lines)
      if [[ $# -lt 2 ]]; then
        echo "new-packet.sh: --lines needs an argument" >&2
        exit 2
      fi
      LINES="$2"; shift 2 ;;
    --worktree) PRINT_WORKTREE=true; shift ;;
    --force) FORCE=true; shift ;;
    -h|--help) usage; exit 0 ;;
    -*)
      echo "new-packet.sh: unknown flag: $1" >&2
      usage
      exit 2
      ;;
    *)
      if [[ -n "$NAME" ]]; then
        echo "new-packet.sh: unexpected extra argument: $1" >&2
        exit 2
      fi
      NAME="$1"; shift ;;
  esac
done

if [[ -z "$NAME" ]]; then
  echo "new-packet.sh: <name> is required" >&2
  usage
  exit 2
fi

if [[ ! "$NAME" =~ ^[a-z0-9][a-z0-9-]*$ ]]; then
  echo "new-packet.sh: <name> must be lowercase kebab-case (letters, digits, hyphens): got '$NAME'" >&2
  exit 2
fi

MAP="docs/product/capability-map.md"
OUT_DIR=".agent-runtime"
OUT="$OUT_DIR/packet-$NAME.md"
# ABSOLUTE, and for the same reason the packet path is: an editing worker's
# cwd is its worktree, where `.agent-runtime/` is gitignored and absent. A
# relative report path sends the report into the worktree, where the watch --
# armed on the main checkout's path -- can never see it, and the worker looks
# like it finished with no report. Both halves of that bug were paid for in
# batch 47.
REPORT_REL="$(pwd)/$OUT_DIR/report-$NAME.md"
TASK_ID="GH-$(printf '%s' "$NAME" | tr '[:lower:]' '[:upper:]')"
BRANCH="claude/$NAME"

if [[ -e "$OUT" && "$FORCE" != true ]]; then
  echo "new-packet.sh: $OUT already exists — pass --force to overwrite" >&2
  exit 1
fi

# Anchor the worktree path on the main checkout, not the caller's worktree —
# the same reasoning coedit.sh uses: git worktree add -C with a relative
# path creates the worktree INSIDE the repo (a trap this project has hit
# before). --git-common-dir is the shared .git of the whole worktree family;
# its parent is the main checkout from anywhere.
MAIN_ROOT="$(cd "$(git rev-parse --git-common-dir)/.." && pwd)"
WORKTREE_ABS="$MAIN_ROOT/.worktrees/$NAME"

# --- --lines: resolve and validate every box line before writing anything --
BOX_LINES_MD=""
if [[ -n "$LINES" ]]; then
  if [[ ! -f "$MAP" ]]; then
    echo "new-packet.sh: $MAP not found — run this from a checkout of the repository" >&2
    exit 1
  fi
  IFS=',' read -r -a LINE_NUMS <<<"$LINES"
  for n in "${LINE_NUMS[@]}"; do
    n="$(printf '%s' "$n" | tr -d '[:space:]')"
    if [[ ! "$n" =~ ^[0-9]+$ ]]; then
      echo "new-packet.sh: --lines value '$n' is not a line number" >&2
      exit 1
    fi
    line="$(sed -n "${n}p" "$MAP")"
    if [[ -z "$line" ]]; then
      echo "new-packet.sh: $MAP has no line $n" >&2
      exit 1
    fi
    if [[ "$line" =~ ^(☐|☑)(.*)$ ]]; then
      marker="${BASH_REMATCH[1]}"
      text="${BASH_REMATCH[2]}"
    else
      echo "new-packet.sh: $MAP:$n is not a ☐/☑ box line — refusing to quote it:" >&2
      echo "    $line" >&2
      exit 1
    fi
    text="$(printf '%s' "$text" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
    BOX_LINES_MD="${BOX_LINES_MD}    - **${n}** ${marker} ${text}
"
  done
fi

# --- render ------------------------------------------------------------
mkdir -p "$OUT_DIR"

{
  printf '# TASK PACKET — %s\n\n' "$TASK_ID"
  printf 'TASK ID: %s\n' "$TASK_ID"
  printf 'ROLE / MODEL: TODO — e.g. "Sonnet implementer", "Ox leaf", "team lead" (docs/process/worker-capabilities.md)\n'
  printf 'CAPABILITY: TODO — this worker'"'"'s capability tier (docs/process/worker-capabilities.md)\n'
  printf 'BEHAVIORAL CONTRACT: TODO — which behavioral contract this worker operates under (docs/process/worker-capabilities.md)\n'
  printf 'WORKTREE: `%s` (branch `%s`)\n\n' "$WORKTREE_ABS" "$BRANCH"

  cat <<'EOF'
## READ ONLY THIS

1. `CLAUDE.md`
2. this packet
3. `docs/process/worker-capabilities.md` — what its tier may and may not decide
4. TODO: practice §§ by number — from `docs/process/orchestration-practice.md`. **Do not read the whole file.**

EOF

  if $RECON; then
    printf '## FEASIBILITY: not applicable -- this is a read-only recon package; it investigates and reports, wires no mechanism, and closes no capability-map box.\n\n'
  else
    cat <<'EOF'
## FEASIBILITY

TODO — Phase -1 (docs/process/assurance-economics.md). validate_round.py's
check 6 refuses this round until every row below names something findable in
current production code: the producing type/symbol, the caller field or
state that holds it, the propagation path, and the consumer that observes
it. Do not invent a plausible-looking link — an unresolved row stays blank,
not guessed.

- Producer:
- Caller:
- Propagation:
- Consumer:

EOF
  fi

  printf '## OBJECTIVE\n\nTODO: describe the work.\n\n'

  if [[ -n "$BOX_LINES_MD" ]]; then
    printf '## BOX LINES\n\n'
    printf '%s' "$BOX_LINES_MD"
    printf '\n'
  fi

  printf '## EXPECTED FILES\n\n**YOURS**\n\n'
  printf '    %s (new)\n' "$REPORT_REL"
  if ! $RECON; then
    printf '\nTODO: add this package'"'"'s real target files above the report line, each on its own indented line.\n'
  fi
  printf '\n'

  printf '## FORBIDDEN FILES\n\n'
  if $RECON; then
    printf '    crates/**   do not touch product code — recon is read-only\n'
    printf '    docs/**     do not touch docs — recon is read-only\n'
  else
    printf '    docs/product/capability-map.md   never edit the map\n'
    printf '    docs/process/handoff.md          never edit the handoff\n'
    printf '    docs/product/evidence/**         never edit the evidence ledger\n'
    printf '    scripts/validate_round.py        read it, do not change it\n'
    printf 'TODO: name this round'"'"'s other live workers'"'"' files here.\n'
  fi
  printf '\n'

  if $RECON; then
    printf '## REQUIRED BEHAVIOR\n\nRead-only. Investigate and report; do not edit any file.\n\n'
    printf '## SECURITY / ISOLATION INVARIANTS\n\nNone beyond read-only — no writes outside the report path.\n\n'
    printf '## CROSS-PLATFORM REQUIREMENTS\n\nN/A — investigation only, no build or run.\n\n'
    printf '## ACCEPTANCE TESTS\n\nThe report answers what this packet asked, with a file:line citation for every claim.\n\n'
    printf '## VERIFICATION COMMANDS\n\nNone — recon runs no build or test.\n\n'
  else
    printf '## REQUIRED BEHAVIOR\n\nTODO\n\n'
    printf '## SECURITY / ISOLATION INVARIANTS\n\nTODO\n\n'
    printf '## CROSS-PLATFORM REQUIREMENTS\n\nTODO\n\n'
    printf '## ACCEPTANCE TESTS\n\nTODO\n\n'
    printf '## VERIFICATION COMMANDS\n\nTODO\n\n'
  fi

  cat <<'EOF'
## STOP CONDITIONS

- Stop if architecture is ambiguous.
- Stop before expanding beyond expected files.
- Do not edit map, evidence ledger, or handoff.
- Do not commit.
- Report changed files, diffstat, gates, failures, and remaining risks.

EOF

  # THE FACTS BLOCK, WITH ITS SCHEMA.
  #
  # `scripts/evidence_from_report.py` consumes this block to draft the ledger
  # entry. Batch 47 asked four workers for "a glasshouse-facts block" without
  # ever saying what shape it takes; all four invented a flat `key: value`
  # form, and the tool refused every one of them with `missing top-level
  # lines`. The schema lived only in the consuming script's docstring, which
  # the worker is explicitly told not to read. Emitting it here is what makes
  # the two tools actually meet.
  cat <<'FACTS'
## FACTS BLOCK — required, and this exact schema

End your report with one fenced block. `scripts/evidence_from_report.py`
parses it; a flat `key: value` list is REFUSED. `lines:` is mandatory.

```glasshouse-facts
task: GH-EXAMPLE
status: complete            # complete | partial | blocked
worktree: .worktrees/example
lines:
  - id: 1641                # the map line number, as an integer
    verdict: closed         # closed | open | refused
    contract: "Given ..., when ..., Glasshouse ..., while preserving ..."
    production:
      - "src/foo.rs :: Type::method"
    regression:
      - "test_file::test_name"
    mutations:
      - vocabulary: skip-state-update
        change: "the exact find -> replace"
        result: killed      # killed | survived | not-run
        killed_by: "test_file::test_name"
        observed: "what the failure actually printed"
    limits:
      - "what this does NOT prove"
packet_errors:
  - "the packet said X; current source says Y (src/foo.rs:12)"
scope_overflow:
  - path: "src/bar.rs"
    reason: "why it was unavoidable"
gates:
  - "cargo clippy --all-targets --all-features -D warnings: clean"
```

FACTS
  printf '## REPORT TO\n\n`%s`\n' "$REPORT_REL"
} > "$OUT"

echo "wrote $OUT"

if $PRINT_WORKTREE; then
  echo "git worktree add -b $BRANCH $WORKTREE_ABS"
fi
