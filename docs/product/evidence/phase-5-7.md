# Capability evidence — phase 5-7

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 5/7 — the terminal handshake, and the defect it was hiding

Contract: Given a live harness session, when the harness asks its terminal a
question at startup, Glasshouse answers it, so the harness's own interface
works exactly as it would in a real terminal — and does not quietly degrade
itself, or the user's installation, when it does not.

State: COMPLETE. Phase 7's "Preserve the complete native Claude Code TUI
inside the Glasshouse PTY" box is now checked on the strength of this.

The defect, and how it was found:
- Driving the shipped shell against the real Claude Code 2.1.245, the viewport
  carried the harness's own notice: *"Claude Code's fullscreen renderer has
  repeatedly failed to start on this machine, so it has been turned off
  here."* The user's `~/.claude.json` had gained
  `fullscreenAutoDisabled = {"version": "2.1.245", …, "strikes": 2}`.
- So a Glasshouse session had made a harness permanently change the user's own
  installation, globally — breaking the product invariant that Glasshouse
  operates real harnesses without altering them. Worse, that user's
  `settings.json` reads `"tui": "fullscreen"`: Glasshouse had overridden an
  explicit preference.

The cause:
- A real Claude Code startup was captured in a pseudo-terminal. Of everything
  it writes before drawing, three sequences are *questions*: `ESC[6n`
  (cursor position), `ESC[c` (primary device attributes) and `ESC[>0q`
  (XTVERSION). The rest — bracketed paste, focus reporting, synchronised
  output, keyboard-protocol pushes — are instructions.
- Glasshouse answered exactly one of the three. Phase 5's own design note had
  already stated the rule ("an embedded session must always answer, or the
  harness hangs"); only the cursor-position half was ever built.

The fix:
- `session/runtime.rs: TerminalQuery` / `TerminalQueryScanner` recognise all
  three across chunk boundaries, and `answer_terminal_queries` replies to each:
  the emulated screen's cursor position, `ESC[?1;2c` for device attributes
  (what the viewport actually is, rather than a richer terminal whose sequences
  it could not draw), and its own name for XTVERSION rather than impersonating
  a terminal it is not.

Regression evidence:
- `every_startup_question_a_harness_asks_is_answered` (PTY smoke, Unix) — a
  harness asks all three through a real pseudo-terminal and every reply is
  found in its scrollback. **Mutation-checked three ways**: making either new
  query unrecognisable, or emptying the cursor reply, fails it.
- `a_query_is_found_however_a_read_splits_it`,
  `one_byte_at_a_time_still_finds_every_query`,
  `several_queries_in_one_chunk_are_all_found`,
  `a_near_miss_does_not_count_and_does_not_poison_the_next_match`,
  `a_reply_flowing_back_is_not_mistaken_for_a_question`.

Platform/external evidence (macOS, real Claude Code 2.1.245):
- **Before:** two sessions were enough to trigger `fullscreenAutoDisabled`.
- **After:** three consecutive sessions against an isolated Claude
  configuration left it **absent**, and the failure notice was gone —
  replaced first by Claude Code's *offer* of the fullscreen renderer, and then,
  with `"tui": "fullscreen"` set, by the fullscreen interface itself rendering
  in the viewport with no notice at all.
- The isolated configuration was used precisely so the verification did not
  touch the user's own; it was deleted afterwards.

Missing evidence:
- The user's real `~/.claude.json` still carries the `fullscreenAutoDisabled`
  record this defect caused. Glasshouse will not edit a harness's own
  configuration, so clearing it is the user's to do — `/tui fullscreen` in any
  Claude Code session resets it, and it also resets on the next update.
- Verified on macOS. The queries and replies are platform-independent and the
  tests run everywhere, but no real harness has been driven through the
  viewport on Windows.
