// The measured data, drawn from products.js and nothing else: the Pane-vs-Codex
// comparison as grouped bars on one axis per metric, and every measured run as
// a timeline. Each has a table view built from the same records.

const SVG = 'http://www.w3.org/2000/svg';
const el = (tag, attrs = {}, text) => {
  const node = tag === 'svg' || ['g', 'rect', 'path', 'text', 'line', 'title'].includes(tag)
    ? document.createElementNS(SVG, tag)
    : document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
  if (text !== undefined) node.textContent = text;
  return node;
};

const fmt = (metric, v) => {
  if (metric.key === 'tokens' || metric.key === 'uncached') return v >= 1000 ? `${(v / 1000).toFixed(2).replace(/0$/, '')}M` : `${Math.round(v)}k`;
  if (metric.key === 'passed') return `${v} of 3`;
  return `${Math.round(v)} s`;
};

// "Codex 1.74× faster", "Pane 20 % fewer": the comparison a reader wants,
// said from the side that wins it; within 5 % is "about equal".
const compare = (metric, p, c) => {
  if (metric.key === 'passed') return p === c ? 'Equal' : p > c ? 'Pane passed more' : 'Codex passed more';
  if (Math.abs(p - c) / Math.max(p, c) < 0.05) return 'About equal';
  if (metric.key === 'time') return p < c ? `Pane ${(c / p).toFixed(2)}× faster` : `Codex ${(p / c).toFixed(2)}× faster`;
  return p < c ? `Pane ${Math.round((1 - p / c) * 100)} % fewer` : `Codex ${Math.round((1 - c / p) * 100)} % fewer`;
};

const toggleGroup = (labels, current, onPick, name) => {
  const group = el('div', { class: 'viz-toggle', role: 'group', 'aria-label': name });
  labels.forEach(([key, label]) => {
    const b = el('button', { type: 'button', 'aria-pressed': String(key === current), 'data-key': key }, label);
    b.addEventListener('click', () => {
      for (const other of group.children) other.setAttribute('aria-pressed', String(other === b));
      onPick(key);
    });
    group.append(b);
  });
  return group;
};

