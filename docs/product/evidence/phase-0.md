# Capability evidence — phase 0

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

These eight lines were ticked before the evidence-ledger discipline existed, and this is the first entry written for them. Verified against the binary built from this worktree's tree (`claude/phase0-evidence`, head `9e3cdde`), on **macOS (darwin/arm64) only** — no Linux or Windows execution was performed for this entry.

---

### Phase 0 — Create Glasshouse as a Rust workspace that builds a single `glasshouse` executable

Map line 1 (`docs/product/capability-map.md:81`).

Contract: The repository's Cargo workspace produces exactly one binary, named `glasshouse`.

State: **LOCALLY VERIFIED**.

Production evidence:
- `Cargo.toml`: `[workspace] members = ["crates/glasshouse"]` — one member.
- `crates/glasshouse/Cargo.toml`: one `[[bin]]` stanza, `name = "glasshouse"`, `path = "src/main.rs"`.
- `cargo metadata --no-deps --format-version 1`, parsed: one package (`glasshouse`), targets `[('glasshouse', ['lib']), ('glasshouse', ['bin'])]` plus 20 `test`-kind targets (integration tests, not shipped binaries). No other `[[bin]]` exists anywhere in the tree (`grep -rn "^\[\[bin\]\]" --include=Cargo.toml .` returns exactly the one line above).
- `cargo build --bin glasshouse` → `Finished` dev profile; `target/debug/` contains one executable, `glasshouse`.

Missing evidence: none for this line.

---

### Phase 0 — Keep the initial dependency set limited to libraries required for async execution, terminal UI, PTYs, serialization, SQLite, and basic process control

Map line 2 (`docs/product/capability-map.md:82`).

Contract: Every direct dependency of the `glasshouse` crate serves one of the six named purposes.

State: **PARTIALLY VERIFIED — contradicted as literally worded against the tree today.** See `RECOMMEND UNTICK` in the report.

Production evidence — `cargo tree --depth 1` (macOS build; 22 direct dependencies, one of which, `keyring`, is macOS-only):

| dependency | purpose | one of the six? |
|---|---|---|
| crossterm, ratatui | terminal UI | yes |
| portable-pty | PTYs | yes |
| serde, serde_json, toml | serialization | yes |
| rusqlite | SQLite | yes |
| ctrlc, libc (unix), windows-sys (windows) | signal handling / low-level process control | defensible as "basic process control" |
| vt100 | terminal-emulation parsing of PTY byte streams | defensible as adjunct to PTYs/terminal UI |
| which | locates a harness executable before spawning it | borderline; adjacent to process control |
| **anyhow, thiserror** | error types/handling | **no** |
| **clap** | CLI argument parsing | **no** |
| **directories** | OS standard-directory resolution | **no** |
| **getrandom** | CSPRNG | **no** |
| **hex** | hex encode/decode | **no** |
| **sha2** | cryptographic hashing | **no** |
| **tracing, tracing-subscriber** | structured logging | **no** — this is box 7's own dependency |
| **ureq** | blocking HTTP client | **no** — network/HTTP is not one of the six |
| **keyring** (macOS only) | OS credential store | **no** — secret storage is not one of the six |

