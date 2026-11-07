// Shoal Studio: new-simulation form. Drag-and-drop seeds, validation, a live cost estimate
// (same formula as cli.estimate_calls) and planned god events.

const SVG_NS = "http://www.w3.org/2000/svg";
const ICONS = {
  upload: "M12 15V3m0 0L7 8m5-5 5 5M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4",
  x: "M18 6 6 18M6 6l12 12",
  plus: "M12 5v14M5 12h14",
  event: "M13 2 3 14h9l-1 8 10-12h-9l1-8z",
  file: "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6",
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

const ACCEPT = [".pdf", ".md", ".markdown", ".txt"];
const MAX_BYTES = 20 * 1024 * 1024;
const MAX_FILES = 20;
const MODELS = ["haiku", "sonnet", "opus"];
const PREFS_KEY = "shoal.newrun.prefs";
const LIMITS = {
  agents: [2, 200], rounds: [1, 50], batch: [1, 20], concurrency: [1, 16],
};

// Seconds per call, [optimistic, pessimistic], by model. Rough local-CLI figures.
const CALL_SECONDS = { haiku: [6, 18], sonnet: [14, 40], opus: [25, 75] };

function ext(name) {
  const m = /\.[^.]+$/.exec(name.toLowerCase());
  return m ? m[0] : "";
}

function fmtBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** Python's round(): halves go to the even neighbour. */
function pyRound(x) {
  const f = Math.floor(x);
  const diff = x - f;
  if (Math.abs(diff - 0.5) < 1e-9) return f % 2 === 0 ? f : f + 1;
  return Math.round(x);
}

/** Mirrors shoal.cli.estimate_calls(config, chunks). */
export function estimateCalls({ agents, rounds, batch }, chunks) {
  const graph = Math.ceil(chunks / 4);
  const personas = Math.ceil(agents / 10);
  const opening = Math.ceil(pyRound(0.3 * agents) / batch) + 1;
  const perRound = Math.ceil(agents / batch) + 1;
  return graph + personas + opening + rounds * perRound + 1;
}

/** Rough wall-clock range in minutes, given concurrency and the models chosen. */
export function estimateMinutes(cfg, chunks) {
  const agentT = CALL_SECONDS[cfg.agent_model] || CALL_SECONDS.sonnet;
  const plannerT = (CALL_SECONDS[cfg.planner_model] || CALL_SECONDS.sonnet).map((t) => t * (cfg.think ? 1.8 : 1));
  const conc = Math.max(1, cfg.concurrency);
  const waves = (calls) => Math.ceil(calls / conc);
  const batchScale = 0.7 + 0.05 * cfg.batch; // bigger batches mean longer answers
  const graphCalls = Math.ceil(chunks / 4);
  const personaCalls = Math.ceil(cfg.agents / 10);
  const openingBatches = Math.ceil(pyRound(0.3 * cfg.agents) / cfg.batch);
  const roundBatches = Math.ceil(cfg.agents / cfg.batch);
  const span = (i) =>
    waves(graphCalls) * plannerT[i] +
    personaCalls * plannerT[i] +
    (waves(openingBatches) * batchScale + 1) * agentT[i] +
    cfg.rounds * (waves(roundBatches) * batchScale + 1) * agentT[i] +
    plannerT[i] * 1.5; // report
  return [span(0) / 60, span(1) / 60];
}

function fmtMinutes([lo, hi]) {
  const r = (m) => (m < 10 ? Math.max(1, Math.round(m)) : Math.round(m / 5) * 5);
  if (hi < 1) return "under a minute";
  if (lo >= 90) return `roughly ${(lo / 60).toFixed(1)}-${(hi / 60).toFixed(1)} hours`;
  const a = r(lo), b = Math.max(r(hi), a + 1);
  return `roughly ${a}-${b} minutes`;
}

function loadPrefs() {
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    return raw ? JSON.parse(raw) || {} : {};
  } catch {
    return {};
  }
}

function savePrefs(p) {
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(p));
  } catch {
    /* storage may be unavailable */
  }
}

function toast(kind, title, body) {
  document.dispatchEvent(new CustomEvent("studio:toast", { detail: { kind, title, body } }));
}

