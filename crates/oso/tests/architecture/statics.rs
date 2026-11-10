//! `static` items, which are global state (ADR 0038, ADR 0042). No lint flags
//! the declaration itself; rustc's `static_mut_refs` covers only references.
//! A `static` declared in an `extern` block names a foreign library's constant,
//! read only, and is allowed there and nowhere else (ADR 0107).
//!
//! Must not depend on: the file system or the other checks; it sees only
//! tokens.

use crate::lexer::{Token, skip_group, test_item_lines};

/// The lines of the `static` items in `tokens` that are not allowed: every
/// `static mut`, and any other `static` outside top-level test items and
/// `extern` blocks.
pub fn forbidden_lines(tokens: &[Token]) -> Vec<usize> {
    let tests = test_item_lines(tokens);
    let in_test = |line| {
        tests
            .iter()
            .any(|&(start, end)| start <= line && line <= end)
    };
    let externs = extern_blocks(tokens);
    let in_extern = |i| externs.iter().any(|&(open, end)| open < i && i < end);
    tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.is_ident("static"))
        .filter(|&(i, token)| {
            let is_mut = tokens.get(i + 1).is_some_and(|next| next.is_ident("mut"));
            is_mut || !(in_test(token.line) || in_extern(i))
        })
        .map(|(_, token)| token.line)
        .collect()
}

/// The token ranges of the `extern` blocks in `tokens`, from the `{` to just past
/// its `}`. The ABI's string is not a token, so `extern "C" {` reads as `extern {`;
/// `extern crate` and `extern fn` open no block.
fn extern_blocks(tokens: &[Token]) -> Vec<(usize, usize)> {
    tokens
        .iter()
        .enumerate()
        .filter(|&(i, token)| {
            token.is_ident("extern") && tokens.get(i + 1).is_some_and(|next| next.is_punct('{'))
        })
        .map(|(i, _)| (i + 1, skip_group(tokens, i + 1)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::forbidden_lines;
    use crate::lexer::tokenize;

    #[test]
    fn statics_are_found_outside_comments_and_literals() {
        let src = "// static A\nstatic B: u8 = 0;\nconst S: &str = \"static C\";\n\
                   fn f(x: &'static mut u8) {}\nfn g() { static D: u8 = 0; }\n\
                   static mut E: u8 = 0;\n";
        assert_eq!(forbidden_lines(&tokenize(src)), [2, 5, 6]);
    }

    #[test]
    fn an_extern_block_may_declare_a_foreign_static_and_nothing_else_may() {
        let src = "unsafe extern \"C\" {\n    fn f() -> u8;\n    static K: &'static u8;\n    \
                   static mut M: u8;\n}\nstatic S: u8 = 0;\nextern crate core;\n\
                   extern \"C\" fn g() {\n    static T: u8 = 0;\n}\n";
        assert_eq!(forbidden_lines(&tokenize(src)), [4, 6, 9]);
    }

    #[test]
    fn test_items_may_hold_a_static_but_not_a_static_mut() {
        let src =
            "#[cfg(test)]\nmod tests {\n    static A: u8 = 0;\n    static mut B: u8 = 0;\n}\n";
        assert_eq!(forbidden_lines(&tokenize(src)), [4]);
    }
}