export function benchChart(root, data) {
  const series = [['pane', 'Pane'], ['codex', 'Codex']];
  let metric = data.metrics[0];
  let view = 'chart';

  const controls = el('div', { class: 'viz-controls' });
  const legend = el('div', { class: 'viz-legend', 'aria-hidden': 'true' });
  for (const [key, label] of series) {
    const item = el('span', { class: 'viz-key' });
    item.append(el('span', { class: `viz-swatch ${key}` }), document.createTextNode(label));
    legend.append(item);
  }
  const summary = el('p', { class: 'viz-summary', 'aria-live': 'polite' });
  const plot = el('div', { class: 'viz-plot' });
  const tip = el('div', { class: 'viz-tip', role: 'tooltip', hidden: '' });
  const note = el('p', { class: 'viz-note' });
  const table = el('div', { class: 'viz-table', hidden: '' });
  plot.append(tip);

  controls.append(
    toggleGroup(data.metrics.map((m) => [m.key, m.label]), metric.key, (key) => { metric = data.metrics.find((m) => m.key === key); render(); }, 'Metric'),
    toggleGroup([['chart', 'Chart'], ['table', 'Table']], view, (key) => { view = key; render(); }, 'View'),
  );
  root.replaceChildren(controls, legend, summary, plot, note, table);

  const total = (side) => data.tasks.reduce((sum, t) => sum + t[side][metric.key], 0);

  function showTip(task, anchor) {
    tip.replaceChildren();
    tip.append(el('strong', {}, task.name));
    for (const [key, label] of series) {
      const row = el('span', { class: 'viz-tip-row' });
      row.append(el('span', { class: `viz-line ${key}` }), el('b', {}, fmt(metric, task[key][metric.key])), document.createTextNode(` ${label}`));
      tip.append(row);
    }
    tip.append(el('span', { class: 'viz-tip-verdict' }, compare(metric, task.pane[metric.key], task.codex[metric.key])));
    if (task.pane.facts !== undefined) tip.append(el('span', { class: 'viz-tip-extra' }, `Facts found: Pane ${task.pane.facts.toFixed(1)} of 8, Codex ${task.codex.facts.toFixed(1)} of 8`));
    tip.hidden = false;
    const box = plot.getBoundingClientRect();
    const a = anchor.getBoundingClientRect();
    const top = a.bottom - box.top + 6;
    const left = Math.max(0, Math.min(box.width - tip.offsetWidth, a.left - box.left + 64));
    tip.style.top = `${top}px`;
    tip.style.left = `${left}px`;
  }
  const hideTip = () => { tip.hidden = true; };

  function drawChart() {
    const width = Math.max(280, plot.clientWidth);
    const labelW = width < 480 ? 50 : 64;
    const valueW = 58;
    const barH = 14;
    const gap = 2;
    const rowH = 22 + barH * 2 + gap + 18;
    const height = rowH * data.tasks.length;
    const max = metric.key === 'passed' ? 3 : Math.max(...data.tasks.flatMap((t) => [t.pane[metric.key], t.codex[metric.key]]));
    const scale = (v) => ((width - labelW - valueW) * v) / max;
    const svg = el('svg', { width, height, viewBox: `0 0 ${width} ${height}`, role: 'img', 'aria-label': `${metric.label}, Pane against Codex, per task. ${summary.textContent}` });
    data.tasks.forEach((task, i) => {
      const y0 = i * rowH;
      const g = el('g', { class: 'viz-row', tabindex: '0', role: 'button', 'aria-label': `${task.name}: Pane ${fmt(metric, task.pane[metric.key])}, Codex ${fmt(metric, task.codex[metric.key])}. ${compare(metric, task.pane[metric.key], task.codex[metric.key])}.` });
      g.append(el('rect', { class: 'viz-hit', x: 0, y: y0, width, height: rowH - 6 }));
      g.append(el('text', { class: 'viz-task', x: 0, y: y0 + 14 }, task.name));
      series.forEach(([key, label], s) => {
        const y = y0 + 22 + s * (barH + gap);
        const w = Math.max(2, scale(task[key][metric.key]));
        const r = Math.min(4, w / 2);
        const x = labelW;
        g.append(el('text', { class: 'viz-series', x: 0, y: y + barH - 3 }, label));
        g.append(el('path', { class: `viz-bar ${key}`, d: `M${x},${y}h${w - r}a${r},${r} 0 0 1 ${r},${r}v${barH - 2 * r}a${r},${r} 0 0 1 -${r},${r}h-${w - r}z` }));
        g.append(el('text', { class: 'viz-value', x: x + w + 6, y: y + barH - 3 }, fmt(metric, task[key][metric.key])));
      });
      g.addEventListener('pointerenter', () => showTip(task, g));
      g.addEventListener('pointerleave', hideTip);
      g.addEventListener('focus', () => showTip(task, g));
      g.addEventListener('blur', hideTip);
      g.addEventListener('keydown', (e) => { if (e.key === 'Escape') hideTip(); });
      svg.append(g);
    });
    for (const old of plot.querySelectorAll('svg')) old.remove();
    plot.prepend(svg);
  }

  function drawTable() {
    const t = el('table', { class: 'table viz-data' });
    const head = el('tr');
    ['Task', 'Pane', 'Codex', ''].forEach((h) => head.append(el('th', { scope: 'col' }, h)));
    const thead = el('thead');
    thead.append(head);
    t.append(thead);
    const body = el('tbody');
    for (const task of data.tasks) {
      const tr = el('tr');
      tr.append(el('td', {}, task.name), el('td', { 'data-label': 'Pane' }, fmt(metric, task.pane[metric.key])), el('td', { 'data-label': 'Codex' }, fmt(metric, task.codex[metric.key])), el('td', {}, compare(metric, task.pane[metric.key], task.codex[metric.key])));
      body.append(tr);
    }
    const all = el('tr', { class: 'viz-total' });
    all.append(el('td', {}, 'All four'), el('td', { 'data-label': 'Pane' }, fmt(metric, total('pane'))), el('td', { 'data-label': 'Codex' }, fmt(metric, total('codex'))), el('td', {}, compare(metric, total('pane'), total('codex'))));
    body.append(all);
    t.append(body);
    table.replaceChildren(t);
  }

  function render() {
    summary.textContent = `All four tasks: Pane ${fmt(metric, total('pane'))}, Codex ${fmt(metric, total('codex'))} — ${compare(metric, total('pane'), total('codex'))}.`;
    note.textContent = `${metric.note}${metric.lower ? ' Shorter is better.' : ''}`;
    hideTip();
    plot.hidden = view !== 'chart';
    legend.hidden = view !== 'chart';
    table.hidden = view !== 'table';
    if (view === 'chart') drawChart();
    drawTable();
  }

  render();
  let last = plot.clientWidth;
  new ResizeObserver(() => { if (view === 'chart' && Math.abs(plot.clientWidth - last) > 4) { last = plot.clientWidth; drawChart(); } }).observe(plot);
}

