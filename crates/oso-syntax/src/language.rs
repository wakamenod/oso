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
    /// Go (`tree-sitter-go`).
    Go,
    /// JavaScript, JSX among it (`tree-sitter-javascript`).
    JavaScript,
    /// TypeScript (`tree-sitter-typescript`).
    TypeScript,
    /// TypeScript with JSX, a grammar of its own (`tree-sitter-typescript`).
    Tsx,
}

impl Language {
    /// Every grammar, once each.
    pub(crate) const ALL: [Self; 8] = [
        Self::Rust,
        Self::Markdown,
        Self::MarkdownInline,
        Self::Toml,
        Self::Go,
        Self::JavaScript,
        Self::TypeScript,
        Self::Tsx,
    ];

    /// The grammar a buffer in `mode` is parsed with, if any. Markdown's inline grammar is never
    /// a buffer's: it parses the text inside Markdown's blocks.
    pub fn of(mode: Mode) -> Option<Self> {
        match mode {
            Mode::Rust => Some(Self::Rust),
            Mode::Markdown => Some(Self::Markdown),
            Mode::Toml => Some(Self::Toml),
            Mode::Go => Some(Self::Go),
            Mode::JavaScript => Some(Self::JavaScript),
            Mode::TypeScript => Some(Self::TypeScript),
            Mode::Tsx => Some(Self::Tsx),
            // These modes of ADR 0138 have no grammar yet.
            Mode::Fundamental
            | Mode::Latex
            | Mode::Swift
            | Mode::CopyList
            | Mode::CopyView
            | Mode::Brief
            | Mode::Worktrees
            | Mode::Review
            | Mode::Terminal
            | Mode::Directory
            | Mode::GitStatus
            | Mode::GitGraph
            | Mode::GitPr
            | Mode::GitPrList
            | Mode::GitCommit
            | Mode::GitPrMessage
            | Mode::ReviewMessage
            | Mode::GitLog
            | Mode::Conversation
            | Mode::GitDiscarded
            | Mode::Sessions => None,
        }
    }

    /// The grammar's name, for log lines.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Markdown => "markdown",
            Self::MarkdownInline => "markdown_inline",
            Self::Toml => "toml",
            Self::Go => "go",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
        }
    }

    /// The compiled-in grammar.
    pub(crate) fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
            Self::MarkdownInline => tree_sitter_md::INLINE_LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Mode;

    use super::Language;

    #[test]
    fn every_grammar_fits_the_runtime() {
        for language in Language::ALL {
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
        assert_eq!(Language::of(Mode::Go), Some(Language::Go));
        assert_eq!(Language::of(Mode::JavaScript), Some(Language::JavaScript));
        assert_eq!(Language::of(Mode::TypeScript), Some(Language::TypeScript));
        assert_eq!(Language::of(Mode::Tsx), Some(Language::Tsx));
        assert_eq!(Language::of(Mode::Latex), None);
        assert_eq!(Language::of(Mode::Swift), None);
        assert_eq!(Language::of(Mode::Fundamental), None);
        assert_eq!(Language::of(Mode::CopyList), None);
        assert_eq!(Language::of(Mode::CopyView), None);
    }
}
