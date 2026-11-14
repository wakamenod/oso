//! Tests of the directory editor's glue that need no window.
//!
//! Must not depend on: winit.

use std::path::{Path, PathBuf};

use oso_core::editor::Editor;

use super::{lexically_clean, listing_name};

#[test]
fn a_directory_is_cleaned_by_its_words() {
    let start = Path::new("/start");
    assert_eq!(
        lexically_clean(Path::new("/a/./b/../c/"), start),
        PathBuf::from("/a/c")
    );
    assert_eq!(
        lexically_clean(Path::new("sub"), start),
        PathBuf::from("/start/sub")
    );
    assert_eq!(lexically_clean(Path::new("/.."), start), PathBuf::from("/"));
}

#[test]
fn a_listing_is_named_after_its_directory_and_told_apart() {
    let mut editor = Editor::default();
    assert_eq!(
        listing_name(&editor, Path::new("/src/oso/crates"), None),
        "crates/"
    );
    let first = editor.add_listing("crates/", PathBuf::from("/src/oso/crates"));
    assert_eq!(
        listing_name(&editor, Path::new("/src/oso/crates"), Some(first)),
        "crates/"
    );
    assert_eq!(
        listing_name(&editor, Path::new("/src/zed/crates"), None),
        "crates/<zed>"
    );
    assert_eq!(listing_name(&editor, Path::new("/"), None), "/");
}
