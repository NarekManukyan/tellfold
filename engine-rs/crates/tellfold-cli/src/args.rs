//! Command-line parsing with click's rules and messages (what typer runs on): options anywhere, `--opt=value`,
//! `-qvalue`, `--` ends options, parameters processed in the order they were given, and the same usage errors.

use std::collections::HashMap;
use std::path::Path;

/// What a parameter's value is converted to and checked against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Str,
    /// `int`, with typer's `min`/`max` making it an `IntRange`.
    Int {
        min: Option<i64>,
        max: Option<i64>,
    },
    /// `Path` with typer's `exists`, `file_okay` and `dir_okay`.
    Path {
        exists: bool,
        file_okay: bool,
        dir_okay: bool,
    },
    /// A boolean flag (`--think`), or a `--open/--no-open` pair when the option has a secondary name.
    Flag,
}

#[derive(Debug, Clone)]
pub struct Param {
    /// The Python parameter name (`run_dir`, `agent_model`); arguments use it in messages.
    pub name: &'static str,
    /// Option names as declared (`--question`, `-q`); empty for an argument.
    pub opts: Vec<&'static str>,
    /// The negative names of a boolean pair (`--no-open`).
    pub secondary: Vec<&'static str>,
    pub kind: Kind,
    pub required: bool,
    /// An argument that takes every remaining value (`nargs=-1`), or an option that may repeat.
    pub many: bool,
    pub envvar: Vec<&'static str>,
    /// The value when the parameter is not given (as it would be typed), `None` for no default.
    pub default: Option<&'static str>,
    /// What `--help` shows as `[default: ...]`, when it shows one.
    pub default_help: Option<&'static str>,
    /// What `--help` shows.
    pub help: &'static str,
    /// The type column of `--help` (`<int range>`), without the range.
    pub metavar: &'static str,
    pub is_argument: bool,
    /// Only the Rust engine has it, so typer's recorded help screens leave it out.
    pub rust_only: bool,
}

impl Param {
    pub fn argument(name: &'static str, kind: Kind, help: &'static str, metavar: &'static str) -> Self {
        Param {
            name,
            opts: Vec::new(),
            secondary: Vec::new(),
            kind,
            required: true,
            many: false,
            envvar: Vec::new(),
            default: None,
            default_help: None,
            help,
            metavar,
            is_argument: true,
            rust_only: false,
        }
    }

