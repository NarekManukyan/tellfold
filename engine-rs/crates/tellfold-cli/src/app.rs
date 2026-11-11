//! `tellfold [OPTIONS] COMMAND [ARGS]...`: the group (`--version`, `--help`), the command lookup with typer's
//! suggestions, the `shoal` alias, and how usage errors and exits are printed.

use crate::args::{get_close_matches, parse, Parsed, UsageError};
use crate::commands::{self, Done, Exit};
use crate::help;
use crate::spec::{commands as all_commands, Command};
use crate::ui::Console;
use crate::VERSION;

const PROG: &str = "tellfold";

/// What `tellfold` does with `args` (without the program name). Returns the exit code.
pub fn main(argv0: &str, args: &[String]) -> i32 {
    let name = std::path::Path::new(argv0).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name == "shoal" {
        Console::stderr()
            .print("[yellow]`shoal` is now `tellfold`. The old name still works for now; please switch.[/]");
    }
    let out = Console::stdout();
    let commands = all_commands();
    if args.is_empty() {
        out.write(&help::group_help(&commands));
        return 2;
    }
    // The group's own options come before the command: click stops at the first argument that is not an option.
    let mut index = 0;
    // eager options act in the order they were given
    let mut first_eager: Option<&str> = None;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            break;
        }
        match arg.as_str() {
            "--version" | "--help" => {
                first_eager.get_or_insert(arg.as_str());
            }
            _ => {
                let (opt, value) = arg.split_once('=').map_or((arg.as_str(), None), |(o, v)| (o, Some(v)));
                let error = if value.is_some() && (opt == "--version" || opt == "--help") {
                    UsageError::bare(format!("Option '{opt}' does not take a value."))
                } else {
                    UsageError::new(group_no_such_option(arg))
                };
                return usage_error(None, error);
            }
        }
        index += 1;
    }
    match first_eager {
        Some("--help") => {
            out.write(&help::group_help(&commands));
            return 0;
        }
        Some(_) => {
            out.print(&format!("{PROG} {VERSION}"));
            return 0;
        }
        None => {}
    }
    let Some(command_name) = args.get(index) else {
        return usage_error(None, UsageError::new("Missing command."));
    };
    if command_name.starts_with('-') {
        // only after `--`: typer parses it as an option of the group
        return usage_error(None, UsageError::new(group_no_such_option(command_name)));
    }
    let Some(command) = commands.iter().find(|c| c.name == command_name) else {
        let mut message = format!("No such command {}.", crate::args::py_repr(command_name));
        let names: Vec<&str> = commands.iter().map(|c| c.name).collect();
        let matches = get_close_matches(command_name, &names, 3, 0.6);
        if !matches.is_empty() {
            let suggestions = matches.iter().map(|m| crate::args::py_repr(m)).collect::<Vec<_>>().join(", ");
            message = format!("{}. Did you mean {suggestions}?", message.trim_end_matches('.'));
        }
        return usage_error(None, UsageError::new(message));
    };
    let parsed = match parse(&command.params, &args[index + 1..]) {
        Ok(parsed) => parsed,
        Err(e) => return usage_error(Some(command), e),
    };
    if parsed.help {
        out.write(&help::command_help(command));
        return 0;
    }
    match dispatch(&out, command, &parsed) {
        Ok(()) => 0,
        Err(Exit::Code(code)) => code,
        Err(Exit::Usage(e)) => usage_error(Some(command), e),
        Err(Exit::Abort) => {
            let err = Console::stderr();
            err.write("\n");
            err.print("[red]Aborted.[/]");
            1
        }
    }
}

/// click's message for an unknown option of the group (whose only long options are `--version` and `--help`).
fn group_no_such_option(arg: &str) -> String {
    let opt = arg.split_once('=').map_or(arg, |(o, _)| o);
    if !opt.starts_with("--") {
        return format!("No such option: {}", crate::args::escape_control(opt));
    }
    let names = ["--version", "--help"];
    let matches = get_close_matches(opt, &names, 3, 0.6);
    if matches.is_empty() {
        return format!("No such option: {}", crate::args::escape_control(opt));
    }
    let mut sorted = matches.clone();
    sorted.sort();
    format!("No such option: {} (Possible options: {})", crate::args::escape_control(opt), sorted.join(", "))
}

fn usage_error(command: Option<&Command>, error: UsageError) -> i32 {
    let err = Console::stderr();
    err.write(&help::usage_error(command, &error));
    2
}

fn dispatch(con: &Console, command: &Command, p: &Parsed) -> Done {
    if !matches!(command.name, "serve" | "ui" | "mcp") {
        commands::catch_interrupts();
    }
    match command.name {
        "run" => commands::run(con, p),
        "compare" => commands::compare(con, p),
        "check" => commands::check(con, p),
        "branch" => commands::branch(con, p),
        "rewrite" => commands::rewrite(con, p),
        "report" => commands::report(con, p),
        "chat" => commands::chat(con, p),
        "ask" => commands::ask(con, p),
        "agents" => commands::agents(con, p),
        "doctor" => commands::doctor(con, p),
        "runtimes" => commands::runtimes(con, p),
        "serve" => crate::serve::serve(con, p),
        "ui" => crate::serve::ui(con, p),
        "mcp" => crate::mcp::mcp(con, p),
        other => unreachable!("no command {other}"),
    }
}
