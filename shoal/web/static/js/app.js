// Shoal Studio: application shell. Owns the Studio state object, the hash router, the theme, toasts, the
// runs sidebar and the run screen (Overview, Crowd, Report, Details), live updates over SSE and the replay
// player. Model-generated text is only ever inserted with textContent.

import { api, stream, ApiError } from "./api.js";
import { Feed, formatScore, sentimentSign } from "./feed.js";
import { AgentDrawer } from "./agent.js";
import { mountNewRun } from "./newrun.js";
import { avatarImg } from "./avatar.js";

// The canvas modules are loaded on demand so a failure in one of them degrades a single card instead of
// the whole app.
const viz = {
  network: () => import("./network.js"),
  kgraph: () => import("./kgraph.js"),
  charts: () => import("./charts.js"),
};

// ------------------------------------------------------------------ state

export const Studio = {
  health: null,
  theme: document.documentElement.getAttribute("data-theme") || "light",
  route: { name: "home", params: {}, query: {} },
  run: null, // the run screen's state while it is mounted
  runs: [],
};
window.Studio = Studio; // handy for debugging from the console

const STAGES = [
  ["reading", "Reading documents"],
  ["graph", "Mapping the topic"],
  ["personas", "Casting people"],
  ["seeding", "Seeding the conversation"],
  ["rounds", "Simulating rounds"],
  ["report", "Writing the report"],
];
const STAGE_INDEX = Object.fromEntries(STAGES.map(([k], i) => [k, i]));
const TERMINAL = new Set(["done", "failed", "stopped"]);
const ROUND_MS = 2400;
const TABS = [["overview", "Overview"], ["crowd", "Crowd"], ["report", "Report"], ["details", "Details"]];
const TAB_ALIASES = { network: "crowd", agents: "crowd", feed: "crowd", graph: "details" };
const CAMP_COLORS = ["#6366F1", "#0EA5E9", "#14B8A6", "#A855F7", "#EC4899"];
const SLATE = "#94A3B8";
const STATE_WORD = { done: "Done", running: "Running", queued: "Queued", stopped: "Stopped", failed: "Failed" };

// ------------------------------------------------------------------ DOM helpers

const SVG_NS = "http://www.w3.org/2000/svg";
const ICONS = {
  stop: "M7 7h10v10H7z",
  event: "M13 2 3 14h9l-1 8 10-12h-9l1-8z",
  external: "M15 3h6v6M10 14 21 3M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6",
  refresh: "M3 12a9 9 0 0 1 15-6.7L21 8M21 3v5h-5M21 12a9 9 0 0 1-15 6.7L3 16M3 21v-5h5",
  sun: "M12 3v2M12 19v2M4.2 4.2l1.4 1.4M18.4 18.4l1.4 1.4M3 12h2M19 12h2M4.2 19.8l1.4-1.4M18.4 5.6l1.4-1.4M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z",
  moon: "M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z",
  x: "M18 6 6 18M6 6l12 12",
  plus: "M12 5v14M5 12h14",
  minus: "M5 12h14",
  fit: "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5",
  play: "M7 4.5v15l12-7.5z",
  pause: "M8 5v14M16 5v14",
  prev: "M15 6l-6 6 6 6",
  next: "M9 6l6 6-6 6",
  more: "M5 12h.01M12 12h.01M19 12h.01",
  check: "M5 12.5l4.5 4.5L19 7",
  back: "M15 18l-6-6 6-6",
  doc: "M7 3h7l5 5v13H7zM14 3v5h5",
  details: "M4 5h16v14H4zM14 5v14",
  alert: "M12 8v5M12 16.5h.01M10.3 3.9 2.4 17.5A2 2 0 0 0 4.1 20.5h15.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z",
  link: "M10 14a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-1 1M14 10a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l1-1",
  chat: "M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.6A8 8 0 1 1 21 12z",
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
    else if (k === "style") node.style.cssText = v;
    else if (k.startsWith("on") && typeof v === "function") node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v === true ? "" : String(v));
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    node.appendChild(typeof c === "string" || typeof c === "number" ? document.createTextNode(String(c)) : c);
  }
  return node;
}

function svgEl(tag, attrs = {}) {
  const n = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, String(v));
  return n;
}

function campDot(color, cls = "cdot") {
  const d = el("span", { class: cls, "aria-hidden": "true" });
  d.style.background = color;
  return d;
}

function sv(score, extra = {}) {
  return el("span", { class: "sv", "data-s": sentimentSign(score), text: formatScore(score), ...extra });
}

const isScore = (v) => typeof v === "number" && !Number.isNaN(v);

/** Model text without Markdown markers, for one-line summaries inside buttons. */
const plain = (t) => String(t || "").replace(/\*\*(.+?)\*\*/g, "$1").replace(/`([^`]+)`/g, "$1").replace(/(?<![*\w])\*(\S.*?\S|\S)\*/g, "$1");

/** Status in plain words with a small mark. */
function statusChip(state, { cli = false, compact = false } = {}) {
  const word = STATE_WORD[state] || state || "Unknown";
  return el("span", { class: compact ? "status-chip compact" : "status-chip", "data-state": state, title: cli ? "Started from the command line; watching its files" : undefined },
    el("span", { class: "dot", "aria-hidden": "true" }), word);
}

const RUN_GROUPS = [
  ["Running", (st) => st === "running" || st === "queued"],
  ["Done", (st) => st === "done"],
  ["Stopped", (st) => st === "stopped"],
  ["Failed", (st) => st === "failed"],
];

function groupRuns(runs) {
  const out = [];
  const seen = new Set();
  for (const [label, match] of RUN_GROUPS) {
    const list = runs.filter((r) => match(r.state));
    list.forEach((r) => seen.add(r.id));
    if (list.length) out.push([label, list]);
  }
  const rest = runs.filter((r) => !seen.has(r.id));
  if (rest.length) out.push(["Other", rest]);
  return out;
}

const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));

function stateBlock({ title, body, kind = "", glyph = null, action = null, overlay = false, spinner = false }) {
  return el("div", { class: `state ${kind} ${overlay ? "overlay" : ""}`.trim(), role: kind === "error" ? "alert" : undefined },
    spinner ? el("span", { class: "spinner", "aria-hidden": "true" }) : glyph,
    el("div", { class: "state-title", text: title }),
    body ? el("div", { class: "state-body", text: body }) : null,
    action);
}

function skel(w, h = 12, extra = "") {
  return el("span", { class: "skel", style: `width:${w};height:${h}px;${extra}` });
}

function parseTime(iso) {
  if (!iso) return null;
  const t = Date.parse(iso);
  return Number.isNaN(t) ? null : t;
}

function relTime(iso) {
  const t = parseTime(iso);
  if (t === null) return "";
  const s = Math.round((Date.now() - t) / 1000);
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  if (d < 8) return `${d} d ago`;
  return new Date(t).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

function absTime(iso) {
  const t = parseTime(iso);
  return t === null ? "" : new Date(t).toLocaleString();
}

function fmtDuration(ms) {
  if (!Number.isFinite(ms) || ms < 0) return "n/a";
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

/** Where a run's model calls went: "local / claude code / haiku + sonnet". Older configs have no runtime. */
function provenance(cfg = {}) {
  const name = (id) => String(id || "claude-code").replace(/[-_]+/g, " ");
  const agentRt = cfg.agent_runtime || "claude-code";
  const plannerRt = cfg.planner_runtime || "claude-code";
  const rt = agentRt === plannerRt ? name(agentRt) : `${name(agentRt)} + ${name(plannerRt)}`;
  const agentM = cfg.agent_model || "haiku";
  const plannerM = cfg.planner_model || "sonnet";
  const models = agentM === plannerM ? agentM : `${agentM} + ${plannerM}`;
  return {
    text: `local / ${rt} / ${models}${cfg.think ? " / think" : ""}`,
    title: `Ran on this machine. Agents: ${agentM} via ${name(agentRt)}. Planner (graph, personas, report): ${plannerM} via ${name(plannerRt)}.`,
  };
}

// ------------------------------------------------------------------ toasts

const toastHost = document.getElementById("toasts");

export function toast(kind, title, body = "", ms = 5200) {
  if (!toastHost) return;
  const node = el("div", { class: "toast", "data-kind": kind, role: kind === "error" ? "alert" : "status" },
    el("span", { class: "toast-mark", "aria-hidden": "true" }),
    el("div", {}, el("div", { class: "toast-title", text: title }), body ? el("div", { class: "toast-body", text: body }) : null),
    el("button", { class: "icon-btn", type: "button", "aria-label": "Dismiss", style: "width:24px;height:24px", onclick: () => dismiss() }, icon("x", "icon sm")));
  const dismiss = () => {
    if (!node.isConnected) return;
    node.classList.add("leaving");
    setTimeout(() => node.remove(), reducedMotion() ? 0 : 220);
  };
  toastHost.appendChild(node);
  while (toastHost.children.length > 4) toastHost.firstElementChild.remove();
  if (ms) setTimeout(dismiss, kind === "error" ? ms * 1.6 : ms);
}

document.addEventListener("studio:toast", (e) => {
  const d = e.detail || {};
  toast(d.kind || "info", d.title || "", d.body || "");
});

// ------------------------------------------------------------------ theme

const themeBtn = document.getElementById("theme-toggle");
const themeIcon = document.getElementById("theme-icon");

function storedTheme() {
  try {
    const t = localStorage.getItem("shoal.theme");
    return t === "light" || t === "dark" ? t : null;
  } catch {
    return null;
  }
}

function applyTheme(theme, { persist = false } = {}) {
  Studio.theme = theme;
  document.documentElement.setAttribute("data-theme", theme);
  if (persist) {
    try {
      localStorage.setItem("shoal.theme", theme);
    } catch {
      /* storage blocked: the choice lasts for this page only */
    }
  }
  if (themeIcon) themeIcon.setAttribute("d", theme === "dark" ? ICONS.sun : ICONS.moon);
  if (themeBtn) {
    const label = theme === "dark" ? "Switch to light theme" : "Switch to dark theme";
    themeBtn.setAttribute("aria-label", label);
    themeBtn.title = label;
  }
  document.dispatchEvent(new CustomEvent("studio:themechange", { detail: { theme } }));
}

applyTheme(Studio.theme);
themeBtn?.addEventListener("click", () => applyTheme(Studio.theme === "dark" ? "light" : "dark", { persist: true }));
window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", (e) => {
  if (!storedTheme()) applyTheme(e.matches ? "dark" : "light");
});

// ------------------------------------------------------------------ health / connection

const connEl = document.getElementById("conn");
const fakeBadge = document.getElementById("fake-badge");

function setConn(state, label) {
  if (!connEl) return;
  connEl.dataset.state = state;
  connEl.querySelector(".lbl").textContent = label;
}

async function checkHealth() {
  try {
    const h = await api.health();
    Studio.health = h;
    setConn("ok", "Local");
    if (connEl) connEl.title = `Shoal ${h.version || ""} on this machine; nothing leaves it except calls to the model you chose`.trim();
    if (fakeBadge) fakeBadge.hidden = !h.fake_llm;
    return true;
  } catch {
    setConn("down", "Offline");
    return false;
  }
}

// ------------------------------------------------------------------ sidebar (runs, on every screen)

const sideList = document.getElementById("side-list");
const navBtn = document.getElementById("nav-btn");
const scrim = document.getElementById("scrim");
const narrowMQ = window.matchMedia("(max-width: 900px)");
let sideTimer = null;

// Rounds actually recorded per stopped / failed run (stats can run past status.round). The run screen fills
// it for the open run; the sidebar fetches it once for the others so every surface shows the same number.
const recordedRounds = new Map();
const recordedPending = new Set();
function doneRounds(r) {
  return Math.min(Math.max(r.round || 0, recordedRounds.get(r.id) ?? 0), r.total_rounds || Infinity);
}
async function learnRecorded(runs) {
  const todo = runs.filter((r) => (r.state === "stopped" || r.state === "failed") && !recordedRounds.has(r.id) && !recordedPending.has(r.id));
  if (!todo.length) return;
  await Promise.all(todo.map(async (r) => {
    recordedPending.add(r.id);
    try {
      const d = await api.run(r.id);
      recordedRounds.set(r.id, Math.max(0, ...(d.stats || []).map((x) => x.round)));
    } catch {
      /* keep status.round */
    } finally {
      recordedPending.delete(r.id);
    }
  }));
  renderSidebar();
  Studio.onRecorded?.();
}

function setNav(open) {
  document.body.dataset.nav = open ? "open" : "closed";
  navBtn?.setAttribute("aria-expanded", String(open));
  navBtn?.setAttribute("aria-label", open ? "Hide runs" : "Show runs");
}
navBtn?.addEventListener("click", () => setNav(document.body.dataset.nav !== "open"));
scrim?.addEventListener("click", () => setNav(false));
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && document.body.dataset.nav === "open" && narrowMQ.matches) setNav(false);
});
narrowMQ.addEventListener("change", () => setNav(false));

function currentRunId() {
  return Studio.route && Studio.route.name === "run" ? Studio.route.params.id : null;
}

function renderSidebar() {
  if (!sideList) return;
  const runs = Studio.runs || [];
  const kids = [];
  for (const [label, list] of groupRuns(runs)) {
    kids.push(el("div", { class: "side-group" }, el("span", { text: label }), el("span", { class: "num", text: String(list.length) })));
    for (const r of list) {
      const live = r.state === "running" || r.state === "queued";
      const total = Math.max(1, r.total_rounds || 0);
      const pct = clamp(((r.round || 0) / total) * 100, 0, 100);
      const when = live ? "" : relTime(r.created_at);
      const sub = r.state === "done"
        ? `${r.total_rounds} rounds`
        : live ? `Round ${Math.min((r.round || 0) + 1, r.total_rounds)} of ${r.total_rounds}`
          : `r${doneRounds(r)} of ${r.total_rounds}`;
      kids.push(el("a", {
        class: "side-run", href: `#/run/${encodeURIComponent(r.id)}`, "data-state": r.state, title: r.question || r.id,
        "aria-current": r.id === currentRunId() ? "page" : undefined,
        onclick: () => { if (narrowMQ.matches) setNav(false); },
      },
      el("span", { class: "t", text: r.question || "(untitled run)" }),
      live ? el("span", { class: "pr", "aria-hidden": "true" }, el("i", { style: `width:${pct.toFixed(1)}%` })) : null,
      el("span", { class: "s" }, statusChip(r.state, { compact: true }), el("span", { class: "d", text: sub }),
        when ? el("span", { class: "w", text: when }) : null)));
    }
  }
  if (!kids.length) kids.push(el("p", { class: "side-empty", text: "No runs in this folder yet." }));
  sideList.replaceChildren(...kids);
}

async function loadSidebar() {
  clearTimeout(sideTimer);
  try {
    const runs = await api.runs();
    Studio.runs = runs;
    setConn("ok", "Local");
    renderSidebar();
    learnRecorded(runs);
    const anyLive = runs.some((r) => r.state === "running" || r.state === "queued");
    sideTimer = setTimeout(loadSidebar, anyLive ? 4000 : 15000);
  } catch {
    sideTimer = setTimeout(loadSidebar, 10000);
  }
}

// ------------------------------------------------------------------ router & top bar

const view = document.getElementById("view");
const crumbs = document.getElementById("crumbs");
const topChip = document.getElementById("top-chip");
const topProv = document.getElementById("top-prov");
const topActions = document.getElementById("top-actions");
let unmountView = null;

function parseHash() {
  const raw = (location.hash || "#/").slice(1) || "/";
  const [path, qs = ""] = raw.split("?");
  const query = Object.fromEntries(new URLSearchParams(qs));
  const parts = path.split("/").filter(Boolean);
  if (parts.length === 0) return { name: "home", params: {}, query };
  if (parts[0] === "new" && parts.length === 1) return { name: "new", params: {}, query };
  if (parts[0] === "run" && parts.length === 2) {
    let id = parts[1];
    try {
      id = decodeURIComponent(id);
    } catch {
      /* keep raw */
    }
    return { name: "run", params: { id }, query };
  }
  return { name: "notfound", params: {}, query };
}

/** Update the hash query without triggering the router (used for tab and round). */
function replaceQuery(patch) {
  const r = parseHash();
  const q = { ...r.query, ...patch };
  for (const k of Object.keys(q)) if (q[k] === null || q[k] === undefined || q[k] === "") delete q[k];
  const path = (location.hash || "#/").slice(1).split("?")[0] || "/";
  const qs = new URLSearchParams(q).toString();
  history.replaceState(null, "", `#${path}${qs ? "?" + qs : ""}`);
}

