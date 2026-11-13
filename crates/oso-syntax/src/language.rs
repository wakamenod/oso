//! Which grammar a mode uses (ADR 0111). The grammars are compiled in, pinned to exact
//! versions; nothing is loaded at run time.
//!
//! Must not depend on: `document` or `edit`.

use oso_core::buffer::Mode;

/// A grammar oso parses with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    /// Rust (`tree-sitter-rust`).
    Rust,
    /// Markdown's blocks: headings, lists, quotes and code blocks (`tree-sitter-md`).
    Markdown,
    /// Markdown's inline text, parsed one block's text at a time (`tree-sitter-md`).
    MarkdownInline,
    /// TOML (`tree-sitter-toml-ng`).
    Toml,
}

impl Language {
    /// The grammar a buffer in `mode` is parsed with, if any. Markdown's inline grammar is never
    /// a buffer's: it parses the text inside Markdown's blocks.
    pub fn of(mode: Mode) -> Option<Self> {
        match mode {
            Mode::Rust => Some(Self::Rust),
            Mode::Markdown => Some(Self::Markdown),
            Mode::Toml => Some(Self::Toml),
            Mode::Fundamental | Mode::CopyList | Mode::CopyView | Mode::Terminal => None,
        }
    }

    /// The grammar's name, for log lines.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Markdown => "markdown",
            Self::MarkdownInline => "markdown_inline",
            Self::Toml => "toml",
        }
    }

    /// The compiled-in grammar.
    pub(crate) fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
            Self::MarkdownInline => tree_sitter_md::INLINE_LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Mode;

    use super::Language;

    const ALL: [Language; 4] = [
        Language::Rust,
        Language::Markdown,
        Language::MarkdownInline,
        Language::Toml,
    ];

    #[test]
    fn every_grammar_fits_the_runtime() {
        for language in ALL {
            let mut parser = tree_sitter::Parser::new();
            assert!(
                parser.set_language(&language.grammar()).is_ok(),
                "{}",
                language.name()
            );
        }
    }

    #[test]
    fn only_language_modes_have_a_grammar() {
        assert_eq!(Language::of(Mode::Rust), Some(Language::Rust));
        assert_eq!(Language::of(Mode::Markdown), Some(Language::Markdown));
        assert_eq!(Language::of(Mode::Toml), Some(Language::Toml));
        assert_eq!(Language::of(Mode::Fundamental), None);
        assert_eq!(Language::of(Mode::CopyList), None);
        assert_eq!(Language::of(Mode::CopyView), None);
    }
}
