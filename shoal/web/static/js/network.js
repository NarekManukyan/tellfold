// Shoal Studio: agent network canvas.
//
// Exports NetworkView (the avatar crowd graph: camps, shoal and influence layouts) plus a few small shared helpers that kgraph.js and charts.js reuse:
// readPalette / sentimentColor / rgba (colour tokens), ForceSim (Barnes-Hut force layout) and Stage
// (canvas sizing, camera, pointer + wheel input, render loop, theme and motion preferences).
//
// No libraries. All colours come from CSS custom properties on :root and are re-read on
// `studio:themechange` and on system colour-scheme changes.

import { INFLUENCE_RAMP, influenceStep } from './feed.js';
import { avatarDataURI } from './avatar.js';

export { INFLUENCE_RAMP, influenceStep };

// ---------------------------------------------------------------- colour tokens

const TOKEN_NAMES = [
  'ink-0', 'ink-1', 'ink-2', 'line', 'text', 'text-dim', 'accent',
  'sent-neg', 'sent-neu', 'sent-pos', 'event', 'like', 'dislike', 'reply', 'repost', 'follow',
  'card', 'ground', 'hover', 'border', 'border-strong', 'text-2', 'text-faint', 'accent-text', 'pos', 'neg',
];

// Used only when studio.css has not defined a token (e.g. a bare test page). Light, like the default theme.
const FALLBACK_DARK = {
  'ink-0': '#F7F8FA', 'ink-1': '#FFFFFF', 'ink-2': '#FFFFFF', line: '#E5E7EB', text: '#111827',
  'text-dim': '#6B7280', accent: '#6366F1', 'sent-neg': '#DC2626', 'sent-neu': '#6B7280',
  'sent-pos': '#16A34A', event: '#D97706', like: '#16A34A', dislike: '#DC2626', reply: '#6366F1',
  repost: '#A855F7', follow: '#C9CED6', card: '#FFFFFF', ground: '#F7F8FA', hover: '#F3F4F6',
  border: '#E5E7EB', 'border-strong': '#D1D5DB', 'text-2': '#4B5563', 'text-faint': '#9CA3AF',
  'accent-text': '#4F46E5', pos: '#16A34A', neg: '#DC2626',
};

let probeCtx = null;

/** Parse any CSS colour string to [r, g, b, a] (0..255, alpha 0..1). Returns `fallback` if invalid. */
export function parseColor(str, fallback = [128, 128, 128, 1]) {
  if (!str) return fallback;
  if (!probeCtx) {
    const c = document.createElement('canvas');
    c.width = c.height = 1;
    probeCtx = c.getContext('2d', { willReadFrequently: true });
  }
  const sentinel = '#010203';
  probeCtx.fillStyle = sentinel;
  probeCtx.fillStyle = str.trim();
  if (probeCtx.fillStyle === sentinel && str.trim().toLowerCase() !== sentinel) return fallback;
  probeCtx.clearRect(0, 0, 1, 1);
  probeCtx.fillRect(0, 0, 1, 1);
  const d = probeCtx.getImageData(0, 0, 1, 1).data;
  return [d[0], d[1], d[2], d[3] / 255];
}

export function rgba(c, a = 1) {
  return `rgba(${c[0]},${c[1]},${c[2]},${(a * (c[3] ?? 1)).toFixed(3)})`;
}

export function mixColor(a, b, t) {
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, 1].map(
    (v, i) => (i < 3 ? Math.round(v) : v),
  );
}

function luminance(c) {
  const f = (v) => {
    v /= 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2]);
}

/**
 * Read the shared colour tokens. Returns { raw: {name: cssString}, c: {name: [r,g,b,a]}, dark: bool }.
 * `name` is the token without the leading `--`.
 */
export function readPalette(el = document.documentElement) {
  const cs = getComputedStyle(el);
  const raw = {};
  const c = {};
  let ink0 = cs.getPropertyValue('--ink-0').trim();
  const dark = ink0 ? luminance(parseColor(ink0)) < 0.25 : false;
  for (const name of TOKEN_NAMES) {
    const v = cs.getPropertyValue(`--${name}`).trim();
    raw[name] = v || FALLBACK_DARK[name];
    c[name] = parseColor(raw[name], parseColor(FALLBACK_DARK[name]));
  }
  return { raw, c, dark, extra: (name) => cs.getPropertyValue(`--${name}`).trim() };
}

/** Diverging sentiment colour: coral (neg) <- grey (neutral) -> teal (pos). `null` gives neutral. */
export function sentimentColor(score, pal) {
  const neu = pal.c['sent-neu'];
  if (score == null || Number.isNaN(score)) return neu;
  const s = Math.max(-1, Math.min(1, score));
  const t = Math.abs(s) ** 0.75; // lift small values off the neutral grey so they read
  return mixColor(neu, s < 0 ? pal.c['sent-neg'] : pal.c['sent-pos'], t);
}

export function formatScore(s) {
  if (s == null || Number.isNaN(s)) return 'n/a';
  const v = Math.round(s * 100) / 100;
  return (v > 0 ? '+' : v < 0 ? '−' : '') + Math.abs(v).toFixed(2);
}

/** Tiny DOM builder for overlay content: text always goes in through textContent. */
export function h(tag, cls, text, ...children) {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text != null && text !== '') n.textContent = String(text);
  for (const c of children) if (c) n.appendChild(c);
  return n;
}

