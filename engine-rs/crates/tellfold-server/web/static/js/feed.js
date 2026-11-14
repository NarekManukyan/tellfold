// Tellfold Studio: threaded feed for one round (crowd panel), with avatars. All model text goes in through textContent.

import { avatarImg } from "./avatar.js";

const SVG_NS = "http://www.w3.org/2000/svg";

const ICONS = {
  reply: "M9 14 4 9l5-5M4 9h10.5a5.5 5.5 0 0 1 0 11H11",
  repost: "m17 2 4 4-4 4M3 11V9a3 3 0 0 1 3-3h15M7 22l-4-4 4-4M21 13v2a3 3 0 0 1-3 3H3",
  like: "M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z",
  dislike: "M17 14V2M9 18.12 10 14H4.17a2 2 0 0 1-1.92-2.56l2.33-8A2 2 0 0 1 6.5 2H20a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-2.76a2 2 0 0 0-1.79 1.11L12 22a3.13 3.13 0 0 1-3-3.88Z",
  event: "M13 2 3 14h9l-1 8 10-12h-9l1-8z",
  corner: "M15 10l5 5-5 5M4 4v7a4 4 0 0 0 4 4h12",
};

function icon(name, cls = "icon sm") {
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
    else if (k === "style") node.style.cssText = v;
    else if (k.startsWith("on") && typeof v === "function") node.addEventListener(k.slice(2), v);
    else if (k === "dataset") Object.assign(node.dataset, v);
    else node.setAttribute(k, v === true ? "" : String(v));
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    node.appendChild(typeof c === "string" || typeof c === "number" ? document.createTextNode(String(c)) : c);
  }
  return node;
}

/** CSS colour for a sentiment score in -1..1 on the coral / grey / teal scale. */
export function sentimentColor(score) {
  if (score === null || score === undefined || Number.isNaN(score)) return "var(--sent-neu)";
  const s = Math.max(-1, Math.min(1, Number(score)));
  const pct = Math.round(Math.abs(s) * 100);
  const end = s < 0 ? "var(--sent-neg)" : "var(--sent-pos)";
  return `color-mix(in oklab, ${end} ${pct}%, var(--sent-neu))`;
}

export function formatScore(score) {
  if (score === null || score === undefined || Number.isNaN(score)) return "n/a";
  const s = Math.round(Number(score) * 100) / 100; // same rounding as the canvas labels
  return (s > 0 ? "+" : s < 0 ? "−" : "±") + Math.abs(s).toFixed(2);
}

/** "neg" / "pos" / "neu" for a sentiment score, used as a data attribute so CSS can colour the figure. */
export function sentimentSign(score) {
  if (score === null || score === undefined || Number.isNaN(score)) return "neu";
  return score < -0.15 ? "neg" : score > 0.15 ? "pos" : "neu"; // same bands as the Mac app
}

// Node size is a fixed five-step influence ramp (not a continuous scale), so a node's size always
// reads against the legend. Radii are in CSS pixels at zoom 1.
export const INFLUENCE_RAMP = [
  { max: 0.2, r: 3.5, label: "<0.2" },
  { max: 0.4, r: 5, label: "0.2" },
  { max: 0.6, r: 6.5, label: "0.4" },
  { max: 0.8, r: 8.5, label: "0.6" },
  { max: Infinity, r: 11, label: "0.8+" },
];

export function influenceStep(influence) {
  const v = Math.max(0, Math.min(1, Number(influence ?? 0.5)));
  for (let i = 0; i < INFLUENCE_RAMP.length; i++) if (v < INFLUENCE_RAMP[i].max) return i;
  return INFLUENCE_RAMP.length - 1;
}

function snippet(text, n = 90) {
  const t = String(text || "").replace(/\s+/g, " ").trim();
  return t.length > n ? t.slice(0, n - 1) + "…" : t;
}

const reducedMotion = () => window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

const MAX_DEPTH = 4;

export class Feed {
  constructor(container, { onSelectAgent, onSelectPost, isOrg } = {}) {
    this.el = container;
    this.isOrg = isOrg || (() => false);
    this.onSelectAgent = onSelectAgent || (() => {});
    this.onSelectPost = onSelectPost || (() => {});
    this.posts = new Map(); // id -> PostOut
    this.personas = new Map(); // id -> Persona
    this.events = [];
    this.round = null;
    this.newIds = new Set();
    this.placeholder = "Posts appear here once agents start talking.";
    this.scored = true; // false when no post in the run has a sentiment score
    this.el.classList.add("feed");
    this.el.setAttribute("role", "feed");
    this.el.setAttribute("aria-busy", "false");
    this._render();
  }

