//! Where the control directory may be named, made, listened on, cleaned or written to
//! (ADR 0032, ADR 0085). The transport is in every build, so this check keeps the rest of a
//! release build away from it.
//!
//! Must not depend on: the file system or the other checks; it sees only tokens.
//!
//! - Only the allowed places name `ControlDir` at all, so no alias, type alias or qualified
//!   path reaches its constructors from elsewhere, and none imports `control` by glob.
//! - Of those, only the dev tooling and the agent channel use `listen`, `remove_stale` or `write_private_file`, in a
//!   call, a path or a function pointer. The test harnesses only connect.
//! - The transport defines them, and only its tests use them.
//! - Only the app's agent module starts the agent channel, `channel::start`, from the
//!   person's command, in `app/agent/admit.rs`.

use crate::lexer::{Tok, Token, test_item_lines, tokenize};

/// The places that may listen, clean and write: the dev tooling, behind the `dev` feature,
/// and the agent channel, which listens only once the person runs its start command
/// (ADR 0085). Adding one needs an ADR.
pub const ALLOWED: [&str; 2] = ["crates/oso/src/dev/", "crates/oso/src/channel/"];

/// The places that may also name `ControlDir`, to connect: the end-to-end harness and the
/// benchmark harness, which drive a dev build and never ship.
pub const CLIENTS: [&str; 2] = ["crates/oso/tests/", "crates/oso/examples/"];

/// The one place that starts the agent channel, from the person's `agent-channel-start`
/// (ADR 0085).
pub const STARTS_THE_CHANNEL: &str = "crates/oso/src/app/agent/admit.rs";

/// The transport, which defines them and may use them in its tests only.
pub const TRANSPORT: &str = "crates/oso-platform/src/control/";

/// What is wrong with `files`, given as paths and sources: a use outside the places allowed
/// it, one outside the tests in [`TRANSPORT`], or an allowed place that uses none, which would
/// mean the walk missed it.
pub fn problems(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen_allowed = false;
    for (at, src) in files {
        let tokens = tokenize(src);
        if ALLOWED.iter().any(|allowed| at.starts_with(allowed)) {
            seen_allowed |= !uses(&tokens).is_empty();
            continue;
        }
        let tests = if at.starts_with(TRANSPORT) {
            test_item_lines(&tokens)
        } else {
            Vec::new()
        };
        let in_tests = |line: usize| tests.iter().any(|&(from, to)| (from..=to).contains(&line));
        let client = CLIENTS.iter().any(|client| at.starts_with(client));
        let mut lines = uses(&tokens);
        if !client && !at.starts_with(TRANSPORT) {
            lines.extend(names(&tokens));
        }
        lines.sort_unstable();
        lines.dedup();
        for line in lines.into_iter().filter(|&line| !in_tests(line)) {
            problems.push(format!(
                "{at}:{line}: only {} may name, listen on or clean the control directory \
                 (ADR 0085)",
                ALLOWED.join(", ")
            ));
        }
    }
    if !seen_allowed {
        problems.push(format!("the walk saw no use in {}", ALLOWED.join(", ")));
    }
    let mut starts = 0;
    for (at, src) in files {
        let lines = channel_starts(&tokenize(src));
        if at == STARTS_THE_CHANNEL {
            starts += lines.len();
            continue;
        }
        for line in lines {
            problems.push(format!(
                "{at}:{line}: only {STARTS_THE_CHANNEL} starts the agent channel (ADR 0085)"
            ));
        }
    }
    if starts != 1 {
        problems.push(format!(
            "{STARTS_THE_CHANNEL} must start the agent channel once, not {starts} times"
        ));
    }
    problems
}

/// The lines that name `channel::start`.
pub fn channel_starts(tokens: &[Token]) -> Vec<usize> {
    tokens
        .windows(3)
        .filter(|w| w[0].is_ident("channel") && w[1].tok == Tok::PathSep && w[2].is_ident("start"))
        .map(|w| w[2].line)
        .collect()
}

/// The lines that use one of the guarded methods, outside their own `fn` definitions.
pub fn uses(tokens: &[Token]) -> Vec<usize> {
    tokens
        .iter()
        .enumerate()
        .filter(|&(i, token)| {
            ["listen", "remove_stale", "write_private_file"]
                .iter()
                .any(|name| token.is_ident(name))
                && !(i > 0 && tokens[i - 1].is_ident("fn"))
        })
        .map(|(_, token)| token.line)
        .collect()
}