export function escapeHTML(s) {
  return String(s ?? '').replace(/[&<>"']/g, (ch) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  })[ch]);
}

/** Small deterministic string hash in [0, 1). */
export function hash01(str) {
  let h = 2166136261;
  for (let i = 0; i < str.length; i++) {
    h ^= str.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return ((h >>> 0) % 1000003) / 1000003;
}

const MONO = 'ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace';
const SANS = 'system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif';

// Shared styles for the overlay DOM (tooltips, captions, legends). Scoped by class prefix.
export function injectVizStyles() {
  if (document.getElementById('shoal-viz-style')) return;
  const style = document.createElement('style');
  style.id = 'shoal-viz-style';
  style.textContent = `
.sv-overlay{position:absolute;inset:0;pointer-events:none;overflow:hidden;z-index:2;font-family:${SANS}}
.sv-tip{position:absolute;left:0;top:0;max-width:280px;padding:8px 10px;border-radius:8px;
  background:var(--card,#fff);color:var(--text,#111827);border:1px solid var(--border,#E5E7EB);
  box-shadow:var(--shadow-pop,0 8px 24px rgba(17,24,39,.1));
  font-size:12px;line-height:1.45;opacity:0;transition:opacity .1s ease;font-variant-numeric:tabular-nums;will-change:transform}
.sv-tip.sv-on{opacity:1}
.sv-tip .sv-mono{font-family:${MONO};font-variant-numeric:tabular-nums}
.sv-tip .sv-dim{color:var(--text-dim,#6B7280)}
.sv-tip .sv-row{display:flex;gap:10px;align-items:baseline;justify-content:space-between}
.sv-tip .sv-small{font-size:11px}
.sv-tip .sv-name{font-weight:600;font-size:13px}
.sv-tip .sv-neg{color:var(--neg-text,#DC2626)} .sv-tip .sv-pos{color:var(--pos-text,#15803D)}
.sv-tip .sv-camp{display:inline-flex;align-items:center;gap:5px}
.sv-tip .sv-camp i{width:8px;height:8px;border-radius:50%;display:inline-block}
.sv-tip .sv-tag{display:inline-block;margin-top:4px;padding:0 6px;border-radius:999px;background:var(--hover,#F3F4F6);
  color:var(--text-2,#4B5563);font-size:11px}
.sv-caption{position:absolute;left:50%;top:12px;transform:translate(-50%,-4px);max-width:min(86%,620px);
  padding:8px 12px;border-radius:8px;background:var(--card,#fff);color:var(--text,#111827);
  border:1px solid color-mix(in srgb,var(--event,#D97706) 45%,transparent);box-shadow:var(--shadow-pop,none);
  font-size:13px;line-height:1.45;opacity:0;text-align:left;transition:opacity .3s ease,transform .3s ease}
.sv-caption.sv-on{opacity:1;transform:translate(-50%,0)}
.sv-caption b{color:var(--event-text,#B45309);font-weight:600;margin-right:8px}
.sv-legend{position:absolute;left:10px;bottom:10px;display:flex;flex-wrap:wrap;gap:4px;max-width:calc(100% - 20px);
  pointer-events:auto;font-size:11px}
.sv-legend button{display:inline-flex;align-items:center;gap:6px;height:24px;padding:0 9px;border-radius:999px;
  background:var(--card,#fff);color:var(--text-2,#4B5563);border:1px solid var(--border,#E5E7EB);
  font:inherit;font-size:12px;cursor:pointer}
.sv-legend button:hover,.sv-legend button[aria-pressed="true"]{color:var(--text,#111827);border-color:var(--border-strong,#D1D5DB);background:var(--hover,#F3F4F6)}
.sv-legend button:focus-visible{outline:2px solid var(--accent,#6366F1);outline-offset:1px}
.sv-legend i{display:inline-block;width:9px;height:9px}
.sv-legend .sv-n{font-variant-numeric:tabular-nums;color:var(--text-dim,#6B7280)}
@media (prefers-reduced-motion: reduce){.sv-tip,.sv-caption{transition:none}}
`;
  document.head.appendChild(style);
}

// ---------------------------------------------------------------- force simulation

/**
 * Velocity-Verlet-ish force layout in the style of a classic force-directed graph:
 * many-body repulsion through a Barnes-Hut quadtree (O(n log n)), soft collision, link springs,
 * gentle centering. Nodes are plain objects with x, y, vx, vy and optional r, charge, fx, fy.
 */
export class ForceSim {
  constructor(opts = {}) {
    Object.assign(this, {
      charge: -70,
      theta: 0.9,
      distanceMin: 4,
      distanceMax: 420,
      linkDistance: 46,
      linkStrength: 0.7,
      center: 0.006,
      centerFloor: 0, // centering never weaker than center * centerFloor, even when alpha has decayed
      aspect: 0, // target width / height of the cloud (0 = leave the shape alone); see _shape()
      flat: 0, // current shaping strength: > 0 flattens vertically, < 0 horizontally (adapted every tick)
      collide: 2,
      collideStrength: 0.7,
      velocityDecay: 0.42,
      alpha: 1,
      alphaMin: 0.001,
      alphaDecay: 0.0228,
      alphaTarget: 0,
    }, opts);
    this.nodes = [];
    this.links = [];
  }

  setNodes(nodes) { this.nodes = nodes; }

  setLinks(links) {
    this.links = links;
    const count = new Map();
    for (const l of links) {
      count.set(l.source, (count.get(l.source) || 0) + 1);
      count.set(l.target, (count.get(l.target) || 0) + 1);
    }
    for (const l of links) {
      const cs = count.get(l.source), ct = count.get(l.target);
      l.bias = cs / (cs + ct);
      l.k = (l.strength ?? 1) * this.linkStrength / Math.min(cs, ct);
    }
  }

  reheat(a = 0.3) { this.alpha = Math.max(this.alpha, a); }

  get settled() { return this.alpha < this.alphaMin && this.alphaTarget < this.alphaMin; }

  tick() {
    this.alpha += (this.alphaTarget - this.alpha) * this.alphaDecay;
    const a = this.alpha;
    const nodes = this.nodes;
    if (!nodes.length) return;
    this._links(a);
    this._charge(a);
    const c = this.center * Math.max(a, this.centerFloor);
    for (const n of nodes) {
      const g = n.gravity ?? 1; // a node may be held closer to the centre (isolated graph entities)
      n.vx -= n.x * c * g;
      n.vy -= n.y * c * g;
    }
    this._shape(Math.max(a, this.centerFloor));
    const keep = 1 - this.velocityDecay;
    for (const n of nodes) {
      if (n.fx != null) { n.x = n.fx; n.vx = 0; } else { n.vx *= keep; n.x += n.vx; }
      if (n.fy != null) { n.y = n.fy; n.vy = 0; } else { n.vy *= keep; n.y += n.vy; }
    }
  }

  /**
   * Shape the cloud to a target aspect ratio. A force-directed cloud settles roughly round whatever the
   * centering, so a wide panel would show a small disc in its middle. A gentle squeeze along one axis,
   * whose strength follows the measured width / height of the cloud, stretches it to fit instead.
   */
  _shape(a) {
    if (!this.aspect || this.nodes.length < 3) return;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of this.nodes) {
      if (n.x < x0) x0 = n.x; if (n.x > x1) x1 = n.x;
      if (n.y < y0) y0 = n.y; if (n.y > y1) y1 = n.y;
    }
    const cur = Math.max(1, x1 - x0) / Math.max(1, y1 - y0);
    this.flat = Math.max(-0.2, Math.min(0.2, this.flat + Math.log(this.aspect / cur) * 0.004));
    const f = Math.abs(this.flat) * a;
    if (this.flat > 0) for (const n of this.nodes) n.vy -= n.y * f;
    else for (const n of this.nodes) n.vx -= n.x * f;
  }

  _links(a) {
    for (const l of this.links) {
      const s = l.source, t = l.target;
      let dx = t.x + t.vx - s.x - s.vx || 1e-3 * (hash01(s.id || '') - 0.5);
      let dy = t.y + t.vy - s.y - s.vy || 1e-3 * (hash01(t.id || '') - 0.5);
      let d = Math.sqrt(dx * dx + dy * dy);
      d = (d - (l.distance ?? this.linkDistance)) / d * a * l.k;
      dx *= d; dy *= d;
      t.vx -= dx * l.bias; t.vy -= dy * l.bias;
      s.vx += dx * (1 - l.bias); s.vy += dy * (1 - l.bias);
    }
  }

  _charge(a) {
    const root = this._build();
    if (!root) return;
    const theta2 = this.theta * this.theta;
    const dmin2 = this.distanceMin * this.distanceMin;
    const dmax2 = this.distanceMax * this.distanceMax;
    const collide = this.collide, cs = this.collideStrength;
    const defCharge = this.charge;
    const stack = [];
    for (const n of this.nodes) {
      stack.length = 0;
      stack.push(root);
      while (stack.length) {
        const cell = stack.pop();
        const dx = cell.cx - n.x, dy = cell.cy - n.y;
        let l = dx * dx + dy * dy;
        const w = cell.s;
        if (cell.kids) {
          if (w * w / theta2 < l) {
            if (l < dmax2) {
              if (l < dmin2) l = Math.sqrt(dmin2 * l);
              n.vx += dx * cell.q * a / l;
              n.vy += dy * cell.q * a / l;
            }
            continue;
          }
          for (let i = 0; i < 4; i++) if (cell.kids[i]) stack.push(cell.kids[i]);
          continue;
        }
        for (let b = cell.body; b; b = b._next) {
          if (b === n) continue;
          let bx = b.x - n.x, by = b.y - n.y;
          if (bx === 0 && by === 0) { bx = (hash01(n.id + b.id) - 0.5) * 1e-2; by = (hash01(b.id + n.id) - 0.5) * 1e-2; }
          const raw = bx * bx + by * by;
          let ll = raw;
          if (ll < dmax2) {
            if (ll < dmin2) ll = Math.sqrt(dmin2 * ll);
            const q = b.charge ?? defCharge;
            n.vx += bx * q * a / ll;
            n.vy += by * q * a / ll;
          }
          const rr = (n.r || 0) + (b.r || 0) + collide;
          if (raw < rr * rr) {
            const d = Math.sqrt(raw) || 0.01;
            const o = (rr - d) / d * 0.5 * cs;
            n.vx -= bx * o; n.vy -= by * o;
          }
        }
      }
    }
  }

  _build() {
    const nodes = this.nodes;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of nodes) {
      if (n.x < x0) x0 = n.x; if (n.x > x1) x1 = n.x;
      if (n.y < y0) y0 = n.y; if (n.y > y1) y1 = n.y;
    }
    if (!Number.isFinite(x0)) return null;
    const size = Math.max(x1 - x0, y1 - y0, 1) * 1.001 + 1;
    const root = { x: x0, y: y0, s: size, kids: null, body: null, depth: 0 };
    for (const n of nodes) { n._next = null; this._insert(root, n); }
    this._accumulate(root);
    return root;
  }

  _insert(cell, n) {
    for (;;) {
      if (!cell.kids) {
        if (!cell.body) { cell.body = n; return; }
        if (cell.depth > 28) { n._next = cell.body; cell.body = n; return; } // coincident points
        const old = cell.body;
        cell.body = null;
        cell.kids = [null, null, null, null];
        // re-insert the chain that lived here
        for (let b = old; b;) { const next = b._next; b._next = null; this._child(cell, b); b = next; }
      }
      cell = this._childCell(cell, n);
    }
  }

  _childCell(cell, n) {
    const h = cell.s / 2;
    const i = (n.x >= cell.x + h ? 1 : 0) + (n.y >= cell.y + h ? 2 : 0);
    let k = cell.kids[i];
    if (!k) {
      k = cell.kids[i] = { x: cell.x + (i & 1 ? h : 0), y: cell.y + (i & 2 ? h : 0), s: h, kids: null, body: null, depth: cell.depth + 1 };
    }
    return k;
  }

  _child(cell, n) { this._insert(this._childCell(cell, n), n); }

  _accumulate(cell) {
    let q = 0, wx = 0, wy = 0, wt = 0;
    if (cell.kids) {
      for (const k of cell.kids) {
        if (!k) continue;
        this._accumulate(k);
        const w = Math.abs(k.q) || 1e-9;
        q += k.q; wx += k.cx * w; wy += k.cy * w; wt += w;
      }
    } else {
      for (let b = cell.body; b; b = b._next) {
        const c = b.charge ?? this.charge;
        const w = Math.abs(c) || 1e-9;
        q += c; wx += b.x * w; wy += b.y * w; wt += w;
      }
    }
    cell.q = q;
    cell.cx = wt ? wx / wt : cell.x + cell.s / 2;
    cell.cy = wt ? wy / wt : cell.y + cell.s / 2;
  }
}

// ---------------------------------------------------------------- stage (canvas, camera, input, loop)

/**
 * Owns the <canvas>: DPR-correct sizing via ResizeObserver, a pan/zoom camera with auto-fit, pointer,
 * wheel, pinch and keyboard input, the rAF loop (paused while the document is hidden), theme tokens and
 * prefers-reduced-motion. The view supplies callbacks:
 *   step(now, dt) -> bool (wants another frame)      render(ctx, w, h, now)
 *   hit(sx, sy) -> item|null                         bounds() -> {x0,y0,x1,y1}|null (world units)
 *   onHover(item, sx, sy)  onClick(item|null)  onDrag(item, wx, wy)  onDragEnd(item)
 *   onTheme()  onMotion()  onKey(event) -> bool (handled)  onResize(w, h)
 * and may set fitPad {x, top, bottom} (screen px kept free around the bounds) and maxFitZoom.
 */
export class Stage {
  constructor(canvas, view, { label = '' } = {}) {
    injectVizStyles();
    this.canvas = canvas;
    this.view = view;
    this.ctx = canvas.getContext('2d');
    this.w = 0; this.h = 0; this.dpr = 1;
    this.cam = { x: 0, y: 0, k: 1 };
    this.auto = true; // camera follows the layout until the user pans or zooms
    this.raf = 0;
    this.lastT = 0;
    this.destroyed = false;
    this.pointers = new Map();
    this.press = null;
    this.hoverItem = null;

    // Canvas must fill its container: we size the backing store from its CSS box.
    canvas.style.display = 'block';
    canvas.style.width = '100%';
    canvas.style.height = '100%';
    canvas.style.touchAction = 'none';
    canvas.style.outline = 'none';
    if (!canvas.hasAttribute('tabindex')) canvas.tabIndex = 0;
    if (!canvas.hasAttribute('role')) canvas.setAttribute('role', 'img');
    this.ownLabel = !canvas.hasAttribute('aria-label');
    if (this.ownLabel && label) canvas.setAttribute('aria-label', label);

    const parent = canvas.parentElement;
    if (parent && getComputedStyle(parent).position === 'static') parent.style.position = 'relative';
    this.overlay = document.createElement('div');
    this.overlay.className = 'sv-overlay';
    this.tip = document.createElement('div');
    this.tip.className = 'sv-tip';
    this.tip.setAttribute('aria-hidden', 'true');
    this.overlay.appendChild(this.tip);
    parent?.appendChild(this.overlay);

    this.reducedMQ = window.matchMedia('(prefers-reduced-motion: reduce)');
    this.schemeMQ = window.matchMedia('(prefers-color-scheme: dark)');
    this.reduced = this.reducedMQ.matches;
    this.pal = readPalette();

    this._on = [];
    const on = (target, type, fn, opts) => { target.addEventListener(type, fn, opts); this._on.push([target, type, fn, opts]); };
    on(document, 'studio:themechange', () => this._theme());
    on(this.schemeMQ, 'change', () => this._theme());
    on(this.reducedMQ, 'change', () => { this.reduced = this.reducedMQ.matches; view.onMotion?.(); this.request(); });
    on(document, 'visibilitychange', () => { if (document.hidden) this._stop(); else this.request(); });
    on(canvas, 'wheel', (e) => this._wheel(e), { passive: false });
    on(canvas, 'pointerdown', (e) => this._down(e));
    on(canvas, 'pointermove', (e) => this._move(e));
    on(canvas, 'pointerup', (e) => this._up(e));
    on(canvas, 'pointercancel', (e) => this._up(e, true));
    on(canvas, 'pointerleave', (e) => { if (!this.pointers.size) this._hover(null, e); });
    on(canvas, 'dblclick', (e) => { const p = this._local(e); if (!view.hit(p.x, p.y)) this.fit(); });
    on(canvas, 'keydown', (e) => this._key(e));

    this.ro = new ResizeObserver(() => this.resize());
    this.ro.observe(canvas);
    this.resize();
  }

  // --- coordinates
  toScreen(x, y) { return { x: (x - this.cam.x) * this.cam.k + this.w / 2, y: (y - this.cam.y) * this.cam.k + this.h / 2 }; }
  toWorld(sx, sy) { return { x: (sx - this.w / 2) / this.cam.k + this.cam.x, y: (sy - this.h / 2) / this.cam.k + this.cam.y }; }
  _local(e) { const r = this.canvas.getBoundingClientRect(); return { x: e.clientX - r.left, y: e.clientY - r.top }; }

  resize() {
    if (this.destroyed) return;
    if (!this.overlay.isConnected && this.canvas.parentElement) {
      const parent = this.canvas.parentElement;
      if (getComputedStyle(parent).position === 'static') parent.style.position = 'relative';
      parent.appendChild(this.overlay);
    }
    const rect = this.canvas.getBoundingClientRect();
    const dpr = Math.min(window.devicePixelRatio || 1, 3);
    const w = Math.max(1, Math.round(rect.width)), h = Math.max(1, Math.round(rect.height));
    if (w === this.w && h === this.h && dpr === this.dpr) { this.request(); return; }
    this.w = w; this.h = h; this.dpr = dpr;
    this.canvas.width = Math.round(w * dpr);
    this.canvas.height = Math.round(h * dpr);
    this.view.onResize?.(w, h);
    if (this.auto) this._fitNow();
    this.request();
  }

  _theme() {
    this.pal = readPalette();
    this.view.onTheme?.();
    this.request();
  }

  // --- camera
  fitTarget() {
    const b = this.view.bounds();
    if (!b || this.w <= 1 || this.h <= 1) return null; // hidden canvas: keep the camera until it has a size
    // screen-space padding: room for a node's label beside or under the outermost nodes
    const padX = this.view.fitPad?.x ?? 36, padTop = this.view.fitPad?.top ?? 36, padBottom = this.view.fitPad?.bottom ?? 36;
    const bw = Math.max(b.x1 - b.x0, 40), bh = Math.max(b.y1 - b.y0, 40);
    const availW = Math.max(40, this.w - padX * 2), availH = Math.max(40, this.h - padTop - padBottom);
    const k = Math.max(0.15, Math.min(this.view.maxFitZoom ?? 2.2, Math.min(availW / bw, availH / bh)));
    // centre the bounds inside the padded box (top and bottom padding differ)
    const cy = (b.y0 + b.y1) / 2 + (padBottom - padTop) / 2 / k;
    return { x: (b.x0 + b.x1) / 2, y: cy, k };
  }

  _fitNow() { const t = this.fitTarget(); if (t) this.cam = t; }

  fit() { this.auto = true; if (this.reduced) this._fitNow(); this.request(); }

  zoomAt(sx, sy, factor) {
    const before = this.toWorld(sx, sy);
    this.cam.k = Math.max(0.1, Math.min(8, this.cam.k * factor));
    const after = this.toWorld(sx, sy);
    this.cam.x += before.x - after.x;
    this.cam.y += before.y - after.y;
    this.auto = false;
    this.request();
  }

  /** Pan so that world point (x, y) is visible, if it is not. */
  reveal(x, y) {
    if (this.w <= 1 || this.h <= 1) return; // hidden canvas: nothing to reveal into yet
    const s = this.toScreen(x, y);
    const m = 40;
    if (s.x > m && s.x < this.w - m && s.y > m && s.y < this.h - m) return;
    this.auto = false;
    this.cam.x = x; this.cam.y = y;
    this.request();
  }

  // --- input
  _wheel(e) {
    e.preventDefault();
    const p = this._local(e);
    const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? this.h : 1;
    const dy = e.deltaY * unit;
    if (e.ctrlKey || e.metaKey || Math.abs(e.deltaX) < 1) {
      this.zoomAt(p.x, p.y, Math.exp(-dy * (e.ctrlKey ? 0.01 : 0.0015)));
    } else {
      this.cam.x += e.deltaX * unit / this.cam.k;
      this.cam.y += dy / this.cam.k;
      this.auto = false;
      this.request();
    }
  }

  _down(e) {
    if (e.button !== 0 && e.pointerType === 'mouse') return;
    this.canvas.setPointerCapture?.(e.pointerId);
    const p = this._local(e);
    this.pointers.set(e.pointerId, p);
    if (this.pointers.size === 2) {
      const [a, b] = [...this.pointers.values()];
      this.pinch = { d: Math.hypot(a.x - b.x, a.y - b.y) || 1, m: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 } };
      this.press = null;
      return;
    }
    this.press = { x: p.x, y: p.y, item: this.view.hit(p.x, p.y), moved: false, cam: { ...this.cam } };
  }

  _move(e) {
    const p = this._local(e);
    if (this.pointers.has(e.pointerId)) this.pointers.set(e.pointerId, p);
    if (this.pinch && this.pointers.size === 2) {
      const [a, b] = [...this.pointers.values()];
      const d = Math.hypot(a.x - b.x, a.y - b.y) || 1;
      const m = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
      this.zoomAt(m.x, m.y, d / this.pinch.d);
      this.cam.x -= (m.x - this.pinch.m.x) / this.cam.k;
      this.cam.y -= (m.y - this.pinch.m.y) / this.cam.k;
      this.pinch = { d, m };
      return;
    }
    const pr = this.press;
    if (pr) {
      if (!pr.moved && Math.hypot(p.x - pr.x, p.y - pr.y) > 4) {
        pr.moved = true;
        this._hover(null, e);
      }
      if (pr.moved) {
        if (pr.item && this.view.onDrag) {
          const w = this.toWorld(p.x, p.y);
          this.view.onDrag(pr.item, w.x, w.y);
          this.canvas.style.cursor = 'grabbing';
        } else {
          this.cam.x = pr.cam.x - (p.x - pr.x) / this.cam.k;
          this.cam.y = pr.cam.y - (p.y - pr.y) / this.cam.k;
          this.auto = false;
          this.canvas.style.cursor = 'grabbing';
        }
        this.request();
      }
      return;
    }
    this._hover(this.view.hit(p.x, p.y), e);
  }

  _up(e, cancelled = false) {
    this.pointers.delete(e.pointerId);
    if (this.pointers.size < 2) this.pinch = null;
    const pr = this.press;
    this.press = null;
    this.canvas.style.cursor = '';
    if (!pr || cancelled) return;
    if (pr.moved) {
      if (pr.item) this.view.onDragEnd?.(pr.item);
    } else {
      this.canvas.focus({ preventScroll: true });
      this.view.onClick?.(pr.item);
    }
    const p = this._local(e);
    this._hover(this.view.hit(p.x, p.y), e);
  }

  _hover(item, e) {
    const p = e ? this._local(e) : { x: 0, y: 0 };
    if (item !== this.hoverItem) {
      this.hoverItem = item;
      this.canvas.style.cursor = item ? 'pointer' : '';
      this.request();
    }
    this.view.onHover?.(item, p.x, p.y);
  }

  _key(e) {
    if (this.view.onKey?.(e)) { e.preventDefault(); return; }
    const step = 60 / this.cam.k;
    switch (e.key) {
      case '+': case '=': this.zoomAt(this.w / 2, this.h / 2, 1.25); break;
      case '-': case '_': this.zoomAt(this.w / 2, this.h / 2, 0.8); break;
      case '0': this.fit(); break;
      case 'ArrowLeft': this.cam.x -= step; this.auto = false; break;
      case 'ArrowRight': this.cam.x += step; this.auto = false; break;
      case 'ArrowUp': this.cam.y -= step; this.auto = false; break;
      case 'ArrowDown': this.cam.y += step; this.auto = false; break;
      default: return;
    }
    e.preventDefault();
    this.request();
  }

  // --- tooltip
  /** Show the tooltip with `content`: a DOM node built with textContent (model text never goes in as HTML). */
  showTip(content, sx, sy) {
    const tip = this.tip;
    if (content instanceof Node) tip.replaceChildren(content);
    else tip.textContent = String(content ?? '');
    tip.classList.add('sv-on');
    const pw = this.overlay.clientWidth, ph = this.overlay.clientHeight;
    const tw = tip.offsetWidth, th = tip.offsetHeight;
    let x = sx + 14, y = sy + 14;
    if (x + tw > pw - 6) x = sx - tw - 14;
    if (y + th > ph - 6) y = sy - th - 14;
    x = Math.max(6, x); y = Math.max(6, y);
    // canvas may not sit at the overlay origin; offset by its position inside the parent
    const cr = this.canvas.getBoundingClientRect(), or = this.overlay.getBoundingClientRect();
    tip.style.transform = `translate(${Math.round(x + cr.left - or.left)}px,${Math.round(y + cr.top - or.top)}px)`;
  }

  hideTip() { this.tip.classList.remove('sv-on'); }

  // --- loop
  request() {
    if (this.destroyed || this.raf || document.hidden) return;
    this.raf = requestAnimationFrame((t) => this._frame(t));
  }

  _stop() { if (this.raf) cancelAnimationFrame(this.raf); this.raf = 0; this.lastT = 0; }

  _frame(now) {
    this.raf = 0;
    if (this.destroyed) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 3);
    if (dpr !== this.dpr) { this.resize(); }
    const dt = this.lastT ? Math.max(0, Math.min(100, now - this.lastT)) : 16.7;
    this.lastT = now;
    let more = this.view.step(now, dt);
    if (this.auto && !this.press) {
      const t = this.fitTarget();
      if (t) {
        if (this.reduced) this.cam = t;
        else {
          const f = 1 - Math.pow(1 - 0.08, dt / 16.7);
          this.cam.x += (t.x - this.cam.x) * f;
          this.cam.y += (t.y - this.cam.y) * f;
          this.cam.k += (t.k - this.cam.k) * f;
          if (Math.abs(t.k - this.cam.k) > 1e-3 || Math.abs(t.x - this.cam.x) > 0.3 || Math.abs(t.y - this.cam.y) > 0.3) more = true;
        }
      }
    }
    const ctx = this.ctx;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, this.w, this.h);
    this.view.render(ctx, this.w, this.h, now);
    if (more) this.request();
    else this.lastT = 0;
  }

  destroy() {
    this.destroyed = true;
    this._stop();
    this.ro.disconnect();
    for (const [t, type, fn, opts] of this._on) t.removeEventListener(type, fn, opts);
    this._on = [];
    this.overlay.remove();
  }
}