function setCrumbs(items) {
  crumbs.replaceChildren();
  items.forEach((it, i) => {
    if (i) crumbs.appendChild(el("span", { class: "sep", "aria-hidden": "true", text: "/" }));
    if (it.href) crumbs.appendChild(el("a", { href: it.href, text: it.label }));
    else crumbs.appendChild(el("span", { class: "here", text: it.label, title: it.title || it.label, "aria-current": "page" }));
  });
}

function clearTop() {
  topChip.replaceChildren();
  topProv.textContent = "";
  topProv.title = "";
  topActions.replaceChildren();
}

function route() {
  const r = parseHash();
  const prev = Studio.route;
  if (prev && prev.name === "run" && r.name === "run" && prev.params.id === r.params.id && Studio.run) {
    Studio.route = r;
    Studio.run.applyQuery?.(r.query);
    return;
  }
  Studio.route = r;
  if (unmountView) {
    try {
      unmountView();
    } catch (err) {
      console.error(err);
    }
    unmountView = null;
  }
  clearTop();
  view.replaceChildren();
  view.scrollTop = 0;
  document.getElementById("new-btn")?.setAttribute("aria-current", r.name === "new" ? "page" : "false");
  if (r.name === "home") unmountView = mountHome(view);
  else if (r.name === "new") unmountView = mountNew(view);
  else if (r.name === "run") unmountView = mountRun(view, r.params.id, r.query);
  else unmountView = mountNotFound(view);
  renderSidebar();
}

window.addEventListener("hashchange", route);

// ------------------------------------------------------------------ home

function mountNotFound(root) {
  setCrumbs([{ label: "Not found" }]);
  document.title = "Not found · Shoal";
  root.appendChild(el("div", { class: "page" },
    stateBlock({ title: "Nothing here", body: "That address does not match a Shoal Studio screen.",
      action: el("a", { class: "btn", href: "#/" }, "All runs") })));
  return () => {};
}

function mountHome(root) {
  setCrumbs([{ label: "Runs" }]);
  document.title = "Shoal";
  let timer = null;
  let alive = true;
  let firstLoad = true;

  const list = el("div", { class: "runs", "aria-label": "Simulation runs" });
  const page = el("div", { class: "page" },
    el("div", { class: "page-head" },
      el("div", {},
        el("h1", { text: "Simulations" }),
        el("p", { text: "Seed documents and a question in, a simulated crowd's reaction and a prediction out. Open a run to read its answer, watch the crowd or replay it round by round." })),
      el("a", { class: "btn btn-primary", href: "#/new" }, icon("plus", "icon sm"), "New simulation")),
    list);
  root.appendChild(page);

  const skeleton = () => {
    list.replaceChildren(el("div", { class: "run-group", "aria-hidden": "true" },
      el("h2", {}, skel("80px", 12)),
      el("div", { class: "run-list" }, ...Array.from({ length: 4 }, () =>
        el("div", { class: "run-row skel-row" },
          el("div", { class: "run-q" }, skel("72%", 13), skel("38%", 10)),
          skel("60px", 18),
          el("div", { class: "progress" }, skel("100%", 4), skel("40%", 10)),
          skel("50px", 12, "justify-self:end"),
          skel("70px", 12, "justify-self:end"))))));
  };

  const row = (r) => {
    const total = Math.max(1, r.total_rounds || 0);
    const done = clamp(doneRounds(r), 0, total);
    const pct = r.state === "done" ? 100 : (done / total) * 100;
    const live = r.state === "running" || r.state === "queued";
    return el("a", { class: "run-row", role: "listitem", href: `#/run/${encodeURIComponent(r.id)}`, "data-state": r.state },
      el("div", { class: "run-q" },
        el("span", { class: "q", text: r.question || "(untitled run)", title: r.question || "" }),
        el("span", { class: "id", text: r.has_report ? "Report ready" : live ? STAGES[STAGE_INDEX[r.stage] ?? 0][1] : "No report" })),
      el("span", { class: "state-col" }, statusChip(r.state)),
      el("div", { class: "progress", "data-state": r.state, title: `${done} of ${r.total_rounds} rounds` },
        el("div", { class: "bar" }, el("i", { style: `width:${pct.toFixed(1)}%` })),
        el("span", { class: "lbl", text: `${done} of ${r.total_rounds} rounds` })),
      el("span", { class: "agents r", text: `${r.agents ?? "-"} agents` }),
      el("span", { class: "date r", title: absTime(r.created_at), text: relTime(r.created_at) }));
  };

  const renderGroups = (runs) => {
    list.replaceChildren(...groupRuns(runs).map(([label, rs]) =>
      el("section", { class: "run-group", "aria-label": `${label} runs` },
        el("h2", {}, label, el("span", { class: "num", text: String(rs.length) })),
        el("div", { class: "run-list", role: "list" }, rs.map(row)))));
  };

  const load = async () => {
    if (!alive) return;
    if (firstLoad) skeleton();
    try {
      const runs = await api.runs();
      if (!alive) return;
      Studio.runs = runs;
      renderSidebar();
      learnRecorded(runs);
      setConn("ok", "Local");
      if (!runs.length) {
        list.hidden = true;
        let empty = page.querySelector(".empty-home");
        if (!empty) {
          empty = el("div", { class: "empty-home" },
            shoalGlyph(),
            el("h2", { text: "No simulations yet" }),
            el("p", {}, "Start one from a few seed documents and a question. Runs started from the command line with ", el("code", { text: "shoal run" }), " show up here too."),
            el("a", { class: "btn btn-primary", href: "#/new" }, icon("plus", "icon sm"), "New simulation"));
          page.appendChild(empty);
        }
      } else {
        list.hidden = false;
        page.querySelector(".empty-home")?.remove();
        renderGroups(runs);
      }
      firstLoad = false;
      const anyLive = runs.some((r) => r.state === "running" || r.state === "queued");
      timer = setTimeout(load, anyLive ? 3000 : 15000);
    } catch (err) {
      if (!alive) return;
      if (firstLoad) {
        list.replaceChildren(stateBlock({ kind: "error", title: "Could not load runs", body: err.message,
          action: el("button", { class: "btn", type: "button", onclick: () => { firstLoad = true; load(); } }, "Retry") }));
      }
      setConn("down", "Offline");
      timer = setTimeout(load, 8000);
    }
  };
  load();
  Studio.onRecorded = () => { if (alive && Studio.runs.length) renderGroups(Studio.runs); };
  return () => {
    alive = false;
    Studio.onRecorded = null;
    clearTimeout(timer);
  };
}

function shoalGlyph() {
  const s = svgEl("svg", { width: 104, height: 56, viewBox: "0 0 26 14", "aria-hidden": "true" });
  for (const [x, y, r] of [[22, 7, 2.6], [15.5, 2.8, 2.1], [15.5, 11.2, 2.1], [9, 7, 1.9], [3.4, 3.2, 1.5]]) {
    s.appendChild(svgEl("circle", { cx: x, cy: y, r, fill: "currentColor" }));
  }
  return s;
}

// ------------------------------------------------------------------ new run

function mountNew(root) {
  setCrumbs([{ label: "Runs", href: "#/" }, { label: "New simulation" }]);
  document.title = "New simulation · Shoal";
  const page = el("div", { class: "page" },
    el("div", { class: "page-head" },
      el("div", {},
        el("h1", { text: "New simulation" }),
        el("p", { text: "Shoal reads your documents, maps the topic, casts a crowd of personas and lets them talk for a number of rounds. Everything runs on this machine with the model tools you already have." }))));
  const host = el("div");
  page.appendChild(host);
  root.appendChild(page);
  const unmount = mountNewRun(host, {
    api,
    onCreated: (id) => {
      loadSidebar();
      location.hash = `#/run/${encodeURIComponent(id)}`;
    },
  });
  return () => {
    if (typeof unmount === "function") unmount();
  };
}

// ------------------------------------------------------------------ citation hover card (report, reasons)

const hoverCard = el("div", { class: "hovercard", "aria-hidden": "true" });
document.body.appendChild(hoverCard);
let hoverTarget = null;
let hoverTimer = null;

function showHoverCard(target) {
  const S = Studio.run;
  if (!S || !S.hoverContent) return;
  const content = S.hoverContent(target);
  if (!content) return;
  hoverCard.replaceChildren(content);
  hoverCard.classList.add("on");
  const r = target.getBoundingClientRect();
  const w = hoverCard.offsetWidth, h = hoverCard.offsetHeight;
  let x = r.left + r.width / 2 - w / 2;
  let y = r.top - h - 8;
  if (y < 8) y = r.bottom + 8;
  x = clamp(x, 8, window.innerWidth - w - 8);
  hoverCard.style.left = `${Math.round(x)}px`;
  hoverCard.style.top = `${Math.round(y)}px`;
}

function hideHoverCard() {
  hoverTarget = null;
  clearTimeout(hoverTimer);
  hoverCard.classList.remove("on");
}

document.addEventListener("mouseover", (e) => {
  const t = e.target.closest?.("[data-agent].ref, [data-post].ref");
  if (t === hoverTarget) return;
  if (!t) { hideHoverCard(); return; }
  hoverTarget = t;
  clearTimeout(hoverTimer);
  hoverTimer = setTimeout(() => { if (hoverTarget === t) showHoverCard(t); }, 120);
});
document.addEventListener("focusin", (e) => {
  const t = e.target.closest?.("[data-agent].ref, [data-post].ref");
  if (t) { hoverTarget = t; showHoverCard(t); } else if (hoverTarget) hideHoverCard();
});
document.addEventListener("scroll", hideHoverCard, true);

// ------------------------------------------------------------------ menu

function menuButton(label, items) {
  const btn = el("button", { class: "btn", type: "button", "aria-haspopup": "menu", "aria-expanded": "false", "aria-label": label, title: label },
    icon("more"));
  const menu = el("div", { class: "menu", role: "menu", hidden: true });
  const wrap = el("span", { class: "menu-wrap" }, btn, menu);
  const close = (focus = false) => {
    menu.hidden = true;
    btn.setAttribute("aria-expanded", "false");
    document.removeEventListener("pointerdown", outside, true);
    if (focus) btn.focus();
  };
  const outside = (e) => { if (!wrap.contains(e.target)) close(); };
  const open = () => {
    menu.replaceChildren(...items().filter(Boolean).map((it) => {
      const b = el(it.href ? "a" : "button", {
        role: "menuitem", type: it.href ? undefined : "button", href: it.href, target: it.href ? "_blank" : undefined,
        rel: it.href ? "noopener" : undefined, tabindex: "-1", "aria-disabled": it.disabled ? "true" : undefined, title: it.title,
      }, it.icon ? icon(it.icon, "icon sm") : null, it.label);
      b.addEventListener("click", (e) => {
        if (it.disabled) { e.preventDefault(); return; }
        close();
        it.run?.();
      });
      return b;
    }));
    menu.hidden = false;
    btn.setAttribute("aria-expanded", "true");
    document.addEventListener("pointerdown", outside, true);
    menu.querySelector("[role=menuitem]")?.focus();
  };
  btn.addEventListener("click", () => (menu.hidden ? open() : close()));
  menu.addEventListener("keydown", (e) => {
    const list = [...menu.querySelectorAll("[role=menuitem]")];
    const i = list.indexOf(document.activeElement);
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(true); }
    else if (e.key === "ArrowDown") { e.preventDefault(); list[(i + 1) % list.length]?.focus(); }
    else if (e.key === "ArrowUp") { e.preventDefault(); list[(i - 1 + list.length) % list.length]?.focus(); }
    else if (e.key === "Tab") close();
  });
  return wrap;
}

// ------------------------------------------------------------------ run screen

