# Pane native workbench — 2026-09-20 cutover

This is the current native presentation and interaction contract. It supersedes
older **presentation** decisions about poster cells, muted helper lanes, a flat
settings form, model staging and local notices interleaved with the conversation.
Those documents remain historical evidence, not competing implementation targets.
It does not supersede the sandbox, approval gate, credential boundaries or the
three-component architecture.

## Runtime connection, not another mockup

`session/ui.rs` draws `workbench::render` and routes ordinary events through the
new reducer. `workbench/{document,view,input,settings,models,theme}.rs` own the new
presentation; the old renderer is not a live fallback and has no feature switch.
The retained `tui` record types, selection/path utilities and catalogue DTOs are
inputs, not a second layout. No HTML or browser is needed to run this interface.

Conversation, cell execution, local controls and permission prompts are separate
surfaces. The model's public explanation and returned answer stay in the
conversation. Authored cells show their actual source; host-lowered frames are
identified. Local notices have an Activity log. Prompting, approvals and masked
credential input keep precedence over every ordinary UI action.

## Interaction and appearance

F2 opens categorized settings; F3 opens the model navigator; F4 opens the selected
cell's observed before/after diff; F5 opens its helpers. Ctrl-O expands the cell;
Ctrl-T opens local Activity. Shift-Tab steps the **Ask** rung in place. `?` on an
empty composer opens the sheet of keys. All controls have keyboard routes.

**Four regions, one component, a grammar and a character (2026-09-22, the
application pass; `workbench-ux.md` carries the argument).** The top bar, the
conversation, the session card and the composer dock are drawn as regions with
lines, never with paint. Every control on every surface is a chip, `⟨ label ⟩`,
filled when it is the current choice of a set and for the frames a press rests
on it; the bar drops chips by rank as the terminal narrows and never drops a
warning-toned one. The conversation is turns: the person's under a coloured bar
labelled `you`, Pane's under its mark; a cell is a card with its state in the
top edge and how it ended in the bottom edge; helpers hang under the card;
notices are tagged rows; a finished turn ends in an answer block with a stats
line and, on the latest turn, chips for what to do next. The composer dock's
top edge carries the live status, a notice for a few seconds and an undo chip
for the last dock change; its bottom edge carries the three everyday chips, one
rotating hint and the context reading. Pane's character is a small braille bird
with six states read off the session's activity, and `ui.voice = playful | plain`
chooses between its first-person copy and the plain statement of the same
facts; structure never changes with the voice. The opening greets by project and
local hour and offers chips read from the repository (last commit, uncommitted
changes, a test command), each of which types a message. Inside an open cell
the program's acting calls are lit and the chain of calls the cell actually
made is listed from its record with how each ended; a cell still being
written is shown one row per action with a ticking character count
(`ui.stream = actions | code | raw`).
An approval offers `[a]`, ask Pane for another way: the call is refused as a
denial is and the words reach the program as the refusal's rule.

Clicks activate on release. A drag selects/copies text instead of opening a cell
or path. Wheel events go to the open local surface or to the conversation, not
both. The composer and its draft remain in place. Reading older work stops following
new output; Latest returns to the live edge. Reader anchors preserve content across
reflow. Existing paths are underlined and open on an explicit click.

Foreground roles are normal/code (terminal default, readable), accent (current
operation/selection), failure, warning, success and muted technical detail. Normal
prose and returned helper evidence are not muted. Every new surface keeps the
terminal-default background, including settings, model lists and selection chrome.
The terminal owns opacity and blur. Reduced motion freezes decoration, not real
state or elapsed observations. Unknown helper completion stays unknown.

Code, Diff, Output and Helpers belong to their cell. The diff explicitly compares
**before/after this cell**, not HEAD and not proposed edits awaiting approval.
Missing captured bytes are reported rather than treated as no changes. The current
native diff is unified; the mockup's working-tree comparison and side-by-side view
are not implemented in this cutover.

## Direct preferences

Settings are Workspace, Display, Little helpers, Models & accounts, Subagents and
Advanced. Ordinary categories contain a bounded choice set. Advanced exact-key
search reaches the typed native registry. Model values open a navigator rather
than an unbounded choice carousel. A settings-originated model selection retains
its selected scope and does **not** mutate the running model.

A completed field choice validates and saves immediately through `settings::Store`.
There is no staged basket or batch Apply. Esc closes; Undo restores the previous
single operation atomically, including dependent keys. Confirmation of an individual
safety-related field is not a staged transaction. Viewing creates no files.

Global/project/profile precedence, conflicts, global denials and atomic native
writes remain authoritative. Saved UI changes apply to the edited field now;
saved runtime/security changes require a new session. An unrelated UI save must
not reset an explicit live motion override. Live model changes use the existing
between-turn path, not a rewrite of in-flight records.

## Model navigator and delegation policy

Main, Helpers and Subagents remain distinct. The default navigator excludes known
unavailable accounts and prioritizes connected subscription entries. Missing
credentials and unauthenticated subscriptions are not offered as usable routes.
All sources is explicit. Search accepts spaces. Enter commits one selection.
Measured AA intelligence comes from the gateway first, with the older optional
Glasshouse cache as fallback. Missing measurements remain unknown; a score does
not establish price, entitlement or task suitability.

**Route ownership is unchanged.** The account row describes catalogue availability;
selecting it currently assigns its model, not that provider/account. The gateway
chooses the route. Exact provider/account/entitlement affinity and a no-API-spend
fallback restriction need a versioned gateway request contract and are NOT claimed
by this implementation. Do not infer those constraints from a highlighted row.

Delegation defaults to **off**, never Main. Legacy `agents.mode = "auto"` still
loads for migration but refuses launches and produces a startup notice. An existing
concrete `agents.model` remains a pinned assignment. Pinned means enforced: neither
a template nor `agent.run` may select another model.

Four optional favorites are `quick`, `balanced`, `deep`, `heavy`:

```toml
[agents]
mode = "roster"
[agents.slots.quick]
model = "your-concrete-model"
effort = "low"
[agents.slots.deep]
model = "another-concrete-model"
effort = "high"
```

Fill slots while delegation is off, then enable explicitly. Defaults per populated
slot are low/medium/high/max; these are user-configurable hard efforts, not quality
scores. Empty slots never inherit. The caller chooses a configured slot with
`agent.run(task, {slot: "quick"})`. An explicit model must match it, and an explicit
effort cannot override it. Model-only selection is accepted only if unambiguous.
No unauthorized launch may reach a background thread or provider request.

Human commands: `/subagents quick MODEL [EFFORT]`, `/subagents quick off`, and
`/subagents on|off`. A slot edit saves to the selected profile and applies to the next
launch. Removing the last active favorite switches delegation off. Already running
jobs retain their captured configuration. Helpers remain a separate read-only tier.

## Verification and remaining acceptance

Native assertions are in `tests/workbench.rs`, `tests/delegation_policy.rs`, the
`session::controls::subagents` tests, and `workbench_*` in `tests/tui_live.rs`.
They exercise the shipped Ratatui/input/store/V8 boundaries, not browser fixtures.
The PTY cases spawn the real `pane` binary against a loopback scripted provider.
The root container lacks working Landlock/seccomp confinement; tests requiring it
cannot be treated as acceptance of a real Linux desktop sandbox. Native Ghostty,
macOS and Windows need their own acceptance runs. The old TUI live expectations
and broad agent fixtures must be migrated where they assert superseded behavior;
a passing new targeted suite is not a claim that the complete workspace is green.
