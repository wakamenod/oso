//! That each file of a copy is sealed before anything goes in, seen through a stand-in for the
//! seal, so on any system, and with real lists on macOS (ADR 0065).
//!
//! Must not depend on: `app` or winit.

use std::fs;
use std::io;

use super::{Scratch, file_buffer, published, scratch_editor, texts};
use crate::emergency::{Entry, Ticks, Why, write};

/// A directory for copies, and what is published for one buffer.
fn prepared() -> (Scratch, Entry) {
    let scratch = Scratch::new();
    let directory = scratch.copies();
    fs::create_dir_all(&directory).unwrap();
    let mut editor = scratch_editor("text to keep");
    file_buffer(&mut editor, "/p/copy.txt", Some(&[]), "text to keep");
    let emergency = published(&directory, &editor);
    let entry = emergency.shared.state.lock().unwrap().entries[0].clone();
    (scratch, entry)
}

/// What `write_copy_with` does for each of its temporary files, seen from a seal that refuses from
/// the given call on. Returns how long each file was when sealed, and the result.
fn sealing(refuses_from: usize) -> (Vec<u64>, Scratch, io::Result<()>) {
    let (scratch, entry) = prepared();
    let directory = scratch.copies();
    let (ticks, _received) = std::sync::mpsc::channel();
    let lengths = std::cell::RefCell::new(Vec::new());
    let written = write::write_copy_with(
        &directory,
        &entry,
        Why::Panic,
        std::time::SystemTime::now(),
        &Ticks(&ticks),
        &|file| {
            let mut lengths = lengths.borrow_mut();
            lengths.push(file.metadata()?.len());
            if lengths.len() > refuses_from {
                return Err(io::Error::other("refused by the seal"));
            }
            Ok(())
        },
    );
    (lengths.into_inner(), scratch, written)
}

#[test]
fn each_file_of_a_copy_is_sealed_before_anything_goes_in() {
    let (lengths, scratch, written) = sealing(usize::MAX);
    written.unwrap();
    assert_eq!(lengths, [0, 0]);
    assert_eq!(texts(&scratch.copies()), ["text to keep"]);
}

#[test]
fn a_seal_that_refuses_stops_the_copy_and_leaves_nothing() {
    for refuses_from in [0, 1] {
        let (_, scratch, written) = sealing(refuses_from);
        let error = written.unwrap_err().to_string();
        assert!(error.contains("refused by the seal"), "{error}");
        assert!(!error.contains("text to keep"), "{error}");
        let left: Vec<_> = fs::read_dir(scratch.copies()).unwrap().collect();
        assert!(left.is_empty(), "{left:?}");
    }
}

/// Only here does the real seal meet a real list. Lists are set with `chmod`, as this crate has no
/// bindings of its own, and the dev feature is what lets a test start a process.
#[cfg(all(target_os = "macos", feature = "dev"))]
mod real_lists {
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    use oso_platform::process::{Executable, Sink, Spawn, spawn};

    use super::{Ticks, Why, prepared, texts, write};

    fn add_to_list(path: &Path, entry: &str) {
        let chmod = Executable::absolute("/bin/chmod").unwrap();
        let args = [OsString::from("+a"), entry.into(), path.into()];
        let mut child = spawn(&Spawn {
            executable: &chmod,
            args: &args,
            cwd: None,
            project: None,
            inherit: &[],
            stdout: Sink::Null,
            stderr: Sink::Null,
            own_process_group: false,
        })
        .unwrap();
        assert!(child.wait().unwrap().success());
    }

    fn write_one(entry: &super::Entry, directory: &Path) -> std::io::Result<()> {
        let (ticks, _received) = std::sync::mpsc::channel();
        write::write_copy(
            directory,
            entry,
            Why::Panic,
            std::time::SystemTime::now(),
            &Ticks(&ticks),
        )
    }

    #[test]
    fn a_list_that_grants_anything_stops_a_real_copy_and_leaves_nothing() {
        let (scratch, entry) = prepared();
        let directory = scratch.copies();
        add_to_list(&directory, "everyone allow list,file_inherit");
        let error = write_one(&entry, &directory).unwrap_err().to_string();
        assert!(error.contains("access list"), "{error}");
        assert!(!error.contains("text to keep"), "{error}");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
    }

    #[test]
    fn a_deny_a_real_copy_takes_is_removed_so_that_it_can_be_put_in_place() {
        let (scratch, entry) = prepared();
        let directory = scratch.copies();
        add_to_list(&directory, "everyone deny delete,file_inherit,only_inherit");
        write_one(&entry, &directory).unwrap();
        assert_eq!(texts(&directory), ["text to keep"]);
    }
}
