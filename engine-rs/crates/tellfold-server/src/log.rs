//! Server logging on stderr in the Python server's format (`%(asctime)s %(levelname)s %(name)s: %(message)s`),
//! filtered by `--log-level`.

use std::sync::atomic::{AtomicU8, Ordering};

pub const DEBUG: u8 = 10;
pub const INFO: u8 = 20;
pub const WARNING: u8 = 30;
pub const ERROR: u8 = 40;

static LEVEL: AtomicU8 = AtomicU8::new(WARNING);

/// `debug`, `info`, `warning` or `error` (any case); None for anything else.
pub fn parse_level(name: &str) -> Option<u8> {
    match name.to_lowercase().as_str() {
        "debug" => Some(DEBUG),
        "info" => Some(INFO),
        "warning" => Some(WARNING),
        "error" => Some(ERROR),
        _ => None,
    }
}

pub fn set_level(level: u8) {
    LEVEL.store(level, Ordering::Relaxed);
}

pub fn enabled(level: u8) -> bool {
    level >= LEVEL.load(Ordering::Relaxed)
}

pub fn log(level: u8, name: &str, message: &str) {
    if !enabled(level) {
        return;
    }
    let label = match level {
        DEBUG => "DEBUG",
        INFO => "INFO",
        WARNING => "WARNING",
        _ => "ERROR",
    };
    let now = chrono::Local::now();
    eprintln!("{},{:03} {label} {name}: {message}", now.format("%Y-%m-%d %H:%M:%S"), now.timestamp_subsec_millis());
}

pub fn debug(name: &str, message: &str) {
    log(DEBUG, name, message);
}

pub fn info(name: &str, message: &str) {
    log(INFO, name, message);
}

pub fn warning(name: &str, message: &str) {
    log(WARNING, name, message);
}
