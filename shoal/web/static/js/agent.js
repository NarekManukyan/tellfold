// Shoal Studio: agent inspector (avatar, camp, sentiment, profile, private brief, meters, sentiment sparkline,
// memory, posts, connections) with an in-character interview chat. It lives in the Crowd tab's side panel;
// Esc or the close button closes it. Model text is only ever set via textContent.

import { sentimentColor, formatScore, sentimentSign } from "./feed.js";
import { avatarImg } from "./avatar.js";

const SVG_NS = "http://www.w3.org/2000/svg";
const ICONS = {
  close: "M18 6 6 18M6 6l12 12",
  lock: "M7 11V7a5 5 0 0 1 10 0v4M5 11h14v10H5z",
  send: "m22 2-7 20-4-9-9-4 20-7zM22 2 11 13",
  retry: "M3 12a9 9 0 1 0 3-6.7L3 8M3 3v5h5",
};

function icon(name, cls = "icon") {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("class", cls);
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(SVG_NS, "path");
  path.setAttribute("d", ICONS[name] || "");
  svg.appendChild(path);
  return svg;
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === null || v === false) continue;
    if (k === "class") node.className = v;
    else if (k === "text") node.textContent = v;
    else if (k.startsWith("on") && typeof v === "function") node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v === true ? "" : String(v));
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    node.appendChild(typeof c === "string" || typeof c === "number" ? document.createTextNode(String(c)) : c);
  }
  return node;
}

function svg(tag, attrs = {}) {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, String(v));
  return node;
}

function meanScore(list) {
  const xs = list.filter((v) => typeof v === "number" && !Number.isNaN(v));
  return xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : null;
}

export class AgentDrawer {
  /**
   * container: the docked panel element (hidden while closed).
   * options: { api, runId, onClose(), onToggle(open)?,
   *            onSelectAgent(agentId)?, onSelectPost(postId)?, getPersona(agentId)? }
   */
  constructor(container, { api, runId, onClose, onToggle, onSelectAgent, onSelectPost, getPersona, isOrg, getCamp, getSentiment } = {}) {
    this.isOrg = isOrg || (() => false);
    this.getCamp = getCamp || (() => null);
    this.getSentiment = getSentiment || (() => null);
    this.host = container;
    this.onToggle = onToggle || (() => {});
    this.api = api;
    this.runId = runId;
    this.onClose = onClose || (() => {});
    this.onSelectAgent = onSelectAgent || null;
    this.onSelectPost = onSelectPost || null;
    this.getPersona = getPersona || (() => null);
    this.agentId = null;
    this.histories = new Map(); // agentId -> [[question, answer], ...] (kept for this session)
    this.pending = new Map(); // agentId -> question awaiting a reply
    this.root = null;
    this.restoreFocus = null;
    this._seq = 0;
    this._onKey = (e) => this._keydown(e);
  }

  get isOpen() {
    return !!this.root;
  }

  /** Whether `node` sits inside the open inspector (keyboard shortcuts elsewhere leave it alone). */
  contains(node) {
    return !!(this.root && node && this.root.contains(node));
  }

  async open(agentId) {
    if (!agentId) return;
    const seq = ++this._seq;
    const first = !this.root;
    if (first) {
      this.restoreFocus = document.activeElement;
      this._mount();
    }
    this.agentId = agentId;
    this._renderLoading(agentId);
    if (first) this.head.querySelector(".icon-btn")?.focus({ preventScroll: true });
    try {
      const detail = await this.api.agent(this.runId, agentId);
      if (seq !== this._seq || !this.root) return;
      this.detail = detail;
      this._render(detail);
    } catch (err) {
      if (seq !== this._seq || !this.root) return;
      this._renderError(agentId, err);
    }
  }

