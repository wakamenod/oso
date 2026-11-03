//! Architecture guardrails (ADR 0038, ADR 0039): the dependency direction
//! between crates and between modules, external crates confined to their
//! owner, the file-size ratchet and the ban on `static mut`. The rules live
//! in `guardrails/`; the format is described at the top of each file there.
//!
//! The checks read source text and manifests, so they need no dependency
//! and spawn no process. Each module holds one check or one input, with the
//! unit tests that show it catches a violation; this file holds the checks
//! run against the workspace.

mod lexer;
mod manifests;
mod modules;
mod report;
mod rules;
mod size;
mod statics;
mod workspace;

use std::fs;
use std::path::Path;

use lexer::tokenize;
use rules::{ROOT, Rules};
use workspace::{Workspace, read};

fn rules(workspace: &Workspace) -> Rules {
    let text = read(&workspace.path("guardrails/layers.txt"));
    Rules::parse(&text).unwrap_or_else(|e| panic!("guardrails/layers.txt: {e}"))
}

fn assert_none(problems: &[String]) {
    assert!(problems.is_empty(), "\n{}\n", problems.join("\n"));
}

#[test]
fn crates_depend_only_downward() {
    let workspace = Workspace::current();
    let rules = rules(&workspace);
    let layers = rules
        .section("workspace")
        .expect("[workspace] in guardrails/layers.txt");
    let crates = workspace.crates();
    let manifests: Vec<_> = crates.iter().map(|c| &c.manifest).collect();
    let (names, edges) = manifests::workspace_edges(&manifests);
    assert!(
        names.len() >= 4,
        "expected the four crates, found {names:?}"
    );
    assert_none(&rules::layer_problems(
        "workspace",
        layers,
        &names,
        &edges,
        None,
    ));
}

#[test]
fn confined_crates_stay_in_their_owner() {
    let crates = Workspace::current().crates();
    assert_none(&manifests::ownership_problems(
        crates.iter().map(|c| &c.manifest),
    ));
}

#[test]
fn modules_depend_only_downward() {
    let workspace = Workspace::current();
    let rules = rules(&workspace);
    let mut problems = Vec::new();
    for krate in workspace.crates() {
        let name = &krate.manifest.name;
        let Some(layers) = rules.section(name) else {
            problems.push(format!("guardrails/layers.txt has no [{name}] section"));
            continue;
        };
        let graph = modules::crate_modules(&krate.dir.join("src"));
        problems.extend(rules::layer_problems(
            name,
            layers,
            &graph.nodes,
            &graph.edges,
            Some(ROOT),
        ));
    }
    assert_none(&problems);
}

#[test]
fn files_stay_within_the_size_limit() {
    let workspace = Workspace::current();
    let allowed = size::parse_allowlist(&read(&workspace.path("guardrails/size-allowlist.txt")))
        .unwrap_or_else(|e| panic!("guardrails/size-allowlist.txt: {e}"));
    let sizes = size::file_sizes(&workspace);
    assert!(
        sizes.len() >= 20,
        "the size check sees too few files: {sizes:?}"
    );
    assert_none(&size::problems(&sizes, &allowed));
}

#[test]
fn there_is_no_static_mut() {
    let workspace = Workspace::current();
    let problems: Vec<String> = workspace
        .rust_files()
        .iter()
        .flat_map(|path| {
            let at = workspace.relative_path(path);
            statics::static_mut_lines(&tokenize(&read(path))).into_iter().map(move |line| {
                format!("{at}:{line}: `static mut` is global state (ADR 0038); keep state host-owned")
            })
        })
        .collect();
    assert_none(&problems);
}

/// Writes the design health report to `$OSO_HEALTH_REPORT`, compared with
/// the checkout at `$OSO_HEALTH_BASE` if set. Run by `just health-report`.
#[test]
#[ignore = "writes a report; run with `just health-report`"]
fn design_health_report() {
    let out =
        std::env::var_os("OSO_HEALTH_REPORT").expect("set OSO_HEALTH_REPORT to the output path");
    let base = std::env::var_os("OSO_HEALTH_BASE")
        .filter(|dir| !dir.is_empty())
        .map(|dir| Workspace::at(Path::new(&dir)));
    let report = report::health_report(&Workspace::current(), base.as_ref());
    fs::write(&out, report).expect("write the report");
}
