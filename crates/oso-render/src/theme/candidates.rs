//! Colours still to be picked (ADR 0089): `dusk`'s question colour, drawn as sample frames for
//! the author. Tests only; the picked one moves into `dusk`.
//!
//! Must not depend on: `atlas`, `grid`, `rows`, `scene`, `shape`, `text` or the GPU.

use super::{Color, Kind, Name, Source, Theme, dusk, rgb, source};

/// `dusk` with a question's words in a pale yellow.
fn yellow(name: Name) -> Color {
    match name {
        Name::FgQuestion => rgb(0xfff0a8),
        other => dusk(other),
    }
}

/// `dusk` with a question's words in a pale cyan.
fn cyan(name: Name) -> Color {
    match name {
        Name::FgQuestion => rgb(0xb8f4ff),
        other => dusk(other),
    }
}

/// `dusk` with a question's words in a pale green.
fn green(name: Name) -> Color {
    match name {
        Name::FgQuestion => rgb(0xc8f7c0),
        other => dusk(other),
    }
}

const SOURCES: [Source; 3] = [
    source("dusk-question-yellow", Kind::Dark, yellow),
    source("dusk-question-cyan", Kind::Dark, cyan),
    source("dusk-question-green", Kind::Dark, green),
];

/// Every candidate, resolved as a built-in theme would be.
pub(crate) fn candidates() -> Vec<Theme> {
    SOURCES.iter().map(Theme::resolve).collect()
}