  close() {
    if (!this.root) return;
    const root = this.root;
    const hadFocus = root.contains(document.activeElement);
    this.root = null;
    this.agentId = null;
    this._seq++;
    document.removeEventListener("keydown", this._onKey, true);
    root.remove();
    this.host.hidden = true;
    this.onToggle(false);
    const back = this.restoreFocus;
    this.restoreFocus = null;
    if (hadFocus && back && typeof back.focus === "function" && document.contains(back)) back.focus({ preventScroll: true });
    this.onClose();
  }

  destroy() {
    if (this.root) {
      this.root.remove();
      this.root = null;
    }
    this.host.hidden = true;
    document.removeEventListener("keydown", this._onKey, true);
  }

  // ------------------------------------------------------------------ structure

  _mount() {
    this.root = el("div", { class: "drawer" });
    this.head = el("header", { class: "drawer-head" });
    this.body = el("div", { class: "drawer-body" });
    this.chat = el("div", { class: "chat" });
    this.root.append(this.head, this.body, this.chat);
    this.host.setAttribute("aria-labelledby", "drawer-title");
    this.host.replaceChildren(this.root);
    this.host.hidden = false;
    this.onToggle(true);
    document.addEventListener("keydown", this._onKey, true);
  }

  _keydown(e) {
    if (!this.root || e.key !== "Escape" || e.defaultPrevented) return;
    // a modal dialog on top owns Esc; so does a field outside the inspector that the user is typing in
    if (document.querySelector("dialog[open]")) return;
    const t = e.target;
    if (t && !this.root.contains(t) && t.closest?.("input, textarea, select, [contenteditable]")) return;
    e.preventDefault();
    e.stopPropagation();
    this.close();
  }

  /** Camp and sentiment under the name: the two facts the crowd graph encodes. */
  _facts(p, fallbackScore = null) {
    const camp = this.getCamp(p.id);
    let score = this.getSentiment(p.id);
    if (typeof score !== "number" || Number.isNaN(score)) score = fallbackScore;
    const scored = typeof score === "number" && !Number.isNaN(score);
    const campEl = el("span", {});
    if (camp) {
      const dot = el("span", { class: "cdot", "aria-hidden": "true" });
      dot.style.background = camp.color;
      campEl.append(dot, camp.name);
    } else campEl.textContent = "No camp";
    return el("div", { class: "facts" }, campEl,
      scored ? el("span", {}, "Sentiment ", el("span", { class: "sv", "data-s": sentimentSign(score), text: formatScore(score) })) : null,
      el("span", { text: `Influence ${(Number(p.influence) || 0).toFixed(2)}` }));
  }

  _closeButton() {
    return el("button", { class: "icon-btn", type: "button", "aria-label": "Close agent inspector", title: "Close (Esc)", onclick: () => this.close() }, icon("close"));
  }

  _renderLoading(agentId) {
    const p = this.getPersona(agentId);
    const keepFocus = this.head.contains(document.activeElement);
    this.head.replaceChildren(
      avatarImg(p || { id: agentId }, { size: 44, org: this.isOrg(agentId) }),
      el("div", { class: "who" },
        el("h2", { id: "drawer-title", text: p ? p.name : "Loading agent…" }),
        el("span", { class: "handle", text: p ? `@${p.handle}` : agentId })),
      this._closeButton(),
      p ? this._facts(p) : null);
    if (keepFocus) this.head.querySelector(".icon-btn")?.focus({ preventScroll: true });
    const sk = (w, h = 12) => el("span", { class: "skel", style: `width:${w};height:${h}px` });
    this.body.replaceChildren(
      el("section", {}, sk("90%"), sk("70%"), sk("80%")),
      el("section", {}, sk("40%", 10), sk("100%", 56)),
      el("section", {}, sk("30%", 10), sk("100%"), sk("95%"), sk("60%")));
    this.chat.replaceChildren();
  }

