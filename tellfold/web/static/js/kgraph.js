// Tellfold Studio: knowledge graph canvas.
//
// Entities told apart by shape (with a legend), directed relations with labels on hover, entities that a
// persona embodies get a second, outer rim. The force layout is shaped to the panel and fitted to it,
// with a collision radius that leaves room for names; isolated entities are held near the centre
// instead of drifting to the corners. Once settled the canvas idles (no ambient motion).

import { ForceSim, Stage, rgba, parseColor, hash01, h } from './network.js';

const KINDS = ['person', 'organization', 'group', 'product', 'place', 'concept', 'event'];

// Colour in Studio only means state or action type, so entity kinds are told apart by shape, with the
// kind written in the legend: people are circles, organisations and groups squares, everything else
// (products, places, concepts, events) a diamond. Fills are the neutral node colour.
const SHAPE = {
  person: 'circle', organization: 'square', group: 'square', product: 'diamond',
  place: 'diamond', concept: 'diamond', event: 'diamond', other: 'diamond',
};
const SVG_NS = 'http://www.w3.org/2000/svg';

function glyph(shape, stroke, fill) {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('width', '10'); svg.setAttribute('height', '10'); svg.setAttribute('viewBox', '0 0 10 10');
  svg.setAttribute('aria-hidden', 'true');
  const el = document.createElementNS(SVG_NS, shape === 'circle' ? 'circle' : shape === 'square' ? 'rect' : 'path');
  if (shape === 'circle') { el.setAttribute('cx', '5'); el.setAttribute('cy', '5'); el.setAttribute('r', '4'); }
  else if (shape === 'square') { el.setAttribute('x', '1.5'); el.setAttribute('y', '1.5'); el.setAttribute('width', '7'); el.setAttribute('height', '7'); }
  else el.setAttribute('d', 'M5 0.8 9.2 5 5 9.2 0.8 5Z');
  el.setAttribute('fill', fill); el.setAttribute('stroke', stroke);
  svg.appendChild(el);
  return svg;
}

const MONO = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace';
const SANS = 'system-ui, -apple-system, "Segoe UI", Roboto, sans-serif';

function kindOf(e) {
  const k = String(e.kind || '').toLowerCase().trim();
  return KINDS.includes(k) ? k : 'other';
}

export class KnowledgeGraphView {
  /**
   * @param {HTMLCanvasElement} canvas
   * @param {{onSelectEntity?: (entityId: string) => void}} opts
   */
  constructor(canvas, { onSelectEntity } = {}) {
    this.onSelectEntity = onSelectEntity || (() => {});
    this.nodes = [];
    this.byId = new Map();
    this.edges = [];
    this.neighbours = new Map();
    this.selected = null;
    this.hovered = null; // node
    this.hoverEdge = null;
    this.kindFocus = null; // legend: emphasise one kind
    this.kindColors = {};
    this.sim = new ForceSim({ charge: -70, linkDistance: 58, linkStrength: 0.55, center: 0.035, centerFloor: 0, distanceMax: 320, collide: 16, collideStrength: 0.85, alphaDecay: 0.03, aspect: 1.6 });
    // fit the layout to the panel; names sit under nodes and can run wider than them
    this.fitPad = { x: 72, top: 16, bottom: 76 }; // the bottom keeps clear of the kind legend
    this.maxFitZoom = 2.4;
    this.stage = new Stage(canvas, this, { label: 'Knowledge graph' });
    this.legend = document.createElement('div');
    this.legend.className = 'sv-legend';
    this.legend.setAttribute('role', 'group');
    this.legend.setAttribute('aria-label', 'Entity kinds');
    this.stage.overlay.appendChild(this.legend);
    this._readKindColors();
  }