// ---------------------------------------------------------------- crowd view (avatar graph)

/** Camp colours, categorical in camp order; "Other" and the fallback groups are slate. */
export const CAMP_COLORS = ['#6366F1', '#0EA5E9', '#14B8A6', '#A855F7', '#EC4899'];
export const SLATE = '#94A3B8';

/** Avatar diameter (world units = CSS px at zoom 1) on the fixed influence ramp: <0.2 / 0.2 / 0.4 / 0.6 / 0.8+. */
export const AVATAR_RAMP = [22, 26, 30, 36, 44];
export function avatarDiameter(influence) { return AVATAR_RAMP[influenceStep(influence)]; }

// Avatars are rasterised once per persona from the shared SVG (avatar.js), plus a greyed copy for agents
// without a score yet, so drawing 60 of them every frame is a cheap drawImage each.
const SPRITE_PX = 144;
const sprites = new Map(); // key -> {img, grey, ready}
const spriteWaiters = new Set();

function spriteFor(p, org) {
  const key = `${p.id}\u0000${p.handle || ''}\u0000${org ? 1 : 0}\u0000${org ? p.name || '' : ''}`;
  let s = sprites.get(key);
  if (s) return s;
  s = { img: null, grey: null, ready: false };
  sprites.set(key, s);
  const im = new Image();
  im.decoding = 'async';
  im.onload = () => {
    try {
      const c = document.createElement('canvas');
      c.width = c.height = SPRITE_PX;
      const cx = c.getContext('2d');
      cx.drawImage(im, 0, 0, SPRITE_PX, SPRITE_PX);
      s.img = c;
      try {
        const g = document.createElement('canvas');
        g.width = g.height = SPRITE_PX;
        const gx = g.getContext('2d', { willReadFrequently: true });
        gx.drawImage(c, 0, 0);
        const data = gx.getImageData(0, 0, SPRITE_PX, SPRITE_PX);
        const d = data.data;
        for (let i = 0; i < d.length; i += 4) {
          const l = 0.299 * d[i] + 0.587 * d[i + 1] + 0.114 * d[i + 2];
          const v = l * 0.55 + 200 * 0.45; // desaturated and washed toward grey
          d[i] = d[i + 1] = d[i + 2] = v;
        }
        gx.putImageData(data, 0, 0);
        s.grey = g;
      } catch { s.grey = null; }
    } catch {
      s.img = im;
    }
    s.ready = true;
    for (const fn of spriteWaiters) fn();
  };
  im.src = avatarDataURI({ id: p.id, handle: p.handle || '', name: p.name || '', org });
  return s;
}

