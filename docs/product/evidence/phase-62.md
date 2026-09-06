# Capability evidence — phase 62

Phase 62 — Parallel-session coordination, second slice (map lines 2493–2518), recorded 2026-09-06 from the user's decision on the census's Table 2 (`.agent-runtime/report-refused-lines-census.md`): fourteen parked Maybe lines promoted in ranked order; line 15 of the ranking (read-intent, parked 2081) stays parked until 62A lands. Nothing is built yet. Order of work: after `GH-PANE-61E-WIRE` and the ruler's two-column run (decision of 02:30, `ruling-benchmark-first.md`). Every package here is Amber or Red by the CLAUDE.md table: 62A and 62B touch session lifecycle and claims (Red where a worker is paused or a tree is joined); 62C is Amber advisory; 62D rides the existing `PostToolUse` hook path (Amber); 62E's measurement gate is Green-to-Amber.

## 62A — Queue, override and re-plan around a claimed file

### Consider allowing Glasshouse to queue a worker turn when it would edit a file actively claimed by another session. (line 2493)

State: **NOT STARTED** — promoted from Maybe A (parked line 2101); the census's pitch and mechanism note are the packet's starting point.

### Wake a queued worker automatically when the conflicting claim is released. (line 2494)

State: **NOT STARTED** — promoted from Maybe A (parked line 2104); the census's pitch and mechanism note are the packet's starting point.

### Allow the user to override a conflict warning and let both sessions continue. (line 2495)

State: **NOT STARTED** — promoted from Maybe E (parked line 2111); the census's pitch and mechanism note are the packet's starting point.

### Allow the user to assign reconciliation to a new worker session. (line 2496)

State: **NOT STARTED** — promoted from Maybe E (parked line 2117); the census's pitch and mechanism note are the packet's starting point.

### Allow the orchestrator to instruct one worker to work on tests, documentation, or analysis while another owns the conflicting implementation file. (line 2497)

State: **NOT STARTED** — promoted from Maybe H (parked line 2143); the census's pitch and mechanism note are the packet's starting point.

## 62B — Convergent co-editing of one contended file

### Consider letting two sessions work on the same file concurrently in isolated buffers, rather than serializing them behind a claim, when both genuinely need it. (line 2501)

State: **NOT STARTED** — promoted from Maybe L (parked line 2161); the census's pitch and mechanism note are the packet's starting point.

### Escalate to the orchestrator or the user, with both versions visible, when reconciliation cannot preserve both intents. (line 2502)

State: **NOT STARTED** — promoted from Maybe L (parked line 2182); the census's pitch and mechanism note are the packet's starting point.

## 62C — Session drift and rework, advisory first

### Experiment with detecting when an active agent session appears to be compounding an invalid premise, drifting from the requested task, or repeatedly repairing its own avoidable changes. (line 2506)

State: **NOT STARTED** — promoted from Maybe K (parked line 2277); the census's pitch and mechanism note are the packet's starting point.

### Start with a quiet session marker and concise evidence summary rather than interrupting the agent immediately. (line 2507)

State: **NOT STARTED** — promoted from Maybe K (parked line 2298); the census's pitch and mechanism note are the packet's starting point.

## 62D — In-turn repository diagnostics

### Use deterministic repository diagnostics to catch newly introduced mechanical errors before they survive until a CI run. (line 2511)

State: **NOT STARTED** — promoted from Maybe J (parked line 2202); the census's pitch and mechanism note are the packet's starting point.

### Allow PostToolUse diagnostics to be returned as concise model-visible feedback so the same agent can repair newly introduced problems. (line 2512)

State: **NOT STARTED** — promoted from Maybe J (parked line 2216); the census's pitch and mechanism note are the packet's starting point.

## 62E — Prediction, read visibility and the measurement gate

### Allow the router to use task plans, touched-file history, Git diffs, and current claims as conflict-prediction inputs. (line 2516)

State: **NOT STARTED** — promoted from Maybe C (parked line 2093); the census's pitch and mechanism note are the packet's starting point.

### Prefer allowing reads of the last committed or current filesystem state with an explicit stale-or-changing warning before implementing hard read blocking. (line 2517)

State: **NOT STARTED** — promoted from Maybe G (parked line 2132); the census's pitch and mechanism note are the packet's starting point.

### Measure how often parallel sessions actually produce overlapping file edits before enabling automatic file coordination by default. (line 2518)

State: **NOT STARTED** — promoted from Maybe I (parked line 2149); the census's pitch and mechanism note are the packet's starting point.

