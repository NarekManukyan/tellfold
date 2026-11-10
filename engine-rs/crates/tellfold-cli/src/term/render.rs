//! The console protocol: rendering to segments, measuring and line splitting (ports of `Console.render`,
//! `Console.render_lines`, `Console.print`, `Measurement.get` and the `Segment` line helpers), plus the small
//! renderables (`Padding`, `Align`, `Constrain`, `Group`, `Columns`).

use super::cells::{cell_len, set_cell_size};
use super::panel::Panel;
use super::style::Style;
use super::table::Table;
use super::text::{Justify, Overflow, Text};

/// A piece of text in one style (port of `rich.segment.Segment`, without control codes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Seg {
    pub text: String,
    pub style: Style,
}

impl Seg {
    pub fn new(text: String, style: Style) -> Self {
        Seg { text, style }
    }

    pub fn plain(text: &str) -> Self {
        Seg { text: text.to_string(), style: Style::null() }
    }

    pub fn line() -> Self {
        Seg::plain("\n")
    }
}

pub(crate) type Line = Vec<Seg>;

/// The parts of `ConsoleOptions` that change the output.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Options {
    pub max_width: usize,
    pub justify: Option<Justify>,
    pub overflow: Option<Overflow>,
    pub no_wrap: Option<bool>,
}

impl Options {
    pub fn new(width: usize) -> Self {
        Options { max_width: width, justify: None, overflow: None, no_wrap: None }
    }

    /// Port of `ConsoleOptions.update_width`: negative widths become 0.
    pub fn width(self, width: isize) -> Self {
        Options { max_width: width.max(0) as usize, ..self }
    }
}

/// A `(minimum, maximum)` cell width (port of `rich.measure.Measurement`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Measurement(pub isize, pub isize);

impl Measurement {
    pub fn normalize(self) -> Self {
        let Measurement(minimum, maximum) = self;
        let minimum = minimum.max(0).min(maximum);
        Measurement(minimum.max(0), minimum.max(maximum).max(0))
    }

    pub fn with_maximum(self, width: isize) -> Self {
        Measurement(self.0.min(width), self.1.min(width))
    }

    pub fn with_minimum(self, width: isize) -> Self {
        let width = width.max(0);
        Measurement(self.0.max(width), self.1.max(width))
    }

    pub fn clamp(self, min_width: Option<isize>, max_width: Option<isize>) -> Self {
        let mut m = self;
        if let Some(w) = min_width {
            m = m.with_minimum(w);
        }
        if let Some(w) = max_width {
            m = m.with_maximum(w);
        }
        m
    }
}

/// Horizontal alignment (rich's `AlignMethod`), for [`Align`] and panel titles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignMethod {
    Left,
    Center,
    Right,
}

/// Anything rich can render, for the renderables this port supports.
#[derive(Clone, Debug)]
pub(crate) enum Renderable {
    Text(Text),
    Padding(Box<Padding>),
    Align(Box<Align>),
    Constrain(Box<Renderable>, Option<usize>),
    Group(Vec<Renderable>),
    Columns(Vec<Renderable>),
    Table(Box<Table>),
    Panel(Box<Panel>),
}

/// Port of `rich.padding.Padding` (style "none").
#[derive(Clone, Debug)]
pub(crate) struct Padding {
    pub renderable: Renderable,
    /// `(top, right, bottom, left)`.
    pub pad: (usize, usize, usize, usize),
    pub expand: bool,
}

/// Port of `rich.align.Align` (no vertical alignment, no style).
#[derive(Clone, Debug)]
pub(crate) struct Align {
    pub renderable: Renderable,
    pub align: AlignMethod,
    pub pad: bool,
}

impl Renderable {
    pub fn text(plain: &str) -> Self {
        Renderable::Text(Text::plain(plain))
    }

    pub fn padding(renderable: Renderable, pad: (usize, usize, usize, usize)) -> Self {
        Renderable::Padding(Box::new(Padding { renderable, pad, expand: true }))
    }
}

