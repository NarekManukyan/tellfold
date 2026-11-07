// Shoal Studio: sentiment by round (SVG).
//
// Mean sentiment per round on a fixed −1..+1 axis with a zero line, one point per round (value on hover and
// on the selected round), injected events as amber markers with their text on hover. Clicking a round, or
// the arrow keys while the plot has focus, moves the playhead. Colours come from the shared CSS tokens, so
// the chart follows theme changes without re-reading anything.

import { formatScore } from './network.js';

const NS = 'http://www.w3.org/2000/svg';

function injectStyles() {
  if (document.getElementById('shoal-chart-style')) return;
  const s = document.createElement('style');
  s.id = 'shoal-chart-style';
  s.textContent = `
.sc-wrap{position:absolute;inset:0}
.sc-plot{position:absolute;inset:0;outline:none;border-radius:8px;cursor:pointer;touch-action:none}
.sc-plot:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
.sc-plot svg{position:absolute;inset:0;width:100%;height:100%;overflow:visible;display:block}
.sc-plot text{font-family:ui-monospace,"SF Mono",SFMono-Regular,Menlo,monospace;font-size:11px;font-variant-numeric:tabular-nums}
.sc-plot text.lbl{font-family:-apple-system,BlinkMacSystemFont,system-ui,sans-serif;font-weight:500}
.sc-plot text.halo{paint-order:stroke;stroke:var(--card);stroke-width:3px;stroke-linejoin:round}
.sc-tip{position:absolute;left:0;top:0;pointer-events:none;padding:8px 10px;border-radius:8px;background:var(--card);
  border:1px solid var(--border);box-shadow:var(--shadow-pop);color:var(--text);font-size:12px;line-height:1.45;max-width:280px;
  opacity:0;transition:opacity .1s ease;z-index:3;font-variant-numeric:tabular-nums}
.sc-tip.on{opacity:1}
.sc-tip b{font-weight:600}
.sc-tip .neg{color:var(--neg-text)} .sc-tip .pos{color:var(--pos-text)} .sc-tip .d{color:var(--text-dim)}
.sc-tip .ev{margin-top:6px;padding-top:6px;border-top:1px solid var(--border)}
.sc-tip .ev b{color:var(--event-text);margin-right:4px}
@media (prefers-reduced-motion: reduce){.sc-tip{transition:none}}
`;
  document.head.appendChild(s);
}

function el(name, attrs = {}, parent) {
  const e = document.createElementNS(NS, name);
  for (const [k, v] of Object.entries(attrs)) if (v != null) e.setAttribute(k, v);
  if (parent) parent.appendChild(e);
  return e;
}

const signOf = (v) => (v == null || Number.isNaN(v) ? 'neu' : v < -0.15 ? 'neg' : v > 0.15 ? 'pos' : 'neu');
const colorOf = (v) => ({ neg: 'var(--neg)', pos: 'var(--pos)', neu: 'var(--neu)' })[signOf(v)];

/** A monotone cubic path through `p` (sorted by x), like d3.curveMonotoneX: no overshoot between rounds. */
function monotonePath(p) {
  const n = p.length;
  if (n < 3) return p.map(([x, y], i) => `${i ? 'L' : 'M'}${x.toFixed(1)} ${y.toFixed(1)}`).join(' ');
  const d = [], m = new Array(n);
  for (let i = 0; i < n - 1; i++) d.push((p[i + 1][1] - p[i][1]) / ((p[i + 1][0] - p[i][0]) || 1));
  m[0] = d[0]; m[n - 1] = d[n - 2];
  for (let i = 1; i < n - 1; i++) m[i] = d[i - 1] * d[i] <= 0 ? 0 : (d[i - 1] + d[i]) / 2;
  for (let i = 0; i < n - 1; i++) {
    if (d[i] === 0) { m[i] = 0; m[i + 1] = 0; continue; }
    const a = m[i] / d[i], b = m[i + 1] / d[i], h = a * a + b * b;
    if (h > 9) { const t = 3 / Math.sqrt(h); m[i] = t * a * d[i]; m[i + 1] = t * b * d[i]; }
  }
  let out = `M${p[0][0].toFixed(1)} ${p[0][1].toFixed(1)}`;
  for (let i = 0; i < n - 1; i++) {
    const [x0, y0] = p[i], [x1, y1] = p[i + 1], dx = (x1 - x0) / 3;
    out += ` C${(x0 + dx).toFixed(1)} ${(y0 + m[i] * dx).toFixed(1)} ${(x1 - dx).toFixed(1)} ${(y1 - m[i + 1] * dx).toFixed(1)} ${x1.toFixed(1)} ${y1.toFixed(1)}`;
  }
  return out;
}

