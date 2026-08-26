# Capability evidence — phase 9b

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 9B — the child's environment, and Phase 9B at nine of nine

Line, quoted exactly: "Preserve the user's existing shell environment except for
explicit launch-profile overrides."

Contract: Given a user with an existing shell environment, when Glasshouse
launches a harness under a launch profile, the child sees the user's environment
plus exactly the profile's declared overrides and nothing else — while
preserving: no variable the user set is dropped, none is altered that the profile
did not name, and nothing outside the spawned process tree is touched.

State: **COMPLETE.** Phase 9B is **nine of nine**.

**The line did not already hold**, which the packet had explicitly allowed for.
Two production behaviours broke it, both at the common PTY boundary:

1. `TerminalCommand::new` recorded a `TERM` override unconditionally, changing
   an unset, empty or `dumb` value even though no profile named `TERM`.
2. **`portable-pty` 0.9.0 rewrites the environment it was given.**
   `CommandBuilder::new` starts from `std::env::vars_os()`, but its Windows
   branch then merges registry-composed system and user values over that map —
   including replacing `PATH`. On Unix it adds `SHELL` when the parent had none.

The second is the find worth keeping. **A pre-existing smoke test had already
observed that Windows `PATH` mismatch and responded by compiling its inheritance
assertion out on Windows** — a known-wrong case papered over rather than fixed.
`into_builder` now calls `env_clear()`, copies an exact snapshot of Glasshouse's
own environment, and layers only the recorded overrides and removals on top. The
two skipped assertions now run on Windows.

**On removing the `TERM` fallback.** Its doc justified it by "Glasshouse itself
was started from a context without a terminal" — but `session::attach` **refuses
outright** unless both stdin and stdout are terminals, with a message telling the
user to run from an interactive terminal. The motivating case cannot reach a
harness launch, so the justification was stale. Recorded here so nobody restores
it on the strength of that comment. A user who deliberately sets `TERM=dumb` now
keeps it, which is what the line asks for.

#### Evidence quality

Three mutations by the worker, each killed, each reported with the named test's
own result line: drop the parent snapshot; drop the profile overrides; add an
unconditional `TERM`. The orchestrator ran a fourth, independent of that set —
**apply the parent snapshot *after* the overrides, so the user's value wins over
the profile's** — killed by the same test.

**The worker could not run the full suite and said so rather than working around
it.** Codex ran under `-s workspace-write -a never`, which denies loopback bind
and Keychain access; 27 gateway and 3 secret tests failed on that alone. It
reported the exact failure count, identified the cause, and stated that no
production or test workaround had been added for infrastructure restrictions.
**The orchestrator ran the suite unsandboxed: 777 passing, 0 failing**, which
confirms all 30 were sandbox artifacts.

#### First batch run on the Codex harness

Model `gpt-5.6-sol` at `xhigh`, to match the effort the Claude Code workers run
at. Notes for whoever runs the next one:

- **Model identifiers need the full prefix.** `sol` is rejected on a ChatGPT
  subscription account; it is `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`.
- **Codex needs no bypass shim.** `-s workspace-write -a never` is a real
  automatic-review mode, unlike Antigravity's blanket bypass — the same
  distinction Glasshouse's own adapters record.
- **That sandbox blocks loopback and Keychain**, so the orchestrator must run
  the full suite for any Codex batch.
- **It also blocks writing outside the worktree**, so the report path in the main
  checkout was refused. The worker wrote to `/tmp` and said so. Future Codex
  packets should put the report inside the worktree.

---

### Phase 9B — scoped harness wrappers and shims (eight of nine)

Contract: Given a launch profile, when the user starts a harness from the shell
— through `glasshouse run` or a shim they asked Glasshouse to generate — the
harness runs with exactly the profile behaviour it would have had from the TUI,
with every override confined to that process tree, while Glasshouse never
touches a shell startup file and deleting a generated shim is enough to remove
it.

State: **COMPLETE for eight lines. Line 384 is reopened** — see "A claim
Windows would not support" below.

Production evidence:
- `cli.rs` — `Command::Run` (fields identical to `Command::Launch`) and
  `Command::Shim { harness, --profile, --dir, --name, --force }`.
