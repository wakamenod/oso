//! Tests of the images kept for sessions: names, references, files and their modes, the TIFF
//! conversion, and what a sweep and a delete remove.
//!
//! Must not depend on: the person's cache directory: each test has a scratch directory.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oso_platform::clipboard::{Image, ImageKind};

use super::{
    KEEP, delete_file, delete_session, plain_name, reference, save, session_directory, stem, sweep,
};
use crate::sessions::identity::SessionId;

/// A scratch directory, removed when dropped. Its name has no space, as an `@` reference
/// cannot hold one.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("oso-images-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        Self(dir)
    }

    /// The images' root in it, as [`super::root`] gives it under the cache.
    fn root(&self) -> PathBuf {
        self.0.join("oso").join("images")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn id(n: u8) -> SessionId {
    SessionId::from_random([n; 16])
}

fn at(seconds: u64, millis: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds) + Duration::from_millis(millis)
}

fn png(bytes: &[u8]) -> Image {
    Image {
        kind: ImageKind::Png,
        bytes: bytes.to_vec(),
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .map(|read| {
            read.map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// A 1×1 grey TIFF, uncompressed, little-endian: the smallest `sips` reads.
fn tiny_tiff() -> Vec<u8> {
    const SHORT: u16 = 3;
    const LONG: u16 = 4;
    let entries: [(u16, u16, u32); 9] = [
        (256, SHORT, 1),  // width
        (257, SHORT, 1),  // length
        (258, SHORT, 8),  // bits per sample
        (259, SHORT, 1),  // no compression
        (262, SHORT, 1),  // black is zero
        (273, LONG, 122), // the strip's offset: 8 + 2 + 9 × 12 + 4
        (277, SHORT, 1),  // samples per pixel
        (278, SHORT, 1),  // rows per strip
        (279, LONG, 1),   // the strip's length
    ];
    let mut tiff = b"II".to_vec();
    tiff.extend(42u16.to_le_bytes());
    tiff.extend(8u32.to_le_bytes());
    tiff.extend(u16::try_from(entries.len()).unwrap().to_le_bytes());
    for (tag, kind, value) in entries {
        tiff.extend(tag.to_le_bytes());
        tiff.extend(kind.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend(value.to_le_bytes());
    }
    tiff.extend(0u32.to_le_bytes());
    assert_eq!(tiff.len(), 122);
    tiff.push(0x80);
    tiff
}

#[test]
fn a_name_is_the_local_time_to_the_millisecond() {
    assert_eq!(stem(UNIX_EPOCH, 0), "19700101-000000-000");
    assert_eq!(stem(at(1_794_797_058, 359), 0), "20261116-024418-359");
    assert_eq!(stem(at(1_709_251_199, 999), 0), "20240229-235959-999");
    // East and west of UTC, across a day.
    assert_eq!(stem(at(0, 5), 9 * 3600), "19700101-090000-005");
    assert_eq!(stem(UNIX_EPOCH, -3600), "19691231-230000-000");
    assert_eq!(stem(at(1_709_251_199, 0), 3600), "20240301-005959-000");
    // Before the epoch, which no clock here gives, is the epoch.
    assert_eq!(
        stem(UNIX_EPOCH - Duration::from_secs(1), 0),
        "19700101-000000-000"
    );
}

#[test]
fn a_reference_is_spaced_and_refuses_what_it_cannot_hold() {
    assert_eq!(
        reference(Path::new("/c/oso/images/a/1.png")).unwrap(),
        " @/c/oso/images/a/1.png "
    );
    for bad in [
        "/Users/a b/1.png",
        "/a/\n.png",
        "/a/\t.png",
        "/a/\u{202e}gnp.png",
        "/a/\u{200b}.png",
    ] {
        assert!(reference(Path::new(bad)).is_err(), "{bad:?}");
    }
    let not_utf8 = std::ffi::OsStr::from_bytes(b"/a/\xff.png");
    assert!(reference(Path::new(not_utf8)).is_err());
}

use std::os::unix::ffi::OsStrExt;

#[test]
fn only_a_plain_id_names_a_directory() {
    let root = Path::new("/r");
    assert_eq!(
        session_directory(root, &id(1)),
        Some(root.join(id(1).as_str()))
    );
    for bad in ["", "..", "a/b", "a b", "a.b", &"a".repeat(65)] {
        assert!(!plain_name(bad), "{bad:?}");
    }
    assert!(plain_name("0f1e2d3c-aaaa-4bbb-8ccc-0123456789ab"));
}

#[test]
fn an_image_is_a_new_private_file_in_a_private_directory() {
    let scratch = Scratch::new("save");
    let root = scratch.root();
    let now = at(1_794_797_058, 359);
    let kept = save(&root, &id(1), &png(b"first"), now, 0).unwrap();
    let directory = root.join(id(1).as_str());
    assert_eq!(kept.path, directory.join("20261116-024418-359.png"));
    assert_eq!(kept.reference, format!(" @{} ", kept.path.display()));
    assert!(!kept.converted);
    assert_eq!(fs::read(&kept.path).unwrap(), b"first");
    assert_eq!(mode(&kept.path), 0o600);
    assert_eq!(mode(&directory), 0o700);
    assert_eq!(mode(&root), 0o700);

    // The same millisecond again takes the next name, and never writes over the first.
    let second = save(&root, &id(1), &png(b"second"), now, 0).unwrap();
    assert_eq!(second.path, directory.join("20261116-024418-359-2.png"));
    assert_eq!(fs::read(&kept.path).unwrap(), b"first");
    let jpeg = Image {
        kind: ImageKind::Jpeg,
        bytes: b"j".to_vec(),
    };
    let third = save(&root, &id(1), &jpeg, now, 0).unwrap();
    assert_eq!(third.path, directory.join("20261116-024418-359.jpg"));
}

#[test]
fn a_name_taken_by_a_link_is_skipped_and_not_followed() {
    let scratch = Scratch::new("link");
    let root = scratch.root();
    let directory = root.join(id(2).as_str());
    let now = at(0, 0);
    save(&root, &id(2), &png(b"x"), at(1, 0), 0).unwrap();
    let outside = scratch.0.join("outside");
    fs::write(&outside, b"keep").unwrap();
    std::os::unix::fs::symlink(&outside, directory.join("19700101-000000-000.png")).unwrap();
    let kept = save(&root, &id(2), &png(b"new"), now, 0).unwrap();
    assert_eq!(kept.path, directory.join("19700101-000000-000-2.png"));
    assert_eq!(fs::read(&outside).unwrap(), b"keep");
}

#[test]
fn every_name_taken_says_so() {
    let scratch = Scratch::new("full");
    let root = scratch.root();
    let now = at(0, 0);
    for _ in 0..super::NAMES_TRIED {
        save(&root, &id(3), &png(b"x"), now, 0).unwrap();
    }
    let refused = save(&root, &id(3), &png(b"x"), now, 0).unwrap_err();
    assert!(refused.contains("every name"), "{refused}");
}

#[test]
fn a_path_with_a_space_writes_nothing() {
    let scratch = Scratch::new("space");
    let root = scratch.0.join("with space").join("images");
    let refused = save(&root, &id(4), &png(b"x"), at(0, 0), 0).unwrap_err();
    assert!(refused.contains("space"), "{refused}");
    assert!(!scratch.0.join("with space").exists(), "nothing was made");
}

#[test]
fn a_shared_directory_is_refused() {
    let scratch = Scratch::new("shared");
    let root = scratch.root();
    let directory = root.join(id(5).as_str());
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    let refused = save(&root, &id(5), &png(b"x"), at(0, 0), 0).unwrap_err();
    assert!(refused.starts_with("Cannot keep the image"), "{refused}");
    assert!(names(&directory).is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn a_tiff_becomes_a_private_png_and_leaves_no_tiff() {
    let scratch = Scratch::new("tiff");
    let root = scratch.root();
    let tiff = Image {
        kind: ImageKind::Tiff,
        bytes: tiny_tiff(),
    };
    let kept = save(&root, &id(6), &tiff, at(0, 0), 0).unwrap();
    assert!(kept.converted);
    assert_eq!(kept.path.extension().unwrap(), "png");
    assert!(
        fs::read(&kept.path)
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );
    assert_eq!(mode(&kept.path), 0o600);
    assert_eq!(
        names(kept.path.parent().unwrap()),
        ["19700101-000000-000.png"]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_tiff_sips_cannot_read_leaves_nothing() {
    let scratch = Scratch::new("badtiff");
    let root = scratch.root();
    let bad = Image {
        kind: ImageKind::Tiff,
        bytes: b"not an image".to_vec(),
    };
    let refused = save(&root, &id(7), &bad, at(0, 0), 0).unwrap_err();
    assert!(refused.contains("PNG"), "{refused}");
    assert!(names(&root.join(id(7).as_str())).is_empty());
}

#[test]
fn a_sweep_removes_old_images_and_empty_directories_only() {
    let scratch = Scratch::new("sweep");
    let root = scratch.root();
    let now = SystemTime::now();
    let old = save(&root, &id(8), &png(b"old"), at(0, 0), 0).unwrap().path;
    let fresh = save(&root, &id(8), &png(b"fresh"), at(1, 0), 0)
        .unwrap()
        .path;
    let alone = save(&root, &id(9), &png(b"alone"), at(0, 0), 0)
        .unwrap()
        .path;
    let other = root.join(id(8).as_str()).join("notes.txt");
    fs::write(&other, b"not ours").unwrap();
    let long_ago = now - KEEP - Duration::from_secs(60);
    for path in [&old, &alone, &other] {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
    }
    assert_eq!(sweep(&root, now), 2);
    assert!(!old.exists() && !alone.exists());
    assert!(fresh.exists(), "younger than KEEP");
    assert!(other.exists(), "not an image save writes");
    assert!(
        !root.join(id(9).as_str()).exists(),
        "left empty, so removed"
    );
    assert!(root.join(id(8).as_str()).exists());
}

#[test]
fn a_sweep_leaves_a_root_that_is_not_private_or_not_there() {
    let scratch = Scratch::new("sweepshared");
    let root = scratch.root();
    assert_eq!(sweep(&root, SystemTime::now()), 0, "nothing there");
    let old = save(&root, &id(10), &png(b"old"), at(0, 0), 0)
        .unwrap()
        .path;
    fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(UNIX_EPOCH)
        .unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(sweep(&root, SystemTime::now()), 0);
    assert!(old.exists());
}

#[test]
fn deleting_a_session_removes_its_directory_and_refuses_a_link() {
    let scratch = Scratch::new("delete");
    let root = scratch.root();
    let kept = save(&root, &id(11), &png(b"x"), at(0, 0), 0).unwrap();
    delete_session(&root, &id(11)).unwrap();
    assert!(!kept.path.exists());
    assert!(!root.join(id(11).as_str()).exists());
    delete_session(&root, &id(11)).unwrap();

    let outside = scratch.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), b"keep").unwrap();
    std::os::unix::fs::symlink(&outside, root.join(id(12).as_str())).unwrap();
    assert!(delete_session(&root, &id(12)).is_err(), "a link is refused");
    assert!(outside.join("keep").exists());

    let file = save(&root, &id(13), &png(b"x"), at(0, 0), 0).unwrap().path;
    delete_file(&file).unwrap();
    assert!(!file.exists());
    delete_file(&file).unwrap();
}

#[test]
fn nothing_is_removed_where_another_user_could_change_the_way() {
    let scratch = Scratch::new("openway");
    let root = scratch.root();
    let kept = save(&root, &id(14), &png(b"keep"), at(0, 0), 0)
        .unwrap()
        .path;
    // Anyone could put a link at `images` now, so nothing under it is trusted.
    fs::set_permissions(scratch.0.join("oso"), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(delete_session(&root, &id(14)).is_err());
    assert!(delete_file(&kept).is_err());
    assert_eq!(sweep(&root, SystemTime::now() + KEEP * 2), 0);
    assert!(kept.exists());
}

#[test]
fn the_keeper_keeps_says_and_removes_in_order() {
    let scratch = Scratch::new("keeper");
    let root = scratch.root();
    let mut keeper = super::Keeper::default();
    let (tx, rx) = std::sync::mpsc::channel();
    keeper
        .save(
            root.clone(),
            id(15),
            png(b"kept"),
            Box::new(move |kept| tx.send(kept).unwrap()),
        )
        .unwrap();
    let kept = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    assert_eq!(fs::read(&kept.path).unwrap(), b"kept");
    keeper.drop_session(root.clone(), id(15));
    // A job after it on the same thread sees it done.
    let (tx, rx) = std::sync::mpsc::channel();
    keeper
        .save(
            root.clone(),
            id(16),
            png(b"x"),
            Box::new(move |kept| tx.send(kept).unwrap()),
        )
        .unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    assert!(!root.join(id(15).as_str()).exists());
}