const ACTION_STYLE = {
  reply: { color: 'accent', width: 1.4, dash: null, arrow: true },
  repost: { color: 'repost', width: 1.4, dash: null, arrow: true },
  like: { color: 'pos', width: 1.6, dash: [0.01, 4.2], cap: 'round' },
  dislike: { color: 'neg', width: 1.3, dash: [5, 3.5] },
};
const VIEW_MS = 620;
const ROUND_MS = 420;
const CURVE_MS = 700;
const CAPTION_MS = 4600;
const easeIO = (t) => (t <= 0 ? 0 : t >= 1 ? 1 : t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);
const easeOut = (t) => (t <= 0 ? 0 : t >= 1 ? 1 : 1 - (1 - t) ** 3);

function quad(p0, c, p1, t) {
  const u = 1 - t;
  return { x: u * u * p0.x + 2 * u * t * c.x + t * t * p1.x, y: u * u * p0.y + 2 * u * t * c.y + t * t * p1.y };
}

function hexToRgb(hex) {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex || '');
  if (!m) return [148, 163, 184, 1];
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 1];
}

export class NetworkView {
  /**
   * @param {HTMLCanvasElement} canvas
   * @param {{onSelectAgent?: (id: string) => void, onHoverAgent?: (id: string|null) => void,
   *          onSelectCamp?: (key: string) => void, onClear?: () => void, compact?: boolean}} opts
   */
  constructor(canvas, { onSelectAgent, onHoverAgent, onSelectCamp, onClear, compact = false } = {}) {
    this.onSelectAgent = onSelectAgent || (() => {});
    this.onHoverAgent = onHoverAgent || (() => {});
    this.onSelectCamp = onSelectCamp || (() => {});
    this.onClear = onClear || (() => {});
    this.compact = compact;
    this.nodes = [];
    this.byId = new Map();
    this.follows = [];
    this.camps = [];
    this.campOf = new Map();
    this.sentiment = {};
    this.hasSentiment = true;
    this.posts = new Map();
    this.actionsByRound = new Map();
    this.view = 'camps';
    this.round = null;
    this.selected = null;
    this.selectedCamp = null;
    this.hovered = null; // {kind:'node', n} | {kind:'edge', e} | {kind:'camp', c}
    this.curves = [];
    this.curveT0 = 0;
    this.infEdges = [];
    this.layers = { regions: { v: 1, t: 1 }, axis: { v: 0, t: 0 }, rings: { v: 0, t: 0 }, follows: { v: 1, t: 1 } };
    this.anim = null; // {t0, dur}
    this.extent = { regions: [], axis: null, rings: null };
    this.fitPad = compact ? { x: 20, top: 30, bottom: 16 } : { x: 48, top: 44, bottom: 40 };
    this.maxFitZoom = compact ? 1.15 : 1.6;
    this.stage = new Stage(canvas, this, { label: 'Crowd' });
    this.caption = document.createElement('div');
    this.caption.className = 'sv-caption';
    this.caption.setAttribute('role', 'status');
    this.caption.setAttribute('aria-live', 'polite');
    this.stage.overlay.appendChild(this.caption);
    this._wake = () => this.stage.request();
    spriteWaiters.add(this._wake);
    this._colors();
  }

  // ------------------------------------------------ data

