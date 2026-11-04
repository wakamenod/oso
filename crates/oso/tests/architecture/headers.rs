//! Module responsibility headers (ADR 0038, ADR 0041): every Rust file opens
//! with a `//!` paragraph stating its responsibility, then one that starts
//! with [`MARKER`] and states its boundary.
//!
//! Must not depend on: the workspace on disk; callers pass in the source text
//! and the layers that apply to it.
//!
//! The boundary is free text, mostly concepts that `guardrails/layers.txt`
//! cannot express. Only a name in backticks that the layers list is checked,
//! so a named module cannot contradict the rules.

use std::path::Path;

use crate::rules::{ROOT, forbids};

/// What the boundary paragraph starts with.
pub const MARKER: &str = "Must not depend on:";

/// The module a file under `src/` belongs to in the layer rules, from its
/// path relative to `src/`: its top-level module, or [`ROOT`] for the crate
/// root.
pub fn top_module(relative: &Path) -> String {
    if relative == Path::new("lib.rs") || relative == Path::new("main.rs") {
        return ROOT.to_owned();
    }
    let first = Path::new(relative.iter().next().unwrap_or_default());
    let stem = first.file_stem().unwrap_or_default();
    stem.to_string_lossy().into_owned()
}

/// The text of a boundary paragraph, after the marker, or what is wrong with
/// the header of `src`.
fn boundary(src: &str) -> Result<String, &'static str> {
    let doc: Vec<&str> = src
        .lines()
        .map_while(|line| line.strip_prefix("//!"))
        .map(str::trim)
        .collect();
    if doc.is_empty() {
        return Err("does not start with a `//!` header");
    }
    let mut paragraphs = doc.split(|line| line.is_empty()).filter(|p| !p.is_empty());
    let first = paragraphs.next();
    if first.is_none_or(|p| p[0].starts_with(MARKER)) {
        return Err("has no responsibility paragraph before its boundary");
    }
    let Some(text) = paragraphs
        .next()
        .and_then(|p| p.join(" ").strip_prefix(MARKER).map(str::to_owned))
    else {
        return Err("has no boundary as its second paragraph");
    };
    if !text.contains(char::is_alphanumeric) {
        return Err("has an empty boundary");
    }
    if text.matches('`').count() % 2 != 0 || text.contains("``") {
        return Err("has unbalanced backticks in its boundary");
    }
    Ok(text)
}

/// The layer node a backticked span names, in the forms code and docs use:
/// `crate::buffer`, `super::buffer`, `buffer::Item` and `buffer.rs` all name
/// `buffer`.
fn node_name(span: &str) -> &str {
    let mut path = span.trim().trim_start_matches("::");
    while let Some(rest) = ["crate::", "super::", "self::"]
        .iter()
        .find_map(|prefix| path.strip_prefix(prefix))
    {
        path = rest;
    }
    let first = path.split([':', '/']).next().unwrap_or(path);
    first.strip_suffix(".rs").unwrap_or(first)
}

