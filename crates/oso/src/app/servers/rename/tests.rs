//! Tests of a rename's names and paths (ADR 0138, step 14): the identifier at point, and a
//! path as the preview shows it.
//!
//! Must not depend on: the window system or the servers' threads.

use std::path::Path;

use super::snapshot::{NEW_NAME_MAX, identifier_at, shown_path};

#[test]
fn the_identifier_at_point_is_its_letters_digits_and_underscores() {
    let text = "let value_2 = other(x);";
    assert_eq!(identifier_at(text, 4).as_deref(), Some("value_2"));
    assert_eq!(identifier_at(text, 8).as_deref(), Some("value_2"));
    assert_eq!(
        identifier_at(text, 11).as_deref(),
        Some("value_2"),
        "just past it"
    );
    assert_eq!(identifier_at(text, 12), None, "on a blank");
    assert_eq!(identifier_at("名前 = 1", 3).as_deref(), Some("名前"));
    assert_eq!(identifier_at("", 0), None);
    assert_eq!(identifier_at("abc", 99).as_deref(), Some("abc"));
    let long = "a".repeat(NEW_NAME_MAX + 1);
    assert_eq!(identifier_at(&long, 1), None, "longer than a name may be");
}

#[test]
fn a_path_is_shown_from_the_root_with_slashes() {
    let root = Path::new("/p/proj");
    assert_eq!(
        shown_path(Some(Path::new("/p/proj/src/lib.rs")), root, "lib.rs"),
        "src/lib.rs"
    );
    assert_eq!(
        shown_path(Some(Path::new("/elsewhere/a.rs")), root, "a.rs"),
        "/elsewhere/a.rs"
    );
    assert_eq!(shown_path(None, root, "*scratch*"), "*scratch*");
}
