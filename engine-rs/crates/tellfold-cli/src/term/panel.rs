//! Panels (port of `rich/panel.py`; no subtitle, height or panel style).

use super::render::{
    measure, measure_all, print, render, render_lines, AlignMethod, Measurement, Options, Renderable, Seg,
};
use super::style::Style;
use super::table::{BoxChars, ROUNDED};
use super::text::Text;

/// A box around a renderable with an optional title in the top edge (port of `rich.panel.Panel`).
#[derive(Clone, Debug)]
pub struct Panel {
    renderable: Renderable,
    title: Option<Text>,
    title_align: AlignMethod,
    expand: bool,
    border_style: Style,
    box_chars: BoxChars,
    padding: (usize, usize, usize, usize),
    width: Option<usize>,
}

impl Panel {
    /// A panel with rich's defaults (`box.ROUNDED`, expanded to the full width, padding `(0, 1)`), except that
    /// the title is left-aligned, as every panel of the CLI has it (rich's default is centred).
    pub fn new(body: Text) -> Self {
        Panel::of(Renderable::Text(body))
    }

    pub(crate) fn of(renderable: Renderable) -> Self {
        Panel {
            renderable,
            title: None,
            title_align: AlignMethod::Left,
            expand: true,
            border_style: Style::null(),
            box_chars: ROUNDED,
            padding: (0, 1, 0, 1),
            width: None,
        }
    }

    /// The title, as console markup (like a `str` title in rich); `""` means none.
    pub fn title(mut self, title: &str) -> Self {
        self.title = (!title.is_empty()).then(|| Text::from_markup(title));
        self
    }

    pub fn title_align(mut self, align: AlignMethod) -> Self {
        self.title_align = align;
        self
    }

    /// The border (and title) style, e.g. `"cyan"`.
    pub fn border_style(mut self, style: &str) -> Self {
        self.border_style = Style::parse(style);
        self
    }

    /// `false` sizes the panel to its contents (`Panel.fit`).
    pub fn expand(mut self, expand: bool) -> Self {
        self.expand = expand;
        self
    }

    pub fn box_chars(mut self, box_chars: BoxChars) -> Self {
        self.box_chars = box_chars;
        self
    }

    /// Padding inside the border as `(top, right, bottom, left)`.
    pub fn padding(mut self, padding: (usize, usize, usize, usize)) -> Self {
        self.padding = padding;
        self
    }

    /// A fixed width.
    pub fn width(mut self, width: usize) -> Self {
        self.width = Some(width);
        self
    }

    /// What `Console.print(panel)` writes at `width` cells, with ANSI styles when `color`.
    pub fn render(&self, width: usize, color: bool) -> String {
        print(&Renderable::Panel(Box::new(self.clone())), width, color)
    }

    /// Port of `Panel._title`.
    fn title_text(&self) -> Option<Text> {
        let mut title = self.title.clone()?;
        title.end = "";
        let chars: Vec<char> = title.chars.iter().map(|&c| if c == '\n' { ' ' } else { c }).collect();
        title.chars = chars;
        title.no_wrap = Some(true);
        title.expand_tabs(8);
        title.pad(1);
        Some(title)
    }

    fn padded(&self) -> Renderable {
        let (t, r, b, l) = self.padding;
        if t + r + b + l > 0 {
            Renderable::padding(self.renderable.clone(), self.padding)
        } else {
            self.renderable.clone()
        }
    }

    /// Port of `Panel.__rich_measure__`.
    pub(crate) fn measure(&self, options: Options) -> Measurement {
        let (_, right, _, left) = self.padding;
        let padding = (left + right) as isize;
        let width = match self.width {
            Some(width) => width as isize,
            None => {
                let mut renderables = vec![self.renderable.clone()];
                if let Some(title) = self.title_text() {
                    renderables.push(Renderable::Text(title));
                }
                measure_all(&renderables, options.width(options.max_width as isize - padding - 2)).1 + padding + 2
            }
        };
        Measurement(width, width)
    }

    /// Port of `Panel.__rich_console__`.
    pub(crate) fn render_segments(&self, options: Options) -> Vec<Seg> {
        let renderable = self.padded();
        let border = self.border_style;
        let b = self.box_chars.rows_of();
        let max_width = options.max_width as isize;
        let width = self.width.map_or(max_width, |w| (w as isize).min(max_width));
        let mut title = self.title_text();
        if let Some(title) = title.as_mut() {
            title.stylize_before(border);
        }
        let mut child_width = if self.expand { width - 2 } else { measure(&renderable, options.width(width - 2)).1 };
        if let Some(title) = &title {
            child_width = (max_width - 2).min(child_width.max(title.cell_len() as isize + 2));
        }
        let width = child_width + 2;
        let child_options = options.width(child_width);
        let lines = render_lines(&renderable, child_options, Style::null(), true);
        let inner = (width - 2).max(0) as usize;
        let mut out = Vec::new();
        match title {
            Some(title) if width > 4 => {
                let title = align_text(&title, width - 4, self.title_align, b[0][1], border);
                out.push(Seg::new([b[0][0], b[0][1]].iter().collect(), border));
                out.extend(render(&Renderable::Text(title), child_options.width(width - 4)));
                out.push(Seg::new([b[0][1], b[0][3]].iter().collect(), border));
            }
            _ => out.push(Seg::new(edge(b[0], inner), border)),
        }
        out.push(Seg::line());
        for line in lines {
            out.push(Seg::new(b[3][0].to_string(), border));
            out.extend(line);
            out.push(Seg::new(b[3][3].to_string(), border));
            out.push(Seg::line());
        }
        out.push(Seg::new(edge(b[7], inner), border));
        out.push(Seg::line());
        out
    }
}

fn edge(row: [char; 4], width: usize) -> String {
    let mut out = String::new();
    out.push(row[0]);
    out.extend(std::iter::repeat(row[1]).take(width));
    out.push(row[3]);
    out
}

/// The title cut to `width` and filled out with the border character (the `align_text` helper in
/// `Panel.__rich_console__`).
fn align_text(text: &Text, width: isize, align: AlignMethod, character: char, style: Style) -> Text {
    let mut text = text.clone();
    text.truncate_to(width);
    let excess = width - text.cell_len() as isize;
    let base = text.style;
    text.stylize(base);
    if excess <= 0 {
        return text;
    }
    let fill = |count: isize| character.to_string().repeat(count as usize);
    let mut out = Text::new().with_no_wrap(true);
    out.end = "";
    match align {
        AlignMethod::Left => {
            out.append_text(&text);
            out.push_style(&fill(excess), style);
        }
        AlignMethod::Center => {
            let left = excess / 2;
            out.push_style(&fill(left), style);
            out.append_text(&text);
            out.push_style(&fill(excess - left), style);
        }
        AlignMethod::Right => {
            out.push_style(&fill(excess), style);
            out.append_text(&text);
        }
    }
    out
}
