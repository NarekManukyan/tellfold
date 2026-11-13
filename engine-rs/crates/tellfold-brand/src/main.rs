//! `cargo run -p tellfold-brand` rewrites every logo SVG in `brand/`; `--appicon` also renders the macOS AppIcon PNGs
//! with `rsvg-convert` (`brew install librsvg`).

use std::process::{Command, ExitCode};

use tellfold_brand::{brand_dir, files, APPICON_SLOTS};

fn main() -> ExitCode {
    let root = brand_dir();
    let source = match std::fs::read_to_string(root.join("src/wordmark-source.svg")) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("brand/src/wordmark-source.svg: {e}");
            return ExitCode::FAILURE;
        }
    };
    let files = match files(&source) {
        Ok(files) => files,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    for (name, text) in files {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create the brand folders");
        }
        std::fs::write(&path, text).expect("write a brand file");
    }
    if std::env::args().any(|a| a == "--appicon") {
        let set = root.join("../mac/Tellfold/App/Assets.xcassets/AppIcon.appiconset");
        for (name, px, small) in APPICON_SLOTS {
            let src = root.join("app-icon").join(if *small {
                "tellfold-app-icon-small.svg"
            } else {
                "tellfold-app-icon.svg"
            });
            let size = px.to_string();
            let status = Command::new("rsvg-convert")
                .args(["-w", &size, "-h", &size])
                .arg(&src)
                .arg("-o")
                .arg(set.join(name))
                .status();
            match status {
                Ok(s) if s.success() => {}
                Ok(s) => {
                    eprintln!("rsvg-convert failed for {name}: {s}");
                    return ExitCode::FAILURE;
                }
                Err(_) => {
                    eprintln!("rsvg-convert not found: brew install librsvg");
                    return ExitCode::FAILURE;
                }
            }
        }
    }
    ExitCode::SUCCESS
}
