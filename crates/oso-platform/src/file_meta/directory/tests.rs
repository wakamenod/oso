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

/// The permission bits the umask leaves of `0o666`, read from a file made the usual way, as
/// reading the umask itself would change it.
fn umask_leaves(scratch: &Scratch) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    let probe = scratch.0.join("probe");
    fs::File::create(&probe).unwrap();
    let mode = fs::metadata(&probe).unwrap().permissions().mode() & 0o777;
    fs::remove_file(probe).unwrap();
    mode
}

#[test]
fn makes_a_directory_below_or_opens_the_one_there() {
    let scratch = Scratch::new("9");
    let directory = Directory::open(&scratch.0).unwrap();
    let made = directory.create_directory(name("sub")).unwrap().unwrap();
    assert!(fs::metadata(scratch.0.join("sub")).unwrap().is_dir());
    assert_eq!(
        made.metadata().unwrap().ino(),
        fs::metadata(scratch.0.join("sub")).unwrap().ino()
    );
    // Made again, it is the same one, and what it holds stays.
    made.create_file(name("inner"), 0o600).unwrap();
    let again = directory.create_directory(name("sub")).unwrap().unwrap();
    let inner: Vec<_> = again.entries().unwrap().map(|e| e.unwrap().name).collect();
    assert_eq!(inner, [OsStr::new("inner")]);
}

#[test]
fn makes_no_directory_and_no_file_through_a_link_or_over_a_file() {
    let scratch = Scratch::new("10");
    let outside = Scratch::new("11");
    symlink(&outside.0, scratch.0.join("to-dir")).unwrap();
    symlink(outside.0.join("missing"), scratch.0.join("dangling")).unwrap();
    fs::write(scratch.0.join("file"), b"kept").unwrap();
    fifo(&scratch.0.join("pipe"));
    let directory = Directory::open(&scratch.0).unwrap();
    for there in ["to-dir", "dangling", "file", "pipe"] {
        assert!(
            directory.create_directory(name(there)).unwrap().is_none(),
            "{there}"
        );
        assert_eq!(
            directory
                .create_file(name(there), 0o600)
                .unwrap_err()
                .kind(),
            ErrorKind::AlreadyExists,
            "{there}"
        );
    }
    assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);
    assert_eq!(fs::read(scratch.0.join("file")).unwrap(), b"kept");
}

#[test]
fn makes_a_new_file_with_its_permission_bits_only() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("12");
    let leaves = umask_leaves(&scratch);
    let directory = Directory::open(&scratch.0).unwrap();
    let mut file = directory.create_file(name("new"), 0o640).unwrap();
    file.write_all(b"written").unwrap();
    drop(file);
    assert_eq!(fs::read(scratch.0.join("new")).unwrap(), b"written");
    let mode = |path: &str| {
        fs::metadata(scratch.0.join(path))
            .unwrap()
            .permissions()
            .mode()
    };
    assert_eq!(mode("new") & 0o777, 0o640 & leaves);
    // The set-user-ID, set-group-ID and sticky bits are never set.
    directory.create_file(name("setuid"), 0o7640).unwrap();
    assert_eq!(mode("setuid") & 0o7777, 0o640 & leaves);
    assert_eq!(
        directory
            .create_file(name("new"), 0o640)
            .unwrap_err()
            .kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(scratch.0.join("new")).unwrap(), b"written");
}

#[test]
fn makes_nothing_by_what_is_not_one_name() {
    let scratch = Scratch::new("13");
    fs::create_dir(scratch.0.join("a")).unwrap();
    let directory = Directory::open(&scratch.0).unwrap();
    for bad in ["", ".", "..", "a/b", "/tmp"] {
        assert_eq!(
            directory.create_directory(name(bad)).unwrap_err().kind(),
            ErrorKind::InvalidInput,
            "{bad:?}"
        );
        assert_eq!(
            directory.create_file(name(bad), 0o600).unwrap_err().kind(),
            ErrorKind::InvalidInput,
            "{bad:?}"
        );
    }
    assert_eq!(fs::read_dir(scratch.0.join("a")).unwrap().count(), 0);
}

