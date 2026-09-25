// The home page: what a person gains from Pane, shown on Pane itself.
// Every figure is from docs/product/pane/helper-measurements.md §9 (Pane vs
// Codex, 2026-09-24/25, gpt-6-sol, 3 attempts per task) or a named test; the
// replay is the tally session of docs/images/readme/pane-*.svg, trimmed.

const measurements = 'https://github.com/HarzerHeribert/glasshouse/blob/main/docs/product/pane/helper-measurements.md#9-pane-vs-codex-and-a-command-that-yields-2026-09-24-gpt-6-sol-n--3';

const e = (text) => text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

// One replayed line: its markup, the pause before it, and the stage it moves
// the step bar to. `type` lines are typed out, as cells stream in.
const replay = [
  { stage: 0, wait: 500, html: `<div class="t-you"><span class="t-bar">┃</span><b>you</b></div><div class="t-you"><span class="t-bar">┃</span>tests/test_tally.py fails. Find the cause and fix it without weakening the test.</div>` },
  { stage: 0, wait: 900, html: `<div class="t-dim">· decision: modify · needs exploration · 584 ms</div>` },
  { stage: 0, wait: 500, html: `<div class="t-pane"><span class="t-flap">⠿</span> pane</div>` },
  { stage: 0, wait: 900, html: `<div class="t-cell"><span class="t-dim">▸ 001 ·</span> I’ll run the failing test and inspect its implementation <span class="t-ok">✓ EXECUTED</span></div>` },
  { stage: 0, wait: 1100, html: `<div class="t-cell"><span class="t-dim">▸ 002 ·</span> I’ll read <code>spread</code> before fixing its empty input <span class="t-ok">✓ EXECUTED</span></div>` },
  { stage: 1, wait: 700, html: `<div class="t-box-top"><span>╭─ 003 ·</span> writing cell <span class="t-spin">⠇</span></div>` },
  {
    stage: 1, wait: 150, type: true, html: `<pre class="t-code">const change = await <i>edit</i>({path: 'tally/__init__.py',
  old: 'return max(values) - min(values)',
  replacement: 'return max(values) - min(values) if values else 0'});
const run = await <i>bash</i>({command: 'pytest -q tests/test_tally.py'});
return {change, exit: run.exit_code};</pre>`,
  },
  { stage: 2, wait: 900, html: `<div class="t-result"><span class="t-ok">✓</span> edit  tally/__init__.py <span class="t-dim">+1 −1</span></div>` },
  { stage: 2, wait: 1300, html: `<div class="t-result"><span class="t-ok">✓</span> bash  pytest -q <span class="t-dim">·</span> <span class="t-ok">3 passed</span></div><div class="t-box-bottom">╰─ <span class="t-ok">✓ executed</span></div>` },
  { stage: 3, wait: 1000, html: `<div class="t-answer"><code>spread([])</code> failed because it called <code>max()</code> on an empty list, though it is documented to return zero. It now returns zero for empty input, and the test is unchanged. All 3 tests pass.</div>` },
];
const stages = ['Read', 'Edit', 'Check', 'Answer'];

const terminal = `<figure class="term" aria-label="A Pane session fixing a failing test, replayed">
  <div class="term-title"><span><span class="t-flap">⠿</span> <b>PANE</b> / tally</span><span class="term-chips"><span>gpt-6-sol ▾</span><span>Build</span></span></div>
  <ol class="term-steps" aria-hidden="true">${stages.map((s, i) => `<li data-stage="${i}"><span></span>${s}</li>`).join('')}</ol>
  <div class="term-body" aria-live="off"></div>
  <div class="term-composer"><span class="t-prompt">❯</span> <span class="t-dim">Describe the next step</span></div>
  <div class="term-status"><span>effort low · helpers on</span><span class="t-meter">ctx <b class="t-ctx">2.1k</b> / 272.0k <span class="t-gauge"><i></i></span> <b class="t-pct">1%</b></span></div>
  <span class="callout c1" aria-hidden="true">Reads your repo’s rules first</span>
  <span class="callout c2" aria-hidden="true">Runs the check before it answers</span>
  <span class="callout c3" aria-hidden="true">Knows your plan’s real window</span>
</figure>`;


