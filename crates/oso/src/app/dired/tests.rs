//! Tests of the directory editor's glue that need no window.
//!
//! Must not depend on: winit.

use std::path::{Path, PathBuf};

use super::lexically_clean;

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
