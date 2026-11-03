//! The layer rules in `guardrails/layers.txt` and the module edges they
//! constrain.
//!
//! A rules section lists layers, highest first, one per line. An entry may
//! use only entries on lower lines; entries on the same line may not use
//! each other. The same check serves the crate graph (`[workspace]`) and the
//! modules of each crate (`[<crate>]`).

use std::collections::{BTreeMap, BTreeSet};

use crate::source::{Tok, Token, skip_group};

/// The crate root (`lib.rs` or `main.rs`) as a node of the module graph.
pub const ROOT: &str = "crate";

pub type Edges = BTreeSet<(String, String)>;

/// Each section's layers, highest first.
#[derive(Debug, Default)]
pub struct Rules {
    sections: BTreeMap<String, Vec<Vec<String>>>,
}

impl Rules {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut rules = Self::default();
        let mut current: Option<String> = None;
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                if rules.sections.insert(name.to_owned(), Vec::new()).is_some() {
                    return Err(format!("line {}: [{name}] appears twice", n + 1));
                }
                current = Some(name.to_owned());
                continue;
            }
            let Some(section) = &current else {
                return Err(format!("line {}: a layer outside any [section]", n + 1));
            };
            let layer = line.split_whitespace().map(str::to_owned).collect();
            rules
                .sections
                .entry(section.clone())
                .or_default()
                .push(layer);
        }
        Ok(rules)
    }

    pub fn section(&self, name: &str) -> Option<&[Vec<String>]> {
        self.sections.get(name).map(Vec::as_slice)
    }
}

/// Every problem with `edges` between `nodes` under `layers`: unlisted
/// nodes, stale rules, cycles and edges that do not point down. With
/// `implicit_top`, that node may be left out and then sits above every
/// layer.
pub fn check(
    section: &str,
    layers: &[Vec<String>],
    nodes: &BTreeSet<String>,
    edges: &Edges,
    implicit_top: Option<&str>,
) -> Vec<String> {
    let mut level: BTreeMap<&str, usize> = BTreeMap::new();
    let mut problems = Vec::new();
    for (i, layer) in layers.iter().enumerate() {
        for name in layer {
            if level.insert(name, i + 1).is_some() {
                problems.push(format!("[{section}]: `{name}` is listed twice"));
            }
            if !nodes.contains(name) {
                problems.push(format!("[{section}]: `{name}` matches nothing; remove it"));
            }
        }
    }
    if let Some(top) = implicit_top {
        level.entry(top).or_insert(0);
    }
    for node in nodes {
        if !level.contains_key(node.as_str()) {
            problems.push(format!(
                "[{section}]: `{node}` is not in guardrails/layers.txt; place it in a layer"
            ));
        }
    }
    problems.extend(
        find_cycle(edges)
            .map(|cycle| format!("[{section}]: dependency cycle {}", cycle.join(" -> "))),
    );
    for (from, to) in edges {
        if let (Some(&a), Some(&b)) = (level.get(from.as_str()), level.get(to.as_str()))
            && a >= b
        {
            problems.push(format!(
                "[{section}]: `{from}` must not use `{to}` (layer {a} may use only layers below it, \
                 and `{to}` is on layer {b})"
            ));
        }
    }
    problems
}

/// One cycle in `edges`, as a path that ends where it starts.
pub fn find_cycle(edges: &Edges) -> Option<Vec<String>> {
    let mut next: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (from, to) in edges {
        next.entry(from).or_default().push(to);
    }
    let mut done = BTreeSet::new();
    for start in next.keys() {
        let mut path = Vec::new();
        if let Some(cycle) = visit(start, &next, &mut path, &mut done) {
            return Some(cycle);
        }
    }
    None
}

fn visit<'a>(
    node: &'a str,
    next: &BTreeMap<&'a str, Vec<&'a str>>,
    path: &mut Vec<&'a str>,
    done: &mut BTreeSet<&'a str>,
) -> Option<Vec<String>> {
    if let Some(i) = path.iter().position(|&n| n == node) {
        let mut cycle: Vec<String> = path[i..].iter().map(|&n| n.to_owned()).collect();
        cycle.push(node.to_owned());
        return Some(cycle);
    }
    if !done.insert(node) {
        return None;
    }
    path.push(node);
    for &to in next.get(node).into_iter().flatten() {
        if let Some(cycle) = visit(to, next, path, done) {
            return Some(cycle);
        }
    }
    path.pop();
    None
}

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
    let mut walker = Walker::default();
    let mut edges = Edges::new();
    for i in 0..tokens.len() {
        walker.step(tokens, i);
        let current: Vec<&str> = file
            .path
            .iter()
            .map(String::as_str)
            .chain(walker.inline.iter().map(|(name, _)| name.as_str()))
            .collect();
        for to in path_targets(tokens, i, &current, modules, walker.reexport) {
            if to != from {
                edges.insert((from.clone(), to));
            }
        }
    }
    edges
}

/// Tracks inline modules and `pub use` statements while scanning a file.
#[derive(Default)]
struct Walker {
    depth: usize,
    /// Inline modules (`mod name { … }`) around the position, with the
    /// depth inside their braces.
    inline: Vec<(String, usize)>,
    /// Inside a `pub use … ;`.
    reexport: bool,
}

impl Walker {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{Edges, FileModule, ROOT, Rules, check, declared_modules, file_edges, find_cycle};
    use crate::source::tokenize;

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
    fn rules_parse_sections_and_layers() {
        let rules = Rules::parse("# c\n[a]\nx y # z\nw\n\n[b]\nv\n").unwrap();
        assert_eq!(rules.section("a").unwrap(), [vec!["x", "y"], vec!["w"]]);
        assert_eq!(rules.section("b").unwrap(), [vec!["v"]]);
        assert!(Rules::parse("x\n").is_err());
        assert!(Rules::parse("[a]\n[a]\n").is_err());
    }

    #[test]
    fn check_accepts_downward_edges_only() {
        let layers = Rules::parse("[s]\ntop\nmid other\nlow\n").unwrap();
        let layers = layers.section("s").unwrap();
        let nodes = set(&["top", "mid", "other", "low"]);
        let ok = edges(&[("top", "mid"), ("top", "low"), ("mid", "low")]);
        assert!(check("s", layers, &nodes, &ok, None).is_empty());
        for bad in [("low", "mid"), ("mid", "other"), ("mid", "top")] {
            let problems = check("s", layers, &nodes, &edges(&[bad]), None);
            assert_eq!(problems.len(), 1, "{bad:?}: {problems:?}");
        }
    }

    #[test]
    fn check_reports_unlisted_and_stale_entries() {
        let layers = Rules::parse("[s]\na gone\n").unwrap();
        let problems = check(
            "s",
            layers.section("s").unwrap(),
            &set(&["a", "new"]),
            &Edges::new(),
            None,
        );
        assert_eq!(problems.len(), 2, "{problems:?}");
        let implicit = check("s", &[], &set(&[ROOT]), &Edges::new(), Some(ROOT));
        assert!(implicit.is_empty(), "{implicit:?}");
    }

    #[test]
    fn cycles_are_found() {
        assert_eq!(find_cycle(&edges(&[("a", "b"), ("b", "c")])), None);
        let cycle = find_cycle(&edges(&[("a", "b"), ("b", "c"), ("c", "a")])).unwrap();
        assert_eq!(cycle, ["a", "b", "c", "a"]);
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
