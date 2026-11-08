//! The log around a buffer that may hold secrets: while one is shown, or was during what the
//! app is handling, only warnings and errors are written, from every crate (ADR 0078). The app
//! says when, so nothing that logs has to ask.
//!
//! Must not depend on: the editor, key presses or the window. It holds one flag, which the app
//! sets.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use log::{Level, Log, Metadata, Record};

/// The app's handle on the flag the logger reads.
#[derive(Clone, Default)]
pub struct LogGate {
    quiet: Arc<AtomicBool>,
}

/// `env_logger`, behind the flag.
struct Gated {
    inner: env_logger::Logger,
    quiet: Arc<AtomicBool>,
}

impl Log for Gated {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        (metadata.level() <= Level::Warn || !self.quiet.load(Ordering::Relaxed))
            && self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

impl LogGate {
    /// Installs the process's logger, `env_logger` as `RUST_LOG` sets it, behind a gate whose
    /// handle this returns. The logger lives as long as the process, as `log` asks.
    pub fn install() -> Self {
        let gate = Self::default();
        let inner = env_logger::Builder::from_default_env().build();
        let level = inner.filter();
        let gated = Gated {
            inner,
            quiet: Arc::clone(&gate.quiet),
        };
        if log::set_logger(Box::leak(Box::new(gated))).is_ok() {
            log::set_max_level(level);
        }
        gate
    }

    /// Starts what the app handles next, with a buffer that may hold secrets shown or not.
    pub fn begin(&self, shown: bool) {
        self.quiet.store(shown, Ordering::Relaxed);
    }

    /// Notes whether one is shown now. Once one has been, the gate stays shut until the next
    /// [`LogGate::begin`].
    pub fn note(&self, shown: bool) {
        if shown {
            self.quiet.store(true, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    fn quiet(&self) -> bool {
        self.quiet.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::{Gated, LogGate};
    use log::{Level, Log, Metadata};
    use std::sync::Arc;

    fn gated(gate: &LogGate) -> Gated {
        let inner = env_logger::Builder::new()
            .filter_level(log::LevelFilter::Trace)
            .build();
        Gated {
            inner,
            quiet: Arc::clone(&gate.quiet),
        }
    }

    fn passes(logger: &Gated, level: Level) -> bool {
        logger.enabled(
            &Metadata::builder()
                .level(level)
                .target("oso_render")
                .build(),
        )
    }

    #[test]
    fn around_a_secret_only_warnings_and_errors_pass_from_any_crate() {
        let gate = LogGate::default();
        let logger = gated(&gate);
        let all = [
            Level::Error,
            Level::Warn,
            Level::Info,
            Level::Debug,
            Level::Trace,
        ];
        assert!(all.iter().all(|&level| passes(&logger, level)));
        gate.begin(true);
        let passing: Vec<_> = all
            .into_iter()
            .filter(|&level| passes(&logger, level))
            .collect();
        assert_eq!(passing, [Level::Error, Level::Warn]);
    }

    #[test]
    fn once_one_was_shown_the_gate_stays_shut_until_the_next_begin() {
        let gate = LogGate::default();
        gate.begin(false);
        gate.note(true);
        gate.note(false);
        assert!(gate.quiet(), "left in the same batch");
        gate.begin(false);
        assert!(!gate.quiet());
    }
}
