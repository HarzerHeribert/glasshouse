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
    name: 'Pane', role: 'The code-mode harness', action: 'Get the harness',
    title: 'REASON.<br>RUN.<br><span class="outline">CONTINUE.</span>',
    intro: 'A coding harness that keeps tool results out of the context window.<br><br>Pane lets the model write TypeScript over live tool results instead of reading every grep, file, test log, and API response as conversation text. The runtime executes the predictable work. The model comes back when judgment is actually needed.',
    description: 'Stop making the model read every tool result. Let it program over them instead.',
    compare: {
      title: 'The model should not be your JSON parser.',
      diagram: `Traditional tool calling            Pane

grep                                grep ─────► live result
 ↓                                                 │
30,000 tokens of results                    filter/map/query
 ↓                                                 │
model                                              ▼
 ↓                                           small preview
filter the results                                 │
 ↓                                               model
another tool call
 ↓
model`,
      body: 'Tool calling turns every intermediate result into model input. Pane keeps those results in the runtime and lets the model write code against them instead.',
    },
    features: [
      ['Tool results stay out of context', 'Large grep results, files, test logs, and API responses stay inside Pane. The model sees small previews and works over the full results through named live objects.', 'In development'],
      ['Do more with each model call', 'One model turn can search, filter, inspect, test, branch on the result, and continue. Pane asks the model again only when the program reaches something that needs judgment.', 'Proposed extension'],
      ['Finish without another inference', 'A program can return the final answer directly from the results it just verified. No extra model call just to say that the tests passed.', 'Proposed extension'],
      ['Events without the turn storm', 'Background jobs, worker messages, and other events arrive in bounded batches. Predictable events can be handled by code instead of waking the model for every update.', 'In development'],
      ['Works with the project you already have', 'Pane loads your existing project instructions, hooks, permissions, commands, skills, and MCP configuration instead of inventing another project format.', 'Implemented foundation'],
      ['Standalone or orchestrated', 'Run Pane directly as its own coding harness, or let Glasshouse provide routing, capacity management, and multi-agent coordination.', 'Implemented foundation'],
    ],
    status: 'Pane is under active development. Runtime features and extensions are labeled below.',
    link: 'https://github.com/HarzerHeribert/glasshouse/tree/main/crates/pane', cta: 'Explore Pane source',
  },
};
