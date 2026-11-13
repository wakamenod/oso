//! The tests of `[search]` in [`parse`](super::parse): `migemo-dictionary`'s path, and what
//! is reported of a bad one (ADR 0116).
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use super::settings::MAX_DICTIONARY_PATH;
use super::tests::{Fixture, messages};

#[test]
fn a_migemo_dictionary_is_a_path_absolute_or_from_home() {
    let f = Fixture::new();
    // No escape in it, so the TOML string is the path.
    let absolute = if cfg!(windows) {
        "C:/dict"
    } else {
        "/usr/share/dict"
    };
    // Absolute on every system, so only its length is wrong.
    let long = format!("{absolute}/{}", "d".repeat(MAX_DICTIONARY_PATH));
    let elsewhere = r"takes a path on this computer, not one starting with \\ or //";
    let rooted = "takes ~/ and a relative path, not ~/ and one from a root";
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
        ("//server/share/dict", Some(elsewhere)),
        (r"\\\\server\\share\\dict", Some(elsewhere)),
        // Joined to the home directory, a rooted path would replace it.
        ("~///server/share/dict", Some(rooted)),
        ("~//usr/share/dict", Some(rooted)),
        (r"~/\\\\server\\share\\dict", Some(rooted)),
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
    if cfg!(windows) {
        let loaded = f.parse("[search]\nmigemo-dictionary = \"~/C:/dict\"\n");
        let expected = format!("2:21: [search] migemo-dictionary {rooted}");
        assert_eq!(messages(&loaded), [expected]);
    }
    let loaded = f.parse("[search]\nmigemo-dictionary = 1\n");
    assert_eq!(
        messages(&loaded),
        ["2:21: [search] migemo-dictionary takes a file's path, not a number"]
    );
}