  /** graph: KnowledgeGraph {entities, relations}; personas: Persona[] (to badge embodied entities). */
  setData(graph, personas = []) {
    const ents = graph?.entities || [];
    const rels = graph?.relations || [];
    const embodied = new Map();
    for (const p of personas || []) {
      if (!p.entity_id) continue;
      if (!embodied.has(p.entity_id)) embodied.set(p.entity_id, []);
      embodied.get(p.entity_id).push(p);
    }
    const prev = this.byId;
    this.byId = new Map();
    const n0 = ents.length;
    const spread = 22 * Math.sqrt(Math.max(1, n0));
    this.nodes = ents.map((e, i) => {
      const old = prev.get(e.id);
      const ang = i * 2.39996323 + hash01(e.id) * 0.5;
      const rad = spread * Math.sqrt((i + 0.5) / Math.max(1, n0));
      const n = old || { id: e.id, x: Math.cos(ang) * rad, y: Math.sin(ang) * rad, vx: 0, vy: 0 };
      n.e = e;
      n.kind = kindOf(e);
      n.personas = embodied.get(e.id) || [];
      n.deg = 0;
      this.byId.set(e.id, n);
      return n;
    });

    // merge parallel relations; mark reverse pairs so they curve apart
    const map = new Map();
    for (const r of rels) {
      const s = this.byId.get(r.source), t = this.byId.get(r.target);
      if (!s || !t || s === t) continue;
      const key = s.id + '>' + t.id;
      let ed = map.get(key);
      if (!ed) { ed = { s, t, key, labels: [], evidence: [] }; map.set(key, ed); }
      if (r.label && !ed.labels.includes(r.label)) ed.labels.push(r.label);
      if (r.evidence) ed.evidence.push(r.evidence);
    }
    this.edges = [...map.values()];
    for (const ed of this.edges) {
      ed.curve = map.has(ed.t.id + '>' + ed.s.id) ? 0.16 : 0.06;
      ed.s.deg++; ed.t.deg++;
    }
    this.neighbours = new Map(this.nodes.map((n) => [n.id, new Set()]));
    for (const ed of this.edges) {
      this.neighbours.get(ed.s.id).add(ed.t.id);
      this.neighbours.get(ed.t.id).add(ed.s.id);
    }
    for (const n of this.nodes) {
      n.r = 5 + Math.min(10, Math.sqrt(n.deg) * 2.4);
      n.charge = -50 - n.r * 5;
      n.gravity = n.deg === 0 ? 5 : 1; // nothing links an isolated entity in, so hold it near the centre
    }
    if (this.selected && !this.byId.has(this.selected)) this.selected = null;
    this.sim.setNodes(this.nodes);
    this.sim.setLinks(this.edges.map((ed) => ({ source: ed.s, target: ed.t })));
    this.sim.alpha = 1;
    const warm = prev.size ? 120 : 300;
    for (let i = 0; i < warm; i++) this.sim.tick();
    for (const n of this.nodes) { n.vx = 0; n.vy = 0; }
    this.sim.alpha = this.stage.reduced ? 0 : 0.08; // a short visible settle, then idle
    this.stage.auto = true;
    this.stage._fitNow();
    this._buildLegend();
    if (this.stage.ownLabel) {
      this.stage.canvas.setAttribute('aria-label',
        `Knowledge graph: ${this.nodes.length} entities, ${this.edges.length} relations.`);
    }
    this.stage.request();
  }

  resize() { this.stage.resize(); }

  /** Shape the layout to the panel, as the network does. */
  onResize(w, h) {
    if (w <= 1 || h <= 1) return;
    const aspect = Math.max(0.6, Math.min(3, (w - 2 * this.fitPad.x) / Math.max(1, h - this.fitPad.top - this.fitPad.bottom)));
    if (Math.abs(aspect - this.sim.aspect) < 0.04) return;
    this.sim.aspect = aspect;
    if (!this.nodes.length) return;
    this.sim.reheat(0.25);
    if (this.stage.reduced) { for (let i = 0; i < 300 && !this.sim.settled; i++) this.sim.tick(); this.sim.alpha = 0; }
    this.stage.request();
  }

  destroy() { this.stage.destroy(); this.nodes = []; this.edges = []; }

  // ------------------------------------------------ legend & colours

  _readKindColors() {
    const pal = this.stage.pal;
    const node = parseColor(pal.extra('node'), pal.c['ink-2']);
    this.kindColors = {};
    for (const k of [...KINDS, 'other']) this.kindColors[k] = node;
  }

