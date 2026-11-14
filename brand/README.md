# Tellfold logo: files and usage

The mark is a **folding fan**. Its six pleats are the simulated crowd, and they fold into one hinge, which is the answer. The fold shading comes from the name, and nothing in it is a fish or a swarm. The wordmark is drawn by hand from geometric strokes (circles and straight stems), so it needs no font licence.

## Files

| File | Use |
|---|---|
| `tellfold-symbol.svg` | Master symbol, two indigo tones and a dark hinge. Use from 48 px up. |
| `tellfold-symbol-small.svg` | 4-pleat cut for 16 to 47 px (favicons, menu bar, small lists). |
| `tellfold-symbol-reversed.svg` | Symbol for indigo or dark backgrounds. |
| `tellfold-wordmark.svg`, `-white.svg` | Wordmark only. |
| `tellfold-lockup-horizontal.svg`, `-reversed.svg` | Default logo: symbol and wordmark side by side. |
| `tellfold-lockup-stacked.svg`, `-reversed.svg` | For square spaces: symbol above the wordmark. |
| `one-colour/*` | Black, white and indigo (#4F46E5) versions. White gaps carry the folds. Use these for print, embossing, etching, and any surface that only takes one ink. |
| `app-icon/tellfold-app-icon.svg` | macOS icon master on the 1024 grid: 824-unit squircle body and 100-unit margin, glyph about 68 % of the body. |
| `app-icon/tellfold-app-icon-small.svg` | Icon used for the 16 and 32 px slots. |
| `web/` | `favicon.svg`, `favicon.ico` (16/32/48), PNG favicons, `apple-touch-icon.png`, PWA icons (192, 512, maskable 512), `site.webmanifest`, and `head-snippet.html`. |
| `engine-rs/crates/tellfold-brand` | Generates every SVG above from one geometry (`src/wordmark-source.svg` holds the hand-drawn wordmark). Edit it, never the outputs; its test fails when a committed SVG differs from what it writes. |

The macOS app icon PNGs live in `mac/Tellfold/App/Assets.xcassets/AppIcon.appiconset`, and `mac/project.yml` names `AppIcon` as the icon set.

## Colour

| Role | HEX | RGB |
|---|---|---|
| Pleat, light | #6366F1 | 99, 102, 241 |
| Pleat, dark | #4338CA | 67, 56, 202 |
| Hinge | #312E81 | 49, 46, 129 |
| One-colour brand | #4F46E5 | 79, 70, 229 |
| Wordmark ink | #1E1B4B | 30, 27, 75 |
| Reversed pleats | #E0E7FF / #A5B4FC, hinge #FFFFFF | |

These are the app's indigo colours. For print, convert with the printer's profile and proof it, because indigo shifts a lot in CMYK.

## Rules

- **Clear space:** keep at least one hinge diameter (32 units on the 256 grid) of empty space around the symbol and the lockups.
- **Minimum size:** symbol 16 px, using the small cut below 48 px. Horizontal lockup 96 px wide. Stacked lockup 64 px wide.
- **Backgrounds:** use the colour version on white, `#F7F8FA` and light indigo. Use the reversed version on `#4F46E5`, `#312E81` and dark UI grounds (`#0E1013`, `#16191E`). On photos, use a one-colour version over a calm area.
- **Don't** recolour individual pleats, add more pleats, rotate or mirror the fan, add gradients or glows to the flat mark, outline it, set the wordmark in another typeface, or change the spacing between symbol and wordmark.
- The app icon is the only place the mark gets depth (soft gradient tile and shadow). Everywhere else it stays flat.

## Rebuilding

```bash
cd engine-rs && cargo run -p tellfold-brand -- --appicon
```

`--appicon` also re-renders the AppIcon PNGs with `rsvg-convert`; the 16, 16@2x and 32 slots use the small icon. Then rebuild with `cd mac && xcodegen && xcodebuild build -scheme Tellfold -derivedDataPath /private/tmp/tellfold-dd`. If the Dock still shows an old icon after a rebuild, run `lsregister -f` on the built app (LaunchServices caches icons per bundle path).
