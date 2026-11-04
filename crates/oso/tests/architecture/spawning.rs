//! Lint attributes that would let code outside `oso-platform::process` spawn
//! (ADR 0010, ADR 0047). Clippy bans `Command::new`, but a source attribute
//! overrides the workspace lints, so this check keeps them honest.
//!
//! Must not depend on: the file system or the other checks; it sees only
//! tokens.

use crate::lexer::Token;

/// The one file that may silence the ban.
pub const ALLOWED: &str = "crates/oso-platform/src/process.rs";

/// The lines that name the lint, or the lint groups it belongs to: `clippy::all`
/// and `clippy::style`. Anywhere else those words mean something else, such as
/// `cfg(all(...))`.
pub fn silencing_lines(tokens: &[Token]) -> Vec<usize> {
    tokens
        .iter()
        .enumerate()
        .filter(|&(i, token)| {
            token.is_ident("disallowed_methods")
                || ((token.is_ident("all") || token.is_ident("style"))
                    && i.checked_sub(2)
                        .and_then(|before| tokens.get(before))
                        .is_some_and(|before| before.is_ident("clippy")))
        })
        .map(|(_, token)| token.line)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::silencing_lines;
    use crate::lexer::tokenize;

    #[test]
    fn finds_the_lint_and_its_groups() {
        let src = "#[expect(clippy::disallowed_methods, reason = \"x\")]\nfn a() {}\n\
                   #[allow(clippy::style)]\nfn b() {}\n#![allow(clippy::all)]\n";
        assert_eq!(silencing_lines(&tokenize(src)), [1, 3, 5]);
    }

    #[test]
    fn ignores_comments_literals_and_other_uses_of_the_words() {
        let src = "// clippy::disallowed_methods\nconst A: &str = \"clippy::all\";\n\
                   #[cfg(all(unix, test))]\nfn b(style: u8) {}\n\
                   #[allow(clippy::too_many_lines)]\nfn c() { [1].iter().all(|_| true); }\n";
        assert!(silencing_lines(&tokenize(src)).is_empty());
    }
}
