import '@fontsource/barlow-condensed/latin-600.css';
import '@fontsource/ibm-plex-mono/latin-400.css';
import './style.css';
import { repo, installCommand, bird, flap, pane, glasshouse, history as paneHistory, benchmark as paneBench } from './products.js';

const page = document.body.dataset.page || 'home';
const root = page === 'home' ? './' : '../';
const arrow = '<span aria-hidden="true">↗</span>';
const down = '<span aria-hidden="true">↓</span>';
const esc = (text) => text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const scene = (kind, className = '') => `<div class="optics ${className}" data-scene="${kind}" data-root="${root}" aria-hidden="true"><div class="optics-fallback ${kind === 'house' ? 'house-fallback' : 'specimen-fallback'}">${kind === 'house' ? '<span>GLASS<br>HOUSE</span>' : `<img src="${root}specimens.png" alt="" loading="lazy"/>`}</div><canvas></canvas></div>`;
const button = (text, href, dark = false) => `<a class="button ${dark ? 'dark' : ''}" href="${href}">${text} ${arrow}</a>`;
const install = (command = installCommand) => `<div class="install"><span class="install-prompt" aria-hidden="true">$</span><code>${esc(command).replace(/\//g, '/<wbr>')}</code><button class="copy" type="button" data-copy="${esc(command)}">Copy</button></div>`;
const code = (lines) => `<pre class="terminal"><code>${lines.map(esc).join('\n')}</code></pre>`;
const features = (items) => `<div class="feature-list">${items.map(([name, copy, source]) => `<article><h3>${name}</h3><p>${copy}</p><span class="feature-status">${source}</span></article>`).join('')}</div>`;
const flapBird = `<span class="flap" aria-hidden="true">${flap[2]}</span>`;
const perched = `<pre class="bird" aria-hidden="true">${bird.join('\n')}</pre>`;

const nav = `<a class="skip" href="#main">Skip to content</a><header><a class="wordmark" href="${root}" aria-label="Pane home">${flapBird}<span>PANE</span></a><nav aria-label="Main navigation"><a href="${root}pane/" ${page === 'pane' ? 'aria-current="page"' : ''}>Get started</a><a class="nav-glasshouse" href="${root}glasshouse/" ${page === 'glasshouse' ? 'aria-current="page"' : ''}>Glasshouse <span class="tag">preview</span></a><a class="source-link" href="${repo}">Source ${arrow}</a></nav></header>`;

const compare = (c) => `<section class="section compare"><div class="program"><pre>${c.diagram}</pre><div><h2>${c.title}</h2><p>${c.body}</p></div></div><div class="facts">${pane.facts.map(([n, text]) => `<div class="fact"><strong>${esc(n)}</strong><p>${text}</p></div>`).join('')}</div></section>`;

const steps = `<section id="start" class="section steps-section"><div class="relationship-copy"><h2>THREE STEPS<br>TO THE NEST.</h2><p>Install it, sign in once, and work in any repository. Nothing to configure before the first task.</p></div><ol class="steps">
<li><span class="step-number">01</span><h3>Install</h3><p>macOS on Apple silicon, Linux on x86_64 and arm64. The installer checks the archive against the release’s checksums, installs into <code>~/.local</code>, and edits no shell profile.</p>${install()}</li>
<li><span class="step-number">02</span><h3>Sign in</h3><p>Start Pane and type <code>/login</code>. Enter an API key, or connect a ChatGPT or Claude subscription. <a class="inline-link" href="${root}pane/#sign-in">How subscriptions work</a>.</p>${code(['$ cd your-project', '$ pane', '> /login'])}</li>
<li><span class="step-number">03</span><h3>Work</h3><p>Pick a model the first time, then describe the task. <code>pane -p "…"</code> runs one task and exits; <code>pane --continue</code> picks up where you left off.</p>${code(['> fix the failing request_modes tests', '$ pane -p "explain how the ruler scores an attempt"'])}</li>
</ol></section>`;

const featureSection = `<section id="features" class="section detail-section"><div class="relationship-copy"><h2>WHAT IS<br>IN THE NEST.</h2><p>Everything below ships today. Each card names the test or source file that holds it to that.</p></div>${features(pane.features)}</section>`;

