//! Console width detection (port of `Console.size` in `rich/console.py`, and of typer's `TERMINAL_WIDTH`).

use std::io::IsTerminal;

/// `COLUMNS`-style variables count only when they are all ASCII digits (Python's `str.isdigit()` for ASCII).
fn digits_env(name: &str) -> Option<usize> {
    let value = std::env::var(name).ok()?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// `(columns, rows)` of the terminal on `fd` (`os.get_terminal_size`).
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn terminal_size(fd: i32) -> Option<(usize, usize)> {
    #[repr(C)]
    struct WinSize {
        ws_row: u16,
        ws_col: u16,
        ws_xpixel: u16,
        ws_ypixel: u16,
    }
    extern "C" {
        fn ioctl(fd: std::os::raw::c_int, request: std::os::raw::c_ulong, ...) -> std::os::raw::c_int;
    }
    #[cfg(target_os = "macos")]
    const TIOCGWINSZ: std::os::raw::c_ulong = 0x4008_7468;
    #[cfg(target_os = "linux")]
    const TIOCGWINSZ: std::os::raw::c_ulong = 0x5413;
    let mut size = WinSize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCGWINSZ writes one `struct winsize` to the pointer, which points at a live, writable WinSize.
    let ok = unsafe { ioctl(fd, TIOCGWINSZ, &mut size as *mut WinSize) } == 0;
    let _ = (size.ws_xpixel, size.ws_ypixel);
    ok.then_some((size.ws_col as usize, size.ws_row as usize))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn terminal_size(_fd: i32) -> Option<(usize, usize)> {
    None
}

/// Whether stdout is a terminal.
pub fn is_terminal() -> bool {
    std::io::stdout().is_terminal()
}

fn is_dumb_terminal() -> bool {
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    is_terminal() && (term == "dumb" || term == "unknown")
}

/// The width rich's `Console()` uses: `COLUMNS` when it is a number, else the size of the first of stdin,
/// stdout and stderr that is a terminal, else 80 (a dumb terminal is always 80 unless `COLUMNS` and `LINES`
/// are both set).
pub fn console_width() -> usize {
    let columns = digits_env("COLUMNS");
    if let (Some(columns), Some(_)) = (columns, digits_env("LINES")) {
        return columns;
    }
    if is_dumb_terminal() {
        return 80;
    }
    if let Some(columns) = columns {
        return columns;
    }
    let width = (0..3).find_map(terminal_size).map_or(0, |(columns, _)| columns);
    if width == 0 {
        80
    } else {
        width
    }
}

/// The height rich's `Console()` uses: `LINES` when it is a number, else the rows of the first of stdin, stdout
/// and stderr that is a terminal, else 25 (a dumb terminal is always 25 unless `COLUMNS` and `LINES` are both
/// set).
pub fn console_height() -> usize {
    let lines = digits_env("LINES");
    if let (Some(_), Some(lines)) = (digits_env("COLUMNS"), lines) {
        return lines;
    }
    if is_dumb_terminal() {
        return 25;
    }
    if let Some(lines) = lines {
        return lines;
    }
    let height = (0..3).find_map(terminal_size).map_or(0, |(_, rows)| rows);
    if height == 0 {
        25
    } else {
        height
    }
}

/// The width typer's help and error console uses: `TERMINAL_WIDTH` when set to a number, else
/// [`console_width`].
pub fn help_width() -> usize {
    let explicit = std::env::var("TERMINAL_WIDTH").ok().and_then(|v| v.trim().parse::<usize>().ok());
    match explicit {
        Some(width) if !is_dumb_terminal() => width,
        _ => console_width(),
    }
}
