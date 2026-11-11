//! Embeds the Studio frontend (`tellfold/web`) so `tellfold ui` needs no files next to the binary.

use std::path::{Path, PathBuf};

fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with('.') || name == "__pycache__" || name.ends_with(".py") {
            continue;
        }
        if path.is_dir() {
            walk(&path, root, out);
        } else {
            let rel = path.strip_prefix(root).expect("under the root").to_string_lossy().replace('\\', "/");
            out.push((rel, path));
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let web = manifest.join("../../../tellfold/web");
    println!("cargo:rerun-if-changed={}", web.display());
    let mut files = Vec::new();
    if let Ok(root) = web.canonicalize() {
        walk(&root, &root, &mut files);
    }
    let mut code = String::from(
        "/// The frontend files: (path under tellfold/web, contents).\npub static FILES: &[(&str, &[u8])] = &[\n",
    );
    for (rel, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        code.push_str(&format!("    ({rel:?}, include_bytes!({:?})),\n", path.display().to_string()));
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets it")).join("web_files.rs");
    std::fs::write(out, code).expect("write web_files.rs");
}
