//! Lint attributes that would let code outside `oso-platform::process` spawn
//! (ADR 0010, ADR 0047). Clippy bans `Command::new`, but a source attribute
//! overrides the workspace lints, so this check keeps them honest. The
//! benchmark harness is the one recorded exception (ADR 0046).
//!
//! Must not depend on: the file system or the other checks; it sees only
//! tokens.

use crate::lexer::{Token, tokenize};

/// The files that may silence the ban: the process module, its terminals'
/// child-side hook (ADR 0120), and the benchmark harness, which never ships
/// (ADR 0046). Adding one needs an ADR.
pub const ALLOWED: [&str; 3] = [
    "crates/oso-platform/src/process.rs",
    "crates/oso-platform/src/process/pty/unix.rs",
    "crates/oso/examples/perf/process.rs",
];

/// What is wrong with `files`, given as paths and sources. Any file outside
/// [`ALLOWED`] must not silence the ban. Every allowed file must be there and
/// must say that it spawns.
pub fn problems(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for allowed in ALLOWED {
        if !files.iter().any(|(at, _)| at == allowed) {
            problems.push(format!("the walk did not see {allowed}"));
        }
    }
    for (at, src) in files {
        let lines = silencing_lines(&tokenize(src));
        if ALLOWED.contains(&at.as_str()) {
            if lines.is_empty() {
                problems.push(format!("{at} spawns, so it must say so"));
            }
            continue;
        }
        for line in lines {
            problems.push(format!(
                "{at}:{line}: spawn only through oso-platform::process (ADR 0010, ADR 0047)"
            ));
        }
    }
    problems
}

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
    use super::{ALLOWED, problems, silencing_lines};
    use crate::lexer::tokenize;

    const SILENCED: &str = "#[expect(clippy::disallowed_methods, reason = \"x\")]\nfn a() {}\n";

    /// Every allowed file, each saying that it spawns.
    fn allowed_files() -> Vec<(String, String)> {
        ALLOWED
            .iter()
            .map(|at| ((*at).to_owned(), SILENCED.to_owned()))
            .collect()
    }

    #[test]
    fn the_allowed_files_may_silence_the_ban() {
        assert_eq!(problems(&allowed_files()), Vec::<String>::new());
    }

    #[test]
    fn any_other_file_may_not() {
        for at in [
            "crates/oso/examples/perf/bench.rs",
            "crates/oso/examples/other/process.rs",
            "crates/oso/src/process.rs",
        ] {
            let mut files = allowed_files();
            files.push((at.to_owned(), SILENCED.to_owned()));
            assert_eq!(
                problems(&files),
                [format!(
                    "{at}:1: spawn only through oso-platform::process (ADR 0010, ADR 0047)"
                )]
            );
        }
    }

    #[test]
    fn every_allowed_file_must_be_seen_and_say_so() {
        for (index, allowed) in ALLOWED.iter().enumerate() {
            let mut files = allowed_files();
            files[index].1 = "fn a() {}\n".to_owned();
            assert_eq!(
                problems(&files),
                [format!("{allowed} spawns, so it must say so")]
            );
            files.remove(index);
            assert_eq!(
                problems(&files),
                [format!("the walk did not see {allowed}")]
            );
        }
    }

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
