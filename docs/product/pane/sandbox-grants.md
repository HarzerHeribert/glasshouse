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

### Windows — an AppContainer, entered at `CreateProcessW`; the job object is not a sandbox

**Operational since 2026-09-09, and every sentence below is measured on a
Windows ARM64 host, not reasoned.** Before that date the spawn path refused
every spawning tool on this platform — *"pane has no sandbox applier that has
ever executed on this platform"* — so `read`, `grep`, `rg`, `fd`, `jq` and
`bash` did not run at all.

- **`CreateAppContainerProfile`** plus a `SECURITY_CAPABILITIES` attribute on
  the `STARTUPINFOEXW` of `CreateProcessW` gives the child a package SID; the
  project directory's ACL is extended to grant that SID, and nothing else is.
  The container declares **no capabilities at all**, `internetClient`
  included.
- **The confinement is the spawn.** Stable `std` cannot attach a
  `PROC_THREAD_ATTRIBUTE` to a `Command` (`raw_attribute` is nightly,
  rust-lang/rust#114854), so `sandbox/windows.rs` owns the whole
  `CreateProcessW` — the pipes, the environment block, the command line and
  the container, in one call. There is no intermediate "confined `Command`" a
  caller could hold and spawn without checking a result.
- **One regime ships and it is the AppContainer alone.** A
  `WRITE_RESTRICTED` restricted token removes the user's own write reach
  without isolating anything and has no bearing on sockets; the regime built
  on it was removed rather than left reachable, so a half-confinement cannot
  be reported as a confinement. Combining a restricted primary token with the
  container is a **successor**, not a gap in what is claimed here.
- **The job object grants nothing.** Glasshouse already creates one
  (`crates/glasshouse/src/pty/process.rs:168`, with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) and `crates/glasshouse/src/pty/mod.rs:34`
  says it outright — *"this is structure within the sanctioned harness API,
  not a sandbox."* It is a **lifetime** primitive. The confined spawn creates
  one of its own for exactly that: it is this platform's `process_group(0)`,
  so a cancelled `bash` takes its background jobs with it. It must not be read
  as having satisfied any invariant in §1.

**The container SID is per project *and per user*.** It is a pure function of
the AppContainer profile name, so a name derived from the project path alone
would let any local process — including one running as a different account —
derive the same SID, create the same container, and inherit whatever the
project ACL grants it. The name folds in the current user's own SID, so it is
derivable only by someone who already knows both.

**A DENY ACE naming an AppContainer SID does not deny — measured, and it
changes the mechanism.** On 2026-09-09, with `.claude` carrying (in ACL order,
confirmed with `icacls`) an explicit DENY of the write bits for the container,
an explicit ALLOW of the read bits, and the project root's inherited ALLOW of
the read-write bits, a confined child ran `mkdir` inside `.claude` and
**succeeded** — while the same child's `mkdir` outside the project was refused
with *"Access is denied."* An AppContainer's access check is a **grant**
check: the package SID must be granted the access, and a DENY naming it
decides nothing. So §1.5's `.claude` carve-out is an **absence of grant**: the
carve-out directory is written with `PROTECTED_DACL_SECURITY_INFORMATION` so
the root's inheritable grant cannot reach it, and the container is given the
read bits alone.

**An absence of grant is three separate things, because two of them were
missing and a reviewer walked out through each — measured on the same host,
2026-09-09.** All three are in `grant_project_acl` and `write_acl`, and none of
them is sufficient alone:

1. **The carve-out is written *before* the root's grant.** Windows inheritance
   is a copy performed when the ACE is written: `SetNamedSecurityInfoW` on the
   project root walks the tree and propagates the root's inheritable grant into
   every child. Granting the root first *put* the read-write ACE inside
   `.claude`. A protected DACL is not propagated into, so the carve-out is
   written first and the root's grant never arrives.
2. **The container's ACEs are removed by not copying them, not by revoking
   them.** `SetEntriesInAclW`'s `REVOKE_ACCESS` merges *explicit* entries and
   copies an inherited ACE through unchanged, so the revoke passed over exactly
   the ACE it existed to remove and the protected DACL then froze it in place:
   `mkdir .claude\evil1 -> exit Some(0) | landed true`. The carve-out's DACL is
   now built by copying the effective DACL ACE by ACE and dropping every entry
   naming the container, with `INHERITED_ACE` cleared on the survivors so the
   developer, `SYSTEM` and `Administrators` keep explicitly what they had.
3. **The root's grant does not carry `FILE_DELETE_CHILD`.** On a directory that
   bit is the right to delete or rename a child *whose own DACL grants
   nothing* — which is precisely what the carve-out is. With it present a
   confined child ran `move .claude claude-moved-aside` and got
   *"1 dir(s) moved."*, then created a fresh `.claude` that inherited the
   root's write grant and wrote the settings document the next session compiles
   from. It is withheld, and `crates/pane/tests/sandbox_windows_cage.rs`
   reproduces the whole sequence against a real child.

**Escape 3 had two independently sufficient causes, and this document used to
name one.** Point 3 above is true and reproduces; it is also only half of it.
`.claude`'s own DACL grants the container `READ_RIGHTS`, which contains no
`DELETE`, and a rename needs `DELETE` **on the object** *or*
`FILE_DELETE_CHILD` **on the parent** — either one is enough. The pre-patch
build handed the container both: point 2's broken carve-out left the root's
inherited grant (which contains `DELETE`) sitting inside `.claude`, and point
3's mask carried `FILE_DELETE_CHILD` on the root. Withholding one of the two
would not have closed the escape. Measured on the Windows ARM64 VM,
2026-09-09, against the shipping build with one right handed back at a time —
the container's own SID, through a temporary hook in `grant_project_acl`, not
a stand-in:

| root's grant | `.claude`'s grant | `move .claude claude-moved-aside` |
|---|---|---|
| `0x0013_019F` (shipping) | `0x0012_0089` (shipping) | *"Access is denied."*, `0 dir(s) moved.` |
| `0x0013_01DF` (`+FILE_DELETE_CHILD`) | `0x0012_0089` | `1 dir(s) moved.` |
| `0x0013_019F` | `0x0013_0089` (`+DELETE`) | `1 dir(s) moved.` |
| `0x001F_01FF` (the pre-patch mask, and `FILE_EXECUTE` besides) | `0x0012_0089` | `1 dir(s) moved.` |

So neither the withheld bit nor the carve-out may be simplified away on the
grounds that the other one is doing the work, and
`each_half_of_escape_threes_fix_is_independently_load_bearing` runs rows two
and three on every Windows gate — through `ALL APPLICATION PACKAGES`, because
a right handed to the container's own SID is taken straight back by the repair
pass before the next child starts, as *What pane promises about the project's
ACLs* below records.

**An earlier review recorded the opposite of row four and it does not
reproduce.** That review restored the whole pre-patch mask on the project
root, kept the current carve-out, reported that the rename then failed, and
concluded from it that the withheld bit was not what closed escape 3. Re-run
here at both `0x0013_01DF` and `0x001F_01FF` — the second wider than the
pre-patch mask, not narrower — with the masks read back off the root before
the attack, the rename succeeded both times. The disagreement is
recorded rather than explained; the likeliest cause is the stale-artifact trap
`scripts/dev/windows/install-source.ps1` exists to prevent, which has already
made this VM run a test binary built from a previous tree.

**The grant is now bit by bit, and `FILE_ALL_ACCESS` minus one bit is not a
decision.** `READ_WRITE_RIGHTS` was `0x001F_01FF & !FILE_EXECUTE`; it is
`0x0013_019F`, and each surviving bit says in `sandbox/windows.rs` why it is
required. Three were removed and all three were reachable: `FILE_DELETE_CHILD`
(above), and **`WRITE_DAC` and `WRITE_OWNER`**, which are worse — inheritable
across the whole project, they let a confined child rewrite the security of any
file in it, including adding the `ALL APPLICATION PACKAGES` execute ACE that
decides whether the container can load a file the model wrote. `READ_RIGHTS` is
unchanged at `0x0012_0089`. Deleting an ordinary project file still works,
through `DELETE` **on the object**, which is what makes withholding the
parent's `FILE_DELETE_CHILD` cost nothing: measured with the bit restored, a
confined `mkdir`, `move` (file and directory), `rmdir` and `copy` all succeed
without it.

**`cmd`'s own `del` does not work inside the cage, and it is not the mask.**
Measured with `FILE_DELETE_CHILD` *restored* and the target file carrying an
inherited allow of `0x0013_01DF` — which contains `DELETE` — in both the
verbatim and the plain spelling of the path: `del` and `del /f /q` both exit 1
with *"Access is denied."* while the unconfined control removes the same file
and `move`, `rmdir`, `mkdir` and `copy` all succeed inside the container. So it
is a property of `del` under an AppContainer, it predates the narrowed mask,
and it is recorded rather than explained. `rmdir` and `move` are the deletes
that work.

**The program pane runs is never one the model could have written.** The
sandbox decides the program before it creates anything, and both questions are
its own — so both reach a program as a catchable `PermissionDenied` (§1.4)
rather than as an operating-system error:

- **A non-absolute program is refused.** `CreateProcessW` completes a partial
  `lpApplicationName` from the **calling** process's current drive and
  directory, and pane's current directory during a session is the project root
  — which invariant 3 makes writable. `tools::invoke::exec_grant` used to hand
  back the bare name for anything it could not resolve on `PATH`, so on a
  machine without `grep` installed a program wrote `<project>\grep`, called the
  `grep` tool, and pane executed the file it had just written. The unresolved
  branch cannot be salvaged here and is not: this platform has no executable
  roots for a search to be bounded by, and `spawn` passes `lpApplicationName`
  so no search happens at all. `tools::invoke` refuses it first, with a
  sentence naming the tool that is not installed.
- **A program the running profile would let the model *write* is refused**,
  wherever it sits. That is the general shape rather than the one measured
  spelling, and it is asked of `Profile::check` rather than re-derived. It is
  deliberately stricter than the escape required, and the cost is stated: a
  project-local MCP server executable is refused on Windows.

**What running pane leaves behind on a Windows machine, precisely.** Three
things persist, all by construction and none removed on exit:

1. **The AppContainer profile** — a registry entry and a directory under
   `%LOCALAPPDATA%\Packages\Glasshouse.Pane.<hash>\`.
   `DeleteAppContainerProfile` is called nowhere in this crate: the profile is
   per project root and per user and is *reused* by the next session, and
   deleting one a concurrent session is using would be worse than leaving it.
   Deleting stale profiles is a successor.
2. **An empty `.claude/` in a project that had none.** The carve-out has to
   exist to be carved out: the project root is writable, so if `.claude` did
   not exist a program could create it and write the settings document its
   *next* session is compiled from — invariant 5 defeated by a `mkdir`. pane
   creates the directory before protecting it, and a failure to create it is
   a refusal rather than a spawn without the carve-out.
3. **Two ACEs on the project tree** — an inheritable grant for that container
   SID on the project root, and, when the root is writable, an explicit
   read-only grant on `.claude` whose DACL is protected from inheritance. Both
   name **only** the container SID, so no other principal on the machine gains
   anything from either, and nothing pane writes removes an ACE belonging to
   anyone else: the inherited entries are copied into `.claude` before its
   inheritance is blocked, so the developer, `SYSTEM` and `Administrators`
   keep exactly the access they had. The lasting change is that `.claude`'s
   permissions stop tracking the project root's from then on.

   **The first spawn in a project does the whole tree's ACL work inside a tool
   call, and that stall is real.** `SetNamedSecurityInfoW` propagates the
   root's inheritable ACE to every existing child. Measured on the Windows
   ARM64 VM, 2026-09-09, on a 10,000-file project: the propagating spawn cost
   1.01s and the next one 47ms, so the cost is the walk and it scales with the
   repository.

   **An interrupted propagation is detected and repeated, and the carve-out's
   own grant is what detects it.** `SetNamedSecurityInfoW` writes the named
   object's DACL and *then* walks the tree, so a call that dies part way
   leaves the root carrying the final mask over descendants that never got it;
   reading the root's mask cannot tell that from a finished walk. The
   `.claude` carve-out is therefore written twice — closed, with no ACE for
   the container at all, *before* the root propagates, and given its read
   grant only *after* every root write has returned. The carve-out holding
   exactly `READ_RIGHTS` is then proof that the propagation before it ran to
   the end, and any other state re-runs the whole sequence. Measured on the
   same host and 10,000-file tree: with the witness removed, the next spawn
   cost 712ms against a 47ms skip, so the walk really is repeated.
   `an_interrupted_propagation_is_detected_and_repeated` asserts the same
   thing on every Windows gate over its own 5,000-file project — 489ms first,
   50ms skipping, 384ms with the witness gone, 51ms after — and it wants a
   factor of three where the measurement gives seven. Putting the witness on
   the root itself would have cost a second full propagation; putting it on a
   directory holding one settings document costs nothing.

   **A project with no writable root has no carve-out and therefore no
   witness.** There is nothing to carve out of a read-only grant, so that case
   keeps the old test — the root's own mask — and an interrupted propagation
   there is undetected. It fails closed twice over: the grant that did not
   arrive is a read grant, and its absence is a refusal.

   The skip is exact about *width* as well as about completion: a path whose
   grant is a **superset** of the intended one is rewritten rather than
   skipped, which is what stops a project last caged by an earlier build from
   keeping `WRITE_DAC` for ever.

**What pane promises about the project's ACLs, and what it cannot — because
the honest failure direction here is a *retained wider grant*, not only an
unreachable subtree.** pane writes ACEs for exactly one trustee, the
container's own SID, on exactly two paths: the project root and the `.claude`
carve-out. Every spawn rewrites both with `SET_ACCESS`, so that is also the
whole of what it takes back.

- **Taken back.** A grant to the container SID on either of those two paths,
  however it got wider. Measured on the Windows ARM64 VM, 2026-09-09:
  `icacls <root> /grant *<container>:(OI)(CI)(F)` was undone by the next
  spawn, back to `0x0013_019F`, before the child ran —
  `what_the_repair_pass_can_and_cannot_take_back`.
- **Not taken back: an explicit ACE on a descendant.** Windows inheritance
  recomputes only the *inherited* part of a child's DACL, so an explicit entry
  survives every propagation, and pane reads the masks of the two paths it
  grants and of no others. Measured: an explicit `(OI)(CI)(F)` for the
  container on `<root>\sub` was still there after a full re-propagation. A
  read-only walk looking for them would be the same shape of stall as the
  write walk above, and would still not see the next item.
- **Not taken back: an ACE naming a group the container belongs to.** An
  AppContainer's access check is a grant check against the package SID *and*
  the well-known package groups, so `ALL APPLICATION PACKAGES` (`S-1-15-2-1`)
  and `ALL RESTRICTED APPLICATION PACKAGES` (`S-1-15-2-2`) grant the container
  whatever they carry. pane neither writes nor reads those, at any depth.
  Measured: with `*S-1-15-2-1:(DC)` on the project root a confined child ran
  `move .claude claude-moved-aside` and got `1 dir(s) moved.`, and the repair
  pass ran on the same spawn and left the ACE untouched.

**Both of those take a principal with the developer's own rights; the confined
child cannot make one.** Measured on the same host with the shipping grant: a
child copied `cmd.exe` into the project — an ordinary project write — and then
ran `icacls <that file> /grant *S-1-15-2-1:(RX)`, which exited `5` with
*"Access is denied."* and left `image_admits_app_containers` at `Ok(false)`.
`WRITE_DAC` and `WRITE_OWNER` are what would have allowed it and both are
withheld. So the residual is a person, or a program running outside the cage,
widening the project's ACLs by hand — and pane says here that it does not
defend against that rather than implying it does.

**A file the container can load is still not one it managed to run, and that
is unexplained.** Measured twice, 2026-09-09: with `<root>\sub` hand-granted
`ALL APPLICATION PACKAGES:(OI)(CI)(F)`, a child-written copy of `cmd.exe`
inside it reported `image_admits_app_containers -> Ok(true)`, the unconfined
control ran it, and the confined child's attempt answered *"The system cannot
find the path specified."* It is recorded, not relied on: the containment
claim rests on the withheld `WRITE_DAC` above and on the writable-program
refusal in `spawn`, both of which are checked, and not on this.

**Invariant 3 has two writable roots here, not one.**
`%LOCALAPPDATA%\Packages\<container>\` is created by Windows for the container
itself and is writable by it; no rule in a settings document can remove it.
This is a Windows-specific consequence of using an AppContainer at all, it is
stated in the regime sentence a session prints, and it is recorded here rather
than left to be discovered. It is outside the project and outside every
grant pane writes.

**The network claim depends on a service, and pane measures it.** An
AppContainer that declares no `internetClient` capability is refused sockets by
the Windows Filtering Platform, which is hosted by the Windows Firewall
service — not by the object-manager access check that enforces every file
grant. `sandbox::windows::network_isolation()` queries that service and reports
`EnforcedByFirewall`, `NotEnforced` or `Unknown`; there is no pure function of
the regime claiming the network is gone, because no such function could back
the claim.

**A binary the container cannot load is refused, not made runnable.** An
AppContainer cannot load an image whose own ACL lacks an execute ACE for
`ALL APPLICATION PACKAGES`, and a tool installed by a package manager may lack
one. pane does **not** write ACEs onto files outside the project to fix that:
the spawn checks the image's ACL first and refuses by name, so the outcome is
a narrower cage that starts nothing rather than a wider one that starts
something.

**MSYS2 binaries cannot run inside an AppContainer, and that is what stands
between Windows and a working `read` or `grep`.** Measured on the Windows
ARM64 host, 2026-09-09, with Git for Windows' `usr\bin` on `PATH` and the cage
working: `cat.exe` and `grep.exe` both died at start-up with

    *** fatal error - NtCreateDirectoryObject(\BaseNamedObjects\msys-2.0S5-…):
    0xC0000022

and `bash.exe` exited `0xC0000142` (`STATUS_DLL_INIT_FAILED`) for the same
reason. The msys runtime creates a shared object-manager directory under the
global `\BaseNamedObjects`, and an AppContainer's object namespace is
redirected to its own; the create is refused, so the runtime aborts before
`main`. The same container runs `cmd.exe` — a native Windows image — without
complaint, so this is a property of Cygwin/MSYS and not of the cage.

The consequence is a **tool-registry** one, not a sandbox one: on a Windows
machine whose `cat`, `grep` and `bash` come from Git for Windows, those tools
cannot be confined and therefore cannot be offered. Native Windows binaries —
`rg`, `fd` and `jq` are all one — have no such problem. It is why the
in-process `read` in §7's successor list is load-bearing rather than a
tidying-up, and why `crates/pane/tests/sandbox_windows_cage.rs` reports, per
machine, which registry tools the container can load and what they do when it
can.

**Not expressible on Windows:** ACLs are per-object, so extension-filtered
globs get the same treatment as Linux — directory-granular ACL, exact filter
in pane's pre-call check. The same is true of a `deny` naming a subtree inside
a granted root: the OS layer admits it and `Profile::check` is what refuses
it, before any child is spawned. Case-insensitivity is the platform's, and a
`deny` pattern is matched case-insensitively there and case-sensitively
elsewhere; pane states which at session start rather than picking one and
being wrong on one platform.

**`PATHEXT` is part of the sandbox, not a convenience.** `PATH` holds
directories and a tool is named `rg`, so resolving it means trying `rg.exe`
and the rest of `PATHEXT`. Without that step nothing resolved on Windows and
every spawning tool fell back to the *unresolved* branch of the 61D exec-roots
ruling — the wider grant — on the one platform whose applier cannot narrow an
exec grant at all.

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
- **Whether pane defends against the developer's own account.** It does not,
  and §3 says exactly where: an explicit ACE on a descendant, or an ACE naming
  a package group the container belongs to, widens the cage and pane neither
  writes nor reads either. What is decided is that the confined child cannot
  produce one — measured, `WRITE_DAC` and `WRITE_OWNER` are withheld and
  `icacls /grant` from inside answers *"Access is denied."*

**Named successors, so none of them is mistaken for a gap in what §3 claims:**

- `read` spawning `cat` at all, and `grep` spawning `grep`. An in-process
  `read` would need no image, no container-loadable binary and no `PATHEXT`
  lookup on any platform — and on Windows it is the only way either tool can
  work, because the MSYS2 binaries that provide them cannot start inside an
  AppContainer (§3). Until then `read` and `grep` are unusable on a Windows
  machine whose coreutils come from Git for Windows, and
  `tests/bounded_excerpts.rs` and `tests/search_artifacts.rs` fail there for
  that reason and no longer for want of a sandbox.
- The `Regime` sentence printed at session start. §3 requires it and no
  platform implements the printing; all three appliers can produce the
  sentence today.
- Deleting stale AppContainer profiles from `%LOCALAPPDATA%\Packages`.
- **Bounding the first spawn's ACL work.** The completion witness that used
  to be listed here has landed (§3): an interrupted propagation is detected
  and repeated. What is left is the *cost* — the walk is O(files in the
  project) and it happens inside the first tool call, measured at 1.01s for
  10,000 files. Doing it off the tool call, or incrementally, is a successor.
- **Why `del` is refused inside an AppContainer.** §3 records the measurement
  and that it is not the access mask; nobody has explained it. `rmdir` and
  `move` are the deletes that work, so it is not blocking.
- Strengthening the container with a `WRITE_RESTRICTED` primary token, which
  would remove the user's *own* write reach as well as the container's.
- Rendering the full `Profile::rules()` allow/deny set into each platform's
  OS layer, so a `deny` inside a granted root is refused by the operating
  system as well as by the pre-call check. This is the same limitation on all
  three platforms (§8) and is not Windows-specific.
- Extending the spawning test suites that are gated to macOS and Linux
  (`events.rs`'s background-job cases, `helpers.rs`'s post-result cases) to
  Windows now that it has an applier.

## 8. Exact-call suspension seam (development, not interactive approval)

`Runtime::with_approval_gate` is a host-only callback seam for a future
interactive approval implementation. The shipped session and TUI do not
install it. It can only delay or deny a registered call that the existing
immutable profile already admits. Missing grants, explicit denies and
never-grantable actions remain refusals and never reach its request channel.
It does not interpret `permissions.ask`, modify settings, add an OS grant,
or grant MCP discovery or calls.

The runtime waits inside the current Rust tool callback while a host thread
answers a request through its own consumed reply sender. A once answer is
consumed by one attempt, including a failed attempt. Session answers match
the complete canonical tool name, project root and every checked argument;
they are never patterns. Re-resolving the original arguments after the wait
rejects a symlink that changed its canonical target. A disconnected host,
dropped request, user cancellation or V8 termination denies the suspended
call. A late answer cannot apply to another request. The existing cell wall
clock includes this waiting time; it has not been paused or weakened.
Subagent and background calls never inherit this gate. Remembered-action
summaries expose the tool and an identity hash, not argument values.

`tests/approval_boundary.rs` exercises this seam inside real V8 cells,
including a confined Bash append before a suspended write. The append occurs
once, the write resumes in the same cell, and a repeated write needs a new
once answer. Other tests cover exact session matching, execution failure,
denial, disconnect, cancellation, timeout, symlink retargeting and unchanged
OS confinement. These are callback tests, not TUI/PTY approval acceptance.

Full interactive missing-grant approvals remain blocked on the platform
appliers. `macos::profile_text` currently renders the project root and the
`.claude` write carve-out, but does not render the full `Profile::rules()`
allow/deny set; its `Regime::ProjectRootOnly` explicitly describes that
limitation. `linux::landlock_rules` and `linux::bwrap_argv` likewise derive
root-based grants; additive Landlock rules cannot subtract an in-root deny.
Windows process execution is no longer refused: the AppContainer applier is
operational (§3). Like the other two, its OS layer renders the project root
and the `.claude` carve-out rather than the full `Profile::rules()` set, so
the same limitation applies to it. Passing a widened clone to these implementations
would not establish an exact additional capability. A Bash or MCP admission
alone would also not fix a filesystem deny that the OS layer does not render.

Before connecting the TUI, the implementation must classify hard refusals
separately from missing/ask decisions, prove exact per-call OS grants and
deny precedence on each supported platform, define explicit MCP server-start
approval, and add real terminal tests. No cell-replay fallback is permitted.

CONTRACT
behaviour:  Every tool a pane program can call runs under an OS sandbox whose file grants are computed once from `.claude/settings.json` `permissions`, with the project root the only writable root and no network at all.
invariant:  No grant is widened at the model's request, `deny` beats `allow` at every specificity, and a request outside the grant throws `PermissionDenied` inside the program without ever becoming a prompt.
path:       `crates/pane/src/sandbox/{profile,macos,linux,windows}.rs`: one profile compiler from the settings document, three platform appliers, and one pre-call path check that enforces the filters the OS layer cannot express.
test:       `crates/pane/tests/sandbox_grants.rs::a_program_cannot_widen_its_own_grant` — a cell that rewrites `.claude/settings.json` to allow `$HOME` and then reads `~/.ssh/id_ed25519` gets `PermissionDenied` on both calls; plus Phase 46's four named tests run against the sandboxed path.
