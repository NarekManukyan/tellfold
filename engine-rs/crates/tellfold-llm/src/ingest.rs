//! Seed document loading and paragraph-aware chunking (port of `tellfold/ingest.py`).
//!
//! Lengths and slices count code points, as Python's do, and whitespace is Python's `str.isspace()`.

use std::fs;
use std::path::{Path, PathBuf};

use tellfold_models::text::{py_isspace, py_strip};
use tellfold_models::Chunk;

pub const TEXT_SUFFIXES: &[&str] = &[".md", ".markdown", ".txt"];
pub const SUPPORTED_SUFFIXES: &[&str] = &[".md", ".markdown", ".txt", ".pdf"];
const JOINER: &str = "\n\n";
const SENTENCE_ENDS: &[char] = &['.', '!', '?', '。', '！', '？'];

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("{0}")]
    NotFound(String),
    #[error("unsupported seed file type: {0} (use .pdf, .md, .markdown or .txt)")]
    Unsupported(String),
    #[error("chunk size must be positive and overlap smaller than half of it")]
    BadChunking,
    #[error("cannot read {name}: {message}")]
    Unreadable { name: String, message: String },
}

/// Python's `Path.suffix`, lower-cased: the last `.ext` of the name, none for a dot file.
fn suffix(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .and_then(|name| {
            let trimmed = name.trim_start_matches('.');
            trimmed.rfind('.').map(|i| trimmed[i..].to_lowercase())
        })
        .filter(|s| s.len() > 1)
        .unwrap_or_default()
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Read seed files (directories are expanded) into (source name, text) pairs, skipping empty ones.
pub fn load_documents(paths: &[PathBuf]) -> Result<Vec<(String, String)>, IngestError> {
    let mut docs = Vec::new();
    for path in expand(paths)? {
        let text = if suffix(&path) == ".pdf" { read_pdf(&path)? } else { read_text(&path)? };
        let text = normalize(&text);
        if !text.is_empty() {
            docs.push((file_name(&path), text));
        }
    }
    Ok(docs)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        // `rglob` does not descend into symlinked directories (`recurse_symlinks=False`).
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&path, out);
        } else if path.is_dir() {
            continue;
        } else if SUPPORTED_SUFFIXES.contains(&suffix(&path).as_str()) {
            out.push(path);
        }
    }
}

/// The seed files `paths` stand for: files as given (checked), folders recursively, symlinked folders not followed.
pub fn expand(paths: &[PathBuf]) -> Result<Vec<PathBuf>, IngestError> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut found = Vec::new();
            walk(path, &mut found);
            found.sort(); // Path ordering compares components, like sorting pathlib paths
            files.extend(found);
        } else if !path.exists() {
            return Err(IngestError::NotFound(path.display().to_string()));
        } else if !SUPPORTED_SUFFIXES.contains(&suffix(path).as_str()) {
            return Err(IngestError::Unsupported(file_name(path)));
        } else {
            files.push(path.clone());
        }
    }
    Ok(files)
}

/// Each page's text, pages joined by a blank line. pdf-extract panics on some malformed documents instead of
/// returning an error; that becomes `Unreadable` too, like PyMuPDF's exception.
fn read_pdf(path: &Path) -> Result<String, IngestError> {
    let bytes = fs::read(path).map_err(|e| unreadable(path, e))?;
    let extracted = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem_by_pages(&bytes));
    let pages = match extracted {
        Ok(result) => result.map_err(|e| unreadable(path, e))?,
        Err(panic) => {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "the PDF could not be parsed".into());
            return Err(unreadable(path, message));
        }
    };
    Ok(pages.join(JOINER))
}

fn unreadable(path: &Path, e: impl std::fmt::Display) -> IngestError {
    IngestError::Unreadable { name: file_name(path), message: e.to_string() }
}

