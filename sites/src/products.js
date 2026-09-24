// Every claim here is true of the shipped code or was measured; the source of
// each is named beside it so a later edit can re-check it rather than trust it.

export const repo = 'https://github.com/HarzerHeribert/glasshouse';
export const installCommand = 'curl -fsSL https://harzerheribert.github.io/glasshouse/install.sh | sh';

// The braille songbird from crates/pane/src/workbench/voice.rs (IDLE, FLAP).
export const bird = ['    ⡔⠩⠉⠢⣀⣀', '⡠⠒⢤⠎ ⣀⣀⡀⠑⡄', '⠑⢤⠊⡰⠉  ⠈⡢⠃', '  ⠑⠣⡄⡀⡤⠊  '];
export const flap = ['⠑⠤⠊', '⠢⠤⠔', '⠤⠤⠤', '⠔⠒⠢'];

export const pane = {
  // A cell from the 2026-09-24 X1 run at shipped defaults (scratchpad
  // h2h2-pane/X1-pane-1.jsonl, cell 1), trimmed to two calls; the previews
  // are the handle previews that run returned, shortened with an ellipsis.
  example: {
    cell: `const [arch, hits] = await Promise.all([
  read({path: 'docs/product/architecture.md'}),
  rg({pattern: 'pane ruler run|run_attempt', path: 'crates'}),
]);
return {hits: hits.slice(0, 60)};`,
    back: `arch  File  "docs/product/architecture.md"
      3745 B · 73 lines
      L1  "# Architecture"
hits  Grep.Match[]  n=11
      [0] "crates/pane/src/ruler/cli.rs:1  //! \`pane ruler run\`: parse …"
      [1] "crates/pane/src/ruler/cli.rs:26  /// The whole accepted flag …"
      …`,
  },
  // tests/handles.rs::a_grep_of_122kb_costs_under_300_tokens_and_survives_one_yield;
  // cache: the gateway's per-request records for the 2026-09-24 re-run at shipped
  // defaults (81–91 % per task, 182 requests); facts: X1 in that re-run, 3 of 3.
  facts: [
    ['209', 'tokens to show the model 275 KB of grep output. A test regenerates the tree on every run and fails above 300.'],
    ['81–91 %', 'of each task’s input came from the provider’s prompt cache: history only grows, so each request extends the last one byte for byte.'],
    ['8 of 8', 'facts found on the explore task in every attempt, against 7.3 on average for Codex on the same model.'],
  ],
  features: [
    ['A terminal workbench', 'Cells, results and the model’s reasoning stream in as they happen, with context and cache use in the status line. Resume any session with --continue or --resume.', 'tests/tui_live.rs'],
    ['Reads your existing setup', 'Reads AGENTS.md and CLAUDE.md (root, nested and global), the permissions and hooks in .claude/settings.json, and the MCP servers in .mcp.json.', 'tests/scoped_instructions.rs'],
    ['Build, explore and plan modes', 'Three working modes: build edits and runs commands, explore only reads, plan reads and writes the plan file alone. Start with --plan, or switch with /mode.', 'tests/request_modes.rs'],
    ['Four permission levels', 'Four permission rungs — manual, accept-edits, auto and full. Shift-Tab cycles them; a rung changes how often you are asked, never what may be granted. --full-access is one flag for a trusted machine.', 'tests/approval_boundary.rs'],
    ['Subagents and background jobs', 'agent.run hands a goal to a separate turn loop and returns at once; bg.run keeps a build or a watcher going. Results arrive as one batched event, not a turn each.', 'tests/subagent.rs'],
    ['Compaction keeps the runtime', 'When the conversation stops fitting, redundant parts go first and then a checkpoint replaces it — while the runtime keeps running, so a result from turn three is still addressable.', 'tests/prompt_bytes.rs'],
    ['Rollback that keeps your edits', '/rollback previews what the session changed and restores it, leaving edits you made yourself in place.', 'crates/pane/src/changes.rs'],
    ['An OS sandbox', 'Model-written code runs under Seatbelt on macOS and Landlock with seccomp on Linux, with grants compiled from your .claude/settings.json.', 'tests/sandbox_apply.rs'],
    ['Subscriptions or API keys', 'An API key, or a ChatGPT or Claude subscription connected with /login. Switch models mid-session with /models.', 'crates/pane/src/session/controls.rs'],
    ['A daily update check', 'A release install checks for a newer release once a day and installs it beside the running one. pane update does it on demand.', 'crates/pane/src/update.rs'],
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
  // [iso day, label, what was measured, result, kind] — kind is the direction
  // for Pane: 'setback', 'progress' or 'mixed'; shape and word carry it, not colour.
  ['2026-09-06', 'Sep 6', 'Pane vs Claude Code · 3 script repairs · 1 attempt each · DeepSeek V4 Flash', 'Claude Code as good or better wherever anything worked; neither fixed 2 of the 3.', 'setback'],
  ['2026-09-07', 'Sep 7', 'Pane vs Claude Code · same prompt, fixture and checker · 2 trials · Claude Sonnet 4.6', 'Claude Code 2 of 2. Pane 0 of 2 on its own; it passed only after a hint.', 'setback'],
  ['2026-09-13', 'Sep 12 → 13', 'Terminal-Bench 2.0 · 4 tasks × 3 · GPT-5.6', '9 of 12 → 12 of 12 after the interface change — at about 40 % more tokens.', 'progress'],
  ['2026-09-14', 'Sep 14', 'Terminal-Bench 2.0 · full suite · 1 attempt each', 'Paused after 37 of 89 tasks: 18 of 37 passed.', 'mixed'],
  ['2026-09-17', 'Sep 17', 'A hard task (build an ssh tool) · 1 attempt, twice', 'The first run did not compile; the second compiled and passed its tests but was not finished — at 20.2M tokens, up from 11.8M.', 'mixed'],
  ['2026-09-23', 'Sep 23 → 24', 'Ruler fix and explore tasks · 3 attempts each · GPT-6', 'Every attempt passed. Token use moved within the noise: the spread inside one setup (150–190k) is larger than any change between setups.', 'mixed'],
  ['2026-09-24', 'Sep 24', 'Prompt cache fixed', 'The main model’s cache reads went from 9–35 % to 90 %. Every GPT token figure above predates this.', 'progress'],
  ['2026-09-24', 'Sep 24', 'Pane vs Codex · 4 tasks × 3 · GPT-6 · Pane holding each run open for its completion check, provider-default effort', 'Same results; Pane fewer tokens on three of four; Codex faster on all four (191 / 137 / 90 / 381 s against 77 / 73 / 55 / 164). The check changed no outcome, so it no longer holds a one-task run, and GPT now runs at low effort by default.', 'setback'],
  ['2026-09-24', 'Sep 24', 'Pane vs Codex again · the same 4 tasks × 3 · Pane at its new defaults', 'Same results; 18 % fewer tokens, as many uncached; Codex 1.28× faster overall (473 s against 369), Pane faster on the two small tasks.', 'mixed'],
];

// Pane vs Codex, 2026-09-24, pane ruler: same tasks, gpt-6-sol medium, 3
// attempts each (scratchpad h2h-pane, h2h-codex, codexbase attempts.jsonl;
// Codex tokens from its own session logs, cached included).
// Pane vs Codex, 2026-09-24, pane ruler: same tasks, gpt-6-sol, 3 attempts
// each. Pane at its shipped defaults (6fc97dc7; scratchpad h2h2-pane, tokens
// from the gateway's per-request records inside each attempt's span). Codex
// CLI at medium (h2h-codex, codexbase; tokens from its own session logs).
// time: mean seconds launch to exit, with the task's test run; tokens and
// uncached: mean thousands per attempt, cached included in tokens.
export const benchmark = {
  verdict: 'Same results. Pane used 18 % fewer tokens overall and as many uncached ones. Codex was 1.28× faster overall: Pane was faster on the two small tasks and slower on the two larger edits.',
  tasks: [
    { name: 'Fix a failing test', pane: { passed: 3, time: 134, tokens: 452, uncached: 49 }, codex: { passed: 3, time: 77, tokens: 563, uncached: 43 } },
    { name: 'Explore and explain', pane: { passed: 3, time: 71, tokens: 312, uncached: 45, facts: 8.0 }, codex: { passed: 3, time: 73, tokens: 398, uncached: 68, facts: 7.3 } },
    { name: 'Implement a small feature', pane: { passed: 3, time: 53, tokens: 124, uncached: 23 }, codex: { passed: 3, time: 55, tokens: 239, uncached: 25 } },
    { name: 'Rename across files', pane: { passed: 3, time: 215, tokens: 1091, uncached: 97 }, codex: { passed: 3, time: 164, tokens: 1206, uncached: 76 } },
  ],
  metrics: [
    { key: 'time', label: 'Time', unit: 's', lower: true, note: 'Mean seconds from launch to exit, including the task’s test run.' },
    { key: 'tokens', label: 'Tokens', unit: 'k', lower: true, note: 'Mean tokens per attempt, cached included: everything Pane’s gateway served (main model, helpers, the Jev classifier) against Codex’s own session logs.' },
    { key: 'uncached', label: 'Uncached tokens', unit: 'k', lower: true, note: 'The part of each attempt’s input the prompt cache did not serve — the part that weighs most on a subscription’s limits.' },
    { key: 'passed', label: 'Passed', unit: ' of 3', lower: false, note: 'Attempts whose test (or, for the explore task, eight-fact rubric) passed. Every attempt of both passed.' },
  ],
  notes: 'Pane at its shipped defaults, Codex CLI at medium effort, same model (GPT-6 Sol). Codex ran in its workspace-write sandbox, Pane with full access. Three attempts per cell: direction, not proof.',
  check: 'Where Pane’s time goes: its model time per turn is equal or lower than Codex’s; the gap is commands. Codex’s long commands keep running while its model goes on; a Pane cell waits for each one. Handing long commands to background jobs was built and measured: the rename got slower (215 → 253 s) because the model spent turns collecting its jobs, so it was reverted.',
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
