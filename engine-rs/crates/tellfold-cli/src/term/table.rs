//! Tables (port of `rich/table.py`, `rich/box.py` and `rich/_ratio.py`).

use super::render::{
    measure, print, render, render_lines, set_shape, Line, Measurement, Options, Padding, Renderable, Seg,
};
use super::style::Style;
use super::text::{Justify, Overflow, Text};

/// Box drawing characters (port of `rich.box.Box`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxChars {
    rows: [[char; 4]; 8],
}

impl BoxChars {
    const fn new(rows: [[char; 4]; 8]) -> Self {
        BoxChars { rows }
    }

    /// The 8 rows of 4 characters, as in rich's box definition string.
    pub(crate) fn rows_of(&self) -> [[char; 4]; 8] {
        self.rows
    }

    /// The headerless equivalent of a box with header characters (port of `Box.get_plain_headed_box`).
    fn plain_headed(self) -> BoxChars {
        if self == HEAVY_HEAD {
            SQUARE
        } else {
            self
        }
    }

    fn top(&self, widths: &[usize]) -> String {
        let [left, horizontal, divider, right] = self.rows[0];
        edge_line(left, horizontal, divider, right, widths, true)
    }

    fn bottom(&self, widths: &[usize]) -> String {
        let [left, horizontal, divider, right] = self.rows[7];
        edge_line(left, horizontal, divider, right, widths, true)
    }

    /// Port of `Box.get_row`: `level` is 2 for "head", 4 for "row", 3 for "mid".
    fn row(&self, widths: &[usize], level: usize, edge: bool) -> String {
        let [left, horizontal, cross, right] = self.rows[level];
        let horizontal = if level == 3 { ' ' } else { horizontal };
        edge_line(left, horizontal, cross, right, widths, edge)
    }

    /// `(left, right, vertical)` of the head (0), mid (3) and foot (6) rows.
    fn verticals(&self, row: usize) -> (char, char, char) {
        let [left, _, vertical, right] = self.rows[row];
        (left, right, vertical)
    }
}

fn edge_line(left: char, horizontal: char, cross: char, right: char, widths: &[usize], edge: bool) -> String {
    let mut out = String::new();
    if edge {
        out.push(left);
    }
    for (i, &width) in widths.iter().enumerate() {
        out.extend(std::iter::repeat(horizontal).take(width));
        if i + 1 < widths.len() {
            out.push(cross);
        }
    }
    if edge {
        out.push(right);
    }
    out
}

/// `box.HEAVY_HEAD`, the default for tables.
pub const HEAVY_HEAD: BoxChars = BoxChars::new([
    ['┏', '━', '┳', '┓'],
    ['┃', ' ', '┃', '┃'],
    ['┡', '━', '╇', '┩'],
    ['│', ' ', '│', '│'],
    ['├', '─', '┼', '┤'],
    ['├', '─', '┼', '┤'],
    ['│', ' ', '│', '│'],
    ['└', '─', '┴', '┘'],
]);

/// `box.SQUARE`.
pub const SQUARE: BoxChars = BoxChars::new([
    ['┌', '─', '┬', '┐'],
    ['│', ' ', '│', '│'],
    ['├', '─', '┼', '┤'],
    ['│', ' ', '│', '│'],
    ['├', '─', '┼', '┤'],
    ['├', '─', '┼', '┤'],
    ['│', ' ', '│', '│'],
    ['└', '─', '┴', '┘'],
]);

/// `box.ROUNDED`, the default for panels.
pub const ROUNDED: BoxChars = BoxChars::new([
    ['╭', '─', '┬', '╮'],
    ['│', ' ', '│', '│'],
    ['├', '─', '┼', '┤'],
    ['│', ' ', '│', '│'],
    ['├', '─', '┼', '┤'],
    ['├', '─', '┼', '┤'],
    ['│', ' ', '│', '│'],
    ['╰', '─', '┴', '╯'],
]);