/// Every entry `list_with_metadata` gives, by name.
fn listed(directory: &Directory) -> Vec<super::EntryMeta> {
    let mut found = Vec::new();
    directory
        .list_with_metadata(&mut |entry| {
            found.push(entry.unwrap());
            true
        })
        .unwrap();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

#[test]
fn a_listing_says_what_each_entry_is_without_following_it() {
    let scratch = Scratch::new("meta");
    fs::write(scratch.0.join("file"), b"12345").unwrap();
    fs::create_dir(scratch.0.join("dir")).unwrap();
    symlink("file", scratch.0.join("link")).unwrap();
    symlink("nowhere", scratch.0.join("broken")).unwrap();
    fifo(&scratch.0.join("pipe"));
    let directory = Directory::open(&scratch.0).unwrap();
    let found = listed(&directory);
    let names: Vec<_> = found
        .iter()
        .map(|entry| entry.name.to_str().unwrap())
        .collect();
    assert_eq!(names, ["broken", "dir", "file", "link", "pipe"]);
    let kinds: Vec<_> = found.iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        [
            EntryKind::Link,
            EntryKind::Directory,
            EntryKind::File,
            EntryKind::Link,
            EntryKind::Other
        ]
    );
    let file = &found[2];
    let on_disk = fs::symlink_metadata(scratch.0.join("file")).unwrap();
    assert_eq!(file.len, 5);
    assert_eq!(file.mode, on_disk.mode());
    assert_eq!(file.owner, Some((on_disk.uid(), on_disk.gid())));
    assert!(!file.others);
    assert_eq!(file.modified, on_disk.modified().ok());
    // A link's own size is its target's length, as `lstat` says.
    assert_eq!(found[3].len, 4);
    assert_eq!(found[3].mode & 0o170_000, 0o120_000);
    assert_eq!(found[4].mode & 0o170_000, 0o010_000);
}

#[test]
fn a_listing_stops_when_asked() {
    let scratch = Scratch::new("meta-stop");
    for name in ["a", "b", "c"] {
        fs::write(scratch.0.join(name), b"").unwrap();
    }
    let directory = Directory::open(&scratch.0).unwrap();
    let mut seen = 0;
    directory
        .list_with_metadata(&mut |_| {
            seen += 1;
            false
        })
        .unwrap();
    assert_eq!(seen, 1);
    // It lists from the start each time.
    assert_eq!(listed(&directory).len(), 3);
}

#[test]
fn a_listing_reads_a_large_directory_whole() {
    let scratch = Scratch::new("meta-many");
    for index in 0..3000 {
        fs::write(
            scratch
                .0
                .join(format!("file-with-a-longish-name-{index:05}")),
            b"",
        )
        .unwrap();
    }
    let directory = Directory::open(&scratch.0).unwrap();
    let found = listed(&directory);
    assert_eq!(found.len(), 3000);
    assert!(found.iter().all(|entry| entry.kind == EntryKind::File));
}

#[test]
fn a_target_kind_follows_the_link() {
    let scratch = Scratch::new("target");
    fs::create_dir(scratch.0.join("dir")).unwrap();
    symlink("dir", scratch.0.join("to-dir")).unwrap();
    symlink("nowhere", scratch.0.join("broken")).unwrap();
    let directory = Directory::open(&scratch.0).unwrap();
    assert_eq!(
        directory.target_kind(name("to-dir")).unwrap(),
        EntryKind::Directory
    );
    assert_eq!(
        directory.target_kind(name("broken")).unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert!(directory.target_kind(name("../dir")).is_err());
}

#[test]
fn the_dots_are_read_from_the_directory() {
    let scratch = Scratch::new("dots");
    fs::create_dir(scratch.0.join("inner")).unwrap();
    let directory = Directory::open(&scratch.0.join("inner")).unwrap();
    let [here, above] = directory.dots();
    let (here, above) = (here.unwrap(), above.unwrap());
    assert_eq!(here.name, ".");
    assert_eq!(above.name, "..");
    assert_eq!(here.kind, EntryKind::Directory);
    assert_eq!(above.kind, EntryKind::Directory);
    let on_disk = fs::metadata(&scratch.0).unwrap();
    assert_eq!(above.mode, on_disk.mode());
    assert!(here.mode_text().starts_with('d'));
}

#[test]
fn a_rename_refuses_what_is_there_unless_it_replaces_it() {
    let scratch = Scratch::new("rename");
    fs::write(scratch.0.join("a"), b"one").unwrap();
    fs::write(scratch.0.join("b"), b"two").unwrap();
    fs::create_dir(scratch.0.join("sub")).unwrap();
    let directory = Directory::open(&scratch.0).unwrap();
    let sub = directory.open_directory(name("sub")).unwrap().unwrap();
    let refused = directory.rename_to(name("a"), &directory, name("b"), false);
    assert_eq!(refused.unwrap_err().kind(), ErrorKind::AlreadyExists);
    directory
        .rename_to(name("a"), &sub, name("moved"), false)
        .unwrap();
    assert_eq!(fs::read(scratch.0.join("sub/moved")).unwrap(), b"one");
    directory
        .rename_to(name("b"), &sub, name("moved"), true)
        .unwrap();
    assert_eq!(fs::read(scratch.0.join("sub/moved")).unwrap(), b"two");
    assert!(
        directory
            .rename_to(name("../x"), &sub, name("y"), false)
            .is_err()
    );
}

#[test]
fn a_tree_is_removed_without_following_its_links() {
    let scratch = Scratch::new("tree");
    let outside = scratch.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("kept"), b"").unwrap();
    let tree = scratch.0.join("tree");
    fs::create_dir_all(tree.join("a/b")).unwrap();
    fs::write(tree.join("a/b/file"), b"").unwrap();
    symlink(&outside, tree.join("a/link")).unwrap();
    let directory = Directory::open(&scratch.0).unwrap();
    let mut removed = 0;
    directory
        .remove_tree(name("tree"), &|| false, &mut || removed += 1)
        .unwrap();
    assert!(!tree.exists());
    assert!(outside.join("kept").exists(), "a link's target stays");
    assert_eq!(removed, 5);
    // A link itself is removed, not what it leads to.
    symlink(&outside, scratch.0.join("alone")).unwrap();
    directory.remove(name("alone")).unwrap();
    assert!(outside.exists());
}

