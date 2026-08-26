# Capability evidence — phase 9

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 9 — the Antigravity conversation identifier, from an index rather than a walk (lines 2 and 3)

Contract: Given a Glasshouse-started Antigravity session that has just ended,
when Glasshouse looks for its native conversation identifier, it reads **one
shared index file** keyed by project path and records the identifier only if
that project's entry both **changed** during the session and the index's mtime
sits inside the session's window — while preserving: **no conversation database
is ever opened**, an absent or unchanged entry records nothing, and a resume
only ever passes an identifier Glasshouse recorded itself.

State: **COMPLETE** for both lines. Phase 9 is five of seven.

#### Why this needed a new shape at all

`session::native_id::discover` was built for one shape: a directory of session
records, each self-describing in its own first line — it walks, filters by name,
and **opens every survivor**. Antigravity does not have that shape. Its
identifier lives in `~/.gemini/antigravity-cli/cache/last_conversations.json`,
a flat `{project path: uuid}` map with **no timestamps**, and its records are
`conversations/<uuid>.db` — SQLite databases holding the user's private
conversations. A previous packet asked for `session_id_source` to be pointed at
those databases; the worker refused and was right.

So `NativeSessionSource` is now an enum: `RecordPerSession` (Codex's walk, byte
for byte unchanged) and `SharedIndex`, paired with a new pure adapter method
`read_index_entry`. The `SharedIndex` path reads **exactly one named file** and
never calls the directory walk — a property of the code path, not a rule anyone
has to remember, and
`the_shared_index_code_path_never_mentions_the_directory_walk` enforces it.

#### The identity guard is two rules, and both are load-bearing

A shared index has no per-entry timestamp, so the window has to come from
elsewhere:

1. the **index file's own mtime** must fall inside `[started_at, ended_at]`;
2. the entry for this project must have **changed** during the session.

Rule 1 alone is not enough and the hole is worth naming: the mtime moves when
*any* project's entry changes, so an Antigravity session in another project
during our window could make a stale entry for ours look fresh. Rule 2 closes
it, because a stale entry is by definition unchanged. Its one false negative —
resuming the same conversation leaves the entry unchanged — is safe, because
Glasshouse only ever resumes an identifier it already holds.

#### Mutations — by the lead, plus two re-run independently by the orchestrator

- removing the changed-entry guard → `a_shared_index_entry_that_did_not_change_is_never_captured` **FAILED** (killed);
- making the shared-index path walk a directory →
  `the_shared_index_code_path_never_mentions_the_directory_walk` **FAILED** (killed).

Line 3 was additionally proved by hand against the built binary in a real PTY
before any test existed: a launched session listed as `resumable`, and
`glasshouse resume <short>` producing `--conversation <id>`.

#### CI evidence — and two red Windows runs on the way to it

**CI `32908006880` green on Linux, macOS, Windows and lint** at `63e0053`, with
the two decisive tests confirmed **by name in the Windows job's own log**:
`the_shared_index_code_path_never_mentions_the_directory_walk` (the guard that
the shared-index path can never open a conversation database) and
`a_shared_index_entry_that_did_not_change_is_never_captured` (the stale-entry
rule).

Getting there cost two red Windows runs, and neither was a product defect:

1. The scan located a function body by searching for a literal
   newline-brace-newline. `include_str!` reads the file exactly as checked out,
   and where Git converts line endings that literal is absent — so the guard
   **panicked** instead of asserting. Now scanned with `str::lines`, which
   strips the carriage return, making it CRLF-agnostic by construction.
2. The regression guard written for (1) built its CRLF copy with
   `SOURCE.replace('\n', "\r\n")` — but on Windows `SOURCE` is *already*
   CRLF, so that produced `\r\r\n` and `lines` strips only one. **The guard
   depended on the checkout it was guarding against.** Both copies now come
   from a normalised base.

The second fix was verified locally rather than on a third round-trip: the file
was converted to real CRLF, the suite run, the pre-fix guard restored under the
same CRLF to reproduce CI's failure with the **identical assertion message**,
and the file restored. That recipe is now practice §15.

#### `home_env` is `None`, and that is a finding

