//! Crate manifests: the edges between workspace crates, and which crate may
//! declare each external crate that is confined to one owner.
//!
//! Must not depend on: the lexer or the layer order; it reports edges, and
//! `rules` judges them. Manifests are read line by line.
//!
//! The parser is line-based and approximate. It reads every dependency
//! table form and rename that Cargo accepts on one line, and fails on a
//! rename it cannot read. Continuation lines of multi-line arrays are
//! ignored.

use std::collections::{BTreeMap, BTreeSet};

use crate::rules::Edges;
use crate::workspace::data_lines;

/// A crate's name and every crate it declares as a dependency, of any kind
/// (normal, dev, build, target-specific), under its real package name.
#[derive(Debug)]
pub struct Manifest {
    pub name: String,
    pub dependencies: BTreeSet<String>,
}

/// One declared dependency, by the key it is declared under.
#[derive(Debug, Default)]
struct Entry {
    /// The real package, when the key is a rename (`package = "…"`).
    package: Option<String>,
    /// Declared with `workspace = true`, so a rename may come from the root.
    workspace: bool,
}

/// What the lines after a table header declare.
enum Table {
    /// `[dependencies]` and its dev, build, target and workspace variants.
    Dependencies,
    /// `[dependencies.<key>]`: one dependency.
    One(String),
    Package,
    Other,
}

impl Table {
    fn of(header: &str) -> Self {
        const KINDS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
        let header = header.trim();
        if header == "package" {
            return Self::Package;
        }
        if KINDS
            .iter()
            .any(|kind| header == *kind || header.ends_with(&format!(".{kind}")))
        {
            return Self::Dependencies;
        }
        KINDS
            .iter()
            .find_map(|kind| {
                let (_, key) = header.split_once(&format!("{kind}."))?;
                Some(Self::One(unquote(key)))
            })
            .unwrap_or(Self::Other)
    }
}

fn unquote(key: &str) -> String {
    key.trim().trim_matches(['"', '\'']).to_owned()
}

/// The string literal at the start of `value`, in either quote style.
fn string_literal(value: &str) -> Option<String> {
    let value = value.trim_start();
    let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    Some(value[1..].split(quote).next()?.to_owned())
}

/// The value of `key = "…"` inside an inline table, if `key` is set there.
/// A `key` that is set but cannot be read is an error, so that an
/// unreadable rename fails the check instead of hiding a dependency.
fn inline_string(table: &str, key: &str) -> Result<Option<String>, String> {
    for (at, _) in table.match_indices(key) {
        let before = table[..at].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            continue;
        }
        if let Some(rest) = table[at + key.len()..].trim_start().strip_prefix('=') {
            return string_literal(rest)
                .map(Some)
                .ok_or_else(|| format!("cannot read `{key}` in `{table}`"));
        }
    }
    Ok(None)
}

fn is_true(value: &str) -> bool {
    value.trim() == "true"
}

/// Records `key = value` from a table of dependencies.
fn declare(entries: &mut BTreeMap<String, Entry>, key: &str, value: &str) -> Result<(), String> {
    let (alias, field) = match key.split_once('.') {
        Some((alias, field)) => (unquote(alias), Some(unquote(field))),
        None => (unquote(key), None),
    };
    let entry = entries.entry(alias).or_default();
    match field.as_deref() {
        None => {
            entry.package = inline_string(value, "package")?;
            entry.workspace = value.replace(' ', "").contains("workspace=true");
        }
        Some("package") => {
            entry.package = Some(
                string_literal(value).ok_or_else(|| format!("cannot read `{key} = {value}`"))?,
            );
        }
        Some("workspace") => entry.workspace = is_true(value),
        Some(_) => {}
    }
    Ok(())
}

/// Records `field = value` inside a `[dependencies.<alias>]` table.
fn declare_field(entry: &mut Entry, field: &str, value: &str) -> Result<(), String> {
    match field {
        "package" => {
            entry.package = Some(
                string_literal(value).ok_or_else(|| format!("cannot read `package = {value}`"))?,
            );
        }
        "workspace" => entry.workspace = is_true(value),
        _ => {}
    }
    Ok(())
}

