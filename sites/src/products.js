// Every claim here is true of the shipped code or was measured; the source of
// each is named beside it so a later edit can re-check it rather than trust it.

export const repo = 'https://github.com/HarzerHeribert/glasshouse';
export const installCommand = 'curl -fsSL https://harzerheribert.github.io/glasshouse/install.sh | sh';

// The braille songbird from crates/pane/src/workbench/voice.rs (IDLE, FLAP).
export const bird = ['    ⡔⠩⠉⠢⣀⣀', '⡠⠒⢤⠎ ⣀⣀⡀⠑⡄', '⠑⢤⠊⡰⠉  ⠈⡢⠃', '  ⠑⠣⡄⡀⡤⠊  '];
export const flap = ['⠑⠤⠊', '⠢⠤⠔', '⠤⠤⠤', '⠔⠒⠢'];

export const pane = {
  compare: {
    title: 'The model should not be your JSON parser.',
    diagram: `Tool calling          Pane

grep                  grep
 ↓                     │
every match, as text  one live result
 ↓                     │
model reads it all    filter / map / count
 ↓                     │
another tool call     a small preview
 ↓                     ↓
model reads it again  model`,
    body: 'A tool-calling agent turns every intermediate result into conversation text, and pays for it again on every later turn. In Pane a result is a named object in a V8 runtime: the model gets a bounded preview and a handle, and writes TypeScript that works over the whole thing.',
  },
  // tests/handles.rs::a_grep_of_122kb_costs_under_300_tokens_and_survives_one_yield;
  // the gateway's per-request records for a live F1 session, 2026-09-24 (355,328 of 394,961).
  facts: [
    ['209', 'tokens to show the model 275 KB of grep output. A test regenerates the tree on every run and fails above 300.'],
    ['90%', 'of the main model’s input came from the provider’s prompt cache in a measured session: history only grows, so each request extends the last one byte for byte.'],
    ['1', 'line to install. No daemon, no Node, no Python: native binaries, checked against the release’s checksums.'],
  ],
  features: [
    ['A terminal workbench', 'Cells, results and the model’s reasoning stream in as they happen, with context and cache use in the status line. Resume any session with --continue or --resume.', 'tests/tui_live.rs'],
    ['Works with the project you have', 'Reads AGENTS.md and CLAUDE.md (root, nested and global), the permissions and hooks in .claude/settings.json, and the MCP servers in .mcp.json. Nothing to convert.', 'tests/scoped_instructions.rs'],
    ['Plan, then build', 'Three working modes: build edits and runs commands, explore only reads, plan reads and writes the plan file alone. Start with --plan, or switch with /mode.', 'tests/request_modes.rs'],
    ['Asks as often as you want', 'Four permission rungs — manual, accept-edits, auto and full. Shift-Tab cycles them; a rung changes how often you are asked, never what may be granted. --full-access is one flag for a trusted machine.', 'tests/approval_boundary.rs'],
    ['Subagents and background jobs', 'agent.run hands a goal to a separate turn loop and returns at once; bg.run keeps a build or a watcher going. Results arrive as one batched event, not a turn each.', 'tests/subagent.rs'],
    ['Survives a full context', 'When the conversation stops fitting, redundant parts go first and then a checkpoint replaces it — while the runtime keeps running, so a result from turn three is still addressable.', 'tests/prompt_bytes.rs'],
    ['Undo that respects your edits', '/rollback previews what the session changed and restores it, leaving edits you made yourself in place.', 'crates/pane/src/changes.rs'],
    ['An OS sandbox under the code', 'Model-written code runs under Seatbelt on macOS and Landlock with seccomp on Linux, with grants compiled from your .claude/settings.json.', 'tests/sandbox_apply.rs'],
    ['Your subscription or your key', 'An API key, or a ChatGPT or Claude subscription connected with /login. Switch models mid-session with /models.', 'crates/pane/src/session/controls.rs'],
    ['Keeps itself current', 'A release install checks for a newer release once a day and installs it beside the running one. pane update does it on demand.', 'crates/pane/src/update.rs'],
  ],
  limits: [
    ['Pre-release', 'Version 0.1.0 pre-releases. Expect rough edges and say so in an issue.'],
    ['macOS and Linux', 'Apple silicon Macs, and Linux on x86_64 and arm64. Windows archives are published with each release; a Windows installer is not written yet.'],
    ['Web search needs an endpoint', 'web.fetch works out of the box. web.search needs a SearXNG-compatible endpoint you configure.'],
    ['No IDE integration', 'Pane lives in the terminal. There is no editor plugin.'],
    ['Source available', 'The source is public to read and review. It is not open source: all rights are reserved.'],
  ],
};