/// Every problem with the header of the file `at`, whose source is `src`.
/// Each scope pairs a set of layers with the file's node in them, and the
/// boundary may name in backticks only what those layers forbid that node.
pub fn problems(at: &str, src: &str, scopes: &[(&[Vec<String>], &str)]) -> Vec<String> {
    let text = match boundary(src) {
        Ok(text) => text,
        Err(problem) => {
            return vec![format!(
                "{at} {problem}: start it with a paragraph on its responsibility, then one \
                 starting `{MARKER}` (ADR 0038, ADR 0041)"
            )];
        }
    };
    let mut problems = Vec::new();
    for span in text.split('`').skip(1).step_by(2) {
        let name = node_name(span);
        // Code spells a crate `oso_core`; the layers call it `oso-core`.
        let crate_name = name.replace('_', "-");
        for &(layers, node) in scopes {
            let verdict = forbids(layers, node, name).or(forbids(layers, node, &crate_name));
            if verdict == Some(false) {
                problems.push(format!(
                    "{at} says it must not depend on `{name}`, but guardrails/layers.txt does \
                     not forbid that for `{node}`; correct the header"
                ));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{problems, top_module};
    use crate::rules::{ROOT, Rules};

    const GOOD: &str = "//! Does a thing.\n//!\n//! Must not depend on: the GPU.\n\nuse a;\n";

    fn no_scopes(src: &str) -> Vec<String> {
        problems("f.rs", src, &[])
    }

    #[test]
    fn a_conforming_header_passes() {
        assert!(no_scopes(GOOD).is_empty());
        let long = "//! Does a\n//! thing.\n//!\n//! Must not depend on: the GPU\n//! or the OS.\n\
                    //!\n//! More.\n";
        assert!(no_scopes(long).is_empty());
        let wrapped = "//! Does a thing.\n//!\n//! Must not depend on:\n//! the GPU.\n";
        assert!(no_scopes(wrapped).is_empty());
    }

    #[test]
    fn a_nonconforming_header_fails_and_names_the_file() {
        for bad in [
            "",
            "use a;\n",
            "// Not a doc comment.\n//! Does a thing.\n",
            "\n//! Does a thing.\n//!\n//! Must not depend on: the GPU.\n",
            "//! Does a thing.\n",
            "//! Does a thing.\n//! Must not depend on: the GPU.\n",
            "//! Does a thing.\n//!\n//! More about it.\n//!\n//! Must not depend on: the GPU.\n",
            "//! Must not depend on: the GPU.\n//!\n//! Does a thing.\n",
            "//! Does a thing.\n//!\n//! Must not depend on:\n",
            "//! Does a thing.\n//!\n//! Must not depend on:   \n//!\n//! More.\n",
            "//! Does a thing.\n//!\n//! Must not depend on: .\n",
            "//! Does a thing.\n//!\n//! Must not depend on: `a or b.\n",
            "//! Does a thing.\n//!\n//! Must not depend on: ``a``.\n",
        ] {
            let found = no_scopes(bad);
            assert_eq!(found.len(), 1, "{bad:?}: {found:?}");
            assert!(found[0].starts_with("f.rs "), "{found:?}");
        }
    }

    #[test]
    fn named_layers_must_really_be_forbidden() {
        let rules = Rules::parse("[s]\ntop\nmid other\nlow\n").unwrap();
        let layers = rules.section("s").unwrap();
        let header =
            |names: &str| format!("//! Does a thing.\n//!\n//! Must not depend on: {names}.");
        let check =
            |names: &str, node: &str| problems("f.rs", &header(names), &[(layers, node)]).len();
        assert_eq!(check("`top`, `other` or `Unlisted`", "mid"), 0);
        assert_eq!(check("`low`", "mid"), 1);
        assert_eq!(check("`mid`", "mid"), 1);
        assert_eq!(check("`mid` or `low`", "top"), 2);
        // A node the layers do not list sits above all of them.
        assert_eq!(check("`low`", ROOT), 1);
    }

    #[test]
    fn every_spelling_of_a_listed_name_is_checked() {
        let rules = Rules::parse("[s]\noso-top\ndisplay_map\nlow\n").unwrap();
        let layers = rules.section("s").unwrap();
        let check = |name: &str, node: &str| {
            let src = format!("//! Does a thing.\n//!\n//! Must not depend on: `{name}`.\n");
            problems("f.rs", &src, &[(layers, node)]).len()
        };
        for allowed in [
            "low",
            " low ",
            "crate::low",
            "super::super::low",
            "self::low",
            "::low",
            "low::Item",
            "low.rs",
            "low/item.rs",
        ] {
            assert_eq!(check(allowed, "display_map"), 1, "{allowed}");
            assert_eq!(check(allowed, "low"), 1, "{allowed}");
        }
        assert_eq!(check("oso_top::Thing", "low"), 0);
        assert_eq!(check("oso_top::Thing", "oso-top"), 1);
        assert_eq!(check("display_map::cell", "low"), 0);
        assert_eq!(check("display_map::cell", "oso-top"), 1);
    }

    #[test]
    fn every_scope_is_checked() {
        let rules = Rules::parse("[a]\nup\ndown\n[b]\nhigh\nlow\n").unwrap();
        let (a, b) = (rules.section("a").unwrap(), rules.section("b").unwrap());
        let src = "//! Does a thing.\n//!\n//! Must not depend on: `up` or `low`.\n";
        assert_eq!(problems("f.rs", src, &[(a, "down"), (b, "low")]).len(), 1);
        assert_eq!(problems("f.rs", src, &[(a, "down"), (b, "high")]).len(), 1);
        assert_eq!(problems("f.rs", src, &[(a, "up"), (b, "high")]).len(), 2);
    }

    #[test]
    fn top_module_finds_the_node_of_a_file() {
        for (path, node) in [
            ("lib.rs", ROOT),
            ("main.rs", ROOT),
            ("rope.rs", "rope"),
            ("display_map/mod.rs", "display_map"),
            ("display_map/cell.rs", "display_map"),
            ("lib/main.rs", "lib"),
        ] {
            assert_eq!(top_module(Path::new(path)), node, "{path}");
        }
    }
}
