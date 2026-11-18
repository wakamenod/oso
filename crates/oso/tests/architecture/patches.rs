//! oso's copies of third-party crates in `patches/` (ADR 0157): each `[patch.crates-io]` entry
//! of the workspace's manifest is a directory `patches/NAME`, with a section `## NAME VERSION`
//! in `patches/README.md` and a list `patches/NAME.sha256` of every file in it with its
//! SHA-256, as `shasum -a 256` prints it; and nothing else is there. So a change to a copy's C
//! or build script fails until its list is written again, deliberately, on a reviewed commit.
//!
//! Must not depend on: the file system or the other checks; it sees paths and bytes only.

use std::collections::BTreeSet;

use super::sha256;

/// The manifest's `[patch.crates-io]` entries: each crate's name and the path it is built from.
pub fn patched(manifest: &str) -> Result<Vec<(String, String)>, String> {
    let mut entries = Vec::new();
    let mut inside = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[patch.crates-io]";
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .ok_or_else(|| format!("[patch.crates-io]: cannot read `{line}`"))?;
        let path = value
            .split_once("path")
            .and_then(|(_, rest)| rest.split('"').nth(1))
            .ok_or_else(|| format!("[patch.crates-io]: `{line}` names no path"))?;
        entries.push((name.trim().to_owned(), path.to_owned()));
    }
    Ok(entries)
}

/// The crates `README.md` has a section for: `## NAME VERSION`.
fn sections(readme: &str) -> BTreeSet<String> {
    readme
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .filter_map(|heading| heading.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

/// A list's lines: each file's path and SHA-256, the path after two spaces.
fn listed(list: &str) -> Result<Vec<(String, String)>, String> {
    list.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (hash, path) = line
                .split_once("  ")
                .ok_or_else(|| format!("cannot read `{line}`"))?;
            Ok((path.to_owned(), hash.to_owned()))
        })
        .collect()
}

/// What is wrong in `patches/`, given the workspace's manifest and every file under `patches/`
/// as its path from there and its bytes.
pub fn problems(manifest: &str, files: &[(String, Vec<u8>)]) -> Vec<String> {
    let mut problems = Vec::new();
    let entries = match patched(manifest) {
        Ok(entries) => entries,
        Err(problem) => return vec![format!("Cargo.toml: {problem}")],
    };
    let file = |path: &str| {
        files
            .iter()
            .find(|(at, _)| at == path)
            .map(|(_, bytes)| bytes.as_slice())
    };
    let names: BTreeSet<String> = entries.iter().map(|(name, _)| name.clone()).collect();
    for (name, path) in &entries {
        if *path != format!("patches/{name}") {
            problems.push(format!(
                "[patch.crates-io] builds {name} from {path}, not patches/{name}"
            ));
        }
    }
    let dirs: BTreeSet<String> = files
        .iter()
        .filter_map(|(at, _)| at.split_once('/').map(|(dir, _)| dir.to_owned()))
        .collect();
    for dir in dirs.difference(&names) {
        problems.push(format!(
            "patches/{dir} is no [patch.crates-io] entry of Cargo.toml"
        ));
    }
    let readme = file("README.md").map(String::from_utf8_lossy);
    let sections = readme.as_deref().map(sections).unwrap_or_default();
    if readme.is_none() && !names.is_empty() {
        problems.push("patches/README.md is missing".to_owned());
    }
    for section in sections.difference(&names) {
        problems.push(format!(
            "patches/README.md has a section for {section}, which is not patched"
        ));
    }
    for (at, _) in files.iter().filter(|(at, _)| !at.contains('/')) {
        let known = at == "README.md"
            || at
                .strip_suffix(".sha256")
                .is_some_and(|name| names.contains(name));
        if !known {
            problems.push(format!(
                "patches/{at} is neither the README nor a patched crate's list"
            ));
        }
    }
    for name in &names {
        if !sections.contains(name) {
            problems.push(format!("patches/README.md has no `## {name}` section"));
        }
        if !dirs.contains(name) {
            problems.push(format!("patches/{name} is missing"));
        }
        problems.extend(copy_problems(name, files, file(&format!("{name}.sha256"))));
    }
    problems
}

