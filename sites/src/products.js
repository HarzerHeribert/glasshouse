export const products = {
  glasshouse: {
    name: 'Glasshouse', role: 'The orchestrator', action: 'Get the orchestrator',
    title: 'MANY AGENTS.<br>ONE <span class="outline">CLEAR</span><br>VIEW.',
    intro: 'Every session visible. Every worker within reach. You stay in control.',
    description: 'Run the harnesses you already use in sessions you can see and manage. Follow the work, talk to a worker, interrupt it, or pick up where it left off.',
    features: [
      ['Real, visible workers', 'Run Claude Code, Codex, OpenCode, and other installed harnesses in native terminal sessions. Watch, type, interrupt, and resume.'],
      ['Routing that remembers context', 'Choose destinations using measured performance, remaining capacity, and the value of an already-warm session.'],
      ['Memory belongs to the project', 'Keep decisions and their rationale across sessions, with provenance and validity instead of treating every old note as a permanent rule.'],
      ['One project. Hard boundaries.', 'Keep session state and memory scoped to a single project. Isolation is part of the storage and runtime design.'],
      ['A leaner context window', 'Compact tool output before it crosses into the conversation through the context firewall.'],
      ['A single Rust executable', 'Start in your project. No separate service, Node, or Python runtime required by the Glasshouse binary.'],
    ],
    status: 'Active implementation. See the capability map for remaining gates.',
    link: 'https://github.com/HarzerHeribert/glasshouse#build', cta: 'Source & build instructions',
  },
  pane: {
    name: 'Pane', role: 'The harness', action: 'Get the harness',
    title: 'REASON.<br>RUN.<br><span class="outline">CONTINUE.</span>',
    intro: 'A coding harness built around programs over live results.',
    description: 'Let the model prepare the work. Let the runtime carry out the predictable steps. Return to reasoning when the evidence calls for it.',
    features: [
      ['Live objects, smaller conversations', 'Keep tool payloads in the runtime. Let the model work through named handles and bounded previews.', 'In development'],
      ['More work between model calls', 'Prepare guarded continuations in one program. Real results choose the branch; unexpected outcomes yield for fresh judgment.', 'Proposed extension'],
      ['Answers from verified results', 'Finish with a prepared response populated from actual tool data, without another call just to restate the result.', 'Proposed extension'],
      ['Events without the turn storm', 'Bring background completions, messages, and monitors into a bounded batch, with standing handlers for predictable events.', 'In development'],
      ['Your existing project conventions', 'Load project instructions, hooks, permissions, commands, skills, and MCP configuration from the project.', 'Implemented foundation'],
      ['Standalone, or inside Glasshouse', 'Run an independent harness session, or use Glasshouse as the optional gateway and coordination layer.', 'Implemented foundation'],
    ],
    status: 'Pane is under active development. Runtime features and extensions are labeled below.',
    link: 'https://github.com/HarzerHeribert/glasshouse/tree/main/crates/pane', cta: 'Explore Pane source',
  },
};
