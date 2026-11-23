//! Tests of reading a session's earlier turns from its transcript: where it is looked for,
//! which files are read, what a line still being written, a long one and the deadline do, on
//! transcripts written to a scratch directory.
//!
//! Must not depend on: `app`, or the person's own transcripts.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;
use crate::sessions::native::wire::transcript::Said;

const RECORDED: &str = include_str!("../native/fixtures/2.1.330/transcript.jsonl");

/// A directory of its own under the system's temporary one, removed when dropped: `projects`
/// and the session's `work` directory in it.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("oso-past-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("projects")).unwrap();
        std::fs::create_dir_all(dir.join("work")).unwrap();
        Self(dir)
    }

    fn projects(&self) -> PathBuf {
        self.0.join("projects")
    }

    fn work(&self) -> PathBuf {
        self.0.join("work")
    }

    /// Writes `text` as `id`'s transcript in the project directory `slug`.
    fn write(&self, slug: &str, id: &SessionId, text: &str) -> PathBuf {
        let dir = self.projects().join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}.jsonl", id.as_str()));
        std::fs::write(&path, text).unwrap();
        path
    }

    /// The recorded transcript, run in `directory`.
    fn recorded(directory: &Path) -> String {
        let cwd = serde_json::to_string(&directory.to_string_lossy()).unwrap();
        RECORDED.replace(r#""/w/proj""#, &cwd)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn id() -> SessionId {
    SessionId::parse("00000000-0000-4000-8000-0000000000aa").unwrap()
}

fn soon() -> Instant {
    Instant::now() + DEADLINE
}

fn prompts(past: &Past) -> Vec<&str> {
    (past.said.iter())
        .filter_map(|said| match said {
            Said::Prompt(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_transcript_under_its_directorys_slug_is_read() {
    let scratch = Scratch::new("slug");
    let work = scratch.work();
    scratch.write(&slug_of(&work), &id(), &Scratch::recorded(&work));
    let past = read_in(&scratch.projects(), &id(), &work, soon());
    assert_eq!(past.missing, None);
    assert_eq!(past.unreadable, 0);
    assert!(!past.cut);
    assert_eq!(prompts(&past).len(), 4);
    assert_eq!(prompts(&past)[0], "Reply with the single word apple.");
}

#[test]
fn a_transcript_elsewhere_is_found_and_one_of_another_directory_is_not_read() {
    let scratch = Scratch::new("elsewhere");
    let work = scratch.work();
    let path = scratch.write("-some-other-slug", &id(), &Scratch::recorded(&work));
    let past = read_in(&scratch.projects(), &id(), &work, soon());
    assert_eq!(past.missing, None);
    assert_eq!(prompts(&past).len(), 4);
    std::fs::write(&path, Scratch::recorded(Path::new("/somewhere/else"))).unwrap();
    let past = read_in(&scratch.projects(), &id(), &work, soon());
    assert_eq!(past.missing, Some(Missing::Elsewhere));
    assert!(past.said.is_empty());
}

#[test]
fn no_transcript_says_so() {
    let scratch = Scratch::new("none");
    let past = read_in(&scratch.projects(), &id(), &scratch.work(), soon());
    assert_eq!(past.missing, Some(Missing::NotFound));
    let past = read_in(&scratch.0.join("gone"), &id(), &scratch.work(), soon());
    assert_eq!(past.missing, Some(Missing::NotFound));
}

#[cfg(unix)]
#[test]
fn a_link_or_a_second_name_is_not_read() {
    let scratch = Scratch::new("link");
    let work = scratch.work();
    let real = scratch.0.join("real.jsonl");
    std::fs::write(&real, Scratch::recorded(&work)).unwrap();
    let dir = scratch.projects().join(slug_of(&work));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}.jsonl", id().as_str()));
    std::os::unix::fs::symlink(&real, &path).unwrap();
    let past = read_in(&scratch.projects(), &id(), &work, soon());
    assert_eq!(
        past.missing,
        Some(Missing::NotFound),
        "a link is no file here"
    );
    std::fs::remove_file(&path).unwrap();
    std::fs::hard_link(&real, &path).unwrap();
    let past = read_in(&scratch.projects(), &id(), &work, soon());
    assert_eq!(past.missing, Some(Missing::NotRead));
}

#[test]
fn a_line_being_written_or_with_no_fields_is_not_read() {
    let scratch = Scratch::new("partial");
    let work = scratch.work();
    let mut text = Scratch::recorded(&work);
    let last = text.trim_end().rsplit('\n').next().unwrap().to_owned();
    // The newest line again, cut short, and one that does not read.
    let bad = last.replace(r#""type":"assistant""#, r#""type":"assistant","#);
    text.push_str(&bad);
    text.push('\n');
    text.push_str(&last[..last.len() / 2]);
    scratch.write(&slug_of(&work), &id(), &text);
    let past = read_in(&scratch.projects(), &id(), &work, soon());
    assert_eq!(past.missing, None);
    assert_eq!(prompts(&past).len(), 4);
}

#[test]
fn a_line_that_does_not_decode_is_counted() {
    let scratch = Scratch::new("bad");
    let work = scratch.work();
    let mut text = Scratch::recorded(&work);
    let last = text.trim_end().rsplit('\n').next().unwrap().to_owned();
    // Placed by its fields, which read, but not one JSON object as a whole.
    let bad = last.replace(r#""text":"plum""#, r#""text":"plum\x""#);
    assert_ne!(bad, last);
    text = text.replace(&last, &bad);
    scratch.write(&slug_of(&work), &id(), &text);
    let past = read_in(&scratch.projects(), &id(), &work, soon());
    assert_eq!(past.unreadable, 1);
    assert_eq!(prompts(&past).len(), 4);
    assert_eq!(
        past.after(),
        ["1 line of its transcript did not read, and is not shown."]
    );
}

#[test]
fn a_read_past_its_deadline_shows_nothing() {
    let scratch = Scratch::new("slow");
    let work = scratch.work();
    let recorded = Scratch::recorded(&work);
    let line = recorded.lines().nth(1).unwrap();
    let text = format!("{line}\n").repeat(LINES_PER_LOOK * 2);
    scratch.write(&slug_of(&work), &id(), &text);
    let gone = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    let past = read_in(&scratch.projects(), &id(), &work, gone);
    assert_eq!(past.missing, Some(Missing::TooSlow));
}

#[test]
fn an_attachment_is_never_shown_and_a_transcript_naming_no_directory_is_not_read() {
    let scratch = Scratch::new("attachment");
    let work = scratch.work();
    let attachment = format!(
        r#"{{"parentUuid":null,"type":"attachment","attachment":{{"email":"someone@example.com"}},"uuid":"{}"}}"#,
        "00000000-0000-4000-8000-0000000000ff"
    );
    let text = Scratch::recorded(&work).replacen('\n', &format!("\n{attachment}\n"), 1);
    scratch.write(&slug_of(&work), &id(), &text);
    let past = read_in(&scratch.projects(), &id(), &work, soon());
    assert_eq!(past.missing, None);
    let said = format!("{:?}", past.said);
    assert!(!said.contains("someone@example.com"), "{said}");
    // With no line naming a directory, it is not known to be the session's.
    let anywhere: String = (RECORDED.lines())
        .map(|line| format!("{}\n", line.replace(r#""cwd":"/w/proj","#, "")))
        .collect();
    assert!(!anywhere.contains("\"cwd\""));
    scratch.write(&slug_of(&work), &id(), &anywhere);
    let past = read_in(&scratch.projects(), &id(), &work, soon());
    assert_eq!(past.missing, Some(Missing::Elsewhere));
}

#[cfg(unix)]
#[test]
fn a_project_directory_that_is_a_link_is_not_looked_in() {
    let scratch = Scratch::new("dir-link");
    let work = scratch.work();
    let real = scratch.0.join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(
        real.join(format!("{}.jsonl", id().as_str())),
        Scratch::recorded(&work),
    )
    .unwrap();
    std::os::unix::fs::symlink(&real, scratch.projects().join(slug_of(&work))).unwrap();
    let past = read_in(&scratch.projects(), &id(), &work, soon());
    assert_eq!(past.missing, Some(Missing::NotFound));
}