/// The package name and the dependency entries of a manifest.
fn parse_entries(text: &str) -> Result<(Option<String>, BTreeMap<String, Entry>), String> {
    let mut name = None;
    let mut entries = BTreeMap::new();
    let mut table = Table::Other;
    for (_, line) in data_lines(text) {
        let line = line.trim();
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            table = Table::of(header);
            if let Table::One(alias) = &table {
                entries.entry(alias.clone()).or_default();
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        match &table {
            Table::Dependencies => declare(&mut entries, key, value)?,
            Table::One(alias) => {
                declare_field(entries.entry(alias.clone()).or_default(), key, value)?;
            }
            Table::Package if key == "name" => name = string_literal(value),
            Table::Package | Table::Other => {}
        }
    }
    Ok((name, entries))
}

/// The renames in the root manifest's `[workspace.dependencies]`, from key
/// to package, which `<key>.workspace = true` inherits.
pub fn workspace_renames(root: &str) -> Result<BTreeMap<String, String>, String> {
    let (_, entries) = parse_entries(root)?;
    Ok(entries
        .into_iter()
        .filter_map(|(alias, entry)| Some((alias, entry.package?)))
        .collect())
}

impl Manifest {
    pub fn parse(text: &str, workspace_renames: &BTreeMap<String, String>) -> Result<Self, String> {
        let (name, entries) = parse_entries(text)?;
        let name = name.ok_or("no [package] name")?;
        let dependencies = entries
            .into_iter()
            .map(|(alias, entry)| {
                let inherited = entry
                    .workspace
                    .then(|| workspace_renames.get(&alias).cloned())
                    .flatten();
                entry.package.or(inherited).unwrap_or(alias)
            })
            .collect();
        Ok(Self { name, dependencies })
    }
}

/// The workspace crates and the edges between them.
pub fn workspace_edges(manifests: &[&Manifest]) -> (BTreeSet<String>, Edges) {
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

/// Bindings to the operating system, which only `oso-platform` may declare
/// (ADR 0003, ADR 0038). Matched by prefix, so whole families are covered.
const OS_BINDING_PREFIXES: [&str; 13] = [
    "objc",
    "block2",
    "dispatch2",
    "core-foundation",
    "core-graphics",
    "core-text",
    "cocoa",
    "windows",
    "winapi",
    "wayland",
    "x11",
    "xcb",
    "ndk",
];
const OS_BINDINGS: [&str; 3] = ["libc", "nix", "rustix"];

/// The workspace crate that alone may declare `dependency`, if any.
pub fn owner(dependency: &str) -> Option<&'static str> {
    if dependency == "wgpu" || dependency.starts_with("wgpu-") {
        Some("oso-render")
    } else if dependency == "winit" {
        Some("oso")
    } else if OS_BINDINGS.contains(&dependency)
        || OS_BINDING_PREFIXES
            .iter()
            .any(|p| dependency.starts_with(p))
    {
        Some("oso-platform")
    } else {
        None
    }
}

/// Every dependency declared outside its owning crate.
pub fn ownership_problems<'a>(manifests: impl IntoIterator<Item = &'a Manifest>) -> Vec<String> {
    let mut problems = Vec::new();
    for manifest in manifests {
        for dependency in &manifest.dependencies {
            if let Some(owner) = owner(dependency)
                && owner != manifest.name
            {
                problems.push(format!(
                    "{} depends on {dependency}, which only {owner} may use (ADR 0038); \
                     go through {owner}'s API instead",
                    manifest.name
                ));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{Manifest, owner, ownership_problems, workspace_renames};

    const MANIFEST: &str = r#"
[package]
name = "oso-x"
version.workspace = true

[dependencies]
oso-core.workspace = true
log = "0.4"
renamed = { package = "oso-platform", path = "../p" }

[target.'cfg(unix)'.dependencies]
libc = { workspace = true, optional = true }

[dev-dependencies]
proptest.workspace = true

[build-dependencies.cc]
version = "1"

[features]
dev = ["dep:libc"]
"#;

    #[test]
    fn manifests_list_every_kind_of_dependency() {
        let manifest = Manifest::parse(MANIFEST, &BTreeMap::new()).unwrap();
        assert_eq!(manifest.name, "oso-x");
        let found: Vec<&str> = manifest.dependencies.iter().map(String::as_str).collect();
        assert_eq!(
            found,
            ["cc", "libc", "log", "oso-core", "oso-platform", "proptest"]
        );
    }

    #[test]
    fn owned_crates_outside_their_owner_are_problems() {
        assert_eq!(owner("objc2-core-text"), Some("oso-platform"));
        assert_eq!(owner("windows-sys"), Some("oso-platform"));
        assert_eq!(owner("wgpu-hal"), Some("oso-render"));
        assert_eq!(owner("log"), None);
        let manifest = Manifest::parse(MANIFEST, &BTreeMap::new()).unwrap();
        let problems = ownership_problems(&[manifest]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("libc"), "{problems:?}");
    }

    #[test]
    fn every_form_of_rename_is_resolved() {
        let root = "[workspace.dependencies]\nroot-alias = { package = \"windows-sys\", version = \"0.59\" }\n";
        let renames = workspace_renames(root).unwrap();
        let manifest = "[package]\nname = \"oso-x\"\n\
                        [dependencies]\n\
                        quoted = { package = 'libc', version = \"0.2\" }\n\
                        dotted.package = \"objc2\"\n\
                        dotted.version = \"0.6\"\n\
                        root-alias.workspace = true\n\
                        feature-word = { version = \"1\", features = [\"package-x\"] }\n\
                        [dependencies.table-alias]\n\
                        package = \"core-foundation\"\n\
                        version = \"0.10\"\n";
        let found: Vec<String> = Manifest::parse(manifest, &renames)
            .unwrap()
            .dependencies
            .into_iter()
            .collect();
        assert_eq!(
            found,
            [
                "core-foundation",
                "feature-word",
                "libc",
                "objc2",
                "windows-sys"
            ]
        );
    }

    #[test]
    fn an_unreadable_rename_is_an_error() {
        let manifest = "[package]\nname = \"x\"\n[dependencies]\nsys = { package = libc }\n";
        assert!(Manifest::parse(manifest, &BTreeMap::new()).is_err());
    }
}
