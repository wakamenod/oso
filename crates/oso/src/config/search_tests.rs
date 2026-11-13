//! The tests of `[search]` in [`parse`](super::parse): `migemo-dictionary`'s path, and what
//! is reported of a bad one (ADR 0116).
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use super::tests::{Fixture, messages};

#[test]
fn a_migemo_dictionary_is_a_path_absolute_or_from_home() {
    let f = Fixture::new();
    let long = format!("/{}", "d".repeat(4096));
    let absolute = if cfg!(windows) {
        r"C:\\dict"
    } else {
        "/usr/share/dict"
    };
    for (path, said) in [
        ("~/dict", None),
        (absolute, None),
        (
            "dict",
            Some("takes an absolute path, or one starting with ~/"),
        ),
        (
            "~dict",
            Some("takes an absolute path, or one starting with ~/"),
        ),
        (long.as_str(), Some("takes a path of at most 4096 bytes")),
        ("~/a\\u0000b", Some("takes a path with no NUL character")),
    ] {
        let loaded = f.parse(&format!("[search]\nmigemo-dictionary = \"{path}\"\n"));
        let settings = &loaded.settings.search;
        match said {
            None => {
                assert_eq!(messages(&loaded), Vec::<String>::new(), "{path}");
                assert_eq!(settings.migemo_dictionary.as_deref(), Some(path));
            }
            Some(said) => {
                let expected = format!("2:21: [search] migemo-dictionary {said}");
                assert_eq!(messages(&loaded), [expected], "{path}");
                assert_eq!(settings.migemo_dictionary, None);
            }
        }
    }
    let loaded = f.parse("[search]\nmigemo-dictionary = 1\n");
    assert_eq!(
        messages(&loaded),
        ["2:21: [search] migemo-dictionary takes a file's path, not a number"]
    );
}