const KIND = { progress: ['▲', 'Progress'], setback: ['▼', 'Setback'], mixed: ['◆', 'Mixed'] };

export function historyTimeline(root, rows) {
  let selected = rows.length - 1;
  let view = 'timeline';
  const controls = el('div', { class: 'viz-controls' });
  const track = el('div', { class: 'timeline', role: 'group', 'aria-label': 'Measured runs, oldest first. Arrow keys move between them.' });
  const detail = el('div', { class: 'timeline-detail', 'aria-live': 'polite' });
  const list = el('div', { class: 'viz-table', hidden: '' });
  const keyLine = el('p', { class: 'viz-note' }, '▲ progress · ▼ setback · ◆ mixed — for Pane. Small samples throughout: direction, not proof.');
  controls.append(toggleGroup([['timeline', 'Timeline'], ['list', 'List']], view, (k) => { view = k; paint(); }, 'View'));
  root.replaceChildren(controls, track, detail, keyLine, list);

  const buttons = rows.map(([, label, what, , kind], i) => {
    const b = el('button', { type: 'button', class: `timeline-point ${kind}`, 'aria-pressed': 'false', 'aria-label': `${label}, ${KIND[kind][1].toLowerCase()}: ${what}` });
    b.append(el('span', { class: 'timeline-mark', 'aria-hidden': 'true' }, KIND[kind][0]), el('span', { class: 'timeline-day', 'aria-hidden': 'true' }, label.replace('Sep ', '').replace(' → ', '–')));
    b.style.left = `${(i / (rows.length - 1)) * 100}%`;
    b.addEventListener('click', () => { selected = i; paint(); });
    b.addEventListener('keydown', (e) => {
      const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
      if (!step) return;
      e.preventDefault();
      selected = Math.max(0, Math.min(rows.length - 1, selected + step));
      paint();
      buttons[selected].focus();
    });
    track.append(b);
    return b;
  });
  track.prepend(el('span', { class: 'timeline-axis', 'aria-hidden': 'true' }), el('span', { class: 'timeline-month', 'aria-hidden': 'true' }, 'SEP 2026'));

  const t = el('table', { class: 'table' });
  const body = el('tbody');
  for (const [, label, what, result, kind] of rows) {
    const tr = el('tr');
    tr.append(el('td', {}, label), el('td', {}, `${KIND[kind][0]} ${KIND[kind][1]}`), el('td', {}, what), el('td', {}, result));
    body.append(tr);
  }
  t.append(body);
  list.append(t);

  function paint() {
    track.hidden = view !== 'timeline';
    detail.hidden = view !== 'timeline';
    list.hidden = view !== 'list';
    buttons.forEach((b, i) => {
      b.setAttribute('aria-pressed', String(i === selected));
      b.tabIndex = i === selected ? 0 : -1;
    });
    const [, label, what, result, kind] = rows[selected];
    detail.replaceChildren(
      el('p', { class: 'timeline-when' }, `${label} · ${KIND[kind][0]} ${KIND[kind][1]}`),
      el('h3', {}, what),
      el('p', {}, result),
    );
  }
  paint();
}