/// What is wrong with the copy `name`, given every file and its list, if it has one.
fn copy_problems(name: &str, files: &[(String, Vec<u8>)], list: Option<&[u8]>) -> Vec<String> {
    let Some(list) = list else {
        return vec![format!("patches/{name}.sha256 is missing")];
    };
    let listed = match listed(&String::from_utf8_lossy(list)) {
        Ok(listed) => listed,
        Err(problem) => return vec![format!("patches/{name}.sha256: {problem}")],
    };
    let prefix = format!("{name}/");
    let mut problems = Vec::new();
    let copy: Vec<(&str, &[u8])> = files
        .iter()
        .filter_map(|(at, bytes)| Some((at.strip_prefix(&prefix)?, bytes.as_slice())))
        .collect();
    for (path, bytes) in &copy {
        match listed.iter().find(|(at, _)| at == path) {
            None => problems.push(format!(
                "patches/{name}/{path} is not in patches/{name}.sha256"
            )),
            Some((_, hash)) if *hash != sha256::hex(bytes) => problems.push(format!(
                "patches/{name}/{path} changed: its SHA-256 is not the one patches/{name}.sha256 \
                 gives; review the change, then write the list again"
            )),
            Some(_) => {}
        }
    }
    for (path, _) in &listed {
        if !copy.iter().any(|(at, _)| at == path) {
            problems.push(format!(
                "patches/{name}.sha256 lists {path}, which patches/{name} does not hold"
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::{patched, problems, sha256};

    const MANIFEST: &str = "[workspace]\nmembers = [\"crates/*\"]\n\n[patch.crates-io]\n\
                            # oso's copy\nx = { path = \"patches/x\" }\n";

    fn files(extra: &[(&str, &[u8])]) -> Vec<(String, Vec<u8>)> {
        let a = b"int a;\n".as_slice();
        let list = format!("{}  src/a.c\n", sha256::hex(a));
        [
            ("README.md", b"# Patched\n\n## x 1.0.0\n".as_slice()),
            ("x.sha256", list.as_bytes()),
            ("x/src/a.c", a),
        ]
        .iter()
        .chain(extra)
        .map(|(at, bytes)| ((*at).to_owned(), bytes.to_vec()))
        .collect()
    }

    #[test]
    fn the_entries_are_read_from_the_patch_table_only() {
        let manifest = format!("{MANIFEST}\n[profile.dev]\ny = {{ path = \"z\" }}\n");
        assert_eq!(
            patched(&manifest).unwrap(),
            [("x".to_owned(), "patches/x".to_owned())]
        );
    }

    #[test]
    fn a_copy_as_listed_passes() {
        assert_eq!(problems(MANIFEST, &files(&[])), Vec::<String>::new());
    }

    #[test]
    fn a_changed_added_or_removed_file_fails() {
        let mut changed = files(&[]);
        changed[2].1 = b"int b;\n".to_vec();
        assert_eq!(problems(MANIFEST, &changed).len(), 1, "changed");
        let added = files(&[("x/build.rs", b"fn main() {}\n")]);
        assert_eq!(problems(MANIFEST, &added).len(), 1, "added");
        let mut removed = files(&[]);
        removed.pop();
        assert!(!problems(MANIFEST, &removed).is_empty(), "removed");
    }

    #[test]
    fn entries_directories_sections_and_lists_match_one_for_one() {
        let stray = files(&[("y/a.c", b"")]);
        assert_eq!(
            problems(MANIFEST, &stray).len(),
            1,
            "a directory with no entry"
        );
        let mut no_section = files(&[]);
        no_section[0].1 = b"# Patched\n".to_vec();
        assert_eq!(problems(MANIFEST, &no_section).len(), 1, "no section");
        let mut no_list = files(&[]);
        no_list.remove(1);
        assert_eq!(problems(MANIFEST, &no_list).len(), 1, "no list");
        let elsewhere = MANIFEST.replace("patches/x", "vendor/x");
        assert_eq!(
            problems(&elsewhere, &files(&[])).len(),
            1,
            "built from elsewhere"
        );
        let extra = files(&[("notes.txt", b"")]);
        assert_eq!(problems(MANIFEST, &extra).len(), 1, "another file");
        assert_eq!(problems("[workspace]\n", &[]), Vec::<String>::new(), "none");
    }
}