  _renderError(agentId, err) {
    this.body.replaceChildren(
      el("div", { class: "state error" },
        el("div", { class: "state-title", text: "Could not load this agent" }),
        el("div", { class: "state-body", text: (err && err.message) || String(err) }),
        el("button", { class: "btn", type: "button", onclick: () => this.open(agentId) }, icon("retry", "icon sm"), "Retry")));
  }

  _render(d) {
    const p = d.persona;
    const posts = (d.posts || []).slice().sort((a, b) => b.id - a.id);
    const keepFocus = this.head.contains(document.activeElement);
    this.head.replaceChildren(
      avatarImg(p, { size: 44, org: this.isOrg(p.id) }),
      el("div", { class: "who" },
        el("h2", { id: "drawer-title", text: p.name }),
        el("span", { class: "handle", title: p.id, text: `@${p.handle}` + (p.entity_id ? ` · plays ${p.entity_id}` : "") })),
      this._closeButton(),
      this._facts(p, meanScore(posts.map((x) => x.sentiment))));
    if (keepFocus) this.head.querySelector(".icon-btn")?.focus({ preventScroll: true });

    const sections = [];

    if (p.bio) sections.push(el("section", {}, el("p", { text: p.bio })));

    sections.push(el("section", {},
      el("div", { class: "private" },
        el("span", { class: "lbl" }, icon("lock", "icon sm"), "Private to the simulation. Other agents never see this."),
        el("dl", {},
          el("dt", { text: "Background" }), el("dd", { text: p.background || "Not recorded" }),
          el("dt", { text: "Starting stance" }), el("dd", { text: p.stance || "Not recorded" })))));

    if (p.traits && p.traits.length) {
      sections.push(el("section", {}, el("h3", { text: "Traits" }),
        el("div", { class: "traits" }, p.traits.map((t) => el("span", { class: "tag", text: t })))));
    }

    const meter = (label, v, hint) => {
      const val = Math.max(0, Math.min(1, Number(v) || 0));
      const bar = el("div", { class: "bar", role: "meter", "aria-valuemin": "0", "aria-valuemax": "1", "aria-valuenow": val.toFixed(2), "aria-label": label },
        el("i", { style: `width:${(val * 100).toFixed(0)}%` }));
      return el("div", { class: "meter", title: hint },
        el("div", { class: "top" }, el("span", { text: label }), el("b", { text: val.toFixed(2) })), bar);
    };
    sections.push(el("section", {},
      el("div", { class: "meters" },
        meter("Activity", p.activity, "Chance of acting in a given round"),
        meter("Influence", p.influence, "Initial follower weight"))));

    sections.push(el("section", {}, el("h3", { text: "Sentiment by round" }), this._sparkline(d.sentiment_by_round || [])));

    const notes = d.memory || [];
    sections.push(el("section", {}, el("h3", { text: `Memory notes · ${notes.length}` }),
      notes.length
        ? el("ol", { class: "memory" }, notes.map((n) => el("li", { text: n })))
        : el("p", { class: "dim", text: "No memory notes yet. Agents write one after each round they take part in." })));

    sections.push(el("section", {}, el("h3", { text: `Posts · ${posts.length}` }), this._posts(posts)));

    sections.push(el("section", {},
      el("h3", { text: `Followers · ${(d.followers || []).length}` }), this._agentChips(d.followers || [], "No followers yet.")));
    sections.push(el("section", {},
      el("h3", { text: `Following · ${(d.following || []).length}` }), this._agentChips(d.following || [], "Not following anyone.")));

    this.body.replaceChildren(...sections);
    this.body.scrollTop = 0;
    this._renderChat(p);
  }

