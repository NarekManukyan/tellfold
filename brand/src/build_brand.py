"""Builds every Tellfold logo file in brand/ from one geometry.

The mark is a folding fan: pleats (the simulated crowd) fold into one hinge (the answer).
Run: python3 brand/src/build_brand.py
"""
import math, pathlib, re, shutil, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "src"

INK = "#1E1B4B"          # wordmark ink on light grounds
PRIMARY = "#4F46E5"      # Calm Readout primary, used for one-colour brand versions
LIGHT, DARK, HUB = "#6366F1", "#4338CA", "#312E81"
REV_LIGHT, REV_DARK, REV_HUB = "#E0E7FF", "#A5B4FC", "#FFFFFF"

PIVOT = (54, 198)        # hinge centre on the 256 grid (bbox 38..214, optically centred)
RADIUS = 160


def fmt(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s == "-0" else s


def pleat_path(a0, a1, gap, ring):
    """One pleat between angles a0 and a1 (degrees, 0 = right, counter-clockwise),
    with constant-width gaps of `gap` along both radial edges and cut back to circle `ring`."""
    px, py = PIVOT
    h = gap / 2
    pts = []
    for a, side in ((a0, 1), (a1, -1)):
        r = math.radians(a)
        ux, uy = math.cos(r), -math.sin(r)
        # normal pointing into the pleat (towards larger angles on the a0 edge, smaller on a1)
        nx, ny = (uy, -ux) if side == 1 else (-uy, ux)
        t_in = math.sqrt(max(ring * ring - h * h, 0)) if ring else 0
        inner = (px + ux * t_in + nx * h, py + uy * t_in + ny * h)
        outer = (px + ux * RADIUS + nx * h, py + uy * RADIUS + ny * h)
        pts.append((inner, outer))
    (i0, o0), (i1, o1) = pts
    d = f"M{fmt(i0[0])} {fmt(i0[1])}L{fmt(o0[0])} {fmt(o0[1])}L{fmt(o1[0])} {fmt(o1[1])}L{fmt(i1[0])} {fmt(i1[1])}"
    if ring:
        d += f"A{fmt(ring)} {fmt(ring)} 0 0 1 {fmt(i0[0])} {fmt(i0[1])}"
    return d + "Z"


def mark(pleats=6, gap=0.0, hub=16, ring=0.0, colors=(LIGHT, DARK, HUB), mono=None):
    """SVG body of the symbol. mono=a colour gives the one-colour cut (gaps carry the folds)."""
    step = 90 / pleats
    out = []
    for i in range(pleats):
        fill = mono or colors[i % 2]
        out.append(f'<path fill="{fill}" d="{pleat_path(i * step, (i + 1) * step, gap, hub + ring if ring else 0)}"/>')
    out.append(f'<circle fill="{mono or colors[2]}" cx="{PIVOT[0]}" cy="{PIVOT[1]}" r="{hub}"/>')
    return "".join(out)


FULL = dict(pleats=6, gap=0, hub=16)
FULL_MONO = dict(pleats=6, gap=7, hub=16, ring=7)
SMALL = dict(pleats=4, gap=0, hub=22)
SMALL_MONO = dict(pleats=4, gap=14, hub=22, ring=12)


def wordmark_paths():
    s = (SRC / "wordmark-source.svg").read_text()
    return re.search(r'<g id="wordmark" fill="#000">(.*)</g>', s, re.S).group(1)


def svg(body, vb, title):
    x, y, w, h = vb
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x} {y} {w} {h}" width="{w}" height="{h}" '
            f'role="img" aria-labelledby="title"><title id="title">{title}</title>{body}</svg>\n')


def squircle(cx, cy, half, n=5.0, steps=256):
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        pts.append((cx + half * math.copysign(abs(c) ** (2 / n), c), cy + half * math.copysign(abs(s) ** (2 / n), s)))
    return "M" + "L".join(f"{x:.1f} {y:.1f}" for x, y in pts) + "Z"


def app_icon(glyph, scale):
    """macOS icon on the 1024 grid: 824 pt body (100 pt margin), glyph centred optically."""
    sq = squircle(512, 512, 412)
    tx = 512 - 126 * scale
    ty = 512 - 126 * scale
    return (
        '<defs>'
        '<linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FFFFFF"/><stop offset="1" stop-color="#E2E5FF"/></linearGradient>'
        '<linearGradient id="sheen" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FFFFFF" stop-opacity=".6"/><stop offset=".45" stop-color="#FFFFFF" stop-opacity="0"/></linearGradient>'
        '<filter id="glyph" x="-20%" y="-20%" width="140%" height="150%"><feDropShadow dx="0" dy="14" stdDeviation="16" flood-color="#1E1B4B" flood-opacity=".28"/></filter>'
        '<filter id="body" x="-10%" y="-10%" width="120%" height="125%"><feDropShadow dx="0" dy="10" stdDeviation="12" flood-color="#000000" flood-opacity=".22"/></filter>'
        '</defs>'
        f'<path d="{sq}" fill="url(#bg)" filter="url(#body)"/>'
        f'<path d="{sq}" fill="url(#sheen)"/>'
        f'<path d="{sq}" fill="none" stroke="#FFFFFF" stroke-opacity=".7" stroke-width="3"/>'
        f'<g filter="url(#glyph)" transform="translate({fmt(tx)} {fmt(ty)}) scale({scale})">{glyph}</g>'
    )