// The Pane-vs-Codex comparison is still running; its numbers go in this
// section once measured. Until then it says so, and invents nothing.
const benchmark = `<section id="benchmark" class="section bench"><div class="relationship-copy"><h2>MEASURED,<br>NOT CLAIMED.</h2><p>Pane against Codex on the same four tasks, the same model (GPT-6 Sol) and the same effort. ${paneBench.verdict}</p></div><table class="table"><thead><tr>${paneBench.head.map((h) => `<th>${h}</th>`).join('')}</tr></thead><tbody>${paneBench.rows.map((row) => `<tr>${row.map((cell, i) => `<td data-label="${paneBench.head[i]}">${cell}</td>`).join('')}</tr>`).join('')}</tbody></table><p class="bench-notes">${paneBench.notes}</p><p class="bench-notes">${paneBench.check}</p></section>`;

// How Pane's measured numbers have moved over time, setbacks included; filled
// with sourced figures by whoever publishes them, never invented here.
const history = `<section id="history" class="section history"><div class="relationship-copy"><h2>HOW IT HAS<br>BEEN MEASURED.</h2><p>Every measured run, in order — including the ones that went the wrong way. Small samples throughout; read them as direction, not proof.</p></div><table class="table"><tbody>${paneHistory.map(([when, what, result]) => `<tr><td>${when}</td><td>${what}</td><td>${result}</td></tr>`).join('')}</tbody></table></section>`;

const limits = `<section id="limits" class="section limits"><div class="relationship-copy"><h2>WHERE IT<br>STANDS.</h2><p>A pre-release, said plainly. What it does not do yet is as much a part of the product as what it does.</p></div><dl class="limit-list">${pane.limits.map(([term, text]) => `<div><dt>${term}</dt><dd>${text}</dd></div>`).join('')}</dl></section>`;

const glasshousePreview = `<section class="section relationship"><div class="relationship-copy"><h2>GLASSHOUSE.<br><span class="outline">PREVIEW.</span></h2><p>Pane is one agent. Glasshouse is for running several — Pane, Claude Code, Codex, OpenCode — as sessions you can see, with one memory for the project. It is not at Pane’s readiness yet. ${'<a class="inline-link" href="' + root + 'glasshouse/">Read about the preview</a>'}.</p></div><div class="system-map" aria-label="Glasshouse coordinates Pane, Claude Code, Codex and OpenCode sessions"><div class="map-parent">GLASSHOUSE <span>SESSIONS / DELEGATION / PROJECT MEMORY</span></div><div class="map-children"><a href="${root}pane/">PANE <span>READY ↗</span></a><span>CLAUDE CODE</span><span>CODEX</span><span>OPENCODE</span></div></div></section>`;

const home = `<section class="hero"><div class="eyebrow"><span class="status-dot"></span> PRE-RELEASE · MACOS &amp; LINUX · SOURCE AVAILABLE</div><h1>LESS<br>READING.<br>MORE <span class="outline">DOING.</span></h1>${scene('shell')}<div class="hero-bottom"><div class="hero-lede"><p>Pane is a coding agent for your terminal. Tool results stay in a live runtime as named objects; the model writes TypeScript over them and reads only what it needs.</p>${install()}<p class="hero-note">Then run <code>pane</code> in a repository. <a class="inline-link" href="#start">Three steps ${down}</a></p></div></div></section>${compare(pane.compare)}${steps}${featureSection}${benchmark}${history}${limits}${glasshousePreview}`;

const paneHero = `<section class="hero detail-hero"><div class="eyebrow"><span class="status-dot"></span> INSTALL · SIGN IN · WORK</div><h1>GET<br><span class="outline">PANE.</span></h1>${scene('shell')}<div class="hero-bottom"><div class="hero-lede"><p>One line installs Pane and the inference gateway it talks through. Sign in inside Pane with <code>/login</code>.</p>${install()}<p class="hero-note">Only Pane, without the Glasshouse link: <code>… | sh -s -- --pane-only</code></p></div></div></section>`;