  _buildLegend() {
    const counts = new Map();
    for (const n of this.nodes) counts.set(n.kind, (counts.get(n.kind) || 0) + 1);
    const kinds = [...KINDS, 'other'].filter((k) => counts.has(k));
    const embodied = this.nodes.filter((n) => n.personas.length).length;
    this.legend.replaceChildren();
    const rim = rgba(this.stage.pal.c['text-dim']);
    const fill = rgba(this.kindColors.other);
    if (!this.nodes.length) return;
    for (const k of kinds) {
      const b = document.createElement('button');
      b.type = 'button';
      b.dataset.kind = k;
      b.setAttribute('aria-pressed', String(this.kindFocus === k));
      b.title = `Highlight ${k} entities`;
      b.append(glyph(SHAPE[k], rim, fill), document.createTextNode(k + ' '), h('span', 'sv-n', counts.get(k)));
      b.addEventListener('click', () => { this.kindFocus = this.kindFocus === k ? null : k; this._syncLegend(); this.stage.request(); });
      b.addEventListener('mouseenter', () => { this.legendHover = k; this.stage.request(); });
      b.addEventListener('mouseleave', () => { this.legendHover = null; this.stage.request(); });
      this.legend.appendChild(b);
    }
    if (embodied) {
      const b = document.createElement('button');
      b.type = 'button';
      b.dataset.kind = '@';
      b.setAttribute('aria-pressed', String(this.kindFocus === '@'));
      b.title = 'Highlight entities an agent embodies';
      b.append(embodiedGlyph(rim, fill), document.createTextNode('played by an agent '), h('span', 'sv-n', embodied));
      b.addEventListener('click', () => { this.kindFocus = this.kindFocus === '@' ? null : '@'; this._syncLegend(); this.stage.request(); });
      b.addEventListener('mouseenter', () => { this.legendHover = '@'; this.stage.request(); });
      b.addEventListener('mouseleave', () => { this.legendHover = null; this.stage.request(); });
      this.legend.appendChild(b);
    }
  }

  _syncLegend() {
    for (const b of this.legend.querySelectorAll('button')) b.setAttribute('aria-pressed', String(b.dataset.kind === this.kindFocus));
  }

  // ------------------------------------------------ Stage callbacks

  onTheme() { this._readKindColors(); this._buildLegend(); }

  onMotion() { if (this.stage.reduced) this.sim.alpha = 0; }

