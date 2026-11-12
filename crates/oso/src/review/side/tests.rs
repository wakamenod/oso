//! Reading work tree files: links are never followed, on the way or at the end.
//!
//! Must not depend on: anything that runs git; these read scratch directories.

use std::fs;

use oso_platform::file_meta::Directory;

use super::{Found, Got, find, is_binary, read};
use crate::review::tests::scratch_dir;

#[test]
fn a_file_is_found_under_its_directories_and_read() {
    let dir = scratch_dir("side-file");
    fs::create_dir_all(dir.join("a/b")).unwrap();
    fs::write(dir.join("a/b/c.txt"), b"text\n").unwrap();
    let top = Directory::open(&dir).unwrap();
    let Found::File(file, stamp) = find(&top, b"a/b/c.txt").unwrap() else {
        panic!("no file");
    };
    assert_eq!(
        read(file, stamp.size).unwrap(),
        Got::Bytes(b"text\n".to_vec())
    );
    assert!(matches!(find(&top, b"a/missing").unwrap(), Found::Nothing));
    assert!(
        matches!(find(&top, b"a/b").unwrap(), Found::Nothing),
        "a directory"
    );
    for odd in [&b"a/../a/b/c.txt"[..], b"a//b/c.txt", b"./a/b/c.txt"] {
        assert!(!matches!(find(&top, odd), Ok(Found::File(..))), "{odd:?}");
    }
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn links_are_read_as_their_target_and_never_followed() {
    use std::os::unix::fs::symlink;
    let dir = scratch_dir("side-links");
    let outside = scratch_dir("side-links-outside");
    fs::write(outside.join("secret.txt"), b"not here\n").unwrap();
    fs::create_dir(dir.join("top")).unwrap();
    symlink(&outside, dir.join("top/through")).unwrap();
    symlink("../elsewhere", dir.join("top/link")).unwrap();
    let top = Directory::open(&dir.join("top")).unwrap();
    assert!(matches!(
        find(&top, b"through/secret.txt").unwrap(),
        Found::Nothing
    ));
    let Found::Link(target) = find(&top, b"link").unwrap() else {
        panic!("no link");
    };
    assert_eq!(target, b"../elsewhere");
    fs::remove_dir_all(dir).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn binary_is_a_nul_in_the_first_8000_bytes() {
    assert!(is_binary(b"a\0b"));
    let mut late = vec![b'a'; 8000];
    late.push(0);
    assert!(!is_binary(&late));
}
