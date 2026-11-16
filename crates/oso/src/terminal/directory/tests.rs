//! Tests of reading the directory a terminal's program says it is in (ADR 0141).
//!
//! Must not depend on: the file system.

use std::path::PathBuf;

use super::local;

#[cfg(unix)]
#[test]
fn a_file_uri_on_this_machine_gives_its_path() {
    let host = Some("mac.local");
    let path = |sent| local(sent, host);
    assert_eq!(path("file:///w/oso"), Some(PathBuf::from("/w/oso")));
    assert_eq!(
        path("file://localhost/w/oso"),
        Some(PathBuf::from("/w/oso"))
    );
    assert_eq!(
        path("file://mac.local/w/oso/"),
        Some(PathBuf::from("/w/oso/"))
    );
    assert_eq!(
        path("FILE://MAC/w"),
        Some(PathBuf::from("/w")),
        "case, first label"
    );
    assert_eq!(
        local("file://mac.local/w", Some("mac")),
        Some(PathBuf::from("/w"))
    );
}

#[cfg(unix)]
#[test]
fn escapes_are_decoded_to_utf8_or_the_uri_is_dropped() {
    let path = |sent| local(sent, None);
    assert_eq!(
        path("file:///w/my%20notes"),
        Some(PathBuf::from("/w/my notes"))
    );
    assert_eq!(path("file:///w/%E6%97%A5"), Some(PathBuf::from("/w/日")));
    assert_eq!(path("file:///w/%FF"), None, "not UTF-8");
    assert_eq!(path("file:///w/%0a"), None, "a control character");
    assert_eq!(path("file:///w/%2"), None, "cut short");
    assert_eq!(path("file:///w/%zz"), None);
}

#[cfg(unix)]
#[test]
fn another_machine_a_relative_path_and_dots_are_dropped() {
    let path = |sent| local(sent, Some("mac"));
    assert_eq!(path("file://server/w/oso"), None, "after ssh");
    assert_eq!(path("file://server"), None, "no path");
    assert_eq!(path("file:w/oso"), None);
    assert_eq!(path("file:///w/../etc"), None);
    assert_eq!(path("file:///w/./oso"), None);
    assert_eq!(path("w/oso"), None);
    assert_eq!(path(""), None);
    assert_eq!(local("file://server/w", None), None, "no name to match");
}

#[cfg(unix)]
#[test]
fn a_plain_path_is_taken_as_it_is() {
    // OSC 9;9's and OSC 1337 `CurrentDir`'s are paths, with no escapes.
    assert_eq!(
        local("/w/my%20notes", None),
        Some(PathBuf::from("/w/my%20notes"))
    );
}

#[cfg(windows)]
#[test]
fn a_drive_path_loses_the_slash_before_it() {
    assert_eq!(
        local("file:///C:/work", None),
        Some(PathBuf::from("C:/work"))
    );
}
