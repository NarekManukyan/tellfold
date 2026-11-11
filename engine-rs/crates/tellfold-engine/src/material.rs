//! The user's own audience material: customer reviews, survey answers, support emails, social comments and meeting
//! notes, read into short items, redacted on this machine ([`crate::redact`]) and capped, so the cast can be built
//! from what real people said.
//!
//! [`read_material`] does everything before a model is involved and is what a preview shows: the items in it are
//! exactly the text that will be sent. A run keeps only those redacted items (`material.json`), never the user's
//! files. [`read_items`] then asks the planner model where each item stands on the question, and [`cluster`] groups
//! them locally by camp and shared worry, so the cast's camps follow the material's real spread.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tellfold_llm::ingest::decode_text;
use tellfold_llm::{json, Llm};
use tellfold_models::text::py_strip;
use tellfold_models::{material_counts_text, MaterialSummary, MATERIAL_KINDS};
use tellfold_store::estimate::MATERIAL_ITEMS_PER_CALL;
use tellfold_store::files::write_json;

use crate::personas::CAMPS;
use crate::redact::{add_counts, empty_counts, redact_with};

/// The redacted items of a run's material, in its folder.
pub const MATERIAL_FILE: &str = "material.json";
/// File types material is read from.
pub const MATERIAL_SUFFIXES: [&str; 6] = [".txt", ".md", ".markdown", ".csv", ".tsv", ".eml"];
pub const MAX_MATERIAL_FILES: usize = 20;
pub const MAX_MATERIAL_FILE_BYTES: usize = 10 * 1024 * 1024;
/// Items a run reads at most; more are sampled evenly down to this.
pub const MAX_ITEMS: usize = 300;
/// Characters an item keeps; longer ones are cut at a word.
pub const MAX_ITEM_CHARS: usize = 600;
/// Shorter items ("ok", "N/A") say nothing about anyone.
pub const MIN_ITEM_CHARS: usize = 12;
/// Fewer items than this is thin material: a few voices shape the whole crowd.
pub const THIN_ITEMS: usize = 30;
/// Real quotes each grounded persona is written from.
pub const QUOTES_PER_PERSONA: usize = 3;
/// Most words or phrases the user may add to hide.
pub const MAX_HIDE_WORDS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MaterialError(pub String);

/// One file of material as the user gave it. `kind` is one of [`MATERIAL_KINDS`]; None infers it from the file
/// type (.eml are emails, .csv and .tsv survey answers). `column` picks a spreadsheet's column, by header or by
/// 1-based number; None takes the column with the most text.
#[derive(Debug, Clone)]
pub struct MaterialFile {
    pub name: String,
    pub kind: Option<String>,
    pub column: Option<String>,
    pub bytes: Vec<u8>,
}

/// One review, survey answer, email, comment or note, after redaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialItem {
    pub id: String,
    pub kind: String,
    pub file: String,
    pub text: String,
}

/// What became of one file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileReport {
    pub name: String,
    pub kind: String,
    /// items found in it (before the run-wide cap)
    pub items: i64,
    /// a spreadsheet's column headers, so a client can offer them
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    /// the column read
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
}

/// The material ready to send: what a preview shows, and what a run stores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub summary: MaterialSummary,
    pub files: Vec<FileReport>,
    pub items: Vec<MaterialItem>,
    /// thin material, the size cap, files that gave nothing
    pub warnings: Vec<String>,
    pub thin: bool,
}

/// `material.json`: the summary and the redacted items.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredMaterial {
    pub summary: MaterialSummary,
    pub items: Vec<MaterialItem>,
}

/// The kind a file of material holds: the one given, else what its type says.
pub fn kind_of(name: &str, given: Option<&str>) -> Result<String, MaterialError> {
    if let Some(given) = given.map(|g| py_strip(g).to_lowercase()).filter(|g| !g.is_empty()) {
        let kind = match given.as_str() {
            "review" | "reviews" => "reviews",
            "survey" | "surveys" | "survey answers" => "survey",
            "email" | "emails" | "support emails" => "emails",
            "comment" | "comments" | "social comments" => "comments",
            "note" | "notes" | "meeting notes" => "notes",
            _ => {
                return Err(MaterialError(format!(
                    "{name}: unknown kind {given:?}; use one of {}",
                    MATERIAL_KINDS.join(", ")
                )))
            }
        };
        return Ok(kind.into());
    }
    match suffix(name).as_str() {
        ".eml" => Ok("emails".into()),
        ".csv" | ".tsv" => Ok("survey".into()),
        _ => Err(MaterialError(format!("{name}: say what it holds ({})", MATERIAL_KINDS.join(", ")))),
    }
}

fn suffix(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    match base.rfind('.') {
        Some(i) if i > 0 => base[i..].to_lowercase(),
        _ => String::new(),
    }
}

