//! The URI rules of both systems, each refusal with a test; round trips; and a fuzz target
//! that no URI reads as a relative, dotted, remote or device path.
//!
//! Must not depend on: the file system; paths here are only text.
//!
//! The fuzz target runs in `just ci` with proptest's default cases. For a long run:
//! `PROPTEST_CASES=200000 cargo test -p oso-lsp --release uri::tests::fuzz`.

use std::path::Path;

use proptest::prelude::*;

use super::{Parts, encode, from_path, parse, to_path, valid_segment};
use crate::limits::URI_BYTES_MAX;

fn unix(uri: &str) -> Option<Vec<String>> {
    let parts = parse(uri, false)?;
    assert_eq!(parts.drive, None);
    Some(
        parts
            .segments
            .iter()
            .map(|segment| String::from_utf8_lossy(segment).into_owned())
            .collect(),
    )
}

fn windows(uri: &str) -> Option<(char, Vec<String>)> {
    let parts = parse(uri, true)?;
    let segments = parts
        .segments
        .iter()
        .map(|segment| String::from_utf8_lossy(segment).into_owned())
        .collect();
    Some((char::from(parts.drive?), segments))
}

#[test]
fn unix_uris_read_as_paths() {
    assert_eq!(
        unix("file:///home/a/b.rs"),
        Some(vec!["home".into(), "a".into(), "b.rs".into()])
    );
    assert_eq!(unix("FILE://localhost/x"), Some(vec!["x".into()]));
    assert_eq!(unix("file://LocalHost/x/"), Some(vec!["x".into()]));
    assert_eq!(unix("file:///"), Some(vec![]));
    assert_eq!(
        unix("file:///a%20b/%E6%97%A5"),
        Some(vec!["a b".into(), "日".into()])
    );
    assert_eq!(
        unix("file:///a b/日"),
        Some(vec!["a b".into(), "日".into()])
    );
    // `:` is a name's byte on Unix.
    assert_eq!(unix("file:///a:b"), Some(vec!["a:b".into()]));
}

#[test]
fn only_local_file_uris_are_read() {
    for uri in [
        "",
        "file:",
        "file:/a",
        "file:a",
        "http:///a",
        "untitled:Untitled-1",
        "file://server/share/a",
        "file://localhost.evil/a",
        "file://localhost:80/a",
        "file://user@localhost/a",
        "file://%6Cocalhost/a",
        "file://localhost",
        "file:////server/share/a",
    ] {
        assert_eq!(parse(uri, false), None, "{uri}");
        assert_eq!(parse(uri, true), None, "{uri}");
    }
}

#[test]
fn segments_are_names_in_normal_form() {
    for uri in [
        "file:///a/../b",
        "file:///a/./b",
        "file:///a/%2E%2E/b",
        "file:///a/%2e/b",
        "file:///a//b",
        "file:///a/%2Fb",
        "file:///a/%00",
        "file:///a?b",
        "file:///a#b",
        "file:///a%",
        "file:///a%4",
        "file:///a%G0",
        "file:///a\nb",
        "file:///a\u{7f}",
    ] {
        assert_eq!(parse(uri, false), None, "{uri}");
    }
    // A control written as an escape is a byte of a Unix name.
    assert_eq!(unix("file:///a%0Ab"), Some(vec!["a\nb".into()]));
}

#[test]
fn windows_uris_carry_a_drive_letter() {
    let want = Some(('C', vec!["Users".into(), "a.rs".into()]));
    assert_eq!(windows("file:///C:/Users/a.rs"), want);
    assert_eq!(windows("file:///c:/Users/a.rs"), want);
    assert_eq!(windows("file:///c%3A/Users/a.rs"), want);
    assert_eq!(windows("file:///C%3a/Users/a.rs"), want);
    assert_eq!(windows("file://localhost/C:/Users/a.rs"), want);
    assert_eq!(windows("file:///d:"), Some(('D', vec![])));
    assert_eq!(windows("file:///d:/"), Some(('D', vec![])));
    for uri in [
        "file:///Users/a.rs",
        "file:///CC:/a",
        "file:///1:/a",
        "file:///C/a",
        "file:///C:a",
        "file:////server/share",
        "file:///C:/a%5Cb",
        "file:///C:/a\\b",
        "file:///C:/a:stream",
        "file:///C:/a.",
        "file:///C:/a%20",
        "file:///C:/NUL",
        "file:///C:/x/nul.txt",
        "file:///C:/x/Com1.rs",
        "file:///C:/x/LPT%C2%B9",
        "file:///C:/x/CONOUT$",
        "file:///C:/x/%FF",
        "file:///C:/x/a*b",
    ] {
        assert_eq!(parse(uri, true), None, "{uri}");
    }
    // Device-like names that are not devices.
    assert!(windows("file:///C:/x/COM10").is_some());
    assert!(windows("file:///C:/x/console.rs").is_some());
    assert!(windows("file:///C:/x/nullable").is_some());
}

