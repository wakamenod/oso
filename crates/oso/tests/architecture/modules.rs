//! The module graph of a crate: which top-level modules each file uses,
//! found from `crate::` and `super::` paths and, in the crate root, bare
//! paths to child modules.
//!
//! Known gaps (ADR 0039): uses inside macros, `#[path]` attributes, and
//! bare paths in a module that glob-imports the root (`use super::*;` in a
//! top-level file).

use std::collections::BTreeSet;
use std::path::Path;

use crate::lexer::{Tok, Token, skip_group, tokenize};
use crate::rules::{Edges, ROOT};
use crate::workspace::{read, rust_files};

/// The modules a crate root declares with `mod name;`.
pub fn declared_modules(root: &[Token]) -> BTreeSet<String> {
    let mut modules = BTreeSet::new();
    let mut depth = 0usize;
    for (i, token) in root.iter().enumerate() {
        match token.tok {
            Tok::Punct('{') => depth += 1,
            Tok::Punct('}') => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0
            && token.is_ident("mod")
            && let (Some(Tok::Ident(name)), Some(semi)) =
                (root.get(i + 1).map(|t| &t.tok), root.get(i + 2))
            && semi.is_punct(';')
        {
            modules.insert(name.clone());
        }
    }
    modules
}

/// A source file's place in its crate: the module path of the file
/// (empty for the root).
pub struct FileModule<'a> {
    pub path: Vec<String>,
    pub tokens: &'a [Token],
}

/// What `tokens[i..]` names after `crate::` or a path that resolves to the
/// root: each module it reaches, or [`ROOT`] for root items.
fn root_targets(tokens: &[Token], i: usize, modules: &BTreeSet<String>) -> Vec<String> {
    let classify = |name: &str| {
        if modules.contains(name) {
            name.to_owned()
        } else {
            ROOT.to_owned()
        }
    };
    match tokens.get(i).map(|t| &t.tok) {
        Some(Tok::Ident(name)) => vec![classify(name)],
        Some(Tok::Punct('{')) => {
            let end = skip_group(tokens, i);
            let mut depth = 0usize;
            let mut targets = Vec::new();
            for (k, token) in tokens.iter().enumerate().take(end - 1).skip(i + 1) {
                match &token.tok {
                    Tok::Punct('{') => depth += 1,
                    Tok::Punct('}') => depth -= 1,
                    Tok::Ident(name) if depth == 0 && name != "self" => {
                        let first = tokens[k - 1].is_punct('{') || tokens[k - 1].is_punct(',');
                        if first {
                            targets.push(classify(name));
                        }
                    }
                    _ => {}
                }
            }
            targets
        }
        _ => vec![ROOT.to_owned()],
    }
}

/// The top-level module edges in one file, as (from, to).
pub fn file_edges(file: &FileModule<'_>, modules: &BTreeSet<String>) -> Edges {
    let from = file.path.first().map_or(ROOT, String::as_str).to_owned();
    let tokens = file.tokens;
    let mut scope = ScopeTracker::default();
    let mut edges = Edges::new();
    for i in 0..tokens.len() {
        scope.step(tokens, i);
        let current: Vec<&str> = file
            .path
            .iter()
            .map(String::as_str)
            .chain(scope.inline.iter().map(|(name, _)| name.as_str()))
            .collect();
        for to in path_targets(tokens, i, &current, modules, scope.reexport) {
            if to != from {
                edges.insert((from.clone(), to));
            }
        }
    }
    edges
}

/// Tracks inline modules and `pub use` statements while scanning a file.
#[derive(Default)]
struct ScopeTracker {
    depth: usize,
    /// Inline modules (`mod name { … }`) around the position, with the
    /// depth inside their braces.
    inline: Vec<(String, usize)>,
    /// Inside a `pub use … ;`.
    reexport: bool,
}

impl ScopeTracker {
    fn step(&mut self, tokens: &[Token], i: usize) {
        let token = &tokens[i];
        match &token.tok {
            Tok::Punct('{') => {
                self.depth += 1;
                if i >= 2
                    && tokens[i - 2].is_ident("mod")
                    && let Tok::Ident(name) = &tokens[i - 1].tok
                {
                    self.inline.push((name.clone(), self.depth));
                }
            }
            Tok::Punct('}') => {
                if self.inline.last().is_some_and(|(_, d)| *d == self.depth) {
                    self.inline.pop();
                }
                self.depth = self.depth.saturating_sub(1);
            }
            Tok::Punct(';') => self.reexport = false,
            Tok::Ident(word) if word == "use" => self.reexport = is_after_pub(tokens, i),
            _ => {}
        }
    }
}

/// Whether the keyword at `i` follows `pub` or `pub(…)`.
fn is_after_pub(tokens: &[Token], i: usize) -> bool {
    match i.checked_sub(1).map(|k| &tokens[k]) {
        Some(prev) if prev.is_ident("pub") => true,
        Some(prev) if prev.is_punct(')') => tokens[..i - 1]
            .iter()
            .rposition(|t| t.is_punct('('))
            .is_some_and(|open| open > 0 && tokens[open - 1].is_ident("pub")),
        _ => false,
    }
}