// Benefit rows: a heading, the gain in two sentences, and a small picture
// of it. The pictures are markup, so they theme and scale with the page.
const grepBars = `<div class="viz-bars" aria-hidden="true">
  <div><span>What your tools print</span><i style="--w:100%"></i><b>all of it</b></div>
  <div><span>What the model reads</span><i class="acid" style="--w:3%"></i><b>what matters</b></div>
</div>`;
const checks = `<div class="viz-rules" aria-hidden="true">
  <div class="rule-file"><b>AGENTS.md</b><span>Before a change is done, run the tests and the linter.</span></div>
  <div class="rule-run"><span class="ok">✓</span> tests <em>passed</em></div>
  <div class="rule-run"><span class="ok">✓</span> linter <em>passed</em></div>
  <div class="rule-done">Then it answers.</div>
</div>`;
const plans = `<div class="viz-plans" aria-hidden="true">
  <div><span class="plan">ChatGPT</span><span class="model">your plan</span><i style="--w:45%"></i><b>the context your plan really has</b></div>
  <div><span class="plan">Claude</span><span class="model">your plan</span><i style="--w:100%"></i><b>the context your plan really has</b></div>
</div>`;
const sessions = `<div class="viz-sessions" aria-hidden="true">
  <div><b>yesterday</b><span>fix the failing test</span><em>resume</em></div>
  <div><b>this morning</b><span>rename a command everywhere</span><em>resume</em></div>
  <div class="undo"><b>/rollback</b><span>what the session changed · your edits kept</span><em>undo</em></div>
</div>`;

const gains = [
  ['01', 'The same work.<br>Fewer tokens.', 'Search results, logs and test runs reach the model as a short summary it can open further, not a wall of text it has to read. Less noise in every step: the same work on fewer tokens.', grepBars],
  ['02', 'Done the way<br>your repo says.', 'Pane reads your AGENTS.md and CLAUDE.md and does what they ask — including the checks a change needs before it is done. It finishes a change the way your team would, not just until the code compiles.', checks],
  ['03', 'Both subscriptions.<br>One agent.', 'Sign in with ChatGPT and Claude, and switch models in the middle of a task. Pane asks each plan what it really serves, so it works to your actual limits instead of a guess.', plans],
  ['04', 'Nothing gets<br>away from you.', 'Pick any session up where it stopped. Undo what a session changed while your own edits stay. Your keys and logins stay out of the conversation, the transcript and the logs.', sessions],
];

