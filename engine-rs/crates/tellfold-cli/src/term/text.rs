//! Styled text and word wrapping (port of `rich/text.py`, `rich/_wrap.py` and `Lines.justify` in
//! `rich/containers.py`).
//!
//! Offsets are code point offsets, as in Python, so text is held as `Vec<char>`.

use super::cells::{cell_len_chars, chop_cells, set_cell_size};
use super::render::Seg;
use super::style::Style;

/// How lines are aligned in their width (rich's `JustifyMethod`, without "full").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Justify {
    /// rich's "default": no padding at all.
    Default,
    Left,
    Center,
    Right,
}

/// What happens to text wider than its width (rich's `OverflowMethod`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overflow {
    /// Words longer than a line are folded onto the next lines.
    Fold,
    /// Lines are cut at the width.
    Crop,
    /// Lines are cut and end with "…".
    Ellipsis,
    /// Nothing is wrapped or cut.
    Ignore,
}

/// Python's `str.isspace` for one character, which is also what `\s` matches in `re`.
pub(crate) fn py_isspace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\x0b'
            | '\x0c'
            | '\r'
            | '\x1c'..='\x1f'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// Python's `str.rstrip()`.
pub(crate) fn py_rstrip(text: &[char]) -> &[char] {
    let end = text.iter().rposition(|&c| !py_isspace(c)).map_or(0, |i| i + 1);
    &text[..end]
}

/// Python's `str.strip()`.
pub(crate) fn py_strip(text: &str) -> &str {
    text.trim_matches(py_isspace)
}

/// Python's `str.splitlines()`.
fn py_splitlines(text: &[char]) -> Vec<&[char]> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            lines.push(&text[start..i]);
            if c == '\r' && text.get(i + 1) == Some(&'\n') {
                i += 1;
            }
            start = i + 1;
        }
        i += 1;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Removes the control codes rich strips from all text: bell, backspace, vertical tab, form feed, carriage
/// return (port of `strip_control_codes`).
fn strip_control_codes(text: &str) -> Vec<char> {
    text.chars().filter(|c| !matches!(c, '\x07' | '\x08' | '\x0b' | '\x0c' | '\x0d')).collect()
}

/// The `\s*\S+\s*` words of a line as `(start, end)` code point offsets (port of `_wrap.words`).
fn words(text: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut position = 0;
    loop {
        let mut i = position;
        while i < text.len() && py_isspace(text[i]) {
            i += 1;
        }
        if i == text.len() {
            return out;
        }
        while i < text.len() && !py_isspace(text[i]) {
            i += 1;
        }
        while i < text.len() && py_isspace(text[i]) {
            i += 1;
        }
        out.push((position, i));
        position = i;
    }
}

/// Code point offsets to break a line at so each piece fits in `width` cells (port of `_wrap.divide_line`).
pub(crate) fn divide_line(text: &[char], width: usize, fold: bool) -> Vec<usize> {
    let mut breaks = Vec::new();
    let mut cell_offset = 0;
    for (start, end) in words(text) {
        let word = &text[start..end];
        let word_length = cell_len_chars(py_rstrip(word));
        let remaining_space = width as isize - cell_offset as isize;
        if remaining_space >= word_length as isize {
            cell_offset += cell_len_chars(word);
        } else if word_length > width {
            if fold {
                let folded = chop_cells(word, width);
                let mut start = start;
                let count = folded.len();
                for (i, line) in folded.iter().enumerate() {
                    if start > 0 {
                        breaks.push(start);
                    }
                    if i + 1 == count {
                        cell_offset = cell_len_chars(line);
                    } else {
                        start += line.len();
                    }
                }
            } else {
                if start > 0 {
                    breaks.push(start);
                }
                cell_offset = cell_len_chars(word);
            }
        } else if cell_offset > 0 && start > 0 {
            breaks.push(start);
            cell_offset = cell_len_chars(word);
        }
    }
    breaks
}

/// A styled range of a [`Text`] (port of `rich.text.Span`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

