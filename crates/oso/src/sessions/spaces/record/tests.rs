//! The `spaces` file: what is written is read back, a bad line is left out, another
//! instance's spaces are kept, the caps hold, and a file that may not be read is not replaced.
//! Its fuzz target: any bytes, and lines made from the format's parts, parse within the caps,
//! and what parses is written back the same (`PROPTEST_CASES=200000` for a long run).
//!
//! Must not depend on: `app`; these use a scratch directory of their own.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use proptest::prelude::*;

use super::{FILE, Kept, MAX_SPACES, line, parse, parse_line, read, write};
use crate::sessions::identity::SessionId;
use crate::sessions::spaces::{Member, SpaceId, Stored};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oso-spaces-{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// oso's own directory in it, as the state directory gives it.
    fn oso(&self) -> PathBuf {
        self.0.join("oso")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn id(n: u8) -> SpaceId {
    SpaceId::from_random([n; 16])
}

fn space(n: u8) -> Stored {
    Stored::of_repository(
        id(n),
        format!("space-{n}"),
        Member::at(Path::new("/src/oso")),
        1_794_512_815_000,
    )
}

/// What the file holds, which must have been read.
fn held(scratch: &Scratch) -> Vec<Stored> {
    match read(&scratch.oso()) {
        Kept::Read(spaces) => spaces,
        other => panic!("not read: {other:?}"),
    }
}

fn known(ids: &[u8]) -> HashSet<SpaceId> {
    ids.iter().map(|n| id(*n)).collect()
}

#[test]
fn what_is_written_is_read_back_and_no_file_is_none() {
    let scratch = Scratch::new("round");
    assert_eq!(
        read(&scratch.oso()),
        Kept::Missing,
        "no file: made from the sessions"
    );
    let mut full = space(1);
    full.title = "Login error message".into();
    full.issue = "#123".into();
    full.members.push(Member {
        root: PathBuf::from("/src/front"),
        identity: Some((7, 42)),
    });
    full.directory = Some(PathBuf::from("/home/me/spaces/space-1"));
    full.directory_identity = Some((7, 43));
    full.archived = true;
    full.orchestrator = Some(SessionId::from_random([3; 16]));
    full.earlier = vec![SessionId::from_random([4; 16])];
    full.worktrees = vec![PathBuf::from("/src/oso/.claude/worktrees/x")];
    let spaces = [full, space(2)];
    write(&scratch.oso(), &spaces, &known(&[1, 2])).unwrap();
    assert_eq!(read(&scratch.oso()), Kept::Read(spaces.to_vec()));
    write(&scratch.oso(), &[], &known(&[1, 2])).unwrap();
    assert_eq!(
        read(&scratch.oso()),
        Kept::Read(Vec::new()),
        "an empty file is kept spaces"
    );
}

#[test]
fn another_instances_spaces_are_kept_and_ours_replace_theirs() {
    let scratch = Scratch::new("theirs");
    write(&scratch.oso(), &[space(1), space(2)], &known(&[1, 2])).unwrap();
    let mut changed = space(2);
    changed.title = "renamed".into();
    // This instance knows 2 and 3, not 1.
    write(
        &scratch.oso(),
        &[changed.clone(), space(3)],
        &known(&[2, 3]),
    )
    .unwrap();
    let back = held(&scratch);
    assert_eq!(back, [changed, space(3), space(1)]);
    // A space this instance knew and no longer has goes.
    write(&scratch.oso(), &[space(3)], &known(&[1, 2, 3])).unwrap();
    assert_eq!(held(&scratch), [space(3)]);
}

#[test]
fn a_bad_line_is_left_out_and_the_rest_read() {
    let good = format!(
        r#"{{"id":"{}","name":"ok","members":[{{"root":"/r"}}],"created":1,"changed":2}}"#,
        id(1).as_str()
    );
    let bad = [
        format!(
            r#"{{"id":"{}","name":"Not A Slug","created":1,"changed":2}}"#,
            id(2).as_str()
        ),
        format!(
            r#"{{"id":"{}","name":"rel","members":[{{"root":"r"}}],"created":1,"changed":2}}"#,
            id(3).as_str()
        ),
        format!(
            r#"{{"id":"{}","name":"t","title":"a\u001bb","created":1,"changed":2}}"#,
            id(4).as_str()
        ),
        r#"{"id":"short","name":"x","created":1,"changed":2}"#.to_owned(),
        format!(r#"{{"id":"{}","name":"n","created":1}}"#, id(5).as_str()),
        format!(
            r#"{{"id":"{}","name":"i","directory_identity":[1],"created":1,"changed":2}}"#,
            id(6).as_str()
        ),
        "not json".to_owned(),
    ];
    for line in &bad {
        assert_eq!(parse_line(line.as_bytes()), None, "{line}");
    }
    let parsed = parse_line(good.as_bytes()).unwrap();
    assert_eq!(parsed.members, [Member::at(Path::new("/r"))]);
    assert_eq!((parsed.title.as_str(), parsed.archived), ("", false));
    let scratch = Scratch::new("bad");
    let dir = oso_platform::privacy::PrivateDir::open(scratch.oso()).unwrap();
    let text = format!("{}\n{good}\n", bad.join("\n"));
    std::fs::write(dir.path().join(FILE), text).unwrap();
    let back = held(&scratch);
    assert_eq!(back.len(), 1);
}

#[test]
fn at_most_the_cap_is_written() {
    let scratch = Scratch::new("cap");
    let many: Vec<Stored> = (0..=MAX_SPACES)
        .map(|n| {
            let mut bytes = [0u8; 16];
            bytes[..8].copy_from_slice(&(n as u64).to_le_bytes());
            Stored::of_repository(
                SpaceId::from_random(bytes),
                format!("s{n}"),
                Member::at(Path::new("/r")),
                1,
            )
        })
        .collect();
    write(&scratch.oso(), &many, &HashSet::new()).unwrap();
    assert_eq!(held(&scratch).len(), MAX_SPACES);
}

#[test]
fn a_file_that_may_not_be_read_is_not_replaced() {
    let scratch = Scratch::new("refused");
    let dir = oso_platform::privacy::PrivateDir::open(scratch.oso()).unwrap();
    let file = dir.path().join(FILE);
    std::fs::write(&file, "x".repeat(300 * 1024)).unwrap();
    assert_eq!(
        read(&scratch.oso()),
        Kept::Refused,
        "refused, and not made again"
    );
    assert!(write(&scratch.oso(), &[space(1)], &known(&[1])).is_err());
    assert_eq!(std::fs::metadata(&file).unwrap().len(), 300 * 1024);
}

/// A value the format might hold, or not: strings of every kind, numbers, paths, ids.
fn field() -> impl Strategy<Value = serde_json::Value> {
    use serde_json::json;
    prop_oneof![
        any::<String>().prop_map(|text| json!(text)),
        "[a-z0-9-]{0,60}".prop_map(|text| json!(text)),
        "/[ -~]{0,40}".prop_map(|text| json!(text)),
        "[0-9a-f]{30,34}".prop_map(|text| json!(text)),
        any::<u64>().prop_map(|n| json!(n)),
        any::<bool>().prop_map(|b| json!(b)),
        Just(json!(null)),
        prop::collection::vec("/[a-z/]{0,20}", 0..20).prop_map(|paths| json!(paths)),
        prop::collection::vec(any::<u64>(), 0..3).prop_map(|pair| json!(pair)),
    ]
}

/// A line made from the format's keys, each with some value.
fn made_line() -> impl Strategy<Value = String> {
    let keys = [
        "id",
        "name",
        "title",
        "issue",
        "members",
        "directory",
        "directory_identity",
        "archived",
        "orchestrator",
        "earlier",
        "worktrees",
        "created",
        "changed",
    ];
    prop::collection::vec((prop::sample::select(keys.to_vec()), field()), 0..14).prop_map(|pairs| {
        let object: serde_json::Map<String, serde_json::Value> = pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
        serde_json::Value::Object(object).to_string()
    })
}

fn check_parsed(bytes: &[u8]) -> Result<(), TestCaseError> {
    let spaces = parse(bytes);
    prop_assert!(spaces.len() <= MAX_SPACES);
    for space in &spaces {
        prop_assert!(space.name.len() <= crate::sessions::spaces::stored::MAX_NAME);
        prop_assert!(!space.title.chars().any(char::is_control));
        let written = line(space).expect("a parsed space is written");
        let back = parse_line(written.as_bytes());
        prop_assert_eq!(back.as_ref(), Some(space));
    }
    Ok(())
}

proptest! {
    #[test]
    fn any_bytes_as_the_spaces_file_are_refused_or_read(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        check_parsed(&bytes)?;
    }

    #[test]
    fn made_lines_are_refused_or_read_back_the_same(lines in prop::collection::vec(made_line(), 0..8)) {
        check_parsed(lines.join("\n").as_bytes())?;
    }
}

#[test]
fn of_a_space_both_instances_have_the_one_changed_last_is_kept() {
    let scratch = Scratch::new("newer");
    let mut theirs = space(1);
    theirs.changed += 10;
    theirs.title = "theirs".into();
    write(&scratch.oso(), &[theirs.clone()], &known(&[1])).unwrap();
    // A stale copy does not bring back what the other changed.
    write(&scratch.oso(), &[space(1)], &known(&[1])).unwrap();
    assert_eq!(held(&scratch), [theirs.clone()]);
    let mut ours = theirs;
    ours.changed += 1;
    ours.title = "ours".into();
    write(&scratch.oso(), &[ours.clone()], &known(&[1])).unwrap();
    assert_eq!(held(&scratch), [ours]);
}

#[test]
fn a_space_too_large_for_the_file_is_left_out_and_the_rest_kept() {
    let scratch = Scratch::new("large");
    let mut large = space(1);
    let long = format!("/{}", "w".repeat(4000));
    large.worktrees = (0..100)
        .map(|n| PathBuf::from(format!("{long}{n}")))
        .collect();
    write(&scratch.oso(), &[large, space(2)], &known(&[1, 2])).unwrap();
    assert_eq!(held(&scratch), [space(2)]);
}