  _posts(posts) {
    if (!posts.length) return el("p", { class: "dim", text: "This agent has not posted." });
    const LIMIT = 30;
    const list = el("div", { class: "agent-posts" });
    const add = (from, to) => {
      for (const post of posts.slice(from, to)) {
        const kind = post.parent_id != null ? `reply to #${post.parent_id}` : post.repost_of != null ? `repost of #${post.repost_of}` : "post";
        const item = el(this.onSelectPost ? "button" : "div", {
          class: "agent-post", type: this.onSelectPost ? "button" : undefined,
          onclick: this.onSelectPost ? () => { this.onSelectPost(post.id); } : undefined,
          title: this.onSelectPost ? `Show #${post.id} in the feed` : undefined,
        },
          el("div", { class: "meta" },
            el("span", { text: `#${post.id}` }), el("span", { text: `r${post.round}` }), el("span", { text: kind }),
            typeof post.sentiment === "number" ? el("span", { title: "Post sentiment", text: formatScore(post.sentiment) }) : null,
            el("span", { text: `${post.likes || 0} likes / ${post.reposts || 0} reposts / ${post.replies || 0} replies` })),
          el("div", { class: "post-body", text: (post.content || "").trim() || "(no comment)" }));
        list.appendChild(item);
      }
    };
    add(0, LIMIT);
    if (posts.length <= LIMIT) return list;
    const more = el("button", { class: "btn btn-sm btn-ghost", type: "button", text: `Show ${posts.length - LIMIT} more`,
      onclick: () => { add(LIMIT, posts.length); more.remove(); } });
    return el("div", { style: "display:grid;gap:8px" }, list, more);
  }

  _agentChips(ids, emptyText) {
    if (!ids.length) return el("p", { class: "dim", text: emptyText });
    return el("div", { class: "chips" }, ids.map((id) => {
      const p = this.getPersona(id);
      const label = p ? `@${p.handle}` : id;
      return el("button", {
        class: "chip", type: "button", title: p ? `${p.name} (${id})` : id,
        onclick: () => (this.onSelectAgent ? this.onSelectAgent(id) : this.open(id)),
      }, avatarImg(p || { id }, { size: 20, org: this.isOrg(id) }), label);
    }));
  }

  _sparkline(series) {
    const pts = series
      .filter((s) => Array.isArray(s) && s.length >= 2 && typeof s[1] === "number")
      .map(([r, v]) => ({ r: Number(r), v: Math.max(-1, Math.min(1, v)) }))
      .sort((a, b) => a.r - b.r);
    if (!pts.length) return el("p", { class: "dim", text: "No scored posts yet." });
    const W = 400, H = 56, PAD = 6;
    const r0 = pts[0].r, r1 = pts[pts.length - 1].r;
    const x = (r) => (r1 === r0 ? W / 2 : PAD + ((r - r0) / (r1 - r0)) * (W - 2 * PAD));
    const y = (v) => H / 2 - v * (H / 2 - PAD);
    const s = svg("svg", { class: "spark", viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: "none", role: "img",
      "aria-label": "Sentiment by round: " + pts.map((p) => `round ${p.r} ${formatScore(p.v)}`).join(", ") });
    s.appendChild(svg("rect", { x: 0, y: 0, width: W, height: H / 2, fill: "var(--sent-pos)", opacity: 0.05 }));
    s.appendChild(svg("rect", { x: 0, y: H / 2, width: W, height: H / 2, fill: "var(--sent-neg)", opacity: 0.05 }));
    s.appendChild(svg("line", { x1: 0, x2: W, y1: H / 2, y2: H / 2, stroke: "var(--line-strong)", "stroke-dasharray": "3 3", "vector-effect": "non-scaling-stroke" }));
    if (pts.length > 1) {
      s.appendChild(svg("polyline", {
        points: pts.map((p) => `${x(p.r)},${y(p.v)}`).join(" "), fill: "none",
        stroke: "var(--text-dim)", "stroke-width": 1.5, "vector-effect": "non-scaling-stroke", "stroke-linejoin": "round",
      }));
    }
    for (const p of pts) {
      const c = svg("circle", { cx: x(p.r), cy: y(p.v), r: 3.2, fill: sentimentColor(p.v), stroke: "var(--ink-1)", "stroke-width": 1, "vector-effect": "non-scaling-stroke" });
      const t = svg("title", {});
      t.textContent = `Round ${p.r}: ${formatScore(p.v)}`;
      c.appendChild(t);
      s.appendChild(c);
    }
    const last = pts[pts.length - 1];
    return el("div", { class: "spark-wrap" }, s,
      el("div", { class: "spark-legend" },
        el("span", { text: `r${r0}` }),
        el("span", { text: `latest ${formatScore(last.v)}` }),
        el("span", { text: `r${r1}` })));
  }

