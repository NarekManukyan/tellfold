//! Every command, option and argument of `tellfold`, with the names, checks, defaults and help texts of the typer
//! app in `tellfold/cli.py`.

use crate::args::{Kind, Param};

pub const APP_HELP: &str =
    "Seed documents in, a simulated social reaction and a prediction report out. Runs on your local Claude Code CLI.";

pub struct Command {
    pub name: &'static str,
    pub help: &'static str,
    /// What follows `[OPTIONS]` in the usage line.
    pub usage_args: &'static str,
    pub params: Vec<Param>,
}

const STR: Kind = Kind::Str;
const FLAG: Kind = Kind::Flag;
const PATH: Kind = Kind::Path { exists: false, file_okay: true, dir_okay: true };
const EXISTING: Kind = Kind::Path { exists: true, file_okay: true, dir_okay: true };
const RUN_DIR: Kind = Kind::Path { exists: true, file_okay: false, dir_okay: true };

const fn range(min: i64, max: Option<i64>) -> Kind {
    Kind::Int { min: Some(min), max }
}

fn arg(name: &'static str, kind: Kind, help: &'static str, metavar: &'static str) -> Param {
    Param::argument(name, kind, help, metavar)
}

fn opt(name: &'static str, opts: &[&'static str], kind: Kind, help: &'static str) -> Param {
    let metavar = match kind {
        Kind::Str => "<str>",
        Kind::Int { min: None, max: None } => "<int>",
        Kind::Int { .. } => "<int range>",
        Kind::Path { file_okay: false, .. } => "<directory>",
        Kind::Path { .. } => "<path>",
        Kind::Flag => "<boolean>",
    };
    Param::option(name, opts, kind, help, metavar)
}

fn flag(name: &'static str, opts: &[&'static str], help: &'static str) -> Param {
    opt(name, opts, FLAG, help).quiet_default("false")
}

fn run_dir(help: &'static str) -> Param {
    arg("run_dir", RUN_DIR, help, "<directory>")
}

fn model(help: &'static str) -> Param {
    opt("model", &["--model"], STR, help)
}

fn open_report() -> Param {
    flag("open_report", &["--open"], "Open report.html when done.")
}

