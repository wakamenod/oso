//! `static` items, which are global state (ADR 0038, ADR 0042). No lint flags
//! the declaration itself; rustc's `static_mut_refs` covers only references.
//! A `static` declared in an `extern` block at the top of a file or a module
//! names a foreign library's constant, read only, and is allowed there and
//! nowhere else, if its type cannot hold state: no interior mutability, atomic,
//! lock, `*mut` or `&mut` (ADR 0107).
//!
//! Must not depend on: the file system or the other checks; it sees only
//! tokens.

use crate::lexer::{Tok, Token, skip_group, test_item_lines};

/// Types that hold state behind a shared reference, which a foreign constant must not be.
const HOLDS_STATE: [&str; 9] = [
    "Cell",
    "RefCell",
    "UnsafeCell",
    "OnceCell",
    "LazyCell",
    "Mutex",
    "RwLock",
    "OnceLock",
    "LazyLock",
];

/// The lines of the `static` items in `tokens` that are not allowed: every
/// `static mut`, and any other `static` outside top-level test items and
/// `extern` blocks, or in one with a type that can hold state.
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
            let foreign = in_extern(i) && !holds_state(tokens, i);
            is_mut || !(in_test(token.line) || foreign)
        })
        .map(|(_, token)| token.line)
        .collect()
}

/// The token ranges of the `extern` blocks in `tokens` at the top of a file or of a
/// module, from the `{` to just past its `}`: one in a function's body, a macro's or
/// any other group is not counted. The ABI's string is not a token, so `extern "C" {`
/// reads as `extern {`; `extern crate` and `extern fn` open no block.
fn extern_blocks(tokens: &[Token]) -> Vec<(usize, usize)> {
    let mut blocks = Vec::new();
    // Whether each group open around the token is a module's body.
    let mut modules: Vec<bool> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        match token.tok {
            Tok::Punct('{' | '(' | '[') => {
                let after = |back: usize, word: &str| i >= back && tokens[i - back].is_ident(word);
                let brace = token.is_punct('{');
                if brace && after(1, "extern") && modules.iter().all(|&module| module) {
                    blocks.push((i, skip_group(tokens, i)));
                }
                modules.push(brace && after(2, "mod"));
            }
            Tok::Punct('}' | ')' | ']') => {
                modules.pop();
            }
            _ => {}
        }
    }
    blocks
}

/// Whether the type of the `static` at `at`, up to its `;`, can hold state: interior
/// mutability, an atomic, a lock, or a `*mut` or `&mut`.
fn holds_state(tokens: &[Token], at: usize) -> bool {
    let ty: Vec<&Token> = tokens[at..]
        .iter()
        .skip_while(|token| !token.is_punct(':'))
        .skip(1)
        .take_while(|token| !token.is_punct(';'))
        .collect();
    let mutable = ty
        .windows(2)
        .any(|pair| (pair[0].is_punct('*') || pair[0].is_punct('&')) && pair[1].is_ident("mut"));
    let shared = ty.iter().any(|token| match &token.tok {
        Tok::Ident(name) => HOLDS_STATE.contains(&name.as_str()) || name.starts_with("Atomic"),
        _ => false,
    });
    mutable || shared
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
    fn a_foreign_static_holds_no_state_and_is_declared_at_the_top_of_a_module() {
        let src = "unsafe extern \"C\" {\n    pub static A: AtomicU32;\n    static B: *mut u8;\n\
                   static C: &'static Cell<u8>;\n    static D: Mutex<u8>;\n    \
                   static E: &'static mut u8;\n    static F: *const u8;\n}\n\
                   fn f() {\n    unsafe extern \"C\" {\n        static G: u8;\n    }\n}\n\
                   macro_rules! m {\n    () => {\n        extern \"C\" {\n            static H: u8;\n\
                   }\n    };\n}\nmod inner {\n    unsafe extern \"C\" {\n        static I: u8;\n\
                   }\n}\n";
        assert_eq!(forbidden_lines(&tokenize(src)), [2, 3, 4, 5, 6, 11, 17]);
    }

    #[test]
    fn test_items_may_hold_a_static_but_not_a_static_mut() {
        let src =
            "#[cfg(test)]\nmod tests {\n    static A: u8 = 0;\n    static mut B: u8 = 0;\n}\n";
        assert_eq!(forbidden_lines(&tokenize(src)), [4]);
    }
}