/// `box.HEAVY`.
pub const HEAVY: BoxChars = BoxChars::new([
    ['┏', '━', '┳', '┓'],
    ['┃', ' ', '┃', '┃'],
    ['┣', '━', '╋', '┫'],
    ['┃', ' ', '┃', '┃'],
    ['┣', '━', '╋', '┫'],
    ['┣', '━', '╋', '┫'],
    ['┃', ' ', '┃', '┃'],
    ['┗', '━', '┻', '┛'],
]);

/// `box.ASCII`.
pub const ASCII: BoxChars = BoxChars::new([
    ['+', '-', '-', '+'],
    ['|', ' ', '|', '|'],
    ['|', '-', '+', '|'],
    ['|', ' ', '|', '|'],
    ['|', '-', '+', '|'],
    ['|', '-', '+', '|'],
    ['|', ' ', '|', '|'],
    ['+', '-', '-', '+'],
]);

/// Python's `round()` of `a / b` (half to even).
fn py_round_div(a: i64, b: i64) -> i64 {
    ((a as f64) / (b as f64)).round_ties_even() as i64
}

/// Python's `ceil(a / b)` with float division.
fn py_ceil_div(a: i64, b: i64) -> i64 {
    ((a as f64) / (b as f64)).ceil() as i64
}

/// Port of `_ratio.ratio_reduce`.
pub(crate) fn ratio_reduce(total: i64, ratios: &[i64], maximums: &[i64], values: &[i64]) -> Vec<i64> {
    let ratios: Vec<i64> = ratios.iter().zip(maximums).map(|(&r, &m)| if m != 0 { r } else { 0 }).collect();
    let mut total_ratio: i64 = ratios.iter().sum();
    if total_ratio == 0 {
        return values.to_vec();
    }
    let mut total_remaining = total;
    let mut result = Vec::with_capacity(values.len());
    for ((&ratio, &maximum), &value) in ratios.iter().zip(maximums).zip(values) {
        if ratio != 0 && total_ratio > 0 {
            let distributed = maximum.min(py_round_div(ratio * total_remaining, total_ratio));
            result.push(value - distributed);
            total_remaining -= distributed;
            total_ratio -= ratio;
        } else {
            result.push(value);
        }
    }
    result
}

/// Port of `_ratio.ratio_distribute`. rich asserts the ratios sum to more than 0; this returns zeros instead.
pub(crate) fn ratio_distribute(total: i64, ratios: &[i64], minimums: Option<&[i64]>) -> Vec<i64> {
    let ratios: Vec<i64> = match minimums {
        Some(minimums) => ratios.iter().zip(minimums).map(|(&r, &m)| if m != 0 { r } else { 0 }).collect(),
        None => ratios.to_vec(),
    };
    let mut total_ratio: i64 = ratios.iter().sum();
    if total_ratio <= 0 {
        return vec![0; ratios.len()];
    }
    let mut total_remaining = total;
    let mut out = Vec::with_capacity(ratios.len());
    for (i, &ratio) in ratios.iter().enumerate() {
        let minimum = minimums.map_or(0, |m| m[i]);
        let distributed = if total_ratio > 0 {
            minimum.max(py_ceil_div(ratio * total_remaining, total_ratio))
        } else {
            total_remaining
        };
        out.push(distributed);
        total_ratio -= ratio;
        total_remaining -= distributed;
    }
    out
}

/// One table column (port of `rich.table.Column`, without footers or per-column styles).
#[derive(Clone, Debug)]
pub struct Column {
    pub(crate) header: Renderable,
    pub(crate) justify: Justify,
    pub(crate) overflow: Overflow,
    pub(crate) width: Option<usize>,
    pub(crate) min_width: Option<usize>,
    pub(crate) max_width: Option<usize>,
    pub(crate) ratio: Option<usize>,
    pub(crate) no_wrap: bool,
    pub(crate) cells: Vec<Renderable>,
}