const installSection = `<section id="install" class="section doc-section"><div class="relationship-copy"><h2>INSTALL.</h2><p>The installer picks this machine’s archive from the newest release, verifies it against <code>SHA256SUMS</code>, unpacks it into its own version directory and links <code>pane</code> into <code>~/.local/bin</code>. It installs no harness, touches no credential and edits no shell profile.</p></div><div class="doc-grid">
<div><h3>Platforms</h3><table class="table"><tbody><tr><td>macOS, Apple silicon</td><td>installer</td></tr><tr><td>Linux x86_64</td><td>installer</td></tr><tr><td>Linux arm64</td><td>installer</td></tr><tr><td>Windows x64 and arm64</td><td>archives on the <a class="inline-link" href="${repo}/releases">releases page</a>; no installer yet</td></tr></tbody></table></div>
<div><h3>Options</h3>${code(['# only Pane, no glasshouse link', installCommand + ' -s -- --pane-only', '', '# a specific release', 'curl -fsSL https://harzerheribert.github.io/glasshouse/install.sh | GLASSHOUSE_VERSION=v0.1.0-pre.3 sh'])}<p>Updates: a release install checks once a day from an interactive session and installs a newer release beside the running one. <code>pane update --check</code> asks without installing; <code>PANE_DISABLE_AUTOUPDATE=1</code> turns the daily check off.</p></div>
</div></section>`;

const signInSection = `<section id="sign-in" class="section doc-section"><div class="relationship-copy"><h2>SIGN IN.</h2><p>Pane’s requests go through the inference gateway it starts beside itself. The gateway holds your keys and logins; they never enter the conversation, the transcript or a log.</p></div><div class="doc-grid">
<div><h3>An API key</h3><p>Start <code>pane</code>. With no credential stored it says so and points at <code>/login</code>, which lists the providers the gateway knows. <code>/key anthropic</code> takes a key without echoing it.</p>${code(['$ pane', '> /login', '> /key anthropic'])}</div>
<div><h3>A ChatGPT or Claude subscription</h3><p>Declare the account once in the gateway’s configuration — <code>~/Library/Application Support/inference-gateway/gateway.toml</code> on macOS, <code>~/.config/inference-gateway/gateway.toml</code> on Linux — then connect it from Pane with <code>/login chatgpt</code> (for ChatGPT, <code>/login chatgpt device</code> gives a code to enter on another device).</p>${code(['[accounts.chatgpt]', 'kind = "chatgpt"', 'vendor = "openai"', 'subscription_broker = "cliproxyapi"', '', '[accounts.claude]', 'kind = "claude"', 'vendor = "claude"', 'subscription_broker = "cliproxyapi"'])}<p>Subscriptions are served through CLIProxyAPI, which the installer pins to a release and verifies by checksum. Outside Pane: <code>inference-gateway subscriptions connect --entitlement chatgpt openai</code>, and <code>inference-gateway subscriptions usage</code> shows how much of each plan’s limits is used.</p></div>
</div></section>`;

const useSection = `<section id="use" class="section doc-section"><div class="relationship-copy"><h2>WORK.</h2><p>Run <code>pane</code> in a repository. The first time, it opens the model picker; after that it starts where your configuration says.</p></div><div class="doc-grid">
<div><h3>From the shell</h3><table class="table commands"><tbody>
<tr><td><code>pane</code></td><td>start a session in this folder</td></tr>
<tr><td><code>pane -p "task"</code></td><td>run one task and exit; <code>--output-format json</code> for scripts</td></tr>
<tr><td><code>pane --continue</code></td><td>resume the newest session here</td></tr>
<tr><td><code>pane --sessions</code></td><td>list this folder’s sessions; <code>--resume &lt;id&gt;</code> opens one</td></tr>
<tr><td><code>pane --plan</code></td><td>start in plan mode</td></tr>
<tr><td><code>pane --image shot.png</code></td><td>attach up to four images</td></tr>
<tr><td><code>pane --full-access</code></td><td>no questions, no OS confinement of Pane’s own — for a machine you trust</td></tr>
<tr><td><code>pane config global model.parent &lt;id&gt;</code></td><td>the model a scripted run uses</td></tr>
<tr><td><code>pane doctor</code></td><td>check the project, its configuration, permissions and sandbox</td></tr>
</tbody></table></div>
<div><h3>Inside a session</h3><table class="table commands"><tbody>
<tr><td><code>/models</code></td><td>switch models mid-session</td></tr>
<tr><td><code>/login</code> · <code>/key</code></td><td>connect an account, store a key</td></tr>
<tr><td><code>Shift-Tab</code></td><td>how often Pane asks before it acts</td></tr>
<tr><td><code>/rollback</code></td><td>preview and undo what the session changed</td></tr>
<tr><td><code>/usage</code></td><td>how much of each subscription is used</td></tr>
<tr><td><code>?</code></td><td>every key, on an empty composer</td></tr>
</tbody></table>${perched}<p class="bird-says">“The early bird gets the diff.”</p></div>
</div></section>`;