/// Text with a base style and styled spans (port of `rich.text.Text`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    pub(crate) chars: Vec<char>,
    pub(crate) spans: Vec<Span>,
    pub(crate) style: Style,
    pub(crate) justify: Option<Justify>,
    pub(crate) overflow: Option<Overflow>,
    pub(crate) no_wrap: Option<bool>,
    pub(crate) end: &'static str,
}

impl Default for Text {
    fn default() -> Self {
        Text::new()
    }
}

impl Text {
    /// Empty text (`Text()`).
    pub fn new() -> Self {
        Text {
            chars: Vec::new(),
            spans: Vec::new(),
            style: Style::null(),
            justify: None,
            overflow: None,
            no_wrap: None,
            end: "\n",
        }
    }

    /// Unstyled text (`Text(text)`), control codes removed.
    pub fn plain(text: &str) -> Self {
        Text { chars: strip_control_codes(text), ..Text::new() }
    }

    /// Text with a base style (`Text(text, style=style)`).
    pub fn styled(text: &str, style: &str) -> Self {
        Text { style: Style::parse(style), ..Text::plain(text) }
    }

    /// Parses rich console markup (port of `rich.markup.render`, without emoji codes).
    pub fn from_markup(markup: &str) -> Self {
        super::markup::render(markup)
    }

    /// Sets the overflow method of this text.
    pub fn with_overflow(mut self, overflow: Overflow) -> Self {
        self.overflow = Some(overflow);
        self
    }

    /// Sets the justify method of this text.
    pub fn with_justify(mut self, justify: Justify) -> Self {
        self.justify = Some(justify);
        self
    }

    /// Sets `no_wrap` on this text.
    pub fn with_no_wrap(mut self, no_wrap: bool) -> Self {
        self.no_wrap = Some(no_wrap);
        self
    }

    /// Appends a string with a style, `""` for none (port of `Text.append(str, style)`).
    pub fn push(&mut self, text: &str, style: &str) -> &mut Self {
        self.push_style(text, Style::parse(style))
    }

    pub(crate) fn push_style(&mut self, text: &str, style: Style) -> &mut Self {
        let text = strip_control_codes(text);
        if !text.is_empty() {
            let offset = self.chars.len();
            if !style.is_null() {
                self.spans.push(Span { start: offset, end: offset + text.len(), style });
            }
            self.chars.extend(text);
        }
        self
    }

    /// Appends another text, its base style becoming a span (port of `Text.append(Text)`).
    pub fn append_text(&mut self, text: &Text) -> &mut Self {
        if !text.chars.is_empty() {
            let offset = self.chars.len();
            if !text.style.is_null() {
                self.spans.push(Span { start: offset, end: offset + text.chars.len(), style: text.style });
            }
            self.chars.extend_from_slice(&text.chars);
            self.spans.extend(text.spans.iter().map(|s| Span { start: s.start + offset, end: s.end + offset, ..*s }));
        }
        self
    }

    /// The text without styles.
    pub fn plain_text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Cells the text takes on one line.
    pub fn cell_len(&self) -> usize {
        cell_len_chars(&self.chars)
    }

    fn len(&self) -> usize {
        self.chars.len()
    }

    /// An empty text with the same settings (port of `Text.blank_copy`).
    fn blank_copy(&self) -> Text {
        Text { chars: Vec::new(), spans: Vec::new(), ..self.clone() }
    }

    /// Port of the `Text.plain` setter: spans past a shorter text are trimmed.
    fn set_plain(&mut self, chars: Vec<char>) {
        if chars != self.chars {
            let old_length = self.chars.len();
            self.chars = chars;
            if old_length > self.chars.len() {
                self.trim_spans(self.chars.len());
            }
        }
    }

    fn trim_spans(&mut self, max_offset: usize) {
        self.spans = self
            .spans
            .iter()
            .filter(|s| s.start < max_offset)
            .map(|s| if s.end < max_offset { *s } else { Span { end: s.end.min(max_offset), ..*s } })
            .collect();
    }

    /// Port of `Text.right_crop`.
    fn right_crop(&mut self, amount: usize) {
        let max_offset = self.chars.len() - amount;
        self.trim_spans(max_offset);
        self.chars.truncate(max_offset);
    }

    /// Port of `Text.rstrip`.
    fn rstrip(&mut self) {
        let stripped = py_rstrip(&self.chars).to_vec();
        self.set_plain(stripped);
    }

