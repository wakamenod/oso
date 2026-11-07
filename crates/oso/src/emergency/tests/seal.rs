//! That each file of a copy holds no access list before anything goes in, seen through a stand-in
//! for the seal, so on any system (ADR 0065). `oso_platform::file_meta`'s tests set real lists.
//!
//! Must not depend on: `app` or winit.

use std::fs;
use std::io;

use super::{Scratch, file_buffer, published, scratch_editor, texts};
use crate::emergency::{Ticks, Why, write};

/// What `write_copy_sealing` does for each of its temporary files, seen from a seal that
/// `grants` from the given call on. Returns how long each file was when sealed, and the result.
fn sealing(grants_from: usize) -> (Vec<u64>, Scratch, io::Result<()>) {
    let scratch = Scratch::new();
    let directory = scratch.copies();
    fs::create_dir_all(&directory).unwrap();
    let mut editor = scratch_editor("text to keep");
    file_buffer(&mut editor, "/p/copy.txt", Some(&[]), "text to keep");
    let emergency = published(&directory, &editor);
    let entry = emergency.shared.state.lock().unwrap().entries[0].clone();
    let (ticks, _received) = std::sync::mpsc::channel();
    let lengths = std::cell::RefCell::new(Vec::new());
    let written = write::write_copy_sealing(
        &directory,
        &entry,
        Why::Panic,
        std::time::SystemTime::now(),
        &Ticks(&ticks),
        &|file| {
            let mut lengths = lengths.borrow_mut();
            lengths.push(file.metadata()?.len());
            Ok(lengths.len() > grants_from)
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
fn a_list_that_grants_anything_stops_the_copy_and_leaves_nothing() {
    for grants_from in [0, 1] {
        let (_, scratch, written) = sealing(grants_from);
        let error = written.unwrap_err().to_string();
        assert!(error.contains("access list"), "{error}");
        assert!(!error.contains("text to keep"), "{error}");
        let left: Vec<_> = fs::read_dir(scratch.copies()).unwrap().collect();
        assert!(left.is_empty(), "{left:?}");
    }
}
