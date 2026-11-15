//! Tests of reading a listing on scratch directories.
//!
//! Must not depend on: `app` or winit.

use std::fs;
use std::path::PathBuf;

use super::super::{Answer, Options, Request, Sort};
use super::{header_of, list, reaches_out, shown_name};

struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!("osodired-{}-{test}", std::process::id()));
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

fn request(directory: PathBuf, sort: Sort) -> Request {
    Request {
        buffer: oso_core::editor::Editor::default().window().buffer(),
        generation: 7,
        directory,
        options: Options {
            sort,
            details: true,
        },
        home: None,
        partials: true,
        marked: std::sync::Arc::default(),
        invert: false,
    }
}

fn answers(request: &Request) -> Vec<Answer> {
    let mut answers = Vec::new();
    list(request, &|| false, &mut |answer| answers.push(answer));
    answers
}

#[test]
fn a_listing_reads_every_entry_with_dots_first_and_no_backups() {
    let scratch = Scratch::new("read");
    fs::write(scratch.0.join("b.txt"), b"12").unwrap();
    fs::write(scratch.0.join("a.txt~"), b"").unwrap();
    fs::create_dir(scratch.0.join("sub")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("sub", scratch.0.join("to-sub")).unwrap();
    let answers = answers(&request(scratch.0.clone(), Sort::Name));
    let last = answers.last().unwrap();
    assert_eq!(last.generation, 7);
    let built = last.built.as_ref().unwrap();
    assert!(built.done);
    let names: Vec<_> = built
        .rows
        .iter()
        .map(|row| row.name.to_str().unwrap())
        .collect();
    let expected: &[&str] = if cfg!(unix) {
        &[".", "..", "sub", "to-sub", "b.txt"]
    } else {
        &[".", "..", "sub", "b.txt"]
    };
    assert_eq!(names, expected);
    let text = built.text.text().to_string();
    assert!(!text.contains("a.txt~"));
    let face_at = |needle: &str| built.highlights.at(text.find(needle).unwrap());
    assert_eq!(
        face_at("sub\n"),
        Some(oso_core::syntax::Highlight::DiredDirectory),
        "{text}"
    );
    #[cfg(unix)]
    assert!(text.contains("to-sub -> sub"), "{text}");
    assert!(built.resolved.is_some());
}

#[test]
fn a_missing_directory_says_why() {
    let scratch = Scratch::new("missing");
    let answers = answers(&request(scratch.0.join("nowhere"), Sort::Name));
    assert_eq!(answers.len(), 1);
    let built = answers[0].built.as_ref().unwrap();
    assert!(built.rows.is_empty());
    let text = built.text.text().to_string();
    assert!(
        text.lines()
            .nth(1)
            .unwrap()
            .starts_with("  cannot be read: "),
        "{text}"
    );
}

#[test]
fn a_header_writes_home_as_a_tilde() {
    let home = PathBuf::from("/home/me");
    assert_eq!(
        header_of(&PathBuf::from("/home/me/src"), Some(&home)),
        "~/src/"
    );
    assert_eq!(header_of(&home, Some(&home)), "~/");
    assert_eq!(header_of(&PathBuf::from("/etc"), Some(&home)), "/etc/");
    assert_eq!(header_of(&PathBuf::from("/"), None), "/");
}

#[test]
fn a_database_name_shows_only_when_it_is_plain_ascii() {
    assert_eq!(shown_name(Some("root".to_owned()), 0), "root");
    assert_eq!(shown_name(Some("b\u{202e}d".to_owned()), 12), "12");
    assert_eq!(shown_name(Some("a b".to_owned()), 13), "13");
    assert_eq!(shown_name(None, 14), "14");
}

#[test]
fn a_link_that_may_reach_another_machine_is_not_followed() {
    let here = PathBuf::from("/Users/me/src");
    assert!(reaches_out(&here, &PathBuf::from("//host/share")));
    assert!(reaches_out(&here, &PathBuf::from("\\\\host\\share")));
    #[cfg(unix)]
    {
        assert!(reaches_out(&here, &PathBuf::from("/net/host/x")));
        assert!(reaches_out(&here, &PathBuf::from("../../../net/host")));
        assert!(!reaches_out(&here, &PathBuf::from("../lib")));
        assert!(!reaches_out(&here, &PathBuf::from("/usr/local")));
        assert!(!reaches_out(&here, &PathBuf::from("network")));
    }
    #[cfg(windows)]
    {
        let here = PathBuf::from(r"C:\Users\me\src");
        assert!(!reaches_out(&here, &PathBuf::from(r"..\lib")));
        assert!(!reaches_out(&here, &PathBuf::from(r"D:\work")));
        assert!(reaches_out(&here, &PathBuf::from(r"\\?\UNC\host\share")));
    }
}

#[test]
fn a_stale_listing_stops_and_sends_nothing() {
    let scratch = Scratch::new("stale");
    for index in 0..600 {
        fs::write(scratch.0.join(format!("f{index:04}")), b"").unwrap();
    }
    let mut sent = 0;
    list(
        &request(scratch.0.clone(), Sort::Name),
        &|| true,
        &mut |_| sent += 1,
    );
    assert_eq!(sent, 0);
}

#[test]
fn rows_are_found_by_what_they_are_ordered_by() {
    use super::super::{Row, position, position_of_name};
    let row = |name: &str, enters: bool| Row {
        name: name.into(),
        enters,
        modified: None,
        face: None,
        name_len: name.len(),
    };
    let rows = vec![
        row(".", true),
        row("..", true),
        row("alpha", true),
        row("zeta", true),
        row("A.md", false),
        row("b.md", false),
    ];
    for (index, shown) in rows.iter().enumerate() {
        assert_eq!(position(&rows, Sort::Name, shown), Some(index), "{shown:?}");
    }
    // One whose kind changed is still found, by its name.
    assert_eq!(position(&rows, Sort::Name, &row("b.md", true)), Some(5));
    assert_eq!(position(&rows, Sort::Name, &row("gone", false)), None);
    assert_eq!(position_of_name(&rows, "zeta".as_ref()), Some(3));
}

#[test]
fn an_inverted_listing_marks_every_other_row_and_says_which() {
    let scratch = Scratch::new("invert");
    for name in ["a", "b", "c"] {
        fs::write(scratch.0.join(name), b"").unwrap();
    }
    let mut asked = request(scratch.0.clone(), Sort::Name);
    asked.marked = std::sync::Arc::new(["b".into()].into_iter().collect());
    asked.invert = true;
    let answers = answers(&asked);
    let built = answers.last().unwrap().built.as_ref().unwrap();
    let names = built.marked_names.as_ref().unwrap();
    let mut names: Vec<_> = names.iter().map(|name| name.to_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(names, ["a", "c"]);
    assert_eq!(built.marked, 2);
}