/// Reads, splits, redacts and caps the material. `hide` are extra words or phrases to hide as names (staff, pupils,
/// customers the fixed rules would miss). `agents`, when known, warns when there are few items per person.
pub fn read_material(files: &[MaterialFile], hide: &[String], agents: Option<i64>) -> Result<Material, MaterialError> {
    if files.is_empty() {
        return Err(MaterialError("add at least one file of audience material".into()));
    }
    if files.len() > MAX_MATERIAL_FILES {
        return Err(MaterialError(format!("at most {MAX_MATERIAL_FILES} files of audience material")));
    }
    if hide.len() > MAX_HIDE_WORDS {
        return Err(MaterialError(format!("at most {MAX_HIDE_WORDS} words to hide")));
    }
    let mut reports = Vec::new();
    let mut found: Vec<(String, String, String)> = Vec::new(); // (kind, file, text)
    let mut redacted = empty_counts();
    let mut seen: HashSet<String> = HashSet::new();
    let mut warnings = Vec::new();
    for file in files {
        let suffix = suffix(&file.name);
        if !MATERIAL_SUFFIXES.contains(&suffix.as_str()) {
            return Err(MaterialError(format!(
                "{}: audience material is read from .txt, .md, .csv, .tsv and .eml files",
                file.name
            )));
        }
        if file.bytes.len() > MAX_MATERIAL_FILE_BYTES {
            return Err(MaterialError(format!("{} is larger than {} MB", file.name, MAX_MATERIAL_FILE_BYTES >> 20)));
        }
        let kind = kind_of(&file.name, file.kind.as_deref())?;
        let text = decode_text(&file.bytes).replace("\r\n", "\n").replace('\r', "\n");
        let spreadsheet = suffix == ".csv" || suffix == ".tsv";
        if kind == "survey" && !spreadsheet {
            return Err(MaterialError(format!(
                "{}: survey answers come from a .csv or .tsv export; choose the column with the answers",
                file.name
            )));
        }
        let (raw, columns, column) = if spreadsheet {
            let rows = parse_csv(&text, if suffix == ".tsv" { Some('\t') } else { None });
            let (cells, columns, column) = column_cells(&file.name, &rows, file.column.as_deref())?;
            (cells, columns, Some(column))
        } else if kind == "emails" {
            (split_emails(&text, suffix == ".eml"), Vec::new(), None)
        } else {
            (split_items(&text, &kind), Vec::new(), None)
        };
        let mut kept = 0;
        for piece in raw {
            let (clean, counts) = redact_with(&piece, hide);
            let Some(item) = tidy_item(&clean) else { continue };
            if !seen.insert(item.to_lowercase()) {
                continue;
            }
            add_counts(&mut redacted, &counts);
            found.push((kind.clone(), file.name.clone(), item));
            kept += 1;
        }
        if kept == 0 {
            warnings.push(format!("{}: no usable text found", file.name));
        }
        reports.push(FileReport { name: file.name.clone(), kind, items: kept, columns, column });
    }
    if found.is_empty() {
        return Err(MaterialError("no usable text in the audience material".into()));
    }
    let total = found.len();
    let picked: Vec<(String, String, String)> = if total > MAX_ITEMS {
        // evenly across the files, in their order, so every source keeps its share
        (0..MAX_ITEMS).map(|i| found[i * total / MAX_ITEMS].clone()).collect()
    } else {
        found
    };
    let width = picked.len().to_string().len().max(3);
    let items: Vec<MaterialItem> = picked
        .into_iter()
        .enumerate()
        .map(|(i, (kind, file, text))| MaterialItem { id: format!("m{:0width$}", i + 1), kind, file, text })
        .collect();
    let counts = count_kinds(items.iter().map(|i| i.kind.as_str()));
    let dropped = (total - items.len()) as i64;
    let thin = items.len() < THIN_ITEMS;
    if dropped > 0 {
        warnings.push(format!(
            "Kept {} of {total} items, the most a run reads; the rest were left out evenly across your files.",
            items.len()
        ));
    }
    if thin {
        warnings.push(format!(
            "Only {}: a few voices will shape the whole crowd. {THIN_ITEMS} or more give a truer spread.",
            material_counts_text(&counts)
        ));
    } else if let Some(agents) = agents.filter(|a| (items.len() as i64) < *a) {
        warnings.push(format!(
            "{} items for {agents} people: some people will be written from the same quotes.",
            items.len()
        ));
    }
    let summary = MaterialSummary {
        items: items.len() as i64,
        counts,
        dropped,
        redacted,
        files: reports.iter().map(|r| r.name.clone()).collect(),
    };
    Ok(Material { summary, files: reports, items, warnings, thin })
}

/// Items per kind, in [`MATERIAL_KINDS`] order, kinds with none left out.
pub fn count_kinds<'a>(kinds: impl Iterator<Item = &'a str>) -> IndexMap<String, i64> {
    let mut counts: IndexMap<String, i64> = MATERIAL_KINDS.iter().map(|k| (k.to_string(), 0)).collect();
    for kind in kinds {
        *counts.entry(kind.to_string()).or_insert(0) += 1;
    }
    counts.retain(|_, n| *n > 0);
    counts
}