  /**
   * personas: Persona[]; follows: [[follower, followee]]; sentimentByAgent: {agentId: score|null} up to the
   * playhead; posts / actions: everything known (for this round's curves and the influence edges);
   * orgIds: Set of organisation ids; camps: [{key, name, color, members, sentiment}]; hasSentiment: whether the
   * run scored posts at all. Calling it again with the same personas keeps positions and animates changes.
   */
  setData({ personas, follows, sentimentByAgent, posts, actions, orgIds, camps, hasSentiment } = {}) {
    let relayout = false;
    if (personas) {
      const ids = personas.map((p) => p.id).join('|');
      if (ids !== this._ids) {
        this._ids = ids;
        const old = this.byId;
        this.nodes = personas.map((p) => {
          const prev = old.get(p.id);
          const n = prev || { id: p.id, x: 0, y: 0, sx: 0, sy: 0, tx: 0, ty: 0, a: 1, ta: 1, fresh: true };
          n.p = p;
          return n;
        });
        this.byId = new Map(this.nodes.map((n) => [n.id, n]));
        relayout = true;
      } else {
        for (const p of personas) { const n = this.byId.get(p.id); if (n) n.p = p; }
      }
    }
    if (orgIds) for (const n of this.nodes) n.org = orgIds.has(n.id);
    for (const n of this.nodes) {
      n.d = avatarDiameter(n.p.influence);
      n.sprite = spriteFor(n.p, !!n.org);
    }
    if (follows) this.follows = follows.filter(([a, b]) => this.byId.has(a) && this.byId.has(b));
    if (camps) {
      const key = camps.map((c) => `${c.key}:${c.members.join(',')}`).join('|');
      if (key !== this._campKey) { this._campKey = key; relayout = true; }
      this.camps = camps;
      this.campOf = new Map();
      for (const c of camps) for (const id of c.members) this.campOf.set(id, c);
    }
    if (typeof hasSentiment === 'boolean') this.hasSentiment = hasSentiment;
    if (sentimentByAgent) {
      const before = JSON.stringify(this.sentiment);
      this.sentiment = sentimentByAgent;
      if (this.view === 'shoal' && JSON.stringify(sentimentByAgent) !== before) relayout = true;
    }
    if (posts) this.posts = new Map(posts.map((p) => [p.id, p]));
    if (actions) {
      this.actionsByRound = new Map();
      for (const a of actions) {
        if (!this.actionsByRound.has(a.round)) this.actionsByRound.set(a.round, []);
        this.actionsByRound.get(a.round).push(a);
      }
    }
    this._edges();
    if (relayout) this._layout(this.nodes.some((n) => n.fresh) ? 0 : ROUND_MS);
    this.stage.request();
  }

  /** 'camps' | 'shoal' | 'influence'. Shoal without per-agent sentiment falls back to the camps layout. */
  setView(view) {
    if (!['camps', 'shoal', 'influence'].includes(view) || view === this.view) return;
    this.view = view;
    this._edges();
    this._layout(VIEW_MS);
  }

  get effectiveView() { return this.view === 'shoal' && !this.hasSentiment ? 'camps' : this.view; }

  setRound(round) {
    const changed = round !== this.round;
    this.round = round;
    this._edges();
    if (changed) this.curveT0 = performance.now();
    this.stage.request();
  }

  /** Animate a round: this round's interactions draw in along their curves. */
  playRound(round, actions = null) {
    if (actions) this.actionsByRound.set(round, actions);
    this.round = round;
    this._edges();
    this.curveT0 = performance.now();
    this.stage.request();
  }

  showEvent(text) {
    if (!text) return;
    const b = document.createElement('b');
    b.textContent = 'Breaking news';
    this.caption.replaceChildren(b, document.createTextNode(String(text)));
    this.caption.classList.add('sv-on');
    clearTimeout(this._capT);
    this._capT = setTimeout(() => this.caption.classList.remove('sv-on'), CAPTION_MS);
  }

  select(id) {
    this.selected = id && this.byId.has(id) ? id : null;
    this._dim();
    const n = this.selected && this.byId.get(this.selected);
    if (n && !this.compact) this.stage.reveal(n.tx, n.ty);
    this.stage.request();
  }

  selectCamp(key) {
    this.selectedCamp = key || null;
    this._dim();
    this.stage.request();
  }

  resize() { this.stage.resize(); }
  fit() { this.stage.fit(); }
  zoomIn() { this.stage.zoomAt(this.stage.w / 2, this.stage.h / 2, 1.25); }
  zoomOut() { this.stage.zoomAt(this.stage.w / 2, this.stage.h / 2, 0.8); }

  destroy() {
    spriteWaiters.delete(this._wake);
    clearTimeout(this._capT);
    this.stage.destroy();
    this.nodes = [];
  }

  // ------------------------------------------------ derived state

  _colors() {
    const pal = this.stage.pal;
    this.C = pal.c;
    this.dark = pal.dark;
    this.campRgb = new Map();
  }

  _campColor(c) {
    if (!c) return hexToRgb(SLATE);
    let v = this.campRgb.get(c.color);
    if (!v) { v = hexToRgb(c.color); this.campRgb.set(c.color, v); }
    return v;
  }

  _dim() {
    const camp = this.selectedCamp && this.camps.find((c) => c.key === this.selectedCamp);
    for (const n of this.nodes) {
      let on = true;
      if (this.selected) on = n.id === this.selected;
      else if (camp) on = camp.members.includes(n.id);
      n.ta = on ? 1 : 0.25;
    }
  }

  _authorOfPost(id) {
    const p = this.posts.get(id);
    return p ? p.author_id : null;
  }

  /** This round's curves (camps view) and the aggregated reply / repost edges up to the playhead (influence). */
  _edges() {
    const r = this.round;
    const curves = [];
    if (r !== null) {
      const acts = this.actionsByRound.get(r);
      const seen = new Set();
      if (acts && acts.length) {
        for (const a of acts) {
          if (!ACTION_STYLE[a.type]) continue;
          const target = a.target_agent_id || (a.target_post_id != null ? this._authorOfPost(a.target_post_id) : null);
          if (!target || target === a.agent_id || !this.byId.has(a.agent_id) || !this.byId.has(target)) continue;
          const k = `${a.type}:${a.agent_id}>${target}`;
          if (seen.has(k)) continue;
          seen.add(k);
          curves.push({ a: a.agent_id, b: target, type: a.type });
        }
      } else {
        for (const p of this.posts.values()) {
          if (p.round !== r) continue;
          const tgtPost = p.parent_id ?? p.repost_of;
          if (tgtPost == null) continue;
          const target = this._authorOfPost(tgtPost);
          if (!target || target === p.author_id || !this.byId.has(target) || !this.byId.has(p.author_id)) continue;
          curves.push({ a: p.author_id, b: target, type: p.repost_of != null ? 'repost' : 'reply' });
        }
      }
    }
    this.curves = curves;

    const agg = new Map();
    for (const p of this.posts.values()) {
      if (r !== null && p.round > r) continue;
      const tgtPost = p.parent_id ?? p.repost_of;
      if (tgtPost == null) continue;
      const target = this._authorOfPost(tgtPost);
      if (!target || target === p.author_id || !this.byId.has(target) || !this.byId.has(p.author_id)) continue;
      const k = `${p.author_id}>${target}`;
      let e = agg.get(k);
      if (!e) { e = { a: p.author_id, b: target, replies: 0, reposts: 0, n: 0 }; agg.set(k, e); }
      if (p.repost_of != null) e.reposts++; else e.replies++;
      e.n++;
    }
    this.infEdges = [...agg.values()];
  }

  // ------------------------------------------------ layouts

  _groups() {
    const groups = this.camps.length ? this.camps.map((c) => ({ camp: c, nodes: c.members.map((id) => this.byId.get(id)).filter(Boolean) }))
      : [{ camp: null, nodes: this.nodes.slice() }];
    const placed = new Set(groups.flatMap((g) => g.nodes.map((n) => n.id)));
    const rest = this.nodes.filter((n) => !placed.has(n.id));
    if (rest.length) groups.push({ camp: null, nodes: rest });
    return groups.filter((g) => g.nodes.length);
  }

  /** Pack a group tightly around its centre: biggest voices first, each on the first free spot of a spiral. */
  _pack(nodes, gap) {
    const sorted = nodes.slice().sort((a, b) => b.d - a.d || (b.p.influence ?? 0) - (a.p.influence ?? 0) || a.id.localeCompare(b.id));
    const out = [];
    for (const n of sorted) {
      let pos = null;
      if (!out.length) pos = { x: 0, y: 0 };
      for (let t = 0; !pos && t < 4000; t++) {
        const ang = t * 0.42;
        const rad = 3.2 * ang;
        const x = Math.cos(ang) * rad, y = Math.sin(ang) * rad;
        let ok = true;
        for (const o of out) {
          const min = (n.d + o.n.d) / 2 + gap;
          if ((x - o.x) ** 2 + (y - o.y) ** 2 < min * min) { ok = false; break; }
        }
        if (ok) pos = { x, y };
      }
      out.push({ n, x: pos ? pos.x : 0, y: pos ? pos.y : 0 });
    }
    let r = 0;
    for (const o of out) r = Math.max(r, Math.hypot(o.x, o.y) + o.n.d / 2);
    return { items: out, r };
  }

  _layoutCamps() {
    const gap = this.compact ? 5 : 8;
    const groups = this._groups().map((g) => ({ ...g, ...this._pack(g.nodes, gap) }));
    const k = groups.length;
    const pad = this.compact ? 16 : 22;
    let centres;
    if (k === 1) centres = [{ x: 0, y: 0 }];
    else {
      const unit = groups.map((_, i) => {
        const a = (k === 2 ? Math.PI : -Math.PI / 2) + (2 * Math.PI * i) / k;
        return { x: Math.cos(a) * 1.55, y: Math.sin(a) };
      });
      let s = 1;
      // rough label width in world units ("Name · n · +0.00"), so labels at the same height never collide
      const lw = groups.map((g) => ((g.camp ? g.camp.name.length : 8) + 12) * (this.compact ? 6.6 : 7));
      for (let i = 0; i < k; i++) for (let j = i + 1; j < k; j++) {
        const need = groups[i].r + groups[j].r + pad * 2 + (this.compact ? 26 : 40);
        const d = Math.hypot(unit[i].x - unit[j].x, unit[i].y - unit[j].y) || 1;
        s = Math.max(s, need / d);
        const dx = Math.abs(unit[i].x - unit[j].x), dy = Math.abs(unit[i].y - unit[j].y);
        if (dy < 0.6 && dx > 0.05) s = Math.max(s, ((lw[i] + lw[j]) / 2 + 16) / dx);
      }
      centres = unit.map((u) => ({ x: u.x * s, y: u.y * s }));
    }
    groups.forEach((g, i) => {
      for (const it of g.items) { it.n.tx = centres[i].x + it.x; it.n.ty = centres[i].y + it.y; }
    });
    return groups.map((g, i) => ({ camp: g.camp, cx: centres[i].x, cy: centres[i].y, r: g.r + pad, n: g.nodes.length }));
  }

