//! Architecture guardrails (ADR 0038, ADR 0039): the dependency direction
//! between crates and between modules, external crates confined to their
//! owner, the file-size ratchet, the ban on `static` items, the ban on
//! spawning outside one module, where foreign code may be reached, where the control
//! directory may be used, the module
//! headers and conflict markers in any file. The rules live in `guardrails/`; the format is described at the top of
//! each file there.
//!
//! Must not depend on: any crate outside `std`, spawned processes or the
//! compiler. The checks read the repository's files as text only.
//!
//! Each module holds one check or one input, with the unit tests that show
//! it catches a violation; this file holds the checks run against the
//! workspace.

mod agent_tools;
mod ffi;
mod headers;
mod lexer;
mod listening;
mod manifests;
mod markers;
mod modules;
mod report;
mod rules;
mod size;
mod spawning;
mod statics;
mod workspace;

use std::fs;
use std::path::Path;

use lexer::tokenize;
use rules::{ROOT, Rules};
use workspace::{Workspace, read, rust_files};

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
fn there_are_no_statics() {
    let workspace = Workspace::current();
    let mut problems = Vec::new();
    for path in workspace.rust_files() {
        let at = workspace.relative_path(&path);
        for line in statics::forbidden_lines(&tokenize(&read(&path))) {
            problems.push(format!(
                "{at}:{line}: a `static` is global state (ADR 0038, ADR 0042); keep state \
                 host-owned, and use a `const` for data"
            ));
        }
    }
    assert_none(&problems);
}

#[test]
fn only_the_allowed_files_may_silence_the_ban_on_spawning() {
    let workspace = Workspace::current();
    let files: Vec<(String, String)> = workspace
        .rust_files()
        .into_iter()
        .map(|path| (workspace.relative_path(&path), read(&path)))
        .collect();
    let problems = spawning::problems(&files);
    assert_none(&problems);
}

#[test]
fn session_tools_never_type_spawn_or_read_tokens() {
    let workspace = Workspace::current();
    let files: Vec<(String, String)> = workspace
        .rust_files()
        .into_iter()
        .map(|path| (workspace.relative_path(&path), read(&path)))
        .collect();
    assert_none(&agent_tools::problems(&files));
}

#[test]
fn foreign_code_is_reached_only_where_allowed() {
    let workspace = Workspace::current();
    let files: Vec<(String, String)> = workspace
        .rust_files()
        .into_iter()
        .map(|path| (workspace.relative_path(&path), read(&path)))
        .collect();
    assert_none(&ffi::problems(&files));
    let manifests: Vec<(String, String)> = workspace
        .crates()
        .iter()
        .map(|c| {
            let path = c.dir.join("Cargo.toml");
            (workspace.relative_path(&path), read(&path))
        })
        .collect();
    assert_none(&ffi::manifest_problems(&manifests));
}

#[test]
fn the_control_directory_is_used_only_where_allowed() {
    let workspace = Workspace::current();
    let files: Vec<(String, String)> = workspace
        .rust_files()
        .into_iter()
        .map(|path| (workspace.relative_path(&path), read(&path)))
        .collect();
    assert_none(&listening::problems(&files));
}

#[test]
fn every_file_states_its_responsibility_and_boundary() {
    let workspace = Workspace::current();
    let rules = rules(&workspace);
    let crates = rules
        .section("workspace")
        .expect("[workspace] in guardrails/layers.txt");
    let mut checked = 0;
    let mut problems = Vec::new();
    for krate in workspace.crates() {
        let name = krate.manifest.name.as_str();
        let src = krate.dir.join("src");
        for path in rust_files(&krate.dir) {
            let at = workspace.relative_path(&path);
            // tests/ and examples/ have no module layers, only their crate's.
            let module = path.strip_prefix(&src).ok().map(headers::top_module);
            let mut scopes = vec![(crates, name)];
            if let (Some(module), Some(layers)) = (&module, rules.section(name)) {
                scopes.push((layers, module));
            }
            problems.extend(headers::problems(&at, &read(&path), &scopes));
            checked += 1;
        }
    }
    assert!(
        checked >= 20,
        "the header check sees too few files: {checked}"
    );
    assert_none(&problems);
}

#[test]
fn no_file_holds_a_conflict_marker() {
    assert_none(&markers::problems(&Workspace::current().repository_texts()));
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
