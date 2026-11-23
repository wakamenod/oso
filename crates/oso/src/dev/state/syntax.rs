//! Whether a buffer's state says it shows highlighted text: any of its first runs of a syntax
//! class, as a review's side is once highlighted (ADR 0122, step 9), and whether any is of one
//! only a parse gives, as a native session's reply is once its code is (ADR 0165).
//!
//! Must not depend on: the app, the editor or the socket; this reads highlights it is given.

use oso_core::syntax::{Highlight, Highlights};

/// The most runs looked at, so that a state costs the same whatever the text.
const MOST_RUNS_LOOKED_AT: usize = 4096;

/// Whether any of the first runs of `highlights` is of a syntax class.
pub(super) fn has_syntax(highlights: &Highlights) -> bool {
    any(highlights, |highlight| {
        code(highlight)
            || matches!(
                highlight,
                Highlight::String | Highlight::Heading | Highlight::Link | Highlight::Markup
            )
    })
}

/// Whether any of the first runs of `highlights` is of a class of code, which no row of oso's
/// own draws but a tool's name, a function's.
pub(super) fn has_code(highlights: &Highlights) -> bool {
    any(highlights, |highlight| {
        code(highlight) && highlight != Highlight::Function
    })
}

fn any(highlights: &Highlights, is: impl Fn(Highlight) -> bool) -> bool {
    (highlights.runs(0..highlights.len()))
        .take(MOST_RUNS_LOOKED_AT)
        .any(|(_, highlight)| highlight.is_some_and(&is))
}

fn code(highlight: Highlight) -> bool {
    matches!(
        highlight,
        Highlight::Comment
            | Highlight::Keyword
            | Highlight::Type
            | Highlight::Function
            | Highlight::Constant
            | Highlight::Property
            | Highlight::Macro
    )
}