/// Writes `material.json` into a run folder.
pub fn write_material(run_dir: &Path, material: &Material) -> std::io::Result<()> {
    let stored = StoredMaterial { summary: material.summary.clone(), items: material.items.clone() };
    write_json(&run_dir.join(MATERIAL_FILE), &stored)
}

pub fn load_material(run_dir: &Path) -> Result<StoredMaterial, MaterialError> {
    let path = run_dir.join(MATERIAL_FILE);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| MaterialError(format!("could not read the audience material ({MATERIAL_FILE}): {e}")))?;
    serde_json::from_str(&text).map_err(|e| MaterialError(format!("{MATERIAL_FILE} is not valid: {e}")))
}

// ---------- splitting ----------

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

macro_rules! pattern {
    ($name:ident, $pattern:expr) => {
        fn $name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| re($pattern))
        }
    };
}

pattern!(re_separator, r"(?m)^[ \t]*(?:-{3,}|={3,}|\*{3,}|_{3,}|#{3,})[ \t]*$");
pattern!(re_blank_lines, r"\n[ \t]*\n");
pattern!(re_bullet, r"^[ \t]*(?:[-*•‣◦·–]|\d{1,3}[.)])[ \t]+");
pattern!(re_space, r"\s+");
pattern!(
    re_quoted_reply,
    r"(?im)^(?:on\s.{0,200}\swrote:|-{2,}\s*original message\s*-{2,}|_{5,}|from:\s.*\bsent:\s.*)\s*$"
);
pattern!(re_sent_from, r"(?im)^\s*sent from my .*$");
pattern!(re_email_header, r"(?i)^(from|to|cc|bcc|date|sent|subject|reply-to|message-id|received):\s?(.*)$");

/// Cleans one redacted piece: whitespace collapsed, bullet marker dropped, cut to [`MAX_ITEM_CHARS`]; None when too
/// short to say anything.
fn tidy_item(text: &str) -> Option<String> {
    let text = re_bullet().replace(py_strip(text), "");
    let text = re_space().replace_all(&text, " ");
    let text = py_strip(&text);
    if text.chars().count() < MIN_ITEM_CHARS || !text.chars().any(char::is_alphabetic) {
        return None;
    }
    if text.chars().count() <= MAX_ITEM_CHARS {
        return Some(text.to_string());
    }
    let cut: String = text.chars().take(MAX_ITEM_CHARS).collect();
    let at = cut.rfind(char::is_whitespace).filter(|i| *i > MAX_ITEM_CHARS / 2).unwrap_or(cut.len());
    Some(format!("{}…", cut[..at].trim_end()))
}

/// Reviews, comments and notes: one item per separated entry, else per paragraph; a single block of lines is one
/// review or comment per line, and bulleted notes are one item per bullet.
pub fn split_items(text: &str, kind: &str) -> Vec<String> {
    let blocks: Vec<&str> = if re_separator().is_match(text) {
        re_separator().split(text).collect()
    } else {
        re_blank_lines().split(text).collect()
    };
    let blocks: Vec<&str> = blocks.into_iter().map(py_strip).filter(|b| !b.is_empty()).collect();
    let one_per_line = blocks.len() == 1 && matches!(kind, "reviews" | "comments");
    let mut out = Vec::new();
    for block in blocks {
        let lines: Vec<&str> =
            block.lines().filter(|l| !l.trim_start().starts_with('#')).filter(|l| !l.trim().is_empty()).collect();
        let bullets = lines.iter().filter(|l| re_bullet().is_match(l)).count();
        if one_per_line && lines.len() > 1 {
            out.extend(lines.iter().map(|l| l.to_string()));
        } else if bullets >= 2 {
            // a bullet and the lines that continue it
            let mut current: Option<String> = None;
            for line in lines {
                if re_bullet().is_match(line) {
                    out.extend(current.take());
                    current = Some(line.to_string());
                } else if let Some(c) = current.as_mut() {
                    c.push('\n');
                    c.push_str(line);
                } else {
                    out.push(line.to_string());
                }
            }
            out.extend(current);
        } else if !lines.is_empty() {
            out.push(lines.join("\n"));
        }
    }
    out
}