- `main.rs` — `Command::Launch { .. } | Command::Run { .. }` share **one**
  dispatch arm calling `launch_session`. The or-pattern only type-checks while
  both variants declare identical fields, so a divergence is a compile error
  rather than a review miss. That is line 390 made structural.
- `shim.rs` — `ShimRequest`, `ShimError`, `render`/`default_file_name` keyed
  off the injected `HostPlatform` (never `#[cfg]`, so the Windows `.cmd` shape
  is exercised on every runner), and `generate`, the only function in the
  module that touches a filesystem and which writes exactly one file inside
  `request.dir`.

Regression evidence — the twelve named acceptance tests, plus one added by the
orchestrator:
- `glasshouse_run_and_glasshouse_launch_take_the_same_path`,
  `a_profile_behaves_identically_from_run_and_from_launch`
- `an_override_reaches_the_spawned_process_and_not_the_parent`,
  `the_users_environment_survives_except_for_explicit_overrides` — both spawn
  a real env-dumping child and confirm `PATH`, which no launch names, arrives
  unchanged beside the one explicit override.
- `a_generated_shim_contains_no_secret_and_no_url`,
  `a_generated_shim_calls_glasshouse_run`,
  `a_shim_is_written_only_inside_the_user_selected_directory`,
  `a_windows_shim_is_a_cmd_file_and_a_unix_shim_is_a_shell_script`,
  `generating_a_shim_never_touches_a_shell_startup_file`,
  `deleting_a_generated_shim_leaves_nothing_behind`,
  `an_existing_file_is_not_overwritten_without_force`
- `a_generated_shim_actually_starts_the_harness` — end-to-end: generates a shim
  through the real subcommand, then executes **only the generated file**, and
  asserts the harness received the native profile's `--permission-mode auto`.
  Unix-only, flagged, with precedent in this file.
- `a_shell_unsafe_name_is_refused_before_any_file_is_written` — **added by the
  orchestrator.** See below.

**A profile name is untrusted input reaching a command line.** The generated
shim interpolates the harness and profile names into a script, and a profile
name is user-chosen. The worker flagged that it had quoted but not escaped
them, and judged a general shell-escaper out of scope — correctly, because the
right answer here is not escaping. This codebase already answers this class of
problem by **refusing**: `platform::exec` rejects `cmd.exe` metacharacters in
harness arguments rather than trying to quote them
(`spawn_command_windows_script_rejects_each_cmd_metacharacter`). So
`check_name` now refuses any name outside `[A-Za-z0-9._-]`, before a path is
computed or a byte written, and says which character it objected to. An
allow-list is right by construction where an escaper has to be right about two
shells forever, and a profile name is a TOML table key, so nothing legitimate
is lost.

Non-vacuity: **six mutations, six kills** — the unsafe-name check removed; a
provider URL embedded in the shim; a shell-startup-file write added; the
overwrite guard removed; Windows rendering the Unix script; and a second
`launch_session` call site introduced. The last verdict was re-verified by
reading the **bin** target's own result line after the first reading showed
only the lib target's, which had filtered the test out.

Platform/external evidence — the real binary:
- `glasshouse shim claude-code --profile native --dir <tools>` wrote a
  125-byte, mode-0755 file whose entire contents are
  `#!/bin/sh` and one `exec "<glasshouse>" run "claude-code" --profile
  "native" -- "$@"` — no secret, no URL, no routing logic, no duplicated
  adapter argument. It printed the exact path and the line saying that
  deleting the file is all it takes.
- `glasshouse shim claude-code --profile 'evil"; id; echo "'` was refused:
  "refusing to generate a shim for profile `…`: it contains `"`, which a shell
  would interpret rather than pass through."

CI evidence:
- **CI `32887437992` green on Linux, macOS, Windows and lint** at `5f99865`.
  Windows executed the shim tests by name
  (`a_generated_shim_calls_glasshouse_run`,
  `a_generated_shim_contains_no_secret_and_no_url`,
  `deleting_a_generated_shim_leaves_nothing_behind`) and 451 lib tests against
  macOS's 459 — the difference is the Unix-gated set, including the
  environment-inheritance assertion described below.
- The lint job's `Check README progress block` step ran and passed, so the
  README's generated block is verified against the map on every push rather
  than trusted.