  setData({ posts = [], personas = [], events = [] } = {}) {
    this.posts = new Map(posts.map((p) => [p.id, p]));
    this.personas = new Map(personas.map((p) => [p.id, p]));
    this.events = events.slice();
    this._render();
  }

  /** Text shown when there are no posts at all (for example while personas are still being created). */
  setPlaceholder(text) {
    this.placeholder = text;
    if (this.posts.size === 0) this._render();
  }

  setEvents(events = []) {
    this.events = events.slice();
    this._render();
  }

  showRound(round) {
    if (round === this.round) return;
    this.round = round;
    this.newIds.clear();
    this._render();
    this.el.scrollTop = 0;
  }

  appendPosts(posts = []) {
    let touchesView = false;
    for (const p of posts) {
      if (this.posts.has(p.id)) {
        this.posts.set(p.id, p);
        continue;
      }
      this.posts.set(p.id, p);
      this.newIds.add(p.id);
      if (p.round === this.round) touchesView = true;
    }
    if (touchesView) {
      const top = this.el.scrollTop;
      this._render();
      this.el.scrollTop = top;
    }
    this.newIds.clear();
  }

  highlightPost(postId) {
    const node = this.el.querySelector(`[data-post-id="${Number(postId)}"]`);
    if (!node) return false;
    node.scrollIntoView({ block: "center", behavior: reducedMotion() ? "auto" : "smooth" });
    node.classList.remove("is-hl");
    void node.offsetWidth;
    node.classList.add("is-hl");
    clearTimeout(this._hlTimer);
    this._hlTimer = setTimeout(() => node.classList.remove("is-hl"), 2600);
    return true;
  }

  // ------------------------------------------------------------------ rendering

  _handle(agentId, fallback) {
    const p = this.personas.get(agentId);
    return (p && p.handle) || fallback || agentId;
  }

  _render() {
    const root = this.el;
    root.replaceChildren();
    this.scored = [...this.posts.values()].some((p) => typeof p.sentiment === "number" && !Number.isNaN(p.sentiment));

    if (this.posts.size === 0 && this.round === null) {
      root.appendChild(this._state("No posts yet", this.placeholder));
      return;
    }
    const r = this.round ?? 0;

    for (const ev of this.events.filter((e) => e.round === r)) {
      root.appendChild(
        el("div", { class: "feed-event", role: "note" },
          icon("event", "icon"),
          el("span", { class: "lbl", text: `Breaking news in round ${r}` }),
          el("span", { text: ev.content || "" }))
      );
    }

    const inRound = [...this.posts.values()].filter((p) => p.round === r).sort((a, b) => a.id - b.id);
    if (inRound.length === 0) {
      const title = this.posts.size === 0 ? "No posts yet" : `Quiet round`;
      const body = this.posts.size === 0 ? this.placeholder : `Nobody posted in round ${r}. Likes, follows and other actions still show on the network.`;
      root.appendChild(this._state(title, body));
      return;
    }

    const ids = new Set(inRound.map((p) => p.id));
    const children = new Map();
    const roots = [];
    for (const p of inRound) {
      if (p.parent_id !== null && p.parent_id !== undefined && ids.has(p.parent_id)) {
        if (!children.has(p.parent_id)) children.set(p.parent_id, []);
        children.get(p.parent_id).push(p);
      } else {
        roots.push(p);
      }
    }

    const frag = document.createDocumentFragment();
    for (const p of roots) {
      frag.appendChild(el("div", { class: "thread" }, this._post(p, children, 0, true)));
    }
    // said once here rather than "n/a" on every post
    if (!this.scored) frag.appendChild(el("p", { class: "panel-note", text: "This run has no per-post sentiment scores." }));
    root.appendChild(frag);
  }

  _state(title, body) {
    return el("div", { class: "state" },
      el("div", { class: "state-title", text: title }),
      el("div", { class: "state-body", text: body }));
  }

  _avatar(agentId, handle, size) {
    const p = this.personas.get(agentId) || { id: agentId, handle };
    return el("button", {
      class: "av-btn", type: "button", "aria-label": `Open @${handle}`, tabindex: "-1",
      onclick: (e) => { e.stopPropagation(); this.onSelectAgent(agentId); },
    }, avatarImg(p, { size, org: this.isOrg(agentId) }));
  }

  _postRef(id, label) {
    return el("button", {
      class: "linkish", type: "button", text: label || `#${id}`,
      onclick: (e) => { e.stopPropagation(); this.onSelectPost(id); },
    });
  }