export class SentimentChart {
  /** @param {HTMLElement} container  @param {{onSelectRound?: (round:number)=>void}} opts */
  constructor(container, { onSelectRound } = {}) {
    injectStyles();
    this.container = container;
    this.onSelectRound = onSelectRound || (() => {});
    this.stats = [];
    this.events = [];
    this.total = 0;
    this.round = null;
    this.hover = null;
    this.w = 0; this.h = 0;
    this.wrap = document.createElement('div');
    this.wrap.className = 'sc-wrap';
    this.plot = document.createElement('div');
    this.plot.className = 'sc-plot';
    this.plot.tabIndex = 0;
    this.plot.setAttribute('role', 'img');
    this.svg = el('svg', { 'aria-hidden': 'true' }, this.plot);
    this.tip = document.createElement('div');
    this.tip.className = 'sc-tip';
    this.tip.setAttribute('aria-hidden', 'true');
    this.wrap.append(this.plot, this.tip);
    container.appendChild(this.wrap);
    this._on = [];
    const on = (t, type, fn) => { t.addEventListener(type, fn); this._on.push([t, type, fn]); };
    on(this.plot, 'pointermove', (e) => this._move(e));
    on(this.plot, 'pointerleave', () => { this.hover = null; this._hideTip(); this.render(); });
    on(this.plot, 'click', (e) => { const r = this._roundAt(e); if (r != null) this.onSelectRound(r); });
    on(this.plot, 'keydown', (e) => this._key(e));
    this.ro = new ResizeObserver(() => this._size());
    this.ro.observe(this.plot);
    this._size();
  }

  setData({ stats = [], events = [], totalRounds = 0 } = {}) {
    this.stats = stats.filter((s) => typeof s.round === 'number').slice().sort((a, b) => a.round - b.round);
    this.events = events.slice();
    this.total = Math.max(totalRounds || 0, ...this.stats.map((s) => s.round), 1);
    this._label();
    this.render();
  }

  setRound(r) {
    if (r === this.round) return;
    this.round = r;
    this.render();
  }

  destroy() {
    this.ro.disconnect();
    for (const [t, type, fn] of this._on) t.removeEventListener(type, fn);
    this.wrap.remove();
  }

  _label() {
    const pts = this.stats.filter((s) => s.sentiment != null);
    this.plot.setAttribute('aria-label', pts.length
      ? `Mean sentiment by round: ${pts.map((s) => `round ${s.round} ${formatScore(s.sentiment)}`).join(', ')}.`
        + (this.events.length ? ` Events: ${this.events.map((e) => `round ${e.round}`).join(', ')}.` : '')
        + ' Use the left and right arrow keys to move the playhead.'
      : 'Mean sentiment by round: no rounds scored yet.');
  }

  _size() {
    const r = this.plot.getBoundingClientRect();
    const w = Math.round(r.width), h = Math.round(r.height);
    if (w === this.w && h === this.h) return;
    this.w = w; this.h = h;
    this.render();
  }

