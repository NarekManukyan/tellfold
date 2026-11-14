//! Builds every Tellfold logo file in `brand/` from one geometry.
//!
//! The mark is a folding fan: pleats (the simulated crowd) fold into one hinge (the answer). Edit this file, never
//! the SVGs it writes.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};

/// Wordmark ink on light grounds.
pub const INK: &str = "#1E1B4B";
/// Calm Readout primary, used for one-colour brand versions.
pub const PRIMARY: &str = "#4F46E5";
pub const COLORS: [&str; 3] = ["#6366F1", "#4338CA", "#312E81"];
pub const REVERSED: [&str; 3] = ["#E0E7FF", "#A5B4FC", "#FFFFFF"];

/// Hinge centre on the 256 grid (bbox 38..214, optically centred).
pub const PIVOT: (f64, f64) = (54.0, 198.0);
pub const RADIUS: f64 = 160.0;

/// The `brand/` folder of the repository this tool was built from.
pub fn brand_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../brand")
}

/// A coordinate with at most two decimals and no trailing zeros.
pub fn fmt(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { &s };
    if s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

/// One pleat between angles `a0` and `a1` (degrees, 0 = right, counter-clockwise), with constant-width gaps of `gap`
/// along both radial edges and cut back to circle `ring`.
fn pleat_path(a0: f64, a1: f64, gap: f64, ring: f64) -> String {
    let (px, py) = PIVOT;
    let h = gap / 2.0;
    let mut pts = Vec::new();
    for (a, side) in [(a0, 1), (a1, -1)] {
        let r = a * (PI / 180.0);
        let (ux, uy) = (r.cos(), -r.sin());
        // normal pointing into the pleat (towards larger angles on the a0 edge, smaller on a1)
        let (nx, ny) = if side == 1 { (uy, -ux) } else { (-uy, ux) };
        let t_in = if ring != 0.0 { (ring * ring - h * h).max(0.0).sqrt() } else { 0.0 };
        let inner = (px + ux * t_in + nx * h, py + uy * t_in + ny * h);
        let outer = (px + ux * RADIUS + nx * h, py + uy * RADIUS + ny * h);
        pts.push((inner, outer));
    }
    let ((i0, o0), (i1, o1)) = (pts[0], pts[1]);
    let mut d = format!(
        "M{} {}L{} {}L{} {}L{} {}",
        fmt(i0.0),
        fmt(i0.1),
        fmt(o0.0),
        fmt(o0.1),
        fmt(o1.0),
        fmt(o1.1),
        fmt(i1.0),
        fmt(i1.1)
    );
    if ring != 0.0 {
        d += &format!("A{} {} 0 0 1 {} {}", fmt(ring), fmt(ring), fmt(i0.0), fmt(i0.1));
    }
    d + "Z"
}

/// One cut of the symbol: pleat count, gap, hinge radius and the ring the pleats are cut back to.
#[derive(Clone, Copy)]
pub struct Cut {
    pub pleats: u32,
    pub gap: f64,
    pub hub: u32,
    pub ring: f64,
}

pub const FULL: Cut = Cut { pleats: 6, gap: 0.0, hub: 16, ring: 0.0 };
pub const FULL_MONO: Cut = Cut { pleats: 6, gap: 7.0, hub: 16, ring: 7.0 };
pub const SMALL: Cut = Cut { pleats: 4, gap: 0.0, hub: 22, ring: 0.0 };
pub const SMALL_MONO: Cut = Cut { pleats: 4, gap: 14.0, hub: 22, ring: 12.0 };

/// SVG body of the symbol. `mono` gives the one-colour cut (gaps carry the folds).
pub fn mark(cut: Cut, colors: [&str; 3], mono: Option<&str>) -> String {
    let step = 90.0 / cut.pleats as f64;
    let ring = if cut.ring != 0.0 { cut.hub as f64 + cut.ring } else { 0.0 };
    let mut out = String::new();
    for i in 0..cut.pleats {
        let fill = mono.unwrap_or(colors[(i % 2) as usize]);
        let d = pleat_path(i as f64 * step, (i + 1) as f64 * step, cut.gap, ring);
        out += &format!(r#"<path fill="{fill}" d="{d}"/>"#);
    }
    out +=
        &format!(r#"<circle fill="{}" cx="{}" cy="{}" r="{}"/>"#, mono.unwrap_or(colors[2]), PIVOT.0, PIVOT.1, cut.hub);
    out
}

/// The wordmark's paths, drawn by hand in `brand/src/wordmark-source.svg`.
pub fn wordmark_paths(source: &str) -> Result<String, String> {
    let pattern = regex::Regex::new(r##"(?s)<g id="wordmark" fill="#000">(.*)</g>"##).expect("pattern");
    pattern.captures(source).map(|c| c[1].to_string()).ok_or_else(|| "no wordmark group in the source".into())
}

pub fn svg(body: &str, vb: (i32, i32, i32, i32), title: &str) -> String {
    let (x, y, w, h) = vb;
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x} {y} {w} {h}" width="{w}" height="{h}" role="img" aria-labelledby="title"><title id="title">{title}</title>{body}</svg>"#
    ) + "\n"
}

fn squircle(cx: f64, cy: f64, half: f64, n: f64, steps: u32) -> String {
    let pts: Vec<String> = (0..steps)
        .map(|i| {
            let t = 2.0 * PI * i as f64 / steps as f64;
            let (c, s) = (t.cos(), t.sin());
            let x = cx + half * c.abs().powf(2.0 / n).copysign(c);
            let y = cy + half * s.abs().powf(2.0 / n).copysign(s);
            format!("{x:.1} {y:.1}")
        })
        .collect();
    format!("M{}Z", pts.join("L"))
}

/// The macOS icon on the 1024 grid: 824 pt body (100 pt margin), glyph centred optically.
pub fn app_icon(glyph: &str, scale: f64) -> String {
    let sq = squircle(512.0, 512.0, 412.0, 5.0, 256);
    let t = 512.0 - 126.0 * scale;
    concat!(
        "<defs>",
        r##"<linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FFFFFF"/><stop offset="1" stop-color="#E2E5FF"/></linearGradient>"##,
        r##"<linearGradient id="sheen" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FFFFFF" stop-opacity=".6"/><stop offset=".45" stop-color="#FFFFFF" stop-opacity="0"/></linearGradient>"##,
        r##"<filter id="glyph" x="-20%" y="-20%" width="140%" height="150%"><feDropShadow dx="0" dy="14" stdDeviation="16" flood-color="#1E1B4B" flood-opacity=".28"/></filter>"##,
        r##"<filter id="body" x="-10%" y="-10%" width="120%" height="125%"><feDropShadow dx="0" dy="10" stdDeviation="12" flood-color="#000000" flood-opacity=".22"/></filter>"##,
        "</defs>",
    )
    .to_string()
        + &format!(r#"<path d="{sq}" fill="url(#bg)" filter="url(#body)"/>"#)
        + &format!(r#"<path d="{sq}" fill="url(#sheen)"/>"#)
        + &format!(r##"<path d="{sq}" fill="none" stroke="#FFFFFF" stroke-opacity=".7" stroke-width="3"/>"##)
        + &format!(r#"<g filter="url(#glyph)" transform="translate({} {}) scale({scale})">{glyph}</g>"#, fmt(t), fmt(t))
}

/// Every generated file: (path under `brand/`, contents).
pub fn files(wordmark_source: &str) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut write = |name: &str, text: String| out.push((name.to_string(), text));
    let sym_vb = (0, 0, 256, 256);
    let ones = [("black", "#000000"), ("white", "#FFFFFF"), ("indigo", PRIMARY)];
    write("tellfold-symbol.svg", svg(&mark(FULL, COLORS, None), sym_vb, "Tellfold symbol"));
    write("tellfold-symbol-reversed.svg", svg(&mark(FULL, REVERSED, None), sym_vb, "Tellfold symbol, reversed"));
    write("tellfold-symbol-small.svg", svg(&mark(SMALL, COLORS, None), sym_vb, "Tellfold symbol, 16 to 32 px cut"));
    for (tag, col) in ones {
        write(
            &format!("one-colour/tellfold-symbol-{tag}.svg"),
            svg(&mark(FULL_MONO, COLORS, Some(col)), sym_vb, &format!("Tellfold symbol, one colour {tag}")),
        );
        write(
            &format!("one-colour/tellfold-symbol-small-{tag}.svg"),
            svg(&mark(SMALL_MONO, COLORS, Some(col)), sym_vb, &format!("Tellfold small symbol, one colour {tag}")),
        );
    }

    let wm = wordmark_paths(wordmark_source)?;
    let wvb = (0, 30, 672, 176);
    write("tellfold-wordmark.svg", svg(&format!(r#"<g fill="{INK}">{wm}</g>"#), wvb, "Tellfold wordmark"));
    write(
        "tellfold-wordmark-white.svg",
        svg(&format!(r##"<g fill="#FFFFFF">{wm}</g>"##), wvb, "Tellfold wordmark, white"),
    );

    // Horizontal lockup: symbol bbox 38..214, wordmark at 0.62 starting 48 units after the symbol.
    let horizontal = |sym: String, ink: &str| {
        format!(
            r#"<g id="symbol">{sym}</g><g id="wordmark" fill="{ink}" transform="translate(262 55) scale(0.62)">{wm}</g>"#
        )
    };
    let hvb = (30, 30, 646, 192);
    write("tellfold-lockup-horizontal.svg", svg(&horizontal(mark(FULL, COLORS, None), INK), hvb, "Tellfold logo"));
    write(
        "tellfold-lockup-horizontal-reversed.svg",
        svg(&horizontal(mark(FULL, REVERSED, None), "#FFFFFF"), hvb, "Tellfold logo, reversed"),
    );
    for (tag, col) in ones {
        write(
            &format!("one-colour/tellfold-lockup-horizontal-{tag}.svg"),
            svg(&horizontal(mark(FULL_MONO, COLORS, Some(col)), col), hvb, &format!("Tellfold logo, one colour {tag}")),
        );
    }

    // Stacked lockup: symbol centred over the wordmark.
    let stacked = |sym: String, ink: &str| {
        format!(
            r#"<g id="symbol" transform="translate(47 0)">{sym}</g><g id="wordmark" fill="{ink}" transform="translate(10 232) scale(0.5)">{wm}</g>"#
        )
    };
    let svb = (0, 28, 346, 314);
    write("tellfold-lockup-stacked.svg", svg(&stacked(mark(FULL, COLORS, None), INK), svb, "Tellfold logo, stacked"));
    write(
        "tellfold-lockup-stacked-reversed.svg",
        svg(&stacked(mark(FULL, REVERSED, None), "#FFFFFF"), svb, "Tellfold logo, stacked, reversed"),
    );

    let ivb = (0, 0, 1024, 1024);
    write("app-icon/tellfold-app-icon.svg", svg(&app_icon(&mark(FULL, COLORS, None), 3.2), ivb, "Tellfold app icon"));
    write(
        "app-icon/tellfold-app-icon-small.svg",
        svg(&app_icon(&mark(SMALL, COLORS, None), 3.5), ivb, "Tellfold app icon, 16 and 32 px"),
    );
    Ok(out)
}

/// The macOS AppIcon slots: (file name, pixel size, use the small icon).
pub const APPICON_SLOTS: &[(&str, u32, bool)] = &[
    ("icon_16x16.png", 16, true),
    ("icon_16x16@2x.png", 32, true),
    ("icon_32x32.png", 32, true),
    ("icon_32x32@2x.png", 64, false),
    ("icon_128x128.png", 128, false),
    ("icon_128x128@2x.png", 256, false),
    ("icon_256x256.png", 256, false),
    ("icon_256x256@2x.png", 512, false),
    ("icon_512x512.png", 512, false),
    ("icon_512x512@2x.png", 1024, false),
];