No `tokio`, `async-std`, or `smol` appears anywhere in the tree (`cargo tree \| grep -i async` — no output), so "async execution" is a granted-but-unused category; the binary is entirely synchronous (threads + `mpsc`, per the Phase 9D evidence's `spawn_provider_probe`).

At minimum **eleven** direct dependencies — `anyhow`, `clap`, `directories`, `getrandom`, `hex`, `keyring`, `sha2`, `thiserror`, `tracing`, `tracing-subscriber`, `ureq` — serve purposes outside the six named categories, unambiguously so even under a generous reading. Several are load-bearing for other already-shipped, already-ticked capabilities in this same binary: `tracing`/`tracing-subscriber` are what box 7 (two lines below) *is*; `clap` is what boxes 5 and 6 are built on; `keyring` is Phase 9E's credential store; `ureq` is Phase 9D/9G's provider connectivity and local gateway. The capability map appears to have grown callers that box 2, read literally, forbids the dependencies for.

Failure/isolation evidence: not applicable — this is a manifest-shape claim, not a runtime behavior.

Missing evidence: none — the dependency tree is fully enumerable and was fully enumerated. The finding is not a gap in evidence; it is a claim the current tree does not satisfy.

---

### Phase 0 — Make glasshouse run without requiring a global daemon, background service, Node installation, or Python environment

Map line 3 (`docs/product/capability-map.md:83`).

Contract: Given a machine with no Node or Python resolvable on `PATH`, and no Glasshouse-owned background process already running, `glasshouse --version` and `glasshouse doctor` complete normally, spawning no long-lived process that outlives the command.

State: **LOCALLY VERIFIED** (macOS/arm64 only; this is a claim about other machines and cannot be fully ruled out from here — see Missing evidence).

Production evidence:
- No source file under `crates/glasshouse/src/` names `"node"`, `"npm"`, or `"python"` as a command to spawn (`grep -rn '"node"\|"npm"\|"python' crates/glasshouse/src/` — no matches outside tests), and there is no `build.rs`. Nothing in the build or the binary's own logic can require either toolchain.
- `crates/glasshouse/src/gateway/mod.rs`'s own doc comment: the local gateway is "a listener, an address, a token, an upstream, and the moment each of them stops existing" — an in-process, ephemeral-port (`127.0.0.1:0`) loopback proxy tied to one Glasshouse instance's lifetime, not a separately daemonized or persisted service. It is the only long-lived listener in the codebase and it is not a background daemon in the sense this box forbids.

Platform/external evidence — the shipped debug binary, copied to a directory outside the build tree (a stand-in for a user-owned tools directory) and run with `env -i HOME="$HOME" PATH=/no/such/dir`, i.e. **nothing at all resolvable on `PATH`, node/python/npm included**:
- `glasshouse --version` → `glasshouse 0.1.0`, exit 0.
- `glasshouse --scope <fresh dir> --data-dir <fresh dir> --config-dir <fresh dir> doctor` → exit 0, full doctor report printed (all harnesses correctly reported `[not found]` since none were resolvable, rather than erroring).
- No child process was left running after either command returned (checked with `ps` immediately after).

Missing evidence: this rules out Node/Python/PATH dependence and daemon behavior on **macOS only**. Windows and Linux are not checked from this machine, and nothing here rules out a hidden requirement that only manifests on those platforms.

---

### Phase 0 — Make all runtime paths configurable so the binary can be used from a user-owned tools directory without a package-manager installation

Map line 4 (`docs/product/capability-map.md:84`).

Contract: Every runtime location the binary uses derives from exactly two overridable roots (data dir, config dir), each resolvable from an explicit CLI flag, then an environment variable, then an OS convention — never from a fixed system path.

State: **LOCALLY VERIFIED**.

Production evidence:
- `crates/glasshouse/src/paths.rs`, `RuntimePaths::resolve` — precedence is `--data-dir`/`--config-dir` > `GLASSHOUSE_DATA_DIR`/`GLASSHOUSE_CONFIG_DIR` > `directories::ProjectDirs`. Every other location on the type (`projects_dir`, `project_state_dir`, `user_config_file`, `provider_cache_dir`) is a fixed join off one of these two roots — there is no third, independently-configured location in this file.
- `cli.rs` exposes `--data-dir`, `--config-dir`, `--scope`, `--log-file` as documented flags; `--help` (below) lists `GLASSHOUSE_DATA_DIR`/`GLASSHOUSE_CONFIG_DIR` as the corresponding environment variables.

Platform/external evidence — binary copied to a fresh directory (see box 3's command), run with `--scope`, `--data-dir`, and `--config-dir` all pointing at directories created fresh for the test, none of them touched by any installer: `doctor` created `<data-dir>/projects/<project-id>/` and read `<config-dir>/config.toml` correctly, and a second run with `--log-level debug` (no `--log-file`) wrote `<data-dir>/projects/<project-id>/logs/glasshouse.log` — nothing was written outside the two overridden roots.

Missing evidence: macOS only, as with box 3; the OS-convention fallback path (`directories::ProjectDirs`) is read in source but its Linux/Windows values were not independently checked from this machine.

---

### Phase 0 — Add a `glasshouse --version` command that prints the binary version

Map line 5 (`docs/product/capability-map.md:85`).

Contract: `glasshouse --version` prints the crate version and exits 0.

State: **LOCALLY VERIFIED**.

Production evidence:
- `./target/debug/glasshouse --version` → stdout `glasshouse 0.1.0`, exit code 0.
- `cli.rs::version_is_the_crate_version` — asserts `Cli::command().get_version() == Some(env!("CARGO_PKG_VERSION"))`; would fail if the version were hardcoded separately from `Cargo.toml` and drifted.

Missing evidence: none.

---

### Phase 0 — Add a `glasshouse --help` command that documents the initial CLI surface

Map line 6 (`docs/product/capability-map.md:86`).

Contract: `glasshouse --help` documents every subcommand, every top-level option, the environment variables that mirror them, and the project-scope model.

State: **LOCALLY VERIFIED**.

Production evidence:
- `./target/debug/glasshouse --help` → exit 0; documents nine subcommands (`doctor`, `setup`, `sessions`, `memory`, `checkpoint`, `resume`, `launch`, `run`, `shim`), eight top-level options including `--scope`, `--data-dir`, `--config-dir`, `--log-level`, `--log-file`, `--log-stderr`, three environment variables, and a `PROJECT SCOPE` section explaining root resolution.
- `cli.rs::cli_definition_is_valid` runs clap's own `debug_assert()` over the CLI definition — catches malformed help text/argument conflicts at test time, not just at first manual run.

Missing evidence: none.

---

### Phase 0 — Add structured application logging that can be enabled for debugging without polluting the interactive TUI

Map line 7 (`docs/product/capability-map.md:87`).

Contract: Given no logging flag, Glasshouse emits nothing to stdout/stderr beyond its normal output. Given `--log-level`, `GLASSHOUSE_LOG`, or `--log-file`, it emits `tracing`-structured records at the requested filter, and unless `--log-stderr` is explicitly given, they go to a file — never to the stream the TUI draws on.

State: **LOCALLY VERIFIED**.

Production evidence:
- `crates/glasshouse/src/logging.rs`: `LogConfig::resolve` — no level and no explicit sink ⇒ `LogSink::Disabled`; an explicit file or `--log-stderr` implies logging even with no level; otherwise the default sink is `LogSink::File(default_dir.join("glasshouse.log"))`. The module's own doc comment: "Logging is off unless explicitly enabled, and when enabled it defaults to a file so diagnostic output can never be interleaved into the interactive TUI."
- Log files are opened at mode `0600` under a `0700` directory (`open_log_file`, `create_dir_secure`), and rotate once past 16 MiB (`rotate_if_large`).

Platform/external evidence — shipped debug binary, isolated data/config dirs:
- No flags: `doctor` → stdout has the report, **stderr is 0 bytes**, no log file created.
- `--log-level debug`, no `--log-file`: stdout unchanged, stderr 0 bytes, and `<data-dir>/projects/<id>/logs/glasshouse.log` appears with 4 lines including 3 at `DEBUG` (`grep -c DEBUG` → 3) — e.g. `keyring: creating entry with service glasshouse, user glasshouse-availability-probe, and no target`.
- `--log-file <path>`: the named file is created (not the default path); stderr stays 0 bytes.
- `--log-stderr`: stderr carries the same `tracing`-formatted lines (264 bytes for one `INFO` line); confirmed mutually exclusive with `--log-file` by `cli.rs::log_file_and_log_stderr_conflict`.
- `GLASSHOUSE_LOG=debug` (env var, no CLI flag): same file-sink behavior as `--log-level debug`.

Failure/isolation evidence — one real gap, not blocking: `LogSink::Stderr`'s own doc comment says it is "Only valid for non-interactive commands," but nothing in `main.rs` enforces that — `--log-stderr` combined with entering the interactive shell (`shell::run`) is not rejected at the CLI or runtime level; it is a documented caveat, not a guarded invariant. It does not affect the default, recommended path (file logging), which is what makes the box's core claim true.

Missing evidence: none for the default and documented paths; the unenforced `--log-stderr`-while-interactive caveat above is worth a follow-up but does not on its own make the box false, since the box's claim is that logging *can* be enabled without polluting the TUI, not that every combination of flags is guarded.

---

### Phase 0 — Add a clean shutdown path that restores the terminal state after normal exit, panic, or interrupt

Map line 8 (`docs/product/capability-map.md:88`).

Contract: Given the TUI has engaged raw mode and the alternate screen, whether Glasshouse ends by a normal quit, a signal, or a panic, the terminal is left in its normal (non-raw, primary-screen, cursor-visible) state and the process exits promptly with no leaked process.

State: **VERIFIED as of `HEAD` — see the update at the end of this entry. The account below is the first-hand reproduction that was true of commit `9e3cdde`, kept because it is what found the defect and because two of its measurements are the before-halves of the fix.**

Original state, at `9e3cdde`: **PARTIALLY VERIFIED — true for normal exit and a single interrupt; reproducibly false for the terminal-hangup case, about one run in four.** `docs/process/orchestration-practice.md` §33: this is a case, not a coin flip, but see the mechanism below.

Checked at `crates/glasshouse/src/shutdown.rs`: `restore_terminal` (disables raw mode, leaves the alternate screen, shows the cursor, idempotent), `install_panic_hook` (calls `restore_terminal` before the previous hook), `install_signal_handler`/`force_exit` (`std::process::exit(130)`, bypassing destructors, after running best-effort forced-exit cleanups and `restore_terminal` directly).

**Normal exit and a single interrupt — SUPPORTED.** Method: the shipped debug binary driven on a real pty via `pty.fork()`-equivalent (`os.openpty`, `setsid`, `TIOCSCTTY`), onboarding pre-completed, settled 1.5s into the idle drawn state (mirroring `tests/terminal_loss.rs`'s own `SETTLE`), then:
- sending the `q` keystroke: exit code 0, terminal-restore escape sequences (`\x1b[?2004l` … `\x1b[?1049l\x1b[?25h`, i.e. leave bracketed-paste, leave alternate screen, show cursor) present at the end of the captured stream.
- sending one `SIGINT` to the process (not a keystroke — the OS signal path via `install_signal_handler`): exit code 0, same restore sequence observed, ~20ms after the signal.

**A second, closely-spaced `SIGINT` (the forced-exit path, `EXIT_INTERRUPTED = 130`) — NOT INDEPENDENTLY REPRODUCED.** Sent a second `SIGINT` at gaps from 2ms to 400ms after the first: every trial still exited 0, because the graceful path is fast enough (single-digit milliseconds) that the second signal always arrived at an already-exiting or already-reaped process. The `force_exit` code path was read and is unambiguous, but this harness could not externally force the specific race (`SHUTDOWN_REQUESTED` already `true` when a second signal is *handled*) that reaches it via `SIGINT` alone — see below for where the same code path was reached a different way.

**The terminal going away (the case this box's "panic" claim actually turns on) — reproducibly fails.** Method: same pty setup, settled the same way, then the **master side of the pty was closed** with no signal sent by the harness — this is the exact scenario `crates/glasshouse/tests/terminal_loss.rs` (added in `9e3cdde`, same tree) exercises, and its own comment names the mechanism: closing the pty makes the child (a session leader whose controlling terminal is that pty) receive `SIGHUP` at the same moment `tui/event.rs`'s own `POLLHUP` detection fires on the main thread — a genuine two-thread race between:

  1. the `ctrlc` handler thread, which — if `SHUTDOWN_REQUESTED` is already `true` from the main thread's `POLLHUP` detection — calls `force_exit()` → `std::process::exit(130)`, skipping all destructors, cleanly; and
  2. the main thread's own normal return path, which (having returned `Event::Shutdown` and unwound back through `shell::run`) drops a `ratatui::Terminal`, whose `Drop` tries to show the cursor, fails (the fd is dead), falls back to `eprintln!` of the error — which **also** fails because stderr is the same dead terminal — and Rust's `eprintln!` panics on a failed write. Nothing catches that panic, so the process aborts with **exit 101**.

Twelve trials, both stdin/stdout/stderr on the pty (the realistic configuration, matching `tests/terminal_loss.rs`'s own fixture): **9/12 exited 130 (clean), 3/12 exited 101 (uncaught panic)** — a real, present-tense race, not a hypothetical.

To confirm the causal mechanism rather than just the symptom: fifteen further trials kept stderr on a *separate, live* pipe (only stdin/stdout on the pty — the part that actually "goes away"). In that configuration the `eprintln!` fallback succeeds instead of double-faulting, and 14/15 trials printed `Failed to show the cursor: Input/output error (os error 5)` to the live stderr pipe and exited cleanly (0 or 130, depending on which side of the same race won); none panicked. This isolates the defect precisely to the case where stdout *and* stderr are the same dead terminal — the normal, real-world shape of "the terminal has gone away."

`tests/terminal_loss.rs::a_terminal_that_goes_away_ends_the_interface_instead_of_spinning` passes on this tree (`cargo test --test terminal_loss` → ok, 2.13s) but **does not and cannot support "exits cleanly"**: its own comment says so — it asserts prompt exit and low CPU only, explicitly "not asserting an exit code," for exactly this reason. §35: a test whose assertion is silent on exit code would pass identically whether this panic is present or fixed, so it is not evidence for this box's "panic" clause either way.

`crates/glasshouse/.agent-runtime/report-HANGUP-FOLLOWUP.md` (checked immediately before writing this entry, and again now) **does not exist yet** — the `hangup-followup` worker named in this packet has not landed its fix on this tree. The finding above is this worker's own first-hand reproduction, not a restatement of that worker's claim.

Missing evidence: a synthetic in-TUI panic *unrelated* to terminal loss (i.e., on a still-live terminal) was not exercised — there is no reachable CLI path to trigger one without editing source, and none was needed to answer the box's actual open question. The forced-exit path (a genuine second `SIGINT`) is implemented and read, not independently forced.


#### Update — both halves fixed, measured on `HEAD`

Two changes landed after the account above was written, and this entry's own
finding is what routed one of them.

**The uncaught panic (exit 101) is fixed.** `tui::mod`'s `Screen` holds its
`ratatui::Terminal` in `ManuallyDrop` and drops it through
`drop_terminal_tolerantly`, under `catch_unwind`. Ratatui shows the cursor on
drop and `eprintln!`s behind an `.expect` when that write fails; on a terminal
that has gone away the fallback write fails too, and that is the panic. Killed
by a mutation reducing the function to a bare `drop`, re-run by the
orchestrator: `dropping_a_terminal_that_writes_on_drop_does_not_panic ...
FAILED`, restored, `ok`. Exit code through `Screen::drop` after a hangup:
**101 before, 0 after.**

**The exit-130 half was not clean either, and is also fixed.** The entry above
correctly identifies the race — the `ctrlc` handler finding `SHUTDOWN_REQUESTED`
already `true` and calling `force_exit` — and describes that outcome as
"clean". It is controlled, but it is `std::process::exit` with **no destructors
run**, and it was reached by a *single* hangup rather than by the impatient
second Ctrl-C the policy exists for. `install_signal_handler` now counts
signals in a `SIGNALS_SEEN` counter that only it may touch, and the decision is
a named function, `interpret_signal`, so it can be tested; re-applying the old
line as a mutation fails
`a_shutdown_already_requested_elsewhere_is_not_a_second_interrupt`.

Measured end to end by the orchestrator, ten trials each, on a pty **with a
controlling terminal** (`pty.fork`, so `SIGHUP` is genuinely delivered):

| tree | exit codes |
|---|---|
| old line, re-applied as a mutation | `130 130 TIMEOUT 130 130 130 130 130 130 TIMEOUT` |
| shipped | `0 0 0 0 0 0 0 0 0 TIMEOUT` |

**The remaining limit, and it is this box's honest edge.** `TIMEOUT` is a
process that survived the hangup, at `Rs+ 100.0` with cumulative processor time
equal to its whole lifetime — the residual spin, roughly two in sixty across
this measurement and forty further trials. It is not a terminal-restoration
failure, which is what this box claims, and an idle Glasshouse with a live
terminal is `Ss+ 0:00.03 0.3%` over the same interval across twelve trials, so
it is not the harness's own load either. It is tracked as the first item in the
handoff's next-action list, with its reproducing harness kept at
`.agent-runtime/diagnostics/`.

This box is ticked for what it claims — the terminal is restored and the
process exits promptly on normal exit, on interrupt, and on the panic path that
was failing. The process that does not exit at all is a different defect and is
recorded as one.

---