/// UTF-8 (with or without a byte order mark); otherwise the detected encoding.
pub fn decode_text(raw: &[u8]) -> String {
    let body = raw.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(raw);
    if let Ok(text) = std::str::from_utf8(body) {
        return text.to_string();
    }
    if let Some((encoding, bom)) = encoding_rs::Encoding::for_bom(raw) {
        return encoding.decode_without_bom_handling(&raw[bom..]).0.into_owned();
    }
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(raw, true);
    let encoding = detector.guess(None, false);
    encoding.decode_without_bom_handling(raw).0.into_owned()
}

fn read_text(path: &Path) -> Result<String, IngestError> {
    let raw = fs::read(path).map_err(|e| unreadable(path, e))?;
    Ok(decode_text(&raw))
}

/// Line endings to `\n`, NULs dropped, runs of spaces/tabs/form feeds collapsed, lines stripped, at most one blank
/// line in a row.
pub fn normalize(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n").replace('\0', "");
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            let mut out = String::with_capacity(line.len());
            let mut in_run = false;
            for c in line.chars() {
                if matches!(c, ' ' | '\t' | '\x0c' | '\x0b') {
                    if !in_run {
                        out.push(' ');
                    }
                    in_run = true;
                } else {
                    out.push(c);
                    in_run = false;
                }
            }
            py_strip(&out).to_string()
        })
        .collect();
    let joined = lines.join("\n");
    // \n{3,} -> \n\n
    let mut out = String::with_capacity(joined.len());
    let mut newlines = 0;
    for c in joined.chars() {
        if c == '\n' {
            newlines += 1;
            continue;
        }
        if newlines > 0 {
            out.push_str(if newlines >= 3 { JOINER } else { &"\n\n"[..newlines] });
            newlines = 0;
        }
        out.push(c);
    }
    if newlines > 0 {
        out.push_str(if newlines >= 3 { JOINER } else { &"\n\n"[..newlines] });
    }
    py_strip(&out).to_string()
}

/// Split documents into chunks of at most `size` characters, packing whole paragraphs where possible.
///
/// Consecutive chunks of the same document share up to `overlap` trailing characters so that facts straddling a
/// boundary stay readable in at least one chunk. Chunk ids are global and sequential.
pub fn chunk_text(docs: &[(String, String)], size: i64, overlap: i64) -> Result<Vec<Chunk>, IngestError> {
    if size <= 0 || !(0 <= overlap && overlap < size.div_euclid(2)) {
        return Err(IngestError::BadChunking);
    }
    let (size, overlap) = (size as usize, overlap as usize);
    let mut chunks = Vec::new();
    for (source, text) in docs {
        let limit = size - overlap - JOINER.len();
        for body in pack(&segments(text, limit), size, overlap) {
            chunks.push(Chunk { id: chunks.len() as i64, source: source.clone(), text: body });
        }
    }
    Ok(chunks)
}

pub fn chunk_text_default(docs: &[(String, String)]) -> Vec<Chunk> {
    chunk_text(docs, 1800, 200).expect("the default sizes are valid")
}

fn len(s: &str) -> usize {
    s.chars().count()
}

/// `s[a:b]` by code points.
fn slice(s: &str, start: usize, end: usize) -> &str {
    let mut indices = s.char_indices().map(|(i, _)| i).chain(std::iter::once(s.len()));
    let a = indices.by_ref().nth(start).unwrap_or(s.len());
    let b = if end <= start { a } else { indices.nth(end - start - 1).unwrap_or(s.len()) };
    &s[a..b.max(a)]
}

/// `re.split(r"\n\s*\n", text)`.
fn split_paragraphs(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut parts = Vec::new();
    let mut piece_start = 0;
    let mut i = 0;
    while i < chars.len() {
        if chars[i].1 == '\n' {
            // greedy \s*, then back off to the last \n of the run
            let mut j = i + 1;
            while j < chars.len() && py_isspace(chars[j].1) {
                j += 1;
            }
            if let Some(last) = (i + 1..j).rev().find(|&k| chars[k].1 == '\n') {
                parts.push(&text[piece_start..chars[i].0]);
                piece_start = chars[last].0 + 1;
                i = last + 1;
                continue;
            }
        }
        i += 1;
    }
    parts.push(&text[piece_start..]);
    parts
}

