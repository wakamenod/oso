//! The file: what it keeps, what it refuses, and two instances writing it.
//!
//! Must not depend on: the app or the window.

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::PathBuf;

use super::*;

/// A fresh private directory for one test, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let test = thread::current();
        let test = test.name().unwrap_or("test").replace("::", "-");
        let name = format!("oso-prompts-{}-{test}", std::process::id());
        let path = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    /// Puts `bytes` in the file, as another program might.
    fn put(&self, bytes: &[u8]) {
        fs::write(self.0.join(FILE), bytes).unwrap();
        fs::set_permissions(self.0.join(FILE), fs::Permissions::from_mode(0o600)).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn history(sent: &[(&str, u64)]) -> History {
    let mut history = History::default();
    for (text, now) in sent {
        history.note(text, *now);
    }
    history
}

fn texts(history: &History) -> Vec<&str> {
    (history.entries().iter()).map(|e| &*e.text).collect()
}

#[test]
fn what_is_written_reads_back_in_a_private_file() {
    let scratch = Scratch::new();
    let ours = history(&[("a", 1), ("日本語\nand \"more\"\u{7}", 2)]);
    let merged = write(&scratch.0, &ours, false).unwrap();
    assert_eq!(read(&scratch.0).unwrap().entries(), merged.entries());
    assert_eq!(
        texts(&read(&scratch.0).unwrap()),
        ["日本語\nand \"more\"\u{7}", "a"]
    );
    for name in [FILE, LOCK] {
        let mode = fs::metadata(scratch.0.join(name)).unwrap().mode();
        assert_eq!(mode & 0o777, 0o600, "{name}");
    }
    assert!(!scratch.0.join(PART).exists());
}

#[test]
fn nothing_there_reads_as_empty() {
    let scratch = Scratch::new();
    assert_eq!(read(&scratch.0).unwrap(), History::default());
    assert_eq!(read(&scratch.0.join("no")).unwrap(), History::default());
}

#[test]
fn two_instances_lose_none_of_each_others_nor_bring_back_what_one_cleared() {
    let scratch = Scratch::new();
    let one = history(&[("one", 1)]);
    let two = history(&[("two", 2)]);
    write(&scratch.0, &one, false).unwrap();
    assert_eq!(
        texts(&write(&scratch.0, &two, false).unwrap()),
        ["two", "one"]
    );
    let mut cleared = one.clone();
    cleared.clear(5);
    write(&scratch.0, &cleared, true).unwrap();
    assert!(read(&scratch.0).unwrap().is_empty());
    let mut later = two;
    later.note("later", 7);
    assert_eq!(texts(&write(&scratch.0, &later, false).unwrap()), ["later"]);
    assert_eq!(read(&scratch.0).unwrap().cleared(), 5);
}

#[test]
fn a_clear_takes_out_what_a_clock_ahead_stamped_also_where_another_instance_holds_it() {
    let scratch = Scratch::new();
    let ahead = history(&[("x", 500)]);
    write(&scratch.0, &ahead, false).unwrap();
    let mut cleared = History::default();
    cleared.clear(5);
    write(&scratch.0, &cleared, true).unwrap();
    assert!(read(&scratch.0).unwrap().is_empty());
    assert!(write(&scratch.0, &ahead, false).unwrap().is_empty());
}

#[test]
fn many_instances_at_once_lose_nothing() {
    let scratch = Scratch::new();
    let writers: Vec<_> = (0..8)
        .map(|n| {
            let directory = scratch.0.clone();
            thread::spawn(move || {
                let ours = history(&[(&format!("from {n}"), n + 1)]);
                // A write gives up on a lock held past `LOCK_WAIT`, as by the others' on a loaded
                // machine, and changes nothing then.
                let until = Instant::now() + Duration::from_secs(60);
                loop {
                    match write(&directory, &ours, false) {
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < until, "the lock was never had: {e}");
                        }
                        written => {
                            written.unwrap();
                            break;
                        }
                    }
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    assert_eq!(read(&scratch.0).unwrap().entries().len(), 8);
}

#[test]
fn bad_lines_are_left_out() {
    let scratch = Scratch::new();
    let long = "x".repeat(MAX_MESSAGE + 1);
    let text = format!(
        "{}\nnot json\n{}\n{}\n{}\n{}\n\n{}\n{}",
        json!({ "version": 1, "cleared": 2 }),
        json!({ "text": "a", "sent": 3 }),
        json!({ "text": "no stamp" }),
        json!({ "text": " \n", "sent": 4 }),
        json!({ "text": long, "sent": 4 }),
        json!({ "text": "before the clear", "sent": 1 }),
        json!({ "text": "never", "sent": u64::MAX }),
    );
    scratch.put(text.as_bytes());
    assert_eq!(texts(&read(&scratch.0).unwrap()), ["a"]);
    // A corrupt head loses no entry.
    scratch.put(b"{\"vers\n{\"text\":\"b\",\"sent\":3}\n");
    assert_eq!(texts(&read(&scratch.0).unwrap()), ["b"]);
    scratch.put(b"\xff\xfe\x00garbage");
    assert!(read(&scratch.0).unwrap().is_empty());
}

#[test]
fn a_file_that_may_not_be_read_is_left_until_a_clear() {
    let scratch = Scratch::new();
    let ours = history(&[("a", 1)]);
    let newer = b"{\"version\":2}\n{\"text\":\"kept\",\"sent\":1}\n";
    let large = vec![b' '; MAX_BYTES as usize + 1];
    for bytes in [&newer[..], &large] {
        scratch.put(bytes);
        assert!(read(&scratch.0).is_err());
        assert!(write(&scratch.0, &ours, false).is_err());
        assert_eq!(fs::read(scratch.0.join(FILE)).unwrap(), bytes);
        write(&scratch.0, &History::default(), true).unwrap();
        assert!(read(&scratch.0).unwrap().is_empty());
    }
    // A link, and a file with two names: never read nor written through, and a clear takes
    // only the name.
    let elsewhere = scratch.0.join("elsewhere");
    fs::write(&elsewhere, "{\"text\":\"a\",\"sent\":1}\n").unwrap();
    for hard in [false, true] {
        let file = scratch.0.join(FILE);
        fs::remove_file(&file).unwrap();
        match hard {
            true => fs::hard_link(&elsewhere, &file).unwrap(),
            false => std::os::unix::fs::symlink(&elsewhere, &file).unwrap(),
        }
        assert!(read(&scratch.0).is_err());
        assert!(write(&scratch.0, &ours, false).is_err());
        write(&scratch.0, &History::default(), true).unwrap();
        assert_eq!(
            fs::read_to_string(&elsewhere).unwrap(),
            "{\"text\":\"a\",\"sent\":1}\n"
        );
    }
}

#[test]
fn a_lock_another_holds_is_not_waited_for_long() {
    let scratch = Scratch::new();
    let private = PrivateDir::open(&scratch.0).unwrap();
    let held = lock(&private).unwrap();
    let refused = write(&scratch.0, &history(&[("a", 1)]), false);
    assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::WouldBlock);
    drop(held);
    write(&scratch.0, &history(&[("a", 1)]), false).unwrap();
}

#[test]
fn a_directory_others_can_reach_is_not_used() {
    let scratch = Scratch::new();
    fs::set_permissions(&scratch.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(read(&scratch.0).is_err());
    assert!(write(&scratch.0, &history(&[("a", 1)]), false).is_err());
}

#[test]
fn texts_that_escape_long_still_fit_what_is_read() {
    let scratch = Scratch::new();
    let mut ours = History::default();
    for n in 0..8 {
        ours.note(&format!("{n}{}", "\u{1}".repeat(MAX_MESSAGE - 1)), n + 1);
    }
    write(&scratch.0, &ours, false).unwrap();
    let kept = read(&scratch.0).unwrap();
    assert!(!kept.is_empty() && kept.entries().len() < 8);
    assert!(kept.entries()[0].text.starts_with('7'));
}
