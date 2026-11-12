//! Tests of opening and listing a directory relative to itself, following no link, on scratch
//! directories.
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use std::ffi::{CString, OsStr};
use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::PathBuf;

use super::{Directory, EntryKind};

struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!("osod-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fifo(path: &std::path::Path) {
    let name = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: `name` is NUL-terminated, outlives the call and is only read.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
}

fn name(name: &str) -> &OsStr {
    OsStr::new(name)
}

#[test]
fn lists_each_entry_with_its_kind_following_no_link() {
    let scratch = Scratch::new("1");
    fs::write(scratch.0.join("file"), b"text").unwrap();
    fs::create_dir(scratch.0.join("sub")).unwrap();
    symlink(scratch.0.join("sub"), scratch.0.join("link")).unwrap();
    fifo(&scratch.0.join("pipe"));
    let directory = Directory::open(&scratch.0).unwrap();
    // Twice: each listing starts from the start.
    for _ in 0..2 {
        let mut entries: Vec<_> = directory
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.name.into_string().unwrap(), entry.kind)
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            entries,
            [
                ("file".to_owned(), EntryKind::File),
                ("link".to_owned(), EntryKind::Link),
                ("pipe".to_owned(), EntryKind::Other),
                ("sub".to_owned(), EntryKind::Directory),
            ]
        );
    }
    assert_eq!(directory.kind_of(name("link")).unwrap(), EntryKind::Link);
    assert_eq!(
        directory.kind_of(name("sub")).unwrap(),
        EntryKind::Directory
    );
    assert_eq!(
        directory.kind_of(name("missing")).unwrap_err().kind(),
        ErrorKind::NotFound
    );
}

#[test]
fn opens_a_directory_below_by_its_name_and_never_a_link() {
    let scratch = Scratch::new("2");
    fs::create_dir(scratch.0.join("sub")).unwrap();
    fs::write(scratch.0.join("sub/inner"), b"inside").unwrap();
    fs::write(scratch.0.join("file"), b"text").unwrap();
    symlink(scratch.0.join("sub"), scratch.0.join("link")).unwrap();
    let directory = Directory::open(&scratch.0).unwrap();
    let sub = directory.open_directory(name("sub")).unwrap().unwrap();
    let inner: Vec<_> = sub.entries().unwrap().map(|e| e.unwrap().name).collect();
    assert_eq!(inner, [OsStr::new("inner")]);
    assert_eq!(
        sub.metadata().unwrap().ino(),
        fs::metadata(scratch.0.join("sub")).unwrap().ino()
    );
    assert!(directory.open_directory(name("link")).unwrap().is_none());
    assert!(directory.open_directory(name("file")).unwrap().is_none());
    assert_eq!(
        directory
            .open_directory(name("missing"))
            .unwrap_err()
            .kind(),
        ErrorKind::NotFound
    );
}

#[test]
fn a_directory_replaced_by_a_link_after_its_listing_is_not_entered() {
    let scratch = Scratch::new("3");
    let outside = Scratch::new("4");
    fs::write(outside.0.join("elsewhere"), b"not here").unwrap();
    fs::create_dir(scratch.0.join("sub")).unwrap();
    let directory = Directory::open(&scratch.0).unwrap();
    let listed: Vec<_> = directory.entries().unwrap().map(|e| e.unwrap()).collect();
    assert_eq!(listed[0].kind, EntryKind::Directory);
    fs::remove_dir(scratch.0.join("sub")).unwrap();
    symlink(&outside.0, scratch.0.join("sub")).unwrap();
    assert!(directory.open_directory(name("sub")).unwrap().is_none());
}

#[test]
fn what_an_open_directory_holds_stays_its_own_when_its_path_changes() {
    let scratch = Scratch::new("5");
    let outside = Scratch::new("6");
    fs::create_dir(scratch.0.join("sub")).unwrap();
    fs::write(scratch.0.join("sub/file"), b"inside").unwrap();
    fs::write(outside.0.join("file"), b"outside").unwrap();
    let root = Directory::open(&scratch.0).unwrap();
    let sub = root.open_directory(name("sub")).unwrap().unwrap();
    // The directory moves away, and a link to another takes its name.
    fs::rename(scratch.0.join("sub"), scratch.0.join("moved")).unwrap();
    symlink(&outside.0, scratch.0.join("sub")).unwrap();
    let (mut file, _) = sub.open_file(name("file")).unwrap().unwrap();
    let mut read = String::new();
    file.read_to_string(&mut read).unwrap();
    assert_eq!(read, "inside");
}

#[test]
fn opens_only_a_regular_file_without_waiting() {
    let scratch = Scratch::new("7");
    fs::write(scratch.0.join("file"), b"made up").unwrap();
    symlink(scratch.0.join("file"), scratch.0.join("link")).unwrap();
    symlink("/dev/null", scratch.0.join("null")).unwrap();
    fifo(&scratch.0.join("pipe"));
    fs::create_dir(scratch.0.join("sub")).unwrap();
    let directory = Directory::open(&scratch.0).unwrap();
    let (mut file, metadata) = directory.open_file(name("file")).unwrap().unwrap();
    assert_eq!(metadata.len(), 7);
    // SAFETY: the descriptor is open while `file` is, and only its flags are read.
    let flags = unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(&file), libc::F_GETFL) };
    assert_eq!(flags & libc::O_NONBLOCK, 0, "the flag is cleared");
    let mut read = String::new();
    file.read_to_string(&mut read).unwrap();
    assert_eq!(read, "made up");
    for refused in ["link", "null", "pipe", "sub"] {
        assert!(
            directory.open_file(name(refused)).unwrap().is_none(),
            "{refused}"
        );
    }
}

#[test]
fn reads_a_links_words_and_refuses_what_is_not_one_name() {
    let scratch = Scratch::new("8");
    symlink("../somewhere/else", scratch.0.join("link")).unwrap();
    let directory = Directory::open(&scratch.0).unwrap();
    assert_eq!(
        directory.read_link(name("link")).unwrap(),
        PathBuf::from("../somewhere/else")
    );
    for bad in ["", ".", "..", "a/b", "/etc"] {
        assert_eq!(
            directory.open_directory(name(bad)).unwrap_err().kind(),
            ErrorKind::InvalidInput,
            "{bad:?}"
        );
        assert_eq!(
            directory.open_file(name(bad)).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
    }
    assert!(Directory::open(&scratch.0.join("link")).is_err());
}
