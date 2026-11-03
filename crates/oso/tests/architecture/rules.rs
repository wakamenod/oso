//! The layer rules in `guardrails/layers.txt`, and the check of a
//! dependency graph against them.
//!
//! A rules section lists layers, highest first, one per line. An entry may
//! use only entries on lower lines; entries on the same line may not use
//! each other. The same check serves the crate graph (`[workspace]`) and the
//! modules of each crate (`[<crate>]`).

use std::collections::{BTreeMap, BTreeSet};

use crate::workspace::data_lines;

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
        for (n, line) in data_lines(text) {
            let line = line.trim();
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
pub fn layer_problems(
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
        let (Some(&a), Some(&b)) = (level.get(from.as_str()), level.get(to.as_str())) else {
            problems.push(format!(
                "[{section}]: `{from}` uses `{to}`, and one of them is not a known node"
            ));
            continue;
        };
        if a >= b {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{Edges, ROOT, Rules, find_cycle, layer_problems};

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|&n| n.to_owned()).collect()
    }

    fn edges(pairs: &[(&str, &str)]) -> Edges {
        pairs
            .iter()
            .map(|&(a, b)| (a.to_owned(), b.to_owned()))
            .collect()
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
    fn only_downward_edges_pass() {
        let layers = Rules::parse("[s]\ntop\nmid other\nlow\n").unwrap();
        let layers = layers.section("s").unwrap();
        let nodes = set(&["top", "mid", "other", "low"]);
        let ok = edges(&[("top", "mid"), ("top", "low"), ("mid", "low")]);
        assert!(layer_problems("s", layers, &nodes, &ok, None).is_empty());
        for bad in [("low", "mid"), ("mid", "other"), ("mid", "top")] {
            let problems = layer_problems("s", layers, &nodes, &edges(&[bad]), None);
            assert_eq!(problems.len(), 1, "{bad:?}: {problems:?}");
        }
    }

    #[test]
    fn unlisted_and_stale_entries_are_problems() {
        let layers = Rules::parse("[s]\na gone\n").unwrap();
        let problems = layer_problems(
            "s",
            layers.section("s").unwrap(),
            &set(&["a", "new"]),
            &Edges::new(),
            None,
        );
        assert_eq!(problems.len(), 2, "{problems:?}");
        let implicit = layer_problems("s", &[], &set(&[ROOT]), &Edges::new(), Some(ROOT));
        assert!(implicit.is_empty(), "{implicit:?}");
    }

    #[test]
    fn cycles_are_found() {
        assert_eq!(find_cycle(&edges(&[("a", "b"), ("b", "c")])), None);
        let cycle = find_cycle(&edges(&[("a", "b"), ("b", "c"), ("c", "a")])).unwrap();
        assert_eq!(cycle, ["a", "b", "c", "a"]);
    }

    #[test]
    fn edges_to_unknown_nodes_are_problems() {
        let rules = Rules::parse("[s]\na\n").unwrap();
        let unknown = edges(&[("a", "elsewhere")]);
        let problems = layer_problems(
            "s",
            rules.section("s").unwrap(),
            &set(&["a"]),
            &unknown,
            None,
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
    }
}