#[test]
fn a_tree_removal_stops_when_asked() {
    let scratch = Scratch::new("tree-stop");
    fs::create_dir(scratch.0.join("tree")).unwrap();
    fs::write(scratch.0.join("tree/a"), b"").unwrap();
    let directory = Directory::open(&scratch.0).unwrap();
    let stopped = directory.remove_tree(name("tree"), &|| true, &mut || {});
    assert_eq!(stopped.unwrap_err().kind(), ErrorKind::Interrupted);
    assert!(scratch.0.join("tree/a").exists());
}

#[test]
fn a_copy_keeps_the_mode_and_time_and_replaces_only_when_asked() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("copy");
    let source = scratch.0.join("source");
    fs::write(&source, b"text").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(old)
        .unwrap();
    fs::create_dir(scratch.0.join("to")).unwrap();
    fs::write(scratch.0.join("to/there"), b"old").unwrap();
    let directory = Directory::open(&scratch.0).unwrap();
    let to = directory.open_directory(name("to")).unwrap().unwrap();
    directory
        .copy_file_to(name("source"), &to, name("copy"), false)
        .unwrap();
    let copy = fs::metadata(scratch.0.join("to/copy")).unwrap();
    assert_eq!(fs::read(scratch.0.join("to/copy")).unwrap(), b"text");
    assert_eq!(copy.mode() & 0o777, 0o640);
    assert_eq!(copy.modified().unwrap(), old);
    let refused = directory.copy_file_to(name("source"), &to, name("there"), false);
    assert_eq!(refused.unwrap_err().kind(), ErrorKind::AlreadyExists);
    assert_eq!(fs::read(scratch.0.join("to/there")).unwrap(), b"old");
    directory
        .copy_file_to(name("source"), &to, name("there"), true)
        .unwrap();
    assert_eq!(fs::read(scratch.0.join("to/there")).unwrap(), b"text");
    // No file of its own is left beside it.
    assert_eq!(fs::read_dir(scratch.0.join("to")).unwrap().count(), 2);
}

#[test]
fn a_link_is_copied_as_a_link_and_never_followed_as_a_file() {
    let scratch = Scratch::new("copy-link");
    symlink("somewhere/else", scratch.0.join("link")).unwrap();
    fs::write(scratch.0.join("target"), b"secret").unwrap();
    symlink("target", scratch.0.join("to-target")).unwrap();
    let directory = Directory::open(&scratch.0).unwrap();
    directory
        .copy_link_to(name("link"), &directory, name("copy"), false)
        .unwrap();
    assert_eq!(
        fs::read_link(scratch.0.join("copy")).unwrap(),
        PathBuf::from("somewhere/else")
    );
    // A link given as a file is refused, not read through.
    assert!(
        directory
            .copy_file_to(name("to-target"), &directory, name("x"), false)
            .is_err()
    );
    assert!(!scratch.0.join("x").exists());
}

#[test]
fn a_directory_is_made_with_a_mode_or_the_one_there_is_opened() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("mkdir");
    let directory = Directory::open(&scratch.0).unwrap();
    let made = directory.make_directory(name("private"), 0o700).unwrap();
    assert!(made.is_some());
    let mode = fs::metadata(scratch.0.join("private"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    assert!(
        directory
            .make_directory(name("private"), 0o755)
            .unwrap()
            .is_some()
    );
    symlink("private", scratch.0.join("link")).unwrap();
    assert!(
        directory
            .make_directory(name("link"), 0o755)
            .unwrap()
            .is_none()
    );
    let (device, _) = directory.identity().unwrap().unwrap();
    assert_eq!(device, fs::metadata(&scratch.0).unwrap().dev());
}
