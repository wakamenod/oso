//! Tests of `transcript`: finding a transcript without following links, reading its tail, and
//! the paths its edits touched, from made-up lines and arbitrary bytes.
//!
//! Must not depend on: a real Claude Code configuration; each test makes its own under the
//! temporary directory.

use std::path::{Path, PathBuf};

use proptest::prelude::*;

#[cfg(unix)]
use super::Touched;
use super::{MAX_TOUCHED, TAIL, config_dir, find, read_tail, touched};

const ID: &str = "0b5c7a5e-1f2d-4c3b-9a8e-7d6c5b4a3f21";

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "oso-transcript-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("projects")).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_config_dir_is_absolute() {
    // The environment is the process's, so this only checks the shape of what is found.
    assert!(config_dir().is_none_or(|dir| dir.is_absolute()));
}

#[test]
fn the_right_file_is_found() {
    let scratch = Scratch::new("found");
    let dir = scratch.0.join("projects/-repo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(scratch.0.join("projects/-other")).unwrap();
    std::fs::write(scratch.0.join("projects/stray.jsonl"), "").unwrap();
    let file = dir.join(format!("{ID}.jsonl"));
    std::fs::write(&file, "{}\n").unwrap();
    assert_eq!(find(&scratch.0, ID), Some(file));
    assert_eq!(
        find(&scratch.0, "0b5c7a5e-0000-0000-0000-000000000000"),
        None
    );
    for bad in ["", "../x", "a/b", "*", &"a".repeat(65)] {
        assert_eq!(find(&scratch.0, bad), None, "{bad:?}");
    }
    assert_eq!(find(&scratch.0.join("nothing"), ID), None);
}

#[cfg(unix)]
#[test]
fn a_linked_directory_is_not_followed() {
    let scratch = Scratch::new("dirlink");
    let elsewhere = scratch.0.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join(format!("{ID}.jsonl")), "{}\n").unwrap();
    std::os::unix::fs::symlink(&elsewhere, scratch.0.join("projects/-linked")).unwrap();
    assert_eq!(find(&scratch.0, ID), None);
}

#[cfg(unix)]
#[test]
fn a_linked_file_is_not_followed() {
    let scratch = Scratch::new("filelink");
    let real = scratch.0.join("real.jsonl");
    std::fs::write(&real, "{}\n").unwrap();
    let dir = scratch.0.join("projects/-repo");
    std::fs::create_dir_all(&dir).unwrap();
    let link = dir.join(format!("{ID}.jsonl"));
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(find(&scratch.0, ID), None);
    assert!(read_tail(&link).unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn a_linked_projects_directory_is_not_followed() {
    let scratch = Scratch::new("projectslink");
    let other = scratch.0.join("other");
    std::fs::create_dir_all(other.join("-repo")).unwrap();
    std::fs::write(other.join(format!("-repo/{ID}.jsonl")), "{}\n").unwrap();
    let config = scratch.0.join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::os::unix::fs::symlink(&other, config.join("projects")).unwrap();
    assert_eq!(find(&config, ID), None);
}

#[test]
fn a_short_file_is_read_whole() {
    let scratch = Scratch::new("short");
    let file = scratch.0.join("t.jsonl");
    std::fs::write(&file, b"line one\nline two\n").unwrap();
    assert_eq!(
        read_tail(&file).unwrap(),
        Some((b"line one\nline two\n".to_vec(), false))
    );
    assert!(read_tail(&scratch.0).unwrap().is_none());
    assert!(read_tail(&scratch.0.join("missing")).is_err());
}

#[test]
fn a_long_file_is_cut() {
    let scratch = Scratch::new("long");
    let file = scratch.0.join("t.jsonl");
    let tail = usize::try_from(TAIL).unwrap();
    let mut bytes = vec![b'a'; 100];
    bytes.extend(std::iter::repeat_n(b'b', tail));
    std::fs::write(&file, &bytes).unwrap();
    let (read, cut) = read_tail(&file).unwrap().unwrap();
    assert!(cut);
    assert_eq!(read.len(), tail);
    assert!(read.iter().all(|&b| b == b'b'));
}

#[cfg(unix)]
fn edit(name: &str, key: &str, path: &str) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"{name}","input":{{"{key}":"{path}"}}}}]}}}}"#
    )
}