  _geom() {
    const padL = 36, padR = 14, padT = 22, padB = 26;
    const W = Math.max(60, this.w), H = Math.max(60, this.h);
    const x = (r) => padL + (this.total ? r / this.total : 0) * (W - padL - padR);
    const y = (v) => padT + (1 - (Math.max(-1, Math.min(1, v)) + 1) / 2) * (H - padT - padB);
    return { W, H, padL, padR, padT, padB, x, y };
  }

  _roundAt(e) {
    if (!this.total) return null;
    const rect = this.plot.getBoundingClientRect();
    const g = this._geom();
    const px = e.clientX - rect.left;
    const r = Math.round(((px - g.padL) / (g.W - g.padL - g.padR)) * this.total);
    const max = this.stats.length ? this.stats[this.stats.length - 1].round : 0;
    return Math.max(0, Math.min(max, r));
  }

  _key(e) {
    const max = this.stats.length ? this.stats[this.stats.length - 1].round : 0;
    const cur = this.round ?? max;
    let next = null;
    if (e.key === 'ArrowRight') next = Math.min(max, cur + 1);
    else if (e.key === 'ArrowLeft') next = Math.max(0, cur - 1);
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = max;
    if (next === null) return;
    e.preventDefault();
    e.stopPropagation();
    this.onSelectRound(next);
  }

  _move(e) {
    const r = this._roundAt(e);
    if (r !== this.hover) { this.hover = r; this.render(); }
    if (r == null) { this._hideTip(); return; }
    const stat = this.stats.find((s) => s.round === r);
    const evs = this.events.filter((ev) => ev.round === r);
    const box = document.createElement('div');
    const head = document.createElement('div');
    const b = document.createElement('b');
    b.textContent = r === 0 ? 'Round 0 (opening posts)' : `Round ${r}`;
    head.appendChild(b);
    box.appendChild(head);
    const v = document.createElement('div');
    if (stat && stat.sentiment != null) {
      v.append('Mean sentiment ');
      const s = document.createElement('span');
      s.className = signOf(stat.sentiment);
      s.textContent = formatScore(stat.sentiment);
      v.appendChild(s);
    } else {
      v.className = 'd';
      v.textContent = stat ? 'No scored posts' : 'Not simulated yet';
    }
    box.appendChild(v);
    if (stat) {
      const d = document.createElement('div');
      d.className = 'd';
      d.textContent = `${stat.new_posts ?? 0} posts · ${stat.active_agents ?? 0} agents active`;
      box.appendChild(d);
    }
    for (const ev of evs) {
      const x = document.createElement('div');
      x.className = 'ev';
      const eb = document.createElement('b');
      eb.textContent = 'Event';
      x.append(eb, document.createTextNode(ev.content || ''));
      box.appendChild(x);
    }
    this.tip.replaceChildren(box);
    this.tip.classList.add('on');
    const rect = this.wrap.getBoundingClientRect();
    const g = this._geom();
    let tx = g.x(r) + 12;
    const tw = this.tip.offsetWidth, th = this.tip.offsetHeight;
    if (tx + tw > rect.width - 4) tx = g.x(r) - tw - 12;
    const ty = Math.max(0, Math.min(rect.height - th, e.clientY - rect.top - th / 2));
    this.tip.style.transform = `translate(${Math.round(Math.max(0, tx))}px,${Math.round(ty)}px)`;
  }

  _hideTip() { this.tip.classList.remove('on'); }

