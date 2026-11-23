//! A clipboard in memory, for tests, `just e2e` and `just perf`, so that they never
//! touch the person's (ADR 0074).
//!
//! Must not depend on: the system's clipboard.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use super::{Backend, Found, Image, ImageContents, ImageKind};

/// What it holds. Like the system's, it holds text or an image, and a copy of one replaces
/// the other. The types are those another app marked its text with, which a copy drops
/// (ADR 0175).
#[derive(Default)]
struct Held {
    text: Option<(String, bool)>,
    image: Option<(Image, bool)>,
    types: Vec<String>,
    count: i64,
}

/// A clipboard in memory that stands in for the system's. Clones share it, so a test or
/// the dev socket can play another app that copies text.
#[derive(Clone, Default)]
pub struct MemoryClipboard(Arc<Mutex<Held>>);

impl fmt::Debug for MemoryClipboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MemoryClipboard") // Never the text or image: they could reach a log.
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
        self.set_marked(text, concealed, Vec::new());
    }

    /// Puts `text` there, as [`Self::set`] does, with the types `types` beside it, as an app
    /// marks text it pastes itself (ADR 0175).
    pub fn set_marked(&self, text: String, concealed: bool, types: Vec<String>) {
        self.with(|held| {
            held.text = Some((text, concealed));
            held.image = None;
            held.types = types;
            held.count += 1;
        });
    }

    /// Puts an image of type `kind` there, as another app would, in place of any text, marked
    /// concealed if `concealed`.
    pub fn set_image(&self, kind: ImageKind, bytes: Vec<u8>, concealed: bool) {
        self.with(|held| {
            held.image = Some((Image { kind, bytes }, concealed));
            held.text = None;
            held.types.clear();
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

    fn read_image(&mut self, max: usize) -> ImageContents {
        self.with(|held| match &held.image {
            Some((_, true)) => ImageContents::Concealed,
            Some((image, false)) if image.bytes.len() > max => ImageContents::TooLarge,
            Some((image, false)) if !image.bytes.is_empty() => ImageContents::Image(image.clone()),
            _ => ImageContents::Nothing,
        })
    }

    fn has_type(&mut self, name: &str) -> Option<bool> {
        Some(self.with(|held| held.types.iter().any(|held| held == name)))
    }
}
