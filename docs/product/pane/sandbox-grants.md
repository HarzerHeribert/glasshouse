# pane — sandbox grants

Unblocks **61D**. How `.claude/settings.json`'s `permissions` become an OS
sandbox on macOS, Linux and Windows; what can never be granted; and what a
program sees when it asks for something outside the grant.

The model's TypeScript is already contained: a V8 isolate has no ambient
authority, no filesystem, no sockets. This document is about the other half —
the **tools** the program calls, which spawn real processes and touch real
files. `cargo_test` is what needs a sandbox, not `hits.filter(...)`.

## 1. The invariants

These are numbered because 61D's acceptance quotes them.

1. **No grant is ever widened at the model's request.** There is no tool, no
   argument, no escape sequence and no prompt that adds a path to a profile.
   The only widening path is a person editing `.claude/settings.json` and
   starting a new session.
2. **`deny` beats `allow`, at every specificity.** A path matched by any
   `deny` pattern is refused even when a longer, more specific `allow` names
   it exactly. There is no "most specific wins" rule to reason about.
3. **The project root is the only writable root by default.** Not the home
   directory, not a temp directory, not the parent of the project.
4. **A request outside the grant is refused inside the program.** The tool
   call throws `PermissionDenied { tool, path, rule }`, catchable by the
   model's own program and previewed like any other error
   (`runtime-contract.md` §5). It never becomes an interactive prompt, never
   reaches the user as a question, and never escalates.
5. **The profile is computed once, at session start, and is immutable for the
   life of the session.** This is not a performance decision: `.claude/`
   lives *inside* the project root, which invariant 3 makes writable, so a
   profile recomputed from disk mid-session would let a program widen its own
   sandbox by editing the file it was derived from. `.claude/**` is therefore
   also in the deny-write set by default, and `settings.json` is read before
   the sandbox is entered.

## 2. The pattern language, and what each pattern is

`.claude/settings.json` (this repository's own is the fixture — it carries
seven `allow` and two `deny` entries, all `Read`/`Write`/`Edit` with absolute
globs) admits these forms. Each maps to a different **kind** of rule, and
conflating them is the mistake this table exists to prevent.

| pattern | kind | becomes |
|---|---|---|
| `Read(<glob>)` | filesystem | read grant on the realpath closure of the glob |
| `Write(<glob>)` | filesystem | create+write grant |
| `Edit(<glob>)` | filesystem | read+write grant on existing files |
| `Bash(<prefix>*)` | **argv admission** | nothing in the OS profile |
| `Bash` (bare) | argv admission | every command line admitted; the profile is unchanged |
| `WebFetch(domain:…)` | network | **not registered**; see §4 |
| `mcp__<server>__<tool>` | tool admission | that MCP tool is registered; no OS rule |

**`Bash(cargo test*)` grants `cargo test` nothing.** It admits the command
line, and the process it spawns still gets exactly the file grants the
`Read`/`Write`/`Edit` patterns produced. A reader who takes a `Bash` allow-list
for a capability list has inverted the model: the allow-list says which
commands may be *attempted*, and the sandbox says what any of them may
*touch*. Both are checked, in that order.

Path patterns are resolved before matching: `~` expands, relative paths
resolve against the project root, and every candidate is compared after
symlink resolution — the discipline
`crates/glasshouse/src/commands/context_firewall.rs:257`
(`project_relative_path`) already applies, and for the same reason: two
spellings of one path are how a containment check comes to disagree with
itself.

## 3. Per platform

### macOS — seatbelt

`sandbox_init` with a generated profile, applied to each spawned tool process
before `exec`. Shape:

    (version 1)
    (deny default)
    (allow process-exec* (subpath "/usr/bin") (subpath "/opt/homebrew/bin"))
    (allow file-read*  (subpath "/Users/e/proj") (literal "/etc/passwd"))
    (allow file-write* (subpath "/Users/e/proj"))
    (deny  file-write* (subpath "/Users/e/proj/.claude"))
    (deny  network*)

