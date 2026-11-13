// Tellfold Studio: fetch wrappers for the local HTTP API and an auto-reconnecting EventSource helper.

export class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

const enc = encodeURIComponent;

function errorMessage(status, body) {
  if (body && typeof body === "object") {
    const d = body.detail ?? body.error ?? body.message;
    if (typeof d === "string") return d;
    if (Array.isArray(d) && d.length) {
      // FastAPI validation errors: [{loc, msg, type}]
      return d.map((e) => (e && e.msg ? `${(e.loc || []).slice(1).join(".") || "request"}: ${e.msg}` : String(e))).join("; ");
    }
  }
  if (typeof body === "string" && body.trim()) return body.trim().slice(0, 300);
  if (status === 0) return "Cannot reach the Tellfold server. Is `tellfold ui` still running?";
  return `Request failed (HTTP ${status})`;
}

async function request(method, path, { json, form, signal } = {}) {
  const init = { method, headers: { Accept: "application/json" }, signal };
  if (json !== undefined) {
    init.headers["Content-Type"] = "application/json";
    init.body = JSON.stringify(json);
  } else if (form !== undefined) {
    init.body = form; // the browser sets the multipart boundary
  }
  let res;
  try {
    res = await fetch(path, init);
  } catch (err) {
    if (err && err.name === "AbortError") throw err;
    throw new ApiError(0, errorMessage(0));
  }
  const type = res.headers.get("content-type") || "";
  let body = null;
  try {
    body = type.includes("application/json") ? await res.json() : await res.text();
  } catch {
    body = null;
  }
  if (!res.ok) throw new ApiError(res.status, errorMessage(res.status, body));
  return body;
}

export const api = {
  health: () => request("GET", "/api/health"),
  settings: () => request("GET", "/api/settings"),
  runtimes: ({ checkAuth = false } = {}) => request("GET", `/api/runtimes?check_auth=${checkAuth ? 1 : 0}`),
  runs: () => request("GET", "/api/runs"),
  run: (id) => request("GET", `/api/runs/${enc(id)}`),
  posts: (id, sinceId = 0) => request("GET", `/api/runs/${enc(id)}/posts?since_id=${Number(sinceId) || 0}`),
  actions: (id, round) =>
    request("GET", `/api/runs/${enc(id)}/actions` + (round === undefined || round === null ? "" : `?round=${Number(round)}`)),
  agent: (id, agentId) => request("GET", `/api/runs/${enc(id)}/agents/${enc(agentId)}`),
  createRun: (formData) => request("POST", "/api/runs", { form: formData }),
  seeds: (id) => request("GET", `/api/runs/${enc(id)}/seeds`),
  /** One saved seed document as a File, so a run can be started again with the same material. */
  seedFile: async (id, name) => {
    let res;
    try {
      res = await fetch(`/api/runs/${enc(id)}/seeds/${enc(name)}`);
    } catch {
      throw new ApiError(0, errorMessage(0));
    }
    if (!res.ok) throw new ApiError(res.status, `Could not read ${name} (HTTP ${res.status})`);
    const blob = await res.blob();
    return new File([blob], name, { type: blob.type || "" });
  },
  injectEvent: (id, round, content) =>
    request("POST", `/api/runs/${enc(id)}/events`, { json: { round: Number(round), content } }),
  stop: (id) => request("POST", `/api/runs/${enc(id)}/stop`),
  chat: (id, agentId, message, history = []) =>
    request("POST", `/api/runs/${enc(id)}/chat`, { json: { agent_id: agentId, message, history } }),
  ask: (id, question) => request("POST", `/api/runs/${enc(id)}/ask`, { json: { question } }),
  rebuildReport: (id) => request("POST", `/api/runs/${enc(id)}/report`),
  reportUrl: (id) => `/api/runs/${enc(id)}/report.html`,
};

const EVENT_NAMES = [
  "status", "stage", "round_start", "round_done", "posts_added", "batch_failed",
  "event_injected", "run_done", "run_failed", "run_stopped",
];

/**
 * Subscribe to a run's server-sent events.
 * handlers: {status, stage, round_start, round_done, posts_added, batch_failed, event_injected,
 *            run_done, run_failed, run_stopped, error, open}
 * Returns close(). Reconnects with exponential backoff (1 s .. 15 s) until closed.
 */
export function stream(id, handlers = {}) {
  let es = null;
  let closed = false;
  let timer = null;
  let delay = 1000;

  const call = (name, payload) => {
    const fn = handlers[name];
    if (typeof fn !== "function") return;
    try {
      fn(payload);
    } catch (err) {
      console.error(`[tellfold] stream handler "${name}" failed`, err);
    }
  };

  const connect = () => {
    if (closed) return;
    es = new EventSource(`/api/runs/${enc(id)}/stream`);
    es.onopen = () => {
      delay = 1000;
      call("open", {});
    };
    for (const name of EVENT_NAMES) {
      es.addEventListener(name, (ev) => {
        let data = {};
        try {
          data = ev.data ? JSON.parse(ev.data) : {};
        } catch {
          data = { raw: ev.data };
        }
        call(name, data);
      });
    }
    es.onerror = () => {
      if (closed) return;
      es.close();
      es = null;
      call("error", { retryInMs: delay });
      timer = setTimeout(connect, delay);
      delay = Math.min(delay * 2, 15000);
    };
  };

  connect();
  return function close() {
    closed = true;
    clearTimeout(timer);
    if (es) es.close();
    es = null;
  };
}
