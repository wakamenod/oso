//! A clipboard in memory, for tests, `just e2e` and `just perf`, so that they never
//! touch the person's (ADR 0074).
//!
//! Must not depend on: the system's clipboard.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use super::{Backend, Found};

#[derive(Default)]
struct Held {
    text: Option<(String, bool)>,
    count: i64,
}

/// A clipboard in memory that stands in for the system's. Clones share it, so a test or
/// the dev socket can play another app that copies text.
#[derive(Clone, Default)]
pub struct MemoryClipboard(Arc<Mutex<Held>>);

impl fmt::Debug for MemoryClipboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MemoryClipboard") // Never the text: it could reach a log.
    }
}

impl MemoryClipboard {
    fn with<R>(&self, f: impl FnOnce(&mut Held) -> R) -> R {
        // Nothing can leave it half changed, so a panic elsewhere does not matter.
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// What it holds: the text, and whether it is marked concealed.
    pub fn get(&self) -> Option<(String, bool)> {
        self.with(|held| held.text.clone())
    }

    /// Puts `text` there, as another app would, marked concealed if `concealed`.
    pub fn set(&self, text: String, concealed: bool) {
        self.with(|held| {
            held.text = Some((text, concealed));
            held.count += 1;
        });
    }
}

impl Backend for MemoryClipboard {
    fn change_count(&mut self) -> Option<i64> {
        Some(self.with(|held| held.count))
    }

    fn write(&mut self, text: &str, concealed: bool) -> Option<i64> {
        self.set(text.to_owned(), concealed);
        self.change_count()
    }

    fn read(&mut self, max: usize) -> Found {
        self.with(|held| match &held.text {
            None => Found::Nothing,
            Some((text, _)) if text.len() > max => Found::TooLarge,
            Some((text, concealed)) => Found::Text {
                text: text.clone(),
                concealed: *concealed,
            },
        })
    }
}
