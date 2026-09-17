# Phase 70 — Pane: the network the user configures, and the Windows console

Opened 2026-09-17 by the user's rulings (design-decisions.md, *Pane first, and Pane gets a network*). Work under the in-session rule: forks of the orchestrator's session, no packets beyond a brief, commits by pathspec.

## 2654 — COMPLETE 2026-09-17 (GH-PANE-WINDOWS-VT-INPUT, a fork; the line redefined to what the console can do)

**Contract.** Given Pane's TUI on a Windows console that grants `ENABLE_VIRTUAL_TERMINAL_INPUT`, when the terminal sends a key, a modified key or an SGR mouse report, Pane receives the same `Event` it receives on macOS, independent of the keyboard layout; a console that refuses the flag keeps the record path with the AltGr rule; macOS and Linux input is unchanged.

**Measured first, on the ARM64 VM (German layout), with the flag granted:** Up/Home/End/Delete/F5/Shift-Up → `Key(Up)`, `Key(Home)`, `Key(End)`, `Key(Delete)`, `Key(F(5))`, `Key(Up, SHIFT)`; Ctrl-A/Alt-X → `Key(Char('a'), CONTROL)`, `Key(Char('x'), ALT)`; an SGR click and wheel → `Mouse(Down(Left) 9,4)`, `Mouse(Up(Left) 9,4)`, `Mouse(ScrollDown 100,27)` as mouse records; a bracketed two-line paste → the characters of both lines with `Key(Enter, CONTROL)` between them and **no marker**. So the flag delivers keys and mouse as crossterm's own events with nothing for Pane to parse (crossterm 0.29 `event/sys/windows/parse.rs:245–262`), and the paste half of the line as first written is unreachable on this console under every input mode: the line was redefined to the half that holds; the paste limit is design-decisions.md, *Bracketed paste does not reach the app through ConPTY*.

**Production.** `crates/pane/src/session/ui/console_mode.rs :: select / enable / disable` (the flag set on the input handle after raw mode, cleared on restore only if this process set it; stubs off Windows); `crates/pane/src/session/ui/terminal_input.rs :: Console {Unix, Records, VtInput}`, `TerminalInput::new(Console)`, `reassembles_pastes()`; `crates/pane/src/session/ui.rs :: run` (select after `enable_raw_mode`), `restore_terminal` (disable before `disable_raw_mode`).

**Regression.** `session::ui::terminal_input::tests::raw_terminal_input_reassembles_a_paste_too`; on the VM `tui_live` 21/21 including `a_fragmented_click_report_does_not_become_prompt_text`, `a_report_whose_halves_are_a_third_of_a_second_apart_is_still_not_typed`, `fragmented_mouse_reports_do_not_become_prompt_text`, `a_fragmented_wheel_report_still_scrolls_the_transcript`.

**Mutations.** `vtinput-does-not-reassemble-pastes` (`self.console != Console::Unix` → `== Console::Records`): KILLED by `raw_terminal_input_reassembles_a_paste_too` ("assertion left == right failed at terminal_input.rs:1079 — the paste arrived as typed characters"). `flag-never-set` (`select()` → `Console::host()`): SURVIVED, by design of the fallback — after the AltGr rule the record path passes every live test too; the observable is the trace (mouse records with the flag, characters without), not an assertion. Recorded, not hidden.

**Gates.** Host: `--lib session::ui::terminal_input` 26/26, `--test tui_live` 23/23, clippy clean, blast radius: windows-gnu check clean, rustdoc clean. VM: `--lib session::ui::terminal_input` 26/26, `--test tui_live` 21/21 (the once-session paste guard prints `skipped:` there, as designed). Windows clippy is the `pane (windows-latest)` cell (`cargo-clippy` is not installed on the VM's toolchain).

**Limits.** The paste half is a console limit on this Windows; the two paste guards keep their `skipped:` branches on ConPTY. No test distinguishes the two console modes (the survived mutation); the trace is the evidence.
