//! Per-operation memory lines in the daemon log.
//!
//! # Why this exists
//!
//! The daemon's watchdog sees one number — the process RSS — and acts on it.
//! When that number reads 25–37 GB nobody can say WHICH work put it there:
//! measured 2026-10-19, a retired daemon with zero rust-analyzer projects
//! loaded sat at 25 GB, and the log offered only "unloaded 0 project(s)".
//! The work that runs in a daemon is a handful of kinds — tool calls, the
//! background sync, vector-store maintenance — so wrapping each in a
//! [`MemSpan`] turns the log into an attribution: RSS before and after every
//! operation, plus whether the operation set a new process peak.
//!
//! # Reading the lines
//!
//! `mem: <what> took 10.6s, RSS 127 -> 1828 MB (+1701), peak 2381 MB (new)`.
//!
//! - `before -> after` is what the operation LEFT behind: freed-but-retained
//!   heap counts, because that is exactly what the watchdog counts too.
//! - `peak ... (new)` means the process high-water mark (`VmHWM`) rose while
//!   the operation ran. Operations overlap in a daemon serving several
//!   sessions, so "new peak" says the peak happened during this window, not
//!   that this operation alone caused it — overlapping lines in the log tell
//!   which.
//!
//! Linux-only readings; elsewhere every field is `None` and the line says
//! "unknown" instead of inventing zeros.

use std::time::Instant;

/// The `tracing` target of every `mem:` line.
///
/// Its own target rather than this module's path: the daemon's default filter
/// keeps `rmc_engine` at `warn` (rust-analyzer-adjacent crates are chatty), and
/// under it these lines vanished silently — the first live probe of this
/// module logged nothing at all. The binary's default filter names this target
/// explicitly; a user-supplied `RUST_LOG` has to name it too.
pub const LOG_TARGET: &str = "rmc_mem";

/// `VmRSS` and `VmHWM` of this process, in KiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemReading {
    pub rss_kib: Option<u64>,
    pub peak_kib: Option<u64>,
}

impl MemReading {
    pub fn now() -> Self {
        #[cfg(target_os = "linux")]
        {
            match std::fs::read_to_string("/proc/self/status") {
                Ok(status) => Self::parse(&status),
                Err(_) => Self::unknown(),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::unknown()
        }
    }

    fn unknown() -> Self {
        Self {
            rss_kib: None,
            peak_kib: None,
        }
    }

    pub(crate) fn parse(status: &str) -> Self {
        let field = |name: &str| {
            status.lines().find_map(|line| {
                line.strip_prefix(name)?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
        };
        Self {
            rss_kib: field("VmRSS:"),
            peak_kib: field("VmHWM:"),
        }
    }
}

fn mb(kib: Option<u64>) -> String {
    kib.map_or_else(|| "unknown".to_string(), |k| (k / 1024).to_string())
}

/// An operation being measured. Logs one `mem:` line when finished.
///
/// Finished explicitly with [`MemSpan::finish`] rather than on `Drop`: a span
/// dropped by an early `?` return would otherwise log a line indistinguishable
/// from a completed operation. The early-return case still logs, from `Drop`,
/// but says so.
pub struct MemSpan {
    what: String,
    started: Instant,
    start: MemReading,
    finished: bool,
}

impl MemSpan {
    pub fn start(what: impl Into<String>) -> Self {
        Self {
            what: what.into(),
            started: Instant::now(),
            start: MemReading::now(),
            finished: false,
        }
    }

    pub fn finish(mut self) {
        self.log("");
        self.finished = true;
    }

    fn log(&self, suffix: &str) {
        let end = MemReading::now();
        let delta = match (self.start.rss_kib, end.rss_kib) {
            (Some(a), Some(b)) => format!(" ({:+})", (b as i64 - a as i64) / 1024),
            _ => String::new(),
        };
        let new_peak = match (self.start.peak_kib, end.peak_kib) {
            (Some(a), Some(b)) if b > a => " (new)",
            _ => "",
        };
        tracing::info!(
            target: LOG_TARGET,
            "mem: {} took {:.1}s, RSS {} -> {} MB{}, peak {} MB{}{}",
            self.what,
            self.started.elapsed().as_secs_f64(),
            mb(self.start.rss_kib),
            mb(end.rss_kib),
            delta,
            mb(end.peak_kib),
            new_peak,
            suffix,
        );
    }
}

impl Drop for MemSpan {
    fn drop(&mut self) {
        if !self.finished {
            self.log(" [ended early: error or cancellation]");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rss_and_peak() {
        let status = "Name:\tx\nVmHWM:\t  2048000 kB\nVmRSS:\t  1024000 kB\n";
        assert_eq!(
            MemReading::parse(status),
            MemReading {
                rss_kib: Some(1_024_000),
                peak_kib: Some(2_048_000)
            }
        );
    }

    /// Absence must stay absence: a zero would read as "used no memory".
    #[test]
    fn missing_fields_are_none_not_zero() {
        assert_eq!(MemReading::parse("Name:\tx\n"), MemReading::unknown());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn readable_on_linux() {
        let r = MemReading::now();
        assert!(r.rss_kib.is_some_and(|k| k > 0));
        assert!(
            r.peak_kib >= r.rss_kib,
            "high-water mark below current RSS: {r:?}"
        );
    }
}
