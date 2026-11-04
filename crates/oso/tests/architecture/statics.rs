//! `static mut` items, which are global state (ADR 0038). No lint flags the
//! declaration itself; rustc's `static_mut_refs` covers only references.

use crate::lexer::Token;

/// The lines of the `static mut` items in `tokens`.
pub fn static_mut_lines(tokens: &[Token]) -> Vec<usize> {
    tokens
        .windows(2)
        .filter(|pair| pair[0].is_ident("static") && pair[1].is_ident("mut"))
        .map(|pair| pair[0].line)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::static_mut_lines;
    use crate::lexer::tokenize;

    #[test]
    fn static_mut_is_found_outside_comments_and_literals() {
        let src = "// static mut A\nstatic mut B: u8 = 0;\nconst S: &str = \"static mut C\";\n\
                   fn f(x: &'static mut u8) {}\n";
        assert_eq!(static_mut_lines(&tokenize(src)), [2]);
    }
}