    pub fn option(
        name: &'static str,
        opts: &[&'static str],
        kind: Kind,
        help: &'static str,
        metavar: &'static str,
    ) -> Self {
        Param {
            name,
            opts: opts.to_vec(),
            secondary: Vec::new(),
            kind,
            required: false,
            many: false,
            envvar: Vec::new(),
            default: None,
            default_help: None,
            help,
            metavar,
            is_argument: false,
            rust_only: false,
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn many(mut self) -> Self {
        self.many = true;
        self.required = self.required && self.is_argument;
        self
    }

    /// A default that `--help` shows as typed.
    pub fn default(mut self, value: &'static str) -> Self {
        self.default = Some(value);
        self.default_help = Some(value);
        self
    }

    /// A default that `--help` does not show (boolean flags that are off).
    pub fn quiet_default(mut self, value: &'static str) -> Self {
        self.default = Some(value);
        self
    }

    pub fn default_help(mut self, text: &'static str) -> Self {
        self.default_help = Some(text);
        self
    }

    pub fn secondary(mut self, names: &[&'static str]) -> Self {
        self.secondary = names.to_vec();
        self
    }

    pub fn rust_only(mut self) -> Self {
        self.rust_only = true;
        self
    }

    pub fn envvar(mut self, names: &[&'static str]) -> Self {
        self.envvar = names.to_vec();
        self
    }

    /// How click names the parameter in messages: `'--question' / '-q'` or `'seeds'`.
    pub fn hint(&self) -> String {
        if self.is_argument {
            format!("'{}'", self.name)
        } else {
            let names = self.opts.iter().map(|o| format!("'{o}'")).collect::<Vec<_>>().join(" / ");
            if self.envvar.is_empty() {
                return names;
            }
            // click shows the option's envvar list as Python prints it
            let list = self.envvar.iter().map(|e| py_repr(e)).collect::<Vec<_>>().join(", ");
            format!("{names} (env var: '[{list}]')")
        }
    }
}

/// A converted value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    None,
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<String>),
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub values: HashMap<&'static str, Value>,
    pub help: bool,
}

impl Parsed {
    pub fn str(&self, name: &str) -> Option<String> {
        match self.values.get(name) {
            Some(Value::Str(s)) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn int(&self, name: &str) -> i64 {
        match self.values.get(name) {
            Some(Value::Int(n)) => *n,
            other => panic!("{name} is not an int: {other:?}"),
        }
    }

    /// An optional number: None when the option was not given.
    pub fn opt_int(&self, name: &str) -> Option<i64> {
        match self.values.get(name) {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        }
    }

    pub fn flag(&self, name: &str) -> bool {
        matches!(self.values.get(name), Some(Value::Bool(true)))
    }

    pub fn list(&self, name: &str) -> Vec<String> {
        match self.values.get(name) {
            Some(Value::List(items)) => items.clone(),
            _ => Vec::new(),
        }
    }
}

/// A usage error: the message click prints in the Error panel, and whether click shows the usage line above it.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageError {
    pub message: String,
    pub show_usage: bool,
}

impl UsageError {
    pub fn new(message: impl Into<String>) -> Self {
        UsageError { message: message.into(), show_usage: true }
    }

    pub fn bare(message: impl Into<String>) -> Self {
        UsageError { message: message.into(), show_usage: false }
    }
}

/// typer's `_escape_control_characters`: C0 and C1 control characters as `\xNN`, for values echoed in errors.
pub fn escape_control(text: &str) -> String {
    text.chars()
        .map(|c| match c as u32 {
            0x00..=0x1f | 0x7f..=0x9f => format!("\\x{:02x}", c as u32),
            _ => c.to_string(),
        })
        .collect()
}

/// Python's `repr()` of a str.
pub fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// click's `_describe_range` for an `IntRange`.
pub fn describe_range(min: Option<i64>, max: Option<i64>) -> String {
    match (min, max) {
        (Some(lo), Some(hi)) => format!("{lo}<=x<={hi}"),
        (Some(lo), None) => format!("x>={lo}"),
        (None, Some(hi)) => format!("x<={hi}"),
        (None, None) => String::new(),
    }
}

fn convert(param: &Param, raw: &str) -> Result<Value, UsageError> {
    let invalid = |why: String| UsageError::new(format!("Invalid value for {}: {why}", param.hint()));
    match param.kind {
        Kind::Str => Ok(Value::Str(raw.to_string())),
        Kind::Flag => Ok(Value::Bool(true)),
        Kind::Int { min, max } => {
            let ranged = min.is_some() || max.is_some();
            let Some(n) = py_int(raw) else {
                let what = if ranged { "int range" } else { "int" };
                return Err(invalid(format!("{} is not a valid {what}.", py_repr(raw))));
            };
            if min.is_some_and(|lo| n < lo) || max.is_some_and(|hi| n > hi) {
                return Err(invalid(format!("{n} is not in the range {}.", describe_range(min, max))));
            }
            Ok(Value::Int(n))
        }
        Kind::Path { exists, file_okay, dir_okay } => {
            let label = match (file_okay, dir_okay) {
                (true, false) => "File",
                (false, true) => "Directory",
                _ => "Path",
            };
            let path = Path::new(raw);
            match std::fs::metadata(path) {
                Err(_) if exists => Err(invalid(format!("{label} {} does not exist.", py_repr(raw)))),
                Ok(meta) if !file_okay && meta.is_file() => {
                    Err(invalid(format!("{label} {} is a file.", py_repr(raw))))
                }
                Ok(meta) if !dir_okay && meta.is_dir() => {
                    Err(invalid(format!("{label} {} is a directory.", py_repr(raw))))
                }
                _ => Ok(Value::Str(raw.to_string())),
            }
        }
    }
}

/// `int(text)` as click's INT type accepts it: optional sign and surrounding whitespace, `_` between digits.
fn py_int(raw: &str) -> Option<i64> {
    let text = tellfold_models::text::py_strip(raw);
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let cleaned: String = digits.chars().filter(|c| *c != '_').collect();
    tellfold_engine::pytext::decimal_int(&cleaned).map(|n| sign * n)
}

/// Parses `args` (after the command name) against `params`.
pub fn parse(params: &[Param], args: &[String]) -> Result<Parsed, UsageError> {
    let mut parsed = Parsed::default();
    // values as typed, per parameter, in the order the parameters first appeared
    let mut given: Vec<(usize, Vec<String>, Option<bool>)> = Vec::new();
    let mut positional: Vec<String> = Vec::new();
    let longs: Vec<(&str, usize, bool)> = params
        .iter()
        .enumerate()
        .flat_map(|(i, p)| {
            p.opts.iter().map(move |o| (*o, i, true)).chain(p.secondary.iter().map(move |o| (*o, i, false)))
        })
        .collect();
    let record = |index: usize,
                  value: Option<String>,
                  flag: Option<bool>,
                  given: &mut Vec<(usize, Vec<String>, Option<bool>)>| {
        match given.iter_mut().find(|(i, _, _)| *i == index) {
            Some(entry) => {
                if params[index].many {
                    entry.1.extend(value);
                } else {
                    entry.1 = value.into_iter().collect();
                }
                if flag.is_some() {
                    entry.2 = flag;
                }
            }
            None => given.push((index, value.into_iter().collect(), flag)),
        }
    };
    let mut tokens = args.iter();
    let mut only_args = false;
    while let Some(arg) = tokens.next() {
        if only_args || arg == "-" || !arg.starts_with('-') {
            positional.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_args = true;
            continue;
        }
        if arg == "--help" {
            parsed.help = true;
            continue;
        }
        // `--opt=value`, or a short option with its value attached (`-qvalue`)
        let (name, value): (String, Option<String>) = if arg.starts_with("--") {
            match arg.split_once('=') {
                Some((name, value)) => (name.into(), Some(value.into())),
                None => (arg.clone(), None),
            }
        } else {
            let rest: String = arg.chars().skip(2).collect();
            (arg.chars().take(2).collect(), (!rest.is_empty()).then_some(rest))
        };
        let Some(&(_, index, positive)) = longs.iter().find(|(o, _, _)| *o == name) else {
            // click suggests only among the long options (and --help) of the command
            let mut known: Vec<&str> = longs.iter().map(|(o, _, _)| *o).filter(|o| o.starts_with("--")).collect();
            known.push("--help");
            let suggest = if name.starts_with("--") { known } else { Vec::new() };
            return Err(no_such_option(&name, &suggest));
        };
        let param = &params[index];
        if param.kind == Kind::Flag {
            if value.is_some() && name.starts_with("--") {
                return Err(UsageError::bare(format!("Option '{name}' does not take a value.")));
            }
            record(index, None, Some(positive), &mut given);
            continue;
        }
        let value = match value {
            Some(v) => v,
            None => match tokens.next() {
                Some(v) => v.clone(),
                None => return Err(UsageError::bare(format!("Option '{name}' requires an argument."))),
            },
        };
        record(index, Some(value), None, &mut given);
    }

    // arguments take the positional values in order; a `many` argument takes what the others leave
    let arguments: Vec<usize> = params.iter().enumerate().filter(|(_, p)| p.is_argument).map(|(i, _)| i).collect();
    let fixed = arguments.iter().filter(|i| !params[**i].many).count();
    let mut rest = positional.into_iter();
    let mut extra: Vec<String> = Vec::new();
    let total = rest.len();
    for &index in &arguments {
        let take = if params[index].many { total.saturating_sub(fixed) } else { 1 };
        let values: Vec<String> = rest.by_ref().take(take).collect();
        if !values.is_empty() {
            given.push((index, values, None));
        }
    }
    extra.extend(rest);

    if parsed.help {
        return Ok(parsed);
    }
    // click processes the parameters given on the command line first, in that order, then the rest as declared
    let mut order: Vec<usize> = given.iter().map(|(i, _, _)| *i).collect();
    let rest_order: Vec<usize> = (0..params.len()).filter(|i| !order.contains(i)).collect();
    order.extend(rest_order);
    for index in order {
        let param = &params[index];
        let entry = given.iter().find(|(i, _, _)| *i == index);
        let value = match (param.kind, entry) {
            (Kind::Flag, Some((_, _, flag))) => Value::Bool(flag.unwrap_or(true)),
            (Kind::Flag, None) => Value::Bool(param.default == Some("true")),
            (_, Some((_, values, _))) if param.many => {
                for v in values {
                    convert(param, v)?;
                }
                Value::List(values.clone())
            }
            (_, Some((_, values, _))) => convert(param, &values[0])?,
            (_, None) => {
                let from_env = param.envvar.iter().find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()));
                match from_env {
                    Some(v) => convert(param, &v)?,
                    None if param.required => {
                        let what = if param.is_argument { "argument" } else { "option" };
                        return Err(UsageError::new(format!("Missing {what} {}.", param.hint())));
                    }
                    None if param.many => Value::List(Vec::new()),
                    None => match param.default {
                        Some(d) => convert(param, d)?,
                        None => Value::None,
                    },
                }
            }
        };
        parsed.values.insert(param.name, value);
    }
    if !extra.is_empty() {
        let extra = extra.iter().map(|a| escape_control(a)).collect::<Vec<_>>().join(" ");
        return Err(UsageError::new(format!("Got unexpected extra argument(s) ({extra})")));
    }
    Ok(parsed)
}

fn no_such_option(name: &str, known: &[&str]) -> UsageError {
    let mut close = get_close_matches(name, known, 3, 0.6);
    let mut message = format!("No such option: {}", escape_control(name));
    // typer shows click's close matches sorted, one or more alike
    if !close.is_empty() {
        close.sort();
        message.push_str(&format!(" (Possible options: {})", close.join(", ")));
    }
    UsageError::new(message)
}

/// `difflib.get_close_matches`: up to `n` of `possibilities` whose `SequenceMatcher.ratio()` with `word` is at least
/// `cutoff`, best first.
pub fn get_close_matches<'a>(word: &str, possibilities: &[&'a str], n: usize, cutoff: f64) -> Vec<&'a str> {
    let mut scored: Vec<(f64, &str)> = possibilities
        .iter()
        .filter_map(|p| {
            let r = ratio(p, word);
            (r >= cutoff).then_some((r, *p))
        })
        .collect();
    // heapq.nlargest on (score, string): the bigger string first on ties
    scored.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(n).map(|(_, p)| p).collect()
}