  _post(post, children, depth, isRoot) {
    const persona = this.personas.get(post.author_id);
    const handle = this._handle(post.author_id, post.author_handle);
    const art = el("article", {
      class: "post" + (this.newIds.has(post.id) && !reducedMotion() ? " is-new" : ""),
      "data-post-id": post.id,
      "aria-label": `Post #${post.id} by @${handle}`,
      tabindex: "-1",
    });

    art.appendChild(this._avatar(post.author_id, handle, isRoot ? 28 : 22));
    const head = el("div", { class: "post-head" },
      el("button", {
        class: "who", type: "button", text: (persona && persona.name) || `@${handle}`,
        title: `Open @${handle}`, onclick: (e) => { e.stopPropagation(); this.onSelectAgent(post.author_id); },
      }),
      persona && persona.name ? el("span", { class: "handle", text: `@${handle}` }) : null);
    if (post.repost_of !== null && post.repost_of !== undefined) {
      head.appendChild(el("span", { class: "tag" }, icon("repost"), "repost"));
    }
    if (this.scored && typeof post.sentiment === "number") head.appendChild(el("span", { class: "sv", "data-s": sentimentSign(post.sentiment), title: "Post sentiment, scored by the model", text: formatScore(post.sentiment) }));
    head.appendChild(el("span", { class: "pid", text: `#${post.id}` }));
    art.appendChild(head);

    // replying to a post from an earlier round (or a root reply whose parent is not in view)
    if (post.parent_id !== null && post.parent_id !== undefined && isRoot) {
      const parent = this.posts.get(post.parent_id);
      const ctx = el("div", { class: "post-ctx" }, icon("corner"), "replying to ");
      if (parent) {
        ctx.appendChild(el("button", {
          class: "linkish", type: "button", text: `@${this._handle(parent.author_id, parent.author_handle)}`,
          onclick: (e) => { e.stopPropagation(); this.onSelectAgent(parent.author_id); },
        }));
        ctx.appendChild(this._postRef(parent.id));
        ctx.appendChild(el("span", { class: "snip", text: `r${parent.round}: “${snippet(parent.content)}”` }));
      } else {
        ctx.appendChild(this._postRef(post.parent_id));
      }
      art.appendChild(ctx);
    }

    const text = (post.content || "").trim();
    if (text) art.appendChild(el("div", { class: "post-body", text: post.content }));
    else if (post.repost_of === null || post.repost_of === undefined) {
      art.appendChild(el("div", { class: "post-body empty", text: "(empty post)" }));
    }

    if (post.repost_of !== null && post.repost_of !== undefined) {
      const orig = this.posts.get(post.repost_of);
      const quote = el("button", {
        class: "quote", type: "button",
        "aria-label": `Quoted post #${post.repost_of}`,
        onclick: (e) => { e.stopPropagation(); this.onSelectPost(post.repost_of); },
      });
      if (orig) {
        const op = this.personas.get(orig.author_id) || { id: orig.author_id, handle: orig.author_handle };
        const qh = el("div", { class: "post-head" },
          avatarImg(op, { size: 18, org: this.isOrg(orig.author_id) }),
          el("span", { class: "handle", text: `@${this._handle(orig.author_id, orig.author_handle)}` }),
          el("span", { class: "pid", style: "margin-left:auto", text: `#${orig.id} · r${orig.round}` }));
        quote.append(qh, el("div", { class: "post-body", text: orig.content || "" }));
      } else {
        quote.append(el("div", { class: "post-body empty", text: `Post #${post.repost_of} is not loaded` }));
      }
      art.appendChild(quote);
    }

    const counts = this._counts(post);
    if (counts) art.appendChild(counts);

    const kids = children.get(post.id) || [];
    if (kids.length) {
      if (depth + 1 < MAX_DEPTH) {
        const list = el("div", { class: "replies" });
        for (const k of kids) list.appendChild(this._post(k, children, depth + 1, false));
        art.appendChild(list);
      } else {
        // flatten very deep threads to keep the rail readable
        const flat = [];
        const walk = (p) => { for (const k of children.get(p.id) || []) { flat.push(k); walk(k); } };
        walk(post);
        const list = el("div", { class: "replies" });
        for (const k of flat) list.appendChild(this._post(k, new Map(), MAX_DEPTH, false));
        art.appendChild(list);
      }
    }
    return art;
  }

  _counts(post) {
    // only the reactions that happened; a post nobody answered shows no counter row at all
    const items = [["reply", "reply", post.replies, "replies"], ["repost", "repost", post.reposts, "reposts"],
      ["like", "like", post.likes, "likes"], ["dislike", "dislike", post.dislikes, "dislikes"]].filter(([, , n]) => n > 0);
    if (!items.length) return null;
    return el("div", { class: "counts", "aria-label": items.map(([, , n, label]) => `${n} ${n === 1 ? label.replace(/ies$/, "y").replace(/s$/, "") : label}`).join(", ") },
      items.map(([cls, name, n, label]) => el("span", { class: `c-${cls}`, title: `${n} ${label}` }, icon(name), String(n))));
  }

}