#[test]
fn uris_are_bounded_before_decoding() {
    let head = "file:///";
    let at = format!("{head}{}", "a".repeat(URI_BYTES_MAX - head.len()));
    assert!(parse(&at, false).is_some());
    let past = format!("{at}a");
    assert_eq!(parse(&past, false), None);
    let many = format!("{head}{}", "a/".repeat(URI_BYTES_MAX));
    assert_eq!(parse(&many, false), None);
}

#[cfg(not(windows))]
#[test]
fn a_path_too_long_for_a_uri_has_none() {
    let long = format!("/{}", "a".repeat(URI_BYTES_MAX - 8));
    assert!(from_path(Path::new(&long)).is_some());
    assert_eq!(from_path(Path::new(&format!("{long}a"))), None);
}

#[test]
fn writing_escapes_all_but_unreserved_bytes() {
    let segments = vec![
        b"a b".to_vec(),
        "日".as_bytes().to_vec(),
        b"x-y_z.~%#?".to_vec(),
    ];
    assert_eq!(
        encode(None, &segments),
        "file:///a%20b/%E6%97%A5/x-y_z.~%25%23%3F"
    );
    assert_eq!(encode(Some(b'C'), &[b"a".to_vec()]), "file:///C:/a");
    assert_eq!(encode(Some(b'C'), &[]), "file:///C:/");
    assert_eq!(encode(None, &[]), "file:///");
}

#[cfg(not(windows))]
#[test]
fn unix_paths_convert_both_ways() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    assert_eq!(
        from_path(Path::new("/w/oso/src/lib.rs")).as_deref(),
        Some("file:///w/oso/src/lib.rs")
    );
    assert_eq!(
        to_path("file:///w/oso/src/lib.rs").as_deref(),
        Some(Path::new("/w/oso/src/lib.rs"))
    );
    assert_eq!(from_path(Path::new("/")).as_deref(), Some("file:///"));
    assert_eq!(from_path(Path::new("w/oso")), None);
    assert_eq!(from_path(Path::new("/w/../oso")), None);
    let odd = Path::new(OsStr::from_bytes(b"/w/\xff\n"));
    let uri = from_path(odd);
    assert_eq!(uri.as_deref(), Some("file:///w/%FF%0A"));
    assert_eq!(to_path(&uri.unwrap_or_default()).as_deref(), Some(odd));
}

#[cfg(windows)]
#[test]
fn windows_paths_convert_both_ways() {
    assert_eq!(
        from_path(Path::new(r"c:\w\oso\lib.rs")).as_deref(),
        Some("file:///C:/w/oso/lib.rs")
    );
    assert_eq!(
        from_path(Path::new(r"\\?\C:\w\lib.rs")).as_deref(),
        Some("file:///C:/w/lib.rs")
    );
    assert_eq!(
        to_path("file:///c%3A/w/lib.rs").as_deref(),
        Some(Path::new(r"C:\w\lib.rs"))
    );
    assert_eq!(from_path(Path::new(r"\\server\share\a")), None);
    assert_eq!(from_path(Path::new(r"\\?\UNC\server\share\a")), None);
    assert_eq!(from_path(Path::new(r"\\.\COM1")), None);
    assert_eq!(from_path(Path::new(r"C:a")), None);
    assert_eq!(from_path(Path::new(r"\a")), None);
    assert_eq!(from_path(Path::new(r"C:\a\NUL")), None);
}