/// Support emails: an .eml file is one message; a text file holds one, or several that start with a `From:` line or
/// are split by separator lines. Headers, quoted replies, forwarded parts and signatures are dropped; a subject is
/// kept as the first line.
pub fn split_emails(text: &str, eml: bool) -> Vec<String> {
    let messages: Vec<String> = if eml {
        vec![eml_text(text)]
    } else if text.lines().filter(|l| l.to_lowercase().starts_with("from:")).count() >= 2 {
        let mut out: Vec<String> = Vec::new();
        for line in text.lines() {
            if line.to_lowercase().starts_with("from:") || out.is_empty() {
                out.push(String::new());
            }
            let last = out.last_mut().expect("one message");
            last.push_str(line);
            last.push('\n');
        }
        out
    } else if re_separator().is_match(text) {
        re_separator().split(text).map(str::to_string).collect()
    } else {
        vec![text.to_string()]
    };
    messages.iter().map(|m| email_body(m)).filter(|m| !m.trim().is_empty()).collect()
}

/// A message's own words: header lines turned into the subject alone, everything from a quoted reply, a forwarded
/// part or a signature on dropped.
fn email_body(message: &str) -> String {
    let mut subject = None;
    let mut lines: Vec<&str> = Vec::new();
    let mut in_headers = true;
    for line in message.lines() {
        if in_headers {
            if let Some(c) = re_email_header().captures(line) {
                if c[1].eq_ignore_ascii_case("subject") && !c[2].trim().is_empty() && !c[2].contains("=?") {
                    subject = Some(c[2].trim().to_string());
                }
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            in_headers = false;
        }
        if line == "-- " || line == "--" || re_quoted_reply().is_match(line) {
            break;
        }
        if line.trim_start().starts_with('>') || re_sent_from().is_match(line) {
            continue;
        }
        lines.push(line);
    }
    let body = lines.join("\n");
    match subject {
        Some(s) if !body.trim().is_empty() => format!("{s}\n{body}"),
        Some(s) => s,
        None => body,
    }
}

/// The readable text of an .eml message: its first text/plain part (else text/html without tags), decoded.
fn eml_text(raw: &str) -> String {
    let (headers, body) = split_head(raw);
    let subject = header(&headers, "subject").filter(|s| !s.contains("=?"));
    let text = part_text(&headers, body).unwrap_or_default();
    match subject {
        Some(s) => format!("Subject: {s}\n\n{text}"),
        None => text,
    }
}

/// Unfolded headers and the body after the first blank line.
fn split_head(raw: &str) -> (Vec<(String, String)>, &str) {
    let (head, body) = match raw.find("\n\n") {
        Some(i) => (&raw[..i], &raw[i + 2..]),
        None => (raw, ""),
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in head.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some((_, v)) = headers.last_mut() {
                v.push(' ');
                v.push_str(line.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_lowercase(), v.trim().to_string()));
        }
    }
    (headers, body)
}

fn header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
}

fn part_text(headers: &[(String, String)], body: &str) -> Option<String> {
    let content_type = header(headers, "content-type").unwrap_or_else(|| "text/plain".into()).to_lowercase();
    if content_type.starts_with("multipart/") {
        let boundary = boundary_of(&content_type, headers)?;
        let marker = format!("--{boundary}");
        let parts: Vec<&str> = body.split(marker.as_str()).skip(1).filter(|p| !p.starts_with("--")).collect();
        let decoded: Vec<(String, String)> = parts
            .iter()
            .filter_map(|part| {
                let (h, b) = split_head(part.trim_start_matches('\n'));
                let kind = header(&h, "content-type").unwrap_or_else(|| "text/plain".into()).to_lowercase();
                part_text(&h, b).map(|t| (kind, t))
            })
            .collect();
        return decoded
            .iter()
            .find(|(k, _)| k.starts_with("text/plain"))
            .or_else(|| decoded.first())
            .map(|(_, t)| t.clone());
    }
    if !content_type.starts_with("text/") {
        return None;
    }
    let encoding = header(headers, "content-transfer-encoding").unwrap_or_default().to_lowercase();
    let bytes = match encoding.as_str() {
        "base64" => base64_decode(body),
        "quoted-printable" => quoted_printable(body),
        _ => body.as_bytes().to_vec(),
    };
    let text = decode_text(&bytes).replace("\r\n", "\n");
    Some(if content_type.starts_with("text/html") { strip_html(&text) } else { text })
}

fn boundary_of(content_type: &str, headers: &[(String, String)]) -> Option<String> {
    // the header keeps its case for the boundary itself
    let original = header(headers, "content-type").unwrap_or_default();
    let at = content_type.find("boundary=")?;
    let rest = &original[at + "boundary=".len()..];
    let rest = rest.trim_start_matches('"');
    let end = rest.find(['"', ';']).unwrap_or(rest.len());
    Some(rest[..end].trim().to_string()).filter(|b| !b.is_empty())
}