  // ------------------------------------------------------------------ chat

  /** The chat currently on screen, if it belongs to `agentId`. */
  _liveChat(agentId) {
    const v = this._chatView;
    return v && v.agentId === agentId && this.agentId === agentId && this.root && v.log.isConnected ? v : null;
  }

  _renderChat(persona) {
    const agentId = persona.id;
    const log = el("div", { class: "chat-log", role: "log", "aria-live": "polite", "aria-label": `Interview with @${persona.handle}` });
    const input = el("textarea", { class: "textarea", rows: "1", placeholder: `Ask @${persona.handle} something…`, "aria-label": `Message to @${persona.handle}` });
    const send = el("button", { class: "btn btn-primary", type: "submit", "aria-label": "Send" }, icon("send", "icon sm"));
    const form = el("form", { class: "chat-form" }, input, send);
    this.chat.replaceChildren(log, form);

    const render = () => {
      const hist = this.histories.get(agentId) || [];
      log.replaceChildren(el("p", { class: "chat-intro", text:
        `Interview @${persona.handle}. Replies are written in character from their persona, memory and posts.` }));
      for (const [q, a] of hist) {
        log.append(el("div", { class: "msg me", text: q }), el("div", { class: "msg them", text: a }));
      }
      const waiting = this.pending.get(agentId);
      if (waiting) {
        log.append(el("div", { class: "msg me", text: waiting }),
          el("div", { class: "msg them typing", "aria-label": `@${persona.handle} is typing` }, el("i"), el("i"), el("i")));
      }
      log.scrollTop = log.scrollHeight;
      input.disabled = !!waiting;
      send.disabled = !!waiting;
    };

    const grow = () => {
      input.style.height = "auto";
      input.style.height = Math.min(120, input.scrollHeight + 2) + "px";
    };

    const submit = async (message) => {
      const text = (message ?? input.value).trim();
      if (!text || this.pending.get(agentId)) return;
      input.value = "";
      grow();
      const history = (this.histories.get(agentId) || []).map(([q, a]) => [q, a]);
      this.pending.set(agentId, text);
      render();
      try {
        const res = await this.api.chat(this.runId, agentId, text, history);
        const reply = (res && typeof res.reply === "string") ? res.reply : "";
        this.histories.set(agentId, [...(this.histories.get(agentId) || []), [text, reply || "(no reply)"]]);
        this.pending.delete(agentId);
        // The drawer may have been re-rendered while waiting: update the chat that is on screen now.
        const view = this._liveChat(agentId);
        if (view) {
          view.render();
          view.input.focus({ preventScroll: true });
        }
      } catch (err) {
        this.pending.delete(agentId);
        const view = this._liveChat(agentId);
        if (!view) return;
        view.render();
        view.input.value = text;
        view.log.append(el("div", { class: "msg err" },
          `Could not get a reply: ${(err && err.message) || err}. `,
          el("button", { class: "linkish", type: "button", text: "Try again", onclick: () => view.submit(text) })));
        view.log.scrollTop = view.log.scrollHeight;
      }
    };
    this._chatView = { agentId, log, input, render, submit };

    form.addEventListener("submit", (e) => {
      e.preventDefault();
      submit();
    });
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        submit();
      }
    });
    input.addEventListener("input", grow);
    render();
  }
}
