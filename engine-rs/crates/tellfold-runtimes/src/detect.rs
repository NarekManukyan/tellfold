//! Find agent CLIs on disk and read their versions, quickly and without a login shell (port of
//! `tellfold/runtimes/detect.py`).
//!
//! The Mac app starts the engine with a minimal PATH, so lookup also searches the usual install folders:
//! ~/.local/bin, Homebrew, /usr/local/bin, and npm / bun / volta / nvm global bins. Child processes get the same
//! extended PATH, because most of these CLIs are Node scripts that run `#!/usr/bin/env node`.

use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime};

use regex::Regex;
use tellfold_models::text::{py_prefix, py_splitlines, py_strip};

use crate::settings::home_dir;

pub const VERSION_TIMEOUT_S: f64 = 3.0;
pub const AUTH_TIMEOUT_S: f64 = 5.0;

fn ansi() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07").expect("valid pattern"))
}

pub fn strip_ansi(text: &str) -> String {
    ansi().replace_all(text, "").replace('\r', "")
}

static EXTRA_DIRS_OVERRIDE: RwLock<Option<Vec<PathBuf>>> = RwLock::new(None);

/// Replaces [`extra_bin_dirs`] (tests keep the real machine's Homebrew and npm folders out).
#[doc(hidden)]
pub fn override_extra_bin_dirs(dirs: Option<Vec<PathBuf>>) {
    *EXTRA_DIRS_OVERRIDE.write().expect("not poisoned") = dirs;
}

pub fn extra_bin_dirs() -> Vec<PathBuf> {
    if let Some(dirs) = EXTRA_DIRS_OVERRIDE.read().expect("not poisoned").clone() {
        return dirs;
    }
    let home = home_dir();
    let mut dirs = vec![
        home.join(".local").join("bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".npm-global").join("bin"),
        home.join(".bun").join("bin"),
        home.join(".volta").join("bin"),
        home.join(".opencode").join("bin"),
        home.join(".cargo").join("bin"),
        home.join("bin"),
    ];
    let prefix = std::env::var("NPM_CONFIG_PREFIX").ok().filter(|p| !p.is_empty());
    if let Some(prefix) = prefix.or_else(|| std::env::var("npm_config_prefix").ok().filter(|p| !p.is_empty())) {
        dirs.insert(1, PathBuf::from(prefix).join("bin"));
    }
    if let Ok(entries) = std::fs::read_dir(home.join(".nvm").join("versions").join("node")) {
        let mut versions: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        // sorted(..., reverse=True) is stable: equal keys keep their listing order
        versions.sort_by_key(|p| std::cmp::Reverse(node_version_key(p)));
        dirs.extend(versions.into_iter().map(|v| v.join("bin")));
    }
    dirs
}

fn node_version_key(path: &Path) -> Vec<i64> {
    static R: OnceLock<Regex> = OnceLock::new();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let key: Vec<i64> = R
        .get_or_init(|| Regex::new(r"\d+").expect("valid pattern"))
        .find_iter(&name)
        .take(3)
        .map(|m| m.as_str().parse().unwrap_or(i64::MAX))
        .collect();
    if key.is_empty() {
        vec![0]
    } else {
        key
    }
}

/// PATH followed by the extra install folders (deduplicated).
pub fn search_path() -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    let mut parts: Vec<String> = path.split(':').filter(|p| !p.is_empty()).map(str::to_string).collect();
    for d in extra_bin_dirs() {
        let d = d.to_string_lossy().into_owned();
        if !parts.contains(&d) {
            parts.push(d);
        }
    }
    for d in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        if !parts.iter().any(|p| p == d) {
            parts.push(d.to_string());
        }
    }
    parts.join(":")
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// `shutil.which(name, path=path)`.
pub fn which_in(name: &str, path: &str) -> Option<String> {
    if name.contains('/') {
        return is_executable_file(Path::new(name)).then(|| name.to_string());
    }
    path.split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(name))
        .find(|candidate| is_executable_file(candidate))
        .map(|p| p.to_string_lossy().into_owned())
}

pub fn find_binary(names: &[&str]) -> Option<String> {
    let path = search_path();
    names.iter().find_map(|name| which_in(name, &path))
}