  _layoutShoal() {
    const n = this.nodes.length;
    const W = Math.max(this.compact ? 420 : 620, n * (this.compact ? 20 : 26));
    const gap = this.compact ? 3 : 5;
    const scored = [];
    const lane = [];
    for (const nd of this.nodes) {
      const s = this.sentiment[nd.id];
      nd.unscored = !(typeof s === 'number' && !Number.isNaN(s));
      if (nd.unscored) lane.push(nd);
      else scored.push({ nd, x: Math.max(-1, Math.min(1, s)) * (W / 2) });
    }
    // agents with a score: x is their mean sentiment, stacked around the centre line (beeswarm)
    scored.sort((a, b) => b.nd.d - a.nd.d || a.x - b.x || a.nd.id.localeCompare(b.nd.id));
    const placed = [];
    for (const it of scored) {
      let best = 0;
      for (let step = 0; step < 400; step++) {
        const y = (step % 2 ? 1 : -1) * Math.ceil(step / 2) * 3;
        let ok = true;
        for (const o of placed) {
          const min = (it.nd.d + o.nd.d) / 2 + gap;
          if ((it.x - o.x) ** 2 + (y - o.y) ** 2 < min * min) { ok = false; break; }
        }
        if (ok) { best = y; break; }
      }
      it.y = best;
      placed.push(it);
      it.nd.tx = it.x;
      it.nd.ty = it.y;
    }
    let y1 = 0, y0 = 0;
    for (const it of placed) { y1 = Math.max(y1, it.y + it.nd.d / 2); y0 = Math.min(y0, it.y - it.nd.d / 2); }
    if (!placed.length) { y0 = -20; y1 = 20; }
    // agents with no score yet are not neutral: they wait in their own lane under the axis
    let laneTop = null, laneBottom = null;
    if (lane.length) {
      lane.sort((a, b) => a.id.localeCompare(b.id));
      laneTop = y1 + 22 + 58;
      const cell = Math.max(...lane.map((nd) => nd.d)) + gap + 2;
      const perRow = Math.max(1, Math.floor(W / cell));
      const rows = Math.ceil(lane.length / perRow);
      lane.forEach((nd, i) => {
        const row = Math.floor(i / perRow);
        const inRow = row === rows - 1 ? lane.length - row * perRow : perRow;
        const col = i % perRow;
        nd.tx = (col - (inRow - 1) / 2) * cell;
        nd.ty = laneTop + cell / 2 + row * cell;
      });
      laneBottom = laneTop + rows * cell;
    }
    return { half: W / 2, y0, y1, laneTop, laneBottom, laneN: lane.length };
  }

  _layoutInfluence() {
    const n = this.nodes.length;
    const R = Math.max(this.compact ? 150 : 210, Math.sqrt(n) * (this.compact ? 34 : 46));
    const order = [];
    for (const g of this._groups()) order.push(...g.nodes.slice().sort((a, b) => a.id.localeCompare(b.id)));
    const items = order.map((nd, i) => {
      const inf = Math.max(0, Math.min(1, Number(nd.p.influence ?? 0.5)));
      const rr = (1 - inf) * R;
      const a = -Math.PI / 2 + (2 * Math.PI * i) / Math.max(1, order.length);
      return { nd, rr, x: Math.cos(a) * Math.max(rr, 4), y: Math.sin(a) * Math.max(rr, 4) };
    });
    const gap = this.compact ? 4 : 7;
    for (let it = 0; it < 120; it++) {
      for (let i = 0; i < items.length; i++) for (let j = i + 1; j < items.length; j++) {
        const A = items[i], B = items[j];
        const dx = B.x - A.x, dy = B.y - A.y;
        const d = Math.hypot(dx, dy) || 0.01;
        const min = (A.nd.d + B.nd.d) / 2 + gap;
        if (d < min) {
          const push = (min - d) / 2 / d;
          A.x -= dx * push; A.y -= dy * push; B.x += dx * push; B.y += dy * push;
        }
      }
      for (const A of items) {
        const d = Math.hypot(A.x, A.y) || 0.01;
        const want = d + (A.rr - d) * 0.35;
        A.x *= want / d; A.y *= want / d;
      }
    }
    for (const A of items) { A.nd.tx = A.x; A.nd.ty = A.y; }
    return { R };
  }

  _layout(dur) {
    if (!this.nodes.length) return;
    const v = this.effectiveView;
    this.extent = { regions: [], axis: null, rings: null };
    if (v === 'camps') this.extent.regions = this._layoutCamps();
    else if (v === 'shoal') this.extent.axis = this._layoutShoal();
    else this.extent.rings = this._layoutInfluence();
    if (v !== 'shoal') for (const n of this.nodes) n.unscored = false;
    this.layers.regions.t = v === 'camps' ? 1 : 0;
    this.layers.follows.t = v === 'camps' ? 1 : 0;
    this.layers.axis.t = v === 'shoal' ? 1 : 0;
    this.layers.rings.t = v === 'influence' ? 1 : 0;
    const now = performance.now();
    const instant = dur === 0 || this.stage.reduced;
    for (const n of this.nodes) {
      if (n.fresh || instant) { n.x = n.sx = n.tx; n.y = n.sy = n.ty; n.fresh = false; }
      else { n.sx = n.x; n.sy = n.y; }
    }
    if (instant) for (const l of Object.values(this.layers)) l.v = l.t;
    this.anim = instant ? null : { t0: now, dur };
    this._dim();
    if (this.selected) { const s = this.byId.get(this.selected); if (s && !this.compact) this.stage.reveal(s.tx, s.ty); }
    this.stage.request();
  }

  // ------------------------------------------------ Stage callbacks

  step(now, dt) {
    let more = false;
    if (this.anim) {
      const t = (now - this.anim.t0) / this.anim.dur;
      const e = easeIO(t);
      for (const n of this.nodes) { n.x = n.sx + (n.tx - n.sx) * e; n.y = n.sy + (n.ty - n.sy) * e; }
      if (t >= 1) this.anim = null; else more = true;
    }
    const f = this.stage.reduced ? 1 : 1 - Math.pow(1 - 0.16, dt / 16.7);
    for (const l of Object.values(this.layers)) {
      if (Math.abs(l.t - l.v) > 0.004) { l.v += (l.t - l.v) * f; more = true; } else l.v = l.t;
    }
    for (const n of this.nodes) {
      if (Math.abs(n.ta - n.a) > 0.004) { n.a += (n.ta - n.a) * f; more = true; } else n.a = n.ta;
    }
    if (!this.stage.reduced && this.curves.length && now - this.curveT0 < CURVE_MS + 40) more = true;
    return more;
  }