impl Column {
    /// A column with rich's defaults: justify left, overflow ellipsis. The header is console markup, like a `str`
    /// header in rich.
    pub fn new(header: &str) -> Self {
        Column {
            header: Renderable::Text(Text::from_markup(header)),
            justify: Justify::Left,
            overflow: Overflow::Ellipsis,
            width: None,
            min_width: None,
            max_width: None,
            ratio: None,
            no_wrap: false,
            cells: Vec::new(),
        }
    }

    pub fn justify(mut self, justify: Justify) -> Self {
        self.justify = justify;
        self
    }

    pub fn overflow(mut self, overflow: Overflow) -> Self {
        self.overflow = overflow;
        self
    }

    /// A fixed width in cells (without padding).
    pub fn width(mut self, width: usize) -> Self {
        self.width = Some(width);
        self
    }

    pub fn min_width(mut self, width: usize) -> Self {
        self.min_width = Some(width);
        self
    }

    pub fn max_width(mut self, width: usize) -> Self {
        self.max_width = Some(width);
        self
    }

    /// A flexible share of the spare width (needs an expanding table).
    pub fn ratio(mut self, ratio: usize) -> Self {
        self.ratio = Some(ratio);
        self
    }

    pub fn no_wrap(mut self, no_wrap: bool) -> Self {
        self.no_wrap = no_wrap;
        self
    }

    fn flexible(&self) -> bool {
        self.ratio.is_some()
    }
}

/// A table (port of `rich.table.Table`; no footer, caption, row styles or leading).
#[derive(Clone, Debug)]
pub struct Table {
    columns: Vec<Column>,
    rows: usize,
    title: Option<Text>,
    title_justify: Justify,
    box_chars: Option<BoxChars>,
    padding: (usize, usize, usize, usize),
    collapse_padding: bool,
    pad_edge: bool,
    expand: bool,
    show_header: bool,
    show_edge: bool,
    show_lines: bool,
    width: Option<usize>,
    min_width: Option<usize>,
    header_style: Style,
    title_style: Style,
    border_style: Style,
}

impl Default for Table {
    fn default() -> Self {
        Table::new()
    }
}

impl Table {
    /// A table with rich's defaults (`box.HEAVY_HEAD`, padding `(0, 1)`, bold header, italic title), except
    /// that the title is left-justified, as every table of the CLI has it (rich's default is centred).
    pub fn new() -> Self {
        Table {
            columns: Vec::new(),
            rows: 0,
            title: None,
            title_justify: Justify::Left,
            box_chars: Some(HEAVY_HEAD),
            padding: (0, 1, 0, 1),
            collapse_padding: false,
            pad_edge: true,
            expand: false,
            show_header: true,
            show_edge: true,
            show_lines: false,
            width: None,
            min_width: None,
            header_style: Style::parse("table.header"),
            title_style: Style::parse("table.title"),
            border_style: Style::null(),
        }
    }

    /// Port of `Table.grid`.
    pub(crate) fn grid(padding: (usize, usize, usize, usize), collapse_padding: bool, pad_edge: bool) -> Self {
        Table {
            box_chars: None,
            padding,
            collapse_padding,
            pad_edge,
            show_header: false,
            show_edge: false,
            ..Table::new()
        }
    }

    /// The title, as console markup (like a `str` title in rich); `""` means none.
    pub fn title(mut self, title: &str) -> Self {
        self.title = (!title.is_empty()).then(|| Text::from_markup(title));
        self
    }

    pub fn title_justify(mut self, justify: Justify) -> Self {
        self.title_justify = justify;
        self
    }

    /// The box style, `None` for no lines.
    pub fn box_chars(mut self, box_chars: Option<BoxChars>) -> Self {
        self.box_chars = box_chars;
        self
    }

    /// Cell padding as `(top, right, bottom, left)`.
    pub fn padding(mut self, padding: (usize, usize, usize, usize)) -> Self {
        self.padding = padding;
        self
    }

    pub fn collapse_padding(mut self, collapse: bool) -> Self {
        self.collapse_padding = collapse;
        self
    }

    pub fn pad_edge(mut self, pad_edge: bool) -> Self {
        self.pad_edge = pad_edge;
        self
    }