fn quoted_printable(body: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            if bytes.get(i + 1) == Some(&b'\n') {
                i += 2;
                continue;
            }
            let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn base64_decode(body: &str) -> Vec<u8> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0);
    for c in body.bytes() {
        let Some(v) = value(c) else { continue };
        buffer = (buffer << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    out
}

fn strip_html(html: &str) -> String {
    static TAGS: OnceLock<Regex> = OnceLock::new();
    static BREAKS: OnceLock<Regex> = OnceLock::new();
    let breaks = BREAKS.get_or_init(|| re(r"(?i)<\s*(?:br|/p|/div|/li|/tr)\s*/?>"));
    let tags = TAGS.get_or_init(|| re(r"(?s)<[^>]*>"));
    let text = breaks.replace_all(html, "\n");
    let text = tags.replace_all(&text, "");
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

// ---------- spreadsheets ----------

/// Rows of a CSV (quoted fields, doubled quotes and line breaks inside quotes). Without a delimiter, the one of
/// `,` `;` and tab the header line uses most.
pub fn parse_csv(text: &str, delimiter: Option<char>) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delimiter = delimiter.unwrap_or_else(|| {
        let header = text.lines().next().unwrap_or("");
        [',', ';', '\t'].into_iter().max_by_key(|d| (header.matches(*d).count(), *d == ',')).unwrap_or(',')
    });
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
        } else if c == '"' && field.is_empty() {
            quoted = true;
        } else if c == delimiter {
            row.push(std::mem::take(&mut field));
        } else if c == '\n' {
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
        } else {
            field.push(c);
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    rows
}

/// The cells of the chosen column (by header, any case, or 1-based number), else of the column with the most text;
/// with the headers and the chosen header.
fn column_cells(
    name: &str,
    rows: &[Vec<String>],
    wanted: Option<&str>,
) -> Result<(Vec<String>, Vec<String>, String), MaterialError> {
    let Some((header, body)) = rows.split_first() else {
        return Err(MaterialError(format!("{name} is empty")));
    };
    let columns: Vec<String> = header.iter().map(|h| py_strip(h).to_string()).collect();
    let index = match wanted.map(py_strip).filter(|w| !w.is_empty()) {
        Some(w) => columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(w))
            .or_else(|| w.parse::<usize>().ok().filter(|n| (1..=columns.len()).contains(n)).map(|n| n - 1))
            .ok_or_else(|| {
                MaterialError(format!("{name}: no column {w:?}; the columns are: {}", columns.join(", ")))
            })?,
        None => (0..columns.len())
            .max_by_key(|&i| {
                let text: usize = body.iter().filter_map(|r| r.get(i)).map(|c| c.trim().chars().count()).sum();
                (text, std::cmp::Reverse(i))
            })
            .unwrap_or(0),
    };
    let cells = body.iter().filter_map(|r| r.get(index)).map(|c| c.to_string()).collect();
    let chosen = columns.get(index).cloned().unwrap_or_default();
    Ok((cells, columns, chosen))
}

// ---------- reading with the model ----------

/// Where one item stands on the question, as the planner model reads it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReadItem {
    pub item_id: String,
    #[serde(default)]
    pub camp: String,
    /// the worry or hope behind it, a few words
    #[serde(default)]
    pub theme: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MaterialRead {
    #[serde(default)]
    pub items: Vec<ReadItem>,
}

// Rust-only (the Python engine has no material), so it lives with the crate: golden/make.py rewrites schemas/.
tellfold_llm::json_model!(MaterialRead, "../schemas/MaterialRead.json");

pub const READ_SYSTEM: &str = "\
You read what real people wrote (reviews, survey answers, support emails, social comments, meeting notes) to see \
where each of them would stand on a question someone is about to put to the public. Judge each item on its own \
words: how the person would most likely react to the question's subject, and the main worry or hope that drives \
it. Text in square brackets such as [name] or [email] replaced personal details; ignore it. Reply with JSON only.";

/// An item's camp and theme, by its index in the material.
#[derive(Debug, Clone, PartialEq)]
pub struct Classified {
    pub index: usize,
    pub camp: String,
    pub theme: String,
}

fn read_prompt(batch: &[(usize, &MaterialItem)], question: &str, language: &str) -> String {
    let listing = batch
        .iter()
        .map(|(_, item)| format!("[{}] ({}) {}", item.id, item.kind, item.text))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Question about to be simulated:\n{question}\n\nItems:\n{listing}\n\n\
For every item, give `camp`, the person's likely starting leaning toward the question's subject, one of: {}; \
and `theme`, the worry or hope behind it in at most eight words, in {language}, in the item's own terms \
(\"price keeps going up\", \"loves the offline mode\").\n\n\
Return a JSON object:\n{{\"items\": [{{\"item_id\": item id, \"camp\": str, \"theme\": str}}]}}",
        CAMPS.join(", ")
    )
}

