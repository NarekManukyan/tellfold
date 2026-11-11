//! `--help` screens and usage errors for the commands in [`crate::spec`], in typer's layout (rendered by
//! [`crate::term::render_help`] and [`crate::term::render_error`]).

use crate::args::{describe_range, Kind, Param, UsageError};
use crate::spec::{Command, APP_HELP};
use crate::term::{render_error, render_help, CommandHelp, ParamHelp};

const PROG: &str = "tellfold";
const HELP_OPTION: &str = "Show this message and exit.";

fn help_option() -> ParamHelp {
    ParamHelp { long: "--help".into(), help: HELP_OPTION.into(), ..Default::default() }
}

/// One parameter the way typer's `_print_options_panel` lists it.
fn param_help(param: &Param) -> ParamHelp {
    let names = if param.is_argument { vec![param.name] } else { param.opts.clone() };
    let (long, short): (Vec<&str>, Vec<&str>) = names.iter().partition(|n| n.contains("--"));
    let (secondary_long, secondary_short): (Vec<&str>, Vec<&str>) =
        param.secondary.iter().partition(|n| n.contains("--"));
    let mut types = match param.kind {
        Kind::Flag => String::new(),
        _ => param.metavar.to_string(),
    };
    if let (Kind::Int { min, max }, false) = (param.kind, param.is_argument) {
        if min.is_some() || max.is_some() {
            types.push_str(&format!(" [{}]", describe_range(min, max)));
        }
    }
    ParamHelp {
        required: param.required,
        long: long.join(","),
        short: short.join(","),
        secondary_long: secondary_long.join(","),
        secondary_short: secondary_short.join(","),
        types,
        help: param.help.into(),
        default: param.default_help.map(str::to_string),
        envvar: (!param.envvar.is_empty()).then(|| param.envvar.join(", ")),
    }
}

fn usage(command: Option<&Command>) -> String {
    match command {
        None => format!("Usage: {PROG} [OPTIONS] COMMAND [ARGS]..."),
        Some(c) if c.usage_args.is_empty() => format!("Usage: {PROG} {} [OPTIONS]", c.name),
        Some(c) => format!("Usage: {PROG} {} [OPTIONS] {}", c.name, c.usage_args),
    }
}

/// What `tellfold --help` prints.
pub fn group_help(commands: &[Command]) -> String {
    render_help(&group_spec_help(commands), crate::term::help_width())
}

pub fn group_spec_help(commands: &[Command]) -> CommandHelp {
    CommandHelp {
        usage: usage(None),
        help: APP_HELP.into(),
        arguments: Vec::new(),
        options: vec![
            ParamHelp { long: "--version".into(), help: "Show the version and exit.".into(), ..Default::default() },
            help_option(),
        ],
        commands: commands.iter().map(|c| (c.name.to_string(), c.help.to_string())).collect(),
    }
}

pub fn command_spec_help(command: &Command) -> CommandHelp {
    let mut options: Vec<ParamHelp> = command.params.iter().filter(|p| !p.is_argument).map(param_help).collect();
    options.push(help_option());
    CommandHelp {
        usage: usage(Some(command)),
        help: command.help.into(),
        arguments: command.params.iter().filter(|p| p.is_argument).map(param_help).collect(),
        options,
        commands: Vec::new(),
    }
}

/// What `tellfold <command> --help` prints.
pub fn command_help(command: &Command) -> String {
    render_help(&command_spec_help(command), crate::term::help_width())
}

/// What a usage error prints on stderr: the usage line and hint (unless click shows none) and the Error panel.
pub fn usage_error(command: Option<&Command>, error: &UsageError) -> String {
    let path = match command {
        None => PROG.to_string(),
        Some(c) => format!("{PROG} {}", c.name),
    };
    let usage = if error.show_usage { usage(command) } else { String::new() };
    render_error(&usage, &path, &error.message, crate::term::help_width())
}