    pub fn expand(mut self, expand: bool) -> Self {
        self.expand = expand;
        self
    }

    pub fn show_header(mut self, show: bool) -> Self {
        self.show_header = show;
        self
    }

    pub fn show_edge(mut self, show: bool) -> Self {
        self.show_edge = show;
        self
    }

    pub fn show_lines(mut self, show: bool) -> Self {
        self.show_lines = show;
        self
    }

    /// A fixed table width (implies expand).
    pub fn width(mut self, width: usize) -> Self {
        self.width = Some(width);
        self
    }

    pub fn min_width(mut self, width: usize) -> Self {
        self.min_width = Some(width);
        self
    }

    /// The border style, e.g. `"dim"`.
    pub fn border_style(mut self, style: &str) -> Self {
        self.border_style = Style::parse(style);
        self
    }

    /// The header row style (default `"table.header"`, bold).
    pub fn header_style(mut self, style: &str) -> Self {
        self.header_style = Style::parse(style);
        self
    }

    /// The title style (default `"table.title"`, italic).
    pub fn title_style(mut self, style: &str) -> Self {
        self.title_style = Style::parse(style);
        self
    }

    /// Adds a column with a header, a justify and an overflow method.
    pub fn column(self, header: &str, justify: Justify, overflow: Overflow) -> Self {
        self.with_column(Column::new(header).justify(justify).overflow(overflow))
    }

    /// Adds a column with a header (console markup) and a justify; overflow is rich's default, ellipsis.
    pub fn add_column(&mut self, header: &str, justify: Justify) {
        self.add_column_with(header, justify, Overflow::Ellipsis);
    }

    /// Adds a column with a header (console markup), a justify and an overflow method.
    pub fn add_column_with(&mut self, header: &str, justify: Justify, overflow: Overflow) {
        let column = Column::new(header).justify(justify).overflow(overflow);
        *self = std::mem::take(self).with_column(column);
    }

    /// Adds a column.
    pub fn with_column(mut self, mut column: Column) -> Self {
        column.cells = vec![Renderable::text(""); self.rows];
        self.columns.push(column);
        self
    }

    /// Adds a row; missing cells are blank (port of `Table.add_row`).
    pub fn row(mut self, cells: Vec<Text>) -> Self {
        self.add_row(cells);
        self
    }

    /// Adds a row; missing cells are blank (port of `Table.add_row`).
    pub fn add_row(&mut self, cells: Vec<Text>) {
        self.push_row(cells.into_iter().map(Renderable::Text).collect());
    }

    /// Port of `Table.add_row` for any renderables: extra cells add default columns.
    pub(crate) fn push_row(&mut self, mut cells: Vec<Renderable>) {
        while cells.len() < self.columns.len() {
            cells.push(Renderable::text(""));
        }
        for (index, cell) in cells.into_iter().enumerate() {
            if index == self.columns.len() {
                let mut column = Column::new("");
                column.cells = vec![Renderable::text(""); self.rows];
                self.columns.push(column);
            }
            self.columns[index].cells.push(cell);
        }
        self.rows += 1;
    }

    /// The number of rows.
    pub fn row_count(&self) -> usize {
        self.rows
    }

    /// What `Console.print(table)` writes at `width` cells, with ANSI styles when `color`.
    pub fn render(&self, width: usize, color: bool) -> String {
        print(&Renderable::Table(Box::new(self.clone())), width, color)
    }

    fn expands(&self) -> bool {
        self.expand || self.width.is_some()
    }

    fn extra_width(&self) -> isize {
        let mut width = 0;
        if self.box_chars.is_some() && self.show_edge {
            width += 2;
        }
        if self.box_chars.is_some() {
            width += self.columns.len() as isize - 1;
        }
        width
    }

    /// Port of `Table._get_padding_width`.
    fn padding_width(&self, column_index: usize) -> isize {
        let (_, mut pad_right, _, mut pad_left) = self.padding;
        if self.collapse_padding {
            pad_left = 0;
            pad_right = pad_right.abs_diff(pad_left);
        }
        if !self.pad_edge {
            if column_index == 0 {
                pad_left = 0;
            }
            if column_index + 1 == self.columns.len() {
                pad_right = 0;
            }
        }
        (pad_left + pad_right) as isize
    }