    /// Drops trailing whitespace beyond `size` code points (port of `Text.rstrip_end`).
    fn rstrip_end(&mut self, size: usize) {
        if self.len() > size {
            let excess = self.len() - size;
            let whitespace = self.len() - py_rstrip(&self.chars).len();
            if whitespace > 0 {
                self.right_crop(whitespace.min(excess));
            }
        }
    }

    /// Cuts the text to `max_width` cells and optionally pads it (port of `Text.truncate`).
    fn truncate(&mut self, max_width: isize, overflow: Option<Overflow>, pad: bool) {
        let overflow = overflow.or(self.overflow).unwrap_or(Overflow::Fold);
        if overflow == Overflow::Ignore {
            return;
        }
        let length = self.cell_len() as isize;
        if length > max_width {
            if overflow == Overflow::Ellipsis {
                let mut cut = set_cell_size(&self.chars, max_width - 1);
                cut.push('…');
                self.set_plain(cut);
            } else {
                let cut = set_cell_size(&self.chars, max_width);
                self.set_plain(cut);
            }
        }
        if pad && length < max_width {
            self.chars.extend(std::iter::repeat_n(' ', (max_width - length) as usize));
        }
    }

    /// `Text.truncate(width)` with the text's own overflow.
    pub(crate) fn truncate_to(&mut self, max_width: isize) {
        self.truncate(max_width, None, false);
    }

    /// Port of `Text.pad`.
    pub(crate) fn pad(&mut self, count: usize) {
        if count > 0 {
            self.pad_left(count as isize);
            self.chars.extend(std::iter::repeat_n(' ', count));
        }
    }

    /// Port of `Text.pad_left`.
    fn pad_left(&mut self, count: isize) {
        if count > 0 {
            let count = count as usize;
            let mut chars: Vec<char> = std::iter::repeat_n(' ', count).collect();
            chars.append(&mut self.chars);
            self.chars = chars;
            for span in &mut self.spans {
                span.start += count;
                span.end += count;
            }
        }
    }

    /// Port of `Text.pad_right`.
    fn pad_right(&mut self, count: isize) {
        if count > 0 {
            self.chars.extend(std::iter::repeat_n(' ', count as usize));
        }
    }

    /// Port of `Text.stylize_before` over the whole text.
    pub(crate) fn stylize_before(&mut self, style: Style) {
        if !style.is_null() && !self.chars.is_empty() {
            self.spans.insert(0, Span { start: 0, end: self.chars.len(), style });
        }
    }

    /// Port of `Text.stylize` over the whole text.
    pub(crate) fn stylize(&mut self, style: Style) {
        if !style.is_null() && !self.chars.is_empty() {
            self.spans.push(Span { start: 0, end: self.chars.len(), style });
        }
    }

    /// Port of `Text.extend_style`.
    fn extend_style(&mut self, spaces: usize) {
        if spaces == 0 {
            return;
        }
        let end_offset = self.len();
        for span in &mut self.spans {
            if span.end >= end_offset {
                span.end += spaces;
            }
        }
        self.chars.extend(std::iter::repeat_n(' ', spaces));
    }

    /// Splits into lines at the given offsets, sharing out the spans (port of `Text.divide`).
    fn divide(&self, offsets: &[usize]) -> Vec<Text> {
        if offsets.is_empty() {
            return vec![self.clone()];
        }
        let mut bounds = Vec::with_capacity(offsets.len() + 2);
        bounds.push(0);
        bounds.extend_from_slice(offsets);
        bounds.push(self.len());
        let mut lines: Vec<Text> = bounds
            .windows(2)
            .map(|w| Text {
                chars: self.chars[w[0]..w[1]].to_vec(),
                spans: Vec::new(),
                style: self.style,
                justify: self.justify,
                overflow: self.overflow,
                no_wrap: None,
                end: "\n",
            })
            .collect();
        for span in &self.spans {
            for (line_no, w) in bounds.windows(2).enumerate() {
                let (line_start, line_end) = (w[0], w[1]);
                let new_start = span.start.saturating_sub(line_start);
                let new_end = (span.end as isize - line_start as isize).min((line_end - line_start) as isize);
                if new_end > new_start as isize {
                    lines[line_no].spans.push(Span { start: new_start, end: new_end as usize, style: span.style });
                }
            }
        }
        lines
    }

