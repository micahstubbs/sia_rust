//! Centralized logging configuration for SIA. Port of `sia/logging_setup.py`.
//!
//! Provides a minimal [`log::Log`] implementation that writes to **stderr** using
//! the same format/datefmt as the Python reference:
//! `"%(asctime)s [%(levelname)s] %(message)s"` with datefmt `"%Y-%m-%d %H:%M:%S"`.
//!
//! Logs go to stderr so the parity-golden stdout/JSON outputs are unaffected.
//!
//! Level resolution precedence mirrors Python: explicit level (CLI flag) →
//! `$SIA_LOG_LEVEL` → INFO. A level may be a name ("DEBUG"/"INFO"/"WARNING"/
//! "ERROR"/"CRITICAL", case-insensitive) or a numeric string (Python's
//! DEBUG=10/INFO=20/WARNING=30/ERROR=40 mapping; nearest level).

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use log::{Level, LevelFilter, Log, Metadata, Record};

const ENV_VAR: &str = "SIA_LOG_LEVEL";
const DATE_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// Tracks whether the boxed logger has been installed (idempotent guard).
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// The single logger instance, installed once via `log::set_logger`.
static LOGGER: OnceLock<StderrLogger> = OnceLock::new();

/// Minimal `log::Log` implementation writing to stderr in Python's format.
struct StderrLogger;

/// Map a Rust `log::Level` to Python's `levelname` spelling.
///
/// Python uses WARNING/INFO/DEBUG/ERROR/CRITICAL; Rust's `Warn`/`Trace` differ.
fn python_level_name(level: Level) -> &'static str {
    match level {
        Level::Error => "ERROR",
        Level::Warn => "WARNING",
        Level::Info => "INFO",
        Level::Debug => "DEBUG",
        Level::Trace => "TRACE",
    }
}

impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let asctime = chrono::Local::now().format(DATE_FORMAT);
        let levelname = python_level_name(record.level());
        // Write to stderr — never stdout — so parity-golden output is unchanged.
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{asctime} [{levelname}] {}", record.args());
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// Parse a level string into a `log::LevelFilter`.
///
/// Accepts a level name (case-insensitive) or a numeric string using Python's
/// `logging` numbering (DEBUG=10, INFO=20, WARNING=30, ERROR=40, CRITICAL=50).
/// Numeric values snap to the nearest enclosing level. Unknown values → `Info`.
pub fn parse_level(level: &str) -> LevelFilter {
    let name = level.trim().to_ascii_uppercase();
    if name.is_empty() {
        return LevelFilter::Info;
    }
    if name.chars().all(|c| c.is_ascii_digit()) {
        let n: i64 = name.parse().unwrap_or(20);
        // Python numeric mapping (DEBUG=10/INFO=20/WARNING=30/ERROR=40/CRITICAL=50).
        // `log` has no CRITICAL, so ERROR and above both map to Error. Below DEBUG
        // (but positive) captures everything via Trace; <=0 disables logging.
        return if n >= 40 {
            LevelFilter::Error
        } else if n >= 30 {
            LevelFilter::Warn
        } else if n >= 20 {
            LevelFilter::Info
        } else if n >= 10 {
            LevelFilter::Debug
        } else if n > 0 {
            LevelFilter::Trace
        } else {
            LevelFilter::Off
        };
    }
    match name.as_str() {
        "DEBUG" => LevelFilter::Debug,
        "INFO" => LevelFilter::Info,
        "WARNING" | "WARN" => LevelFilter::Warn,
        "ERROR" => LevelFilter::Error,
        // Python's CRITICAL is more severe than ERROR; `log` has no separate
        // level, so map it to the most severe filter (Error).
        "CRITICAL" => LevelFilter::Error,
        _ => LevelFilter::Info,
    }
}

/// Resolve the effective level from an explicit value, then `$SIA_LOG_LEVEL`,
/// then INFO. Mirrors Python's `_resolve_level`.
fn resolve_level(cli_level: Option<&str>) -> LevelFilter {
    if let Some(l) = cli_level {
        let trimmed = l.trim();
        if !trimmed.is_empty() {
            return parse_level(trimmed);
        }
    }
    match std::env::var(ENV_VAR) {
        Ok(v) if !v.trim().is_empty() => parse_level(&v),
        _ => LevelFilter::Info,
    }
}

/// Initialize logging.
///
/// On the first call this installs the stderr logger and sets the max level. The
/// level is taken from `cli_level` if given, else `$SIA_LOG_LEVEL`, else INFO. A
/// later call with an explicit `cli_level` updates `log::set_max_level`; otherwise
/// subsequent calls are no-ops. Never panics.
pub fn init(cli_level: Option<&str>) {
    let resolved = resolve_level(cli_level);

    if INSTALLED.swap(true, Ordering::SeqCst) {
        // Already installed: a later explicit level updates the max level.
        if cli_level.map(|c| !c.trim().is_empty()).unwrap_or(false) {
            log::set_max_level(resolved);
        }
        return;
    }

    let logger = LOGGER.get_or_init(|| StderrLogger);
    // `set_logger` only fails if a logger was already set by someone else; either
    // way we apply the resolved max level and never panic.
    let _ = log::set_logger(logger);
    log::set_max_level(resolved);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_level_names_case_insensitive() {
        assert_eq!(parse_level("DEBUG"), LevelFilter::Debug);
        assert_eq!(parse_level("debug"), LevelFilter::Debug);
        assert_eq!(parse_level("Info"), LevelFilter::Info);
        assert_eq!(parse_level("warning"), LevelFilter::Warn);
        assert_eq!(parse_level("WARNING"), LevelFilter::Warn);
        assert_eq!(parse_level("error"), LevelFilter::Error);
        assert_eq!(parse_level("CRITICAL"), LevelFilter::Error);
    }

    #[test]
    fn test_parse_level_numeric() {
        assert_eq!(parse_level("10"), LevelFilter::Debug);
        assert_eq!(parse_level("20"), LevelFilter::Info);
        assert_eq!(parse_level("30"), LevelFilter::Warn);
        assert_eq!(parse_level("40"), LevelFilter::Error);
        assert_eq!(parse_level("50"), LevelFilter::Error);
    }

    #[test]
    fn test_parse_level_unknown_is_info() {
        assert_eq!(parse_level("nonsense"), LevelFilter::Info);
        assert_eq!(parse_level(""), LevelFilter::Info);
        assert_eq!(parse_level("   "), LevelFilter::Info);
    }

    #[test]
    fn test_init_is_idempotent_and_never_panics() {
        // Multiple calls, including with explicit levels, must not panic.
        init(None);
        init(Some("DEBUG"));
        init(Some(""));
        init(Some("WARNING"));
        // A log call after init must not panic either.
        log::warn!("logging smoke test (expected on stderr)");
        log::info!("logging smoke info");
    }
}
