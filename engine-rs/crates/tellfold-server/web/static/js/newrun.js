// Tellfold Studio: the "New simulation" form. Three plain questions (what to find out, what it is about, how
// big), with every engine option kept under Advanced at its usual default. A typed description is sent as a
// seed file named description.md, exactly like an uploaded file.

const SVG_NS = "http://www.w3.org/2000/svg";
const ICONS = {
  upload: "M12 15V3m0 0L7 8m5-5 5 5M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4",
  x: "M18 6 6 18M6 6l12 12",
  plus: "M12 5v14M5 12h14",
  chevron: "m9 6 6 6-6 6",
  info: "M12 8h.01M11 12h1v5h1M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z",
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

/** Text with `code` spans (runtime hints use backticks), built without innerHTML. */
function withCode(text) {
  return String(text || "").split(/(`[^`]+`)/).filter(Boolean)
    .map((part) => (part.startsWith("`") && part.endsWith("`") ? el("code", { text: part.slice(1, -1) }) : part));
}

// ------------------------------------------------------------------ constants

export const EXAMPLES = [
  "How will customers react if we raise the price by 20%?",
  "Will people switch to our app from the one they use now?",
  "How will users react if we remove the free plan?",
];

export const PRESETS = [
  { id: "quick", label: "Quick look", agents: 10, rounds: 3, minutes: 3 },
  { id: "standard", label: "Standard", agents: 20, rounds: 6, minutes: 8 },
  { id: "deep", label: "Deep", agents: 40, rounds: 10, minutes: 20 },
];

const ACCEPT = [".pdf", ".md", ".markdown", ".txt"];
const MAX_BYTES = 20 * 1024 * 1024;
const MAX_FILES = 100;
const MIN_DESCRIBE = 40;
const MIN_QUESTION = 8;
const DESCRIPTION_NAME = "description.md";
const CLAUDE_MODELS = ["haiku", "sonnet", "opus"];
const PREFS_KEY = "tellfold.newrun.prefs";
const ADVANCED_KEY = "tellfold.newrun.advanced";
const DEFAULTS = {
  agents: 20, rounds: 6, batch: 6, concurrency: 4, agent_runtime: "claude-code", agent_model: "haiku",
  planner_runtime: "claude-code", planner_model: "sonnet", language: "English", seed: 7, think: false,
};
const LIMITS = {
  agents: [2, 200], rounds: [1, 50], batch: [1, 20], concurrency: [1, 16], seed: [0, 2 ** 31 - 1],
};
const RUNTIME_ID = /^[a-z0-9][a-z0-9._-]{0,39}(:[a-z0-9][a-z0-9_-]{0,39})?$/;
const MODEL_NAME = /^[A-Za-z0-9][A-Za-z0-9._:/@[\]-]{0,119}$/;

function ext(name) {
  const m = /\.[^.]+$/.exec(name.toLowerCase());
  return m ? m[0] : "";
}

function fmtBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** "about 8 min" for a custom size, on the same curve as the preset estimates (3 / 8 / 20 min). */
function aboutMinutes(cfg) {
  const work = Math.max(1, cfg.agents * cfg.rounds);
  let mid = 3 * Math.pow(work / 30, 0.73);
  mid *= Math.pow(4 / Math.max(1, cfg.concurrency), 0.6); // more calls at once, shorter run
  if (cfg.think) mid *= 1.3;
  if (mid < 1.5) return "about a minute";
  if (mid < 10) return `about ${Math.round(mid)} min`;
  if (mid < 90) return `about ${Math.round(mid / 5) * 5} min`;
  return `about ${(mid / 60).toFixed(1).replace(/\.0$/, "")} hours`;
}

function readStore(key) {
  try {
    // Falls back to the key saved while the app was called Shoal ("shoal.newrun.prefs" and so on).
    return localStorage.getItem(key) ?? localStorage.getItem(key.replace(/^tellfold\./, "shoal."));
  } catch {
    return null;
  }
}

function writeStore(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* storage may be unavailable */
  }
}

function loadPrefs() {
  try {
    const raw = readStore(PREFS_KEY);
    const p = raw ? JSON.parse(raw) : {};
    return p && typeof p === "object" ? p : {};
  } catch {
    return {};
  }
}

function toast(kind, title, body) {
  document.dispatchEvent(new CustomEvent("studio:toast", { detail: { kind, title, body } }));
}

const clampInt = (v, [lo, hi], d) => {
  const n = parseInt(v, 10);
  return Number.isFinite(n) ? Math.max(lo, Math.min(hi, n)) : d;
};

const prettyRuntime = (id) => String(id || "").replace(/[-_]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());

const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/i.test(navigator.platform || navigator.userAgent || "");

// ------------------------------------------------------------------ form

/**
 * Mount the form into `container`.
 * prefill: { question?, config?: SimConfig of an earlier run (Run again),
 *            seeds?: File[] saved with that run (null when they could not be read) }
 * Returns unmount().
 */
export function mountNewRun(container, { api, onCreated, cancelHref = "#/", prefill = {} } = {}) {
  const saved = loadPrefs();
  const cfg = prefill.config || null;
  const pick = (cfgKey, prefKey, d, ok = () => true) => {
    if (cfg && cfg[cfgKey] !== undefined && cfg[cfgKey] !== null && ok(cfg[cfgKey])) return cfg[cfgKey];
    if (saved[prefKey] !== undefined && saved[prefKey] !== null && ok(saved[prefKey])) return saved[prefKey];
    return d;
  };
  const okRuntime = (v) => typeof v === "string" && RUNTIME_ID.test(v);
  const okModel = (v) => typeof v === "string" && MODEL_NAME.test(v);
  const state = {
    question: String(prefill.question || (cfg && cfg.question) || ""),
    mode: "describe", // which way of answering "What is it about?" is showing
    description: "",
    files: [], // {file}
    fileErrors: [],
    agents: cfg ? clampInt(cfg.num_agents, LIMITS.agents, DEFAULTS.agents) : DEFAULTS.agents,
    rounds: cfg ? clampInt(cfg.rounds, LIMITS.rounds, DEFAULTS.rounds) : DEFAULTS.rounds,
    batch: clampInt(pick("batch_size", "batch", DEFAULTS.batch), LIMITS.batch, DEFAULTS.batch),
    concurrency: clampInt(pick("concurrency", "concurrency", DEFAULTS.concurrency), LIMITS.concurrency, DEFAULTS.concurrency),
    agent_runtime: pick("agent_runtime", "agent_runtime", null, okRuntime),
    agent_model: pick("agent_model", "agent_model", null, okModel),
    planner_runtime: pick("planner_runtime", "planner_runtime", null, okRuntime),
    planner_model: pick("planner_model", "planner_model", null, okModel),
    language: String(pick("language", "language", DEFAULTS.language, (v) => typeof v === "string" && v.trim())).slice(0, 40),
    seed: cfg ? clampInt(cfg.seed, LIMITS.seed, DEFAULTS.seed) : DEFAULTS.seed,
    think: !!pick("think", "think", DEFAULTS.think),
    events: cfg && Array.isArray(cfg.events)
      ? cfg.events.filter((e) => e && typeof e.content === "string").map((e) => ({ round: Number(e.round) || 1, content: e.content }))
      : [],
    runtimes: null, // RuntimeInfo[] once loaded
    fake: false,
    submitting: false,
    attempted: false,
  };
  // the runtime / model fields that came from nowhere (no run, no saved choice) take the server's settings
  const fromSettings = new Set(["agent_runtime", "agent_model", "planner_runtime", "planner_model"].filter((k) => state[k] === null));
  for (const k of fromSettings) state[k] = DEFAULTS[k];

  // ------------------------------------------------------------------ 1. question
  const question = el("textarea", { class: "textarea nr-question", id: "nr-question", rows: "3", maxlength: "2000",
    placeholder: "How will people react to …?", "aria-describedby": "nr-examples-label" });
  question.value = state.question;
  const examples = el("div", { class: "nr-examples", role: "group", "aria-labelledby": "nr-examples-label" },
    el("span", { class: "nr-examples-label", id: "nr-examples-label", text: "Try an example:" }),
    EXAMPLES.map((text) => el("button", { class: "nr-example", type: "button", text,
      onclick: () => {
        question.value = text;
        state.question = text;
        update();
        question.focus();
        question.setSelectionRange(text.length, text.length);
      } })));

  // ------------------------------------------------------------------ 2. what it is about
  const describeTab = el("button", { class: "", type: "button", role: "tab", id: "nr-tab-describe", "aria-controls": "nr-pane-describe" });
  const filesTab = el("button", { class: "", type: "button", role: "tab", id: "nr-tab-files", "aria-controls": "nr-pane-files" });
  const seg = el("div", { class: "seg nr-seg", role: "tablist", "aria-label": "How to give the context" }, describeTab, filesTab);

  const description = el("textarea", { class: "textarea", id: "nr-description", rows: "4", maxlength: "200000",
    placeholder: "Paste a product description, a pricing page, an announcement or your own notes.",
    "aria-describedby": "nr-description-hint", "aria-label": "Describe it" });
  const descHint = el("div", { class: "hint", id: "nr-description-hint" });
  const describePane = el("div", { class: "nr-pane", role: "tabpanel", id: "nr-pane-describe", "aria-labelledby": "nr-tab-describe" },
    description, descHint);

  const fileInput = el("input", { type: "file", multiple: true, accept: ACCEPT.join(","), class: "sr-only", tabindex: "-1", "aria-hidden": "true" });
  const chooseBtn = el("button", { class: "btn", type: "button", onclick: () => fileInput.click() }, "Choose files…");
  const dropzone = el("div", { class: "dropzone nr-drop" },
    icon("upload"),
    el("div", { class: "nr-drop-title", text: "Drop files here" }),
    el("div", { class: "hint", id: "nr-files-hint", text: ".md, .txt or .pdf, up to 20 MB each" }),
    chooseBtn);
  const fileList = el("div", { class: "files", role: "list", "aria-label": "Added files" });
  const fileErr = el("div", { class: "err", role: "alert" });
  const filesPane = el("div", { class: "nr-pane", role: "tabpanel", id: "nr-pane-files", "aria-labelledby": "nr-tab-files" },
    dropzone, fileInput, fileList, fileErr);

  // Run again: the earlier run's documents come back (description.md as text, the rest as files); only when
  // none could be read does the form ask for them again
  const seedFiles = Array.isArray(prefill.seeds) ? prefill.seeds.filter((f) => f && f.size > 0) : [];
  const seedText = seedFiles.find((f) => f.name.toLowerCase() === DESCRIPTION_NAME) || null;
  const seedOthers = seedFiles.filter((f) => f !== seedText);
  const againNote = cfg && !seedFiles.length && Array.isArray(cfg.seed_files) && cfg.seed_files.length
    ? el("div", { class: "nr-note" }, icon("info", "icon sm"),
      el("div", {}, el("b", { text: "Add the documents again. " }),
        `The earlier run used ${cfg.seed_files.map((p) => String(p).replace(/\\/g, "/").split("/").pop()).join(", ")}; `,
        "its documents are not copied over."))
    : null;

  // ------------------------------------------------------------------ 3. size
  const presetName = `nr-size-${Math.random().toString(36).slice(2, 8)}`;
  const presetInputs = new Map();
  const presetCards = PRESETS.map((p) => {
    const input = el("input", { type: "radio", name: presetName, value: p.id, class: "sr-only" });
    input.addEventListener("change", () => {
      if (!input.checked) return;
      state.agents = p.agents;
      state.rounds = p.rounds;
      agentsIn.value = p.agents;
      roundsIn.value = p.rounds;
      update();
    });
    presetInputs.set(p.id, input);
    return el("label", { class: "nr-preset", "data-preset": p.id },
      input,
      el("span", { class: "nr-preset-name", text: p.label }),
      el("span", { class: "nr-preset-meta", text: `${p.agents} people · ${p.rounds} rounds` }),
      el("span", { class: "nr-preset-time", text: `about ${p.minutes} min` }));
  });
  const customTag = el("span", { class: "nr-custom", hidden: true });

  // ------------------------------------------------------------------ 4. advanced
  const numberField = (key, label, hint) => {
    const [lo, hi] = LIMITS[key];
    const input = el("input", { class: "input num-input", type: "number", id: `nr-${key}`, min: lo, max: hi, step: "1",
      value: state[key], inputmode: "numeric", "aria-describedby": hint ? `nr-${key}-hint` : undefined });
    input.addEventListener("input", () => {
      const n = parseInt(input.value, 10);
      if (Number.isFinite(n) && n >= lo && n <= hi) {
        state[key] = n;
        update();
      }
    });
    input.addEventListener("change", () => {
      state[key] = clampInt(input.value, LIMITS[key], state[key]);
      input.value = state[key];
      update();
    });
    const field = el("div", { class: "field" }, el("label", { for: `nr-${key}`, text: label }), input,
      hint ? el("div", { class: "hint", id: `nr-${key}-hint`, text: hint }) : null);
    return { field, input };
  };
  const { field: agentsField, input: agentsIn } = numberField("agents", "People", "2 to 200");
  const { field: roundsField, input: roundsIn } = numberField("rounds", "Rounds", "1 to 50");
  const { field: batchField } = numberField("batch", "People per model call", "Bigger batches mean fewer, longer calls");
  const { field: concField } = numberField("concurrency", "Concurrency", "Model calls at the same time");
  const { field: seedField } = numberField("seed", "Random seed", "Same seed, same activity order");

  const language = el("input", { class: "input", id: "nr-language", value: state.language, maxlength: "40", autocomplete: "off",
    "aria-describedby": "nr-language-hint" });
  language.addEventListener("input", () => {
    state.language = language.value.trim() || DEFAULTS.language;
    update();
  });
  const languageField = el("div", { class: "field" }, el("label", { for: "nr-language", text: "Language" }), language,
    el("div", { class: "hint", id: "nr-language-hint", text: "What the crowd writes in" }));

  const think = el("input", { type: "checkbox", id: "nr-think", checked: state.think });
  think.addEventListener("change", () => {
    state.think = think.checked;
    update();
  });
  const thinkField = el("label", { class: "switch", for: "nr-think" }, think,
    el("span", {}, "Extended thinking ", el("span", { class: "dim", text: "for the topic map, the people and the report (slower, more thorough)" })));

  // runtime + model pickers, one pair per role
  const runtimeById = (id) => (state.runtimes || []).find((r) => r.id === id) || null;
  const runtimeName = (id) => (runtimeById(id) || {}).name || (id === "claude-code" ? "Claude Code" : prettyRuntime(id));
  const modelsFor = (id) => {
    const rt = runtimeById(id);
    if (rt) return rt.models || [];
    return id === "claude-code" ? CLAUDE_MODELS : [];
  };
  const pickerPair = (role, label, hint) => {
    const rtKey = `${role}_runtime`, mKey = `${role}_model`;
    const rtSel = el("select", { class: "select", id: `nr-${rtKey}` });
    const modelHost = el("div", { class: "nr-model-host" });
    const renderRuntime = () => {
      const list = state.runtimes && state.runtimes.length ? state.runtimes : [{ id: state[rtKey], name: runtimeName(state[rtKey]), installed: true }];
      const ids = list.map((r) => r.id);
      const opts = list.map((r) => el("option", { value: r.id, selected: r.id === state[rtKey],
        text: r.name + (r.installed === false ? " (not found)" : r.auth === "missing" ? " (not signed in)" : "") }));
      if (!ids.includes(state[rtKey])) opts.unshift(el("option", { value: state[rtKey], selected: true, text: runtimeName(state[rtKey]) }));
      rtSel.replaceChildren(...opts);
    };
    const renderModel = () => {
      const models = modelsFor(state[rtKey]);
      let control;
      if (models.length) {
        const list = models.includes(state[mKey]) || !state[mKey] ? models : [state[mKey], ...models];
        control = el("select", { class: "select", id: `nr-${mKey}` },
          list.map((m) => el("option", { value: m, text: m, selected: m === state[mKey] })));
        if (!state[mKey]) state[mKey] = list[0];
      } else {
        control = el("input", { class: "input", id: `nr-${mKey}`, value: state[mKey] || "", maxlength: "120", autocomplete: "off",
          placeholder: "the runtime's default" });
      }
      const sync = () => {
        state[mKey] = control.value.trim();
        fromSettings.delete(mKey);
        update();
      };
      control.addEventListener("change", sync);
      if (control.tagName === "INPUT") control.addEventListener("input", sync);
      modelHost.replaceChildren(control);
    };
    rtSel.addEventListener("change", () => {
      state[rtKey] = rtSel.value;
      fromSettings.delete(rtKey);
      const rt = runtimeById(rtSel.value);
      const dm = rt && rt.default_models ? rt.default_models[role] : "";
      state[mKey] = dm || (rt && rt.models && rt.models[0]) || "";
      fromSettings.delete(mKey);
      renderModel();
      update();
    });
    const field = el("div", { class: "nr-pair" },
      el("div", { class: "field" }, el("label", { for: `nr-${rtKey}`, text: `${label} runtime` }), rtSel),
      el("div", { class: "field" }, el("label", { for: `nr-${mKey}`, text: `${label} model` }), modelHost),
      el("div", { class: "hint nr-pair-hint", text: hint }));
    return { field, render: () => { renderRuntime(); renderModel(); } };
  };
  const agentPicker = pickerPair("agent", "Agent", "Writes what each person does every round. Many small calls.");
  const plannerPicker = pickerPair("planner", "Planner", "Maps the topic, casts the people and writes the report.");

  // breaking-news events
  const eventsList = el("div", { class: "events-list" });
  const addEventBtn = el("button", { class: "btn btn-sm", type: "button" }, icon("plus", "icon sm"), "Add news");

  const advBody = el("div", { class: "nr-adv-body", id: "nr-advanced" },
    el("div", { class: "nr-adv-group" },
      el("h3", { text: "Crowd and rounds" }),
      el("div", { class: "grid-2" }, agentsField, roundsField),
      el("div", { class: "grid-2" }, batchField, concField)),
    el("div", { class: "nr-adv-group" },
      el("h3", { text: "Models" }),
      agentPicker.field,
      plannerPicker.field,
      thinkField),
    el("div", { class: "nr-adv-group" },
      el("h3", { text: "Output" }),
      el("div", { class: "grid-2" }, languageField, seedField)),
    el("div", { class: "nr-adv-group" },
      el("div", { class: "nr-adv-head" }, el("h3", { text: "Breaking news" }), addEventBtn),
      el("p", { class: "hint", text: "Shown to every person at the start of the round you pick. You can also add news while the run is live." }),
      eventsList));
  let advOpen = readStore(ADVANCED_KEY) === "open";
  const advToggle = el("button", { class: "nr-adv-toggle", type: "button", "aria-controls": "nr-advanced" },
    icon("chevron", "icon sm nr-chev"), el("span", { text: "Advanced" }),
    el("span", { class: "nr-adv-sub", text: "exact size, models, language, breaking news" }));
  const setAdvanced = (open, remember = true) => {
    advOpen = open;
    advToggle.setAttribute("aria-expanded", String(open));
    advBody.hidden = !open;
    if (remember) writeStore(ADVANCED_KEY, open ? "open" : "closed");
  };
  advToggle.addEventListener("click", () => setAdvanced(!advOpen));

  // ------------------------------------------------------------------ readiness, footer
  const readyNote = el("div", { class: "nr-note nr-ready", role: "status", hidden: true });
  const fakeNote = el("div", { class: "nr-note", hidden: true }, icon("info", "icon sm"),
    el("div", { text: "This server uses the fake model, so runs finish fast with placeholder text." }));
  const formErr = el("div", { class: "form-error", role: "alert", hidden: true });

  const summary = el("p", { class: "nr-summary", "aria-live": "polite" });
  const reason = el("span", { class: "nr-reason", id: "nr-reason", "aria-live": "polite" });
  const submitBtn = el("button", { class: "btn btn-primary nr-start", type: "submit", "aria-describedby": "nr-reason" },
    el("span", { class: "lbl", text: "Start simulation" }),
    el("kbd", { class: "nr-kbd", "aria-hidden": "true", text: isMac ? "⌘↩" : "Ctrl ↩" }));
  submitBtn.setAttribute("aria-keyshortcuts", isMac ? "Meta+Enter" : "Control+Enter");
  const cancel = el("a", { class: "btn", href: cancelHref }, "Cancel");

  const section = (id, title, sub, ...body) => el("section", { class: "card nr-card", "aria-labelledby": id },
    el("div", { class: "nr-head" }, el("h2", { id, text: title }), sub),
    ...body);

  const form = el("form", { class: "nr", novalidate: true, "aria-label": "New simulation" },
    readyNote,
    fakeNote,
    section("nr-q-title", "What do you want to find out?", null,
      el("label", { class: "sr-only", for: "nr-question", text: "Your question" }), question, examples),
    section("nr-ctx-title", "What is it about?", null,
      el("p", { class: "nr-help", text: "The simulated crowd only knows what you give it here." }),
      againNote,
      seg, describePane, filesPane),
    section("nr-size-title", "How big?", customTag,
      el("div", { class: "nr-presets", role: "radiogroup", "aria-labelledby": "nr-size-title" }, presetCards),
      el("p", { class: "hint", text: "Times are rough and depend on your machine and model." })),
    el("section", { class: "card nr-card nr-adv" }, advToggle, advBody),
    formErr,
    el("div", { class: "nr-foot" },
      el("div", { class: "nr-foot-bar" },
        summary,
        el("div", { class: "nr-actions" }, reason, cancel, submitBtn))));

  container.replaceChildren(form);

  // ------------------------------------------------------------------ the segmented control
  const setMode = (mode, focus = false) => {
    state.mode = mode;
    const d = mode === "describe";
    describeTab.setAttribute("aria-selected", String(d));
    filesTab.setAttribute("aria-selected", String(!d));
    describeTab.tabIndex = d ? 0 : -1;
    filesTab.tabIndex = d ? -1 : 0;
    describePane.hidden = !d;
    filesPane.hidden = d;
    if (focus) (d ? description : chooseBtn).focus();
  };
  describeTab.addEventListener("click", () => setMode("describe"));
  filesTab.addEventListener("click", () => setMode("files"));
  seg.addEventListener("keydown", (e) => {
    if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      e.preventDefault();
      setMode(state.mode === "describe" ? "files" : "describe");
      (state.mode === "describe" ? describeTab : filesTab).focus();
    }
  });
  const renderTabs = () => {
    const chars = state.description.trim().length;
    describeTab.replaceChildren(...["Describe it", chars ? el("span", { class: "nr-dot", title: "Has text" }, el("span", { class: "sr-only", text: "(has text)" })) : null].filter(Boolean));
    filesTab.replaceChildren(...["Add files", state.files.length
      ? el("span", { class: "nr-count" }, String(state.files.length), el("span", { class: "sr-only", text: state.files.length === 1 ? " file" : " files" }))
      : null].filter(Boolean));
  };

  description.addEventListener("input", () => {
    state.description = description.value;
    update();
  });

  // ------------------------------------------------------------------ files
  const addFiles = (list) => {
    const errors = [];
    for (const file of list) {
      const e = ext(file.name);
      if (!ACCEPT.includes(e)) {
        errors.push(`${file.name}: only .md, .txt and .pdf files work.`);
        continue;
      }
      if (file.size > MAX_BYTES) {
        errors.push(`${file.name} is ${fmtBytes(file.size)}; the limit is 20 MB.`);
        continue;
      }
      if (file.size === 0) {
        errors.push(`${file.name} is empty.`);
        continue;
      }
      if (state.files.some((f) => f.file.name === file.name && f.file.size === file.size)) continue;
      if (state.files.length + (state.description.trim() ? 1 : 0) >= MAX_FILES) {
        errors.push(`At most ${MAX_FILES} files.`);
        break;
      }
      state.files.push({ file });
    }
    state.fileErrors = errors;
    renderFiles();
    update();
  };

  const renderFiles = () => {
    fileList.replaceChildren(...state.files.map((entry, i) =>
      el("div", { class: "file-row", role: "listitem" },
        el("span", { class: "ext", text: ext(entry.file.name).slice(1) }),
        el("span", { class: "name", title: entry.file.name, text: entry.file.name }),
        el("span", { class: "num dim", text: fmtBytes(entry.file.size) }),
        el("button", { class: "icon-btn", type: "button", "aria-label": `Remove ${entry.file.name}`,
          onclick: () => { state.files.splice(i, 1); renderFiles(); update(); chooseBtn.focus(); } }, icon("x", "icon sm")))));
    fileErr.textContent = state.fileErrors.join(" ");
  };

  fileInput.addEventListener("change", () => {
    addFiles([...fileInput.files]);
    fileInput.value = "";
  });
  let dragDepth = 0;
  dropzone.addEventListener("dragenter", (e) => {
    e.preventDefault();
    dragDepth++;
    dropzone.classList.add("over");
  });
  dropzone.addEventListener("dragover", (e) => {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
  });
  dropzone.addEventListener("dragleave", () => {
    dragDepth = Math.max(0, dragDepth - 1);
    if (!dragDepth) dropzone.classList.remove("over");
  });
  dropzone.addEventListener("drop", (e) => {
    e.preventDefault();
    dragDepth = 0;
    dropzone.classList.remove("over");
    if (e.dataTransfer && e.dataTransfer.files) addFiles([...e.dataTransfer.files]);
  });
  // a file dropped anywhere on the form goes to the files pane; elsewhere it must not navigate away
  const swallow = (e) => {
    if (e.dataTransfer && [...(e.dataTransfer.types || [])].includes("Files")) e.preventDefault();
  };
  const formDrop = (e) => {
    if (dropzone.contains(e.target) || !e.dataTransfer || !e.dataTransfer.files || !e.dataTransfer.files.length) return;
    e.preventDefault();
    setMode("files");
    addFiles([...e.dataTransfer.files]);
  };
  form.addEventListener("drop", formDrop);
  window.addEventListener("dragover", swallow);
  window.addEventListener("drop", swallow);

  // ------------------------------------------------------------------ question
  question.addEventListener("input", () => {
    state.question = question.value;
    update();
  });

  // ------------------------------------------------------------------ events
  const renderEvents = () => {
    eventsList.replaceChildren();
    if (!state.events.length) {
      eventsList.appendChild(el("p", { class: "faint nr-none", text: "No breaking news planned." }));
    }
    state.events.forEach((ev, i) => {
      const r = el("input", { class: "input num-input", type: "number", min: "1", max: state.rounds, value: ev.round,
        "aria-label": `News ${i + 1}: round` });
      const t = el("input", { class: "input", value: ev.content, maxlength: "2000",
        placeholder: "e.g. A major outlet reports the price will double", "aria-label": `News ${i + 1}: text` });
      r.addEventListener("input", () => { ev.round = parseInt(r.value, 10); update(); });
      t.addEventListener("input", () => { ev.content = t.value; update(); });
      eventsList.appendChild(el("div", { class: "event-row" },
        el("div", { class: "nr-ev-round" }, el("span", { class: "dim", text: "Round" }), r),
        t,
        el("button", { class: "icon-btn", type: "button", "aria-label": `Remove news ${i + 1}`,
          onclick: () => { state.events.splice(i, 1); renderEvents(); update(); addEventBtn.focus(); } }, icon("x", "icon sm"))));
    });
  };
  addEventBtn.addEventListener("click", () => {
    const used = new Set(state.events.map((e) => e.round));
    let round = Math.min(state.rounds, Math.max(1, Math.ceil(state.rounds / 2)));
    while (used.has(round) && round < state.rounds) round++;
    state.events.push({ round, content: "" });
    renderEvents();
    update();
    const inputs = eventsList.querySelectorAll(".event-row .input:not(.num-input)");
    if (inputs.length) inputs[inputs.length - 1].focus();
  });

  // ------------------------------------------------------------------ derived state
  const preset = () => PRESETS.find((p) => p.agents === state.agents && p.rounds === state.rounds) || null;
  const plannedEvents = () => state.events.filter((e) => e.content.trim());
  const badEvent = () => plannedEvents().find((e) => !Number.isInteger(e.round) || e.round < 1 || e.round > state.rounds) || null;

  /** Why Start is not available yet: [message, element to focus], or null when ready. */
  const blocker = () => {
    const q = state.question.trim();
    if (!q) return ["Add a question", question];
    if (q.length < MIN_QUESTION) return ["Make the question a little longer", question];
    const chars = state.description.trim().length;
    if (!state.files.length && !chars) return ["Describe it or add a file", state.mode === "files" ? chooseBtn : description];
    if (!state.files.length && chars < MIN_DESCRIBE) return [`Describe it in a few more words (${MIN_DESCRIBE}+ characters)`, description];
    if (badEvent()) return [`Breaking news must fall in rounds 1 to ${state.rounds}`, eventsList.querySelector(".num-input")];
    return null;
  };

  const whereLine = () => {
    if (state.fake) return "Uses the fake model.";
    const models = (m) => m || "default model";
    const where = (id) => {
      const rt = runtimeById(id);
      const local = !rt || rt.kind === "cli" || id === "ollama" || /^(127\.|localhost|\[::1\])/.test(String(rt.path || "").replace(/^https?:\/\//, ""));
      return { name: runtimeName(id), local };
    };
    const a = where(state.agent_runtime), p = where(state.planner_runtime);
    const think = state.think ? ", extended thinking" : "";
    if (state.agent_runtime === state.planner_runtime) {
      const ms = state.agent_model === state.planner_model ? models(state.agent_model) : `${models(state.agent_model)} + ${models(state.planner_model)}`;
      return `${a.local ? "Runs locally with" : "Uses"} ${a.name} (${ms}${think}).`;
    }
    const verb = a.local && p.local ? "Runs locally with" : "Uses";
    return `${verb} ${a.name} (${models(state.agent_model)}) and ${p.name} (${models(state.planner_model)}${think}).`;
  };

  const renderSummary = () => {
    const p = preset();
    const time = p ? `about ${p.minutes} min` : aboutMinutes(state);
    const news = plannedEvents().length;
    summary.replaceChildren(
      el("b", { text: `${state.agents} people react over ${state.rounds} round${state.rounds === 1 ? "" : "s"}` }),
      `, ${time}${news ? `, with ${news} piece${news === 1 ? "" : "s"} of breaking news` : ""}. `,
      whereLine());
  };

  const renderReadiness = () => {
    readyNote.hidden = true;
    if (state.fake || !state.runtimes) return;
    const ids = [...new Set([state.agent_runtime, state.planner_runtime])];
    for (const id of ids) {
      const rt = runtimeById(id);
      if (!rt) continue;
      let lead = null;
      if (rt.installed === false) lead = `${rt.name} is not available on this machine.`;
      else if (rt.auth === "missing") lead = `${rt.name} is not signed in.`;
      if (!lead) continue;
      readyNote.replaceChildren(icon("info", "icon sm"),
        el("div", {}, el("b", { text: `${lead} ` }), ...withCode(rt.auth_hint || ""),
          " ", el("button", { class: "linkish", type: "button", text: "Pick another runtime",
            onclick: () => { setAdvanced(true); document.getElementById(`nr-${id === state.agent_runtime ? "agent" : "planner"}_runtime`)?.focus(); } })));
      readyNote.hidden = false;
      return;
    }
  };

  const update = () => {
    const p = preset();
    for (const [id, input] of presetInputs) {
      input.checked = !!p && p.id === id;
      input.closest(".nr-preset").classList.toggle("on", input.checked);
    }
    customTag.hidden = !!p;
    customTag.textContent = p ? "" : `Custom: ${state.agents} people · ${state.rounds} rounds`;
    eventsList.querySelectorAll(".event-row .num-input").forEach((n) => n.setAttribute("max", state.rounds));

    const chars = state.description.trim().length;
    descHint.textContent = chars && chars < MIN_DESCRIBE
      ? `${chars} of at least ${MIN_DESCRIBE} characters. Sent to the simulation as ${DESCRIPTION_NAME}.`
      : `A few sentences is enough. Sent to the simulation as ${DESCRIPTION_NAME}.`;
    renderTabs();
    renderSummary();
    renderReadiness();

    const block = blocker();
    reason.textContent = block && !state.submitting ? block[0] : "";
    submitBtn.setAttribute("aria-disabled", block || state.submitting ? "true" : "false");
    if (state.attempted) {
      question.setAttribute("aria-invalid", block && block[1] === question ? "true" : "false");
      description.setAttribute("aria-invalid", block && block[1] === description ? "true" : "false");
    }
    writeStore(PREFS_KEY, JSON.stringify({
      batch: state.batch, concurrency: state.concurrency, language: state.language, think: state.think,
      ...(fromSettings.has("agent_runtime") ? {} : { agent_runtime: state.agent_runtime }),
      ...(fromSettings.has("planner_runtime") ? {} : { planner_runtime: state.planner_runtime }),
      ...(fromSettings.has("agent_model") ? {} : { agent_model: state.agent_model }),
      ...(fromSettings.has("planner_model") ? {} : { planner_model: state.planner_model }),
    }));
  };

  // ------------------------------------------------------------------ submit
  const submit = async () => {
    if (state.submitting) return;
    state.attempted = true;
    formErr.hidden = true;
    const block = blocker();
    if (block) {
      update();
      const target = block[1];
      if (target) {
        if (target === description) setMode("describe");
        if (target === chooseBtn) setMode("files");
        if (eventsList.contains(target)) setAdvanced(true);
        target.focus();
      }
      return;
    }
    const fd = new FormData();
    const text = state.description.trim();
    if (text) fd.append("files[]", new File([text + "\n"], DESCRIPTION_NAME, { type: "text/markdown" }), DESCRIPTION_NAME);
    for (const { file } of state.files) fd.append("files[]", file, file.name);
    fd.append("question", state.question.trim());
    fd.append("agents", String(state.agents));
    fd.append("rounds", String(state.rounds));
    fd.append("batch", String(state.batch));
    fd.append("concurrency", String(state.concurrency));
    fd.append("agent_runtime", state.agent_runtime);
    fd.append("planner_runtime", state.planner_runtime);
    if (state.agent_model) fd.append("agent_model", state.agent_model);
    if (state.planner_model) fd.append("planner_model", state.planner_model);
    fd.append("language", state.language || DEFAULTS.language);
    fd.append("seed", String(state.seed));
    fd.append("think", state.think ? "true" : "false");
    fd.append("events", JSON.stringify(plannedEvents().map((e) => ({ round: e.round, content: e.content.trim() }))));

    state.submitting = true;
    update();
    submitBtn.replaceChildren(el("span", { class: "spinner", "aria-hidden": "true" }), el("span", { class: "lbl", text: "Starting…" }));
    try {
      const res = await api.createRun(fd);
      if (!res || !res.id) throw new Error("The server did not return a run id.");
      toast("ok", "Simulation started", state.question.trim().slice(0, 120));
      if (onCreated) onCreated(res.id);
    } catch (err) {
      formErr.textContent = (err && err.message) || String(err);
      formErr.hidden = false;
      formErr.scrollIntoView({ block: "nearest" });
    } finally {
      state.submitting = false;
      if (submitBtn.isConnected) {
        submitBtn.replaceChildren(el("span", { class: "lbl", text: "Start simulation" }),
          el("kbd", { class: "nr-kbd", "aria-hidden": "true", text: isMac ? "⌘↩" : "Ctrl ↩" }));
        update();
      }
    }
  };

  form.addEventListener("submit", (e) => {
    e.preventDefault();
    submit();
  });
  form.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      submit();
    }
  });

  // ------------------------------------------------------------------ server facts: fake mode, runtimes, settings
  let alive = true;
  const renderPickers = () => {
    agentPicker.render();
    plannerPicker.render();
  };
  if (api && api.health) {
    api.health().then((h) => {
      if (!alive || !h || !h.fake_llm) return;
      state.fake = true;
      fakeNote.hidden = false;
      update();
    }).catch(() => {});
  }
  if (api && api.settings && fromSettings.size) {
    api.settings().then((s) => {
      if (!alive || !s) return;
      for (const k of [...fromSettings]) if (typeof s[k] === "string" && s[k]) state[k] = s[k];
      if (!cfg && saved.concurrency === undefined && Number.isInteger(s.concurrency)) state.concurrency = clampInt(s.concurrency, LIMITS.concurrency, state.concurrency);
      renderPickers();
      update();
    }).catch(() => {});
  }
  if (api && api.runtimes) {
    api.runtimes({ checkAuth: true }).then((list) => {
      if (!alive || !Array.isArray(list)) return;
      state.runtimes = list;
      renderPickers();
      update();
    }).catch(() => {});
  }

  setMode("describe");
  setAdvanced(advOpen, false);
  renderPickers();
  if (seedOthers.length) {
    addFiles(seedOthers);
    if (!seedText) setMode("files");
  }
  if (seedText) {
    seedText.text().then((t) => {
      if (!alive || state.description.trim()) return;
      description.value = t.replace(/\s+$/, "");
      state.description = description.value;
      update();
    }).catch(() => {});
  }
  renderFiles();
  renderEvents();
  update();
  setTimeout(() => {
    const target = state.question.trim() ? description : question;
    target.focus({ preventScroll: true });
    if (target === question) question.setSelectionRange(question.value.length, question.value.length);
  }, 0);

  return function unmount() {
    alive = false;
    window.removeEventListener("dragover", swallow);
    window.removeEventListener("drop", swallow);
  };
}