    /// Port of `Text.split(separator, include_separator, allow_blank)`.
    fn split(&self, separator: char, include_separator: bool, allow_blank: bool) -> Vec<Text> {
        if !self.chars.contains(&separator) {
            return vec![self.clone()];
        }
        let positions: Vec<usize> = (0..self.len()).filter(|&i| self.chars[i] == separator).collect();
        let mut lines = if include_separator {
            let offsets: Vec<usize> = positions.iter().map(|p| p + 1).collect();
            self.divide(&offsets)
        } else {
            let offsets: Vec<usize> = positions.iter().flat_map(|&p| [p, p + 1]).collect();
            self.divide(&offsets).into_iter().filter(|line| line.chars != [separator]).collect()
        };
        if !allow_blank && self.chars.last() == Some(&separator) {
            lines.pop();
        }
        lines
    }

    /// Joins texts with this one between them (port of `Text.join`).
    fn join(&self, lines: &[Text]) -> Text {
        let mut new = self.blank_copy();
        let mut offset = 0;
        let mut add = |text: &Text, new: &mut Text| {
            new.chars.extend_from_slice(&text.chars);
            if !text.style.is_null() {
                new.spans.push(Span { start: offset, end: offset + text.len(), style: text.style });
            }
            new.spans.extend(text.spans.iter().map(|s| Span { start: s.start + offset, end: s.end + offset, ..*s }));
            offset += text.len();
        };
        for (i, line) in lines.iter().enumerate() {
            if i > 0 && !self.chars.is_empty() {
                add(self, &mut new);
            }
            add(line, &mut new);
        }
        new
    }

    /// Converts tabs to spaces (port of `Text.expand_tabs`).
    pub(crate) fn expand_tabs(&mut self, tab_size: usize) {
        if !self.chars.contains(&'\t') {
            return;
        }
        let mut new_text = Vec::new();
        for line in self.split('\n', true, false) {
            if !line.chars.contains(&'\t') {
                new_text.push(line);
                continue;
            }
            let mut cell_position = 0;
            for mut part in line.split('\t', true, false) {
                if part.chars.last() == Some(&'\t') {
                    *part.chars.last_mut().expect("ends with a tab") = ' ';
                    cell_position += part.cell_len();
                    let remainder = cell_position % tab_size;
                    if remainder > 0 {
                        let spaces = tab_size - remainder;
                        part.extend_style(spaces);
                        cell_position += spaces;
                    }
                } else {
                    cell_position += part.cell_len();
                }
                new_text.push(part);
            }
        }
        let result = Text::new().join(&new_text);
        self.chars = result.chars;
        self.spans = result.spans;
    }

    /// Word wraps at `width` cells with rich's defaults (justify "default", overflow "fold"), one text per line
    /// (port of `Text.wrap`).
    pub fn wrap(&self, width: usize) -> Vec<Text> {
        self.wrap_with(width, None, None, None)
    }

    /// Port of `Text.wrap` with explicit settings (tab size 8).
    pub(crate) fn wrap_with(
        &self,
        width: usize,
        justify: Option<Justify>,
        overflow: Option<Overflow>,
        no_wrap: Option<bool>,
    ) -> Vec<Text> {
        let wrap_justify = justify.or(self.justify).unwrap_or(Justify::Default);
        let wrap_overflow = overflow.or(self.overflow).unwrap_or(Overflow::Fold);
        let no_wrap = no_wrap.or(self.no_wrap).unwrap_or(false) || overflow == Some(Overflow::Ignore);
        let mut lines = Vec::new();
        for mut line in self.split('\n', false, true) {
            line.expand_tabs(8);
            let mut new_lines = if no_wrap {
                if overflow == Some(Overflow::Ignore) {
                    lines.push(line);
                    continue;
                }
                vec![line]
            } else {
                let offsets = divide_line(&line.chars, width, wrap_overflow == Overflow::Fold);
                let mut pieces = line.divide(&offsets);
                for piece in &mut pieces {
                    piece.rstrip_end(width);
                }
                pieces
            };
            justify_lines(&mut new_lines, width, wrap_justify, wrap_overflow);
            for line in &mut new_lines {
                line.truncate(width as isize, Some(wrap_overflow), false);
            }
            lines.extend(new_lines);
        }
        lines
    }