    /// The column's cells, header first, each wrapped in its padding (port of `Table._get_cells`).
    fn cells(&self, column_index: usize) -> Vec<Renderable> {
        let column = &self.columns[column_index];
        let first_column = column_index == 0;
        let last_column = column_index + 1 == self.columns.len();
        let mut raw: Vec<&Renderable> = Vec::new();
        if self.show_header {
            raw.push(&column.header);
        }
        raw.extend(column.cells.iter());
        let (t, r, b, l) = self.padding;
        if t + r + b + l == 0 {
            return raw.into_iter().cloned().collect();
        }
        let count = raw.len();
        raw.into_iter()
            .enumerate()
            .map(|(i, cell)| {
                let (first_row, last_row) = (i == 0, i + 1 == count);
                let (mut top, mut right, mut bottom, mut left) = self.padding;
                if self.collapse_padding {
                    if !first_column {
                        left = left.saturating_sub(right);
                    }
                    if !last_row {
                        bottom = top.saturating_sub(bottom);
                    }
                }
                if !self.pad_edge {
                    if first_column {
                        left = 0;
                    }
                    if last_column {
                        right = 0;
                    }
                    if first_row {
                        top = 0;
                    }
                    if last_row {
                        bottom = 0;
                    }
                }
                Renderable::Padding(Box::new(Padding {
                    renderable: cell.clone(),
                    pad: (top, right, bottom, left),
                    expand: true,
                }))
            })
            .collect()
    }

    /// Port of `Table._measure_column`.
    fn measure_column(&self, options: Options, column_index: usize) -> Measurement {
        let max_width = options.max_width as isize;
        if max_width < 1 {
            return Measurement(0, 0);
        }
        let column = &self.columns[column_index];
        let padding_width = self.padding_width(column_index);
        if let Some(width) = column.width {
            let width = width as isize + padding_width;
            return Measurement(width, width).with_maximum(max_width);
        }
        let measurements: Vec<Measurement> =
            self.cells(column_index).iter().map(|cell| measure(cell, options)).collect();
        let measurement = Measurement(
            measurements.iter().map(|m| m.0).max().unwrap_or(1),
            measurements.iter().map(|m| m.1).max().unwrap_or(max_width),
        )
        .with_maximum(max_width);
        measurement.clamp(
            column.min_width.map(|w| w as isize + padding_width),
            column.max_width.map(|w| w as isize + padding_width),
        )
    }

    /// Port of `Table._collapse_widths`.
    fn collapse_widths(widths: Vec<i64>, wrapable: &[bool], max_width: i64) -> Vec<i64> {
        let mut widths = widths;
        let mut total_width: i64 = widths.iter().sum();
        let mut excess_width = total_width - max_width;
        if wrapable.iter().any(|&w| w) {
            while total_width != 0 && excess_width > 0 {
                let max_column =
                    widths.iter().zip(wrapable).filter(|(_, &w)| w).map(|(&width, _)| width).max().unwrap_or(0);
                let second_max_column = widths
                    .iter()
                    .zip(wrapable)
                    .map(|(&width, &w)| if w && width != max_column { width } else { 0 })
                    .max()
                    .unwrap_or(0);
                let column_difference = max_column - second_max_column;
                let ratios: Vec<i64> =
                    widths.iter().zip(wrapable).map(|(&width, &w)| (width == max_column && w) as i64).collect();
                if !ratios.iter().any(|&r| r != 0) || column_difference == 0 {
                    break;
                }
                let max_reduce = vec![excess_width.min(column_difference); widths.len()];
                widths = ratio_reduce(excess_width, &ratios, &max_reduce, &widths);
                total_width = widths.iter().sum();
                excess_width = total_width - max_width;
            }
        }
        widths
    }

