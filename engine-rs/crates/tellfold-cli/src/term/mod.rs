//! Terminal rendering: the subset of rich (and typer's help layout) the CLI prints.
//!
//! A port of rich 15.0.0 and typer 0.28.0, matching their output byte for byte when there is no colour, at any
//! width: [`Text`] wrapping, [`Table`], [`Panel`], typer's `--help` screens ([`render_help`]) and usage errors
//! ([`render_error`]). With `color` the same segments are written with rich's ANSI codes; styles cover the
//! attributes and the 16 standard colours. Not ported: emoji codes in markup, rich's automatic highlighting of
//! printed strings (numbers, paths, ...), and colour in typer's help.

mod cell_widths;
mod cells;
mod help;
mod markup;
mod panel;
mod render;
mod style;
mod table;
mod text;
mod width;

pub use cells::cell_len;
pub use help::{render_error, render_help, CommandHelp, ParamHelp};
pub use panel::Panel;
pub use render::AlignMethod;
pub use style::{Color, Style};
pub use table::{BoxChars, Column, Table, ASCII, HEAVY, HEAVY_HEAD, ROUNDED, SQUARE};
pub use text::{Justify, Overflow, Text};
pub use width::{console_height, console_width, help_width, is_terminal};

use render::{print, render, to_ansi, Options, Renderable};

/// What `Console.print(text)` writes at `width` cells: word wrapped (fold), cropped to the width,
/// newline-terminated, with ANSI styles when `color`.
pub fn render_text(text: &Text, width: usize, color: bool) -> String {
    print(&Renderable::Text(text.for_print()), width, color)
}

/// What `Console.print(text, soft_wrap=True)` writes: no wrapping or cropping (tabs still expand),
/// newline-terminated.
pub fn render_text_soft(text: &Text, color: bool) -> String {
    let options = Options { no_wrap: Some(true), overflow: Some(Overflow::Ignore), ..Options::new(80) };
    to_ansi(&render(&Renderable::Text(text.for_print()), options), color)
}