    /// The narrowest and widest the text can render: its longest word and its longest line (port of
    /// `Text.__rich_measure__`).
    pub(crate) fn measure(&self) -> (isize, isize) {
        let max_width = py_splitlines(&self.chars).into_iter().map(cell_len_chars).max().unwrap_or(0);
        let min_width = self
            .chars
            .split(|&c| py_isspace(c))
            .filter(|w| !w.is_empty())
            .map(cell_len_chars)
            .max()
            .unwrap_or(max_width);
        (min_width as isize, max_width as isize)
    }

    /// Port of `Text.__rich_console__`: wrapped lines joined by newlines, then `end`.
    pub(crate) fn render(
        &self,
        width: usize,
        justify: Option<Justify>,
        overflow: Option<Overflow>,
        no_wrap: Option<bool>,
    ) -> Vec<Seg> {
        let justify = self.justify.or(justify).unwrap_or(Justify::Default);
        let overflow = self.overflow.or(overflow).unwrap_or(Overflow::Fold);
        let no_wrap = Some(self.no_wrap.or(no_wrap).unwrap_or(false));
        let lines = self.wrap_with(width, Some(justify), Some(overflow), no_wrap);
        Text::plain("\n").join(&lines).segments(self.end)
    }

    /// Port of `Text.render`: one segment per run between span edges.
    fn segments(&self, end: &str) -> Vec<Seg> {
        let mut out = Vec::new();
        let text = &self.chars;
        if self.spans.is_empty() {
            out.push(Seg::plain(&text.iter().collect::<String>()));
        } else {
            let mut styles = vec![self.style];
            styles.extend(self.spans.iter().map(|s| s.style));
            let mut events: Vec<(usize, bool, usize)> = vec![(0, false, 0)];
            events.extend(self.spans.iter().enumerate().map(|(i, s)| (s.start, false, i + 1)));
            events.extend(self.spans.iter().enumerate().map(|(i, s)| (s.end, true, i + 1)));
            events.push((text.len(), true, 0));
            events.sort_by_key(|&(offset, leaving, _)| (offset, leaving));
            let mut stack: Vec<usize> = Vec::new();
            for pair in events.windows(2) {
                let (offset, leaving, style_id) = pair[0];
                let next_offset = pair[1].0;
                if leaving {
                    if let Some(i) = stack.iter().position(|&s| s == style_id) {
                        stack.remove(i);
                    }
                } else {
                    stack.push(style_id);
                }
                if next_offset > offset {
                    let mut ids = stack.clone();
                    ids.sort_unstable();
                    let style = ids.iter().fold(Style::null(), |acc, &id| acc + styles[id]);
                    out.push(Seg::new(text[offset..next_offset].iter().collect(), style));
                }
            }
        }
        if !end.is_empty() {
            out.push(Seg::plain(end));
        }
        out
    }

    /// Joins several texts into one for `Console.print` (its `sep_text.join`), ending in a newline.
    pub(crate) fn for_print(&self) -> Text {
        Text::plain(" ").join(std::slice::from_ref(self))
    }
}

/// Port of `Lines.justify` (without "full").
fn justify_lines(lines: &mut [Text], width: usize, justify: Justify, overflow: Overflow) {
    let width = width as isize;
    for line in lines.iter_mut() {
        match justify {
            Justify::Default => {}
            Justify::Left => line.truncate(width, Some(overflow), true),
            Justify::Center => {
                line.rstrip();
                line.truncate(width, Some(overflow), false);
                let len = line.cell_len() as isize;
                line.pad_left((width - len).div_euclid(2));
                let len = line.cell_len() as isize;
                line.pad_right(width - len);
            }
            Justify::Right => {
                line.rstrip();
                line.truncate(width, Some(overflow), false);
                let len = line.cell_len() as isize;
                line.pad_left(width - len);
            }
        }
    }
}