/// Port of `Console.render`.
pub(crate) fn render(renderable: &Renderable, options: Options) -> Vec<Seg> {
    if options.max_width < 1 {
        return Vec::new();
    }
    match renderable {
        Renderable::Text(text) => text.render(options.max_width, options.justify, options.overflow, options.no_wrap),
        Renderable::Padding(padding) => render_padding(padding, options),
        Renderable::Align(align) => render_align(align, options),
        Renderable::Constrain(child, width) => match width {
            None => render(child, options),
            Some(width) => render(child, options.width((*width).min(options.max_width) as isize)),
        },
        Renderable::Group(children) => children.iter().flat_map(|child| render(child, options)).collect(),
        Renderable::Columns(items) => render_columns(items, options),
        Renderable::Table(table) => table.render_segments(options),
        Renderable::Panel(panel) => panel.render_segments(options),
    }
}

/// Port of `Measurement.get`.
pub(crate) fn measure(renderable: &Renderable, options: Options) -> Measurement {
    let max_width = options.max_width as isize;
    if max_width < 1 {
        return Measurement(0, 0);
    }
    let measured = match renderable {
        // `Columns` has no `__rich_measure__`, so rich gives it the whole width.
        Renderable::Columns(_) => return Measurement(0, max_width),
        Renderable::Text(text) => {
            let (minimum, maximum) = text.measure();
            Measurement(minimum, maximum)
        }
        Renderable::Padding(padding) => {
            let (_, right, _, left) = padding.pad;
            let extra = (left + right) as isize;
            if max_width - extra < 1 {
                Measurement(max_width, max_width)
            } else {
                let Measurement(minimum, maximum) = measure(&padding.renderable, options);
                Measurement(minimum + extra, maximum + extra).with_maximum(max_width)
            }
        }
        Renderable::Align(align) => measure(&align.renderable, options),
        Renderable::Constrain(child, width) => match width {
            None => measure(child, options),
            Some(width) => measure(child, options.width(*width as isize)),
        },
        Renderable::Group(children) => measure_all(children, options),
        Renderable::Table(table) => table.measure(options),
        Renderable::Panel(panel) => panel.measure(options),
    };
    let width = measured.normalize().with_maximum(max_width);
    if width.1 < 1 {
        return Measurement(0, 0);
    }
    width.normalize()
}

/// Port of `measure_renderables`.
pub(crate) fn measure_all(renderables: &[Renderable], options: Options) -> Measurement {
    if renderables.is_empty() {
        return Measurement(0, 0);
    }
    let measurements: Vec<Measurement> = renderables.iter().map(|r| measure(r, options)).collect();
    Measurement(
        measurements.iter().map(|m| m.0).max().unwrap_or(0),
        measurements.iter().map(|m| m.1).max().unwrap_or(0),
    )
}

/// Cells a line takes (port of `Segment.get_line_length`).
pub(crate) fn line_length(line: &[Seg]) -> usize {
    line.iter().map(|s| cell_len(&s.text)).sum()
}

/// Crops or pads a line to `length` cells (port of `Segment.adjust_line_length`).
pub(crate) fn adjust_line_length(line: &[Seg], length: usize, style: Style, pad: bool) -> Line {
    let current = line_length(line);
    if current < length {
        let mut new_line = line.to_vec();
        if pad {
            new_line.push(Seg::new(" ".repeat(length - current), style));
        }
        new_line
    } else if current > length {
        let mut new_line = Vec::new();
        let mut used = 0;
        for segment in line {
            let size = cell_len(&segment.text);
            if used + size < length {
                new_line.push(segment.clone());
                used += size;
            } else {
                let chars: Vec<char> = segment.text.chars().collect();
                let text = set_cell_size(&chars, (length - used) as isize).into_iter().collect();
                new_line.push(Seg::new(text, segment.style));
                break;
            }
        }
        new_line
    } else {
        line.to_vec()
    }
}