    /// Port of `Table._calculate_column_widths`: widths including padding, not borders.
    fn column_widths(&self, options: Options) -> Vec<usize> {
        let max_width = options.max_width as i64;
        let ranges: Vec<Measurement> = (0..self.columns.len()).map(|i| self.measure_column(options, i)).collect();
        let mut widths: Vec<i64> = ranges.iter().map(|r| if r.1 != 0 { r.1 as i64 } else { 1 }).collect();
        let extra_width = self.extra_width() as i64;
        if self.expands() {
            let ratios: Vec<i64> =
                self.columns.iter().filter(|c| c.flexible()).map(|c| c.ratio.unwrap_or(0) as i64).collect();
            if ratios.iter().any(|&r| r != 0) {
                let fixed_widths: Vec<i64> =
                    ranges.iter().zip(&self.columns).map(|(r, c)| if c.flexible() { 0 } else { r.1 as i64 }).collect();
                let flex_minimum: Vec<i64> = self
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.flexible())
                    .map(|(i, c)| c.width.filter(|&w| w != 0).unwrap_or(1) as i64 + self.padding_width(i) as i64)
                    .collect();
                let flexible_width = max_width - fixed_widths.iter().sum::<i64>();
                let flex_widths = ratio_distribute(flexible_width, &ratios, Some(&flex_minimum));
                let mut flex = flex_widths.into_iter();
                for (index, column) in self.columns.iter().enumerate() {
                    if column.flexible() {
                        widths[index] = fixed_widths[index] + flex.next().unwrap_or(0);
                    }
                }
            }
        }
        let mut table_width: i64 = widths.iter().sum();
        if table_width > max_width {
            let wrapable: Vec<bool> = self.columns.iter().map(|c| c.width.is_none() && !c.no_wrap).collect();
            widths = Table::collapse_widths(widths, &wrapable, max_width);
            table_width = widths.iter().sum();
            if table_width > max_width {
                let excess_width = table_width - max_width;
                widths = ratio_reduce(excess_width, &vec![1; widths.len()], &widths, &widths);
                table_width = widths.iter().sum();
            }
            widths = widths
                .iter()
                .enumerate()
                .map(|(i, &width)| self.measure_column(options.width(width as isize), i).1 as i64)
                .collect();
        }
        let min_width = self.min_width.map(|w| w as i64);
        if (table_width < max_width && self.expands()) || min_width.is_some_and(|m| table_width < m - extra_width) {
            let target = match min_width {
                None => max_width,
                Some(m) => (m - extra_width).min(max_width),
            };
            let pad_widths = ratio_distribute(target - table_width, &widths, None);
            widths = widths.iter().zip(pad_widths).map(|(w, p)| w + p).collect();
        }
        widths.into_iter().map(|w| w.max(0) as usize).collect()
    }

    /// Port of `Table.__rich_measure__`.
    pub(crate) fn measure(&self, options: Options) -> Measurement {
        let max_width = self.width.map_or(options.max_width as isize, |w| w as isize);
        if max_width < 0 {
            return Measurement(0, 0);
        }
        let extra_width = self.extra_width();
        let max_width: usize = self.column_widths(options.width(max_width - extra_width)).iter().sum();
        let measurements: Vec<Measurement> =
            (0..self.columns.len()).map(|i| self.measure_column(options.width(max_width as isize), i)).collect();
        let minimum = measurements.iter().map(|m| m.0).sum::<isize>() + extra_width;
        let maximum = match self.width {
            None => measurements.iter().map(|m| m.1).sum::<isize>() + extra_width,
            Some(width) => width as isize,
        };
        Measurement(minimum, maximum).clamp(self.min_width.map(|w| w as isize), None)
    }

    /// Port of `Table.__rich_console__`.
    pub(crate) fn render_segments(&self, options: Options) -> Vec<Seg> {
        if self.columns.is_empty() {
            return vec![Seg::line()];
        }
        let max_width = self.width.map_or(options.max_width as isize, |w| w as isize);
        let extra_width = self.extra_width();
        let widths = self.column_widths(options.width(max_width - extra_width));
        let table_width = widths.iter().sum::<usize>() as isize + extra_width;
        let render_options = options.width(table_width);
        let mut out = Vec::new();
        if let Some(title) = &self.title {
            let mut title = title.clone();
            title.style = self.title_style + title.style;
            let title_options = Options { justify: Some(self.title_justify), ..render_options };
            out.extend(render(&Renderable::Text(title), title_options));
        }
        out.extend(self.render_rows(&widths));
        out
    }

    /// Port of `Table._render`.
    fn render_rows(&self, widths: &[usize]) -> Vec<Seg> {
        let border = self.border_style;
        let column_cells: Vec<Vec<Renderable>> = (0..self.columns.len()).map(|i| self.cells(i)).collect();
        let row_count = column_cells.first().map_or(0, Vec::len);
        let box_chars = self.box_chars.map(|b| if self.show_header { b } else { b.plain_headed() });
        let mut out = Vec::new();
        if let Some(b) = box_chars {
            if self.show_edge {
                out.push(Seg::new(b.top(widths), border));
                out.push(Seg::line());
            }
        }
        for index in 0..row_count {
            let (first, last) = (index == 0, index + 1 == row_count);
            let header_row = first && self.show_header;
            let mut max_height = 1;
            let mut cells: Vec<Vec<Line>> = Vec::new();
            for ((column, column_cell), &width) in self.columns.iter().zip(&column_cells).zip(widths) {
                let render_options = Options {
                    max_width: width,
                    justify: Some(column.justify),
                    overflow: Some(column.overflow),
                    no_wrap: Some(column.no_wrap),
                };
                let cell_style = if header_row { self.header_style } else { Style::null() };
                let lines = render_lines(&column_cell[index], render_options, cell_style, true);
                max_height = max_height.max(lines.len());
                cells.push(lines);
            }
            let row_height = cells.iter().map(Vec::len).max().unwrap_or(0);
            let cells: Vec<Vec<Line>> = cells
                .into_iter()
                .enumerate()
                .map(|(column_index, lines)| {
                    let width = widths[column_index];
                    let style = if header_row { self.header_style } else { Style::null() };
                    let blank = vec![Seg::new(" ".repeat(width), style)];
                    let mut aligned = lines;
                    if aligned.len() < row_height {
                        let extra = row_height - aligned.len();
                        if header_row {
                            let mut padded = vec![blank; extra];
                            padded.extend(aligned);
                            aligned = padded;
                        } else {
                            aligned.extend(std::iter::repeat(blank).take(extra));
                        }
                    }
                    set_shape(&aligned, width, max_height)
                })
                .collect();
            if let Some(b) = box_chars {
                let (left, right, divider) = b.verticals(if first {
                    1
                } else if last {
                    6
                } else {
                    3
                });
                for line_no in 0..max_height {
                    if self.show_edge {
                        out.push(Seg::new(left.to_string(), border));
                    }
                    for (i, cell) in cells.iter().enumerate() {
                        out.extend(cell[line_no].iter().cloned());
                        if i + 1 < cells.len() {
                            out.push(Seg::new(divider.to_string(), border));
                        }
                    }
                    if self.show_edge {
                        out.push(Seg::new(right.to_string(), border));
                    }
                    out.push(Seg::line());
                }
            } else {
                for line_no in 0..max_height {
                    for cell in &cells {
                        out.extend(cell[line_no].iter().cloned());
                    }
                    out.push(Seg::line());
                }
            }
            if let Some(b) = box_chars {
                if first && self.show_header {
                    out.push(Seg::new(b.row(widths, 2, self.show_edge), border));
                    out.push(Seg::line());
                }
                if self.show_lines && !last && !header_row {
                    out.push(Seg::new(b.row(widths, 4, self.show_edge), border));
                    out.push(Seg::line());
                }
            }
        }
        if let Some(b) = box_chars {
            if self.show_edge {
                out.push(Seg::new(b.bottom(widths), border));
                out.push(Seg::line());
            }
        }
        out
    }
}