  bounds() {
    if (!this.nodes.length) return null;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of this.nodes) {
      const r = n.r + 3; // the outer rim of an embodied entity; name room is in fitPad (screen space)
      if (n.x - r < x0) x0 = n.x - r; if (n.x + r > x1) x1 = n.x + r;
      if (n.y - r < y0) y0 = n.y - r; if (n.y + r > y1) y1 = n.y + r;
    }
    return { x0, y0, x1, y1 };
  }

  hit(sx, sy) {
    const st = this.stage, k = st.cam.k;
    let best = null, bd = Infinity;
    for (const n of this.nodes) {
      const s = st.toScreen(n.x, n.y);
      const d = Math.hypot(s.x - sx, s.y - sy);
      if (d < Math.max(7, n.r * k + 4) && d < bd) { bd = d; best = n; }
    }
    if (best) return best;
    // relation hit-test against the sampled curve
    let be = null, bed = 6;
    for (const ed of this.edges) {
      const pts = this._curve(ed, 10);
      for (let i = 1; i < pts.length; i++) {
        const d = segDist(sx, sy, pts[i - 1], pts[i]);
        if (d < bed) { bed = d; be = ed; }
      }
    }
    return be ? { edge: be } : null;
  }

  onHover(item, sx, sy) {
    const node = item && !item.edge ? item : null;
    const edge = item && item.edge ? item.edge : null;
    if (node !== this.hovered || edge !== this.hoverEdge) {
      this.hovered = node;
      this.hoverEdge = edge;
      this.stage.request();
    }
    if (node) {
      const e = node.e;
      const sum = e.summary ? (e.summary.length > 220 ? e.summary.slice(0, 217) + '…' : e.summary) : '';
      const tip = h('div', '', null,
        h('div', 'sv-row', null, h('strong', '', e.name), h('span', 'sv-mono sv-dim sv-small', e.kind || node.kind)),
        sum ? h('div', 'sv-dim', sum) : null,
        h('div', 'sv-dim sv-mono sv-small', `${node.deg} relation${node.deg === 1 ? '' : 's'}`),
        node.personas.length ? h('div', 'sv-tag', 'embodied by ' + node.personas.map((p) => '@' + p.handle).join(', ')) : null);
      this.stage.showTip(tip, sx, sy);
    } else if (edge) {
      const ev = edge.evidence[0] ? (edge.evidence[0].length > 200 ? edge.evidence[0].slice(0, 197) + '…' : edge.evidence[0]) : '';
      const tip = h('div', '', null,
        h('div', '', null, h('strong', '', edge.s.e.name), h('span', 'sv-dim', ' -> '), h('strong', '', edge.t.e.name)),
        h('div', 'sv-mono sv-small', edge.labels.join('; ') || 'related to'),
        ev ? h('div', 'sv-dim', '"' + ev + '"') : null);
      this.stage.showTip(tip, sx, sy);
    } else {
      this.stage.hideTip();
    }
  }

  onClick(item) {
    if (item && !item.edge) {
      this.selected = item.id;
      this.onSelectEntity(item.id);
    } else if (!item) {
      this.selected = null;
    }
    this.stage.request();
  }

  onDrag(item, wx, wy) {
    if (item.edge) return;
    item.fx = wx; item.fy = wy; item.x = wx; item.y = wy;
    if (!this.stage.reduced) this.sim.reheat(0.12);
  }

  onDragEnd(item) {
    if (item.edge) return;
    item.fx = null; item.fy = null;
  }

  onKey(e) {
    if (e.key === 'Escape' && (this.selected || this.kindFocus)) {
      this.selected = null; this.kindFocus = null; this._syncLegend(); this.stage.request();
      return true;
    }
    return false;
  }

  step() {
    if (!this.nodes.length) return false;
    if (this.sim.settled) return false;
    this.sim.tick();
    return !this.sim.settled;
  }

  // ------------------------------------------------ rendering

  _curve(ed, segs) {
    const st = this.stage;
    const a = st.toScreen(ed.s.x, ed.s.y), b = st.toScreen(ed.t.x, ed.t.y);
    const dx = b.x - a.x, dy = b.y - a.y;
    const c = { x: (a.x + b.x) / 2 - dy * ed.curve, y: (a.y + b.y) / 2 + dx * ed.curve };
    const pts = [];
    for (let i = 0; i <= segs; i++) {
      const t = i / segs, u = 1 - t;
      pts.push({ x: u * u * a.x + 2 * u * t * c.x + t * t * b.x, y: u * u * a.y + 2 * u * t * c.y + t * t * b.y });
    }
    return pts;
  }

  render(ctx, w, h) {
    const st = this.stage, pal = st.pal, C = pal.c, k = st.cam.k;
    if (!this.nodes.length) return;
    for (const n of this.nodes) {
      const s = st.toScreen(n.x, n.y);
      n.sx = s.x; n.sy = s.y; n.sr = Math.max(4, Math.min(22, n.r * k));
    }
    const sel = this.selected ? this.byId.get(this.selected) : null;
    const focus = sel || this.hovered;
    let related = null;
    if (focus) related = new Set([focus.id, ...(this.neighbours.get(focus.id) || [])]);
    const kf = this.legendHover || this.kindFocus;
    const nodeOn = (n) => {
      if (related && !related.has(n.id)) return false;
      if (kf === '@') return n.personas.length > 0;
      if (kf) return n.kind === kf;
      return true;
    };
    const edgeOn = (ed) => {
      if (this.hoverEdge) return ed === this.hoverEdge;
      if (focus) return ed.s === focus || ed.t === focus;
      if (kf) return nodeOn(ed.s) || nodeOn(ed.t);
      return true;
    };

    // relations
    const labelled = [];
    for (const ed of this.edges) {
      const on = edgeOn(ed);
      const strong = (focus && on) || ed === this.hoverEdge;
      const pts = this._curve(ed, 14);
      // trim ends to node rims
      const trim = (pts, r, fromEnd) => {
        while (pts.length > 2) {
          const p = fromEnd ? pts[pts.length - 1] : pts[0];
          const n = fromEnd ? ed.t : ed.s;
          if (Math.hypot(p.x - n.sx, p.y - n.sy) < r) { if (fromEnd) pts.pop(); else pts.shift(); } else break;
        }
      };
      trim(pts, ed.s.sr + 1, false);
      trim(pts, ed.t.sr + 3, true);
      if (pts.length < 2) continue;
      ctx.beginPath();
      pts.forEach((p, i) => (i ? ctx.lineTo(p.x, p.y) : ctx.moveTo(p.x, p.y)));
      const col = strong ? C.text : C.line;
      const alpha = strong ? 0.85 : on ? (pal.dark ? 0.9 : 1) : 0.25;
      ctx.strokeStyle = rgba(col, alpha);
      ctx.lineWidth = strong ? 1.4 : 1;
      ctx.stroke();
      // arrowhead
      const e1 = pts[pts.length - 1], e0 = pts[pts.length - 2];
      const ux = e1.x - e0.x, uy = e1.y - e0.y, ul = Math.hypot(ux, uy) || 1;
      const tx = ux / ul, ty = uy / ul, sz = strong ? 6 : 4.5;
      ctx.beginPath();
      ctx.moveTo(e1.x, e1.y);
      ctx.lineTo(e1.x - tx * sz - ty * sz * 0.5, e1.y - ty * sz + tx * sz * 0.5);
      ctx.lineTo(e1.x - tx * sz + ty * sz * 0.5, e1.y - ty * sz - tx * sz * 0.5);
      ctx.closePath();
      ctx.fillStyle = rgba(col, alpha);
      ctx.fill();
      if (strong) labelled.push({ ed, mid: pts[Math.floor(pts.length / 2)] });
    }

    // nodes
    for (const n of this.nodes) {
      const on = nodeOn(n);
      const col = this.kindColors[n.kind];
      ctx.globalAlpha = on ? 1 : 0.18;
      shapePath(ctx, SHAPE[n.kind], n.sx, n.sy, n.sr);
      ctx.fillStyle = rgba(col);
      ctx.fill();
      ctx.lineWidth = 1;
      ctx.strokeStyle = rgba(C['text-dim'], 0.8);
      ctx.stroke();
      if (n.personas.length) {
        // played by an agent: a second rim, drawn, not a text badge, so it never collides with names
        shapePath(ctx, SHAPE[n.kind], n.sx, n.sy, n.sr + 3);
        ctx.lineWidth = 1;
        ctx.strokeStyle = rgba(C.text, 0.85);
        ctx.stroke();
      }
    }
    ctx.globalAlpha = 1;

    if (sel) {
      shapePath(ctx, SHAPE[sel.kind], sel.sx, sel.sy, sel.sr + (sel.personas.length ? 6.5 : 4.5));
      ctx.strokeStyle = rgba(C.accent);
      ctx.lineWidth = 1.5;
      ctx.stroke();
    }

    // entity names
    const placed = [];
    const order = [...this.nodes].sort((a, b) => (b === focus) - (a === focus) || b.deg - a.deg);
    const showAll = k > 1.3 || this.nodes.length <= 24;
    // names try below, above, right and left of their entity and are dropped (unless focused) when
    // every spot would cover another entity or name
    ctx.textBaseline = 'top';
    ctx.textAlign = 'left';
    const ext = (n) => n.sr * (SHAPE[n.kind] === 'diamond' ? 1.15 : 1) + (n.personas.length ? 3 : 0);
    const bodies = this.nodes.map((n) => { const e = ext(n) + 1.5; return { n, x0: n.sx - e, y0: n.sy - e, x1: n.sx + e, y1: n.sy + e }; });
    const hits = (a, b) => a.x0 < b.x1 && a.x1 > b.x0 && a.y0 < b.y1 && a.y1 > b.y0;
    for (const n of order) {
      const strong = n === focus || (related && related.has(n.id));
      if (!showAll && !strong && n.deg < 3) continue;
      if (!nodeOn(n) && !strong) continue;
      ctx.font = `${strong ? 600 : 500} 11px ${SANS}`;
      const text = n.e.name.length > 28 ? n.e.name.slice(0, 27) + '…' : n.e.name;
      const tw = ctx.measureText(text).width;
      const e = ext(n), LH = 14, G = 4;
      const spots = [
        { x: n.sx - tw / 2, y: n.sy + e + G },
        { x: n.sx - tw / 2, y: n.sy - e - G - LH },
        { x: n.sx + e + G + 1, y: n.sy - LH / 2 },
        { x: n.sx - e - G - 1 - tw, y: n.sy - LH / 2 },
      ];
      let chosen = null;
      for (const sp of spots) {
        const box = { x0: sp.x - 3, y0: sp.y - 1, x1: sp.x + tw + 3, y1: sp.y + LH };
        if (box.x0 < 2 || box.y0 < 2 || box.x1 > w - 2 || box.y1 > h - 2) continue;
        if (placed.some((b) => hits(box, b)) || bodies.some((b) => b.n !== n && hits(box, b))) continue;
        chosen = { ...sp, box };
        break;
      }
      if (!chosen) {
        if (n !== focus) continue;
        const sp = spots[0];
        chosen = { ...sp, box: { x0: sp.x - 3, y0: sp.y - 1, x1: sp.x + tw + 3, y1: sp.y + LH } };
      }
      placed.push(chosen.box);
      ctx.strokeStyle = rgba(C['ink-1']);
      ctx.lineWidth = 3;
      ctx.lineJoin = 'round';
      ctx.strokeText(text, chosen.x, chosen.y);
      ctx.fillStyle = rgba(strong ? C.text : C['text-dim']);
      ctx.fillText(text, chosen.x, chosen.y);
    }
    ctx.textAlign = 'center';

    // relation labels (hovered edge, or all relations of the focused entity)
    ctx.font = `500 10.5px ${MONO}`;
    ctx.textBaseline = 'middle';
    for (const { ed, mid } of labelled) {
      const text = (ed.labels.join('; ') || 'related to');
      const t = text.length > 40 ? text.slice(0, 39) + '…' : text;
      const tw = ctx.measureText(t).width;
      const box = { x0: mid.x - tw / 2 - 5, y0: mid.y - 8, x1: mid.x + tw / 2 + 5, y1: mid.y + 8 };
      if (ed !== this.hoverEdge && placed.some((b) => !(box.x1 < b.x0 || box.x0 > b.x1 || box.y1 < b.y0 || box.y0 > b.y1))) continue;
      placed.push(box);
      ctx.fillStyle = rgba(C['ink-1'], 0.95);
      roundRect(ctx, box.x0, box.y0, box.x1 - box.x0, box.y1 - box.y0, 4);
      ctx.fill();
      ctx.strokeStyle = rgba(C.line);
      ctx.lineWidth = 1;
      ctx.stroke();
      ctx.fillStyle = rgba(C.text);
      ctx.fillText(t, mid.x, mid.y + 0.5);
    }
    ctx.textAlign = 'start';
  }
}