// Every measured run of Pane, in order, setbacks included. Sources: the
// benchmark-history compilation of 2026-09-24 (.agent-runtime/benchmarks,
// ~/projects/pane-benchmarks, docs/product/pane/helper-measurements.md,
// docs/process/dogfooding.md). n is small everywhere; say so.
export const history = [
  ['Sep 6', 'Pane vs Claude Code · 3 script repairs · 1 attempt each · DeepSeek V4 Flash', 'Claude Code as good or better wherever anything worked; neither fixed 2 of the 3.'],
  ['Sep 7', 'Pane vs Claude Code · same prompt, fixture and checker · 2 trials · Claude Sonnet 4.6', 'Claude Code 2 of 2. Pane 0 of 2 on its own; it passed only after a hint.'],
  ['Sep 12 → 13', 'Terminal-Bench 2.0 · 4 tasks × 3 · GPT-5.6', '9 of 12 → 12 of 12 after the interface change — at about 40 % more tokens.'],
  ['Sep 14', 'Terminal-Bench 2.0 · full suite · 1 attempt each', 'Paused after 37 of 89 tasks: 18 of 37 passed.'],
  ['Sep 17', 'A hard task (build an ssh tool) · 1 attempt, twice', 'The first run did not compile; the second compiled and passed its tests but was not finished — at 20.2M tokens, up from 11.8M.'],
  ['Sep 23 → 24', 'Ruler fix and explore tasks · 3 attempts each · GPT-6', 'Every attempt passed. Token use moved within the noise: the spread inside one setup (150–190k) is larger than any change between setups.'],
  ['Sep 24', 'Prompt cache fixed', 'The main model’s cache reads went from 9–35 % to 90 %. Every GPT token figure above predates this.'],
];

// Pane vs Codex, 2026-09-24, pane ruler: same tasks, gpt-6-sol medium, 3
// attempts each (scratchpad h2h-pane, h2h-codex, codexbase attempts.jsonl;
// Codex tokens from its own session logs, cached included).
export const benchmark = {
  verdict: 'Same results. Pane used fewer tokens on three of the four tasks. Codex was faster on all four.',
  head: ['Task', 'Pane', 'Pane, low effort', 'Codex'],
  rows: [
    ['Fix a failing test', '3/3 · 191 s · 367k', '3/3 · 172 s · 377k', '3/3 · 77 s · 563k'],
    ['Explore and explain', '3/3 · 137 s · 330k · 7.3 of 8 facts', '3/3 · 136 s · 331k · 7.7 of 8', '3/3 · 73 s · 398k · 7.3 of 8'],
    ['Implement a small feature', '3/3 · 90 s · 164k', '3/3 · 57 s · 105k', '3/3 · 55 s · 239k'],
    ['Rename across files', '3/3 · 381 s · 1.81M', '3/3 · 313 s · 1.05M', '3/3 · 164 s · 1.21M'],
  ],
  notes: 'Passed · mean time from launch to exit, including the task’s test run · mean tokens, cached included (Pane’s main model; its helpers add about 12 %). Pane waits for its own completion check after answering, 15–60 s of each time. Codex ran in its workspace-write sandbox, Pane with full access. Three attempts per cell: direction, not proof.',
};

export const glasshouse = {
  intro: 'Glasshouse runs several coding-agent sessions side by side — Pane, Claude Code, Codex, OpenCode — as real terminal sessions you can watch and type into, and gives them one view: memory that belongs to the project, delegation between sessions, and a warning when two sessions head for the same file.',
  status: 'Preview. Glasshouse runs, and it is where Pane came from, but it is not at Pane’s readiness: several of its parts are still moving and its setup is for people who read the source. If you want one agent that works today, use Pane.',
  features: [
    ['Real, visible sessions', 'Each session is the real installed harness, launched in its own profile and driven over a terminal. You can watch it, type into it, interrupt it and resume it.', 'Preview'],
    ['Memory that belongs to the project', 'Decisions and their reasons carry across sessions with their provenance and age, instead of every old note becoming a permanent rule.', 'Preview'],
    ['Delegation between sessions', 'An orchestrating session hands work to other first-class sessions and hears back from them. Every one of them stays a session you can see.', 'Preview'],
    ['File coordination', 'Sessions announce what they are about to edit; when two head for the same file, the orchestrating session hears about it and re-plans only that part.', 'Preview'],
    ['One project, hard boundaries', 'Sessions, memory, logs and runtime state are scoped to one project root. Cross-project access is disabled by construction.', 'Preview'],
    ['Built on the same gateway', 'The inference gateway that serves Pane — keys, subscriptions, protocol translation — is the one Glasshouse sessions use.', 'Shared with Pane'],
  ],
};