/// The lines that name `ControlDir`, or import `control` by glob.
pub fn names(tokens: &[Token]) -> Vec<usize> {
    tokens
        .iter()
        .enumerate()
        .filter(|&(i, token)| {
            token.is_ident("ControlDir")
                || (token.is_punct('*')
                    && i >= 2
                    && tokens[i - 1].tok == Tok::PathSep
                    && tokens[i - 2].is_ident("control"))
        })
        .map(|(_, token)| token.line)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ALLOWED, CLIENTS, STARTS_THE_CHANNEL, TRANSPORT, names, problems, uses};
    use crate::lexer::tokenize;

    const LISTENS: &str = "fn a(dir: &ControlDir) {\n    dir.listen(Socket::Agent);\n}\n";
    const STARTS: &str = "fn start_agent_channel() {\n    channel::start(run, send);\n}\n";

    /// The allowed place, using one, the one start of the channel, and `others`.
    fn files(others: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut files = vec![
            (format!("{}mod.rs", ALLOWED[0]), LISTENS.to_owned()),
            (STARTS_THE_CHANNEL.to_owned(), STARTS.to_owned()),
        ];
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
        let forms = "ControlDir::listen(&d, s);\nlet f = ControlDir::listen;\nd.remove_stale();\nd.write_private_file(n, b);\n";
        assert_eq!(uses(&tokenize(forms)), [1, 2, 3, 4]);
        let other = "fn listen() {}\n// dir.listen(x)\nlet s = \"a.listen(b)\";\nFile::open(p);\n";
        assert_eq!(uses(&tokenize(other)), Vec::<usize>::new());
    }

    #[test]
    fn finds_the_name_however_it_is_reached() {
        let forms = "use oso_platform::control::ControlDir as D;\ntype T = ControlDir;\n<ControlDir>::open();\nuse oso_platform::control::*;\n";
        assert_eq!(names(&tokenize(forms)), [1, 2, 3, 4]);
        let other = "use oso_platform::control::Socket;\nlet a = 2 * 3;\n// ControlDir\n";
        assert_eq!(names(&tokenize(other)), Vec::<usize>::new());
    }

    #[test]
    fn a_use_outside_the_allowed_places_is_a_problem() {
        assert_eq!(problems(&files(&[])), Vec::<String>::new());
        let found = problems(&files(&[("crates/oso/src/app.rs", LISTENS)]));
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found[0].starts_with("crates/oso/src/app.rs:1:"),
            "{found:?}"
        );
        assert!(
            found[1].starts_with("crates/oso/src/app.rs:2:"),
            "{found:?}"
        );
        let aliased = "use oso_platform::control::ControlDir as D;\nfn a() { D::open(); }\n";
        let found = problems(&files(&[("crates/oso/src/app.rs", aliased)]));
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn the_harnesses_may_name_it_to_connect_but_not_listen() {
        let at = format!("{}e2e/harness.rs", CLIENTS[0]);
        let connects = "fn c() { ControlDir::open_existing().connect(s, None); }\n";
        assert_eq!(problems(&files(&[(&at, connects)])), Vec::<String>::new());
        assert_eq!(problems(&files(&[(&at, LISTENS)])).len(), 1);
    }

    #[test]
    fn the_transport_may_use_them_in_its_tests_only() {
        let at = format!("{TRANSPORT}unix.rs");
        let tested = "pub struct ControlDir;\npub fn listen() {}\n#[cfg(test)]\nmod tests {\n    fn t() { d.listen(x); }\n}\n";
        assert_eq!(problems(&files(&[(&at, tested)])), Vec::<String>::new());
        let wrapped = "pub fn listen() {}\npub fn wrap(d: &D) { d.listen(x); }\n";
        let found = problems(&files(&[(&at, wrapped)]));
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn only_the_app_starts_the_channel_and_only_once() {
        let elsewhere = problems(&files(&[("crates/oso/src/commands/x.rs", STARTS)]));
        assert_eq!(elsewhere.len(), 1, "{elsewhere:?}");
        assert!(
            elsewhere[0].contains("only crates/oso/src/app/agent/admit.rs"),
            "{elsewhere:?}"
        );
        let twice = format!("{STARTS}{STARTS}");
        let mut doubled = files(&[]);
        doubled[1].1 = twice;
        assert!(problems(&doubled).iter().any(|p| p.contains("not 2 times")));
    }

    #[test]
    fn a_walk_that_misses_the_allowed_places_is_a_problem() {
        let found = problems(&[("crates/oso/src/app.rs".to_owned(), String::new())]);
        assert_eq!(
            found[0],
            "the walk saw no use in crates/oso/src/dev/, crates/oso/src/channel/"
        );
    }
}
