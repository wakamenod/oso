//! Where a control socket may be opened for listening (ADR 0032, ADR 0085). The transport
//! is in every build, so this check keeps release builds from listening: only the dev
//! tooling, which release builds never contain, calls `ControlDir::listen`. The agent
//! channel joins it once its start command exists.
//!
//! Must not depend on: the file system or the other checks; it sees only tokens.

use crate::lexer::{Token, tokenize};

/// The places that may call `.listen(`: the dev tooling, behind the `dev` feature, and the
/// transport's own tests. Adding one needs an ADR.
pub const ALLOWED: [&str; 2] = ["crates/oso/src/dev/", "crates/oso-platform/src/control/"];

/// What is wrong with `files`, given as paths and sources: a call to `.listen(` outside
/// [`ALLOWED`].
pub fn problems(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (at, src) in files {
        if ALLOWED.iter().any(|allowed| at.starts_with(allowed)) {
            continue;
        }
        for line in listen_calls(&tokenize(src)) {
            problems.push(format!(
                "{at}:{line}: only the dev tooling listens on a control socket (ADR 0085)"
            ));
        }
    }
    problems
}

/// The lines of method calls named `listen`: `.listen(`.
pub fn listen_calls(tokens: &[Token]) -> Vec<usize> {
    tokens
        .windows(3)
        .filter(|w| w[0].is_punct('.') && w[1].is_ident("listen") && w[2].is_punct('('))
        .map(|w| w[1].line)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{listen_calls, problems};
    use crate::lexer::tokenize;

    const LISTENS: &str = "fn a(dir: &ControlDir) {\n    dir.listen(Socket::Agent);\n}\n";

    fn file(at: &str, src: &str) -> Vec<(String, String)> {
        vec![(at.to_owned(), src.to_owned())]
    }

    #[test]
    fn finds_a_call_and_nothing_else() {
        assert_eq!(listen_calls(&tokenize(LISTENS)), [2]);
        let other = "fn listen() {}\n// dir.listen(x)\nlet s = \"a.listen(b)\";\n";
        assert_eq!(listen_calls(&tokenize(other)), Vec::<usize>::new());
    }

    #[test]
    fn a_call_outside_the_dev_tooling_is_a_problem() {
        assert_eq!(
            problems(&file("crates/oso/src/dev/mod.rs", LISTENS)),
            Vec::<String>::new()
        );
        let found = problems(&file("crates/oso/src/app.rs", LISTENS));
        assert_eq!(found.len(), 1);
        assert!(
            found[0].starts_with("crates/oso/src/app.rs:2:"),
            "{found:?}"
        );
    }
}
