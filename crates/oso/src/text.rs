//! The default font, and the sample text drawn until buffers arrive.

use oso_platform::text::{self, FontRequest, NullTextSystem, TextSystem};

/// Font families tried in order, until configuration exists (ADR 0019).
/// SF Mono is found by name only where it is installed as a font; Menlo
/// ships with every macOS.
pub const FONT_FAMILIES: &[&str] = &["SF Mono", "Menlo"];

/// The font size in points, until configuration exists (ADR 0019).
pub const FONT_SIZE_PT: f32 = 13.0;

/// TEMPORARY (M1-5): rows drawn at the top left until M1-6 draws buffers.
/// The cursor moves over the first row, one cell per `<right>`.
pub const SAMPLE_ROWS: &[&str] = &[
    "abc 日本語 🍣 xyz",
    "oso: OS-native text on a monospace cell grid",
    "全角は２セル、半角は１セル: ｱｲｳ ABC",
    "Emoji: 🍣 👍 ✨ 🦀 ❤️",
    "0123456789 !@#$%^&*() []{}<> -> => != ==",
];

/// Loads the default font for a window at `scale` physical pixels per
/// point. Where text cannot be drawn (no backend on this OS, or no font),
/// the grid still exists and nothing is drawn on it.
pub fn load(scale: f64) -> Box<dyn TextSystem> {
    let request = FontRequest {
        families: FONT_FAMILIES,
        size_pt: FONT_SIZE_PT,
    };
    let started = std::time::Instant::now();
    match text::load(&request, scale) {
        Ok(system) => {
            log::debug!(
                "font loaded at scale {scale} in {:?}: {:?}",
                started.elapsed(),
                system.metrics()
            );
            system
        }
        Err(e) => {
            log::warn!("{e}; drawing no text");
            Box::new(NullTextSystem::new(FONT_SIZE_PT, scale))
        }
    }
}
