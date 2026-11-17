//! A review buffer's or a pane's whole text, laid out from its sections on the review's view
//! thread (ADR 0122, "The views"): the bytes end to end through `load::Builder`, which keeps
//! invalid bytes for `\xNN` as a loaded file's, compared with the text before, with what each
//! line is and its highlights.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system.

use std::sync::Arc;

use oso_core::display_map::TabWidth;
use oso_core::load::{Builder, Reloaded};
use oso_core::review::Rows;
use oso_core::rope::Rope;
use oso_core::syntax::Highlights;

use super::section::Rendered;

/// One text laid out from sections: the text compared with the one before, what each line is,
/// its highlights, the text itself, and whether every section's bytes were UTF-8.
pub(super) struct Assembled {
    pub(super) text: Reloaded,
    pub(super) rows: Rows,
    pub(super) highlights: Highlights,
    pub(super) rope: Rope,
    pub(super) valid: bool,
}

/// Lays `sections` end to end as one text, compared with `before`.
pub(super) fn assemble(sections: &[&Rendered], before: &Rope) -> Assembled {
    let size: usize = sections.iter().map(|rendered| rendered.bytes.len()).sum();
    let mut builder = Builder::new(Some(size as u64), TabWidth::default());
    let mut decoded = 0;
    for rendered in sections {
        builder.push(&rendered.bytes);
        decoded += rendered.runs.iter().map(|(len, _)| len).sum::<usize>();
    }
    builder.finish();
    let snapshot = builder.snapshot();
    let mut highlights = Highlights::new(snapshot.text().len());
    let runs = sections
        .iter()
        .flat_map(|rendered| rendered.runs.iter().copied());
    highlights.replace(0..decoded, runs);
    let rows = Rows::new(
        sections
            .iter()
            .map(|rendered| Arc::clone(&rendered.section))
            .collect(),
    );
    Assembled {
        rope: snapshot.text().clone(),
        text: snapshot.compared_with(before),
        rows,
        highlights,
        valid: sections.iter().all(|rendered| rendered.valid),
    }
}
