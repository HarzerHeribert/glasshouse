# Pane project context

At each user task, including a resumed task, Pane refreshes root `AGENTS.md`
and `CLAUDE.md` from the session's existing read profile. Each document is
labelled with its file and directory scope. Deeper scopes apply to their
subtrees; same-directory documents have equal scope. Settings and sandbox
grants remain fixed for the session.

A bounded task-start environment snapshot supplies platform/architecture,
UTC time, execution shell, absolute project root, shell cwd/reset semantics,
a scratch-path suggestion, permitted Git metadata, sorted top-level entries,
and detected project files. It runs no project executable, creates nothing,
and reports unavailable facts as unknown/refused. It does not guess installed
language versions. The snapshot is stable between inference requests.

A path-only index points to nested guidance. Before a path tool uses an
unseen scope, the runtime stops that cell, delivers the complete applicable
documents, and asks the model to continue. The blocked call and the remainder
of that cell did not run; earlier completed effects remain and are never
replayed automatically. A changed instruction document triggers a fresh
boundary. A successful instruction-file write is explicitly reported as
completed before that boundary. Subagents follow the same rules.

Bash and background shell commands are opaque, so their first call loads all
indexed guidance before spawn. This is guidance delivery, not a filesystem
security boundary: scripts can contain arbitrary paths and intermediate
changes. Generated/auxiliary directories (including `.git`, `.pane`, `target`,
`node_modules`, `.worktrees`, and `.agent-runtime`) are excluded from the index;
explicit structured paths into them still load ancestor guidance. Guidance
outside the project root is not loaded. Incomplete or oversized required
guidance stops the operation with a visible reason.

The loader caps a document at 64 KiB, a batch at 256 KiB/128 documents, and
index discovery at 10,000 entries/32 directory levels. It never presents a
truncated instruction prefix as a complete document. These are host context
bounds, not limits on the model's cell source.

Task context and appended instruction deliveries are saved as rollout
metadata, not chat messages. Resume retains visible chat and refreshes the
next task's base context. No model inference is used to collect these facts;
the supplied context still contributes to provider input tokens.
