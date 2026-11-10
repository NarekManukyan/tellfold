//! Terminal cell widths (port of `rich/cells.py` with the table from `rich/_unicode_data`).
//!
//! Strings are handled as `[char]` here because rich indexes Python strings by code point.

use super::cell_widths::{NARROW_TO_WIDE, WIDTHS};

const ZWJ: char = '\u{200d}';
const VS16: char = '\u{fe0f}';

/// Whether rich knows `c` is one cell wide without a table lookup (`_SINGLE_CELL_UNICODE_RANGES`).
fn is_single_cell(c: char) -> bool {
    matches!(c as u32, 0x20..=0x7E | 0xA0..=0xAC | 0xAE..=0x2FF | 0x370..=0x482 | 0x2500..=0x25FC | 0x2800..=0x28FF)
}

fn all_single_cell(text: &[char]) -> bool {
    text.iter().all(|&c| is_single_cell(c))
}

/// Cells one character takes: 0, 1 or 2 (port of `get_character_cell_size`).
pub fn char_cells(c: char) -> usize {
    let cp = c as u32;
    if (cp != 0 && cp < 32) || (0x7F..0xA0).contains(&cp) {
        return 0;
    }
    let last = WIDTHS[WIDTHS.len() - 1].1;
    if cp > last {
        return 1;
    }
    match WIDTHS.binary_search_by(|&(start, end, _)| {
        if cp < start {
            std::cmp::Ordering::Greater
        } else if cp > end {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    }) {
        Ok(index) => WIDTHS[index].2 as usize,
        Err(_) => 1,
    }
}

fn narrow_to_wide(c: char) -> bool {
    NARROW_TO_WIDE.binary_search(&(c as u32)).is_ok()
}

/// Cells a string takes in the terminal (port of rich's `cell_len`), counting zero width joiner sequences
/// and variation selector 16 the way rich does.
pub fn cell_len(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    cell_len_chars(&chars)
}

pub(crate) fn cell_len_chars(text: &[char]) -> usize {
    if all_single_cell(text) {
        return text.len();
    }
    if !text.iter().any(|&c| c == ZWJ || c == VS16) {
        return text.iter().map(|&c| char_cells(c)).sum();
    }
    let mut total = 0;
    let mut last_measured: Option<char> = None;
    let mut index = 0;
    while index < text.len() {
        let c = text[index];
        if c == ZWJ {
            index += 1;
        } else if c == VS16 {
            if let Some(last) = last_measured.take() {
                total += narrow_to_wide(last) as usize;
            }
        } else {
            let width = char_cells(c);
            if width > 0 {
                last_measured = Some(c);
                total += width;
            }
        }
        index += 1;
    }
    total
}

/// `(start, end, cells)` spans of single graphemes covering the whole string, and its cell length (port of
/// `split_graphemes`).
fn split_graphemes(text: &[char]) -> (Vec<(usize, usize, usize)>, usize) {
    let count = text.len();
    let mut index = 0;
    let mut last_measured: Option<char> = None;
    let mut total = 0;
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    while index < count {
        let c = text[index];
        if c == ZWJ || c == VS16 {
            if spans.is_empty() {
                spans.push((index, index + 1, 0));
                index += 1;
                continue;
            }
            if c == ZWJ {
                index += if index < count - 1 { 2 } else { 1 };
                spans.last_mut().expect("not empty").1 = index;
            } else {
                index += 1;
                let span = spans.last_mut().expect("not empty");
                if let Some(last) = last_measured {
                    if narrow_to_wide(last) {
                        last_measured = None;
                        span.2 += 1;
                        total += 1;
                    }
                }
                span.1 = index;
            }
            continue;
        }
        let width = char_cells(c);
        if width > 0 {
            last_measured = Some(c);
            spans.push((index, index + 1, width));
            total += width;
        } else if let Some(span) = spans.last_mut() {
            span.1 = index + 1;
        } else {
            spans.push((index, index + 1, 0));
        }
        index += 1;
    }
    (spans, total)
}

/// Splits `text` at a cell position; a double width character across the split turns into two spaces (port of
/// `_split_text`). Only called when `0 < position < cell_len(text)`.
fn split_text_at(text: &[char], position: usize) -> (Vec<char>, Vec<char>) {
    if position == 0 {
        return (Vec::new(), text.to_vec());
    }
    let (spans, cell_length) = split_graphemes(text);
    let mut offset = ((position as f64 / cell_length as f64) * spans.len() as f64) as usize;
    let mut left_size: usize = spans[..offset.min(spans.len())].iter().map(|s| s.2).sum();
    let space_split = |start: usize, end: usize| {
        let mut left = text[..start].to_vec();
        left.push(' ');
        let mut right = vec![' '];
        right.extend_from_slice(&text[end..]);
        (left, right)
    };
    loop {
        if left_size == position {
            if offset >= spans.len() {
                return (text.to_vec(), Vec::new());
            }
            let split = spans[offset].0;
            return (text[..split].to_vec(), text[split..].to_vec());
        }
        if left_size < position {
            let (start, end, size) = spans[offset];
            if left_size + size > position {
                return space_split(start, end);
            }
            offset += 1;
            left_size += size;
        } else {
            let (start, end, size) = spans[offset - 1];
            if left_size - size < position {
                return space_split(start, end);
            }
            offset -= 1;
            left_size -= size;
        }
    }
}

/// `text[:n]` with Python's meaning for a negative `n`.
fn py_prefix(text: &[char], n: isize) -> Vec<char> {
    let len = text.len() as isize;
    let end = if n < 0 { (len + n).max(0) } else { n.min(len) };
    text[..end as usize].to_vec()
}

/// Crops or pads `text` with spaces to exactly `total` cells (port of `set_cell_size`, including its slicing of
/// a negative `total` for one-cell text).
pub(crate) fn set_cell_size(text: &[char], total: isize) -> Vec<char> {
    if all_single_cell(text) {
        let size = text.len() as isize;
        if size < total {
            let mut out = text.to_vec();
            out.extend(std::iter::repeat(' ').take((total - size) as usize));
            return out;
        }
        return py_prefix(text, total);
    }
    if total <= 0 {
        return Vec::new();
    }
    let size = cell_len_chars(text) as isize;
    if size == total {
        return text.to_vec();
    }
    if size < total {
        let mut out = text.to_vec();
        out.extend(std::iter::repeat(' ').take((total - size) as usize));
        return out;
    }
    split_text_at(text, total as usize).0
}

/// Splits text into lines of at most `width` cells (port of `chop_cells`).
pub(crate) fn chop_cells(text: &[char], width: usize) -> Vec<Vec<char>> {
    if all_single_cell(text) {
        return text.chunks(width.max(1)).map(<[char]>::to_vec).collect();
    }
    let (spans, _) = split_graphemes(text);
    let mut line_size = 0;
    let mut lines = Vec::new();
    let mut line_offset = 0;
    for (start, _end, size) in spans {
        if line_size + size > width {
            lines.push(text[line_offset..start].to_vec());
            line_offset = start;
            line_size = 0;
        }
        line_size += size;
    }
    if line_size > 0 {
        lines.push(text[line_offset..].to_vec());
    }
    lines
}