/// A segment's bytes: names, odd bytes, and ones the rules refuse.
fn segment() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        "[a-zA-Z0-9._~ -]{1,8}".prop_map(String::into_bytes),
        "\\PC{1,4}".prop_map(String::into_bytes),
        prop::collection::vec(any::<u8>(), 1..6),
        Just(b"..".to_vec()),
        Just(b"NUL".to_vec()),
        Just(b"a:b".to_vec()),
    ]
}

/// The names Windows reads as devices, written out here apart from the module's own rule.
const DEVICES: [&str; 28] = [
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4",
    "COM5", "COM6", "COM7", "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9", "COM¹", "LPT¹",
];

/// Whether what a URI read as is a local, absolute name in normal form, checked apart from
/// the module's rules: on the path's own text, and on Unix through `Path`'s components.
fn plainly_local(parts: &Parts, windows: bool) -> bool {
    let names_ok = parts.segments.iter().all(|segment| {
        !segment.is_empty()
            && segment != b"."
            && segment != b".."
            && !segment.contains(&b'/')
            && !segment.contains(&0)
    });
    if !names_ok {
        return false;
    }
    if windows {
        let Some(letter) = parts.drive else {
            return false;
        };
        let mut text = format!("{}:", char::from(letter));
        for segment in &parts.segments {
            let Ok(name) = std::str::from_utf8(segment) else {
                return false;
            };
            let stem = name
                .split('.')
                .next()
                .unwrap_or_default()
                .trim_end()
                .to_uppercase();
            if name.contains(|c: char| "\\/:*?\"<>|".contains(c) || c.is_control())
                || name.ends_with('.')
                || name.ends_with(' ')
                || DEVICES.contains(&stem.as_str())
                || ["COM²", "COM³", "LPT²", "LPT³"].contains(&stem.as_str())
            {
                return false;
            }
            text.push('\\');
            text.push_str(name);
        }
        return letter.is_ascii_uppercase() && !text.starts_with("\\\\");
    }
    #[cfg(not(windows))]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        use std::path::Component;

        let mut path = std::path::PathBuf::from("/");
        for segment in &parts.segments {
            path.push(OsStr::from_bytes(segment));
        }
        let mut components = path.components();
        if components.next() != Some(Component::RootDir) {
            return false;
        }
        let normal = components
            .filter(|c| matches!(c, Component::Normal(_)))
            .count();
        if normal != parts.segments.len() || path.components().count() != normal + 1 {
            return false;
        }
    }
    parts.drive.is_none()
}

proptest! {
    #[test]
    fn written_uris_read_back(
        segments in prop::collection::vec(segment(), 0..6),
        letter in prop::sample::select(b"aCz".to_vec()),
        windows in any::<bool>(),
    ) {
        prop_assume!(segments.iter().all(|segment| valid_segment(segment, windows)));
        let drive = windows.then_some(letter.to_ascii_uppercase());
        let uri = encode(drive, &segments);
        prop_assert!(uri.is_ascii());
        prop_assert_eq!(parse(&uri, windows), Some(Parts { drive, segments }));
    }

    /// Arbitrary text and URI-shaped text: reading never panics, and what it reads is
    /// always a local path in normal form.
    #[test]
    fn fuzz(
        uri in prop_oneof![
            "\\PC{0,40}",
            "(file|FILE|http)://(localhost|LOCALHOST|server|)/([a-zA-Z0-9.:%/\\\\ ?#]{0,12}/?){0,5}",
            "file:///[cC](:|%3[aA])/([a-zA-Z0-9.%$¹ ]{0,8}/?){0,4}",
            "file:///[cC]:/(x/)?(NUL|nul\\.txt|COM1|lpt¹|CONOUT\\$|aux |x\\.|a:b|a%5Cb|a%2Fb|%2E%2E|ok)",
        ],
        windows in any::<bool>(),
    ) {
        if let Some(parts) = parse(&uri, windows) {
            prop_assert!(plainly_local(&parts, windows), "{uri}");
            prop_assert_eq!(parse(&encode(parts.drive, &parts.segments), windows), Some(parts));
        }
        let _ = to_path(&uri);
    }
}