/// Variables a CLI child may inherit from the engine. Everything else is dropped: the Mac app puts every Keychain
/// secret (ANTHROPIC_API_KEY, OPENAI_API_KEY, TELLFOLD_KEY_*) and the engine's own bearer token
/// (TELLFOLD_SERVE_TOKEN) in the engine's environment, and none of them may reach a third-party agent CLI. A
/// runtime that needs one of its own variables (GEMINI_API_KEY, CURSOR_API_KEY, ...) names it in `passthrough`.
pub const CHILD_ENV_ALLOW: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "TMPDIR",
    "TZ",
    "TERM",
    "COLORTERM",
    "__CF_USER_TEXT_ENCODING",
    "SSH_AUTH_SOCK",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
    "NVM_DIR",
    "NVM_BIN",
    "VOLTA_HOME",
    "BUN_INSTALL",
    "NPM_CONFIG_PREFIX",
    "npm_config_prefix",
];
pub const CHILD_ENV_ALLOW_PREFIXES: &[&str] = &["LC_", "XDG_"];
/// Never passed to a child, even when a runtime asks for it. The SHOAL_ names are the same secrets from before
/// the app was renamed.
pub const CHILD_ENV_DENY: &[&str] =
    &["TELLFOLD_SERVE_TOKEN", "SHOAL_SERVE_TOKEN", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"];
pub const CHILD_ENV_DENY_PREFIXES: &[&str] = &["TELLFOLD_KEY_", "SHOAL_KEY_"];

/// True for the engine's own secrets, which no child process may see.
pub fn is_secret_env(name: &str) -> bool {
    CHILD_ENV_DENY.contains(&name) || CHILD_ENV_DENY_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Environment for a CLI child: an allowlist of ours (plus `passthrough`), with the extended PATH (binary's own
/// folder first). Engine secrets are always removed.
pub fn child_env(binary: Option<&str>, extra: &[(&str, &str)], passthrough: &[&str]) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = std::env::vars()
        .filter(|(k, _)| {
            (CHILD_ENV_ALLOW.contains(&k.as_str())
                || CHILD_ENV_ALLOW_PREFIXES.iter().any(|p| k.starts_with(p))
                || passthrough.contains(&k.as_str()))
                && !is_secret_env(k)
        })
        .collect();
    let mut path = search_path();
    if let Some(binary) = binary {
        let folder = Path::new(binary).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let rest: Vec<&str> = path.split(':').filter(|p| *p != folder).collect();
        path = std::iter::once(folder.as_str()).chain(rest).collect::<Vec<_>>().join(":");
    }
    env.insert("PATH".into(), path);
    env.entry("NO_COLOR".into()).or_insert_with(|| "1".into());
    env.entry("TERM".into()).or_insert_with(|| "dumb".into());
    // fewer interactive niceties (spinners, update prompts) in most Node CLIs
    env.entry("CI".into()).or_insert_with(|| "1".into());
    for (k, v) in extra {
        env.insert((*k).into(), (*v).into());
    }
    env
}

fn neutral_cwd() -> PathBuf {
    let tmp = std::env::var("TMPDIR").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| "/tmp".into());
    if Path::new(&tmp).is_dir() {
        PathBuf::from(tmp)
    } else {
        PathBuf::from("/")
    }
}

/// Run a short command: (exit code, or None on timeout or error; combined output stripped of ANSI).
pub async fn run_quick(argv: &[String], timeout_s: f64) -> (Option<i32>, String) {
    let Some((program, args)) = argv.split_first() else { return (None, String::new()) };
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .env_clear()
        .envs(child_env(Some(program), &[], &[]))
        .current_dir(neutral_cwd())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let Ok(child) = command.spawn() else { return (None, String::new()) };
    let Ok(Ok(output)) = tokio::time::timeout(Duration::from_secs_f64(timeout_s), child.wait_with_output()).await
    else {
        return (None, String::new());
    };
    let text = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    (Some(output.status.code().unwrap_or(-1)), py_strip(&strip_ansi(&text)).to_string())
}

pub fn parse_version(output: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let version = R.get_or_init(|| Regex::new(r"\d+\.\d+(?:\.\d+)?(?:[-+.][0-9A-Za-z.-]+)?").expect("valid pattern"));
    for line in py_splitlines(output) {
        if let Some(m) = version.find(line) {
            return Some(m.as_str().to_string());
        }
    }
    let stripped = py_strip(output);
    py_splitlines(stripped).first().filter(|_| !stripped.is_empty()).map(|line| py_prefix(line, 60).to_string())
}

type VersionKey = (String, Option<SystemTime>, Vec<String>);

/// `<binary> --version` with a 3 s timeout, cached per (path, mtime).
pub async fn binary_version(path: &str, args: &[&str]) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashMap<VersionKey, Option<String>>>> = OnceLock::new();
    let mtime = std::fs::metadata(path).ok()?.modified().ok();
    let key = (path.to_string(), mtime, args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().expect("not poisoned").get(&key) {
        return hit.clone();
    }
    let argv: Vec<String> = std::iter::once(path.to_string()).chain(key.2.iter().cloned()).collect();
    let (code, out) = run_quick(&argv, VERSION_TIMEOUT_S).await;
    let version = code.and_then(|_| parse_version(&out));
    cache.lock().expect("not poisoned").insert(key, version.clone());
    version
}