// Absolute paths in these lines are Unix ones; the parser is the same everywhere.
#[cfg(unix)]
#[test]
fn touched_files_and_plans() {
    let lines = [
        edit("Edit", "file_path", "/abs/a.rs"),
        "not json".to_owned(),
        edit("Write", "file_path", "/abs/b.rs"),
        edit("Edit", "file_path", "/abs/a.rs"),
        edit("Read", "file_path", "/abs/read.rs"),
        edit("MultiEdit", "file_path", "relative.rs"),
        edit("NotebookEdit", "notebook_path", "/abs/n.ipynb"),
        edit("Write", "file_path", "/cfg/plans/plan.md"),
        edit("Write", "file_path", "/cfg/plans/notes.txt"),
        edit("Write", "file_path", "/cfg/plans/../x.md"),
        edit("Edit", "file_path", "/abs/\\u001b[2J"),
        edit("Edit", "file_path", ""),
        r#"{"message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":7}}]}}"#
            .to_owned(),
        r#"{"message":{"content":"text"}}"#.to_owned(),
        r#"{"message":{"content":[{"type":"text","name":"Edit","input":{"file_path":"/abs/t"}}]}}"#
            .to_owned(),
        r#"{"message":{"content":[{"type":"tool_use","name":"Edit","input":"/abs/s"}]}}"#
            .to_owned(),
        r#"[1,2,3]"#.to_owned(),
    ];
    let tail = lines.join("\n");
    assert_eq!(
        touched(tail.as_bytes(), false, Path::new("/cfg/plans")),
        Touched {
            files: [
                "/abs/a.rs",
                "/abs/b.rs",
                "/abs/n.ipynb",
                "/cfg/plans/notes.txt"
            ]
            .map(PathBuf::from)
            .to_vec(),
            plans: vec![PathBuf::from("/cfg/plans/plan.md")],
        }
    );
}

// Absolute paths in these lines are Unix ones; the parser is the same everywhere.
#[cfg(unix)]
#[test]
fn a_cut_tail_skips_its_first_line() {
    let tail = format!(
        "{}\n{}\n",
        edit("Edit", "file_path", "/abs/first"),
        edit("Edit", "file_path", "/abs/second")
    );
    let plans = Path::new("/p");
    assert_eq!(
        touched(tail.as_bytes(), true, plans).files,
        [PathBuf::from("/abs/second")]
    );
    assert_eq!(touched(tail.as_bytes(), false, plans).files.len(), 2);
    assert_eq!(touched(b"no line break", true, plans), Touched::default());
}

// Absolute paths in these lines are Unix ones; the parser is the same everywhere.
#[cfg(unix)]
#[test]
fn touched_is_bounded() {
    let tail: Vec<String> = (0..MAX_TOUCHED + 50)
        .map(|i| edit("Edit", "file_path", &format!("/abs/{i}")))
        .collect();
    let found = touched(tail.join("\n").as_bytes(), false, Path::new("/p"));
    assert_eq!(found.files.len(), MAX_TOUCHED);
    let long = format!("/{}", "a".repeat(4096));
    let found = touched(
        edit("Edit", "file_path", &long).as_bytes(),
        false,
        Path::new("/p"),
    );
    assert!(found.files.is_empty());
}

proptest! {
    #[test]
    fn any_bytes_parse_without_panic(bytes in proptest::collection::vec(any::<u8>(), 0..1024), cut: bool) {
        let found = touched(&bytes, cut, Path::new("/p"));
        prop_assert!(found.files.len() + found.plans.len() <= MAX_TOUCHED);
    }

    #[test]
    fn any_lines_of_json_parse_without_panic(paths in proptest::collection::vec(".{0,20}", 0..20)) {
        let tail: Vec<String> = paths
            .iter()
            .map(|p| serde_json::json!({"message": {"content": [{"type": "tool_use", "name": "Edit", "input": {"file_path": p}}]}}).to_string())
            .collect();
        let found = touched(tail.join("\n").as_bytes(), false, Path::new("/p"));
        for path in found.files.iter().chain(&found.plans) {
            prop_assert!(path.is_absolute());
        }
    }
}
