//! `static` items, which are global state (ADR 0038, ADR 0042). No lint flags
//! the declaration itself; rustc's `static_mut_refs` covers only references.
//!
//! Must not depend on: the file system or the other checks; it sees only
//! tokens.

use crate::lexer::{Token, test_item_lines};

/// The lines of the `static` items in `tokens` that are not allowed: every
/// `static mut`, and any other `static` outside top-level test items.
pub fn forbidden_lines(tokens: &[Token]) -> Vec<usize> {
    let tests = test_item_lines(tokens);
    let in_test = |line| {
        tests
            .iter()
            .any(|&(start, end)| start <= line && line <= end)
    };
    tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.is_ident("static"))
        .filter(|&(i, token)| {
            let is_mut = tokens.get(i + 1).is_some_and(|next| next.is_ident("mut"));
            is_mut || !in_test(token.line)
        })
        .map(|(_, token)| token.line)
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
    fn test_items_may_hold_a_static_but_not_a_static_mut() {
        let src =
            "#[cfg(test)]\nmod tests {\n    static A: u8 = 0;\n    static mut B: u8 = 0;\n}\n";
        assert_eq!(forbidden_lines(&tokenize(src)), [4]);
    }
}