/// Asks the planner model, [`MATERIAL_ITEMS_PER_CALL`] items a call, where each item stands. Items of a failed call
/// or with an unknown camp are left out; an empty result means the material could not be read at all.
pub async fn read_items(
    llm: &dyn Llm,
    items: &[MaterialItem],
    question: &str,
    model: &str,
    language: &str,
) -> Vec<Classified> {
    let indexed: Vec<(usize, &MaterialItem)> = items.iter().enumerate().collect();
    let batches: Vec<&[(usize, &MaterialItem)]> = indexed.chunks(MATERIAL_ITEMS_PER_CALL as usize).collect();
    let calls = batches.iter().map(|batch| async move {
        let prompt = read_prompt(batch, question, language);
        let read = json::<MaterialRead>(llm, &prompt, READ_SYSTEM, model, "material.read").await.ok()?;
        Some(match_reads(batch, read.items))
    });
    futures::future::join_all(calls).await.into_iter().flatten().flatten().collect()
}

/// Pairs answers with items by echoed id, then hands leftovers to unanswered items in order.
fn match_reads(batch: &[(usize, &MaterialItem)], reads: Vec<ReadItem>) -> Vec<Classified> {
    let ids: HashSet<&str> = batch.iter().map(|(_, i)| i.id.as_str()).collect();
    let mut by_id: IndexMap<String, ReadItem> = IndexMap::new();
    let mut leftovers: std::collections::VecDeque<ReadItem> = Default::default();
    for read in reads {
        let id = py_strip(&read.item_id).trim_matches(['[', ']']).to_string();
        if ids.contains(id.as_str()) && !by_id.contains_key(&id) {
            by_id.insert(id, read);
        } else {
            leftovers.push_back(read);
        }
    }
    let mut out = Vec::new();
    for (index, item) in batch {
        let Some(read) = by_id.shift_remove(&item.id).or_else(|| leftovers.pop_front()) else { continue };
        let camp = py_strip(&read.camp).to_lowercase();
        if !CAMPS.contains(&camp.as_str()) {
            continue;
        }
        let theme: String = py_strip(&read.theme).chars().take(80).collect();
        out.push(Classified { index: *index, camp, theme });
    }
    out
}

// ---------- clustering ----------

/// Items of one camp that share a worry or hope.
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    pub camp: String,
    pub theme: String,
    /// indexes into the material, in its order
    pub items: Vec<usize>,
}

const STOPWORDS: &str = "a an the and or but of to in on for with at by from is are was were be been it its this that \
these those as not no so too very more most less about over into than then they them their there here what which who \
how why when will would can could should just only also really much many some any all my our your his her we you i \
me us he she one get got make makes made keep keeps going goes every each app apps thing things people time way lot";

fn stopwords() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| STOPWORDS.split_whitespace().collect())
}

/// The content words of a theme, lower case, plural "s" dropped.
fn theme_words(theme: &str) -> HashSet<String> {
    theme
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 3 && !stopwords().contains(w.as_str()))
        .map(|w| if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") { w[..w.len() - 1].to_string() } else { w })
        .collect()
}

