//! Where foreign code may be reached (ADR 0120): `extern` blocks only in `oso-platform`,
//! which binds the operating system, and in `oso-term`'s `ffi` with its mouse part, which
//! binds libghostty-vt; in `oso-term`, `unsafe` only in that `ffi` and the safe layer over
//! it, `raw` with its callbacks and its mouse part; no `safe` item in an `extern` block,
//! which would be callable without `unsafe`; and only `oso-term` with a build script, a
//! `links` key, or the directives that link a native library. `deny.toml` cannot see the
//! library, as it is no crate, so this check confines it, as `manifests` confines
//! tree-sitter.
//!
//! Must not depend on: the file system or the other checks; it sees only tokens and
//! manifest text.

use crate::lexer::{Token, tokenize};

/// The files outside `oso-platform` that may declare `extern` blocks.
pub const EXTERN_ALLOWED: [&str; 2] = [
    "crates/oso-term/src/engine/ghostty/ffi.rs",
    "crates/oso-term/src/engine/ghostty/ffi/mouse.rs",
];
/// The files of `oso-term` that may hold `unsafe`, and silence the crate's ban on it.
pub const UNSAFE_ALLOWED: [&str; 5] = [
    "crates/oso-term/src/engine/ghostty/ffi.rs",
    "crates/oso-term/src/engine/ghostty/ffi/mouse.rs",
    "crates/oso-term/src/engine/ghostty/raw.rs",
    "crates/oso-term/src/engine/ghostty/raw/callbacks.rs",
    "crates/oso-term/src/engine/ghostty/raw/mouse.rs",
];
/// The one crate whose manifest may name a native library to link, or a build script.
pub const LINKS_ALLOWED: &str = "crates/oso-term/Cargo.toml";
/// The one build script, which links libghostty-vt.
pub const BUILD_SCRIPT: &str = "crates/oso-term/build.rs";

/// What is wrong with `files`, given as paths and sources.
pub fn problems(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for allowed in EXTERN_ALLOWED.iter().chain(&UNSAFE_ALLOWED) {
        if !files.iter().any(|(at, _)| at == allowed) {
            problems.push(format!("the walk did not see {allowed}"));
        }
    }
    for (at, src) in files {
        let tokens = tokenize(src);
        let platform = at.starts_with("crates/oso-platform/");
        if !platform && !EXTERN_ALLOWED.contains(&at.as_str()) {
            for line in extern_block_lines(&tokens) {
                problems.push(format!(
                    "{at}:{line}: an `extern` block outside oso-platform and oso-term's ffi (ADR 0120)"
                ));
            }
        }
        for line in safe_foreign_lines(&tokens) {
            problems.push(format!(
                "{at}:{line}: a `safe` foreign item can be called without `unsafe` (ADR 0120)"
            ));
        }
        let build_script = at.ends_with("/build.rs") && !at.contains("/src/");
        if build_script && at != BUILD_SCRIPT {
            problems.push(format!("{at}: only oso-term has a build script (ADR 0120)"));
        }
        if at != BUILD_SCRIPT && links_natively(src) {
            problems.push(format!(
                "{at}: only oso-term's build script links a native library (ADR 0120)"
            ));
        }
        if at.starts_with("crates/oso-term/") && !UNSAFE_ALLOWED.contains(&at.as_str()) {
            for line in unsafe_lines(&tokens) {
                problems.push(format!(
                    "{at}:{line}: `unsafe` in oso-term outside engine/ghostty's ffi and raw (ADR 0120)"
                ));
            }
        }
    }
    problems
}

/// What is wrong with the workspace's manifests, given as paths and texts.
pub fn manifest_problems(manifests: &[(String, String)]) -> Vec<String> {
    manifests
        .iter()
        .filter(|(at, _)| at != LINKS_ALLOWED)
        .flat_map(|(at, text)| {
            text.lines()
                .enumerate()
                .filter(|(_, line)| {
                    let key = line.trim_start().trim_start_matches("package.");
                    let key = key.trim_start_matches('"');
                    key.starts_with("links") || key.starts_with("build ") || key.starts_with("build=")
                })
                .map(move |(i, _)| {
                    format!(
                        "{at}:{}: only oso-term links a native library or has a build script (ADR 0120)",
                        i + 1
                    )
                })
        })
        .collect()
}

/// The lines of `extern` blocks: `extern` followed by `{`, the ABI's string not being a token.
fn extern_block_lines(tokens: &[Token]) -> Vec<usize> {
    tokens
        .windows(2)
        .filter(|pair| pair[0].is_ident("extern") && pair[1].is_punct('{'))
        .map(|pair| pair[0].line)
        .collect()
}

