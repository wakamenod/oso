//! oso's highlight queries, compiled into the binary, each compiled at its language's first use
//! on the worker and kept for the session (ADR 0111). Nothing is loaded at run time.
//!
//! Must not depend on: `document`; documents borrow the queries, which own none of them.
//!
//! - Each query file names one of oso's highlight classes in every capture, so a capture maps
//!   to a [`Highlight`] by its name alone.
//! - Predicates are only those tree-sitter's crate evaluates itself, with `regex`: `#eq?`,
//!   `#match?` and `#any-of?`. A unit test rejects any other.

use oso_core::syntax::Highlight;
use tree_sitter::Query;

use crate::language::Language;

const RUST: &str = include_str!("../queries/rust/highlights.scm");
const MARKDOWN: &str = include_str!("../queries/markdown/highlights.scm");
const MARKDOWN_INLINE: &str = include_str!("../queries/markdown_inline/highlights.scm");
const TOML: &str = include_str!("../queries/toml/highlights.scm");

/// Finds, in Markdown's block tree, the text another grammar parses, as the grammar's own
/// injection query does: the inline grammar's text, and fenced code blocks, whose info string
/// may name a language.
const INJECTIONS: &str = "[(inline) (pipe_table_cell)] @inline\n(fenced_code_block) @fence";

/// A query once compiled, with the class each of its captures names; or the note that it
/// could not be.
enum Compiled {
    Ready(Prepared),
    Broken,
}

/// A compiled query and the class of each capture, by capture index.
pub(crate) struct Prepared {
    pub(crate) query: Query,
    pub(crate) classes: Vec<Option<Highlight>>,
}

/// The compiled highlight queries, one per language. The worker owns it and lends it to each
/// document; [`Queries::new`] compiles nothing.
#[derive(Default)]
pub struct Queries {
    /// By [`slot`].
    compiled: [Option<Compiled>; 5],
}

/// Which query of a language: its highlights, or the one that finds the text other grammars
/// parse in Markdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Highlights(Language),
    Injections,
}

impl Queries {
    /// No query compiled yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Compiles the query if it is not yet, and says whether it compiled.
    pub(crate) fn prepare(&mut self, kind: Kind) -> bool {
        let compiled = self.compiled[slot(kind)].get_or_insert_with(|| {
            let (language, source) = source(kind);
            // A unit test compiles every query, so this fails only with a broken build.
            match Query::new(&language.grammar(), source) {
                Ok(query) => {
                    let classes = query
                        .capture_names()
                        .iter()
                        .map(|name| class(name))
                        .collect();
                    Compiled::Ready(Prepared { query, classes })
                }
                Err(_) => Compiled::Broken,
            }
        });
        matches!(compiled, Compiled::Ready(_))
    }

    /// The query, once [`Self::prepare`] compiled it.
    pub(crate) fn get(&self, kind: Kind) -> Option<&Prepared> {
        match &self.compiled[slot(kind)] {
            Some(Compiled::Ready(prepared)) => Some(prepared),
            Some(Compiled::Broken) | None => None,
        }
    }
}

/// Where a query is kept in [`Queries`].
fn slot(kind: Kind) -> usize {
    match kind {
        Kind::Highlights(Language::Rust) => 0,
        Kind::Highlights(Language::Markdown) => 1,
        Kind::Highlights(Language::MarkdownInline) => 2,
        Kind::Highlights(Language::Toml) => 3,
        Kind::Injections => 4,
    }
}

/// The grammar a query is for, and its text.
fn source(kind: Kind) -> (Language, &'static str) {
    match kind {
        Kind::Highlights(Language::Rust) => (Language::Rust, RUST),
        Kind::Highlights(Language::Markdown) => (Language::Markdown, MARKDOWN),
        Kind::Highlights(Language::MarkdownInline) => (Language::MarkdownInline, MARKDOWN_INLINE),
        Kind::Highlights(Language::Toml) => (Language::Toml, TOML),
        Kind::Injections => (Language::Markdown, INJECTIONS),
    }
}

/// The class a capture names, or none for a name that is not one.
pub(crate) fn class(capture: &str) -> Option<Highlight> {
    Some(match capture {
        "comment" => Highlight::Comment,
        "string" => Highlight::String,
        "keyword" => Highlight::Keyword,
        "type" => Highlight::Type,
        "function" => Highlight::Function,
        "constant" => Highlight::Constant,
        "property" => Highlight::Property,
        "macro" => Highlight::Macro,
        "heading" => Highlight::Heading,
        "link" => Highlight::Link,
        "markup" => Highlight::Markup,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