/// Splits segments at newlines (port of `Segment.split_lines`, and of `split_and_crop_lines` when `crop` is
/// given as `(length, pad style, pad)`).
/// The second value says whether the last line had no newline after it.
fn split_segments(segments: &[Seg], crop: Option<(usize, Style, bool)>) -> (Vec<Line>, bool) {
    let finish = |line: &[Seg]| match crop {
        Some((length, style, pad)) => adjust_line_length(line, length, style, pad),
        None => line.to_vec(),
    };
    let mut lines = Vec::new();
    let mut line: Line = Vec::new();
    for segment in segments {
        if segment.text.contains('\n') {
            let mut rest = segment.text.as_str();
            while !rest.is_empty() {
                let (piece, newline, tail) = match rest.find('\n') {
                    Some(i) => (&rest[..i], true, &rest[i + 1..]),
                    None => (rest, false, ""),
                };
                if !piece.is_empty() {
                    line.push(Seg::new(piece.to_string(), segment.style));
                }
                if newline {
                    lines.push(finish(&line));
                    line.clear();
                }
                rest = tail;
            }
        } else {
            line.push(segment.clone());
        }
    }
    let open_tail = !line.is_empty();
    if open_tail {
        lines.push(finish(&line));
    }
    (lines, open_tail)
}

/// Port of `Segment.split_and_crop_lines` (without the newlines).
pub(crate) fn split_and_crop_lines(segments: &[Seg], length: usize, style: Style, pad: bool) -> Vec<Line> {
    split_segments(segments, Some((length, style, pad))).0
}

/// Port of `Segment.apply_style` with a leading style.
pub(crate) fn apply_style(segments: Vec<Seg>, style: Style) -> Vec<Seg> {
    if style.is_null() {
        return segments;
    }
    segments.into_iter().map(|s| Seg { style: style + s.style, ..s }).collect()
}

/// Port of `Console.render_lines`.
pub(crate) fn render_lines(renderable: &Renderable, options: Options, style: Style, pad: bool) -> Vec<Line> {
    let rendered = apply_style(render(renderable, options), style);
    split_and_crop_lines(&rendered, options.max_width, style, pad)
}

/// Port of `Segment.set_shape` (no height cut, as in rich).
pub(crate) fn set_shape(lines: &[Line], width: usize, height: usize) -> Vec<Line> {
    let height = if height == 0 { lines.len() } else { height };
    let mut shaped: Vec<Line> = lines.iter().map(|l| adjust_line_length(l, width, Style::null(), true)).collect();
    while shaped.len() < height {
        shaped.push(vec![Seg::plain(&" ".repeat(width))]);
    }
    shaped
}

/// Writes segments as a terminal would get them: with escape codes when `color`, else the bare text (port of
/// `Console._render_buffer`).
pub(crate) fn to_ansi(segments: &[Seg], color: bool) -> String {
    let mut out = String::new();
    for segment in segments {
        if color && !segment.style.is_null() {
            out.push_str(&segment.style.render(&segment.text));
        } else {
            out.push_str(&segment.text);
        }
    }
    out
}

/// What `Console.print(renderable)` writes at a console width, the crop to the width included.
pub(crate) fn print(renderable: &Renderable, width: usize, color: bool) -> String {
    let segments = render(renderable, Options::new(width));
    let mut cropped = Vec::with_capacity(segments.len());
    let (lines, open_tail) = split_segments(&segments, Some((width, Style::null(), false)));
    let count = lines.len();
    for (i, line) in lines.into_iter().enumerate() {
        cropped.extend(line);
        if i + 1 < count || !open_tail {
            cropped.push(Seg::line());
        }
    }
    to_ansi(&cropped, color)
}

fn spaces(count: usize) -> Seg {
    Seg::plain(&" ".repeat(count))
}

