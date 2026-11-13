//! Tests of what the migemo dictionary's reader decides (ADR 0116): when it reads, what a
//! change of the setting does, a read for an older setting, a failure and trying again, and
//! a path from `~/`. Every dictionary here is synthetic.
//!
//! Must not depend on: key presses, the window or the thread; these run on the reader alone.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::migemo::Dictionary;

use super::{DictionaryRead, Effects, MigemoReader, Named, grouped, named};

fn file(name: &str) -> Option<Named> {
    Some(Named::File(PathBuf::from(format!("/dict/{name}"))))
}

fn table() -> Arc<Dictionary> {
    Arc::new(Dictionary::new("かんじ\t漢字\n".to_owned()))
}

/// The generation of the file `effects` sends to the thread.
fn sent(effects: &Effects) -> u64 {
    effects
        .read
        .as_ref()
        .map(|(generation, _)| *generation)
        .expect("a file to read")
}

fn done(generation: u64, read: Result<(Arc<Dictionary>, bool), String>) -> DictionaryRead {
    DictionaryRead { generation, read }
}

#[test]
fn nothing_is_read_until_a_search_wants_it_and_then_once() {
    let mut reader = MigemoReader::default();
    assert!(reader.configure(file("a")).read.is_none());
    let effects = reader.wanted();
    let generation = sent(&effects);
    assert!(reader.wanted().read.is_none(), "being read");
    let effects = reader.finished(done(generation, Ok((table(), false))));
    assert!(matches!(effects.give, Some(Some(_))));
    assert_eq!(effects.notes, ["Migemo dictionary: 1 readings"]);
    assert!(reader.wanted().read.is_none(), "read");
}

#[test]
fn with_none_named_nothing_is_read() {
    let mut reader = MigemoReader::default();
    assert!(reader.configure(None).read.is_none());
    assert!(reader.wanted().read.is_none());
}

#[test]
fn a_group_that_can_change_the_file_is_warned_of() {
    let mut reader = MigemoReader::default();
    reader.configure(file("a"));
    let generation = sent(&reader.wanted());
    let effects = reader.finished(done(generation, Ok((table(), true))));
    assert_eq!(effects.notes.len(), 2);
    assert!(effects.notes[0].starts_with("Migemo dictionary: other users in a group"));
}

#[test]
fn a_new_file_is_read_at_once_while_a_table_serves_and_a_failure_drops_it() {
    let mut reader = MigemoReader::default();
    reader.configure(file("a"));
    let generation = sent(&reader.wanted());
    reader.finished(done(generation, Ok((table(), false))));
    let effects = reader.configure(file("b"));
    let newer = sent(&effects);
    assert!(effects.give.is_none(), "the old table serves");
    let effects = reader.finished(done(newer, Err("it is not UTF-8 text".to_owned())));
    assert!(matches!(effects.give, Some(None)));
    assert!(effects.free.is_some(), "dropped off the UI thread");
    assert_eq!(
        effects.notes,
        ["Migemo dictionary not read: it is not UTF-8 text"]
    );
}

#[test]
fn a_new_file_replaces_the_table_and_frees_the_old_one() {
    let mut reader = MigemoReader::default();
    reader.configure(file("a"));
    let generation = sent(&reader.wanted());
    reader.finished(done(generation, Ok((table(), false))));
    let newer = sent(&reader.configure(file("b")));
    let effects = reader.finished(done(newer, Ok((table(), false))));
    assert!(matches!(effects.give, Some(Some(_))));
    assert!(effects.free.is_some());
}

#[test]
fn a_read_for_an_older_setting_is_dropped() {
    let mut reader = MigemoReader::default();
    reader.configure(file("a"));
    let older = sent(&reader.wanted());
    reader.configure(file("b"));
    let effects = reader.finished(done(older, Ok((table(), false))));
    assert!(effects.give.is_none() && effects.notes.is_empty());
    assert!(effects.free.is_some());
    // The newer one is read at the next search.
    assert!(sent(&reader.wanted()) > older);
}

#[test]
fn a_failed_file_is_tried_again_only_when_the_configuration_is_read_again() {
    let mut reader = MigemoReader::default();
    reader.configure(file("a"));
    let generation = sent(&reader.wanted());
    reader.finished(done(generation, Err("there is no such file".to_owned())));
    assert!(reader.wanted().read.is_none());
    assert!(reader.configure(file("a")).read.is_none());
    sent(&reader.wanted());
}

#[test]
fn the_setting_taken_away_drops_the_table() {
    let mut reader = MigemoReader::default();
    reader.configure(file("a"));
    let generation = sent(&reader.wanted());
    reader.finished(done(generation, Ok((table(), false))));
    let effects = reader.configure(None);
    assert!(matches!(effects.give, Some(None)));
    assert!(effects.free.is_some());
}

#[test]
fn a_thread_that_cannot_start_reads_nothing_until_the_configuration_says_so() {
    let mut reader = MigemoReader::default();
    reader.configure(file("a"));
    sent(&reader.wanted());
    reader.could_not_send();
    assert!(reader.wanted().read.is_none());
    reader.configure(file("a"));
    sent(&reader.wanted());
}

#[test]
fn a_path_from_home_needs_an_absolute_home() {
    let home = Path::new("/home/me");
    let file = |path: &str| Named::File(PathBuf::from(path));
    assert_eq!(
        named("~/dict/migemo-dict", Some(home)),
        file("/home/me/dict/migemo-dict")
    );
    assert_eq!(named("~/dict", None), Named::NoHome);
    // An empty or relative `HOME` would name a file in the directory oso started in.
    assert_eq!(named("~/dict", Some(Path::new(""))), Named::NoHome);
    assert_eq!(named("~/dict", Some(Path::new("relative"))), Named::NoHome);
    let absolute = if cfg!(windows) {
        r"C:\dict"
    } else {
        "/usr/share/dict"
    };
    assert_eq!(named(absolute, Some(home)), file(absolute));
    let mut reader = MigemoReader::default();
    let effects = reader.configure(Some(Named::NoHome));
    assert_eq!(
        effects.notes,
        ["Migemo dictionary not read: no absolute home directory is known for ~/"]
    );
    assert!(reader.wanted().read.is_none());
}

#[test]
fn counts_have_a_comma_between_each_three_digits() {
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1000), "1,000");
    assert_eq!(grouped(137_025), "137,025");
    assert_eq!(grouped(1_048_576), "1,048,576");
}