export function homePage({ install, bird, repo, root, arrow }) {
  return `
<section class="h-hero">
  <div class="h-hero-copy">
    <div class="eyebrow"><span class="status-dot"></span> A CODING AGENT FOR YOUR TERMINAL · PRE-RELEASE</div>
    <h1>MORE WORK<br>PER <span class="outline">PLAN.</span></h1>
    <p class="h-lede">Pane works in your repository with the ChatGPT or Claude subscription you already have. It finishes what Codex finishes, on fewer tokens, and checks its work the way your project asks.</p>
    ${install()}
    <div class="h-hero-links"><a class="text-link" href="#gains">Why Pane <span aria-hidden="true">↓</span></a><a class="text-link" href="${repo}">Source ${arrow}</a></div>
  </div>
  <div class="h-hero-visual">${terminal}</div>
</section>

<section class="h-proof" aria-label="Measured against Codex">
  <strong>18 %</strong>
  <div><p class="h-proof-claim">fewer tokens for the same finished work.</p><p class="h-proof-note">Measured on real coding tasks against the Codex CLI, same model, same results, and re-measured on this release. <a class="inline-link" href="${measurements}">How it was measured</a>.</p></div>
</section>

<section id="gains" class="h-gains">
  ${gains.map(([n, title, copy, viz]) => `<article class="h-gain"><div class="h-gain-copy"><span class="product-number">${n}</span><h2>${title}</h2><p>${copy}</p></div><div class="h-gain-viz">${viz}</div></article>`).join('')}
</section>

<section id="start" class="h-start">
  <div class="h-start-head"><h2>START IN<br><span class="outline">A MINUTE.</span></h2><pre class="bird" aria-hidden="true">${bird.join('\n')}</pre></div>
  <ol>
    <li><span class="product-number">01</span><h3>Install</h3><p>macOS and Linux. Checked against the release’s checksums; no shell profile edited.</p>${install()}</li>
    <li><span class="product-number">02</span><h3>Sign in</h3><p>Start <code>pane</code> and type <code>/login</code>: a ChatGPT or Claude subscription, or an API key.</p></li>
    <li><span class="product-number">03</span><h3>Say what you need</h3><p>“Fix the failing test.” Pane reads, edits, checks, and tells you what it did.</p></li>
  </ol>
  <a class="button" href="${root}pane/">The full guide ${arrow}</a>
</section>

<aside class="other-product"><span>Running several agents at once?</span><a href="${root}glasshouse/">Glasshouse, in preview ${arrow}</a></aside>`;
}

// Plays the session into the terminal, then again after a pause. With
// reduced motion, or when the tab is hidden at start, it shows the end state.
export function startReplay(still) {
  const term = document.querySelector('.term');
  if (!term) return;
  const body = term.querySelector('.term-body');
  const steps = [...term.querySelectorAll('.term-steps li')];
  const ctx = term.querySelector('.t-ctx');
  const pct = term.querySelector('.t-pct');
  const gauge = term.querySelector('.t-gauge i');
  const setStage = (stage) => steps.forEach((li, i) => { li.classList.toggle('done', i < stage); li.classList.toggle('now', i === stage); });
  const setCtx = (k) => { ctx.textContent = `${k.toFixed(1)}k`; const p = Math.max(1, Math.round((k / 272) * 100)); pct.textContent = `${p}%`; gauge.style.width = `${Math.min(100, p * 4)}%`; };
  const finalCtx = 10.8;
  const showAll = () => {
    body.innerHTML = replay.map((line) => line.html).join('');
    setStage(stages.length); setCtx(finalCtx); term.classList.add('shown');
  };
  if (still.matches) { showAll(); return; }
  let run = 0;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const play = async (id) => {
    body.innerHTML = ''; setStage(0); setCtx(2.1); term.classList.remove('shown');
    for (const [i, line] of replay.entries()) {
      await sleep(line.wait);
      if (id !== run) return;
      if (still.matches) { showAll(); return; }
      setStage(line.stage);
      const holder = document.createElement('div');
      holder.innerHTML = line.html;
      const node = holder.firstElementChild;
      if (line.type) {
        const full = node.innerHTML;
        const text = node.textContent;
        node.textContent = '';
        body.append(node);
        for (let c = 0; c <= text.length; c += 3) {
          if (id !== run) return;
          node.textContent = text.slice(0, c);
          body.scrollTop = body.scrollHeight;
          await sleep(12);
        }
        node.innerHTML = full;
      } else {
        body.append(...holder.childNodes);
      }
      body.scrollTop = body.scrollHeight;
      setCtx(2.1 + ((finalCtx - 2.1) * (i + 1)) / replay.length);
    }
    setStage(stages.length);
    term.classList.add('shown');
    await sleep(7000);
    if (id === run) play(++run);
  };
  // Start when the terminal is on screen, so the first thing seen is the start.
  const seen = new IntersectionObserver((entries) => {
    if (entries.some((entry) => entry.isIntersecting)) { seen.disconnect(); play(++run); }
  });
  seen.observe(term);
  still.addEventListener?.('change', () => { if (still.matches) { run += 1; showAll(); } });
}
