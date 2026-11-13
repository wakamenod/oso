//! Which keys a buffer takes, what its modeline calls it, and which language it holds (ADR
//! 0064, ADR 0111). This is a closed list until major modes arrive in M3, which replace it with
//! something the tip crate can add to.
//!
//! Must not depend on: `window`, `editor` or `command`.

use std::path::Path;

use crate::indent::{self, Answer, Budget, Line, Settings};
use crate::rope::Rope;

/// Which keys a buffer takes, what its modeline calls it, and which language it holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    /// A buffer of the user's in no language oso knows, or `*Messages*`.
    #[default]
    Fundamental,
    /// The list of emergency copies.
    CopyList,
    /// The text of one emergency copy, or how it differs from its file.
    CopyView,
    /// Rust source.
    Rust,
    /// Markdown.
    Markdown,
    /// TOML.
    Toml,
    /// A review's buffer (ADR 0122).
    Review,
    /// A terminal (ADR 0120): its keys go to the program in it, but for oso's own.
    Terminal,
}

impl Mode {
    /// The mode a file's name gives it, as Emacs's `auto-mode-alist` does: only the name decides,
    /// never what the file holds (ADR 0111). Extensions are compared ignoring ASCII case.
    pub fn for_path(path: &Path) -> Self {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Self::Fundamental;
        };
        if name == "Cargo.lock" {
            return Self::Toml;
        }
        let Some(extension) = Path::new(name).extension().and_then(|e| e.to_str()) else {
            return Self::Fundamental;
        };
        let is = |known: &str| extension.eq_ignore_ascii_case(known);
        if is("rs") {
            Self::Rust
        } else if is("md") || is("markdown") {
            Self::Markdown
        } else if is("toml") {
            Self::Toml
        } else {
            Self::Fundamental
        }
    }

    /// What the modeline calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Fundamental => "Fundamental",
            Self::CopyList => "Emergency Copies",
            Self::CopyView => "Emergency Copy",
            Self::Rust => "Rust",
            Self::Markdown => "Markdown",
            Self::Toml => "TOML",
            Self::Review => "Review",
            Self::Terminal => "Terminal",
        }
    }

    /// Whether this is one of the buffers that show emergency copies.
    pub fn shows_copies(self) -> bool {
        matches!(self, Self::CopyList | Self::CopyView)
    }

    /// The rules its lines are indented by (ADR 0115), or none for the emergency copies'
    /// buffers, whose text is not the person's to indent, and for a terminal, which has none.
    #[expect(
        clippy::type_complexity,
        reason = "the rules' own signature, which naming would make a public item of its own"
    )]
    pub fn indent_rules(self) -> Option<fn(&Rope, Settings, &[Line], &mut Budget) -> Vec<Answer>> {
        match self {
            Self::Rust => Some(indent::rust),
            Self::Toml => Some(indent::toml),
            Self::Markdown => Some(indent::markdown),
            Self::Fundamental => Some(indent::fundamental),
            Self::CopyList | Self::CopyView | Self::Review | Self::Terminal => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::Mode;

    #[test]
    fn a_files_name_decides_its_mode() {
        let mode = |path: &str| Mode::for_path(Path::new(path));
        assert_eq!(mode("/src/main.rs"), Mode::Rust);
        assert_eq!(mode("README.md"), Mode::Markdown);
        assert_eq!(mode("notes/Today.MARKDOWN"), Mode::Markdown);
        assert_eq!(mode("Cargo.toml"), Mode::Toml);
        assert_eq!(mode("work/Cargo.lock"), Mode::Toml);
        assert_eq!(mode("LIB.RS"), Mode::Rust);
    }

    #[test]
    fn each_mode_but_the_copies_has_indentation_rules() {
        let modes = [Mode::Rust, Mode::Toml, Mode::Markdown, Mode::Fundamental];
        assert!(modes.iter().all(|mode| mode.indent_rules().is_some()));
        assert!(Mode::CopyList.indent_rules().is_none());
        assert!(Mode::CopyView.indent_rules().is_none());
        assert!(Mode::Terminal.indent_rules().is_none());
    }

    #[test]
    fn a_name_with_no_known_extension_is_fundamental() {
        let mode = |path: &str| Mode::for_path(Path::new(path));
        assert_eq!(mode("notes.txt"), Mode::Fundamental);
        assert_eq!(mode(".rs"), Mode::Fundamental);
        assert_eq!(mode("rs"), Mode::Fundamental);
        assert_eq!(mode("cargo.lock"), Mode::Fundamental);
        assert_eq!(mode("main.rs.orig"), Mode::Fundamental);
        assert_eq!(mode("/"), Mode::Fundamental);
        assert_eq!(mode(""), Mode::Fundamental);
    }
}