/// Groups the read items by camp, and within a camp by shared theme words (an item joins the group sharing the most
/// of its words, when that is at least a third of them, else starts one). Camps in [`CAMPS`] order, groups biggest first; the same input always gives
/// the same groups.
pub fn cluster(read: &[Classified]) -> Vec<Cluster> {
    let mut out = Vec::new();
    for camp in CAMPS {
        let mut groups: Vec<(Cluster, HashSet<String>)> = Vec::new();
        for item in read.iter().filter(|r| r.camp == camp) {
            let words = theme_words(&item.theme);
            let best = groups
                .iter()
                .enumerate()
                .map(|(g, (_, seen))| (g, words.intersection(seen).count()))
                .filter(|(_, shared)| *shared > 0 && *shared * 3 >= words.len())
                .max_by_key(|(g, shared)| (*shared, std::cmp::Reverse(*g)));
            match best {
                Some((g, _)) => {
                    groups[g].0.items.push(item.index);
                    groups[g].1.extend(words);
                }
                None => groups.push((
                    Cluster { camp: camp.to_string(), theme: item.theme.clone(), items: vec![item.index] },
                    words,
                )),
            }
        }
        let mut groups: Vec<Cluster> = groups.into_iter().map(|(c, _)| c).collect();
        groups.sort_by_key(|c| (std::cmp::Reverse(c.items.len()), c.items[0]));
        out.extend(groups);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, kind: Option<&str>, column: Option<&str>, text: &str) -> MaterialFile {
        MaterialFile {
            name: name.into(),
            kind: kind.map(String::from),
            column: column.map(String::from),
            bytes: text.as_bytes().to_vec(),
        }
    }

    #[test]
    fn kinds_are_given_or_inferred() {
        assert_eq!(kind_of("a.txt", Some("Reviews")).unwrap(), "reviews");
        assert_eq!(kind_of("a.txt", Some("support emails")).unwrap(), "emails");
        assert_eq!(kind_of("inbox.eml", None).unwrap(), "emails");
        assert_eq!(kind_of("answers.csv", None).unwrap(), "survey");
        assert!(kind_of("a.txt", None).unwrap_err().0.contains("say what it holds"));
        assert!(kind_of("a.txt", Some("tweets")).unwrap_err().0.contains("unknown kind"));
    }

    #[test]
    fn reviews_split_by_blank_lines_separators_or_lines() {
        let text = "Love the app but the price doubled.\n\nSync broke twice this week, very annoying.";
        assert_eq!(split_items(text, "reviews").len(), 2);
        let lines = "Way too expensive now.\nBest plant app I have used.\nWhy is offline mode paid?";
        assert_eq!(split_items(lines, "comments").len(), 3);
        let separated = "First review here.\nIt spans two lines.\n---\nSecond review here.";
        assert_eq!(
            split_items(separated, "reviews"),
            vec!["First review here.\nIt spans two lines.", "Second review here."]
        );
        let notes =
            "# Parent forum\n- Worried about the bus times\n  for the younger ones\n- Phones are needed for safety";
        assert_eq!(
            split_items(notes, "notes"),
            vec!["- Worried about the bus times\n  for the younger ones", "- Phones are needed for safety"]
        );
    }

    #[test]
    fn csv_columns_by_name_number_or_most_text() {
        let csv = "Name,Email,What would change your mind?\n\
Ann Lee,ann@x.com,\"If the price stayed the same, I'd stay\"\n\
Bo,bo@x.com,\"Honestly \"\"nothing\"\", it is great\"\n";
        let rows = parse_csv(csv, None);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2][2], "Honestly \"nothing\", it is great");
        let (cells, columns, chosen) = column_cells("s.csv", &rows, None).unwrap();
        assert_eq!(chosen, "What would change your mind?");
        assert_eq!(columns.len(), 3);
        assert_eq!(cells.len(), 2);
        assert_eq!(column_cells("s.csv", &rows, Some("email")).unwrap().2, "Email");
        assert_eq!(column_cells("s.csv", &rows, Some("1")).unwrap().2, "Name");
        let err = column_cells("s.csv", &rows, Some("Age")).unwrap_err();
        assert!(err.0.contains("the columns are: Name, Email"), "{err}");
        let semi = parse_csv("a;b\n1;2\n", None);
        assert_eq!(semi[1], vec!["1", "2"]);
    }

    #[test]
    fn only_the_chosen_column_is_read() {
        let csv =
            "Name,Answer\nTamsin Greaves,The new price is too much for a family\nOlu Ade,I like it fine overall\n";
        let material = read_material(&[file("s.csv", None, None, csv)], &[], None).unwrap();
        let sent: String = material.items.iter().map(|i| i.text.clone()).collect();
        assert!(!sent.contains("Tamsin") && !sent.contains("Olu"), "{sent}");
        assert_eq!(material.files[0].column.as_deref(), Some("Answer"));
    }

    #[test]
    fn emails_lose_headers_quotes_and_signatures() {
        let eml = "From: Tamsin Greaves <tamsin@example.com>\nTo: support@mossbook.app\nSubject: Charged twice\n\
Content-Type: text/plain; charset=utf-8\nContent-Transfer-Encoding: quoted-printable\n\n\
Hi team,\nI was charged twice for order #A88213 =E2=80=94 please fix.\n\nThanks,\nTamsin\n\n\
On Tue, 3 Mar 2026, Support wrote:\n> We are looking into it\n";
        let items = split_emails(eml, true);
        assert_eq!(items.len(), 1);
        let text = &items[0];
        assert!(text.starts_with("Charged twice\nHi team,"), "{text}");
        assert!(text.contains("charged twice for order #A88213 — please fix"), "{text}");
        assert!(!text.contains("looking into it") && !text.contains("tamsin@"), "{text}");
        let (clean, _) = crate::redact::redact(text);
        assert!(!clean.contains("Tamsin") && !clean.contains("A88213"), "{clean}");
    }

    #[test]
    fn multipart_and_base64_emails() {
        let eml = "Subject: Hello\nContent-Type: multipart/alternative; boundary=\"XyZ\"\n\n--XyZ\n\
Content-Type: text/html\n\n<p>html <b>body</b></p>\n--XyZ\nContent-Type: text/plain\n\
Content-Transfer-Encoding: base64\n\nVGhlIHN5bmMgaXMgYnJva2VuIGFnYWlu\n--XyZ--\n";
        assert_eq!(split_emails(eml, true), vec!["Hello\nThe sync is broken again"]);
    }

    #[test]
    fn a_text_file_of_several_emails() {
        let text = "From: a@x.com\nSubject: One\n\nThe export button does nothing.\n\
From: b@x.com\nSubject: Two\n\nPlease bring back the yearly plan.\n";
        assert_eq!(split_emails(text, false).len(), 2);
    }

    #[test]
    fn material_is_redacted_deduplicated_and_numbered() {
        let reviews = "Great app, my order #A12345 arrived fast. — Sarah K.\n\n\
Too expensive now, call me on +44 7700 900123.\n\nok\n\nToo expensive now, call me on +44 7700 900123.";
        let material = read_material(&[file("r.txt", Some("reviews"), None, reviews)], &[], Some(10)).unwrap();
        let texts: Vec<&str> = material.items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["Great app, my order #[number] arrived fast. — [name]", "Too expensive now, call me on [phone]."]
        );
        assert_eq!(material.items[0].id, "m001");
        assert_eq!(material.summary.redacted["numbers"], 1);
        assert_eq!(material.summary.redacted["names"], 1);
        assert_eq!(material.summary.redacted["phones"], 1);
        assert!(material.thin);
        assert!(material.warnings.iter().any(|w| w.contains("Only 2 reviews")), "{:?}", material.warnings);
    }

    #[test]
    fn the_cap_samples_evenly() {
        let many: String = (0..(MAX_ITEMS + 100)).map(|i| format!("Comment number {i} about the change\n")).collect();
        let material = read_material(&[file("c.txt", Some("comments"), None, &many)], &[], None).unwrap();
        assert_eq!(material.items.len(), MAX_ITEMS);
        assert_eq!(material.summary.dropped, 100);
        // item i of the 300 is item i * 400 / 300 of the 400
        assert_eq!(material.items[1].text, "Comment number 1 about the change");
        assert_eq!(material.items.last().unwrap().text, "Comment number 398 about the change");
        assert!(material.warnings.iter().any(|w| w.starts_with("Kept 300 of 400")));
    }

    #[test]
    fn long_items_are_cut_at_a_word() {
        let long = "word ".repeat(400);
        let material = read_material(&[file("n.txt", Some("notes"), None, &long)], &[], None).unwrap();
        let text = &material.items[0].text;
        assert!(text.chars().count() <= MAX_ITEM_CHARS + 1 && text.ends_with("word…"), "{text}");
    }

    #[test]
    fn refusals() {
        let err = |files: &[MaterialFile]| read_material(files, &[], None).unwrap_err().0;
        assert!(err(&[]).contains("at least one"));
        assert!(err(&[file("a.pdf", Some("notes"), None, "x")]).contains(".txt, .md, .csv"));
        assert!(err(&[file("a.txt", Some("survey"), None, "x")]).contains(".csv or .tsv"));
        assert!(err(&[file("a.txt", Some("notes"), None, "ok\n\nno")]).contains("no usable text"));
        let big =
            MaterialFile { bytes: vec![b'a'; MAX_MATERIAL_FILE_BYTES + 1], ..file("a.txt", Some("notes"), None, "") };
        assert!(err(&[big]).contains("larger than"));
    }

    #[test]
    fn counts_follow_kind_order() {
        let counts = count_kinds(["notes", "reviews", "reviews", "survey"].into_iter());
        assert_eq!(counts.keys().collect::<Vec<_>>(), vec!["reviews", "survey", "notes"]);
        assert_eq!(material_counts_text(&counts), "2 reviews, 1 survey answer and 1 meeting note");
    }

    fn read(index: usize, camp: &str, theme: &str) -> Classified {
        Classified { index, camp: camp.into(), theme: theme.into() }
    }

    #[test]
    fn clusters_by_camp_then_shared_worry() {
        let reads = vec![
            read(0, "lean_critical", "price keeps going up"),
            read(1, "strong_support", "loves offline mode"),
            read(2, "lean_critical", "subscription price too high"),
            read(3, "lean_critical", "sync breaks often"),
            read(4, "strong_support", "offline mode is great"),
            read(5, "lean_critical", "prices rising every year"),
        ];
        let clusters = cluster(&reads);
        let shape: Vec<(&str, Vec<usize>)> = clusters.iter().map(|c| (c.camp.as_str(), c.items.clone())).collect();
        assert_eq!(
            shape,
            vec![("lean_critical", vec![0, 2, 5]), ("lean_critical", vec![3]), ("strong_support", vec![1, 4])]
        );
        assert_eq!(clusters[0].theme, "price keeps going up");
        assert_eq!(cluster(&reads), clusters, "deterministic");
    }

    #[test]
    fn answers_match_items_by_id_then_in_order() {
        let items: Vec<MaterialItem> = (1..=3)
            .map(|i| MaterialItem { id: format!("m00{i}"), kind: "reviews".into(), file: "r".into(), text: "t".into() })
            .collect();
        let batch: Vec<(usize, &MaterialItem)> = items.iter().enumerate().collect();
        let answer = |id: &str, camp: &str| ReadItem { item_id: id.into(), camp: camp.into(), theme: "x".into() };
        let got = match_reads(
            &batch,
            vec![answer("[m003]", "undecided"), answer("zz", "Lean_Support"), answer("m001", "nonsense")],
        );
        assert_eq!(
            got.iter().map(|c| (c.index, c.camp.as_str())).collect::<Vec<_>>(),
            vec![(1, "lean_support"), (2, "undecided")]
        );
    }
}