/// Port of `Padding.__rich_console__`.
fn render_padding(padding: &Padding, options: Options) -> Vec<Seg> {
    let (top, right, bottom, left) = padding.pad;
    let width = if padding.expand {
        options.max_width
    } else {
        ((measure(&padding.renderable, options).1 + (left + right) as isize).max(0) as usize).min(options.max_width)
    };
    let child = options.width(width as isize - (left + right) as isize);
    let lines = render_lines(&padding.renderable, child, Style::null(), true);
    let mut out = Vec::new();
    let blank = Seg::plain(&format!("{}\n", " ".repeat(width)));
    for _ in 0..top {
        out.push(blank.clone());
    }
    for line in lines {
        if left > 0 {
            out.push(spaces(left));
        }
        out.extend(line);
        if right > 0 {
            out.push(spaces(right));
        }
        out.push(Seg::line());
    }
    for _ in 0..bottom {
        out.push(blank.clone());
    }
    out
}

/// Port of `Align.__rich_console__` (no vertical alignment, no style).
fn render_align(align: &Align, options: Options) -> Vec<Seg> {
    let width = measure(&align.renderable, options).1.max(0) as usize;
    let constrained = Renderable::Constrain(Box::new(align.renderable.clone()), Some(width));
    let lines = split_segments(&render(&constrained, options), None).0;
    let width = lines.iter().map(|l| line_length(l)).max().unwrap_or(0);
    let lines = set_shape(&lines, width, lines.len());
    let excess = options.max_width as isize - width as isize;
    let mut out = Vec::new();
    for line in lines {
        if excess <= 0 {
            out.extend(line);
        } else {
            let excess = excess as usize;
            match align.align {
                AlignMethod::Left => {
                    out.extend(line);
                    if align.pad {
                        out.push(spaces(excess));
                    }
                }
                AlignMethod::Center => {
                    let left = excess / 2;
                    if left > 0 {
                        out.push(spaces(left));
                    }
                    out.extend(line);
                    if align.pad {
                        out.push(spaces(excess - left));
                    }
                }
                AlignMethod::Right => {
                    out.push(spaces(excess));
                    out.extend(line);
                }
            }
        }
        out.push(Seg::line());
    }
    out
}

/// Port of `Columns.__rich_console__` with its defaults: padding `(0, 1)`, no expand, no equal widths, left to
/// right, no alignment and no title.
fn render_columns(items: &[Renderable], options: Options) -> Vec<Seg> {
    if items.is_empty() {
        return Vec::new();
    }
    let width_padding = 1usize;
    let max_width = options.max_width;
    let item_widths: Vec<usize> = items.iter().map(|r| measure(r, options).1.max(0) as usize).collect();
    let count = items.len();
    let mut column_count = count;
    while column_count > 1 {
        // The widths in reading order, padded with empty cells to whole rows.
        let mut laid_out = item_widths.clone();
        if count % column_count != 0 {
            laid_out.extend(std::iter::repeat(0).take(column_count - count % column_count));
        }
        let mut widths: Vec<usize> = Vec::new();
        let mut column_no = 0;
        let mut too_wide = false;
        for width in laid_out {
            if column_no == widths.len() {
                widths.push(0);
            }
            widths[column_no] = widths[column_no].max(width);
            let total_width = widths.iter().sum::<usize>() + width_padding * (widths.len() - 1);
            if total_width > max_width {
                column_count = widths.len() - 1;
                too_wide = true;
                break;
            }
            column_no = (column_no + 1) % column_count;
        }
        if !too_wide {
            break;
        }
    }
    let mut cells: Vec<Renderable> = items.to_vec();
    if count % column_count != 0 {
        cells.extend(std::iter::repeat(Renderable::text("")).take(column_count - count % column_count));
    }
    let mut table = Table::grid((0, 1, 0, 1), true, false);
    for row in cells.chunks(column_count) {
        table.push_row(row.to_vec());
    }
    render(&Renderable::Table(Box::new(table)), options)
}