The design left the variable name open ("`GEMINI_DIR` or whatever agy
honours"). The lead searched the 1.1.20 binary for `GEMINI_DIR`, `GEMINI_HOME`,
`ANTIGRAVITY_HOME`, `AGY_HOME`, every `XDG_*` and every `*_HOME`/`*_DIR`
symbol: **Antigravity honours no environment variable for its state root.** So
`home_env` became `Option<&'static str>` — `Some("CODEX_HOME")` for Codex,
`None` here. Declaring `"GEMINI_DIR"` would have been a fifth invented
declaration in a module whose own doc already records two.

#### An orchestrator design rule that was too broad, corrected

The design said "no log line, no diagnostic" for a conversation identifier.
Two **pre-existing** log lines carry one (`native_id::capture`'s success log and
`resume_session`), and the second has a comment deliberately arguing the
identifier is the one fact that makes a failed resume diagnosable. The lead
reported the collision rather than choosing.

**Orchestrator's decision: the log lines stay.** The identifier is not a
credential — it grants no access and names local state Glasshouse already
records in its own database. The real property is narrower and is what the rule
should have said: *never log the index's contents, and never log an identifier
belonging to another project.* Both hold.

### Phase 9 — the Antigravity adapter (three of seven, and one design defect it caught)

Contract: Given a signed-in Antigravity CLI, when Glasshouse starts a session
in the current project, it runs the real `agy` in the viewport and treats
anything the harness does not report as unknown.

State: **COMPLETE for lines 1, 4 and 7.** Lines 2 and 3 are blocked on an
interface change described below; lines 5 and 6 are unavailable.

The user signed the CLI in, which unblocked the whole phase.

Production evidence:
- `harness/antigravity.rs` — starts `agy`, resumes with `--conversation <id>`,
  declares `session_ids` as `Discoverable` from the CLI's own index, and
  `Antigravity::read_last_conversation`, a pure function from index text to an
  identifier.

Platform/external evidence — the real signed-in harness:
- `the_real_antigravity_interface_appears_in_the_viewport` **passes**:
  Antigravity's own version string `1.1.20` reaches the Glasshouse viewport.
  That is the same assertion Claude Code and Codex are held to, and it is
  deliberately the version rather than a name — an earlier revision of this
  probe matched a harness *name* and passed against Glasshouse's own error
  message.
- On its first run the probe failed at Antigravity's **workspace-trust
  prompt**, which gates the banner the version sits in. The captured screen
  showed the harness's ASCII logo, "Welcome to the Antigravity CLI", its
  sign-in spinner, the trust prompt and its navigation hints, all rendering
  inside the viewport — none of which Glasshouse's chrome can draw. Trusting
  the directory once, exactly as the Claude Code call site's comment already
  assumes ("the project is this repository, which the user's Claude Code
  already trusts"), and the probe passes unchanged. The assertion was never
  weakened.

**Corrections to what this ledger previously recorded.** It said conversations
live in `~/.gemini/antigravity/conversations/` and that the directory was
empty. That is the *desktop app's* state root; the CLI's is
`~/.gemini/antigravity-cli/`. Conversations there are **SQLite databases named
by UUID**, and there is a machine-readable index at
`cache/last_conversations.json` mapping **absolute project path → UUID**. This
is the fourth declaration in this project derived from an artifact that did
not serve the purpose it was cited for.

Two further facts established against the signed-in CLI:
- **Print mode records nothing.** `agy -p` completes a turn and adds no entry;
  only interactive sessions are recorded, at session end.
- **Resume does not fail closed.** `agy --conversation <unknown-uuid>` prints
  `warning: conversation "…" not found` and then **starts a fresh conversation,
  exiting 0**. Codex refuses with an error; Antigravity does not. Glasshouse
  must therefore only ever pass an identifier it recorded itself, or a user
  would get a new conversation wearing an old one's name.

### Phase 9 — the Antigravity adapter (probed, and blocked on authentication)

State: **BLOCKED — the installed CLI is not signed in.** Everything that can be
established without an account has been.

What the binary says (Antigravity CLI 1.1.20, `/opt/homebrew/bin/agy`, read
2026-08-25):
- Resume is `--conversation <id>` — "Resume a previous conversation by ID" —
  which is what the adapter already declares. `--continue` / `-c` continues the
  most recent.
- `--project <id|name>` and `--new-project` scope a session to a project.
- `--dangerously-skip-permissions` and `--sandbox`, both already declared under
  Phase 6's approval modes.
- Also `--mode accept-edits|plan`, `--effort low|medium|high`, `--model`,
  `--agent`, and `-p/--print` for non-interactive runs.
- **No hook, event or notification mechanism appears anywhere in `--help`.**
  Subcommands are `agent(s)`, `changelog`, `help`, `install`, `mcp`,
  `mic-serve`, `models`, `plugin(s)`, `update`. So Phase 9's lines 5 and 6
  (structured lifecycle events) look genuinely unavailable, the way Claude
  Code's compaction line is — but that should be confirmed against a signed-in
  CLI before being declared.
- Conversations live in `~/.gemini/antigravity/conversations/`. The directory
  exists and is **empty**, because the CLI has only ever been interrogated on
  this machine, never run. So the identifier's format and discoverability are
  unestablished.

**Line 4 has real supporting evidence already.** Driving the shipped shell
against `agy` through `probe_real_harness_interface` put Antigravity's own
interface in Glasshouse's viewport — its welcome box, the text "Welcome to the
Antigravity CLI", "You are currently not signed in", "Select login method",
its numbered options and "[Use arrow keys to navigate, Enter to select]". None
of that is anything Glasshouse's chrome can draw, so the round trip through
`vt100` into Ratatui cells demonstrably works for this harness.

**The probe was nonetheless reverted rather than weakened.** It asserts the
harness's own *version string* reaches the viewport, deliberately, because an
earlier revision matched a harness *name* and passed against Glasshouse's own
error message. An unauthenticated `agy` opens on a login menu that carries no
version, so the assertion cannot hold here. Loosening it to match the login
text would reintroduce exactly the weakness the assertion exists to prevent, so
the test is not in the tree.

Missing evidence, and what unblocks it:
- **Somebody must sign the CLI in** (`agy` offers Google OAuth or a Google
  Cloud project). That is the user's credential and the user's action; nothing
  here can or should do it.
- Once signed in: re-add `probe_real_harness_interface("antigravity", "agy")`,
  which should then pass unchanged and close lines 1 and 4 together; take one
  turn to populate `~/.gemini/antigravity/conversations/` and read the
  identifier's shape for line 2; and confirm from a signed-in `--help` whether
  any hook mechanism exists before declaring lines 5-6 unavailable.
