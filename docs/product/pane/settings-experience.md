# Pane settings experience

Decision accepted and implementation verified 2026-09-12.

## Ownership and scope

Pane owns its configuration. Compatibility does not make another harness's
files authoritative.

| Settings | Destination |
|---|---|
| Global defaults, including permissions | `$XDG_CONFIG_HOME/pane/config.toml`, otherwise `~/.config/pane/config.toml` |
| Project overrides, including permissions and named profiles | `<project>/.pane/config.toml` |
| Global instructions (not structured settings) | The same user configuration directory's `AGENTS.md` |
| Legacy Pane configuration | `.glasshouse/pane.toml`: read-only fallback until explicit migration |
| Claude permission compatibility | `.claude/settings.json`: explicit import only; never written by Pane |

Project means the launch folder or explicit `--root`, with or without Git.
There is no upward search. Global means this OS user, not administrator settings.

Ordinary precedence: built-in → global → project → selected named profile →
explicit session/CLI override. Named profiles are project-local runtime
overlays. The tabs edit base scopes and warn about profile masking.
`/models` remains the live model-assignment path, writing through the native
store into the active profile when selected.

## Everyday settings

`/settings` opens a Pane-themed panel with Global/Project tabs, destination
path, saved/effective values and origins. Curated controls cover model tiers,
reasoning effort, working mode, helper completion, theme, sidebar, status-line
layout and reduced motion. Runtime ceilings, per-helper tuning, endpoints and
permission patterns remain advanced.

Arrows navigate/cycle choices; Enter edits validated text if no catalog choices
are available. Backspace removes a scoped override (inheritance). Ctrl-S applies;
Escape cancels without creating files. Dirty edits must be applied or cancelled
before changing scopes. Provider `default` effort is distinct from inheritance.

Panel presentation changes apply immediately; runtime and permission changes
require a new session. Use `/models` for live model changes.
Bare `/statusline` opens the scoped layout picker with a textual preview.
`/statusline full|compact|hide` persists to project configuration and applies
immediately; `hidden` is also accepted. Shell-script status lines are out of scope.

## Advanced configuration

```text
/config [global|local] <key> <value>
pane config [global|local] <key> <value>
pane config --root /path/to/project local ui.theme amber
pane config global session.effort high
pane config local --unset session.effort
pane config --help
```

The default scope is local; project is an alias. No key shows effective settings
and origins; a key without a value reads it. `/status` retains the session
summary. CLI configuration needs no model, gateway or terminal. Slash saves
report that a new session is required; use the panel for immediate presentation.

One typed registry backs files, commands and panel choices. Unknown keys,
invalid values and inconsistent tier combinations are rejected. Strings need
no TOML quoting; arrays use typed list syntax. Values are never shell-expanded
or executed. Credentials do not belong here. These are user actions, not
model-callable tools for changing their own authority.

## Migration and permissions

```text
pane config import legacy
pane config import legacy --apply
pane config import claude
pane config import claude --apply
```

Imports preview without writing; `--apply` is explicit. Sources are preserved.
Legacy/native disagreement is a visible conflict; explicit migration records
native authority. Claude import handles supported allow/deny rules, not hooks,
environment or credentials. Rejected permission rules block applying, so
unsupported denials cannot disappear while allows are imported.

Global denials survive project overlays and `--yolo`; hard-denied resources
remain denied. Saved permissions never widen the running session.

The store preserves unrelated TOML/comments, validates before writing, checks
stale snapshots, and uses exclusive temporary files with atomic replacement.
Unix writes use directory-relative no-follow operations against symlink swaps.
Viewing creates no files; failed saves leave live state unchanged.

**Linux limitation:** active Landlock filesystem confinement cannot carve
`.pane` out of a writable project for arbitrary admitted subprocesses. Such a
shell can modify settings for a future session. Direct file-tool denials and
immutable live snapshots do not replace OS isolation. Startup warns; sandbox
parity stays partial until the active spawn path gains mount-view exclusions
or an equivalent. The bubblewrap argv builder alone is not active enforcement.
macOS has native exclusions; Windows policy has structural coverage, not a
native acceptance run here.

## Evidence

Implementation: `settings.rs`, `settings/registry.rs`, `settings_commands.rs`,
`settings_ui.rs`, `settings_session.rs`, and session/sandbox integration.

Regressions: `tests/settings_store.rs`, `tests/settings_commands.rs`,
`tests/settings_ui.rs`, `tests/settings_security.rs`, and real-PTY settings
cases in `tests/tui_live.rs`. Coverage includes layering, profiles, inheritance,
migration, invalid/no-write cases, comments, concurrency, path safety, scoped
navigation, cancel, preview and persistence. Code and tests were written before
the combined gate. Verification:

- Full workspace Nextest audit: 5,524 tests across 246 binaries, initially
  5,519 passing and five failing; six existing manual/live tests skipped.
- The five failures were corrected: the launch fixture needed a concrete
  native model; the installed Codex catalogue observation needed refreshing;
  the short-terminal layout needed to reserve a visible row; and two existing
  output/count expectations needed reconciling.
- Post-correction gate: all 315 tests in 20 affected integration targets passed,
  including the five failed targets, all 23 live-terminal cases, settings,
  sandbox, web, approvals, profiles, images, and machine output. This is a
  correction gate, not a claim that a second full-workspace run was performed.
- Native Linux aarch64/non-root: 69 tests passed, including 29 store tests and
  the explicit Landlock exclusion limitation regression.
- Workspace Clippy with warnings denied, formatting, diff checks, and all five
  documentation tests passed.

Local evidence logs: `/tmp/pane-settings-nextest.log`,
`/tmp/pane-settings-corrections.log`, `/tmp/pane-settings-linux-final.log`,
`/tmp/pane-settings-clippy-final.log`, and `/tmp/pane-settings-doctests.log`.
The Nextest processes used a 4,096-descriptor limit: a separate initial
standard-run database stress failure at the host's 256 limit passed unchanged
with the raised limit, including in the full Nextest run.

The current project's legacy migration was also previewed successfully without
applying it. Existing configuration files are not migrated by installation.