/// `SequenceMatcher(None, a, b).ratio()` for short strings (no autojunk below 200 characters).
fn ratio(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    2.0 * matching(&a, &b) as f64 / total as f64
}

/// The number of matched characters in `SequenceMatcher.get_matching_blocks()`.
fn matching(a: &[char], b: &[char]) -> usize {
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, c) in b.iter().enumerate() {
        b2j.entry(*c).or_default().push(j);
    }
    let mut queue = vec![(0, a.len(), 0, b.len())];
    let mut total = 0;
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest_match(a, b, &b2j, (alo, ahi), (blo, bhi));
        if k > 0 {
            total += k;
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    total
}

/// `SequenceMatcher.find_longest_match` without junk (no extension pass is needed then).
fn longest_match(
    a: &[char],
    b: &[char],
    b2j: &HashMap<char, Vec<usize>>,
    (alo, ahi): (usize, usize),
    (blo, bhi): (usize, usize),
) -> (usize, usize, usize) {
    let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
    let mut j2len: HashMap<usize, usize> = HashMap::new();
    for (i, c) in a.iter().enumerate().take(ahi).skip(alo) {
        let mut new: HashMap<usize, usize> = HashMap::new();
        for &j in b2j.get(c).map(Vec::as_slice).unwrap_or_default() {
            if j < blo {
                continue;
            }
            if j >= bhi {
                break;
            }
            let k = j.checked_sub(1).and_then(|p| j2len.get(&p)).copied().unwrap_or(0) + 1;
            new.insert(j, k);
            if k > bestsize {
                besti = i + 1 - k;
                bestj = j + 1 - k;
                bestsize = k;
            }
        }
        j2len = new;
    }
    debug_assert!(b.len() >= bestj + bestsize);
    (besti, bestj, bestsize)
}
