//! Whether a buffer's state says it shows highlighted text: any of its first runs of a syntax
//! class, as a review's side is once highlighted (ADR 0122, step 9).
//!
//! Must not depend on: the app, the editor or the socket; this reads highlights it is given.

/// The most runs [`has_syntax`] looks at, so that a state costs the same whatever the text.
const MOST_RUNS_LOOKED_AT: usize = 4096;

/// Whether any of the first runs of `highlights` is of a syntax class.
pub(super) fn has_syntax(highlights: &oso_core::syntax::Highlights) -> bool {
    use oso_core::syntax::Highlight;
    highlights
        .runs(0..highlights.len())
        .take(MOST_RUNS_LOOKED_AT)
        .any(|(_, highlight)| {
            matches!(
                highlight,
                Some(
                    Highlight::Comment
                        | Highlight::String
                        | Highlight::Keyword
                        | Highlight::Type
                        | Highlight::Function
                        | Highlight::Constant
                        | Highlight::Property
                        | Highlight::Macro
                        | Highlight::Heading
                        | Highlight::Link
                        | Highlight::Markup
                )
            )
        })
}