/** Legend glyph for "played by an agent": a node with the second rim drawn on the canvas. */
function embodiedGlyph(stroke, fill) {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('width', '12'); svg.setAttribute('height', '12'); svg.setAttribute('viewBox', '0 0 12 12');
  svg.setAttribute('aria-hidden', 'true');
  const inner = document.createElementNS(SVG_NS, 'circle');
  inner.setAttribute('cx', '6'); inner.setAttribute('cy', '6'); inner.setAttribute('r', '3');
  inner.setAttribute('fill', fill); inner.setAttribute('stroke', stroke);
  const outer = document.createElementNS(SVG_NS, 'circle');
  outer.setAttribute('cx', '6'); outer.setAttribute('cy', '6'); outer.setAttribute('r', '5.3');
  outer.setAttribute('fill', 'none'); outer.setAttribute('stroke', 'currentColor');
  svg.append(inner, outer);
  return svg;
}

function shapePath(ctx, shape, x, y, r) {
  ctx.beginPath();
  if (shape === 'square') { const q = r * 0.9; ctx.rect(x - q, y - q, q * 2, q * 2); }
  else if (shape === 'diamond') { const q = r * 1.15; ctx.moveTo(x, y - q); ctx.lineTo(x + q, y); ctx.lineTo(x, y + q); ctx.lineTo(x - q, y); ctx.closePath(); }
  else ctx.arc(x, y, r, 0, Math.PI * 2);
}

function segDist(px, py, a, b) {
  const dx = b.x - a.x, dy = b.y - a.y;
  const l2 = dx * dx + dy * dy || 1;
  let t = ((px - a.x) * dx + (py - a.y) * dy) / l2;
  t = Math.max(0, Math.min(1, t));
  return Math.hypot(px - a.x - t * dx, py - a.y - t * dy);
}

function roundRect(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}
