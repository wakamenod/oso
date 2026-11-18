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
const GO: &str = include_str!("../queries/go/highlights.scm");
// JavaScript, TypeScript and TSX share the ecma part, after the more specific ones: of two
// captures with the same span the first pattern wins, a parent's and its child's too (`paint`).
// TypeScript's `void` is a predefined type only because the typescript part comes before the
// ecma part's keyword `"void"` (ADR 0147).
const JAVASCRIPT: &str = concat!(
    include_str!("../queries/jsx/highlights.scm"),
    include_str!("../queries/ecma/highlights.scm"),
);
const TYPESCRIPT: &str = concat!(
    include_str!("../queries/typescript/highlights.scm"),
    include_str!("../queries/ecma/highlights.scm"),
);
const TSX: &str = concat!(
    include_str!("../queries/typescript/highlights.scm"),
    include_str!("../queries/jsx/highlights.scm"),
    include_str!("../queries/ecma/highlights.scm"),
);

const HTML: &str = include_str!("../queries/html/highlights.scm");
const CSS: &str = include_str!("../queries/css/highlights.scm");
const JSON: &str = include_str!("../queries/json/highlights.scm");
const YAML: &str = include_str!("../queries/yaml/highlights.scm");

/// Finds, in Markdown's block tree, the text another grammar parses, as the grammar's own
/// injection query does: the inline grammar's text, and fenced code blocks, whose info string
/// may name a language.
const MARKDOWN_INJECTIONS: &str =
    "[(inline) (pipe_table_cell)] @inline\n(fenced_code_block) @fence";

/// Finds, in HTML's tree, the elements whose text another grammar parses (ADR 0157); which one,
/// if any, their start tag says.
const HTML_INJECTIONS: &str = "[(script_element) (style_element)] @embedded";

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
    compiled: [Option<Compiled>; Kind::ALL.len()],
}

/// Which query of a language: its highlights, or the one that finds the text other grammars
/// parse in a host's tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Highlights(Language),
    Injections(Host),
}

/// A language whose tree holds text other grammars parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Host {
    /// Inline text and fenced code (ADR 0111).
    Markdown,
    /// `<script>` and `<style>` elements (ADR 0157).
    Html,
}

impl Host {
    /// Every host, once each.
    const ALL: [Self; 2] = [Self::Markdown, Self::Html];

    /// The host `language` is, if its tree holds text other grammars parse.
    pub(crate) fn of(language: Language) -> Option<Self> {
        match language {
            Language::Markdown => Some(Self::Markdown),
            Language::Html => Some(Self::Html),
            Language::Rust
            | Language::MarkdownInline
            | Language::Toml
            | Language::Go
            | Language::JavaScript
            | Language::TypeScript
            | Language::Tsx
            | Language::Css
            | Language::Json
            | Language::Yaml => None,
        }
    }
}

impl Kind {
    /// Every query, once each: each language's highlights, then each host's injections.
    pub(crate) const ALL: [Self; Language::ALL.len() + Host::ALL.len()] = {
        let mut all = [Self::Injections(Host::Markdown); Language::ALL.len() + Host::ALL.len()];
        let mut at = 0;
        while at < Language::ALL.len() {
            all[at] = Self::Highlights(Language::ALL[at]);
            at += 1;
        }
        while at < all.len() {
            all[at] = Self::Injections(Host::ALL[at - Language::ALL.len()]);
            at += 1;
        }
        all
    };
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

    /// Whether [`Self::prepare`] has tried to compile the query, whether or not it compiled.
    pub(crate) fn tried(&self, kind: Kind) -> bool {
        self.compiled[slot(kind)].is_some()
    }

    /// The query, once [`Self::prepare`] compiled it.
    pub(crate) fn get(&self, kind: Kind) -> Option<&Prepared> {
        match &self.compiled[slot(kind)] {
            Some(Compiled::Ready(prepared)) => Some(prepared),
            Some(Compiled::Broken) | None => None,
        }
    }
}

/// Where a query is kept in [`Queries`]: each of [`Kind::ALL`] its own place, which a test
/// checks.
fn slot(kind: Kind) -> usize {
    match kind {
        Kind::Highlights(Language::Rust) => 0,
        Kind::Highlights(Language::Markdown) => 1,
        Kind::Highlights(Language::MarkdownInline) => 2,
        Kind::Highlights(Language::Toml) => 3,
        Kind::Injections(Host::Markdown) => 4,
        Kind::Highlights(Language::Go) => 5,
        Kind::Highlights(Language::JavaScript) => 6,
        Kind::Highlights(Language::TypeScript) => 7,
        Kind::Highlights(Language::Tsx) => 8,
        Kind::Highlights(Language::Html) => 9,
        Kind::Highlights(Language::Css) => 10,
        Kind::Highlights(Language::Json) => 11,
        Kind::Highlights(Language::Yaml) => 12,
        Kind::Injections(Host::Html) => 13,
    }
}

/// The grammar a query is for, and its text.
fn source(kind: Kind) -> (Language, &'static str) {
    match kind {
        Kind::Highlights(Language::Rust) => (Language::Rust, RUST),
        Kind::Highlights(Language::Markdown) => (Language::Markdown, MARKDOWN),
        Kind::Highlights(Language::MarkdownInline) => (Language::MarkdownInline, MARKDOWN_INLINE),
        Kind::Highlights(Language::Toml) => (Language::Toml, TOML),
        Kind::Injections(Host::Markdown) => (Language::Markdown, MARKDOWN_INJECTIONS),
        Kind::Highlights(Language::Go) => (Language::Go, GO),
        Kind::Highlights(Language::JavaScript) => (Language::JavaScript, JAVASCRIPT),
        Kind::Highlights(Language::TypeScript) => (Language::TypeScript, TYPESCRIPT),
        Kind::Highlights(Language::Tsx) => (Language::Tsx, TSX),
        Kind::Highlights(Language::Html) => (Language::Html, HTML),
        Kind::Highlights(Language::Css) => (Language::Css, CSS),
        Kind::Highlights(Language::Json) => (Language::Json, JSON),
        Kind::Highlights(Language::Yaml) => (Language::Yaml, YAML),
        Kind::Injections(Host::Html) => (Language::Html, HTML_INJECTIONS),
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