Globs map to `(subpath …)`; an extension-filtered glob maps to a `(regex …)`
term, which seatbelt supports directly. Every pattern in the table above is
expressible.

### Linux — bubblewrap for the view, Landlock for the grants

Two primitives, and they do different jobs. `bwrap --unshare-all --ro-bind /
/ --bind <project> <project> --dev /dev --proc /proc` builds the mount view
and removes the network namespace. A Landlock ruleset (ABI ≥ 3;
`landlock_create_ruleset`, `landlock_add_rule` with
`LANDLOCK_RULE_PATH_BENEATH`) then applies the per-path rights inside it, and
`no_new_privs` is set so nothing regains them.

**Not expressible on Linux, and stated rather than dropped:** Landlock rules
are path-handle based. There is no glob and no regex. `Read(**/*.rs)` becomes
a Landlock read grant on the *enclosing directory* — coarser than the
pattern — with the extension filter enforced by pane's own pre-call path
check. So on Linux the OS layer is deliberately coarser than the written
pattern for extension-filtered globs, and the pattern's precision comes from
the in-process check alone. On a kernel without Landlock, bubblewrap's mount
view is the whole enforcement and directory-granularity is all there is; pane
says so at session start in the sidebar rather than implying an exactness it
does not have.

### Windows — a restricted token and an AppContainer; the job object is not a sandbox

Three primitives, and only two of them grant anything:

- **`CreateRestrictedToken`** with `WRITE_RESTRICTED` and deny-only SIDs
  removes the user's own write reach.
- **`CreateAppContainerProfile`** plus `SECURITY_CAPABILITIES` in
  `STARTUPINFOEX` gives the process a capability SID; the project directory's
  ACL is extended to grant that SID, and nothing else is. An AppContainer
  without `internetClient` has no network, which is how §4's network refusal
  is enforced here.
- **The job object grants nothing.** Glasshouse already creates one
  (`crates/glasshouse/src/pty/process.rs:168`, with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) and `crates/glasshouse/src/pty/mod.rs:34`
  says it outright — *"this is structure within the sanctioned harness API,
  not a sandbox."* It is a **lifetime** primitive: it guarantees the tree dies
  with pane. 61D reuses it for that and must not be read as having satisfied
  any invariant in §1.

**Not expressible on Windows:** ACLs are per-object, so extension-filtered
globs get the same treatment as Linux — directory-granular ACL, exact filter
in pane's pre-call check. Case-insensitivity is the platform's, and a `deny`
pattern is matched case-insensitively there and case-sensitively elsewhere;
pane states which at session start rather than picking one and being wrong on
one platform.

## 4. What is never grantable, by any pattern, on any platform

1. **Network.** No `permissions` pattern names a host, a port or a protocol,
   so a network grant would have to be invented — and an invented capability
   is the one thing an allow-list must never produce. Tools that need network
   are **not registered** under the sandbox; `WebFetch` is absent from the
   registry in 61D rather than present and failing.
2. **The OS keyring or credential store.** Keychain, Secret Service, DPAPI —
   and, for writing, the machine's own credential and identity store, by
   name: `/etc/sudoers`, `/etc/sudoers.d`, `/etc/shadow`, `/etc/gshadow`,
   `/etc/passwd`, `/etc/master.passwd`, `/etc/group`, `/etc/pam.d`,
   `/etc/ssh`, `/etc/security`, and `%SystemRoot%\System32\config` where
   that variable is set. Write-only on purpose: `/etc/hosts` is an ordinary
   readable file and §3's own seatbelt example reads `/etc/passwd`, so the
   read side is untouched. (Ruled 2026-09-05 after `GH-PANE-61D-VERIFY`
   found `Write(/**)` reaching `/etc/sudoers`; the rule is implemented in
   `sandbox/profile.rs::system_credential_paths`.)
