//! The console the commands print to: rich's `Console.print`, `Live` and `Status` for the subset `tellfold/cli.py`
//! uses. Off a terminal nothing animates and no color is written, like rich: a live view prints its last frame once
//! when it stops, and a status line prints nothing.

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::term::{self, Panel, Table, Text};

const DOTS: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const DOTS_INTERVAL_S: f64 = 0.08;

/// rich's `Spinner("dots", text, style=...)`: a frame that changes every 80 ms, a space, then the text.
#[derive(Debug, Clone)]
pub struct Spinner {
    text: Text,
    style: String,
    started: Instant,
}

impl Spinner {
    pub fn new(text: Text, style: &str) -> Self {
        Spinner { text, style: style.into(), started: Instant::now() }
    }

    fn frame(&self) -> Text {
        let n = (self.started.elapsed().as_secs_f64() / DOTS_INTERVAL_S) as usize;
        let mut out = Text::new();
        out.push(DOTS[n % DOTS.len()], &self.style);
        out.push(" ", "");
        out.append_text(&self.text);
        out
    }
}

/// One line (or table) of a rich `Group`.
#[derive(Debug, Clone)]
pub enum Block {
    Text(Text),
    Spinner(Spinner),
    Table(Box<Table>),
}

#[derive(Debug, Clone, Copy)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone)]
pub struct Console {
    stream: Stream,
    pub width: usize,
    pub terminal: bool,
}

impl Console {
    pub fn new(stream: Stream) -> Self {
        let terminal = match stream {
            Stream::Stdout => term::is_terminal(),
            Stream::Stderr => std::io::IsTerminal::is_terminal(&std::io::stderr()),
        };
        Console { stream, width: term::console_width(), terminal }
    }

    pub fn stdout() -> Self {
        Console::new(Stream::Stdout)
    }

    pub fn stderr() -> Self {
        Console::new(Stream::Stderr)
    }

    pub fn write(&self, text: &str) {
        match self.stream {
            Stream::Stdout => {
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(text.as_bytes());
                let _ = out.flush();
            }
            Stream::Stderr => {
                let mut err = std::io::stderr().lock();
                let _ = err.write_all(text.as_bytes());
                let _ = err.flush();
            }
        }
    }

    /// `console.print(markup)`: wrapped to the width.
    pub fn print(&self, markup: &str) {
        self.print_text(&Text::from_markup(markup));
    }

    /// `console.print(markup, soft_wrap=True)`: one line per line of text, however long.
    pub fn print_soft(&self, markup: &str) {
        self.write(&term::render_text_soft(&Text::from_markup(markup), self.terminal));
    }

    pub fn print_text(&self, text: &Text) {
        self.write(&term::render_text(text, self.width, self.terminal));
    }

    /// `console.print()` / `console.line()`.
    pub fn line(&self) {
        self.write("\n");
    }

    /// `Panel(body, title=title, title_align="left", border_style="cyan")`.
    pub fn panel(&self, body: Text, title: &str) {
        let panel = Panel::new(body).title(title).border_style("cyan");
        self.write(&panel.render(self.width, self.terminal));
    }

    pub fn table(&self, table: &Table) {
        self.write(&table.render(self.width, self.terminal));
    }

    fn render_blocks(&self, blocks: &[Block]) -> String {
        blocks
            .iter()
            .map(|b| match b {
                Block::Text(t) => term::render_text(t, self.width, self.terminal),
                Block::Spinner(s) => term::render_text(&s.frame(), self.width, self.terminal),
                Block::Table(t) => t.render(self.width, self.terminal),
            })
            .collect()
    }

    /// `console.input(prompt)`: the line typed, without its newline; `None` at the end of input.
    pub fn input(&self, prompt_markup: &str) -> Option<String> {
        self.write(term::render_text_soft(&Text::from_markup(prompt_markup), self.terminal).trim_end_matches('\n'));
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                Some(line.strip_suffix('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).unwrap_or(&line).to_string())
            }
        }
    }

    /// `console.status(text)`: a spinner on a terminal until the guard drops, then nothing is left; nothing elsewhere.
    pub fn status(&self, text: &str) -> Live {
        let spinner = Spinner::new(Text::from_markup(text), "green");
        Live::start(self.clone(), Arc::new(move || vec![Block::Spinner(spinner.clone())]), 12.5, true)
    }

    /// `Live(get_renderable=..., refresh_per_second=8)`.
    pub fn live(&self, render: Arc<dyn Fn() -> Vec<Block> + Send + Sync>) -> Live {
        Live::start(self.clone(), render, 8.0, false)
    }
}

type Render = Arc<dyn Fn() -> Vec<Block> + Send + Sync>;

struct LiveState {
    /// lines of the frame on screen
    height: usize,
}

/// A view redrawn in place on a terminal (rich's `Live`). Stop it with [`Live::stop`]; dropping it stops it too.
pub struct Live {
    console: Console,
    render: Render,
    transient: bool,
    state: Arc<Mutex<LiveState>>,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    stopped: bool,
}

impl Live {
    fn start(console: Console, render: Render, per_second: f64, transient: bool) -> Live {
        let state = Arc::new(Mutex::new(LiveState { height: 0 }));
        let running = Arc::new(AtomicBool::new(true));
        let mut thread = None;
        if console.terminal {
            console.write("\x1b[?25l");
            draw(&console, &render, &state);
            let (console, render, state, running) = (console.clone(), render.clone(), state.clone(), running.clone());
            thread = Some(std::thread::spawn(move || {
                let tick = Duration::from_secs_f64(1.0 / per_second);
                loop {
                    std::thread::sleep(tick);
                    if !running.load(Ordering::SeqCst) {
                        break;
                    }
                    draw(&console, &render, &state);
                }
            }));
        }
        Live { console, render, transient, state, running, thread, stopped: false }
    }

    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.running.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if !self.console.terminal {
            if !self.transient {
                // the last frame, without a newline after its last line (rich only adds one on a terminal)
                let frame = self.console.render_blocks(&(self.render)());
                self.console.write(frame.strip_suffix('\n').unwrap_or(&frame));
            }
            return;
        }
        draw(&self.console, &self.render, &self.state);
        self.console.write("\n\x1b[?25h");
        if self.transient {
            let height = self.state.lock().expect("not poisoned").height;
            // rich's restore_cursor: up to the first line of the frame, erasing every line on the way
            let mut out = String::from("\r");
            for _ in 0..height {
                out.push_str("\x1b[1A\x1b[2K");
            }
            self.console.write(&out);
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Redraw the frame over the previous one (rich's `LiveRender.position_cursor`, then the frame without its last
/// newline), cut to the terminal's height.
fn draw(console: &Console, render: &Render, state: &Arc<Mutex<LiveState>>) {
    let frame = console.render_blocks(&render());
    let rows = term::console_height().max(1);
    let mut lines: Vec<&str> = frame.strip_suffix('\n').unwrap_or(&frame).split('\n').collect();
    if lines.len() > rows {
        lines.truncate(rows);
        if let Some(last) = lines.last_mut() {
            *last = "...";
        }
    }
    let mut state = state.lock().expect("not poisoned");
    let mut out = String::new();
    if state.height > 0 {
        out.push_str("\r\x1b[2K");
        for _ in 1..state.height {
            out.push_str("\x1b[1A\x1b[2K");
        }
    }
    out.push_str(&lines.join("\n"));
    state.height = lines.len();
    console.write(&out);
}