  render() {
    const svg = this.svg;
    svg.replaceChildren();
    if (this.w < 10 || this.h < 10) return;
    const g = this._geom();
    svg.setAttribute('viewBox', `0 0 ${g.W} ${g.H}`);
    const gridC = 'var(--border)';
    // horizontal grid and the zero line
    for (const v of [1, 0.5, 0, -0.5, -1]) {
      const y = g.y(v);
      el('line', { x1: g.padL, x2: g.W - g.padR, y1: y, y2: y, stroke: v === 0 ? 'var(--border-strong)' : gridC,
        'stroke-width': 1, 'stroke-dasharray': v === 0 ? null : '2 4' }, svg);
      const t = el('text', { x: g.padL - 8, y: y + 4, 'text-anchor': 'end', fill: 'var(--text-dim)' }, svg);
      t.textContent = v === 0 ? '0' : (v > 0 ? '+' : '−') + Math.abs(v).toFixed(1);
    }
    // round ticks
    const step = this.total > 12 ? Math.ceil(this.total / 12) : 1;
    for (let r = 0; r <= this.total; r += step) {
      const t = el('text', { x: g.x(r), y: g.H - 6, 'text-anchor': 'middle', fill: r === this.round ? 'var(--text)' : 'var(--text-dim)' }, svg);
      t.textContent = `r${r}`;
    }
    // hovered / selected round column
    for (const [r, op] of [[this.hover, 0.5], [this.round, 1]]) {
      if (r == null) continue;
      const colW = Math.max(14, (g.W - g.padL - g.padR) / Math.max(1, this.total) * 0.7);
      el('rect', { x: g.x(r) - colW / 2, y: g.padT - 8, width: colW, height: g.H - g.padT - g.padB + 8, rx: 6,
        fill: r === this.round ? 'var(--accent-soft)' : 'var(--hover)', opacity: op }, svg);
    }
    // events: amber dashed marker, diamond on top
    for (const ev of this.events) {
      if (ev.round > this.total) continue;
      const x = g.x(ev.round);
      el('line', { x1: x, x2: x, y1: g.padT - 4, y2: g.H - g.padB, stroke: 'var(--event)', 'stroke-width': 1.2, 'stroke-dasharray': '3 3' }, svg);
      el('path', { d: `M${x} ${g.padT - 14} l4.5 4.5 -4.5 4.5 -4.5 -4.5z`, fill: 'var(--event)' }, svg);
      const t = el('text', { x: x + 8, y: g.padT - 6, fill: 'var(--event-text)', class: 'lbl halo' }, svg);
      t.textContent = 'event';
    }
    const pts = this.stats.filter((s) => s.sentiment != null && !Number.isNaN(s.sentiment));
    if (!pts.length) {
      const t = el('text', { x: (g.padL + g.W - g.padR) / 2, y: g.y(0) - 10, 'text-anchor': 'middle', fill: 'var(--text-dim)', class: 'lbl' }, svg);
      t.textContent = this.stats.length ? 'No scored posts yet' : 'Rounds appear here as they finish';
      return;
    }
    const last = pts[pts.length - 1];
    // one quiet line in secondary ink (monotone, no area fill); the points carry the sign colour
    if (pts.length > 1) {
      const d = monotonePath(pts.map((p) => [g.x(p.round), g.y(p.sentiment)]));
      el('path', { d, fill: 'none', stroke: 'var(--text-2)', 'stroke-width': 1.5, 'stroke-linejoin': 'round', 'stroke-linecap': 'round' }, svg);
    }
    for (const p of pts) {
      const on = p.round === this.round || p.round === this.hover;
      el('circle', { cx: g.x(p.round), cy: g.y(p.sentiment), r: on ? 5.2 : 3.8, fill: colorOf(p.sentiment),
        stroke: 'var(--card)', 'stroke-width': 1.5 }, svg);
    }
    // values: the selected round and the latest one
    const show = new Set([last.round]);
    if (this.round != null) show.add(this.round);
    for (const p of pts) {
      if (!show.has(p.round)) continue;
      const up = p.sentiment < 0.7;
      const t = el('text', { x: g.x(p.round), y: g.y(p.sentiment) + (up ? -10 : 18), 'text-anchor': p.round === this.total ? 'end' : 'middle',
        fill: ({ neg: 'var(--neg-text)', pos: 'var(--pos-text)', neu: 'var(--text-2)' })[signOf(p.sentiment)], class: 'halo', 'font-weight': 600 }, svg);
      t.textContent = formatScore(p.sentiment);
    }
  }
}