3. **`$HOME` outside the project** — the whole of it, by any pattern; the
   names that matter most are `~/.claude`, `~/.codex`, `~/.ssh`, `~/.aws`,
   `~/.config`, and they are examples, not the rule. A settings document
   cannot grant `~/notes/`, a sibling project under `~/projects/`, or
   `~/.cargo/registry` either; a project that needs scratch space gets it
   inside its own root. (On Windows `%TEMP%` lives under the profile and
   is refused for a project rooted elsewhere — untested until the Windows
   pane cell runs.)
4. **Glasshouse's own state and data directories**, including every SQLite
   database in them. A pane program that could write those could rewrite
   another project's memory, which is the boundary Phase 46 exists to hold.
5. **Any path a `deny` pattern matches** (invariant 2), and `.claude/**` for
   writing (invariant 5).
6. **Process-level escapes**: `ptrace` / `task_for_pid`, debugger attach, and
   re-invoking the sandbox launcher (`sandbox-exec`, `bwrap`) from inside the
   sandbox.

## 5. Refusal, and what the model does with it

    PermissionDenied: read("/Users/e/.ssh/id_ed25519")
      rule: no grant covers this path; the project root is the only readable root
      tool: read

That is the whole of it. It is a JavaScript exception inside the model's own
cell, so a program can `try`/`catch` it and continue; the runtime does not
end the turn, does not retry (`runtime-contract.md` §5), and does not ask the
user. The `rule` string names the *deciding* rule — the `deny` entry that
matched, or the absence of any `allow` — so a person reading the transcript can
fix the settings file without re-deriving the profile.

A refusal is recorded once per distinct `(tool, path, rule)` per task in the
rollout, and shown in the sidebar as a count. A program that probes a hundred
paths produces one sidebar line and one hundred exceptions, not a hundred
lines.

## 6. Acceptance

61D's acceptance is Phase 46's contamination suite run against the sandboxed
path on all three platforms — `crates/glasshouse/tests/project_isolation.rs`,
and specifically:

- `canonicalized_paths_cannot_escape_the_project_root_through_parent_directory_traversal` (:356)
- `symlink_targets_outside_the_project_root_are_rejected_by_project_config_io` (:416)
- `one_project_database_cannot_be_queried_through_another_projects_glasshouse_instance` (:166)
- `deleting_one_projects_state_leaves_a_sibling_projects_state_intact` (:548)

They are not weakened, not parameterised into passing, and not replaced by a
pane-local equivalent. A pane tool that can defeat one of them has defeated
the project boundary, and the sandbox is what must change.

## 7. What this does not decide

- **The tool registry's contents** — which tools exist at all is 61E's; this
  document decides only that a network-needing tool is not among them under
  the sandbox.
- **The seatbelt/bwrap/AppContainer implementation's provenance.** Codex's
  `sandboxing`, `bwrap` and `windows-sandbox-rs` crates are Apache-2.0 and are
  the intended take; which of them is vendored is 61D's own packet.
- **Whether a user may opt out.** No flag for it is specified here. If one is
  ever wanted it is a map line, and invariant 1 says what it may not be: a
  flag the model can reach.

CONTRACT
behaviour:  Every tool a pane program can call runs under an OS sandbox whose file grants are computed once from `.claude/settings.json` `permissions`, with the project root the only writable root and no network at all.
invariant:  No grant is widened at the model's request, `deny` beats `allow` at every specificity, and a request outside the grant throws `PermissionDenied` inside the program without ever becoming a prompt.
path:       `crates/pane/src/sandbox/{profile,macos,linux,windows}.rs`: one profile compiler from the settings document, three platform appliers, and one pre-call path check that enforces the filters the OS layer cannot express.
test:       `crates/pane/tests/sandbox_grants.rs::a_program_cannot_widen_its_own_grant` — a cell that rewrites `.claude/settings.json` to allow `$HOME` and then reads `~/.ssh/id_ed25519` gets `PermissionDenied` on both calls; plus Phase 46's four named tests run against the sandboxed path.