function mountRun(root, runId, query) {
  setCrumbs([{ label: "Runs", href: "#/" }, { label: runId }]);
  document.title = "Run · Shoal";

  const initialTab = TAB_ALIASES[query.tab] || query.tab;
  let graphView = "camps";
  try {
    const v = localStorage.getItem("shoal.graphView");
    if (["camps", "shoal", "influence"].includes(v)) graphView = v;
  } catch {
    /* storage blocked */
  }
  const S = {
    id: runId,
    detail: null,
    status: null,
    live: false, // the server owns this run (stop / inject allowed)
    personas: [],
    personaById: new Map(),
    personaByHandle: new Map(),
    posts: new Map(),
    lastPostId: 0,
    actions: new Map(), // round -> ActionOut[]
    stats: [],
    events: [],
    follows: [],
    report: null,
    round: null,
    latestRound: 0,
    followLive: true,
    playing: false,
    speed: 1,
    tab: TABS.some(([k]) => k === initialTab) ? initialTab : "overview",
    graphView,
    peopleMode: "posts",
    selectedAgent: null,
    selectedCamp: null,
    camps: [],
    campOf: new Map(),
    campsFallback: true,
    startedRound: 0,
    alive: true,
    closeStream: null,
    timers: new Set(),
    qa: [],
    failedBatches: [],
    orgIds: new Set(),
  };
  Studio.run = S;

  // -------------------------------------------------------------- top bar: status, provenance, actions
  const chipHost = el("span");
  topChip.replaceChildren(chipHost);
  const stopBtn = el("button", { class: "btn btn-danger", type: "button", hidden: true, title: "Stop after the current round" },
    icon("stop", "icon sm"), el("span", { class: "act-lbl", text: "Stop" }));
  const injectBtn = el("button", { class: "btn btn-event", type: "button", hidden: true, title: "Inject breaking news into a future round" },
    icon("event", "icon sm"), el("span", { class: "act-lbl", text: "Inject event" }));
  const more = menuButton("More actions", () => [
    S.live && isRunning() ? { label: "Inject event", icon: "event", run: () => openInjectDialog() } : null,
    { label: rebuilding ? "Rebuilding the report…" : "Rebuild report", icon: "refresh", disabled: rebuilding || isRunning(),
      title: isRunning() ? "Available once the run has finished" : "Write the report again from the stored run", run: () => rebuildReport() },
    S.report ? { label: "Open report page", icon: "external", href: api.reportUrl(runId) } : null,
    { label: "Run details", icon: "details", run: () => selectTab("details", { focus: true }) },
  ]);
  topActions.replaceChildren(stopBtn, injectBtn, more);

  // -------------------------------------------------------------- tabs row
  const tabBtns = {};
  const tablist = el("div", { class: "tabs", role: "tablist", "aria-label": "Run views" });
  for (const [key, label] of TABS) {
    const b = el("button", { class: "tab", role: "tab", type: "button", id: `tab-${key}`, "aria-controls": `pane-${key}`, "aria-selected": "false", tabindex: "-1" }, label);
    b.addEventListener("click", () => selectTab(key));
    tabBtns[key] = b;
    tablist.appendChild(b);
  }
  tablist.addEventListener("keydown", (e) => {
    const keys = TABS.map(([k]) => k);
    const cur = keys.indexOf(S.tab);
    let next = null;
    if (e.key === "ArrowRight") next = keys[(cur + 1) % keys.length];
    else if (e.key === "ArrowLeft") next = keys[(cur - 1 + keys.length) % keys.length];
    else if (e.key === "Home") next = keys[0];
    else if (e.key === "End") next = keys[keys.length - 1];
    if (next) {
      e.preventDefault();
      selectTab(next, { focus: true });
    }
  });
  const whereLbl = el("span", { class: "rl" });
  const streamLbl = el("span", { class: "stream-lbl", "aria-live": "polite" });
  const liveBtn = el("button", { class: "btn btn-sm live-btn", type: "button", hidden: true, onclick: () => jumpToLive() },
    el("span", { class: "dot", "aria-hidden": "true" }), "Live");
  const tabsRow = el("div", { class: "run-tabs" }, tablist, el("span", { class: "spacer" }),
    el("span", { class: "where" }, streamLbl, whereLbl, liveBtn));

  // -------------------------------------------------------------- overview
  const summaryCard = el("section", { class: "card summary-card", "aria-label": "Answer" });
  const metricsRow = el("div", { class: "metrics", role: "group", "aria-label": "Run figures" });
  const miniCanvas = el("canvas", { "aria-label": "The crowd as avatars. The Crowd tab lists the same people as text." });
  const miniHost = el("div", { class: "canvas-host" }, miniCanvas);
  const miniState = el("div");
  const miniSeg = viewSwitch();
  const miniNote = el("p", { class: "graph-note", hidden: true });
  const crowdCard = el("section", { class: "card", "aria-labelledby": "ov-crowd-h" },
    el("div", { class: "card-head" }, el("h2", { id: "ov-crowd-h", text: "Crowd" }),
      el("div", { class: "tools" }, miniSeg.root,
        el("button", { class: "btn btn-sm", type: "button", onclick: () => selectTab("crowd", { focus: true }) }, "Open"))),
    el("div", { class: "mini-graph" }, miniHost, miniState),
    el("div", { class: "graph-foot" }, el("span", { text: "Size shows influence. Click a person or a camp to open it." }), miniNote));
  const chartHost = el("div", { class: "chart-host" });
  const chartSel = el("div", { class: "chart-sel", "aria-live": "polite" });
  const chartCard = el("section", { class: "card", "aria-labelledby": "ov-chart-h" },
    el("div", { class: "card-head" }, el("h2", { id: "ov-chart-h", text: "Sentiment by round" }), el("span", { class: "sub", text: "Mean, −1 to +1" })),
    chartHost, chartSel);
  const campsCard = el("section", { class: "card camps-card", "aria-labelledby": "ov-camps-h" });
  const voicesCard = el("section", { class: "card voices-card", "aria-labelledby": "ov-voices-h" });
  const overview = el("div", { class: "ov" }, summaryCard, metricsRow, el("div", { class: "pair" }, crowdCard, chartCard), campsCard, voicesCard);
  const paneOverview = el("div", { class: "pane scroll", id: "pane-overview", role: "tabpanel", "aria-labelledby": "tab-overview", tabindex: "-1" }, overview);

  // -------------------------------------------------------------- crowd
  const netCanvas = el("canvas", { "aria-label": "The crowd as avatars, grouped by camp, sentiment or influence. The People list beside it holds the same information as text." });
  const netHost = el("div", { class: "canvas-host" }, netCanvas);
  const netState = el("div");
  const netNote = el("p", { class: "graph-note crowd-note", hidden: true });
  const zoom = el("div", { class: "zoom", role: "group", "aria-label": "Zoom" },
    el("button", { class: "icon-btn", type: "button", "aria-label": "Zoom in", title: "Zoom in (+)", onclick: () => network?.zoomIn() }, icon("plus")),
    el("button", { class: "icon-btn", type: "button", "aria-label": "Zoom out", title: "Zoom out (−)", onclick: () => network?.zoomOut() }, icon("minus")),
    el("button", { class: "icon-btn", type: "button", "aria-label": "Fit to view", title: "Fit (0)", onclick: () => network?.fit() }, icon("fit")));
  const crowdSeg = viewSwitch();
  const legend = el("div", { class: "graph-legend", "aria-label": "Legend" });
  const campNoteBar = el("div", { class: "graph-note", hidden: true });
  const prevBtn = el("button", { class: "icon-btn", type: "button", "aria-label": "Previous round", title: "Previous round (Left arrow)",
    onclick: () => { pause(); S.followLive = false; selectRound((S.round ?? 0) - 1, { animate: false }); } }, icon("prev"));
  const playBtn = el("button", { class: "icon-btn play", type: "button", "aria-label": "Play replay", "aria-pressed": "false", title: "Play replay (Space)",
    onclick: () => (S.playing ? pause() : play()) }, icon("play"));
  const nextBtn = el("button", { class: "icon-btn", type: "button", "aria-label": "Next round", title: "Next round (Right arrow)",
    onclick: () => { pause(); S.followLive = false; selectRound((S.round ?? 0) + 1, { animate: true }); } }, icon("next"));
  const roundsTrack = el("div", { class: "rounds", role: "group", "aria-label": "Rounds" });
  const speedSeg = el("div", { class: "seg speed", role: "group", "aria-label": "Playback speed" },
    [0.5, 1, 2].map((s) => el("button", { type: "button", "aria-pressed": String(s === 1), "data-speed": s, text: `${s}×`, onclick: () => setSpeed(s) })));
  const scrubber = el("div", { class: "scrubber", role: "group", "aria-label": "Replay" },
    el("span", { class: "ctl" }, prevBtn, playBtn, nextBtn), roundsTrack, speedSeg);
  const crowdStage = el("section", { class: "crowd-stage", "aria-label": "Crowd graph" },
    el("div", { class: "crowd-bar" }, crowdSeg.root, legend, el("span", { class: "spacer" }), campNoteBar),
    el("div", { class: "crowd-canvas" }, netHost, netState, netNote, zoom),
    scrubber);
  const insp = el("aside", { class: "insp", id: "run-insp", "aria-label": "Person", hidden: true });
  const campView = el("div", { class: "panel-view", hidden: true });
  const feedHost = el("div", { class: "feed", tabindex: "-1", "aria-label": "Posts for the selected round" });
  const peopleHost = el("div", { class: "people", hidden: true });
  const panelTitle = el("h2");
  const panelSub = el("span", { class: "sub" });
  const panelSeg = el("div", { class: "seg", role: "group", "aria-label": "Show" },
    el("button", { type: "button", "aria-pressed": "true", "data-mode": "posts", text: "Posts", onclick: () => setPeopleMode("posts") }),
    el("button", { type: "button", "aria-pressed": "false", "data-mode": "people", text: "People", onclick: () => setPeopleMode("people") }));
  const feedView = el("div", { class: "panel-view" },
    el("div", { class: "panel-head" }, el("div", { class: "ttl" }, panelTitle, panelSub), el("span", { class: "spacer" }), panelSeg),
    feedHost, peopleHost);
  const crowdPanel = el("div", { class: "crowd-panel" }, insp, campView, feedView);
  const crowd = el("div", { class: "crowd" }, crowdStage, crowdPanel);
  const paneCrowd = el("div", { class: "pane", id: "pane-crowd", role: "tabpanel", "aria-labelledby": "tab-crowd", hidden: true }, crowd);

  // -------------------------------------------------------------- report
  const reportRoot = el("div", { class: "report" });
  const paneReport = el("div", { class: "pane scroll", id: "pane-report", role: "tabpanel", "aria-labelledby": "tab-report", hidden: true, tabindex: "-1" }, reportRoot);

  // -------------------------------------------------------------- details
  const pipeCard = el("section", { class: "card det-card", "aria-labelledby": "det-pipe-h" });
  const factsCard = el("section", { class: "card det-card", "aria-labelledby": "det-run-h" });
  const roundsCard = el("section", { class: "card det-card", "aria-labelledby": "det-rounds-h" });
  const failCard = el("section", { class: "card det-card", "aria-labelledby": "det-fail-h", hidden: true });
  const runLog = buildRunLog();
  const kgCanvas = el("canvas", { "aria-label": "Knowledge graph of entities and relations extracted from the seed documents. Select an entity for details." });
  const kgHost = el("div", { class: "canvas-host" }, kgCanvas);
  const kgCard = el("div", { hidden: true });
  const kgState = el("div");
  const kgMeta = el("span", { class: "sub" });
  const kgSection = el("section", { class: "card kg-card", "aria-labelledby": "det-kg-h" },
    el("div", { class: "card-head" }, el("h2", { id: "det-kg-h", text: "Knowledge graph" }), kgMeta),
    el("p", { class: "note", text: "Entities and relations read from the seed documents. People and organisations that agents play are outlined twice." }),
    el("div", { class: "kg-body" }, kgHost, kgCard, kgState));
  const details = el("div", { class: "details" },
    el("div", { class: "det-grid" }, pipeCard, factsCard), roundsCard, failCard, runLog.root, kgSection);
  const paneDetails = el("div", { class: "pane scroll", id: "pane-details", role: "tabpanel", "aria-labelledby": "tab-details", hidden: true, tabindex: "-1" }, details);

  const panes = el("div", { class: "panes" }, paneOverview, paneCrowd, paneReport, paneDetails);
  summaryCard.append(skel("90px", 22), skel("60%", 12, "margin-top:12px"), skel("80%", 30, "margin-top:12px"), skel("70%", 14, "margin-top:16px"), skel("50%", 14, "margin-top:8px"));
  const runEl = el("div", { class: "run" }, tabsRow, panes);
  root.appendChild(runEl);
  root.scrollTop = 0;

  // -------------------------------------------------------------- components
  const feed = new Feed(feedHost, {
    onSelectAgent: (id) => openAgent(id),
    onSelectPost: (id) => selectPost(id),
    isOrg: (id) => S.orgIds.has(id),
  });
  const campFeedHost = el("div", { class: "feed" });
  const campFeed = new Feed(campFeedHost, {
    onSelectAgent: (id) => openAgent(id),
    onSelectPost: (id) => selectPost(id),
    isOrg: (id) => S.orgIds.has(id),
  });

  const drawer = new AgentDrawer(insp, {
    api,
    runId,
    onToggle: () => updatePanel(),
    onClose: () => {
      S.selectedAgent = null;
      netSelect();
      runLog.markSelected(null);
      updatePanel();
    },
    onSelectAgent: (id) => openAgent(id),
    onSelectPost: (id) => selectPost(id),
    getPersona: (id) => S.personaById.get(id) || null,
    isOrg: (id) => S.orgIds.has(id),
    getCamp: (id) => S.campOf.get(id) || null,
    getSentiment: (id) => sentimentUpTo(S.round)[id],
  });

  let network = null;
  let mini = null;
  let kgraph = null;
  let chart = null;
  let kgMounted = false;
  S.views = { get network() { return network; }, get mini() { return mini; }, get kgraph() { return kgraph; }, get chart() { return chart; }, feed, drawer };

  viz.network().then((m) => {
    if (!S.alive) return;
    const handlers = {
      onSelectAgent: (id) => openAgent(id),
      onSelectCamp: (key) => selectCamp(key, { tab: true }),
      onClear: () => clearSelection(),
    };
    network = new m.NetworkView(netCanvas, { ...handlers, onHoverAgent: () => {} });
    mini = new m.NetworkView(miniCanvas, { ...handlers, compact: true });
    if (S.detail) syncNetwork({ full: true });
  }).catch((err) => {
    console.error(err);
    const msg = String((err && err.message) || err);
    netState.replaceChildren(stateBlock({ overlay: true, kind: "error", title: "The crowd graph failed to load", body: msg }));
    miniState.replaceChildren(stateBlock({ overlay: true, kind: "error", title: "The crowd graph failed to load", body: msg }));
  });

  viz.charts().then((m) => {
    if (!S.alive) return;
    chart = new m.SentimentChart(chartHost, {
      onSelectRound: (r) => {
        pause();
        S.followLive = isRunning() && r >= S.latestRound;
        selectRound(r, { animate: false });
      },
    });
    if (S.detail) syncChart();
  }).catch((err) => {
    console.error(err);
    chartHost.replaceChildren(stateBlock({ kind: "error", title: "The chart failed to load", body: String((err && err.message) || err) }));
  });

  function ensureKg() {
    if (kgMounted) return;
    kgMounted = true;
    viz.kgraph().then((m) => {
      if (!S.alive) return;
      kgraph = new m.KnowledgeGraphView(kgCanvas, { onSelectEntity: (id) => showEntity(id) });
      syncGraph();
    }).catch((err) => {
      console.error(err);
      kgState.replaceChildren(stateBlock({ overlay: true, kind: "error", title: "The knowledge graph failed to load", body: String((err && err.message) || err) }));
    });
  }

  function viewSwitch() {
    const btns = {};
    const root = el("div", { class: "seg", role: "group", "aria-label": "Arrange the crowd by" });
    for (const [k, label, tip] of [["camps", "Camps", "Grouped by opinion camp"], ["shoal", "Shoal", "Left to right by sentiment"], ["influence", "Influence", "Big voices in the middle"]]) {
      btns[k] = el("button", { type: "button", "aria-pressed": "false", title: tip, text: label, onclick: () => setGraphView(k) });
      root.appendChild(btns[k]);
    }
    return { root, sync: () => { for (const [k, b] of Object.entries(btns)) b.setAttribute("aria-pressed", String(k === S.graphView)); } };
  }

  // -------------------------------------------------------------- data helpers
  const later = (fn, ms) => {
    const t = setTimeout(() => {
      S.timers.delete(t);
      if (S.alive) fn();
    }, ms);
    S.timers.add(t);
    return t;
  };

  function setPersonas(list) {
    S.personas = list || [];
    S.personaById = new Map(S.personas.map((p) => [p.id, p]));
    S.personaByHandle = new Map(S.personas.map((p) => [String(p.handle).toLowerCase(), p]));
    computeOrgs();
  }

  /** Organisations (monogram tiles) vs people (faces): an agent that plays a non-person graph entity. */
  function computeOrgs() {
    const kinds = new Map(((S.detail && S.detail.graph && S.detail.graph.entities) || []).map((e) => [e.id, String(e.kind || "").toLowerCase()]));
    S.orgIds = new Set(S.personas.filter((p) => p.entity_id && kinds.get(p.entity_id) !== "person").map((p) => p.id));
  }

  const isOrg = (id) => S.orgIds.has(id);
  const persona = (id) => S.personaById.get(id) || null;
  const av = (id, size) => avatarImg(persona(id) || { id }, { size, org: isOrg(id) });

  /** Camps from the report (every agent in exactly one), else the fallback groups Organisations and People. */
  function computeCamps() {
    const rep = S.report;
    const known = new Set(S.personas.map((p) => p.id));
    const mean = (ids) => postMean(ids, null);
    let camps = [];
    if (rep && Array.isArray(rep.camps) && rep.camps.length) {
      let ci = 0;
      const seen = new Set();
      camps = rep.camps.map((c, i) => {
        const members = (c.members || []).filter((id) => known.has(id) && !seen.has(id));
        members.forEach((id) => seen.add(id));
        const other = /^other$/i.test(String(c.name || "").trim());
        return { key: `c${i}`, name: c.name || `Camp ${i + 1}`, summary: c.summary || "", members,
          reported: isScore(c.sentiment) ? c.sentiment : null, sentiment: null, color: other ? SLATE : CAMP_COLORS[ci++ % CAMP_COLORS.length] };
      }).filter((c) => c.members.length);
      const rest = S.personas.filter((p) => !seen.has(p.id)).map((p) => p.id);
      if (rest.length) camps.push({ key: "rest", name: "Other", summary: "Agents the report did not place in a camp.", members: rest, reported: null, sentiment: null, color: SLATE });
      S.campsFallback = false;
    } else {
      const orgs = S.personas.filter((p) => isOrg(p.id)).map((p) => p.id);
      const people = S.personas.filter((p) => !isOrg(p.id)).map((p) => p.id);
      if (orgs.length) camps.push({ key: "orgs", name: "Organisations", summary: "", members: orgs, reported: null, sentiment: null, color: SLATE });
      if (people.length) camps.push({ key: "people", name: "People", summary: "", members: people, reported: null, sentiment: null, color: SLATE });
      S.campsFallback = true;
    }
    S.camps = camps;
    updateCampMeans();
    S.campOf = new Map();
    for (const c of camps) for (const id of c.members) S.campOf.set(id, c);
    if (S.selectedCamp && !camps.some((c) => c.key === S.selectedCamp)) S.selectedCamp = null;
  }

  /** Post-weighted mean sentiment of these agents' scored posts up to round r (null: all rounds). */
  function postMean(ids, r) {
    const set = new Set(ids);
    let sum = 0, n = 0;
    for (const p of S.posts.values()) {
      if (!set.has(p.author_id) || !isScore(p.sentiment) || (r !== null && p.round > r)) continue;
      sum += p.sentiment;
      n += 1;
    }
    return n ? sum / n : null;
  }

  /** Camp means follow the playhead: the report's figure at the final round, the measured mean before it. */
  function updateCampMeans() {
    const r = S.round;
    const final = r === null || r >= S.latestRound;
    for (const c of S.camps) c.sentiment = final && isScore(c.reported) ? c.reported : postMean(c.members, final ? null : r);
  }

  function postsList() {
    return [...S.posts.values()].sort((a, b) => a.id - b.id);
  }

  function postsOfRound(r) {
    return postsList().filter((p) => p.round === r);
  }

  function allActions() {
    return [...S.actions.entries()].sort((a, b) => a[0] - b[0]).flatMap(([, list]) => list);
  }

  function sentimentUpTo(r) {
    const sums = new Map();
    for (const p of S.posts.values()) {
      if (r !== null && p.round > r) continue;
      if (!isScore(p.sentiment)) continue;
      const cur = sums.get(p.author_id) || [0, 0];
      cur[0] += p.sentiment;
      cur[1] += 1;
      sums.set(p.author_id, cur);
    }
    const out = {};
    for (const p of S.personas) {
      const s = sums.get(p.id);
      out[p.id] = s ? s[0] / s[1] : null;
    }
    return out;
  }

  /** Whether any post carries a sentiment score (older runs scored rounds only). */
  function hasPostSentiment() {
    for (const p of S.posts.values()) if (isScore(p.sentiment)) return true;
    return false;
  }

  function minRound() {
    const rs = [...S.stats.map((s) => s.round), ...[...S.posts.values()].map((p) => p.round)];
    return rs.length ? Math.min(0, ...rs) : 0;
  }

  function computeLatest() {
    const st = S.status;
    let r = st ? st.round || 0 : 0;
    for (const s of S.stats) r = Math.max(r, s.round);
    for (const p of S.posts.values()) r = Math.max(r, p.round);
    S.latestRound = r;
    return r;
  }

  const isRunning = () => !!S.status && (S.status.state === "running" || S.status.state === "queued");

  /** Rounds completed: status.round, unless the recorded stats / actions go further (trust the data). */
  function roundsDone() {
    let r = S.status ? S.status.round || 0 : 0;
    for (const s of S.stats) r = Math.max(r, s.round);
    for (const k of S.actions.keys()) r = Math.max(r, k);
    const total = totalRounds();
    r = total ? Math.min(r, total) : r;
    if (S.status && !isRunning() && recordedRounds.get(runId) !== r) {
      recordedRounds.set(runId, r);
      renderSidebar();
    }
    return r;
  }
  const totalRounds = () => (S.status ? S.status.total_rounds : (S.detail?.config?.rounds ?? 0));

  // -------------------------------------------------------------- loading
  async function loadAll() {
    try {
      const [detail, posts] = await Promise.all([api.run(runId), api.posts(runId, 0)]);
      if (!S.alive) return;
      ingestDetail(detail);
      ingestPosts(posts);
      try {
        const acts = await api.actions(runId);
        if (!S.alive) return;
        S.actions.clear();
        for (const a of acts) {
          if (!S.actions.has(a.round)) S.actions.set(a.round, []);
          S.actions.get(a.round).push(a);
        }
      } catch {
        /* actions load lazily per round */
      }
      computeLatest();
      const qRound = parseInt(query.round, 10);
      S.round = Number.isFinite(qRound) ? clamp(qRound, minRound(), S.latestRound) : S.latestRound;
      computeCamps();
      S.followLive = isRunning() && S.round >= S.latestRound;
      firstRender();
      if (isRunning()) connect();
    } catch (err) {
      if (!S.alive) return;
      const notFound = err instanceof ApiError && (err.status === 404 || err.status === 400);
      runEl.replaceChildren(el("div", { class: "page" }, stateBlock({
        kind: "error",
        title: notFound ? "Run not found" : "Could not load this run",
        body: notFound ? `There is no run called “${runId}” in this runs folder.` : err.message,
        action: el("div", { style: "display:flex;gap:8px" },
          el("a", { class: "btn", href: "#/" }, "All runs"),
          notFound ? null : el("button", { class: "btn btn-primary", type: "button", onclick: () => route() }, "Retry")),
      })));
    }
  }

  /** Failed batches are kept in status.json (older runs: none) and also arrive live as SSE events. */
  function mergeFailedBatches(list) {
    const key = (b) => `${b.round}:${b.batch}`;
    const seen = new Set(S.failedBatches.map(key));
    let added = false;
    for (const b of list || []) {
      if (!b || typeof b.round !== "number" || seen.has(key(b))) continue;
      seen.add(key(b));
      S.failedBatches.push({ round: b.round, batch: b.batch, agents: b.agents || [], error: b.error || "" });
      added = true;
    }
    return added;
  }

  function ingestDetail(d) {
    const prevPersonaKey = S.personas.map((p) => p.id).join("|");
    S.detail = d;
    S.status = d.status || null;
    mergeFailedBatches(S.status && S.status.failed_batches);
    S.live = !!d.live;
    setPersonas(d.personas || []);
    S.stats = (d.stats || []).slice().sort((a, b) => a.round - b.round);
    S.events = (d.events || []).slice().sort((a, b) => a.round - b.round);
    S.follows = d.follows || [];
    S.report = d.report || null;
    return prevPersonaKey !== S.personas.map((p) => p.id).join("|");
  }

  function ingestPosts(posts) {
    const fresh = [];
    for (const p of posts || []) {
      if (!S.posts.has(p.id)) fresh.push(p);
      S.posts.set(p.id, p);
      if (p.id > S.lastPostId) S.lastPostId = p.id;
    }
    return fresh;
  }

  let refreshing = null;
  let refreshQueued = false;
  function refreshDetail() {
    if (refreshing) {
      refreshQueued = true;
      return refreshing;
    }
    refreshing = (async () => {
      try {
        const d = await api.run(runId);
        if (!S.alive) return;
        const prevReport = JSON.stringify(S.report);
        const personasChanged = ingestDetail(d);
        computeLatest();
        computeCamps();
        renderHead();
        syncNetwork({ full: personasChanged });
        syncChart();
        runLog.render();
        if (personasChanged) {
          feed.setData({ posts: postsList(), personas: S.personas, events: S.events });
          feed.showRound(S.round);
        } else {
          feed.setEvents(S.events);
        }
        if (kgraph) syncGraph();
        if (JSON.stringify(S.report) !== prevReport || !reportRendered) renderReport();
        renderOverview();
        renderDetails();
        updatePanel();
        updateEmptyStates();
      } catch (err) {
        if (S.alive) console.warn("[shoal] refresh failed", err);
      } finally {
        refreshing = null;
        if (refreshQueued && S.alive) {
          refreshQueued = false;
          later(refreshDetail, 300);
        }
      }
    })();
    return refreshing;
  }
  let refreshTimer = null;
  const refreshSoon = (ms = 350) => {
    clearTimeout(refreshTimer);
    refreshTimer = later(refreshDetail, ms);
  };

  let fetchingPosts = null;
  async function fetchNewPosts() {
    if (fetchingPosts) return fetchingPosts;
    fetchingPosts = (async () => {
      try {
        const posts = await api.posts(runId, S.lastPostId);
        if (!S.alive || !posts.length) return [];
        const fresh = ingestPosts(posts);
        if (fresh.length) {
          feed.appendPosts(fresh);
          computeLatest();
          updateEmptyStates();
          renderMetrics();
        }
        return fresh;
      } catch (err) {
        console.warn("[shoal] posts fetch failed", err);
        return [];
      } finally {
        fetchingPosts = null;
      }
    })();
    return fetchingPosts;
  }

  async function actionsFor(r, { force = false } = {}) {
    if (!force && S.actions.has(r)) return S.actions.get(r);
    try {
      const list = await api.actions(runId, r);
      S.actions.set(r, list);
      return list;
    } catch {
      return S.actions.get(r) || [];
    }
  }

  // -------------------------------------------------------------- syncing views
  function netSelect() {
    for (const v of [network, mini]) {
      if (!v) continue;
      v.selectCamp(S.selectedCamp);
      v.select(S.selectedAgent);
    }
  }

  function syncNetwork({ full = false } = {}) {
    const payload = {
      personas: S.personas,
      follows: S.follows,
      sentimentByAgent: sentimentUpTo(S.round),
      orgIds: S.orgIds,
      camps: S.camps,
      hasSentiment: hasPostSentiment(),
      posts: postsList(),
    };
    if (full || S.actions.size) payload.actions = allActions();
    for (const v of [network, mini]) {
      if (!v) continue;
      v.setView(S.graphView);
      v.setData(payload);
      if (S.round !== null) v.setRound(S.round);
    }
    netSelect();
    updateGraphChrome();
  }

  function setGraphView(v) {
    S.graphView = v;
    try {
      localStorage.setItem("shoal.graphView", v);
    } catch {
      /* storage blocked */
    }
    for (const n of [network, mini]) n?.setView(v);
    updateGraphChrome();
  }

  function updateGraphChrome() {
    crowdSeg.sync();
    miniSeg.sync();
    const noSent = S.graphView === "shoal" && !hasPostSentiment();
    const text = noSent ? "This run has no per-agent sentiment, so the crowd is shown by camp." : "";
    netNote.hidden = !text;
    netNote.textContent = text;
    miniNote.hidden = !text;
    miniNote.textContent = text;
    // legend for the current arrangement
    const key = (glyph, label) => el("span", { class: "key" }, glyph, label);
    const line = (color, opts = {}) => {
      const s = svgEl("svg", { width: 22, height: 8, viewBox: "0 0 22 8", "aria-hidden": "true" });
      s.appendChild(svgEl("line", { x1: 1, x2: opts.arrow ? 16 : 21, y1: 4, y2: 4, style: `stroke:${color};stroke-width:${opts.w || 1.5};stroke-linecap:${opts.cap || "butt"};${opts.dash ? `stroke-dasharray:${opts.dash}` : ""}` }));
      if (opts.arrow) s.appendChild(svgEl("path", { d: "M15 1 21 4 15 7z", style: `fill:${color}` }));
      return s;
    };
    const v = S.graphView === "shoal" && !hasPostSentiment() ? "camps" : S.graphView;
    if (v === "camps") {
      legend.replaceChildren(
        key(line("var(--accent)", { arrow: true }), "reply"),
        key(line("var(--violet)", { arrow: true }), "repost"),
        key(line("var(--pos)", { dash: "0.01 4", cap: "round", w: 2 }), "like"),
        key(line("var(--neg)", { dash: "5 3" }), "dislike"),
        key(line("var(--text-faint)", { w: 1 }), "follows"));
    } else if (v === "shoal") {
      legend.replaceChildren(el("span", { text: "Left to right: each person's mean sentiment up to this round. Grey: no score yet." }));
    } else {
      legend.replaceChildren(el("span", { text: "Closer to the centre: more influence. Lines: replies and reposts so far; thicker means more." }));
    }
    campNoteBar.hidden = !S.campsFallback;
    campNoteBar.textContent = S.campsFallback ? (isRunning() ? "Camps appear when the report is written." : "Grouped as organisations and people: this report has no camps.") : "";
  }

  function syncChart() {
    if (!chart) return;
    chart.setData({ stats: S.stats, events: S.events, totalRounds: totalRounds() });
    if (S.round !== null) chart.setRound(S.round);
    renderChartSel();
  }

  function renderChartSel() {
    const r = S.round ?? 0;
    const stat = S.stats.find((x) => x.round === r);
    const evs = S.events.filter((e) => e.round === r);
    const kids = [el("div", {}, el("b", { text: r === 0 ? "Round 0, the opening posts" : `Round ${r}` }),
      stat ? el("span", {}, ": mean ", isScore(stat.sentiment) ? sv(stat.sentiment) : "not scored", `, ${stat.new_posts ?? 0} posts, ${stat.active_agents ?? 0} people active`) : el("span", { text: ": not simulated yet" }))];
    for (const ev of evs) kids.push(el("div", { class: "ev", text: ev.content || "" }));
    if (!evs.length && S.events.length) kids.push(el("div", { class: "dim", text: `Injected event${S.events.length > 1 ? "s" : ""} in round ${S.events.map((e) => e.round).join(", ")}. Click a round to read it.` }));
    chartSel.replaceChildren(...kids);
  }

  function renderGraphMeta() {
    const g = S.detail && S.detail.graph;
    const ents = (g && g.entities) || [];
    const rels = (g && g.relations) || [];
    const played = new Set(S.personas.map((p) => p.entity_id).filter(Boolean));
    const n = ents.filter((e) => played.has(e.id)).length;
    kgMeta.textContent = ents.length ? `${ents.length} entities · ${rels.length} relations · ${n} played by agents` : "No entities";
  }

  function syncGraph() {
    if (!kgraph) return;
    const g = S.detail && S.detail.graph;
    kgraph.setData(g || { entities: [], relations: [] }, S.personas);
    updateEmptyStates();
  }

  function firstRender() {
    document.title = `${(S.detail.config && S.detail.config.question) || runId} · Shoal`;
    renderHead();
    feed.setData({ posts: postsList(), personas: S.personas, events: S.events });
    feed.showRound(S.round);
    syncNetwork({ full: true });
    runLog.render();
    syncChart();
    renderReport();
    renderOverview();
    renderDetails();
    selectTab(S.tab, { focus: false });
    if (query.camp && S.camps.some((c) => c.key === query.camp)) selectCamp(query.camp);
    updatePlayer();
    updatePanel();
    updateEmptyStates();
  }

  // -------------------------------------------------------------- header
  function renderHead() {
    const st = S.status;
    const cfg = (S.detail && S.detail.config) || {};
    const q = cfg.question || runId;
    setCrumbs([{ label: "Runs", href: "#/" }, { label: q, title: q }]);
    const state = st ? st.state : "stopped";
    chipHost.replaceChildren(statusChip(state, { cli: state === "running" && !S.live }));
    const p = provenance(cfg);
    topProv.textContent = p.text;
    topProv.title = p.title;
    const live = isRunning() && S.live;
    stopBtn.hidden = !live;
    injectBtn.hidden = !live || !st || injectMinRound() > st.total_rounds;
    renderMetrics();
    updatePlayer();
  }

  const clock = setInterval(() => { if (isRunning() && S.tab === "details") renderFacts(); }, 1000);

  // -------------------------------------------------------------- empty states
  function updateEmptyStates() {
    const st = S.status;
    const stageName = st ? st.stage : "finished";
    const failed = st && st.state === "failed";
    const early = ["reading", "graph", "personas"].includes(stageName);
    const msg = !S.personas.length ? {
      overlay: true,
      spinner: isRunning(),
      kind: failed ? "error" : "",
      title: failed ? "The run failed before people were cast" : isRunning() ? "The crowd appears once people are cast" : "No people in this run",
      body: failed ? st.error || "" : isRunning() ? `${(STAGES[STAGE_INDEX[stageName] ?? 0] || [])[1] || stageName}. ${st.detail || ""}`.trim() : "",
    } : null;
    netState.replaceChildren(msg ? stateBlock(msg) : "");
    miniState.replaceChildren(msg ? stateBlock(msg) : "");
    zoom.hidden = !!msg;

    renderGraphMeta();
    const g = S.detail && S.detail.graph;
    if (!g || !g.entities || !g.entities.length) {
      kgState.replaceChildren(stateBlock({
        overlay: true,
        spinner: isRunning() && early,
        title: isRunning() && early ? "The knowledge graph appears once the seed documents are read" : "No knowledge graph for this run",
        body: isRunning() && early ? (st.detail || "") : "",
      }));
    } else kgState.replaceChildren();
    feed.setPlaceholder(isRunning() ? "Posts appear here once people start talking." : "This run has no posts.");
  }

  // -------------------------------------------------------------- tabs
  function selectTab(key, { focus = false } = {}) {
    if (!TABS.some(([k]) => k === key)) key = "overview";
    S.tab = key;
    for (const [k, b] of Object.entries(tabBtns)) {
      const on = k === key;
      b.setAttribute("aria-selected", String(on));
      b.tabIndex = on ? 0 : -1;
    }
    paneOverview.hidden = key !== "overview";
    paneCrowd.hidden = key !== "crowd";
    paneReport.hidden = key !== "report";
    paneDetails.hidden = key !== "details";
    if (key === "details") { ensureKg(); renderFacts(); }
    requestAnimationFrame(() => {
      if (key === "crowd" && network) { network.resize(); netSelect(); }
      if (key === "overview" && mini) { mini.resize(); netSelect(); }
      if (key === "details") { kgraph?.resize(); runLog.sync({ scroll: true }); }
    });
    replaceQuery({ tab: key === "overview" ? null : key });
    if (focus) tabBtns[key]?.focus();
  }

  S.applyQuery = (q) => {
    const t = TAB_ALIASES[q.tab] || q.tab || "overview";
    if (t !== S.tab && TABS.some(([k]) => k === t)) selectTab(t);
    const r = parseInt(q.round, 10);
    if (Number.isFinite(r) && r !== S.round) selectRound(r, { animate: false });
  };

  // -------------------------------------------------------------- overview
  function renderOverview() {
    if (!S.detail) return;
    renderSummary();
    renderMetrics();
    renderCamps();
    renderVoices();
    renderChartSel();
    updateGraphChrome();
  }

  function confBadge(conf) {
    const n = conf === "high" ? 3 : conf === "medium" ? 2 : 1;
    const word = conf ? conf[0].toUpperCase() + conf.slice(1) : "Unknown";
    return el("span", { class: "conf", title: "How sure the report is, given how consistent the simulated crowd was" },
      `${word} confidence`, el("span", { class: "bars", "aria-hidden": "true" }, [1, 2, 3].map((i) => el("i", { class: i <= n ? "on" : "" }))));
  }

  function progressFraction(st) {
    if (!st) return 0;
    const total = Math.max(1, st.total_rounds || 1);
    switch (st.stage) {
      case "reading": return 0.04;
      case "graph": return 0.1;
      case "personas": return 0.17;
      case "seeding": return 0.24;
      case "rounds": return 0.26 + 0.64 * clamp((st.round || 0) / total, 0, 1);
      case "report": return 0.93;
      default: return 1;
    }
  }

  function stageTitle(st) {
    if (!st) return "Starting";
    if (st.stage === "rounds") {
      const cur = Math.min((st.round || 0) + 1, st.total_rounds);
      return `Simulating round ${cur} of ${st.total_rounds}`;
    }
    return (STAGES[STAGE_INDEX[st.stage] ?? -1] || [null, st.state === "queued" ? "Waiting to start" : "Starting"])[1];
  }

  function stepsList() {
    const st = S.status;
    const state = st ? st.state : "done";
    const idx = st ? (st.stage === "finished" ? STAGES.length : STAGE_INDEX[st.stage] ?? 0) : STAGES.length;
    return el("ol", { class: "steps", "aria-label": "Pipeline" }, STAGES.map(([key, label], i) => {
      let s = i < idx ? "done" : i === idx ? "current" : "pending";
      if (i === idx && state === "failed") s = "failed";
      if (i === idx && state === "stopped") s = "halted";
      if (state === "done") s = "done";
      const short = { reading: "Read", graph: "Map", personas: "Cast", seeding: "Seed", rounds: "Rounds", report: "Report" }[key];
      return el("li", { "data-s": s, "aria-current": s === "current" ? "step" : undefined, title: label },
        el("span", { class: "ck", "aria-hidden": "true" }), short,
        key === "rounds" && st ? el("span", { class: "sub", text: `${roundsDone()}/${st.total_rounds}` }) : null);
    }));
  }

  function renderSummary() {
    const st = S.status;
    const cfg = S.detail.config || {};
    const rep = S.report;
    const state = st ? st.state : "done";
    const question = el("p", { class: "sum-q", text: cfg.question || "" });
    if (isRunning()) {
      summaryCard.className = "card summary-card progress-card";
      const pct = Math.round(progressFraction(st) * 100);
      summaryCard.replaceChildren(
        el("div", { class: "sum-top" }, statusChip(state, { cli: !S.live }), el("span", { class: "lbl", text: `${pct}%` })),
        question,
        el("h1", { class: "stage-title", text: stageTitle(st) }),
        el("div", { class: "pbar", role: "progressbar", "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(pct), "aria-label": "Run progress" },
          el("i", { style: `width:${pct}%` })),
        el("div", { class: "stage-detail" }, el("span", { class: "spinner", "aria-hidden": "true" }), el("span", { text: st.detail || "Working" })),
        el("p", { class: "wait", text: "The answer appears here when the run finishes. You can watch the crowd in the meantime." }),
        stepsList(),
        el("div", { class: "sum-actions" },
          el("button", { class: "btn", type: "button", onclick: () => selectTab("crowd", { focus: true }) }, "Watch the crowd"),
          S.live ? el("button", { class: "btn btn-danger", type: "button", onclick: () => stopRun() }, icon("stop", "icon sm"), "Stop run")
            : el("span", { class: "note", text: "Started from the command line: stop it there." })));
      return;
    }
    summaryCard.className = "card summary-card";
    const kids = [el("div", { class: "sum-top" }, statusChip(state), rep ? confBadge(rep.confidence) : null), question];
    if (state === "failed" || state === "stopped") {
      const r = roundsDone();
      const sentence = state === "failed"
        ? `The run failed after ${r} of ${st.total_rounds} rounds${st.stage && st.stage !== "rounds" ? `, while ${(STAGES[STAGE_INDEX[st.stage] ?? 0] || [])[1]?.toLowerCase() || st.stage}` : ""}.`
        : `Stopped after round ${r} of ${st.total_rounds}. ${rep ? "The report covers the rounds that ran." : ""}`;
      kids.push(el("div", { class: "sum-status", "data-state": state, role: state === "failed" ? "alert" : undefined },
        icon(state === "failed" ? "alert" : "stop"),
        el("div", {}, el("b", { text: sentence }), state === "failed" && st.error ? el("div", { class: "err-text", text: st.error }) : null)));
    }
    if (rep) {
      kids.push(inline(rep.title || "Report", el("h1", { class: "verdict" })));
      const reasons = Array.isArray(rep.reasons) ? rep.reasons.filter(Boolean) : [];
      if (reasons.length) kids.push(el("ul", { class: "reasons", "aria-label": "Why" }, reasons.map((t) => el("li", {}, inline(t, el("span"))))));
      else if (rep.summary) kids.push(inline(rep.summary, el("p", { class: "sum-lead" })));
      kids.push(el("div", { class: "sum-actions" },
        el("button", { class: "btn btn-primary", type: "button", onclick: () => selectTab("report", { focus: true }) }, icon("doc", "icon sm"), "Read full report"),
        el("button", { class: "btn", type: "button", onclick: () => { selectTab("report"); requestAnimationFrame(() => reportRoot.querySelector(".ask input")?.focus()); } }, icon("chat", "icon sm"), "Ask about it")));
    } else {
      kids.push(el("h1", { class: "verdict", text: state === "failed" ? "No answer: the run ended before its report." : "No report yet" }));
      kids.push(el("p", { class: "sum-lead", text: S.stats.length ? "A report can still be written from the rounds that were recorded." : "No rounds were recorded, so there is nothing to report on." }));
      if (S.stats.length) kids.push(el("div", { class: "sum-actions" }, rebuildButton("Build report")));
    }
    summaryCard.replaceChildren(...kids);
  }

  function rebuildButton(label = "Rebuild report") {
    return el("button", { class: "btn", type: "button", disabled: rebuilding || isRunning() || undefined,
      title: isRunning() ? "Available once the run has finished" : "Write the report again from the stored run (calls the planner model)",
      onclick: () => rebuildReport() }, rebuilding ? el("span", { class: "spinner", "aria-hidden": "true" }) : icon("refresh", "icon sm"), rebuilding ? "Rebuilding…" : label);
  }

  function renderMetrics() {
    if (!S.detail) return;
    const n = S.personas.length || S.detail.config?.num_agents || 0;
    const orgs = S.personas.filter((p) => isOrg(p.id)).length;
    const total = totalRounds();
    const done = isRunning() ? Math.min(S.status.round || 0, total) : roundsDone();
    const posts = S.posts.size;
    const laterPosts = [...S.posts.values()].filter((p) => p.round >= 1).length;
    const scored = S.stats.filter((s) => isScore(s.sentiment));
    const last = scored[scored.length - 1];
    const first = scored[0];
    const tile = (label, value, sub, extra = {}) => el("div", { class: "card metric", ...extra },
      el("span", { class: "l", text: label }), value, el("span", { class: "s" }, sub));
    const evRounds = S.events.map((e) => `r${e.round}`);
    metricsRow.replaceChildren(
      tile("People", el("span", { class: "v", text: String(n) }),
        S.personas.length ? (orgs ? `${n - orgs} ${n - orgs === 1 ? "person" : "people"} · ${orgs} organisation${orgs === 1 ? "" : "s"}` : "All individual people") : "Not cast yet"),
      tile("Rounds", el("span", { class: "v" }, String(done), done < total ? el("span", { class: "of", text: ` / ${total}` }) : null),
        isRunning() ? `Round ${Math.min(done + 1, total)} in progress` : `r0 opening posts${evRounds.length ? `, event at ${evRounds.join(", ")}` : ""}`),
      tile("Posts", el("span", { class: "v", text: String(posts) }),
        posts ? (done ? `${(laterPosts / done).toFixed(1).replace(/\.0$/, "")} per round after the opening posts` : "Opening posts only") : "None yet"),
      tile(isRunning() ? "Sentiment now" : "Final sentiment",
        last ? el("span", { class: "v sv", "data-s": sentimentSign(last.sentiment), text: formatScore(last.sentiment) }) : el("span", { class: "v", text: "n/a" }),
        last && first && last !== first ? el("span", {}, sv(last.sentiment - first.sentiment), ` since r${first.round}`) : "Mean of scored posts, −1 to +1"));
  }

  function renderCamps() {
    const head = el("div", { class: "card-head" }, el("h2", { id: "ov-camps-h", text: "Camps" }),
      el("span", { class: "sub", text: S.campsFallback ? "Organisations and people" : `${S.camps.length} groups from the report` }));
    const kids = [head];
    if (S.campsFallback && S.personas.length) {
      if (isRunning()) kids.push(el("div", { class: "camps-note" }, el("p", { text: "Camps appear when the report is written. Until then the crowd is grouped as organisations and people." })));
      else kids.push(el("div", { class: "camps-note" },
        el("p", { text: S.report ? "This run was reported before camps existed. Rebuild the report to group the crowd by opinion." : "This run has no report, so the crowd is grouped as organisations and people." }),
        S.report || S.stats.length ? rebuildButton() : null));
    }
    if (!S.personas.length) kids.push(el("p", { class: "empty-note", text: isRunning() ? "The crowd appears once people are cast." : "No people in this run." }));
    const scored = hasPostSentiment();
    kids.push(el("div", { class: "camp-rows" }, S.camps.map((c) => {
      const shown = c.members.slice(0, 6);
      return el("button", { class: "camp-row", type: "button", "aria-label": `${c.name}, ${c.members.length} people${isScore(c.sentiment) ? `, mean sentiment ${formatScore(c.sentiment)}` : ""}. Open in Crowd.`,
        onclick: () => selectCamp(c.key, { tab: true }) },
        campDot(c.color),
        el("span", { class: "name", text: c.name }),
        el("span", { class: "av-stack", "aria-hidden": "true" }, shown.map((id) => av(id, 24)),
          c.members.length > shown.length ? el("span", { class: "more", text: `+${c.members.length - shown.length}` }) : null),
        el("span", { class: "count", text: `${c.members.length}` }),
        isScore(c.sentiment) ? sv(c.sentiment) : el("span", { class: "sv", text: scored ? "n/a" : "" }),
        el("span", { class: "summ", text: plain(c.summary) }));
    })));
    campsCard.replaceChildren(...kids);
  }

  /** The quote that stands for an agent: their most engaged post with the strongest score. */
  function quoteFor(id) {
    const ps = postsList().filter((p) => p.author_id === id && (p.content || "").trim());
    if (!ps.length) return null;
    const score = (p) => (p.likes || 0) + (p.replies || 0) * 1.5 + (p.reposts || 0) * 2 + (isScore(p.sentiment) ? Math.abs(p.sentiment) : 0);
    return ps.sort((a, b) => score(b) - score(a) || b.id - a.id)[0];
  }

  function renderVoices() {
    const rep = S.report;
    let picks = [];
    let sub = "";
    if (rep && rep.key_voices && rep.key_voices.length) {
      picks = rep.key_voices.filter((id) => S.personaById.has(id)).slice(0, 5).map((id) => ({ id, post: quoteFor(id) })).filter((x) => x.post);
      sub = "Named by the report";
    }
    if (!picks.length) {
      const seen = new Set();
      const eng = (p) => (p.likes || 0) + (p.replies || 0) * 1.5 + (p.reposts || 0) * 2;
      for (const p of postsList().filter((x) => (x.content || "").trim()).sort((a, b) => eng(b) - eng(a) || a.id - b.id)) {
        if (seen.has(p.author_id)) continue;
        seen.add(p.author_id);
        picks.push({ id: p.author_id, post: p });
        if (picks.length >= 4) break;
      }
      sub = picks.length ? "Most engaged posts so far" : "";
    }
    const kids = [el("div", { class: "card-head" }, el("h2", { id: "ov-voices-h", text: "Key voices" }), el("span", { class: "sub", text: sub }))];
    if (!picks.length) kids.push(el("p", { class: "empty-note", text: isRunning() ? "Quotes appear once people start posting." : "No posts in this run." }));
    else kids.push(el("div", { class: "voices" }, picks.map(({ id, post }) => {
      const p = persona(id);
      const camp = S.campOf.get(id);
      return el("article", { class: "voice", "aria-label": `${p ? p.name : id}, post #${post.id}` },
        el("div", { class: "voice-who" }, av(id, 32),
          el("button", { class: "nm", type: "button", title: `Open ${p ? p.name : id}`, onclick: () => openAgent(id) },
            el("b", { text: p ? p.name : id }), el("span", { text: p ? `@${p.handle}` : "" })),
          el("button", { class: "cite-pill ref", type: "button", "data-post": post.id, title: `Show post #${post.id}`, onclick: () => selectPost(post.id) }, `#${post.id}`)),
        el("blockquote", { text: post.content }),
        el("div", { class: "voice-meta" },
          camp ? el("span", {}, campDot(camp.color), " ", camp.name) : null,
          el("span", { text: `Round ${post.round}` }),
          isScore(post.sentiment) ? sv(post.sentiment) : null));
    })));
    voicesCard.replaceChildren(...kids);
  }

  // -------------------------------------------------------------- crowd panel
  function updatePanel() {
    const agentOpen = drawer.isOpen;
    insp.hidden = !agentOpen;
    const camp = !agentOpen && S.selectedCamp ? S.camps.find((c) => c.key === S.selectedCamp) : null;
    campView.hidden = !camp;
    feedView.hidden = agentOpen || !!camp;
    if (camp) renderCampPanel(camp);
    else if (!agentOpen) renderFeedHead();
  }

  function renderFeedHead() {
    const r = S.round ?? 0;
    const cnt = postsOfRound(r).length;
    panelTitle.textContent = S.peopleMode === "people" ? "People" : r === 0 ? "Round 0" : `Round ${r}`;
    panelSub.textContent = S.peopleMode === "people" ? `${S.personas.length}` : `${cnt} post${cnt === 1 ? "" : "s"}`;
    for (const b of panelSeg.querySelectorAll("button")) b.setAttribute("aria-pressed", String(b.dataset.mode === S.peopleMode));
    feedHost.hidden = S.peopleMode !== "posts";
    peopleHost.hidden = S.peopleMode !== "people";
    if (S.peopleMode === "people") renderPeople();
  }

  function setPeopleMode(m) {
    S.peopleMode = m;
    renderFeedHead();
  }

  function memberButton(id, sent) {
    const p = persona(id);
    return el("button", { class: "member", type: "button", "data-agent": id, "aria-current": String(id === S.selectedAgent),
      "aria-label": `${p ? p.name : id}, @${p ? p.handle : id}${isScore(sent[id]) ? `, sentiment ${formatScore(sent[id])}` : ""}, influence ${(Number(p?.influence) || 0).toFixed(2)}`,
      onclick: () => openAgent(id) },
      av(id, 28),
      el("span", { class: "nm" }, el("b", { text: p ? p.name : id }), el("span", { text: p ? `@${p.handle}` : "" })),
      isScore(sent[id]) ? sv(sent[id]) : el("span", { class: "sv", text: "" }));
  }

  let peopleFilter = "";
  function renderPeople() {
    const sent = sentimentUpTo(S.round);
    const input = el("input", { class: "input", type: "search", placeholder: "Filter by name or handle", "aria-label": "Filter people", value: peopleFilter });
    input.addEventListener("input", () => {
      peopleFilter = input.value;
      const pos = input.selectionStart;
      renderPeople();
      const again = peopleHost.querySelector("input");
      again?.focus();
      again?.setSelectionRange(pos, pos);
    });
    const q = peopleFilter.trim().toLowerCase();
    const groups = S.camps.map((c) => {
      const ids = c.members.filter((id) => {
        const p = persona(id);
        return !q || (p && (p.handle.toLowerCase().includes(q) || (p.name || "").toLowerCase().includes(q)));
      }).sort((a, b) => (persona(b)?.influence ?? 0) - (persona(a)?.influence ?? 0));
      if (!ids.length) return null;
      return el("section", { class: "people-group", "aria-label": c.name },
        el("h3", {}, campDot(c.color), c.name, el("span", { class: "n", text: String(ids.length) })),
        el("div", { class: "member-list" }, ids.map((id) => memberButton(id, sent))));
    }).filter(Boolean);
    peopleHost.replaceChildren(el("div", { class: "people-filter" }, input),
      ...(groups.length ? groups : [el("p", { class: "panel-note", text: S.personas.length ? "Nobody matches." : "People appear once they are cast." })]));
  }

  function renderCampPanel(c) {
    const sent = sentimentUpTo(S.round);
    const r = S.round ?? 0;
    const members = c.members.slice().sort((a, b) => (persona(b)?.influence ?? 0) - (persona(a)?.influence ?? 0));
    const memberSet = new Set(c.members);
    const posts = postsList().filter((p) => memberSet.has(p.author_id));
    campFeed.setData({ posts, personas: S.personas, events: [] });
    campFeed.setPlaceholder("Nobody in this camp has posted yet.");
    campFeed.showRound(r);
    campView.replaceChildren(
      el("div", { class: "camp-head" },
        el("div", { class: "row" },
          el("button", { class: "btn btn-sm btn-ghost", type: "button", onclick: () => selectCamp(null) }, icon("back", "icon sm"), "All posts"),
          el("span", { class: "spacer" }),
          el("button", { class: "icon-btn", type: "button", "aria-label": "Close camp", title: "Close (Esc)", onclick: () => selectCamp(null) }, icon("x"))),
        el("div", { class: "row" }, campDot(c.color), el("h2", { text: c.name })),
        el("div", { class: "row note" }, `${c.members.length} ${c.members.length === 1 ? "person" : "people"}`,
          isScore(c.sentiment) ? el("span", {}, ` · mean${S.round !== null && S.round < S.latestRound ? ` up to round ${S.round}` : ""} `, sv(c.sentiment)) : null),
        c.summary ? inline(c.summary, el("p")) : null),
      el("div", { class: "panel-sec" }, el("h3", { text: "Members" }), el("div", { class: "member-list" }, members.map((id) => memberButton(id, sent)))),
      el("div", { class: "panel-sec" }, el("h3", { text: r === 0 ? "Their posts in round 0" : `Their posts in round ${r}` })),
      campFeedHost);
  }

  // -------------------------------------------------------------- report
  let reportRendered = false;
  let rebuilding = false;

  function linkify(text, parent) {
    const re = /(?<![\w&#])#(\d+)\b|(?<![\w@./])@([A-Za-z0-9_]{1,40})|\b(a\d{2,3})\b/g;
    let last = 0;
    let m;
    while ((m = re.exec(text))) {
      if (m.index > last) parent.appendChild(document.createTextNode(text.slice(last, m.index)));
      if (m[1]) {
        const pid = Number(m[1]);
        if (S.posts.has(pid)) parent.appendChild(el("button", { class: "ref", type: "button", "data-post": pid, text: `#${pid}`, "aria-label": `Show post #${pid}`, onclick: () => selectPost(pid) }));
        else parent.appendChild(document.createTextNode(m[0]));
      } else if (m[2]) {
        const p = S.personaByHandle.get(m[2].toLowerCase());
        if (p) parent.appendChild(el("button", { class: "ref", type: "button", "data-agent": p.id, text: `@${m[2]}`, "aria-label": `Open ${p.name}, @${p.handle}`, onclick: () => openAgent(p.id) }));
        else parent.appendChild(document.createTextNode(m[0]));
      } else if (m[3]) {
        const p = S.personaById.get(m[3]);
        if (p) parent.appendChild(el("button", { class: "ref", type: "button", "data-agent": p.id, text: `@${p.handle}`, "aria-label": `Open ${p.name}, @${p.handle}`, onclick: () => openAgent(p.id) }));
        else parent.appendChild(document.createTextNode(m[0]));
      }
      last = re.lastIndex;
      // keep a possessive with its citation so "'s" never wraps onto a line of its own
      const tail = /^['\u2019]s\b/.exec(text.slice(last));
      if (tail && parent.lastChild && parent.lastChild.nodeName === "BUTTON") {
        const wrap = document.createElement("span");
        wrap.className = "ref-keep";
        wrap.append(parent.lastChild, tail[0]);
        parent.appendChild(wrap);
        last += tail[0].length;
        re.lastIndex = last;
      }
    }
    if (last < text.length) parent.appendChild(document.createTextNode(text.slice(last)));
    return parent;
  }

  /** Inline Markdown subset: `code`, **bold**, *italic*, then #post / @handle links. */
  function inline(text, parent = document.createDocumentFragment()) {
    for (const part of String(text).split(/(`[^`\n]+`)/)) {
      if (part.length > 1 && part.startsWith("`") && part.endsWith("`")) {
        parent.appendChild(el("code", { text: part.slice(1, -1) }));
        continue;
      }
      const bolds = part.split(/\*\*(.+?)\*\*/);
      bolds.forEach((seg, i) => {
        if (i % 2 === 1) {
          parent.appendChild(linkify(seg, el("strong")));
          return;
        }
        const ital = seg.split(/(?<![*\w])\*(?=\S)(.+?)(?<=\S)\*(?![*\w])/);
        ital.forEach((s2, j) => {
          if (j % 2 === 1) parent.appendChild(linkify(s2, el("em")));
          else linkify(s2, parent);
        });
      });
    }
    return parent;
  }

  /** Block Markdown subset: paragraphs, headings, bullet / numbered lists, quotes. */
  function markdown(text) {
    const root = el("div", { class: "md" });
    let para = [];
    let items = [];
    let listTag = "";
    const flush = () => {
      if (para.length) {
        root.appendChild(inline(para.join(" "), el("p")));
        para = [];
      }
      if (items.length) {
        const list = el(listTag);
        for (const it of items) list.appendChild(inline(it, el("li")));
        root.appendChild(list);
        items = [];
        listTag = "";
      }
    };
    for (const raw of String(text || "").split(/\r?\n/)) {
      const line = raw.trim();
      const bullet = /^[-*•]\s+(.*)$/.exec(line);
      const numbered = /^\d+[.)]\s+(.*)$/.exec(line);
      const heading = /^#{1,6}\s+(.*)$/.exec(line);
      if (!line) flush();
      else if (heading) {
        flush();
        root.appendChild(inline(heading[1], el("h4")));
      } else if (bullet || numbered) {
        const tag = bullet ? "ul" : "ol";
        if (para.length || (items.length && listTag !== tag)) flush();
        listTag = tag;
        items.push((bullet || numbered)[1]);
      } else if (line.startsWith(">")) {
        flush();
        root.appendChild(inline(line.replace(/^>\s?/, ""), el("blockquote")));
      } else if (items.length) items[items.length - 1] += " " + line;
      else para.push(line);
    }
    flush();
    return root;
  }

  function renderReport() {
    reportRendered = true;
    const rep = S.report;
    const st = S.status;

    if (!rep) {
      let titleText = "No report yet";
      let body = "";
      if (isRunning()) {
        titleText = st.stage === "report" ? "Writing the report" : "The report is written after the last round";
        body = st.stage === "report" ? "The planner model is reading the evidence. This usually takes under a minute." : `Round ${Math.min(st.round, st.total_rounds)} of ${st.total_rounds} complete.`;
      } else if (st && st.state === "failed") {
        body = "The run failed before a report was written. If some rounds completed, you can still build one from them.";
      } else {
        body = "This run has no report. Build one from the rounds that were recorded.";
      }
      reportRoot.replaceChildren(stateBlock({
        title: titleText, body, spinner: isRunning() && st.stage === "report",
        action: isRunning() ? null : rebuildButton("Build report"),
      }));
      return;
    }

    const head = el("header", { class: "report-head" },
      inline(rep.title || "Report", el("h2")),
      el("div", { class: "meta" },
        confBadge(rep.confidence),
        el("span", { class: "spacer", style: "flex:1" }),
        rebuildButton(),
        el("a", { class: "btn", href: api.reportUrl(runId), target: "_blank", rel: "noopener" }, icon("external", "icon sm"), "Report page")),
      el("p", { class: "note", text: "A scenario exploration of a simulated crowd, not a poll or measurement of real people." }));

    const parts = [head, inline(rep.summary || "", el("p", { class: "lead" }))];
    parts.push(el("div", { class: "prediction" }, el("h3", { text: "Prediction" }), inline(rep.prediction || "", el("p"))));
    if (Array.isArray(rep.reasons) && rep.reasons.length) {
      parts.push(el("section", {}, el("h3", { text: "Why" }), el("div", { class: "md" }, el("ul", {}, rep.reasons.map((t) => inline(t, el("li")))))));
    }

    for (const sec of rep.sections || []) {
      parts.push(el("section", {}, el("h3", { text: sec.title }), markdown(sec.body_markdown)));
    }

    if (!S.campsFallback && S.camps.length) {
      parts.push(el("section", { class: "minor" }, el("h3", { text: "Camps" }),
        el("div", { class: "rep-camps" }, S.camps.map((c) => el("div", { class: "rep-camp" },
          campDot(c.color),
          el("b", {}, el("button", { class: "linkish", type: "button", text: c.name, onclick: () => selectCamp(c.key, { tab: true }) })),
          el("span", { class: "note" }, `${c.members.length} · `, isScore(c.reported ?? postMean(c.members, null)) ? sv(c.reported ?? postMean(c.members, null)) : "n/a"),
          c.summary ? inline(c.summary, el("p")) : null)))));
    }

    if (rep.key_voices && rep.key_voices.length) {
      const sentAll = sentimentUpTo(null);
      parts.push(el("section", { class: "minor" }, el("h3", { text: "Key voices" }),
        el("div", { class: "rep-voices" }, rep.key_voices.map((id) => {
          const p = S.personaById.get(id);
          if (!p) return el("span", { class: "mono dim", text: id });
          return el("button", { class: "chip ref", type: "button", "data-agent": id, "aria-label": `Open ${p.name}, @${p.handle}`, onclick: () => openAgent(id) },
            av(id, 20), `@${p.handle}`, isScore(sentAll[id]) ? sv(sentAll[id]) : null);
        }))));
    }

    const listSection = (label, items) => el("section", { class: "minor" }, el("h3", { text: label }),
      items && items.length ? el("ul", { class: "md", style: "padding-left:20px;margin:0" }, items.map((t) => inline(t, el("li")))) : el("p", { class: "dim", text: "None listed." }));
    parts.push(el("div", { class: "two-col" }, listSection("Risks", rep.risks), listSection("Caveats", rep.caveats)));

    parts.push(buildAsk());
    reportRoot.replaceChildren(...parts);
  }

  function buildAsk() {
    const input = el("input", { class: "input", placeholder: "Ask about this outcome, e.g. Which camp grew fastest after round 3?", "aria-label": "Question about the report", maxlength: "2000" });
    const btn = el("button", { class: "btn btn-primary", type: "submit" }, "Ask");
    const log = el("div", { "aria-live": "polite", style: "display:grid;gap:12px" });
    const renderLog = () => {
      log.replaceChildren(...S.qa.map((x) => el("div", { class: "qa" },
        el("div", { class: "q", text: x.q }),
        x.pending ? el("div", { class: "typing", "aria-label": "Waiting for the answer" }, el("i"), el("i"), el("i"))
          : x.error ? el("div", { class: "err", text: x.error })
            : inline(x.a, el("div", { class: "a" })))));
    };
    const form = el("form", {}, input, btn);
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const q = input.value.trim();
      if (!q) return;
      const entry = { q, a: "", pending: true };
      S.qa.push(entry);
      input.value = "";
      btn.disabled = true;
      renderLog();
      try {
        const res = await api.ask(runId, q);
        entry.a = (res && res.reply) || "(no answer)";
      } catch (err) {
        entry.error = err instanceof ApiError && err.status === 409 ? "The report is not available yet." : `Could not get an answer: ${err.message}`;
      }
      entry.pending = false;
      btn.disabled = false;
      if (S.alive) renderLog();
    });
    renderLog();
    return el("section", { class: "ask" },
      el("h3", { text: "Ask about this outcome" }),
      el("p", { class: "note", text: "Answered by the report analyst from the report and the run's evidence. Post numbers and handles in answers link back to the run." }),
      form, log);
  }

  async function rebuildReport() {
    if (rebuilding || isRunning()) return;
    rebuilding = true;
    renderReport();
    renderOverview();
    renderDetails();
    const before = JSON.stringify(S.report);
    try {
      await api.rebuildReport(runId);
      toast("info", "Rebuilding the report", "It is written in the background; this run updates when it is done.");
    } catch (err) {
      rebuilding = false;
      renderReport();
      renderOverview();
      renderDetails();
      toast("error", "Could not rebuild the report", err.message);
      return;
    }
    const done = (d, msg) => {
      rebuilding = false;
      ingestDetail(d);
      computeCamps();
      renderHead();
      renderReport();
      renderOverview();
      renderDetails();
      syncNetwork({ full: false });
      updatePanel();
      msg();
    };
    const deadline = Date.now() + 6 * 60 * 1000;
    const poll = async () => {
      if (!S.alive) return;
      try {
        const d = await api.run(runId);
        if (!S.alive) return;
        const rb = d.report_rebuild || null;
        if (rb && !rb.running) {
          done(d, () => {
            if (rb.error) toast("error", "The report rebuild failed", rb.error);
            else if (d.report && JSON.stringify(d.report) === before) toast("ok", "Report rebuilt", "The new report is the same as the old one.");
            else toast("ok", "Report rebuilt", (d.report && d.report.title) || "");
          });
          return;
        }
        if (!rb && JSON.stringify(d.report || null) !== before && d.report) {
          done(d, () => toast("ok", "Report rebuilt", d.report.title || ""));
          return;
        }
      } catch {
        /* keep polling */
      }
      if (Date.now() > deadline) {
        rebuilding = false;
        renderReport();
        renderOverview();
        toast("warn", "Still no new report", "The rebuild is taking long. Check the terminal running `shoal ui`.");
        return;
      }
      later(poll, 3000);
    };
    later(poll, 2500);
  }

  // -------------------------------------------------------------- details
  function renderDetails() {
    if (!S.detail) return;
    renderPipeline();
    renderFacts();
    renderRoundsTable();
    renderFailures();
  }

  function renderPipeline() {
    const st = S.status;
    const state = st ? st.state : "done";
    const idx = st ? (st.stage === "finished" ? STAGES.length : STAGE_INDEX[st.stage] ?? 0) : STAGES.length;
    const secs = (pred) => S.stats.filter(pred).reduce((a, s) => a + (Number(s.llm_seconds) || 0), 0);
    const timing = {
      seeding: () => { const v = secs((s) => s.round === 0); return v ? `${fmtDuration(v * 1000)} model time` : ""; },
      rounds: () => { const v = secs((s) => s.round > 0); const n = S.stats.filter((s) => s.round > 0).length; return n ? `${n} of ${st ? st.total_rounds : n} · ${fmtDuration(v * 1000)} model time` : ""; },
      report: () => (S.report ? "written" : ""),
    };
    pipeCard.replaceChildren(
      el("div", { class: "card-head" }, el("h2", { id: "det-pipe-h", text: "Pipeline" }), statusChip(state, { cli: state === "running" && !S.live })),
      st && st.detail ? el("p", { class: "note", text: state === "failed" && st.error ? st.error : st.detail }) : null,
      el("ol", { class: "stage-list" }, STAGES.map(([key, label], i) => {
        let s = i < idx ? "done" : i === idx ? "current" : "pending";
        if (i === idx && state === "failed") s = "failed";
        if (i === idx && state === "stopped") s = "halted";
        if (state === "done") s = "done";
        const word = { done: "", current: "in progress", failed: "failed here", halted: "stopped here", pending: "" }[s];
        const t = timing[key]?.() || "";
        return el("li", { "data-s": s, "aria-current": s === "current" ? "step" : undefined },
          el("span", { class: "ck", "aria-hidden": "true" }), el("span", { text: label }),
          el("span", { class: "t", text: [t, word].filter(Boolean).join(" · ") }));
      })));
  }

  function renderFacts() {
    if (!S.detail) return;
    const st = S.status;
    const cfg = S.detail.config || {};
    const calls = S.detail.calls || {};
    const started = parseTime(st && st.started_at);
    const ended = isRunning() ? Date.now() : parseTime(st && st.updated_at);
    const p = provenance(cfg);
    const row = (k, v, cls) => [el("dt", { text: k }), el("dd", { class: cls, text: v })];
    const live = isRunning() && S.live;
    factsCard.replaceChildren(
      el("div", { class: "card-head" }, el("h2", { id: "det-run-h", text: "Run" })),
      el("dl", { class: "facts-dl" },
        row("Ran on", p.text, "mono"),
        row("Run", runId, "mono"),
        started ? row("Started", absTime(st.started_at)) : null,
        started ? row(isRunning() ? "Elapsed" : "Took", fmtDuration((ended || Date.now()) - started)) : null,
        row("Model calls", `${calls.count ?? 0}${calls.failed ? ` (${calls.failed} failed)` : ""}`),
        row("Model time", fmtDuration(Number(calls.seconds ?? 0) * 1000)),
        row("People", `${S.personas.length} cast, ${S.follows.length} follows`),
        row("Posts", String(S.posts.size)),
        row("Planned events", (cfg.events || []).length ? (cfg.events || []).map((e) => `r${e.round}`).join(", ") : "none")),
      el("div", { class: "det-actions" },
        live ? el("button", { class: "btn btn-event", type: "button", onclick: () => openInjectDialog() }, icon("event", "icon sm"), "Inject event") : null,
        rebuildButton(S.report ? "Rebuild report" : "Build report"),
        S.report ? el("a", { class: "btn", href: api.reportUrl(runId), target: "_blank", rel: "noopener" }, icon("external", "icon sm"), "Report page") : null,
        live ? el("button", { class: "btn btn-danger", type: "button", onclick: () => stopRun() }, icon("stop", "icon sm"), "Stop run") : null));
  }

  function renderRoundsTable() {
    const total = totalRounds();
    const evR = new Set(S.events.map((e) => e.round));
    const rows = S.stats.map((s) => {
      const acts = s.actions || {};
      const nActs = Object.entries(acts).filter(([k]) => k !== "nothing").reduce((a, [, v]) => a + v, 0);
      return el("tr", { class: evR.has(s.round) ? "ev" : undefined },
        el("td", { text: `r${s.round}` }),
        el("td", { text: String(s.active_agents ?? 0) }),
        el("td", { text: String(s.new_posts ?? 0) }),
        el("td", { title: Object.entries(acts).map(([k, v]) => `${v} ${k}`).join(", "), text: String(nActs) }),
        el("td", {}, isScore(s.sentiment) ? sv(s.sentiment) : "n/a"),
        el("td", { text: `${Number(s.llm_seconds || 0).toFixed(0)}s` }));
    });
    roundsCard.replaceChildren(
      el("div", { class: "card-head" }, el("h2", { id: "det-rounds-h", text: "Rounds" }), el("span", { class: "sub", text: `${S.stats.length} of ${total + 1} recorded, r0 is the opening posts` })),
      rows.length ? el("div", { style: "overflow-x:auto" }, el("table", { class: "tbl" },
        el("thead", {}, el("tr", {}, ["Round", "Active", "Posts", "Actions", "Mean sentiment", "Model time"].map((t) => el("th", { scope: "col", text: t })))),
        el("tbody", {}, rows))) : el("p", { class: "empty-note", text: "No rounds recorded yet." }));
  }

  function renderFailures() {
    failCard.hidden = !S.failedBatches.length;
    if (!S.failedBatches.length) return;
    failCard.replaceChildren(
      el("div", { class: "card-head" }, el("h2", { id: "det-fail-h", text: "Failed batches" }), el("span", { class: "sub", text: "Those agents sat the round out; the run carried on." })),
      el("ul", { class: "fail-list" }, S.failedBatches.map((b) => el("li", {},
        el("div", { text: `Round ${b.round}, batch ${b.batch ?? "?"}: ${(b.agents || []).length} agent${(b.agents || []).length === 1 ? "" : "s"} skipped` }),
        b.error ? el("div", { class: "err-text", text: b.error }) : null))));
  }

  // -------------------------------------------------------------- rounds & replay
  function renderScrubber() {
    const total = totalRounds();
    const statBy = new Map(S.stats.map((s) => [s.round, s]));
    const evR = new Set(S.events.map((e) => e.round));
    const kids = [];
    for (let r = 0; r <= total; r++) {
      const s = statBy.get(r);
      const future = r > S.latestRound;
      kids.push(el("button", {
        class: "rbtn", type: "button", "data-round": r, "aria-current": String(r === S.round), "data-future": String(future),
        disabled: future || undefined,
        "aria-label": `Round ${r}${s && isScore(s.sentiment) ? `, mean sentiment ${formatScore(s.sentiment)}` : ""}${evR.has(r) ? ", event" : ""}${future ? ", not simulated yet" : ""}`,
        onclick: () => { pause(); S.followLive = isRunning() && r >= S.latestRound; selectRound(r, { animate: true }); },
      }, `r${r}`, el("span", { class: "v", "data-s": s && isScore(s.sentiment) ? sentimentSign(s.sentiment) : "neu", text: s && isScore(s.sentiment) ? formatScore(s.sentiment) : future ? "" : "·" }),
      evR.has(r) ? el("span", { class: "evm", title: "Injected event", "aria-hidden": "true" }) : null));
    }
    roundsTrack.replaceChildren(...kids);
    roundsTrack.querySelector('[aria-current="true"]')?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }

  function updatePlayer() {
    const total = totalRounds();
    const r = S.round ?? 0;
    whereLbl.textContent = r === 0 ? `Round 0 of ${total}` : `Round ${r} of ${total}`;
    playBtn.replaceChildren(icon(S.playing ? "pause" : "play"));
    playBtn.setAttribute("aria-pressed", String(S.playing));
    playBtn.setAttribute("aria-label", S.playing ? "Pause replay" : "Play replay");
    playBtn.title = S.playing ? "Pause replay (Space)" : "Play replay (Space)";
    prevBtn.disabled = r <= minRound();
    nextBtn.disabled = r >= S.latestRound;
    liveBtn.hidden = !isRunning();
    liveBtn.setAttribute("aria-pressed", String(S.followLive));
    liveBtn.title = S.followLive ? "Following the live round" : "Jump to the live round";
    renderScrubber();
    if (!drawer.isOpen && !S.selectedCamp) renderFeedHead();
  }

  async function selectRound(r, { animate = false } = {}) {
    if (!S.detail) return;
    r = clamp(Math.round(r), minRound(), Math.max(S.latestRound, minRound()));
    S.round = r;
    feed.showRound(r);
    chart?.setRound(r);
    renderChartSel();
    runLog.sync({ scroll: true });
    updatePlayer();
    if (S.selectedCamp && !drawer.isOpen) updatePanel();
    replaceQuery({ round: r === S.latestRound && isRunning() ? null : String(r) });
    updateCampMeans();
    renderCamps();
    const sent = sentimentUpTo(r);
    if (!S.actions.has(r)) {
      await actionsFor(r, { force: isRunning() && r >= S.latestRound });
      if (!S.alive || S.round !== r) return;
    }
    for (const v of [network, mini]) {
      if (!v) continue;
      v.setData({ sentimentByAgent: sent, actions: allActions(), camps: S.camps });
      if (animate) v.playRound(r); else v.setRound(r);
    }
    if (animate && !reducedMotion()) for (const ev of S.events.filter((e) => e.round === r)) network?.showEvent(ev.content);
  }

  let playTimer = null;
  function play() {
    if (!S.detail) return;
    S.playing = true;
    S.followLive = false;
    clearTimeout(playTimer);
    if (S.round === null || S.round >= S.latestRound) selectRound(minRound(), { animate: true });
    else selectRound(S.round + 1, { animate: true });
    scheduleTick();
    updatePlayer();
  }

  function scheduleTick() {
    clearTimeout(playTimer);
    playTimer = later(() => {
      if (!S.playing) return;
      if (S.round >= S.latestRound) {
        pause();
        if (isRunning()) S.followLive = true;
        updatePlayer();
        return;
      }
      selectRound(S.round + 1, { animate: true });
      scheduleTick();
    }, ROUND_MS / S.speed);
  }

  function pause() {
    S.playing = false;
    clearTimeout(playTimer);
    updatePlayer();
  }

  function setSpeed(s) {
    S.speed = s;
    for (const b of speedSeg.querySelectorAll("button")) b.setAttribute("aria-pressed", String(Number(b.dataset.speed) === s));
    if (S.playing) scheduleTick();
  }

  function jumpToLive() {
    pause();
    S.followLive = true;
    selectRound(S.latestRound, { animate: false });
  }

  const onKey = (e) => {
    if (drawer.contains(e.target) || e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return;
    const t = e.target;
    if (document.querySelector("dialog[open]")) return;
    if (e.key === "Escape" && S.selectedCamp && S.tab === "crowd" && !drawer.isOpen) {
      selectCamp(null);
      return;
    }
    if (t && (t.closest("input, textarea, select, [contenteditable], [role=menu]") || (t.closest("button, a, [role=tab]") && e.key === " "))) return;
    if (t && (t.tagName === "CANVAS" || t.closest?.(".sc-plot, .log-body")) && e.key.startsWith("Arrow")) return;
    if (e.key === " ") {
      e.preventDefault();
      S.playing ? pause() : play();
    } else if (e.key === "ArrowRight" && !t.closest?.("[role=tablist]")) {
      e.preventDefault();
      pause();
      S.followLive = false;
      selectRound((S.round ?? 0) + 1, { animate: true });
    } else if (e.key === "ArrowLeft" && !t.closest?.("[role=tablist]")) {
      e.preventDefault();
      pause();
      S.followLive = false;
      selectRound((S.round ?? 0) - 1, { animate: false });
    }
  };
  document.addEventListener("keydown", onKey);

  // -------------------------------------------------------------- selection
  function openAgent(id) {
    if (!S.personaById.has(id)) {
      const p = S.personaByHandle.get(String(id).replace(/^@/, "").toLowerCase());
      if (!p) return;
      id = p.id;
    }
    hideHoverCard();
    if (S.tab !== "crowd") selectTab("crowd");
    S.selectedAgent = id;
    netSelect();
    drawer.open(id);
    updatePanel();
  }

  function selectCamp(key, { tab = false } = {}) {
    hideHoverCard();
    if (drawer.isOpen) drawer.close();
    S.selectedAgent = null;
    S.selectedCamp = key || null;
    if (tab && S.tab !== "crowd") selectTab("crowd");
    replaceQuery({ camp: S.selectedCamp });
    netSelect();
    updatePanel();
  }

  function clearSelection() {
    if (drawer.isOpen) drawer.close();
    S.selectedAgent = null;
    S.selectedCamp = null;
    replaceQuery({ camp: null });
    netSelect();
    updatePanel();
  }

  function selectPost(id) {
    const post = S.posts.get(Number(id));
    hideHoverCard();
    if (!post) {
      toast("warn", `Post #${id} is not loaded`, "It may belong to a part of the run that has not been written yet.");
      return;
    }
    if (drawer.isOpen) drawer.close();
    S.selectedAgent = null;
    S.selectedCamp = null;
    S.peopleMode = "posts";
    replaceQuery({ camp: null });
    netSelect();
    if (S.tab !== "crowd") selectTab("crowd");
    updatePanel();
    const go = () => {
      feed.highlightPost(post.id);
      for (const v of [network]) v?.select(null);
    };
    if (post.round !== S.round) {
      pause();
      S.followLive = false;
      selectRound(post.round, { animate: false }).then(() => requestAnimationFrame(go));
    } else requestAnimationFrame(go);
  }

  /** Hover card content for a citation: the person behind @handle, or the post behind #id. */
  S.hoverContent = (t) => {
    const sent = sentimentUpTo(null);
    if (t.dataset.agent) {
      const p = persona(t.dataset.agent);
      if (!p) return null;
      const camp = S.campOf.get(p.id);
      return el("div", { style: "display:grid;gap:6px" },
        el("div", { class: "hc-who" }, av(p.id, 36), el("div", {}, el("b", { text: p.name }), el("span", { text: `@${p.handle}` }))),
        el("div", { class: "hc-meta" }, camp ? el("span", {}, campDot(camp.color), camp.name) : null, isScore(sent[p.id]) ? sv(sent[p.id]) : null,
          el("span", { text: `Influence ${(Number(p.influence) || 0).toFixed(2)}` })),
        p.bio ? el("div", { class: "hc-body", text: p.bio }) : null);
    }
    if (t.dataset.post) {
      const post = S.posts.get(Number(t.dataset.post));
      if (!post) return null;
      const p = persona(post.author_id);
      return el("div", { style: "display:grid;gap:6px" },
        el("div", { class: "hc-who" }, av(post.author_id, 32), el("div", {}, el("b", { text: p ? p.name : post.author_id }), el("span", { text: `@${p ? p.handle : post.author_handle} · #${post.id} · round ${post.round}` }))),
        el("div", { class: "hc-body", text: post.content || "(repost)" }),
        isScore(post.sentiment) ? el("div", { class: "hc-meta" }, "Sentiment ", sv(post.sentiment)) : null);
    }
    return null;
  };

  // -------------------------------------------------------------- run log (details)
  // Every action and event as a numbered line ("r4  #23  @delaney_fitch  reply -> #20"), built from /actions
  // and /posts. It follows the playhead; activating a line opens that post or person in the Crowd tab.
  function buildRunLog() {
    const meta = el("span", { class: "sub" });
    const list = el("ol", { "aria-label": "Run log lines" });
    const body = el("div", { class: "log-body" }, list);
    let showIdle = false;
    try {
      showIdle = localStorage.getItem("shoal.log.idle") === "show";
    } catch {
      /* storage blocked: default to hidden */
    }
    const idleBox = el("input", { type: "checkbox", id: `log-idle-${runId}` });
    idleBox.checked = showIdle;
    const idleToggle = el("label", { class: "switch log-idle", for: `log-idle-${runId}`, title: "Show agents that did nothing this round" },
      idleBox, el("span", { text: "Show idle" }));
    idleBox.addEventListener("change", () => {
      showIdle = idleBox.checked;
      try {
        localStorage.setItem("shoal.log.idle", showIdle ? "show" : "hide");
      } catch {
        /* storage blocked */
      }
      current = -1;
      render();
      sync({ scroll: true });
    });
    const root = el("section", { class: "card log-card", "aria-labelledby": "det-log-h" },
      el("div", { class: "card-head" }, el("h2", { id: "det-log-h", text: "Run log" }), meta, el("span", { class: "tools" }, idleToggle)), body);
    let lines = [];
    let els = [];
    let current = -1;
    let focusIdx = 0;
    let rendered = false;
    let lastRound = null;

    function buildLines() {
      const out = [];
      const postsByRound = new Map();
      for (const p of S.posts.values()) {
        if (!postsByRound.has(p.round)) postsByRound.set(p.round, []);
        postsByRound.get(p.round).push(p);
      }
      const rounds = new Set([...S.actions.keys(), ...postsByRound.keys(), ...S.events.map((e) => e.round), ...S.failedBatches.map((b) => b.round)]);
      for (const r of [...rounds].sort((a, b) => a - b)) {
        for (const ev of S.events.filter((e) => e.round === r)) out.push({ type: "event", round: r, text: ev.content || "" });
        const posts = (postsByRound.get(r) || []).slice().sort((a, b) => a.id - b.id);
        const used = new Set();
        const take = (author, pred) => {
          const p = posts.find((x) => x.author_id === author && !used.has(x.id) && pred(x));
          if (p) used.add(p.id);
          return p || null;
        };
        for (const a of S.actions.get(r) || []) {
          let post = null;
          if (a.type === "post") post = take(a.agent_id, (p) => p.parent_id == null && p.repost_of == null);
          else if (a.type === "reply") post = take(a.agent_id, (p) => p.parent_id === a.target_post_id) || take(a.agent_id, (p) => p.parent_id != null);
          else if (a.type === "repost") post = take(a.agent_id, (p) => p.repost_of === a.target_post_id) || take(a.agent_id, (p) => p.repost_of != null);
          out.push({ type: a.type, round: r, agent: a.agent_id, postId: post ? post.id : null,
            targetPost: a.target_post_id ?? null, targetAgent: a.target_agent_id ?? null, sentiment: post ? post.sentiment : null });
        }
        for (const p of posts) {
          if (used.has(p.id)) continue;
          const type = p.repost_of != null ? "repost" : p.parent_id != null ? "reply" : "post";
          out.push({ type, round: r, agent: p.author_id, postId: p.id, targetPost: p.parent_id ?? p.repost_of ?? null, targetAgent: null, sentiment: p.sentiment });
        }
        for (const b of S.failedBatches.filter((x) => x.round === r)) out.push({ type: "failed", round: r, batch: b });
      }
      const st = S.status;
      if (st && (st.state === "stopped" || st.state === "failed")) {
        out.push({ type: "end", round: Math.max(st.round ?? 0, S.latestRound ?? 0), state: st.state,
          text: st.state === "failed" ? (st.error || "the run failed") : `stopped after round ${st.round ?? S.latestRound}` });
      }
      out.forEach((ln, i) => { ln.n = i + 1; });
      return out;
    }

    const handle = (id) => {
      const p = S.personaById.get(id);
      return p ? `@${p.handle}` : id || "";
    };

    function lineEl(ln, i) {
      const b = el("button", { class: "log-line", type: "button", tabindex: "-1", "data-type": ln.type, "data-round": ln.round, "data-i": i });
      const cells = [el("span", { class: "ln", text: String(ln.n) }), el("span", { class: "rd", text: `r${ln.round}` })];
      if (ln.type === "event") {
        cells.push(el("span", { class: "id", text: "event" }),
          el("span", { class: "wide" }, el("span", { class: "tgt", text: ln.text })));
        b.setAttribute("aria-label", `Line ${ln.n}, round ${ln.round}, event: ${ln.text}`);
      } else if (ln.type === "failed") {
        const n = (ln.batch.agents || []).length;
        const txt = `batch ${ln.batch.batch ?? "?"}${n ? `, ${n} agent${n === 1 ? "" : "s"} skipped` : ""}${ln.batch.error ? `: ${ln.batch.error}` : ""}`;
        cells.push(el("span", { class: "id none", text: "-" }),
          el("span", { class: "wide" }, el("span", { class: "lbl", text: "Failed" }), el("span", { class: "tgt", text: txt })));
        b.setAttribute("aria-label", `Line ${ln.n}, round ${ln.round}, failed ${txt}`);
      } else if (ln.type === "end") {
        cells.push(el("span", { class: "id none", text: "-" }),
          el("span", { class: "wide" }, el("span", { class: "lbl", text: STATE_WORD[ln.state] || ln.state }), el("span", { class: "tgt", text: ln.text })));
        b.setAttribute("aria-label", `Line ${ln.n}, run ${ln.state}: ${ln.text}`);
      } else {
        const tgt = ln.type === "follow"
          ? el("span", { class: "tgt" }, "-> ", el("b", { text: handle(ln.targetAgent) }))
          : ln.targetPost != null
            ? el("span", { class: "tgt" }, "-> ", el("b", { text: `#${ln.targetPost}` }),
              S.posts.has(ln.targetPost) ? ` ${handle(S.posts.get(ln.targetPost).author_id)}` : "")
            : el("span", { class: "tgt" });
        const hasSent = isScore(ln.sentiment);
        cells.push(
          el("span", { class: "id" + (ln.postId == null ? " none" : ""), text: ln.postId == null ? "-" : `#${ln.postId}` }),
          el("span", { class: "who", text: handle(ln.agent) }),
          el("span", { class: "verb", text: ln.type }),
          tgt,
          el("span", { class: "sv", "data-s": sentimentSign(ln.sentiment), text: hasSent ? formatScore(ln.sentiment) : "" }));
        const target = ln.type === "follow" ? ` ${handle(ln.targetAgent)}` : ln.targetPost != null ? ` to #${ln.targetPost}` : "";
        b.setAttribute("aria-label", `Line ${ln.n}, round ${ln.round}, ${ln.postId != null ? `post #${ln.postId}, ` : ""}${handle(ln.agent)} ${ln.type}${target}${hasSent ? `, sentiment ${formatScore(ln.sentiment)}` : ""}`);
      }
      b.append(...cells);
      return b;
    }

    function render() {
      const prevCount = lines.length;
      const all = buildLines();
      const idle = all.filter((l) => l.type === "nothing").length;
      lines = showIdle ? all : all.filter((l) => l.type !== "nothing");
      els = lines.map((ln, i) => lineEl(ln, i));
      if (!els.length) {
        list.replaceChildren(el("li", { class: "log-empty", text: idle ? "Every agent sat out so far. Turn on Show idle to list them." : isRunning() ? "Lines appear here as agents act." : "No actions were recorded for this run." }));
      } else {
        list.replaceChildren(...els.map((b) => el("li", {}, b)));
      }
      const acts = all.filter((l) => l.agent).length;
      idleToggle.title = showIdle ? `Hide the ${idle} idle lines` : `Show the ${idle} lines where an agent did nothing`;
      meta.textContent = `${all.length} lines · ${acts} actions · ${S.events.length} event${S.events.length === 1 ? "" : "s"} · ${idle} idle`;
      if (current >= els.length) current = -1;
      const follow = !rendered || (S.followLive && lines.length !== prevCount);
      rendered = true;
      sync({ scroll: follow });
    }

    function setFocusIdx(i) {
      if (!els.length) return;
      focusIdx = clamp(i, 0, els.length - 1);
      els.forEach((b, j) => { b.tabIndex = j === focusIdx ? 0 : -1; });
    }

    function sync({ scroll = false } = {}) {
      const r = S.round ?? 0;
      let first = -1;
      let last = -1;
      els.forEach((b, i) => {
        const lr = lines[i].round;
        b.dataset.future = String(lr > r);
        b.dataset.now = String(lr === r);
        b.setAttribute("aria-current", String(i === current));
        if (lr === r) {
          if (first < 0) first = i;
          last = i;
        }
      });
      if (first < 0) first = lines.findIndex((l) => l.round > r);
      if (current < 0 || lines[current]?.round !== r) setFocusIdx(first < 0 ? els.length - 1 : first);
      if (!scroll || !els.length || paneDetails.hidden) return;
      const behavior = reducedMotion() ? "auto" : "smooth";
      if (current >= 0 && lines[current]?.round === r) {
        const li = els[current].parentElement;
        if (li.offsetTop < body.scrollTop || li.offsetTop + li.offsetHeight > body.scrollTop + body.clientHeight) {
          body.scrollTo({ top: li.offsetTop - body.clientHeight / 3, behavior });
        }
        return;
      }
      if (S.followLive && isRunning() && r >= S.latestRound && last >= 0) {
        const li = els[last].parentElement;
        body.scrollTo({ top: Math.max(0, li.offsetTop + li.offsetHeight - body.clientHeight + 8), behavior });
      } else if (first >= 0) {
        body.scrollTo({ top: Math.max(0, els[first].parentElement.offsetTop - 4), behavior: lastRound === null ? "auto" : behavior });
      }
      lastRound = r;
    }

    function activate(i) {
      const ln = lines[i];
      if (!ln) return;
      current = i;
      setFocusIdx(i);
      const pid = ln.postId ?? ln.targetPost;
      if (ln.postId != null && S.posts.has(ln.postId)) selectPost(ln.postId);
      else if (ln.agent) openAgent(ln.agent);
      else if (pid != null && S.posts.has(pid)) selectPost(pid);
      else if (ln.round !== S.round) {
        pause();
        S.followLive = false;
        selectRound(ln.round, { animate: false });
      }
      sync({ scroll: true });
    }

    list.addEventListener("click", (e) => {
      const b = e.target.closest(".log-line");
      if (b) activate(Number(b.dataset.i));
    });
    list.addEventListener("keydown", (e) => {
      const b = e.target.closest(".log-line");
      if (!b) return;
      const i = Number(b.dataset.i);
      let next = null;
      if (e.key === "ArrowDown") next = i + 1;
      else if (e.key === "ArrowUp") next = i - 1;
      else if (e.key === "Home") next = 0;
      else if (e.key === "End") next = els.length - 1;
      else if (e.key === "PageDown") next = i + 10;
      else if (e.key === "PageUp") next = i - 10;
      if (next === null) return;
      e.preventDefault();
      setFocusIdx(next);
      els[focusIdx].focus();
    });

    return {
      root,
      render,
      sync,
      markSelected: (id) => {
        if (id === null) current = -1;
        sync();
      },
    };
  }

  // -------------------------------------------------------------- knowledge graph entity card
  function showEntity(entityId) {
    const g = S.detail && S.detail.graph;
    const ent = g && g.entities.find((e) => e.id === entityId);
    if (!ent) {
      kgCard.hidden = true;
      return;
    }
    const names = new Map(g.entities.map((e) => [e.id, e.name]));
    const rels = g.relations.filter((r) => r.source === entityId || r.target === entityId);
    const embodied = S.personas.filter((p) => p.entity_id === entityId);
    kgCard.replaceChildren(el("div", { class: "entity-card", role: "region", "aria-label": `Entity ${ent.name}` },
      el("div", { style: "display:flex;justify-content:space-between;gap:8px;align-items:start" },
        el("div", {}, el("h3", { text: ent.name }), el("span", { class: "kind", text: ent.kind })),
        el("button", { class: "icon-btn", type: "button", "aria-label": "Close entity details", onclick: () => { kgCard.hidden = true; } }, icon("x", "icon sm"))),
      el("p", { style: "font-size:13px;line-height:1.5", text: ent.summary || "" }),
      embodied.length ? el("div", { class: "chips" }, el("span", { class: "dim", style: "font-size:12px;align-self:center", text: "Played by" }),
        embodied.map((p) => el("button", { class: "chip", type: "button", onclick: () => openAgent(p.id) }, av(p.id, 20), `@${p.handle}`))) : null,
      rels.length ? el("ul", { class: "rels" }, rels.slice(0, 40).map((r) => {
        const out = r.source === entityId;
        return el("li", { title: r.evidence || "" },
          out ? "" : el("b", { text: names.get(r.source) || r.source }),
          out ? "" : " ",
          el("span", { text: r.label }),
          out ? " " : "",
          out ? el("b", { text: names.get(r.target) || r.target }) : el("span", { text: " this" }));
      })) : el("p", { class: "dim", style: "font-size:12px", text: "No relations recorded." })));
    kgCard.hidden = false;
  }

  // -------------------------------------------------------------- stop / inject
  async function stopRun() {
    const ok = await confirmDialog({
      title: "Stop this simulation?",
      body: "The current round finishes first. If at least one round completed, the report is still written from what happened so far.",
      confirm: "Stop run",
      danger: true,
    });
    if (!ok) return;
    stopBtn.disabled = true;
    try {
      await api.stop(runId);
      toast("info", "Stopping after this round", "");
    } catch (err) {
      toast("error", "Could not stop the run", err.message);
    } finally {
      stopBtn.disabled = false;
    }
  }
  stopBtn.addEventListener("click", () => stopRun());
  injectBtn.addEventListener("click", () => openInjectDialog());

  /** First round an event can still be added to: the server rejects rounds that have started. */
  function injectMinRound() {
    const st = S.status;
    return Math.max(S.startedRound || 0, st ? st.round || 0 : 0) + 1;
  }

  function openInjectDialog() {
    const st = S.status;
    if (!st) return;
    const minR = injectMinRound();
    if (minR > st.total_rounds) {
      toast("warn", "Too late to inject", "Every round has already started.");
      return;
    }
    const roundIn = el("input", { class: "input num-input", type: "number", id: "inj-round", min: minR, max: st.total_rounds, value: minR, required: true });
    const textIn = el("textarea", { class: "textarea", id: "inj-text", rows: "3", maxlength: "2000", required: true, placeholder: "Breaking news every agent sees at the start of that round" });
    const err = el("div", { class: "err", role: "alert" });
    const submit = el("button", { class: "btn btn-primary", type: "submit" }, icon("event", "icon sm"), "Inject");
    const cancel = el("button", { class: "btn", type: "button", value: "cancel" }, "Cancel");
    const dlg = el("dialog", { class: "modal", "aria-labelledby": "inj-title" });
    const form = el("form", { method: "dialog", novalidate: true },
      el("h2", { id: "inj-title", text: "Inject an event" }),
      el("p", { text: `Rounds already running or finished cannot change. Choose a round between ${minR} and ${st.total_rounds}.` }),
      el("div", { class: "field" }, el("label", { for: "inj-round", text: "Round" }), roundIn),
      el("div", { class: "field" }, el("label", { for: "inj-text", text: "What happens" }), textIn),
      err,
      el("div", { class: "foot" }, cancel, submit));
    dlg.appendChild(form);
    document.body.appendChild(dlg);
    cancel.addEventListener("click", () => dlg.close());
    dlg.addEventListener("close", () => dlg.remove());
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const r = parseInt(roundIn.value, 10);
      const text = textIn.value.trim();
      const lowest = Math.min(injectMinRound(), st.total_rounds);
      if (!Number.isInteger(r) || r < lowest || r > st.total_rounds) {
        err.textContent = `Pick a round between ${lowest} and ${st.total_rounds}; earlier rounds have already started.`;
        roundIn.focus();
        return;
      }
      if (!text) {
        err.textContent = "Describe the event.";
        textIn.focus();
        return;
      }
      submit.disabled = true;
      try {
        await api.injectEvent(runId, r, text);
        dlg.close();
        toast("event", `Event planned for round ${r}`, text);
        refreshSoon(200);
      } catch (ex) {
        err.textContent = ex instanceof ApiError && ex.status === 409 ? `Too late for round ${r}: ${ex.message}` : ex.message;
        submit.disabled = false;
      }
    });
    dlg.showModal();
    textIn.focus();
  }

  // -------------------------------------------------------------- live stream
  function connect() {
    if (S.closeStream) return;
    streamLbl.textContent = "Connecting…";
    S.closeStream = stream(runId, {
      open: () => {
        streamLbl.textContent = "";
      },
      error: () => {
        streamLbl.textContent = "Reconnecting…";
      },
      status: (st) => {
        if (!st || !st.state) return;
        const prev = S.status;
        S.status = st;
        if (mergeFailedBatches(st.failed_batches)) { runLog.render(); renderFailures(); }
        computeLatest();
        renderHead();
        renderSummary();
        renderPipeline();
        if (!prev || prev.stage !== st.stage || prev.round !== st.round) refreshSoon();
        if (prev && TERMINAL.has(st.state) && !TERMINAL.has(prev.state)) finish(st.state);
        else if (!prev && TERMINAL.has(st.state)) finish(st.state, { quiet: true });
        if (!S.live && prev && st.round > prev.round) onRoundDone({ round: st.round });
      },
      stage: () => refreshSoon(),
      round_start: (p) => {
        if (!p) return;
        if (typeof p.round === "number") S.startedRound = Math.max(S.startedRound || 0, p.round);
        renderHead();
      },
      posts_added: async (p) => {
        await fetchNewPosts();
        if (S.followLive && p && typeof p.round === "number" && p.round !== S.round) {
          S.round = Math.max(p.round, S.round ?? 0);
          feed.showRound(S.round);
          chart?.setRound(S.round);
        }
        runLog.render();
        updatePlayer();
        renderVoices();
      },
      round_done: (p) => onRoundDone(p),
      batch_failed: (p) => {
        if (p && typeof p.round === "number" && mergeFailedBatches([p])) { runLog.render(); renderFailures(); }
        toast("warn", `A batch failed in round ${p && p.round}`, `${(p && p.agents && p.agents.length) || "Some"} agents skip this round; the run continues. Details has the record.`);
      },
      event_injected: (p) => {
        if (!p) return;
        network?.showEvent(p.content || "");
        toast("event", `Breaking news, round ${p.round}`, p.content || "");
        refreshSoon(100);
      },
      run_done: () => finish("done"),
      run_failed: (p) => {
        toast("error", "The run failed", (p && p.error) || "");
        finish("failed", { quiet: true });
      },
      run_stopped: () => finish("stopped"),
    });
  }

  async function onRoundDone(p) {
    if (!p || typeof p.round !== "number") return;
    const r = p.round;
    if (p.actions || p.sentiment !== undefined) {
      const stat = { round: r, actions: p.actions || {}, new_posts: p.new_posts || 0, active_agents: p.active_agents || 0,
        sentiment: p.sentiment ?? null, llm_calls: p.llm_calls || 0, llm_seconds: p.llm_seconds || 0 };
      S.stats = [...S.stats.filter((s) => s.round !== r), stat].sort((a, b) => a.round - b.round);
    }
    await fetchNewPosts();
    if (!S.alive) return;
    if (S.posts.size < 6000) {
      try {
        const all = await api.posts(runId, 0);
        if (!S.alive) return;
        ingestPosts(all);
        feed.setData({ posts: postsList(), personas: S.personas, events: S.events });
      } catch {
        /* counters stay as they were */
      }
    }
    computeLatest();
    await actionsFor(r, { force: true });
    if (!S.alive) return;
    syncChart();
    updateEmptyStates();
    runLog.render();
    renderOverview();
    renderRoundsTable();
    if (S.followLive && !S.playing) {
      S.round = r;
      feed.showRound(r);
      await selectRound(r, { animate: true });
    } else updatePlayer();
    if (S.alive) refreshSoon(150);
  }

  async function finish(state, { quiet = false } = {}) {
    if (S.closeStream) {
      S.closeStream();
      S.closeStream = null;
    }
    streamLbl.textContent = "";
    if (!quiet) {
      if (state === "done") toast("ok", "Simulation finished", "The answer is on the Overview tab.");
      else if (state === "stopped") toast("info", "Simulation stopped", "");
    }
    await fetchNewPosts();
    await refreshDetail();
    if (!S.alive) return;
    try {
      const acts = await api.actions(runId);
      S.actions.clear();
      for (const a of acts) {
        if (!S.actions.has(a.round)) S.actions.set(a.round, []);
        S.actions.get(a.round).push(a);
      }
    } catch {
      /* fine */
    }
    S.followLive = false;
    computeCamps();
    syncNetwork({ full: true });
    renderHead();
    renderReport();
    renderOverview();
    renderDetails();
    runLog.render();
    loadSidebar();
  }

  // -------------------------------------------------------------- go
  loadAll();

  return function unmount() {
    S.alive = false;
    pause();
    hideHoverCard();
    if (S.closeStream) S.closeStream();
    for (const t of S.timers) clearTimeout(t);
    clearInterval(clock);
    document.removeEventListener("keydown", onKey);
    drawer.destroy();
    try {
      network?.destroy();
      mini?.destroy();
      kgraph?.destroy();
      chart?.destroy();
    } catch (err) {
      console.error(err);
    }
    document.querySelectorAll("dialog.modal").forEach((d) => d.remove());
    if (Studio.run === S) Studio.run = null;
  };
}

// ------------------------------------------------------------------ confirm dialog

function confirmDialog({ title, body, confirm = "Confirm", danger = false }) {
  return new Promise((resolve) => {
    const dlg = el("dialog", { class: "modal", "aria-labelledby": "cf-title" });
    const ok = el("button", { class: `btn ${danger ? "btn-danger" : "btn-primary"}`, type: "submit", value: "ok", text: confirm });
    const cancel = el("button", { class: "btn", type: "submit", value: "cancel", text: "Cancel" });
    dlg.appendChild(el("form", { method: "dialog" },
      el("h2", { id: "cf-title", text: title }),
      el("p", { text: body }),
      el("div", { class: "foot" }, cancel, ok)));
    document.body.appendChild(dlg);
    dlg.addEventListener("close", () => {
      resolve(dlg.returnValue === "ok");
      dlg.remove();
    });
    dlg.showModal();
    cancel.focus();
  });
}

// ------------------------------------------------------------------ boot

setNav(false);
checkHealth();
setInterval(checkHealth, 30000);
loadSidebar();
route();