  bounds() {
    if (!this.nodes.length) return null;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of this.nodes) {
      const r = n.d / 2 + 4;
      x0 = Math.min(x0, n.tx - r); x1 = Math.max(x1, n.tx + r);
      y0 = Math.min(y0, n.ty - r); y1 = Math.max(y1, n.ty + r + 14);
    }
    const ex = this.extent;
    for (const g of ex.regions || []) {
      x0 = Math.min(x0, g.cx - g.r); x1 = Math.max(x1, g.cx + g.r);
      y0 = Math.min(y0, g.cy - g.r - 22); y1 = Math.max(y1, g.cy + g.r);
    }
    if (ex.axis) { x0 = Math.min(x0, -ex.axis.half - 20); x1 = Math.max(x1, ex.axis.half + 20); y1 = Math.max(y1, (ex.axis.laneBottom ?? ex.axis.y1) + 44); y0 = Math.min(y0, ex.axis.y0 - 12); }
    if (ex.rings) { const R = ex.rings.R; x0 = Math.min(x0, -R); x1 = Math.max(x1, R); y0 = Math.min(y0, -R); y1 = Math.max(y1, R); }
    return { x0, y0, x1, y1 };
  }

  onTheme() { this._colors(); }
  onMotion() { this.stage.request(); }
  onResize() {}

  _nodeAt(sx, sy) {
    const k = this.stage.cam.k;
    let best = null, bestD = Infinity;
    for (const n of this.nodes) {
      const s = this.stage.toScreen(n.x, n.y);
      const r = (n.d * k) / 2 + 3;
      const d = Math.hypot(s.x - sx, s.y - sy);
      if (d <= r && d < bestD) { best = n; bestD = d; }
    }
    return best;
  }

  _edgeAt(sx, sy) {
    if (this.effectiveView !== 'influence' || this.layers.rings.v < 0.5) return null;
    let best = null, bestD = 6;
    for (const e of this.infEdges) {
      const g = this._edgeGeom(e);
      if (!g) continue;
      for (let i = 0; i <= 16; i++) {
        const p = quad(g.p0, g.c, g.p1, i / 16);
        const d = Math.hypot(p.x - sx, p.y - sy);
        if (d < bestD) { bestD = d; best = e; }
      }
    }
    return best;
  }

  _regionAt(sx, sy) {
    if (this.effectiveView !== 'camps' || !this.camps.length) return null;
    const w = this.stage.toWorld(sx, sy);
    for (const g of this.extent.regions || []) {
      if (g.camp && Math.hypot(w.x - g.cx, w.y - g.cy) <= g.r) return g.camp;
    }
    return null;
  }

  hit(sx, sy) {
    const n = this._nodeAt(sx, sy);
    if (n) return { kind: 'node', n };
    const e = this._edgeAt(sx, sy);
    if (e) return { kind: 'edge', e };
    const c = this._regionAt(sx, sy);
    if (c) return { kind: 'camp', c };
    return null;
  }

  onHover(item, sx, sy) {
    const prevNode = this.hovered && this.hovered.kind === 'node' ? this.hovered.n.id : null;
    this.hovered = item;
    const nowNode = item && item.kind === 'node' ? item.n.id : null;
    if (prevNode !== nowNode) this.onHoverAgent(nowNode);
    if (!item || item.kind === 'camp') { this.stage.hideTip(); this.stage.request(); return; }
    this.stage.showTip(item.kind === 'node' ? this._nodeTip(item.n) : this._edgeTip(item.e), sx, sy);
    this.stage.request();
  }

  onClick(item) {
    if (!item) { this.onClear(); return; }
    if (item.kind === 'node') this.onSelectAgent(item.n.id);
    else if (item.kind === 'camp') this.onSelectCamp(item.c.key);
    else if (item.kind === 'edge') this.onSelectAgent(item.e.a);
  }

  _handle(id) { const n = this.byId.get(id); return n ? `@${n.p.handle}` : id; }

  _nodeTip(n) {
    const p = n.p;
    const box = h('div');
    box.appendChild(h('div', 'sv-name', p.name || p.handle));
    box.appendChild(h('div', 'sv-dim', `@${p.handle}${n.org ? ' · organisation' : ''}`));
    const camp = this.campOf.get(n.id);
    const row = h('div', 'sv-row');
    if (camp) {
      const c = h('span', 'sv-camp');
      const dot = document.createElement('i');
      dot.style.background = camp.color;
      c.append(dot, document.createTextNode(camp.name));
      row.appendChild(c);
    } else row.appendChild(h('span', 'sv-dim', 'No camp'));
    const s = this.sentiment[n.id];
    const scored = typeof s === 'number' && !Number.isNaN(s);
    row.appendChild(h('span', `sv-mono ${scored ? (s < -0.15 ? 'sv-neg' : s > 0.15 ? 'sv-pos' : '') : 'sv-dim'}`,
      scored ? formatScore(s) : (this.hasSentiment ? 'no score yet' : '')));
    box.appendChild(row);
    box.appendChild(h('div', 'sv-dim sv-small', `Influence ${(Number(p.influence ?? 0)).toFixed(2)}`));
    return box;
  }

  _edgeTip(e) {
    const box = h('div');
    const parts = [];
    if (e.replies) parts.push(`${this._handle(e.a)} replied to ${this._handle(e.b)} ${e.replies}×`);
    if (e.reposts) parts.push(`${this._handle(e.a)} reposted ${this._handle(e.b)} ${e.reposts}×`);
    for (const t of parts) box.appendChild(h('div', '', t));
    box.appendChild(h('div', 'sv-dim sv-small', this.round === null ? 'All rounds' : `Up to round ${this.round}`));
    return box;
  }

  // ------------------------------------------------ drawing

  _edgeGeom(e) {
    const A = this.byId.get(e.a), B = this.byId.get(e.b);
    if (!A || !B) return null;
    const st = this.stage, k = st.cam.k;
    const p0 = st.toScreen(A.x, A.y), p1 = st.toScreen(B.x, B.y);
    const dx = p1.x - p0.x, dy = p1.y - p0.y;
    const len = Math.hypot(dx, dy) || 1;
    const ux = dx / len, uy = dy / len;
    const ra = (A.d * k) / 2 + 2, rb = (B.d * k) / 2 + 3;
    if (len < ra + rb + 2) return null;
    const a0 = { x: p0.x + ux * ra, y: p0.y + uy * ra };
    const b0 = { x: p1.x - ux * rb, y: p1.y - uy * rb };
    const bend = Math.min(40, len * 0.16);
    const c = { x: (a0.x + b0.x) / 2 - uy * bend, y: (a0.y + b0.y) / 2 + ux * bend };
    return { p0: a0, p1: b0, c };
  }

  _drawCurve(ctx, g, col, width, dash, cap, arrow, alpha, prog) {
    ctx.save();
    ctx.globalAlpha = alpha;
    ctx.strokeStyle = rgba(col);
    ctx.lineWidth = width;
    ctx.lineCap = cap || 'butt';
    ctx.setLineDash(dash || []);
    ctx.beginPath();
    ctx.moveTo(g.p0.x, g.p0.y);
    const steps = 18;
    const end = Math.max(0.02, prog);
    let last = g.p0;
    for (let i = 1; i <= steps; i++) {
      const p = quad(g.p0, g.c, g.p1, (i / steps) * end);
      ctx.lineTo(p.x, p.y);
      last = p;
    }
    ctx.stroke();
    if (arrow && prog >= 0.98) {
      const p = quad(g.p0, g.c, g.p1, 0.94);
      const ang = Math.atan2(last.y - p.y, last.x - p.x);
      const s = 4 + width * 1.4;
      ctx.setLineDash([]);
      ctx.fillStyle = rgba(col);
      ctx.beginPath();
      ctx.moveTo(last.x, last.y);
      ctx.lineTo(last.x - Math.cos(ang - 0.45) * s, last.y - Math.sin(ang - 0.45) * s);
      ctx.lineTo(last.x - Math.cos(ang + 0.45) * s, last.y - Math.sin(ang + 0.45) * s);
      ctx.closePath();
      ctx.fill();
    }
    ctx.restore();
  }

  _currentRegions() {
    // regions follow their members while nodes move
    const out = [];
    for (const g of this.extent.regions || []) {
      const members = g.camp ? g.camp.members.map((id) => this.byId.get(id)).filter(Boolean)
        : this.nodes.filter((n) => !this.campOf.has(n.id));
      if (!members.length) continue;
      let cx = 0, cy = 0;
      for (const n of members) { cx += n.x; cy += n.y; }
      cx /= members.length; cy /= members.length;
      let r = 0;
      for (const n of members) r = Math.max(r, Math.hypot(n.x - cx, n.y - cy) + n.d / 2);
      out.push({ camp: g.camp, cx, cy, r: Math.max(r + (this.compact ? 10 : 14), 28), n: members.length });
    }
    return out;
  }

  render(ctx, w, h) {
    const st = this.stage, k = st.cam.k, C = this.C;
    if (!this.nodes.length) return;
    const SANS = 'system-ui, -apple-system, "Segoe UI", Roboto, sans-serif';
    const MONO = 'ui-monospace, SFMono-Regular, Menlo, monospace';
    const L = this.layers;
    const campSel = this.selectedCamp;
    const dimOther = !!(this.selected || campSel);

    // regions (camps view)
    const regionLabels = [];
    if (L.regions.v > 0.01) {
      for (const g of this._currentRegions().sort((a, b) => a.cy - b.cy)) {
        const s = st.toScreen(g.cx, g.cy);
        const r = g.r * k;
        const col = this._campColor(g.camp);
        const isSel = campSel && g.camp && g.camp.key === campSel;
        const fade = L.regions.v * (dimOther && !isSel ? (this.selected ? 0.55 : 0.4) : 1);
        ctx.save();
        ctx.globalAlpha = fade;
        ctx.fillStyle = rgba(col, 0.1);
        ctx.strokeStyle = rgba(col, 0.35);
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.arc(s.x, s.y, r, 0, Math.PI * 2);
        ctx.fill();
        ctx.stroke();
        // label: "Name · n · −0.86", the name 11.5/600 sans, the figures 11 mono
        const name = g.camp ? g.camp.name : 'Everyone';
        const sv = g.camp && typeof g.camp.sentiment === 'number' ? formatScore(g.camp.sentiment) : null;
        const rest = ` · ${g.n}${sv ? ` · ${sv}` : ''}`;
        const nameFont = `600 11.5px ${SANS}`, restFont = `400 11px ${MONO}`;
        ctx.font = nameFont;
        const nw = ctx.measureText(name).width;
        ctx.font = restFont;
        const rw = ctx.measureText(rest).width;
        let ly = s.y - r - 8;
        let lx = s.x - (nw + rw) / 2;
        lx = Math.max(6, Math.min(w - nw - rw - 6, lx));
        // a label that would collide with one already drawn moves under its circle
        const hit = (y) => regionLabels.some((b) => b.x0 < lx + nw + rw && lx < b.x1 && b.y0 < y + 4 && y - 14 < b.y1);
        if (hit(ly)) ly = s.y + r + 18;
        regionLabels.push({ x0: lx - 4, x1: lx + nw + rw + 4, y0: ly - 14, y1: ly + 4 });
        ctx.textBaseline = 'alphabetic';
        ctx.lineJoin = 'round';
        ctx.lineWidth = 3;
        ctx.strokeStyle = rgba(C.ground);
        ctx.font = nameFont;
        ctx.strokeText(name, lx, ly);
        ctx.fillStyle = rgba(C.text);
        ctx.fillText(name, lx, ly);
        ctx.font = restFont;
        ctx.strokeText(rest, lx + nw, ly);
        ctx.fillStyle = rgba(C['text-dim']);
        ctx.fillText(rest, lx + nw, ly);
        ctx.restore();
      }
    }

    // ring guides (influence view)
    if (L.rings.v > 0.01 && this.extent.rings) {
      const R = this.extent.rings.R;
      const o = st.toScreen(0, 0);
      ctx.save();
      ctx.globalAlpha = L.rings.v;
      ctx.strokeStyle = rgba(C.border);
      ctx.lineWidth = 1;
      ctx.setLineDash([3, 4]);
      ctx.font = `500 11px ${SANS}`;
      ctx.fillStyle = rgba(C['text-dim']);
      ctx.textAlign = 'left';
      for (const inf of [0.75, 0.5, 0.25]) {
        const rr = (1 - inf) * R * k;
        ctx.beginPath();
        ctx.arc(o.x, o.y, rr, 0, Math.PI * 2);
        ctx.stroke();
        if (!this.compact) ctx.fillText(`influence ${inf}`, o.x + 4, o.y - rr - 4);
      }
      ctx.restore();
    }

    // axis (shoal view)
    if (L.axis.v > 0.01 && this.extent.axis) {
      const ax = this.extent.axis;
      const yA = ax.y1 + 22;
      const left = st.toScreen(-ax.half, yA), right = st.toScreen(ax.half, yA), mid = st.toScreen(0, yA);
      const top = st.toScreen(0, ax.y0 - 6);
      ctx.save();
      ctx.globalAlpha = L.axis.v;
      ctx.strokeStyle = rgba(C['border-strong']);
      ctx.lineWidth = 1;
      ctx.beginPath(); ctx.moveTo(left.x, left.y); ctx.lineTo(right.x, right.y); ctx.stroke();
      ctx.setLineDash([3, 4]);
      ctx.beginPath(); ctx.moveTo(mid.x, top.y); ctx.lineTo(mid.x, mid.y); ctx.stroke();
      ctx.setLineDash([]);
      for (const x of [left.x, mid.x, right.x]) { ctx.beginPath(); ctx.moveTo(x, left.y); ctx.lineTo(x, left.y + 4); ctx.stroke(); }
      ctx.font = `500 ${this.compact ? 10.5 : 11.5}px ${MONO}`;
      ctx.fillStyle = rgba(C['text-dim']);
      ctx.textBaseline = 'top';
      ctx.textAlign = 'center';
      ctx.fillText('−1', left.x, left.y + 7);
      ctx.fillText('0', mid.x, mid.y + 7);
      ctx.fillText('+1', right.x, right.y + 7);
      ctx.font = `600 ${this.compact ? 11 : 12.5}px ${SANS}`;
      ctx.fillStyle = rgba(C['text-2']);
      ctx.textAlign = 'left';
      ctx.fillText('Against', left.x + 14, left.y + 7);
      ctx.textAlign = 'right';
      ctx.fillText('For', right.x - 14, right.y + 7);
      if (ax.laneN) {
        const lt = st.toScreen(-ax.half, ax.laneTop);
        const lr = st.toScreen(ax.half, ax.laneTop);
        ctx.strokeStyle = rgba(C.border);
        ctx.setLineDash([2, 4]);
        ctx.beginPath(); ctx.moveTo(lt.x, lt.y - 4); ctx.lineTo(lr.x, lr.y - 4); ctx.stroke();
        ctx.setLineDash([]);
        ctx.textAlign = 'left';
        ctx.textBaseline = 'bottom';
        ctx.font = `600 ${this.compact ? 11 : 12.5}px ${SANS}`;
        ctx.fillStyle = rgba(C['text-2']);
        const label = 'No posts yet';
        ctx.fillText(label, lt.x, lt.y - 8);
        const lw = ctx.measureText(label).width;
        ctx.font = `500 ${this.compact ? 11 : 12.5}px ${SANS}`;
        ctx.fillStyle = rgba(C['text-dim']);
        ctx.fillText(this.compact ? ` · ${ax.laneN}` : ` · ${ax.laneN} without a score, not placed on the scale`, lt.x + lw, lt.y - 8);
      }
      ctx.restore();
    }

    // follows: faint hairlines (camps view)
    if (L.follows.v > 0.01 && this.follows.length) {
      ctx.save();
      // the follow hairline token at half strength, 0.8px (0.3 while something is selected)
      ctx.strokeStyle = rgba(C.follow, 0.5);
      ctx.lineWidth = 0.8;
      ctx.globalAlpha = L.follows.v * (dimOther ? 0.6 : 1);
      ctx.beginPath();
      for (const [a, b] of this.follows) {
        const A = this.byId.get(a), B = this.byId.get(b);
        const p = st.toScreen(A.x, A.y), q = st.toScreen(B.x, B.y);
        ctx.moveTo(p.x, p.y); ctx.lineTo(q.x, q.y);
      }
      ctx.stroke();
      ctx.restore();
    }

    // this round's actions (camps view): subtle directed curves
    const now = performance.now();
    if (L.regions.v > 0.01 && this.curves.length) {
      const prog = this.stage.reduced ? 1 : easeOut((now - this.curveT0) / CURVE_MS);
      for (const cv of this.curves) {
        const g = this._edgeGeom(cv);
        if (!g) continue;
        const sty = ACTION_STYLE[cv.type];
        const col = sty.color === 'accent' ? C.accent : sty.color === 'repost' ? C.repost : sty.color === 'pos' ? C.pos : C.neg;
        const involved = !dimOther || (this.selected && (cv.a === this.selected || cv.b === this.selected))
          || (campSel && this.campOf.get(cv.a)?.key === campSel);
        this._drawCurve(ctx, g, col, sty.width, sty.dash, sty.cap, sty.arrow, L.regions.v * (involved ? 0.6 : 0.12), prog);
      }
    }

    // aggregated influence edges
    if (L.rings.v > 0.01 && this.infEdges.length) {
      const hovE = this.hovered && this.hovered.kind === 'edge' ? this.hovered.e : null;
      for (const e of this.infEdges) {
        const g = this._edgeGeom(e);
        if (!g) continue;
        const width = Math.min(5, 0.9 + Math.log2(1 + e.n) * 1.1);
        const involved = !dimOther || (this.selected && (e.a === this.selected || e.b === this.selected))
          || (campSel && this.campOf.get(e.a)?.key === campSel);
        const col = e.reposts > e.replies ? C.repost : C.accent;
        this._drawCurve(ctx, g, col, width, null, 'round', true,
          L.rings.v * (e === hovE ? 0.95 : involved ? 0.34 : 0.08), 1);
      }
    }

    // nodes: dimmed ones first, the selected one last
    const order = this.nodes.slice().sort((a, b) => (a.ta - b.ta) || (a.id === this.selected) - (b.id === this.selected) || a.d - b.d);
    const hovId = this.hovered && this.hovered.kind === 'node' ? this.hovered.n.id : null;
    const ringBg = C.card;
    const labels = [];
    for (const n of order) {
      const s = st.toScreen(n.x, n.y);
      const isSel = n.id === this.selected;
      const isHov = n.id === hovId;
      const scale = isSel ? 1.12 : isHov ? 1.06 : 1;
      const size = n.d * k * scale;
      const r = size / 2;
      if (s.x < -size || s.y < -size || s.x > w + size || s.y > h + size) continue;
      const camp = this.campOf.get(n.id);
      const ringCol = L.regions.v > 0.5 ? this._campColor(camp) : ringBg;
      const ring = 2;
      ctx.save();
      ctx.globalAlpha = n.a;
      const shape = (rad) => {
        ctx.beginPath();
        if (n.org) {
          const rr = rad * 2 * (14 / 64);
          ctx.roundRect ? ctx.roundRect(s.x - rad, s.y - rad, rad * 2, rad * 2, rr) : ctx.rect(s.x - rad, s.y - rad, rad * 2, rad * 2);
        } else ctx.arc(s.x, s.y, rad, 0, Math.PI * 2);
      };
      if (isSel) {
        shape(r + ring + 3);
        ctx.fillStyle = rgba(C.accent);
        ctx.fill();
        shape(r + ring + 0.5);
        ctx.fillStyle = rgba(ringBg);
        ctx.fill();
      }
      shape(r + ring);
      ctx.fillStyle = rgba(ringCol);
      ctx.fill();
      const sp = n.sprite;
      const img = sp && sp.ready ? (n.unscored && L.axis.v > 0.5 && sp.grey ? sp.grey : sp.img) : null;
      if (img) {
        if (n.unscored && L.axis.v > 0.5 && !sp.grey) ctx.globalAlpha = n.a * 0.5;
        ctx.drawImage(img, s.x - r, s.y - r, size, size);
      } else {
        shape(r);
        ctx.fillStyle = rgba(C.hover);
        ctx.fill();
      }
      ctx.restore();
      const inf = Number(n.p.influence ?? 0);
      const show = isSel || isHov || (!dimOther || n.ta > 0.5) && (this.compact ? inf >= 0.6 : inf >= 0.4);
      if (show && n.a > 0.3) labels.push({ n, s, r, isSel });
    }
    // handles under the bigger avatars, the selected and the hovered one
    ctx.save();
    ctx.font = `500 ${this.compact ? 10.5 : 11.5}px ${SANS}`;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'top';
    ctx.lineJoin = 'round';
    ctx.lineWidth = 3;
    ctx.strokeStyle = rgba(C.ground);
    // greedy: selected and hovered first, then by influence; a label that would overlap one already
    // placed is skipped (hover still reveals it)
    labels.sort((a, b) => (b.isSel - a.isSel) || (b.n.id === hovId) - (a.n.id === hovId) || (b.n.p.influence ?? 0) - (a.n.p.influence ?? 0));
    const placed = [];
    for (const l of labels) {
      const t = `@${l.n.p.handle}`;
      const y = l.s.y + l.r + 4;
      const tw = ctx.measureText(t).width;
      const box = { x0: l.s.x - tw / 2 - 2, x1: l.s.x + tw / 2 + 2, y0: y - 1, y1: y + 14 };
      const forced = l.isSel || l.n.id === hovId;
      if (!forced && placed.some((b) => b.x0 < box.x1 && box.x0 < b.x1 && b.y0 < box.y1 && box.y0 < b.y1)) continue;
      placed.push(box);
      ctx.globalAlpha = Math.min(1, l.n.a + 0.2);
      ctx.strokeText(t, l.s.x, y);
      ctx.fillStyle = rgba(l.isSel ? C['accent-text'] : C['text-2']);
      ctx.fillText(t, l.s.x, y);
    }
    ctx.restore();
  }
}