const paneFeatures = `<section id="features" class="section detail-section"><div class="relationship-copy"><h2>WHAT IS<br>IN THE NEST.</h2><p>Everything below ships today, and each card names the test or source file that holds it to that.</p></div>${features(pane.features)}</section>`;

const panePage = `${paneHero}${installSection}${signInSection}${useSection}${paneFeatures}${limits}<aside class="other-product"><span>Running several agents at once?</span><a href="${root}glasshouse/">Glasshouse, in preview ${arrow}</a></aside>`;

const glasshousePage = `<section class="hero detail-hero"><div class="eyebrow"><span class="status-dot preview-dot"></span> PREVIEW · NOT YET AT PANE’S READINESS</div><h1>MANY<br>AGENTS.<br>ONE <span class="outline">VIEW.</span></h1>${scene('house')}<div class="hero-bottom"><div class="hero-lede"><p>${glasshouse.intro}</p>${button('Try Pane first', `${root}pane/`, true)}</div></div></section><section id="features" class="section detail-section"><div class="relationship-copy"><h2>WHAT IT<br>IS FOR.</h2><p class="development-note">${glasshouse.status}</p></div>${features(glasshouse.features)}</section><section class="section doc-section"><div class="relationship-copy"><h2>TRY THE<br>PREVIEW.</h2><p>The installer links <code>glasshouse</code> beside <code>pane</code> unless you pass <code>--pane-only</code>. <code>glasshouse doctor</code> finds the harnesses you have installed; <code>glasshouse launch --harness pane</code> starts one as a Glasshouse session. The repository describes the rest, including what is still open.</p></div>${button('Source and status', repo, true)}</section><aside class="other-product"><span>Want one agent that works today?</span><a href="${root}pane/">Get Pane ${arrow}</a></aside>`;

const main = page === 'pane' ? panePage : page === 'glasshouse' ? glasshousePage : home;
const footer = `<footer><a class="wordmark" href="${root}">${flapBird}<span>PANE</span></a><span class="footer-note">By the Glasshouse project · source available, all rights reserved</span><button class="motion-toggle" type="button" aria-pressed="false">Pause motion</button><a href="${repo}">GitHub ${arrow}</a></footer>`;
document.querySelector('#app').innerHTML = `${nav}<main id="main">${main}</main>${footer}`;

// Copy buttons: the clipboard can be refused (an insecure origin, a
// permission prompt declined), and then the command stays selectable.
for (const copy of document.querySelectorAll('.copy')) {
  copy.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(copy.dataset.copy);
      copy.textContent = 'Copied';
    } catch {
      const range = document.createRange();
      range.selectNodeContents(copy.previousElementSibling);
      getSelection().removeAllRanges();
      getSelection().addRange(range);
      copy.textContent = 'Selected';
    }
    setTimeout(() => { copy.textContent = 'Copy'; }, 1600);
  });
}

// The wordmark's bird flaps as the composer's does; reduced motion holds
// it on the still frame, as Pane itself does.
const still = matchMedia('(prefers-reduced-motion: reduce)');
let tick = 0;
setInterval(() => {
  if (still.matches || document.hidden) return;
  tick += 1;
  for (const el of document.querySelectorAll('.flap')) el.textContent = flap[tick % flap.length];
}, 260);

import('./optics.js').then(({ startOptics }) => startOptics([...document.querySelectorAll('.optics')], document.querySelector('.motion-toggle'))).catch(() => { document.querySelector('.motion-toggle').hidden = true; });