/// The lines of `safe` items inside `extern` blocks.
fn safe_foreign_lines(tokens: &[Token]) -> Vec<usize> {
    let mut lines = Vec::new();
    let mut depth = 0usize;
    let mut inside: Option<usize> = None;
    for (i, token) in tokens.iter().enumerate() {
        if token.is_punct('{') {
            depth += 1;
            if i > 0 && tokens[i - 1].is_ident("extern") && inside.is_none() {
                inside = Some(depth);
            }
        } else if token.is_punct('}') {
            if inside == Some(depth) {
                inside = None;
            }
            depth = depth.saturating_sub(1);
        } else if inside.is_some() && token.is_ident("safe") {
            lines.push(token.line);
        }
    }
    lines
}

/// Whether `src` holds a cargo directive that links a native library. The words are built
/// here, so that this file does not hold them.
fn links_natively(src: &str) -> bool {
    let directives = [
        ["rustc-link", "-lib"].concat(),
        ["rustc-link", "-arg"].concat(),
    ];
    directives
        .iter()
        .any(|directive| src.contains(directive.as_str()))
}

/// The lines naming `unsafe`, or the lint that bans it, which a file could silence.
fn unsafe_lines(tokens: &[Token]) -> Vec<usize> {
    tokens
        .iter()
        .enumerate()
        .filter(|&(i, token)| {
            token.is_ident("unsafe")
                || (token.is_ident("unsafe_code")
                    && !tokens
                        .get(i.wrapping_sub(2))
                        .is_some_and(|before| before.is_ident("deny")))
        })
        .map(|(_, token)| token.line)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut all: Vec<(String, String)> = EXTERN_ALLOWED
            .iter()
            .chain(&UNSAFE_ALLOWED)
            .map(|at| ((*at).to_owned(), String::new()))
            .collect();
        all.extend(list.iter().map(|(a, s)| ((*a).to_owned(), (*s).to_owned())));
        all
    }

    #[test]
    fn extern_blocks_are_only_allowed_in_platform_and_the_ffi() {
        let src = "unsafe extern \"C\" {\n    fn f();\n}\nextern \"C\" fn g() {}\n";
        assert!(problems(&files(&[("crates/oso-platform/src/a.rs", src)])).is_empty());
        let found = problems(&files(&[("crates/oso-core/src/a.rs", src)]));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("a.rs:1:"));
    }

    #[test]
    fn unsafe_in_oso_term_is_only_allowed_in_ffi_and_raw() {
        let src =
            "#![deny(unsafe_code)]\nfn a() { unsafe { b() } }\n#[allow(unsafe_code)]\nfn c() {}\n";
        let found = problems(&files(&[("crates/oso-term/src/terminal.rs", src)]));
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains(":2:") && found[1].contains(":3:"));
        let comment = "// unsafe is documented here\nconst A: &str = \"unsafe\";\n";
        assert!(problems(&files(&[("crates/oso-term/src/lib.rs", comment)])).is_empty());
        let raw = files(&[]);
        assert!(problems(&raw).is_empty());
    }

    #[test]
    fn safe_foreign_items_are_rejected() {
        let src = "unsafe extern \"C\" {\n    pub safe fn f();\n    fn g();\n}\nfn safe() {}\n";
        let found = problems(&files(&[("crates/oso-platform/src/a.rs", src)]));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("a.rs:2:"));
    }

    #[test]
    fn only_oso_term_has_a_build_script_that_links() {
        let link = ["cargo:rustc-link", "-lib=dylib=z"].concat();
        let found = problems(&files(&[
            ("crates/oso-core/build.rs", "fn main() {}"),
            ("crates/oso/src/a.rs", link.as_str()),
            (BUILD_SCRIPT, link.as_str()),
            ("crates/oso/src/build.rs", "fn a() {}"),
        ]));
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("crates/oso-core/build.rs"));
        assert!(found[1].contains("crates/oso/src/a.rs"));
    }

    #[test]
    fn a_links_key_is_only_allowed_in_oso_terms_manifest() {
        let manifest = "[package]\nname = \"x\"\nlinks = \"z\"\nbuild = \"b.rs\"\n";
        let found = manifest_problems(&[
            ("crates/oso-core/Cargo.toml".to_owned(), manifest.to_owned()),
            (LINKS_ALLOWED.to_owned(), manifest.to_owned()),
        ]);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].starts_with("crates/oso-core/Cargo.toml:3:"));
        assert!(found[1].starts_with("crates/oso-core/Cargo.toml:4:"));
    }
}
