//! Console markup such as `"[bold red]error:[/] message"` (port of `rich/markup.py` without emoji codes and
//! `@` handlers). rich raises `MarkupError` on a closing tag that matches nothing; this port ignores it.

use super::style::Style;
use super::text::{Span, Text};

/// One `RE_TAGS` match, `((\\*)\[([a-z#/@][^[]*?)])`, as code point offsets.
struct TagMatch {
    start: usize,
    end: usize,
    escapes: usize,
    tag: String,
}

fn find_tags(chars: &[char]) -> Vec<TagMatch> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' || chars[i] == '[' {
            let mut j = i;
            while j < chars.len() && chars[j] == '\\' {
                j += 1;
            }
            if j + 1 < chars.len() && chars[j] == '[' && matches!(chars[j + 1], 'a'..='z' | '#' | '/' | '@') {
                let mut k = j + 1;
                while k < chars.len() && chars[k] != ']' && chars[k] != '[' {
                    k += 1;
                }
                if k < chars.len() && chars[k] == ']' {
                    out.push(TagMatch { start: i, end: k + 1, escapes: j - i, tag: chars[j + 1..k].iter().collect() });
                    i = k + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// What `Style.normalize` makes of a tag name, close enough to match closing tags to opening ones.
fn normalize(name: &str) -> String {
    name.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>().join(" ")
}

/// A tag's style string: its name, then its parameters after a space (`Tag.__str__`).
fn tag_style(name: &str, parameters: &Option<String>) -> Style {
    if name.starts_with('@') {
        return Style::null();
    }
    match parameters {
        None => Style::parse(name),
        Some(p) => Style::parse(&format!("{name} {p}")),
    }
}

/// Port of `rich.markup.render`.
pub(crate) fn render(markup: &str) -> Text {
    if !markup.contains('[') {
        return Text::plain(markup);
    }
    let chars: Vec<char> = markup.chars().collect();
    let mut text = Text::new();
    let mut stack: Vec<(usize, String, Option<String>)> = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut position = 0;
    let push_plain = |text: &mut Text, piece: &[char]| {
        let piece: String = piece.iter().collect();
        text.push_style(&piece.replace("\\[", "["), Style::null());
    };
    for m in find_tags(&chars) {
        let mut start = m.start;
        if start > position {
            push_plain(&mut text, &chars[position..start]);
        }
        if m.escapes > 0 {
            let (backslashes, escaped) = (m.escapes / 2, m.escapes % 2);
            if backslashes > 0 {
                push_plain(&mut text, &vec!['\\'; backslashes]);
                start += backslashes * 2;
            }
            if escaped == 1 {
                push_plain(&mut text, &chars[start + 1..m.end]);
                position = m.end;
                continue;
            }
        }
        let (name, parameters) = match m.tag.split_once('=') {
            Some((name, parameters)) => (name.to_string(), Some(parameters.to_string())),
            None => (m.tag.clone(), None),
        };
        let offset = text.chars.len();
        if let Some(closing) = name.strip_prefix('/') {
            let closing = closing.trim();
            let open = if closing.is_empty() {
                stack.pop()
            } else {
                let closing = normalize(closing);
                stack.iter().rposition(|(_, n, _)| *n == closing).map(|i| stack.remove(i))
            };
            if let Some((open_start, open_name, open_parameters)) = open {
                spans.push(Span { start: open_start, end: offset, style: tag_style(&open_name, &open_parameters) });
            }
        } else {
            stack.push((offset, normalize(&name), parameters));
        }
        position = m.end;
    }
    if position < chars.len() {
        push_plain(&mut text, &chars[position..]);
    }
    let length = text.chars.len();
    while let Some((start, name, parameters)) = stack.pop() {
        spans.push(Span { start, end: length, style: tag_style(&name, &parameters) });
    }
    spans.reverse();
    spans.sort_by_key(|s| s.start);
    // rich keeps spans whose style string is set even when it resolves to no style; they only split segments.
    text.spans = spans;
    text
}
