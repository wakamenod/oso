//! The design health report (ADR 0038), as Markdown: the largest files and
//! how they grew, and the dependency edges added and removed, compared with
//! an older checkout. `just health-report` adds the sections that come from
//! git and clippy.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::manifests::workspace_edges;
use crate::modules::crate_modules;
use crate::rules::Edges;
use crate::size::{MAX_LINES, file_sizes};
use crate::workspace::Workspace;

/// Every dependency edge, crate-level as `crates: a → b` and module-level
/// as `<crate>: a → b`.
fn all_edges(workspace: &Workspace) -> Vec<String> {
    let crates = workspace.crates();
    let manifests: Vec<_> = crates.iter().map(|c| &c.manifest).collect();
    let (_, crate_edges) = workspace_edges(&manifests);
    let mut edges = labelled("crates", &crate_edges);
    for krate in &crates {
        let modules = crate_modules(&krate.dir.join("src"));
        edges.extend(labelled(&krate.manifest.name, &modules.edges));
    }
    edges
}

fn labelled(label: &str, edges: &Edges) -> Vec<String> {
    edges
        .iter()
        .map(|(a, b)| format!("{label}: {a} → {b}"))
        .collect()
}

fn growth(now: usize, before: Option<usize>) -> String {
    match before {
        None => "new".to_owned(),
        Some(before) if now >= before => format!("+{}", now - before),
        Some(before) => format!("−{}", before - now),
    }
}

/// The report for `workspace`, compared with `base` when there is one.
pub fn health_report(workspace: &Workspace, base: Option<&Workspace>) -> String {
    let mut out = String::new();
    let before: BTreeMap<String, usize> = base.map(file_sizes).unwrap_or_default();
    let mut sizes: Vec<(String, usize)> = file_sizes(workspace).into_iter().collect();
    sizes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let _ = writeln!(out, "## Largest files (lines of non-test code)\n");
    let _ = writeln!(
        out,
        "Limit: {MAX_LINES}.\n\n| Lines | Change | File |\n|---:|---:|---|"
    );
    for (path, lines) in sizes.iter().take(15) {
        let change = if base.is_some() {
            growth(*lines, before.get(path).copied())
        } else {
            String::new()
        };
        let _ = writeln!(out, "| {lines} | {change} | `{path}` |");
    }

    let now = all_edges(workspace);
    let _ = writeln!(out, "\n## Dependency edges\n");
    if let Some(base) = base {
        let then = all_edges(base);
        let added: Vec<_> = now.iter().filter(|e| !then.contains(e)).collect();
        let removed: Vec<_> = then.iter().filter(|e| !now.contains(e)).collect();
        let _ = writeln!(out, "Added: {}.\n", list(&added));
        let _ = writeln!(out, "Removed: {}.\n", list(&removed));
    }
    let _ = writeln!(out, "<details><summary>All {} edges</summary>\n", now.len());
    for edge in &now {
        let _ = writeln!(out, "- {edge}");
    }
    let _ = writeln!(out, "\n</details>");
    out
}

fn list(edges: &[&String]) -> String {
    if edges.is_empty() {
        "none".to_owned()
    } else {
        edges
            .iter()
            .map(|e| e.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::growth;

    #[test]
    fn growth_is_signed_or_new() {
        assert_eq!(growth(10, Some(7)), "+3");
        assert_eq!(growth(7, Some(10)), "−3");
        assert_eq!(growth(5, None), "new");
    }
}