pub fn commands() -> Vec<Command> {
    vec![
        Command {
            name: "run",
            help: "Run a full simulation: ingest, graph, personas, rounds, report.",
            usage_args: "{seeds}...",
            params: vec![
                arg("seeds", EXISTING, "Seed files (.pdf .md .txt) or folders.", "<path>").many(),
                opt("question", &["--question", "-q"], STR, "What you want predicted.").required(),
                opt("agents", &["--agents"], range(2, Some(200)), "Number of simulated people.").default("20"),
                opt("rounds", &["--rounds"], range(1, Some(50)), "Simulation rounds.").default("6"),
                opt("batch", &["--batch"], range(1, Some(20)), "Agents decided per model call.").default("6"),
                opt("agent_model", &["--agent-model"], STR, "Model for per-round agent decisions.").default("haiku"),
                opt("planner_model", &["--planner-model"], STR, "Model for graph, personas and report.").default("sonnet"),
                opt("concurrency", &["--concurrency"], range(1, Some(16)), "Parallel Claude CLI calls.").default("4"),
                opt("event", &["--event"], STR, "Breaking news injected at a round, as ROUND:TEXT. Repeatable.").many(),
                opt("out", &["--out"], PATH, "Folder that holds run directories.").default("runs"),
                opt("language", &["--language"], STR, "Language agents post and the report is written in.")
                    .default("English"),
                opt("seed", &["--seed"], Kind::Int { min: None, max: None }, "RNG seed for activity sampling and feed tie-breaks.")
                    .default("7"),
                flag(
                    "think",
                    &["--think"],
                    "Extended thinking for graph, personas and report: more thorough, several times slower.",
                ),
                open_report(),
                opt(
                    "agent_runtime",
                    &["--agent-runtime"],
                    STR,
                    "Runtime for agent decisions and interviews (see `tellfold runtimes`). Default: settings, else claude-code.",
                ),
                opt(
                    "planner_runtime",
                    &["--planner-runtime"],
                    STR,
                    "Runtime for graph, personas and report (see `tellfold runtimes`). Default: settings, else claude-code.",
                ),
                opt(
                    "cast_from",
                    &["--cast-from"],
                    RUN_DIR,
                    "Reuse the topic map and people of this run (in the --out folder) and compare the two: no graph or persona calls. Both runs join a study; see `tellfold compare`.",
                ),
                opt(
                    "variant",
                    &["--variant"],
                    STR,
                    "Label of this run in its study, e.g. \"Draft B\". Default: the next letter.",
                ),
            ],
        },
        Command {
            name: "compare",
            help: "Compare the runs of a study (made with --cast-from) and write studies/<id>/compare.json.",
            usage_args: "{run_dir}",
            params: vec![run_dir("A run that belongs to a study."), model("Override the planner model.")],
        },
        Command {
            name: "check",
            help: "Check it: run RUN_DIR again with the same people and text but other seeds, then say what held.",
            usage_args: "{run_dir}",
            params: vec![
                run_dir("A finished run with a report."),
                opt("runs", &["--runs"], range(2, Some(5)), "Runs in all, this one included.").default("3"),
                opt("agent_model", &["--agent-model"], STR, "Agent model for the new runs (default: the run's)."),
            ],
        },
        Command {
            name: "branch",
            help: "What if...? Copy a run up to round AT, change what happens next, and continue it as a new run.",
            usage_args: "{run_dir}",
            params: vec![
                run_dir("The run to branch."),
                opt("at", &["--at"], range(0, None), "Keep rounds 0 to AT exactly as they happened.").required(),
                opt("event", &["--event"], STR, "Something that happens after AT, as ROUND:TEXT. Repeatable.").many(),
                flag("drop_events", &["--drop-events"], "Drop the run's own events after AT (by default they still happen)."),
                open_report(),
            ],
        },
        Command {
            name: "rewrite",
            help: "Suggest a better version of the run's text (rewrite.json). Test it with: tellfold run --cast-from RUN_DIR.",
            usage_args: "{run_dir}",
            params: vec![run_dir("A run with a report."), model("Override the planner model.")],
        },
        Command {
            name: "report",
            help: "Rebuild report.json / .html / .md from an existing (possibly interrupted) run.",
            usage_args: "{run_dir}",
            params: vec![run_dir("A run directory."), model("Override the planner model."), open_report()],
        },
        Command {
            name: "chat",
            help: "Interview one simulated agent, in character. Type 'exit' to leave.",
            usage_args: "{run_dir}",
            params: vec![
                run_dir("A run directory."),
                opt("agent", &["--agent", "-a"], STR, "Agent id (a03) or handle to interview.").required(),
                model("Override the model (default: the run's agent model)."),
            ],
        },
        Command {
            name: "ask",
            help: "Ask the report agent a one-shot question about the simulation.",
            usage_args: "{run_dir} {question}",
            params: vec![
                run_dir("A run directory."),
                arg("question", STR, "Your question about the outcome.", "<str>"),
                model("Override the planner model."),
            ],
        },
        Command {
            name: "agents",
            help: "Show the simulated personas with their reach and activity.",
            usage_args: "{run_dir}",
            params: vec![run_dir("A run directory.")],
        },
        Command {
            name: "ui",
            help: "Start Tellfold Studio, the local web UI for running, watching and replaying simulations.",
            usage_args: "",
            params: vec![
                opt("runs", &["--runs"], PATH, "Folder that holds run directories.").default("runs"),
                opt("port", &["--port"], range(1, Some(65535)), "Port to serve Tellfold Studio on (127.0.0.1 only).")
                    .default("8765"),
                opt("open_browser", &["--open"], FLAG, "Open Studio in the browser.")
                    .secondary(&["--no-open"])
                    .quiet_default("true")
                    .default_help("open"),
            ],
        },
        Command {
            name: "doctor",
            help: "Check the Claude Code CLI, the SDK, and that a tiny haiku call works.",
            usage_args: "",
            params: vec![],
        },
        Command {
            name: "runtimes",
            help: "Show the LLM runtimes detected on this machine (CLIs, Ollama, OpenAI-compatible servers, APIs).",
            usage_args: "",
            params: vec![
                flag("check_auth", &["--check-auth"], "Also check logins (slower; may call the tools or their APIs)."),
                flag("as_json", &["--json"], "Print JSON instead of a table."),
            ],
        },
        Command {
            name: "serve",
            help: "Headless API server for the Mac app: prints {\"port\": N} on stdout once listening, then logs to stderr.",
            usage_args: "",
            params: vec![
                opt(
                    "token",
                    &["--token"],
                    STR,
                    "Bearer token every /api request must carry. Prefer the TELLFOLD_SERVE_TOKEN env var (argv is visible in ps).",
                )
                .required()
                .envvar(&["TELLFOLD_SERVE_TOKEN", "SHOAL_SERVE_TOKEN"]),
                opt("port", &["--port"], range(0, Some(65535)), "Port on 127.0.0.1; 0 picks a free one.").default("0"),
                opt("runs", &["--runs"], PATH, "Folder that holds run directories (default: settings runs_dir, else ./runs)."),
                opt(
                    "examples",
                    &["--examples"],
                    PATH,
                    "Folder of read-only example runs to list next to the runs (the Mac app passes its bundled examples).",
                ),
                opt("log_level", &["--log-level"], STR, "debug, info, warning or error (logs go to stderr).").default("warning"),
                opt("watch_stdin", &["--watch-stdin"], FLAG, "Exit when stdin closes (the Mac app keeps it open).")
                    .secondary(&["--no-watch-stdin"])
                    .quiet_default("true")
                    .default_help("watch-stdin"),
            ],
        },
    ]
}
