//! typer's help and usage error screens (port of `typer/rich_utils.py`: `rich_format_help`,
//! `_print_options_panel`, `_print_commands_panel`, `_get_help_text`, `_get_parameter_help`,
//! `_make_command_help` and `rich_format_error`, in rich markup mode), plain text only.

use super::panel::Panel;
use super::render::{print, Align, AlignMethod, Renderable};
use super::table::{Column, Table};
use super::text::{py_strip, Overflow, Text};

/// One argument or option as typer lists it, the strings already derived from the click parameter the way
/// `_print_options_panel` does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParamHelp {
    /// Shows the `*` column (`param.required`).
    pub required: bool,
    /// Long option names joined by ",", e.g. `"--question"`.
    pub long: String,
    /// Short option names joined by ",", e.g. `"-q"`; for an argument, its metavar name.
    pub short: String,
    /// Long secondary (negative flag) names, e.g. `"--no-watch-stdin"`.
    pub secondary_long: String,
    /// Short secondary names.
    pub secondary_short: String,
    /// The metavar and range text, e.g. `"<int range> [2<=x<=200]"`; empty for flags.
    pub types: String,
    /// The raw help text (paragraphs separated by blank lines; console markup allowed).
    pub help: String,
    /// The text inside `[default: ...]`, when typer shows a default.
    pub default: Option<String>,
    /// The text inside `[env var: ...]`, when typer shows it (`show_envvar` and an env var).
    pub envvar: Option<String>,
}

/// What `--help` shows for one command or group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandHelp {
    /// The usage line, e.g. `"Usage: tellfold run [OPTIONS] {seeds}..."`.
    pub usage: String,
    /// The command's full help text (its docstring).
    pub help: String,
    pub arguments: Vec<ParamHelp>,
    pub options: Vec<ParamHelp>,
    /// Sub-commands as `(name, help)`, for a group.
    pub commands: Vec<(String, String)>,
}

/// Python's `str.expandtabs()` (tab size 8; columns restart after `\n` and `\r`).
fn expandtabs(text: &str) -> String {
    let mut out = String::new();
    let mut column = 0;
    for c in text.chars() {
        match c {
            '\t' => {
                let spaces = 8 - column % 8;
                out.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            }
            '\n' | '\r' => {
                out.push(c);
                column = 0;
            }
            _ => {
                out.push(c);
                column += 1;
            }
        }
    }
    out
}