/// `re.split(r"(?<=[.!?。！？])\s+", para)`.
fn split_sentences(para: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut prev: Option<char> = None;
    let mut iter = para.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        if py_isspace(c) && prev.is_some_and(|p| SENTENCE_ENDS.contains(&p)) {
            let mut end = i + c.len_utf8();
            while let Some(&(j, d)) = iter.peek() {
                if !py_isspace(d) {
                    break;
                }
                end = j + d.len_utf8();
                iter.next();
            }
            parts.push(&para[start..i]);
            start = end;
            prev = None;
            continue;
        }
        prev = Some(c);
    }
    parts.push(&para[start..]);
    parts
}

/// Paragraphs no longer than `limit`; long paragraphs are split by sentence, then by word.
fn segments(text: &str, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in split_paragraphs(text) {
        let para = py_strip(para);
        if para.is_empty() {
            continue;
        }
        if len(para) <= limit {
            out.push(para.to_string());
            continue;
        }
        out.extend(greedy_join(&split_sentences(para), limit, " "));
    }
    out
}

fn greedy_join(pieces: &[&str], limit: usize, sep: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for piece in pieces {
        for part in hard_split(piece, limit) {
            let candidate = if current.is_empty() { part.clone() } else { format!("{current}{sep}{part}") };
            if len(&candidate) <= limit {
                current = candidate;
            } else {
                out.push(std::mem::replace(&mut current, part));
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn hard_split(piece: &str, limit: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut piece = piece.to_string();
    while len(&piece) > limit {
        // piece.rfind(" ", 0, limit + 1)
        let window: Vec<char> = piece.chars().take(limit + 1).collect();
        let found = window.iter().rposition(|c| *c == ' ');
        let cut = match found {
            Some(c) if c > limit / 2 => c,
            _ => limit,
        };
        parts.push(py_strip(slice(&piece, 0, cut)).to_string());
        piece = py_strip(slice(&piece, cut, len(&piece))).to_string();
    }
    if !piece.is_empty() {
        parts.push(piece);
    }
    parts
}

fn pack(segments: &[String], size: usize, overlap: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for seg in segments {
        let candidate = if current.is_empty() { seg.clone() } else { format!("{current}{JOINER}{seg}") };
        if len(&candidate) <= size {
            current = candidate;
            continue;
        }
        let tail = tail(&current, overlap);
        chunks.push(current);
        current = if tail.is_empty() { seg.clone() } else { format!("{tail}{JOINER}{seg}") };
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// The last `overlap` characters of `text` or fewer, starting at a word boundary.
fn tail(text: &str, overlap: usize) -> String {
    if overlap == 0 {
        return String::new();
    }
    let n = len(text);
    let mut tail = slice(text, n.saturating_sub(overlap), n);
    if n > overlap {
        if let Some((i, c)) = tail.char_indices().find(|(_, c)| py_isspace(*c)) {
            tail = &tail[i + c.len_utf8()..];
        }
    }
    py_strip(tail).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(suffix(Path::new("a/B.MD")), ".md");
        assert_eq!(suffix(Path::new(".bashrc")), "");
        assert_eq!(suffix(Path::new("a.tar.gz")), ".gz");
        assert_eq!(slice("héllo", 1, 3), "él");
        assert_eq!(split_paragraphs("a\n\nb\n \n\nc\nd"), ["a", "b", "c\nd"]);
        assert_eq!(split_sentences("One. Two!  Three? x"), ["One.", "Two!", "Three?", "x"]);
        assert_eq!(normalize("  a\t\tb \r\n\r\n\n\n\nc\0  "), "a b\n\nc");
    }
}
