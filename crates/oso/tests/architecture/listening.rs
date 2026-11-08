//! Where the control directory may be made, listened on, cleaned or written to (ADR 0032,
//! ADR 0085). The transport is in every build, so this check keeps the rest of a release
//! build away from it: only the allowed places name `ControlDir::open`, `ControlDir::at`,
//! `listen`, `remove_stale` or `write_private_file`, in a call, a path or a function pointer.
//! The transport defines them, and only its tests use them.
//!
//! Must not depend on: the file system or the other checks; it sees only tokens.

use crate::lexer::{Tok, Token, test_item_lines, tokenize};

/// The places that may use them: the dev tooling, behind the `dev` feature. Adding one
/// needs an ADR; the agent channel joins with its start command (ADR 0085).
pub const ALLOWED: [&str; 1] = ["crates/oso/src/dev/"];

/// The transport, which defines them and may use them in its tests only.
pub const TRANSPORT: &str = "crates/oso-platform/src/control/";

/// What is wrong with `files`, given as paths and sources: a use outside [`ALLOWED`], one
/// outside the tests in [`TRANSPORT`], or an allowed place that uses none, which would mean
/// the walk missed it.
pub fn problems(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen_allowed = false;
    for (at, src) in files {
        let tokens = tokenize(src);
        let uses = uses(&tokens);
        if ALLOWED.iter().any(|allowed| at.starts_with(allowed)) {
            seen_allowed |= !uses.is_empty();
            continue;
        }
        let tests = if at.starts_with(TRANSPORT) {
            test_item_lines(&tokens)
        } else {
            Vec::new()
        };
        for line in uses {
            if !tests.iter().any(|&(from, to)| (from..=to).contains(&line)) {
                problems.push(format!(
                    "{at}:{line}: only {} may listen on or clean the control directory (ADR 0085)",
                    ALLOWED.join(", ")
                ));
            }
        }
    }
    if !seen_allowed {
        problems.push(format!("the walk saw no use in {}", ALLOWED.join(", ")));
    }
    problems
}

/// The lines that use one of the guarded names, outside their own `fn` definitions.
pub fn uses(tokens: &[Token]) -> Vec<usize> {
    let defined = |i: usize| i > 0 && tokens[i - 1].is_ident("fn");
    let after_dir = |i: usize| {
        i >= 2 && tokens[i - 1].tok == Tok::PathSep && tokens[i - 2].is_ident("ControlDir")
    };
    tokens
        .iter()
        .enumerate()
        .filter(|&(i, token)| {
            let guarded = ["listen", "remove_stale", "write_private_file"]
                .iter()
                .any(|name| token.is_ident(name))
                || ((token.is_ident("open") || token.is_ident("at")) && after_dir(i));
            guarded && !defined(i)
        })
        .map(|(_, token)| token.line)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ALLOWED, TRANSPORT, problems, uses};
    use crate::lexer::tokenize;

    const LISTENS: &str = "fn a(dir: &ControlDir) {\n    dir.listen(Socket::Agent);\n}\n";

    /// The allowed place, using one, and `others`.
    fn files(others: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut files = vec![(format!("{}mod.rs", ALLOWED[0]), LISTENS.to_owned())];
        files.extend(
            others
                .iter()
                .map(|(at, src)| ((*at).to_owned(), (*src).to_owned())),
        );
        files
    }

    #[test]
    fn finds_calls_paths_and_pointers_and_nothing_else() {
        assert_eq!(uses(&tokenize(LISTENS)), [2]);
        let forms = "ControlDir::listen(&d, s);\nlet f = ControlDir::listen;\nControlDir::open();\nControlDir::at(p);\nd.remove_stale();\nd.write_private_file(n, b);\n";
        assert_eq!(uses(&tokenize(forms)), [1, 2, 3, 4, 5, 6]);
        let other = "fn listen() {}\n// dir.listen(x)\nlet s = \"a.listen(b)\";\nFile::open(p);\nControlDir::open_existing();\n";
        assert_eq!(uses(&tokenize(other)), Vec::<usize>::new());
    }

    #[test]
    fn a_use_outside_the_allowed_places_is_a_problem() {
        assert_eq!(problems(&files(&[])), Vec::<String>::new());
        let found = problems(&files(&[("crates/oso/src/app.rs", LISTENS)]));
        assert_eq!(found.len(), 1);
        assert!(
            found[0].starts_with("crates/oso/src/app.rs:2:"),
            "{found:?}"
        );
    }

    #[test]
    fn the_transport_may_use_them_in_its_tests_only() {
        let at = format!("{TRANSPORT}unix.rs");
        let tested =
            "pub fn listen() {}\n#[cfg(test)]\nmod tests {\n    fn t() { d.listen(x); }\n}\n";
        assert_eq!(problems(&files(&[(&at, tested)])), Vec::<String>::new());
        let wrapped = "pub fn listen() {}\npub fn wrap(d: &D) { d.listen(x); }\n";
        let found = problems(&files(&[(&at, wrapped)]));
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn a_walk_that_misses_the_allowed_places_is_a_problem() {
        let found = problems(&[("crates/oso/src/app.rs".to_owned(), String::new())]);
        assert_eq!(found, ["the walk saw no use in crates/oso/src/dev/"]);
    }
}
