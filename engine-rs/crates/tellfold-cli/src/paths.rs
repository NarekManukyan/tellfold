//! Paths as Python's pathlib shows and resolves them, so messages print the same text.

use std::path::{Component, Path, PathBuf};

/// `str(Path(text))`: `.` parts and repeated or trailing slashes dropped (`./runs/` is `runs`), `.` for nothing.
pub fn py_path(text: &str) -> PathBuf {
    let root = if text.starts_with("//") && !text.starts_with("///") {
        "//"
    } else if text.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = text.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let joined = format!("{root}{}", parts.join("/"));
    PathBuf::from(if joined.is_empty() { ".".to_string() } else { joined })
}

/// `Path.resolve()` (not strict): absolute, symlinks of the part that exists followed, `..` applied.
pub fn py_resolve(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(path)
    };
    let mut out = PathBuf::from("/");
    let mut exists = true;
    for component in absolute.components() {
        match component {
            Component::RootDir | Component::Prefix(_) | Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => {
                out.push(part);
                if exists {
                    match std::fs::canonicalize(&out) {
                        Ok(real) => out = real,
                        Err(_) => exists = false,
                    }
                }
            }
        }
    }
    out
}

/// Python's text for an `OSError` about `path`: `[Errno 2] No such file or directory: 'runs/x'`.
pub fn py_os_error(error: &std::io::Error, path: &Path) -> String {
    match error.raw_os_error() {
        Some(code) => {
            let text = error.to_string();
            let reason = text.split(" (os error").next().unwrap_or(&text);
            format!("[Errno {code}] {reason}: {}", crate::args::py_repr(&path.to_string_lossy()))
        }
        None => error.to_string(),
    }
}

/// `Path.as_uri()` of an absolute path.
pub fn as_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"_.-~/".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_print_like_pathlib() {
        for (given, shown) in [
            ("runs", "runs"),
            ("./runs/", "runs"),
            ("a//b/./c", "a/b/c"),
            ("", "."),
            (".", "."),
            ("/", "/"),
            ("//x", "//x"),
            ("///x", "/x"),
            ("../a", "../a"),
        ] {
            assert_eq!(py_path(given), PathBuf::from(shown), "{given}");
        }
    }

    #[test]
    fn uris_quote_like_pathlib() {
        assert_eq!(as_uri(Path::new("/tmp/a b/ü.html")), "file:///tmp/a%20b/%C3%BC.html");
    }
}