/// Port of `inspect.cleandoc` (Python 3.13: only spaces count as indentation).
pub(crate) fn cleandoc(doc: &str) -> String {
    let expanded = expandtabs(doc);
    let mut lines: Vec<String> = expanded.split('\n').map(str::to_string).collect();
    let margin = lines
        .iter()
        .skip(1)
        .filter_map(|line| {
            let content = line.trim_start_matches(' ').chars().count();
            (content > 0).then(|| line.chars().count() - content)
        })
        .min();
    if let Some(first) = lines.first_mut() {
        *first = first.trim_start_matches(' ').to_string();
    }
    if let Some(margin) = margin {
        for line in lines.iter_mut().skip(1) {
            *line = line.chars().skip(margin).collect();
        }
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    while lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
    lines.join("\n")
}

/// Port of `_make_rich_text` in rich markup mode.
fn rich_text(text: &str) -> Text {
    Text::from_markup(&cleandoc(text))
}

/// Port of `_get_help_text`: the first paragraph on one line, then the rest after a blank line.
fn help_texts(help: &str) -> Vec<Renderable> {
    let cleaned = cleandoc(help);
    let help_text = cleaned.split('\x0c').next().unwrap_or("");
    let mut paragraphs = help_text.split("\n\n");
    let mut first = paragraphs.next().unwrap_or("").to_string();
    if !first.starts_with('\x08') {
        first = first.replace('\n', " ");
    }
    let mut out = vec![Renderable::Text(rich_text(py_strip(&first)))];
    let rest: Vec<&str> = paragraphs.collect();
    if !rest.is_empty() {
        out.push(Renderable::text(""));
        out.push(Renderable::Text(rich_text(&rest.join("\n\n"))));
    }
    out
}

/// Port of `_get_parameter_help`: help, env var, default and required as `Columns`.
fn parameter_help(param: &ParamHelp) -> Renderable {
    let mut items = Vec::new();
    if !param.help.is_empty() {
        let paragraphs: Vec<String> = param
            .help
            .split("\n\n")
            .map(|p| {
                if p.starts_with('\x08') {
                    format!("{}\n", p.trim_matches(|c| c == '\x08' || c == '\n'))
                } else {
                    py_strip(&p.replace('\n', " ")).to_string()
                }
            })
            .collect();
        items.push(Renderable::Text(rich_text(py_strip(&paragraphs.join("\n")))));
    }
    if let Some(envvar) = &param.envvar {
        items.push(Renderable::Text(Text::plain(&format!("[env var: {envvar}]"))));
    }
    if let Some(default) = &param.default {
        items.push(Renderable::Text(Text::plain(&format!("[default: {default}]"))));
    }
    if param.required {
        items.push(Renderable::text("[required]"));
    }
    Renderable::Columns(items)
}

/// The grid typer puts inside its panels: no header, no box, padding `(0, 1)` without edge padding.
fn panel_table() -> Table {
    Table::new().show_header(false).expand(true).box_chars(None).pad_edge(false).padding((0, 1, 0, 1))
}

fn titled_panel(table: Table, title: &str) -> Renderable {
    Renderable::Panel(Box::new(
        Panel::of(Renderable::Table(Box::new(table))).title(title).title_align(AlignMethod::Left),
    ))
}

/// Port of `_print_options_panel`.
fn options_panel(name: &str, params: &[ParamHelp], width: usize) -> String {
    if params.is_empty() {
        return String::new();
    }
    let any_required = params.iter().any(|p| p.required);
    let mut table = panel_table();
    for param in params {
        let mut row = Vec::new();
        if any_required {
            row.push(Renderable::text(if param.required { "*" } else { "" }));
        }
        for name in [&param.long, &param.short, &param.secondary_long, &param.secondary_short] {
            row.push(Renderable::text(name));
        }
        row.push(Renderable::Text(Text::plain(&param.types).with_overflow(Overflow::Fold)));
        row.push(parameter_help(param));
        table.push_row(row);
    }
    print(&titled_panel(table, name), width, false)
}

/// Port of `_make_command_help`: the first paragraph, line breaks kept (rich markup mode).
fn command_help(help: &str) -> Text {
    let cleaned = cleandoc(help);
    let mut first = cleaned.split("\n\n").next().unwrap_or("").to_string();
    if first.starts_with('\x08') {
        first = first.replace("\x08\n", "");
    }
    rich_text(py_strip(&first))
}

/// Port of `_print_commands_panel`.
fn commands_panel(commands: &[(String, String)], cmd_len: usize, width: usize) -> String {
    if commands.is_empty() {
        return String::new();
    }
    let mut table = panel_table()
        .with_column(Column::new("").no_wrap(true).width(cmd_len))
        .with_column(Column::new("Description").ratio(10));
    for (name, help) in commands {
        table.push_row(vec![Renderable::text(name), Renderable::Text(command_help(help))]);
    }
    print(&titled_panel(table, "Commands"), width, false)
}

/// What `tellfold ... --help` prints at a console width (port of `rich_format_help`), including the empty line
/// click's `echo` of the (empty) help string adds at the end.
pub fn render_help(help: &CommandHelp, width: usize) -> String {
    let mut out = print(&Renderable::padding(Renderable::text(&help.usage), (1, 1, 1, 1)), width, false);
    if !help.help.is_empty() {
        let align =
            Align { renderable: Renderable::Group(help_texts(&help.help)), align: AlignMethod::Left, pad: false };
        out.push_str(&print(&Renderable::padding(Renderable::Align(Box::new(align)), (0, 1, 1, 1)), width, false));
    }
    out.push_str(&options_panel("Arguments", &help.arguments, width));
    out.push_str(&options_panel("Options", &help.options, width));
    let cmd_len = help.commands.iter().map(|(name, _)| name.chars().count()).max().unwrap_or(0);
    out.push_str(&commands_panel(&help.commands, cmd_len, width));
    out.push('\n');
    out
}

/// What typer prints to stderr for a usage error (port of `rich_format_error`): the usage line, the
/// "Try ... --help" hint and the message in an "Error" panel. An empty `usage` means the error had no click
/// context, which prints the panel alone.
pub fn render_error(usage: &str, command_path: &str, message: &str, width: usize) -> String {
    let mut out = String::new();
    if !usage.is_empty() {
        out.push_str(&print(&Renderable::Text(Text::plain(usage).for_print()), width, false));
        let hint = format!("Try '{command_path} --help' for help.");
        out.push_str(&print(&Renderable::Text(Text::plain(&hint).for_print()), width, false));
    }
    let panel = Panel::new(Text::plain(message)).title("Error").title_align(AlignMethod::Left);
    out.push_str(&print(&Renderable::Panel(Box::new(panel)), width, false));
    out
}
