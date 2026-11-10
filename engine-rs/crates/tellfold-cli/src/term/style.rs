//! Text styles and their ANSI codes (port of the parts of `rich/style.py` and `rich/color.py` the CLI uses:
//! the attributes and the 16 standard colours plus "default").

/// Attribute bits in rich's order (`Style._style_map`): bold, dim, italic, underline, blink, blink2, reverse,
/// conceal, strike, underline2, frame, encircle, overline.
const SGR: [&str; 13] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "21", "51", "52", "53"];

fn attribute_bit(word: &str) -> Option<u16> {
    let bit = match word {
        "bold" | "b" => 0,
        "dim" | "d" => 1,
        "italic" | "i" => 2,
        "underline" | "u" => 3,
        "blink" => 4,
        "blink2" => 5,
        "reverse" | "r" => 6,
        "conceal" | "c" => 7,
        "strike" | "s" => 8,
        "underline2" | "uu" => 9,
        "frame" => 10,
        "encircle" => 11,
        "overline" | "o" => 12,
        _ => return None,
    };
    Some(1 << bit)
}

/// A terminal colour: one of the 16 standard colours or the terminal default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// The terminal's default colour (SGR 39 / 49).
    Default,
    /// Standard colour 0 to 15 (black, red, green, yellow, blue, magenta, cyan, white, then the bright ones).
    Standard(u8),
}

impl Color {
    /// Parses a colour name the way `Color.parse` reads the standard names.
    pub fn parse(name: &str) -> Option<Color> {
        const NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
        let name = name.trim().to_lowercase();
        if name == "default" {
            return Some(Color::Default);
        }
        let (base, offset) = match name.strip_prefix("bright_") {
            Some(base) => (base, 8),
            None => (name.as_str(), 0),
        };
        NAMES.iter().position(|&n| n == base).map(|i| Color::Standard(i as u8 + offset))
    }

    /// Port of `Color.get_ansi_codes`.
    fn ansi(self, foreground: bool) -> String {
        match (self, foreground) {
            (Color::Default, true) => "39".into(),
            (Color::Default, false) => "49".into(),
            (Color::Standard(n), true) => (if n < 8 { 30 + n as u32 } else { 82 + n as u32 }).to_string(),
            (Color::Standard(n), false) => (if n < 8 { 40 + n as u32 } else { 92 + n as u32 }).to_string(),
        }
    }
}

/// A rich style: attributes that are set on or off, and optional foreground and background colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    attributes: u16,
    set_attributes: u16,
    color: Option<Color>,
    bgcolor: Option<Color>,
}

impl Style {
    /// The null style: renders text unchanged.
    pub fn null() -> Style {
        Style::default()
    }

    /// Whether this is the null style (rich's `not style`).
    pub fn is_null(&self) -> bool {
        self.set_attributes == 0 && self.color.is_none() && self.bgcolor.is_none()
    }

    /// Parses a style definition such as `"bold red on white"` or `"not dim"` (port of `Style.parse`, behind
    /// `Console.get_style` with a null default): a word rich cannot parse gives the null style. The theme names
    /// the CLI's renderables use (`table.header`, `table.title`, ...) resolve as rich's default theme does.
    pub fn parse(definition: &str) -> Style {
        match definition.trim() {
            "table.header" => return Style::parse("bold"),
            "table.title" | "table.caption" => return Style::parse("italic"),
            "table.footer" => return Style::parse("bold"),
            _ => {}
        }
        Style::try_parse(definition).unwrap_or_default()
    }

    fn try_parse(definition: &str) -> Option<Style> {
        if definition.trim() == "none" || definition.is_empty() {
            return Some(Style::null());
        }
        let mut style = Style::null();
        let mut words = definition.split_whitespace();
        while let Some(word) = words.next() {
            let word = word.to_lowercase();
            if word == "on" {
                style.bgcolor = Some(Color::parse(words.next()?)?);
            } else if word == "not" {
                let bit = attribute_bit(words.next()?)?;
                style.set_attributes |= bit;
                style.attributes &= !bit;
            } else if word == "link" {
                words.next()?;
            } else if let Some(bit) = attribute_bit(&word) {
                style.set_attributes |= bit;
                style.attributes |= bit;
            } else {
                style.color = Some(Color::parse(&word)?);
            }
        }
        Some(style)
    }

    /// A style with only a foreground colour.
    pub fn color(color: Color) -> Style {
        Style { color: Some(color), ..Style::null() }
    }

    /// `self + other`: `other` wins where it sets something (port of `Style._add`).
    pub fn combine(self, other: Style) -> Style {
        if other.is_null() {
            return self;
        }
        if self.is_null() {
            return other;
        }
        Style {
            attributes: (self.attributes & !other.set_attributes) | (other.attributes & other.set_attributes),
            set_attributes: self.set_attributes | other.set_attributes,
            color: other.color.or(self.color),
            bgcolor: other.bgcolor.or(self.bgcolor),
        }
    }

    /// The SGR parameters, e.g. `"1;36"` (port of `Style._make_ansi_codes`).
    pub fn sgr(&self) -> String {
        let attributes = self.attributes & self.set_attributes;
        let mut codes: Vec<String> =
            (0..13).filter(|bit| attributes & (1 << bit) != 0).map(|bit| SGR[bit].to_string()).collect();
        if let Some(color) = self.color {
            codes.push(color.ansi(true));
        }
        if let Some(color) = self.bgcolor {
            codes.push(color.ansi(false));
        }
        codes.join(";")
    }

    /// Wraps `text` in this style's escape codes (port of `Style.render` with a colour system).
    pub fn render(&self, text: &str) -> String {
        if text.is_empty() || self.is_null() {
            return text.to_string();
        }
        let sgr = self.sgr();
        if sgr.is_empty() {
            text.to_string()
        } else {
            format!("\x1b[{sgr}m{text}\x1b[0m")
        }
    }
}

impl std::ops::Add for Style {
    type Output = Style;

    fn add(self, other: Style) -> Style {
        self.combine(other)
    }
}
