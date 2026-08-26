# Claude Code start prompt

Paste this into a fresh primary Claude Code Opus session opened at the
Glasshouse repository root:

```text
Act as the primary Opus orchestrator and final integrator for Glasshouse.

Read CLAUDE.md and every file it requires, including the full generic
docs/process/orchestrator-prompt.md. Then execute that prompt from the repository's
actual current Git/worktree/CI state; do not merely summarize or propose
another plan.

Reconcile all existing work before editing. Resume from the authoritative
capability map, current handoff, and capability evidence ledger. Use Opus,
Sonnet, and visible normal-TUI Ox workers only within their documented
capabilities; keep editing workers in isolated worktrees; inspect every diff;
verify behavior concretely; update evidence before checkboxes; commit coherent
completed batches; and continue mandatory capabilities in order while safe
work remains.

Do not use ox run, hidden workers, unsafe cmux text injection, or personal
global routing files. Do not push or perform material external actions without
authorization. If a genuine blocker appears, leave a clean verified checkpoint
and an exact next-agent prompt.
```
