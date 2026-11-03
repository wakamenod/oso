//! Crate manifests: the edges between workspace crates, and which crate may
//! declare each external crate that is confined to one owner.

use std::collections::BTreeSet;

/// A crate's name and every crate it declares as a dependency, of any kind
/// (normal, dev, build, target-specific).
#[derive(Debug)]
pub struct Manifest {
    pub name: String,
    pub dependencies: BTreeSet<String>,
}

/// For a table header, whether it is a dependency table: `None` if not,
/// `Some(None)` for a table of dependencies and `Some(Some(name))` for a
/// table of one dependency (`[dependencies.name]`).
fn dependency_table(header: &str) -> Option<Option<String>> {
    const KINDS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let header = header.trim();
    if KINDS
        .iter()
        .any(|kind| header == *kind || header.ends_with(&format!(".{kind}")))
    {
        return Some(None);
    }
    KINDS.iter().find_map(|kind| {
        let (_, name) = header.split_once(&format!("{kind}."))?;
        Some(Some(name.trim_matches(['"', '\'']).to_owned()))
    })
}

/// The dependency a `key = value` line in a dependency table declares,
/// following `package = "…"` renames.
fn dependency_name(line: &str) -> Option<String> {
    let (key, value) = line.split_once('=')?;
    let key = key.split('.').next()?.trim().trim_matches(['"', '\'']);
    let renamed = value.split_once("package").and_then(|(_, rest)| {
        let rest = rest.trim_start().strip_prefix('=')?.trim_start();
        Some(rest.strip_prefix('"')?.split('"').next()?.to_owned())
    });
    renamed.or_else(|| (!key.is_empty()).then(|| key.to_owned()))
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut name = None;
        let mut dependencies = BTreeSet::new();
        let mut table = String::new();
        let mut in_dependencies = false;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or_default().trim();
            if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                header.clone_into(&mut table);
                in_dependencies = match dependency_table(header) {
                    Some(Some(dependency)) => {
                        dependencies.insert(dependency);
                        false
                    }
                    Some(None) => true,
                    None => false,
                };
            } else if in_dependencies {
                dependencies.extend(dependency_name(line));
            } else if table == "package"
                && let Some(value) = line
                    .strip_prefix("name")
                    .and_then(|l| l.trim_start().strip_prefix('='))
            {
                name = Some(value.trim().trim_matches('"').to_owned());
            }
        }
        let name = name.ok_or("no [package] name")?;
        Ok(Self { name, dependencies })
    }
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
pub fn ownership_problems(manifests: &[Manifest]) -> Vec<String> {
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
    use super::{Manifest, owner, ownership_problems};

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
        let manifest = Manifest::parse(MANIFEST).unwrap();
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
        let manifest = Manifest::parse(MANIFEST).unwrap();
        let problems = ownership_problems(&[manifest]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("libc"), "{problems:?}");
    }
}
