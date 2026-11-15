//! The tests of `[spaces]` in [`parse`](super::parse): `root`, its default, and what is said of a
//! bad one (ADR 0136).
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use std::path::{Path, PathBuf};

use super::paths::expand;
use super::spaces::DEFAULT_ROOT;
use super::tests::{Fixture, messages};

#[test]
fn root_is_absolute_or_under_home_and_defaults_to_spaces() {
    let f = Fixture::new();
    let loaded = f.parse("[spaces]\n");
    assert_eq!(loaded.settings.spaces.root, None);
    assert_eq!(loaded.settings.spaces.root(), DEFAULT_ROOT);
    let absolute = if cfg!(windows) {
        "C:/work/spaces"
    } else {
        "/work/spaces"
    };
    for written in ["~/work/spaces", absolute] {
        let loaded = f.parse(&format!("[spaces]\nroot = \"{written}\"\n"));
        assert_eq!(messages(&loaded), Vec::<String>::new());
        assert_eq!(loaded.settings.spaces.root(), written);
    }
}

#[test]
fn a_bad_root_is_said_and_the_default_stays() {
    let f = Fixture::new();
    let long = format!("/{}", "a".repeat(1024));
    for (value, said) in [
        (
            "\"spaces\"",
            "takes an absolute path, or one starting with ~/",
        ),
        ("\"//host/spaces\"", "takes a path on this computer"),
        (
            &format!("\"{long}\"") as &str,
            "takes a path of at most 1024 bytes",
        ),
        ("3", "takes a directory's path, not a number"),
    ] {
        let loaded = f.parse(&format!("[spaces]\nroot = {value}\n"));
        let messages = messages(&loaded);
        assert_eq!(messages.len(), 1, "{value}");
        assert!(messages[0].contains(said), "{}", messages[0]);
        assert_eq!(loaded.settings.spaces.root(), DEFAULT_ROOT);
    }
    let loaded = f.parse("[spaces]\nplace = 1\n");
    assert_eq!(messages(&loaded).len(), 1);
}

#[test]
fn a_root_under_home_needs_a_home() {
    let home = Path::new("/h");
    assert_eq!(
        expand("~/spaces", Some(home)),
        Some(PathBuf::from("/h/spaces"))
    );
    assert_eq!(expand("~/spaces", None), None);
    assert_eq!(expand("/w/s", None), Some(PathBuf::from("/w/s")));
}
