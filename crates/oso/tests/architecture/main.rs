//! Architecture guardrails (ADR 0038): the dependency direction between
//! crates and between modules, external crates confined to their owner,
//! the file-size ratchet and the ban on `static mut`. The rules live in
//! `guardrails/`; the format is described at the top of each file there.
//!
//! The checks read source text and manifests, so they need no dependency
//! and spawn no process.

mod crates;
mod layers;
mod size;
mod source;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use layers::{Edges, FileModule, ROOT, Rules};
use source::{Token, tokenize};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every directory under `crates/`, sorted.
fn crate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(workspace_root().join("crates"))
        .expect("read crates/")
        .map(|entry| entry.expect("read crates/ entry").path())
        .filter(|path| path.join("Cargo.toml").is_file())
        .collect();
    dirs.sort();
    dirs
}

/// Every `.rs` file under `dir`, sorted.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("read directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// `path` relative to the workspace root, with `/` separators.
fn display(path: &Path) -> String {
    let root = workspace_root().canonicalize().expect("workspace root");
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let relative = path.strip_prefix(&root).unwrap_or(&path);
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn rules() -> Rules {
    let path = workspace_root().join("guardrails/layers.txt");
    Rules::parse(&read(&path)).unwrap_or_else(|e| panic!("guardrails/layers.txt: {e}"))
}

fn manifests() -> Vec<crates::Manifest> {
    crate_dirs()
        .iter()
        .map(|dir| {
            let path = dir.join("Cargo.toml");
            crates::Manifest::parse(&read(&path))
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        })
        .collect()
}

fn crate_edges(manifests: &[crates::Manifest]) -> (BTreeSet<String>, Edges) {
    let names: BTreeSet<String> = manifests.iter().map(|m| m.name.clone()).collect();
    let edges = manifests
        .iter()
        .flat_map(|m| {
            m.dependencies
                .iter()
                .filter(|d| names.contains(*d))
                .map(|d| (m.name.clone(), d.clone()))
        })
        .collect();
    (names, edges)
}

/// One crate's modules and the edges between them.
struct CrateModules {
    name: String,
    nodes: BTreeSet<String>,
    edges: Edges,
}

fn crate_modules(dir: &Path, name: &str) -> CrateModules {
    let src = dir.join("src");
    let root = ["lib.rs", "main.rs"]
        .iter()
        .map(|f| src.join(f))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{}: no lib.rs or main.rs", src.display()));
    let modules = layers::declared_modules(&tokenize(&read(&root)));
    let mut edges = Edges::new();
    for file in rust_files(&src) {
        let tokens = tokenize(&read(&file));
        let path = module_path(&src, &file, &root);
        let file_module = FileModule {
            path,
            tokens: &tokens,
        };
        edges.extend(layers::file_edges(&file_module, &modules));
    }
    let mut nodes = modules;
    nodes.insert(ROOT.to_owned());
    CrateModules {
        name: name.to_owned(),
        nodes,
        edges,
    }
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

fn all_crate_modules() -> Vec<CrateModules> {
    crate_dirs()
        .iter()
        .zip(manifests())
        .map(|(dir, manifest)| crate_modules(dir, &manifest.name))
        .collect()
}

/// The size of every checked file: all Rust code under `crates/` outside
/// `tests/` directories.
fn file_sizes() -> BTreeMap<String, usize> {
    rust_files(&workspace_root().join("crates"))
        .into_iter()
        .filter(|path| !path.components().any(|c| c.as_os_str() == "tests"))
        .map(|path| {
            let src = read(&path);
            let tokens = tokenize(&src);
            (display(&path), size::non_test_lines(&src, &tokens))
        })
        .collect()
}

/// `static mut` items, as `path:line`.
fn static_muts(path: &Path, tokens: &[Token]) -> Vec<String> {
    tokens
        .windows(2)
        .filter(|pair| pair[0].is_ident("static") && pair[1].is_ident("mut"))
        .map(|pair| format!("{}:{}", display(path), pair[0].line))
        .collect()
}

fn assert_none(problems: &[String]) {
    assert!(problems.is_empty(), "\n{}\n", problems.join("\n"));
}

#[test]
fn crates_depend_only_downward() {
    let rules = rules();
    let layers = rules
        .section("workspace")
        .expect("[workspace] in guardrails/layers.txt");
    let (names, edges) = crate_edges(&manifests());
    assert!(
        names.len() >= 4,
        "expected the four crates, found {names:?}"
    );
    assert_none(&layers::check("workspace", layers, &names, &edges, None));
}

#[test]
fn confined_crates_stay_in_their_owner() {
    assert_none(&crates::ownership_problems(&manifests()));
}

#[test]
fn modules_depend_only_downward() {
    let rules = rules();
    let mut problems = Vec::new();
    for krate in all_crate_modules() {
        let Some(layers) = rules.section(&krate.name) else {
            problems.push(format!(
                "guardrails/layers.txt has no [{}] section",
                krate.name
            ));
            continue;
        };
        problems.extend(layers::check(
            &krate.name,
            layers,
            &krate.nodes,
            &krate.edges,
            Some(ROOT),
        ));
    }
    assert_none(&problems);
}

#[test]
fn files_stay_within_the_size_limit() {
    let path = workspace_root().join("guardrails/size-allowlist.txt");
    let allowed = size::parse_allowlist(&read(&path))
        .unwrap_or_else(|e| panic!("guardrails/size-allowlist.txt: {e}"));
    assert_none(&size::problems(&file_sizes(), &allowed));
}

#[test]
fn there_is_no_static_mut() {
    let problems: Vec<String> = rust_files(&workspace_root().join("crates"))
        .iter()
        .flat_map(|path| static_muts(path, &tokenize(&read(path))))
        .map(|at| format!("{at}: `static mut` is global state (ADR 0038); keep state host-owned"))
        .collect();
    assert_none(&problems);
}

#[test]
fn the_static_mut_check_detects_one() {
    let found = static_muts(
        Path::new("x.rs"),
        &tokenize("// static mut A\nstatic mut B: u8 = 0;\n"),
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].ends_with(":2"), "{found:?}");
}

/// Writes the design health report (ADR 0038) to `$OSO_HEALTH_REPORT` as
/// Markdown. Run by `just health-report`.
#[test]
#[ignore = "writes a report; run with `just health-report`"]
fn design_health_report() {
    let out =
        std::env::var_os("OSO_HEALTH_REPORT").expect("set OSO_HEALTH_REPORT to the output path");
    let mut report = String::from("## Largest files (lines of non-test code)\n\n");
    let mut sizes: Vec<(String, usize)> = file_sizes().into_iter().collect();
    sizes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    report.push_str(&format!(
        "Limit: {}.\n\n| Lines | File |\n|---:|---|\n",
        size::MAX_LINES
    ));
    for (path, lines) in sizes.iter().take(15) {
        report.push_str(&format!("| {lines} | `{path}` |\n"));
    }
    report.push_str("\n## Dependency edges\n\n");
    let (_, edges) = crate_edges(&manifests());
    let crate_list: Vec<String> = edges.iter().map(|(a, b)| format!("{a} → {b}")).collect();
    report.push_str(&format!("- crates: {}\n", crate_list.join(", ")));
    for krate in all_crate_modules() {
        let list: Vec<String> = krate
            .edges
            .iter()
            .map(|(a, b)| format!("{a} → {b}"))
            .collect();
        report.push_str(&format!(
            "- {} ({} edges): {}\n",
            krate.name,
            list.len(),
            list.join(", ")
        ));
    }
    fs::write(&out, report).expect("write the report");
}