/// The top-level modules (or [`ROOT`]) that a path starting at `i` uses,
/// from inside the module `current`.
fn path_targets(
    tokens: &[Token],
    i: usize,
    current: &[&str],
    modules: &BTreeSet<String>,
    reexport: bool,
) -> Vec<String> {
    let token = &tokens[i];
    let follows = |k: usize, f: &dyn Fn(&Token) -> bool| tokens.get(k).is_some_and(f);
    let path_sep = |t: &Token| t.tok == Tok::PathSep;
    let starts_path = i == 0 || !(tokens[i - 1].tok == Tok::PathSep || tokens[i - 1].is_punct('$'));
    if !starts_path || !follows(i + 1, &path_sep) {
        return Vec::new();
    }
    let in_root = current.is_empty();
    match &token.tok {
        Tok::Ident(word) if word == "crate" => root_targets(tokens, i + 2, modules),
        Tok::Ident(word) if word == "self" && in_root => root_targets(tokens, i + 2, modules),
        Tok::Ident(word) if word == "super" => {
            let mut ups = 1;
            let mut k = i + 2;
            while follows(k, &|t| t.is_ident("super")) && follows(k + 1, &path_sep) {
                ups += 1;
                k += 2;
            }
            match current.len().checked_sub(ups) {
                Some(0) => root_targets(tokens, k, modules),
                Some(_) => vec![current[0].to_owned()],
                None => Vec::new(),
            }
        }
        Tok::Ident(word) if in_root && !reexport && modules.contains(word) => vec![word.clone()],
        _ => Vec::new(),
    }
}

/// One crate's modules and the edges between them.
pub struct CrateModules {
    pub nodes: BTreeSet<String>,
    pub edges: Edges,
}

/// The module graph of the crate whose sources are in `src`.
pub fn crate_modules(src: &Path) -> CrateModules {
    let root = ["lib.rs", "main.rs"]
        .iter()
        .map(|f| src.join(f))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{}: no lib.rs or main.rs", src.display()));
    let modules = declared_modules(&tokenize(&read(&root)));
    let mut edges = Edges::new();
    for file in rust_files(src) {
        let tokens = tokenize(&read(&file));
        let path = module_path(src, &file, &root);
        edges.extend(file_edges(
            &FileModule {
                path,
                tokens: &tokens,
            },
            &modules,
        ));
    }
    let mut nodes = modules;
    nodes.insert(ROOT.to_owned());
    CrateModules { nodes, edges }
}

/// The module path of `file` in the crate whose sources are in `src`.
fn module_path(src: &Path, file: &Path, root: &Path) -> Vec<String> {
    if file == root {
        return Vec::new();
    }
    let relative = file.strip_prefix(src).expect("file under src");
    let mut path: Vec<String> = relative
        .with_extension("")
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if path.last().is_some_and(|last| last == "mod") {
        path.pop();
    }
    path
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{FileModule, declared_modules, file_edges};
    use crate::lexer::tokenize;
    use crate::rules::{Edges, ROOT};

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|&n| n.to_owned()).collect()
    }

    fn edges(pairs: &[(&str, &str)]) -> Edges {
        pairs
            .iter()
            .map(|&(a, b)| (a.to_owned(), b.to_owned()))
            .collect()
    }

    fn edges_of(path: &[&str], src: &str, modules: &[&str]) -> Edges {
        let tokens = tokenize(src);
        let file = FileModule {
            path: path.iter().map(|&p| p.to_owned()).collect(),
            tokens: &tokens,
        };
        file_edges(&file, &set(modules))
    }

    #[test]
    fn declared_modules_skip_inline_ones() {
        let root = tokenize("mod a;\n#[cfg(test)]\nmod tests { mod b; }\npub mod c;\n");
        assert_eq!(declared_modules(&root), set(&["a", "c"]));
    }

    #[test]
    fn crate_paths_and_groups_are_edges() {
        let src = "use crate::{rope::Rope, sum_tree::{Bias, Cursor}, self};\n\
                   fn f() { crate::key::parse(); crate::Item; }\n\
                   // crate::command\n\
                   pub(crate) fn g() {}\n\
                   macro_rules! m { () => { $crate::x } }\n";
        let found = edges_of(
            &["display_map"],
            src,
            &["rope", "sum_tree", "key", "command", "x"],
        );
        let expected = edges(&[
            ("display_map", "rope"),
            ("display_map", "sum_tree"),
            ("display_map", "key"),
            ("display_map", ROOT),
        ]);
        assert_eq!(found, expected);
    }

    #[test]
    fn super_resolves_through_inline_modules() {
        let modules = ["a", "b"];
        // In a top-level file, `super` is the root.
        let top = edges_of(&["a"], "use super::b::X;\nuse super::Item;\n", &modules);
        assert_eq!(top, edges(&[("a", "b"), ("a", ROOT)]));
        // In its inline test module, `super` is the file's module.
        let inline = edges_of(
            &["a"],
            "mod tests { use super::*; use super::super::b; }",
            &modules,
        );
        assert_eq!(inline, edges(&[("a", "b")]));
        // In a nested file, `super` stays inside the top-level module.
        assert!(edges_of(&["a", "inner"], "use super::sibling;", &modules).is_empty());
    }

    #[test]
    fn root_wiring_is_not_an_edge_but_its_code_is() {
        let src = "mod a;\nmod b;\npub use a::Thing;\npub(crate) use b::Other;\n\
                   use b::Used;\nfn main() { a::run(); }\n";
        let found = edges_of(&[], src, &["a", "b"]);
        assert_eq!(found, edges(&[(ROOT, "b"), (ROOT, "a")]));
    }
}