def write(name, text):
    p = ROOT / name
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(text)
    return p


def main():
    sym_vb = (0, 0, 256, 256)
    write("tellfold-symbol.svg", svg(mark(**FULL), sym_vb, "Tellfold symbol"))
    write("tellfold-symbol-reversed.svg", svg(mark(**FULL, colors=(REV_LIGHT, REV_DARK, REV_HUB)), sym_vb, "Tellfold symbol, reversed"))
    write("tellfold-symbol-small.svg", svg(mark(**SMALL), sym_vb, "Tellfold symbol, 16 to 32 px cut"))
    for tag, col in (("black", "#000000"), ("white", "#FFFFFF"), ("indigo", PRIMARY)):
        write(f"one-colour/tellfold-symbol-{tag}.svg", svg(mark(**FULL_MONO, mono=col), sym_vb, f"Tellfold symbol, one colour {tag}"))
        write(f"one-colour/tellfold-symbol-small-{tag}.svg", svg(mark(**SMALL_MONO, mono=col), sym_vb, f"Tellfold small symbol, one colour {tag}"))

    wm = wordmark_paths()
    write("tellfold-wordmark.svg", svg(f'<g fill="{INK}">{wm}</g>', (0, 30, 672, 176), "Tellfold wordmark"))
    write("tellfold-wordmark-white.svg", svg(f'<g fill="#FFFFFF">{wm}</g>', (0, 30, 672, 176), "Tellfold wordmark, white"))

    # Horizontal lockup: symbol bbox 38..214, wordmark at 0.62 starting 48 units after the symbol.
    def horizontal(sym, ink):
        return (f'<g id="symbol">{sym}</g>'
                f'<g id="wordmark" fill="{ink}" transform="translate(262 55) scale(0.62)">{wm}</g>')
    hvb = (30, 30, 646, 192)
    write("tellfold-lockup-horizontal.svg", svg(horizontal(mark(**FULL), INK), hvb, "Tellfold logo"))
    write("tellfold-lockup-horizontal-reversed.svg", svg(horizontal(mark(**FULL, colors=(REV_LIGHT, REV_DARK, REV_HUB)), "#FFFFFF"), hvb, "Tellfold logo, reversed"))
    for tag, col in (("black", "#000000"), ("white", "#FFFFFF"), ("indigo", PRIMARY)):
        write(f"one-colour/tellfold-lockup-horizontal-{tag}.svg", svg(horizontal(mark(**FULL_MONO, mono=col), col), hvb, f"Tellfold logo, one colour {tag}"))

    # Stacked lockup: symbol centred over the wordmark.
    def stacked(sym, ink):
        return (f'<g id="symbol" transform="translate(47 0)">{sym}</g>'
                f'<g id="wordmark" fill="{ink}" transform="translate(10 232) scale(0.5)">{wm}</g>')
    svb = (0, 28, 346, 314)
    write("tellfold-lockup-stacked.svg", svg(stacked(mark(**FULL), INK), svb, "Tellfold logo, stacked"))
    write("tellfold-lockup-stacked-reversed.svg", svg(stacked(mark(**FULL, colors=(REV_LIGHT, REV_DARK, REV_HUB)), "#FFFFFF"), svb, "Tellfold logo, stacked, reversed"))

    write("app-icon/tellfold-app-icon.svg", svg(app_icon(mark(**FULL), 3.2), (0, 0, 1024, 1024), "Tellfold app icon"))
    write("app-icon/tellfold-app-icon-small.svg", svg(app_icon(mark(**SMALL), 3.5), (0, 0, 1024, 1024), "Tellfold app icon, 16 and 32 px"))


APPICONSET = ROOT.parent / "mac/Tellfold/App/Assets.xcassets/AppIcon.appiconset"
# (file name, pixel size, use the small icon)
APPICON_SLOTS = [
    ("icon_16x16.png", 16, True), ("icon_16x16@2x.png", 32, True), ("icon_32x32.png", 32, True),
    ("icon_32x32@2x.png", 64, False), ("icon_128x128.png", 128, False), ("icon_128x128@2x.png", 256, False),
    ("icon_256x256.png", 256, False), ("icon_256x256@2x.png", 512, False), ("icon_512x512.png", 512, False),
    ("icon_512x512@2x.png", 1024, False),
]


def render_appiconset():
    """Render the macOS AppIcon PNGs with rsvg-convert (brew install librsvg)."""
    if not shutil.which("rsvg-convert"):
        sys.exit("rsvg-convert not found: brew install librsvg")
    for name, px, small in APPICON_SLOTS:
        src = ROOT / "app-icon" / ("tellfold-app-icon-small.svg" if small else "tellfold-app-icon.svg")
        subprocess.run(["rsvg-convert", "-w", str(px), "-h", str(px), str(src), "-o", str(APPICONSET / name)], check=True)


if __name__ == "__main__":
    main()
    if "--appicon" in sys.argv:
        render_appiconset()