export function mountNewRun(container, { api, onCreated } = {}) {
  const saved = loadPrefs();
  const clampInt = (v, [lo, hi], d) => {
    const n = parseInt(v, 10);
    return Number.isFinite(n) ? Math.max(lo, Math.min(hi, n)) : d;
  };
  const state = {
    files: [], // {file, chars}
    question: "",
    agents: clampInt(saved.agents, LIMITS.agents, 20),
    rounds: clampInt(saved.rounds, LIMITS.rounds, 6),
    batch: clampInt(saved.batch, LIMITS.batch, 6),
    agent_model: MODELS.includes(saved.agent_model) ? saved.agent_model : "haiku",
    planner_model: MODELS.includes(saved.planner_model) ? saved.planner_model : "sonnet",
    concurrency: clampInt(saved.concurrency, LIMITS.concurrency, 4),
    language: typeof saved.language === "string" && saved.language ? saved.language.slice(0, 40) : "English",
    think: !!saved.think,
    events: [], // {round, content}
    submitting: false,
    touched: false,
    fileErrors: [],
  };

  // ------------------------------------------------------------------ elements
  const fileInput = el("input", { type: "file", multiple: true, accept: ACCEPT.join(","), class: "sr-only", tabindex: "-1", "aria-hidden": "true" });
  const dropzone = el("div", { class: "dropzone", role: "button", tabindex: "0", "aria-describedby": "nr-files-hint",
    "aria-label": "Add seed documents: drop files here or press Enter to browse" },
    icon("upload"),
    el("div", {}, el("strong", { text: "Drop seed documents here" }), " or click to browse"),
    el("div", { id: "nr-files-hint", class: "hint", text: "PDF, Markdown or plain text · up to 20 MB each · at most 20 files" }));
  const fileList = el("div", { class: "files", role: "list", "aria-label": "Selected seed files" });
  const fileErr = el("div", { class: "err", role: "alert" });

  const question = el("textarea", { class: "textarea", id: "nr-question", rows: "3", maxlength: "2000",
    placeholder: "What do you want to predict? e.g. How will developers react if the editor moves its AI features behind a paid tier?" });
  const questionErr = el("div", { class: "err", role: "alert" });

  const slider = (key, label, hint) => {
    const [lo, hi] = LIMITS[key];
    const range = el("input", { type: "range", min: lo, max: hi, step: "1", value: state[key], id: `nr-${key}`, "aria-describedby": `nr-${key}-hint` });
    const num = el("input", { class: "input num-input", type: "number", min: lo, max: hi, step: "1", value: state[key],
      style: "width:76px;min-height:28px;height:28px;padding:2px 8px", "aria-label": `${label} (number)` });
    const sync = (v, from) => {
      const n = clampInt(v, LIMITS[key], state[key]);
      state[key] = n;
      if (from !== range) range.value = n;
      if (from !== num) num.value = n;
      update();
    };
    range.addEventListener("input", () => sync(range.value, range));
    num.addEventListener("change", () => sync(num.value, num));
    num.addEventListener("input", () => {
      const n = parseInt(num.value, 10);
      if (Number.isFinite(n) && n >= lo && n <= hi) {
        state[key] = n;
        range.value = n;
        update();
      }
    });
    return el("div", { class: "field slider-field" },
      el("div", { class: "top" }, el("label", { for: `nr-${key}`, text: label }), num),
      range,
      el("div", { class: "ticks" }, el("span", { text: String(lo) }), el("span", { id: `nr-${key}-hint`, class: "hint", text: hint }), el("span", { text: String(hi) })));
  };

  const numberField = (key, label, hint) => {
    const [lo, hi] = LIMITS[key];
    const input = el("input", { class: "input num-input", type: "number", id: `nr-${key}`, min: lo, max: hi, step: "1", value: state[key] });
    input.addEventListener("change", () => {
      state[key] = clampInt(input.value, LIMITS[key], state[key]);
      input.value = state[key];
      update();
    });
    input.addEventListener("input", () => {
      const n = parseInt(input.value, 10);
      if (Number.isFinite(n) && n >= lo && n <= hi) {
        state[key] = n;
        update();
      }
    });
    return el("div", { class: "field" }, el("label", { for: `nr-${key}`, text: label }), input, el("div", { class: "hint", text: hint }));
  };

  const modelSelect = (key, label, hint) => {
    const sel = el("select", { class: "select", id: `nr-${key}` },
      MODELS.map((m) => el("option", { value: m, text: m, selected: state[key] === m })));
    sel.addEventListener("change", () => {
      state[key] = sel.value;
      update();
    });
    return el("div", { class: "field" }, el("label", { for: `nr-${key}`, text: label }), sel, el("div", { class: "hint", text: hint }));
  };

  const language = el("input", { class: "input", id: "nr-language", value: state.language, maxlength: "40", autocomplete: "off" });
  language.addEventListener("input", () => {
    state.language = language.value.trim() || "English";
    update();
  });
  const think = el("input", { type: "checkbox", id: "nr-think", checked: state.think });
  think.addEventListener("change", () => {
    state.think = think.checked;
    update();
  });

  const eventsList = el("div", { class: "events-list" });
  const eventsErr = el("div", { class: "err", role: "alert" });
  const addEventBtn = el("button", { class: "btn btn-sm", type: "button" }, icon("plus", "icon sm"), "Add event");

  const estimateBig = el("div", { class: "big" });
  const estimateSub = el("div", { class: "dim" });
  const summaryDl = el("dl");
  const formErr = el("div", { class: "form-error", role: "alert", hidden: true });
  const fakeNotice = el("div", { class: "notice", hidden: true,
    text: "This server runs with the fake model (SHOAL_FAKE_LLM). Runs finish fast with placeholder text." });
  const submitBtn = el("button", { class: "btn btn-primary btn-lg", type: "submit" }, "Start simulation");
  const submitHint = el("div", { class: "hint", style: "text-align:center" });
  submitHint.append("or press ", el("kbd", { text: navigator.platform && /Mac/i.test(navigator.platform) ? "⌘" : "Ctrl" }), " ", el("kbd", { text: "Enter" }));
  submitHint.classList.add("kbd-hint");

  const form = el("form", { class: "newrun", novalidate: true, "aria-label": "New simulation" },
    el("div", { class: "newrun-main" },
      el("section", { class: "card" },
        el("div", { class: "card-head" }, el("h2", { text: "Seed documents" }), el("span", { class: "dim", style: "font-size:12px", text: "What the agents know about" })),
        el("div", { class: "card-body" }, dropzone, fileInput, fileList, fileErr)),
      el("section", { class: "card" },
        el("div", { class: "card-head" }, el("h2", { text: "Question" })),
        el("div", { class: "card-body" },
          el("div", { class: "field" },
            el("label", { for: "nr-question", text: "What should the simulation predict?" }),
            question,
            el("div", { class: "hint", text: "Ask about a reaction or an outcome. The report answers this directly and cites posts." }),
            questionErr))),
      el("section", { class: "card" },
        el("div", { class: "card-head" }, el("h2", { text: "Population and rounds" })),
        el("div", { class: "card-body" },
          el("div", { class: "grid-2" },
            slider("agents", "Agents", "personas in the crowd"),
            slider("rounds", "Rounds", "time steps after the opening")),
          el("div", { class: "grid-3" },
            numberField("batch", "Batch size", "agents decided per model call"),
            numberField("concurrency", "Concurrency", "parallel model calls"),
            el("div", { class: "field" }, el("label", { for: "nr-language", text: "Language" }), language, el("div", { class: "hint", text: "what agents write in" }))))),
      el("section", { class: "card" },
        el("div", { class: "card-head" }, el("h2", { text: "Models" })),
        el("div", { class: "card-body" },
          el("div", { class: "grid-2" },
            modelSelect("agent_model", "Agent model", "per-round decisions, many calls"),
            modelSelect("planner_model", "Planner model", "graph, personas and report")),
          el("label", { class: "switch", for: "nr-think" }, think,
            el("span", {}, "Extended thinking ", el("span", { class: "dim", text: "for graph, personas and report (slower, more thorough)" }))))),
      el("section", { class: "card" },
        el("div", { class: "card-head" }, el("h2", { text: "Planned events" }), addEventBtn),
        el("div", { class: "card-body" },
          el("p", { class: "dim", style: "font-size:13px", text: "Breaking news shown to every agent at the start of a round. You can also inject events while the run is live." }),
          eventsList, eventsErr))),
    el("aside", { class: "summary card", "aria-label": "Run summary" },
      el("div", { class: "card-head" }, el("h2", { text: "Summary" })),
      el("div", { class: "card-body" },
        el("div", { class: "estimate", "aria-live": "polite" }, estimateBig, estimateSub),
        summaryDl,
        fakeNotice,
        formErr,
        submitBtn,
        submitHint)));

  container.replaceChildren(form);

  // ------------------------------------------------------------------ files
  const addFiles = async (list) => {
    const errors = [];
    for (const file of list) {
      const e = ext(file.name);
      if (!ACCEPT.includes(e)) {
        errors.push(`${file.name}: only PDF, Markdown and text files are supported.`);
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
      if (state.files.length >= MAX_FILES) {
        errors.push(`At most ${MAX_FILES} files.`);
        break;
      }
      const entry = { file, chars: null };
      state.files.push(entry);
      if (e !== ".pdf") {
        file.text().then((t) => {
          entry.chars = t.length;
          update();
        }).catch(() => {});
      }
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
          onclick: () => { state.files.splice(i, 1); renderFiles(); update(); dropzone.focus(); } }, icon("x", "icon sm")))));
  };

  dropzone.addEventListener("click", () => fileInput.click());
  dropzone.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      fileInput.click();
    }
  });
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
  // a drop that misses the zone should not navigate away from the form
  const swallow = (e) => {
    if (e.dataTransfer && [...(e.dataTransfer.types || [])].includes("Files")) e.preventDefault();
  };
  window.addEventListener("dragover", swallow);
  window.addEventListener("drop", swallow);

  // ------------------------------------------------------------------ question
  question.addEventListener("input", () => {
    state.question = question.value;
    if (state.touched) validate();
  });

  // ------------------------------------------------------------------ events
  const renderEvents = () => {
    eventsList.replaceChildren();
    if (!state.events.length) {
      eventsList.appendChild(el("p", { class: "faint", style: "font-size:13px", text: "No planned events." }));
    }
    state.events.forEach((ev, i) => {
      const r = el("input", { class: "input num-input", type: "number", min: "1", max: state.rounds, value: ev.round, "aria-label": `Event ${i + 1} round` });
      const t = el("input", { class: "input", value: ev.content, maxlength: "2000", placeholder: "e.g. A major outlet reports the price will double", "aria-label": `Event ${i + 1} text` });
      r.addEventListener("input", () => { ev.round = parseInt(r.value, 10); if (state.touched) validate(); updateSummary(); });
      t.addEventListener("input", () => { ev.content = t.value; if (state.touched) validate(); updateSummary(); });
      const row = el("div", { class: "event-row" },
        el("div", { style: "display:flex;align-items:center;gap:6px" }, el("span", { class: "dim", style: "font-size:12px", text: "R" }), r),
        t,
        el("button", { class: "icon-btn", type: "button", "aria-label": `Remove event ${i + 1}`,
          onclick: () => { state.events.splice(i, 1); renderEvents(); update(); addEventBtn.focus(); } }, icon("x", "icon sm")));
      eventsList.appendChild(row);
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

  // ------------------------------------------------------------------ validation & estimate
  const chunksEstimate = () => {
    let chars = 0;
    for (const { file, chars: c } of state.files) {
      if (c !== null) chars += c;
      else chars += ext(file.name) === ".pdf" ? file.size * 0.3 : file.size; // PDFs: rough text yield
    }
    return Math.max(1, Math.ceil(chars / 1600));
  };

  const validate = () => {
    let ok = true;
    const q = state.question.trim();
    if (!q) {
      questionErr.textContent = "Write the question the simulation should answer.";
      ok = false;
    } else if (q.length < 8) {
      questionErr.textContent = "Make the question a little more specific.";
      ok = false;
    } else questionErr.textContent = "";
    question.setAttribute("aria-invalid", questionErr.textContent ? "true" : "false");

    const fe = [...state.fileErrors];
    if (!state.files.length && state.touched) {
      fe.unshift("Add at least one seed document.");
      ok = false;
    }
    fileErr.textContent = fe.join(" ");
    dropzone.setAttribute("aria-invalid", !state.files.length && state.touched ? "true" : "false");

    const evErrs = [];
    state.events.forEach((ev, i) => {
      const rowInputs = eventsList.querySelectorAll(".event-row")[i];
      const rIn = rowInputs && rowInputs.querySelector(".num-input");
      const tIn = rowInputs && rowInputs.querySelector(".input:not(.num-input)");
      const badRound = !Number.isInteger(ev.round) || ev.round < 1 || ev.round > state.rounds;
      const badText = !ev.content.trim();
      if (rIn) rIn.setAttribute("aria-invalid", badRound ? "true" : "false");
      if (tIn) tIn.setAttribute("aria-invalid", badText && state.touched ? "true" : "false");
      if (badRound) evErrs.push(`Event ${i + 1}: round must be between 1 and ${state.rounds}.`);
      if (badText && state.touched) evErrs.push(`Event ${i + 1}: add the news text or remove the event.`);
    });
    eventsErr.textContent = evErrs.join(" ");
    if (evErrs.length) ok = false;
    return ok && state.files.length > 0;
  };

  const updateSummary = () => {
    const chunks = chunksEstimate();
    const cfg = { ...state };
    const calls = estimateCalls(cfg, chunks);
    estimateBig.textContent = `~${calls.toLocaleString()} model calls`;
    estimateSub.textContent = `${fmtMinutes(estimateMinutes(cfg, chunks))}` +
      (state.files.length ? "" : " · add seeds for a firmer number");
    const row = (k, v) => [el("dt", { text: k }), el("dd", { text: v })];
    const events = state.events.filter((e) => e.content.trim()).length;
    summaryDl.replaceChildren(
      ...row("Seeds", state.files.length ? `${state.files.length} · ~${chunks} chunks` : "none"),
      ...row("Agents × rounds", `${state.agents} × ${state.rounds}`),
      ...row("Calls per round", String(Math.ceil(state.agents / state.batch) + 1)),
      ...row("Models", `${state.agent_model} / ${state.planner_model}${state.think ? " +think" : ""}`),
      ...row("Planned events", String(events)));
  };

  const update = () => {
    // keep event round inputs bounded by the round count
    eventsList.querySelectorAll(".event-row .num-input").forEach((n) => n.setAttribute("max", state.rounds));
    updateSummary();
    if (state.touched) validate();
    savePrefs({
      agents: state.agents, rounds: state.rounds, batch: state.batch, agent_model: state.agent_model,
      planner_model: state.planner_model, concurrency: state.concurrency, language: state.language, think: state.think,
    });
  };

  // ------------------------------------------------------------------ submit
  const submit = async () => {
    if (state.submitting) return;
    state.touched = true;
    formErr.hidden = true;
    if (!validate()) {
      const firstBad = form.querySelector('[aria-invalid="true"]');
      if (firstBad) firstBad.focus();
      return;
    }
    const fd = new FormData();
    for (const { file } of state.files) fd.append("files[]", file, file.name);
    fd.append("question", state.question.trim());
    fd.append("agents", String(state.agents));
    fd.append("rounds", String(state.rounds));
    fd.append("batch", String(state.batch));
    fd.append("agent_model", state.agent_model);
    fd.append("planner_model", state.planner_model);
    fd.append("concurrency", String(state.concurrency));
    fd.append("language", state.language || "English");
    fd.append("think", state.think ? "true" : "false");
    fd.append("events", JSON.stringify(state.events
      .filter((e) => e.content.trim())
      .map((e) => ({ round: e.round, content: e.content.trim() }))));

    state.submitting = true;
    submitBtn.disabled = true;
    submitBtn.replaceChildren(el("span", { class: "spinner" }), "Uploading and starting…");
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
      submitBtn.disabled = false;
      submitBtn.replaceChildren("Start simulation");
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

  // ------------------------------------------------------------------ health hint
  if (api && api.health) {
    api.health().then((h) => { if (h && h.fake_llm) fakeNotice.hidden = false; }).catch(() => {});
  }

  renderFiles();
  renderEvents();
  update();
  setTimeout(() => question.focus({ preventScroll: true }), 0);

  return function unmount() {
    window.removeEventListener("dragover", swallow);
    window.removeEventListener("drop", swallow);
  };
}
