//! `--help` of every command against typer's (recorded by `golden/make_cli_help.py`): the parameter lists derived
//! from `spec.rs` must be the ones typer derives from `cli.py`, and the screens must match at every width.

use std::path::Path;

use serde::Deserialize;
use tellfold_cli::help::{command_spec_help, group_spec_help};
use tellfold_cli::spec::{commands, RUST_ONLY};
use tellfold_cli::term::{render_help, CommandHelp, ParamHelp};

#[derive(Deserialize)]
struct Param {
    required: bool,
    long: String,
    short: String,
    secondary_long: String,
    secondary_short: String,
    types: String,
    help: String,
    default: Option<String>,
    envvar: Option<String>,
}

#[derive(Deserialize)]
struct Data {
    usage: String,
    help: String,
    arguments: Vec<Param>,
    options: Vec<Param>,
    #[serde(default)]
    commands: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct Case {
    command: String,
    data: Data,
    renders: std::collections::BTreeMap<String, String>,
}

fn param(p: Param) -> ParamHelp {
    ParamHelp {
        required: p.required,
        long: p.long,
        short: p.short,
        secondary_long: p.secondary_long,
        secondary_short: p.secondary_short,
        types: p.types,
        help: p.help,
        default: p.default,
        envvar: p.envvar,
    }
}

#[test]
fn every_help_screen_matches_typer() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../golden/fixtures/cli/help.json");
    let cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    // typer never had the Rust-only commands
    let all: Vec<_> = commands().into_iter().filter(|c| !RUST_ONLY.contains(&c.name)).collect();
    assert_eq!(cases.len(), all.len() + 1, "the group and every command");
    for case in cases {
        let want = CommandHelp {
            usage: case.data.usage,
            help: case.data.help,
            arguments: case.data.arguments.into_iter().map(param).collect(),
            options: case.data.options.into_iter().map(param).collect(),
            commands: case.data.commands,
        };
        let mine = if case.command.is_empty() {
            group_spec_help(&all)
        } else {
            command_spec_help(all.iter().find(|c| c.name == case.command).expect("a command of that name"))
        };
        assert_eq!(mine, want, "help of {:?}", case.command);
        for (width, text) in &case.renders {
            assert_eq!(&render_help(&mine, width.parse().unwrap()), text, "{:?} at {width}", case.command);
        }
    }
}
