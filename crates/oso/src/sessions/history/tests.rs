//! Tests of the search of conversations: which files are read and in which order, what of them
//! is searched, scopes, bounds, a file read again from where it grew, the preview's text and
//! the thread, on transcripts written to a scratch directory.
//!
//! Must not depend on: `app` or `dispatch`; or on the person's own transcripts.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use oso_core::editor::{ConversationPreviewRequest, ConversationScope, ConversationSearchRequest};

use super::*;
use crate::sessions::identity::SessionId;

/// A directory of its own under the system's temporary one, removed when dropped.
pub(super) struct Scratch(pub(super) PathBuf);

impl Scratch {
    pub(super) fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("oso-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// Writes `lines` as the transcript of `id` in `project`, changed `ago` before now.
    pub(super) fn write(
        &self,
        project: &str,
        id: &SessionId,
        lines: &[String],
        ago: u64,
    ) -> PathBuf {
        let dir = self.0.join(project);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}.jsonl", id.as_str()));
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(&path, text).unwrap();
        let when = SystemTime::now() - Duration::from_secs(ago);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(when)
            .unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn id(n: u8) -> SessionId {
    SessionId::from_random([n; 16])
}

pub(super) fn prompt(cwd: &str, text: &str) -> String {
    serde_json::json!({"type": "user", "cwd": cwd, "message": {"role": "user", "content": text}})
        .to_string()
}

fn reply(text: &str) -> String {
    serde_json::json!({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}})
        .to_string()
}

fn tool_result(text: &str) -> String {
    serde_json::json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": text}]}})
        .to_string()
}

pub(super) fn request(input: &str, scope: ConversationScope) -> ConversationSearchRequest {
    ConversationSearchRequest {
        generation: 1,
        scope,
        query: Arc::new(LineQuery::parse(input)),
        live: Arc::new(AtomicU64::new(1)),
    }
}

/// The rows a search of `hold` for `request` sends, as (session, message, text), and whether
/// it said it was cut.
pub(super) fn search(
    dir: &Path,
    hold: &mut Hold,
    request: &ConversationSearchRequest,
) -> (Vec<(String, usize, String)>, bool) {
    let found = Mutex::new(Vec::new());
    let send = |report: Report| found.lock().unwrap().push(report);
    let mut incoming = Incoming {
        queue: None,
        preview: None,
        read: None,
    };
    run_search(Some(dir), hold, request.clone(), &send, &mut incoming);
    let mut rows = Vec::new();
    let mut cut = None;
    for report in found.into_inner().unwrap() {
        let Report::Found(found) = report else {
            panic!("only rows");
        };
        for hit in found.hits {
            rows.push((
                hit.about.id.as_str().to_owned(),
                hit.message,
                hit.text.to_string(),
            ));
        }
        if let Some(end) = found.end {
            assert!(cut.is_none(), "one end");
            cut = Some(end.cut);
        }
    }
    (rows, cut.expect("an end"))
}

/// A tree of two conversations: an older one in `/w/a` and a newer one in a worktree of
/// `/w/b`, a tool result and a subagent's file that hold the word too, and a link.
pub(super) fn tree(scratch: &Scratch) -> (SessionId, SessionId) {
    let (older, newer) = (id(1), id(2));
    scratch.write(
        "-w-a",
        &older,
        &[
            prompt("/w/a", "find the trust bug"),
            tool_result("trust=secret"),
            reply("The trust record\nkeys by inode"),
        ],
        600,
    );
    let newer_path = scratch.write(
        "-w-b--claude-worktrees-x",
        &newer,
        &[
            r#"{"type":"ai-title","aiTitle":"Worktree trust"}"#.to_owned(),
            prompt("/w/b/.claude/worktrees/x", "does trust survive"),
        ],
        60,
    );
    let sub = scratch
        .0
        .join("-w-b--claude-worktrees-x")
        .join(newer.as_str())
        .join("subagents");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        sub.join("agent-1.jsonl"),
        prompt("/w/b", "trust from a subagent"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        &newer_path,
        scratch
            .0
            .join("-w-b--claude-worktrees-x")
            .join(format!("{}.jsonl", id(3).as_str())),
    )
    .unwrap();
    (older, newer)
}

#[test]
fn prompts_and_replies_are_found_newest_conversation_first() {
    let scratch = Scratch::new("order");
    let (older, newer) = tree(&scratch);
    let mut hold = Hold::default();
    let (rows, cut) = search(
        &scratch.0,
        &mut hold,
        &request("trust", ConversationScope::all()),
    );
    assert!(!cut);
    assert_eq!(
        rows,
        [
            (
                newer.as_str().to_owned(),
                0,
                "does trust survive".to_owned()
            ),
            (
                older.as_str().to_owned(),
                0,
                "find the trust bug".to_owned()
            ),
            (older.as_str().to_owned(), 1, "The trust record".to_owned()),
        ],
        "no tool result, subagent or link"
    );
    let about = &hold.conversations[0].about;
    assert_eq!(about.title.as_deref(), Some("Worktree trust"));
    assert_eq!(
        about.directory,
        Some(PathBuf::from("/w/b/.claude/worktrees/x"))
    );
    assert!(about.gone, "/w/b is no directory here");
    // Every component in one message, in any order, over its lines.
    let (rows, _) = search(
        &scratch.0,
        &mut hold,
        &request("inode record", ConversationScope::all()),
    );
    assert_eq!(
        rows,
        [(older.as_str().to_owned(), 1, "The trust record".to_owned())]
    );
    let (rows, _) = search(
        &scratch.0,
        &mut hold,
        &request("secret", ConversationScope::all()),
    );
    assert!(rows.is_empty(), "a tool result is never searched");
}

#[test]
fn a_scope_keeps_its_directories_and_its_sessions() {
    let scratch = Scratch::new("scope");
    let (older, newer) = tree(&scratch);
    let mut hold = Hold::default();
    let within = |roots: &[&str], sessions: &[&SessionId]| {
        ConversationScope::within(
            "space".into(),
            roots.iter().map(PathBuf::from).collect(),
            sessions.iter().map(|id| id.as_str().into()).collect(),
        )
    };
    let ids = |rows: Vec<(String, usize, String)>| {
        let mut ids: Vec<String> = rows.into_iter().map(|row| row.0).collect();
        ids.dedup();
        ids
    };
    let (rows, _) = search(
        &scratch.0,
        &mut hold,
        &request("trust", within(&["/w/b"], &[])),
    );
    assert_eq!(
        ids(rows),
        [newer.as_str()],
        "a worktree under the repository"
    );
    let (rows, _) = search(
        &scratch.0,
        &mut hold,
        &request("trust", within(&["/w/a"], &[])),
    );
    assert_eq!(ids(rows), [older.as_str()]);
    let (rows, _) = search(
        &scratch.0,
        &mut hold,
        &request("trust", within(&["/w/ab"], &[&newer])),
    );
    assert_eq!(ids(rows), [newer.as_str()], "by id; /w/ab holds no /w/a");
}

#[test]
fn a_file_that_grew_is_read_from_where_it_was() {
    let scratch = Scratch::new("grew");
    let (older, _) = tree(&scratch);
    let mut hold = Hold::default();
    search(
        &scratch.0,
        &mut hold,
        &request("trust", ConversationScope::all()),
    );
    let path = scratch
        .0
        .join("-w-a")
        .join(format!("{}.jsonl", older.as_str()));
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    // A line still being written is not read until it ends.
    write!(
        file,
        "{}\n{}",
        reply("trust again"),
        &prompt("/w/a", "trust half")[..20]
    )
    .unwrap();
    drop(file);
    hold.listed = None;
    let (rows, _) = search(
        &scratch.0,
        &mut hold,
        &request("again", ConversationScope::all()),
    );
    assert_eq!(
        rows,
        [(older.as_str().to_owned(), 2, "trust again".to_owned())]
    );
    let messages = &hold
        .conversations
        .iter()
        .find(|c| c.about.id == older)
        .unwrap()
        .messages;
    assert_eq!(messages.len(), 3, "read once each");
    // Without a new listing, nothing is read.
    std::fs::remove_file(&path).unwrap();
    let (rows, _) = search(
        &scratch.0,
        &mut hold,
        &request("again", ConversationScope::all()),
    );
    assert_eq!(rows.len(), 1, "held until the listing is old");
    hold.listed = None;
    let (rows, _) = search(
        &scratch.0,
        &mut hold,
        &request("again", ConversationScope::all()),
    );
    assert!(rows.is_empty(), "a file gone goes");
}

#[test]
fn a_conversation_gives_at_most_its_rows_and_a_superseded_search_nothing() {
    let scratch = Scratch::new("bounds");
    let many: Vec<String> = (0..PER_CONVERSATION + 10)
        .map(|n| prompt("/w/a", &format!("needle {n}")))
        .collect();
    scratch.write("-w-a", &id(4), &many, 10);
    let mut hold = Hold::default();
    let (rows, cut) = search(
        &scratch.0,
        &mut hold,
        &request("needle", ConversationScope::all()),
    );
    assert_eq!(rows.len(), PER_CONVERSATION);
    assert!(!cut, "a conversation's own limit is no cut");
    let stale = request("needle", ConversationScope::all());
    stale.live.store(2, std::sync::atomic::Ordering::Relaxed);
    let found = Mutex::new(Vec::new());
    let mut incoming = Incoming {
        queue: None,
        preview: None,
        read: None,
    };
    let send = |report| found.lock().unwrap().push(report);
    run_search(Some(&scratch.0), &mut hold, stale, &send, &mut incoming);
    assert!(found.lock().unwrap().is_empty());
}

#[test]
fn a_conversation_past_the_hold_is_a_stub_read_again_only_once_it_changed() {
    let scratch = Scratch::new("stub");
    let (older, newer) = tree(&scratch);
    let mut hold = Hold::default();
    search(
        &scratch.0,
        &mut hold,
        &request("trust", ConversationScope::all()),
    );
    let newest = hold.conversations[0].bytes;
    hold.most = newest;
    hold.listed = None;
    let (rows, cut) = search(
        &scratch.0,
        &mut hold,
        &request("trust", ConversationScope::all()),
    );
    assert!(cut, "a search past the hold says so");
    assert!(rows.iter().all(|row| row.0 == newer.as_str()), "{rows:?}");
    let stub = hold
        .conversations
        .iter()
        .find(|c| c.about.id == older)
        .unwrap();
    assert!(!stub.held && stub.messages.is_empty());
    // Unchanged, the stub stays one and is not read; room again, it is read once it changes.
    hold.most = MAX_HELD;
    hold.listed = None;
    let (rows, _) = search(
        &scratch.0,
        &mut hold,
        &request("trust", ConversationScope::all()),
    );
    assert!(rows.iter().all(|row| row.0 == newer.as_str()));
    let path = scratch
        .0
        .join("-w-a")
        .join(format!("{}.jsonl", older.as_str()));
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{}", reply("more trust")).unwrap();
    drop(file);
    hold.listed = None;
    let (rows, _) = search(
        &scratch.0,
        &mut hold,
        &request("trust", ConversationScope::all()),
    );
    assert_eq!(
        rows.iter().filter(|row| row.0 == older.as_str()).count(),
        3,
        "read whole"
    );
}

#[test]
fn a_newer_search_takes_over_while_the_transcripts_are_read() {
    let scratch = Scratch::new("newer");
    let (older, _) = tree(&scratch);
    let (jobs, queue) = mpsc::channel();
    let first = request("trust", ConversationScope::all());
    let newer = ConversationSearchRequest {
        generation: 2,
        ..request("record", ConversationScope::all())
    };
    newer.live.store(2, std::sync::atomic::Ordering::Relaxed);
    first.live.store(2, std::sync::atomic::Ordering::Relaxed);
    jobs.send(Job::Search(newer)).unwrap();
    let found = Mutex::new(Vec::new());
    let send = |report| found.lock().unwrap().push(report);
    let mut incoming = Incoming {
        queue: Some(&queue),
        preview: None,
        read: None,
    };
    let mut hold = Hold::default();
    run_search(Some(&scratch.0), &mut hold, first, &send, &mut incoming);
    let reports = found.into_inner().unwrap();
    let mut rows = Vec::new();
    for report in reports {
        let Report::Found(found) = report else {
            panic!("rows only");
        };
        assert_eq!(found.generation, 2, "only the newer search sends");
        rows.extend(found.hits.into_iter().map(|hit| hit.about.id.clone()));
    }
    assert_eq!(rows, [older]);
}

fn about(gone: bool) -> About {
    About {
        id: id(9),
        directory: Some(PathBuf::from("/w/oso")),
        directories: vec![PathBuf::from("/w/oso")],
        title: None,
        first_prompt: None,
        changed: SystemTime::now(),
        gone,
    }
}

fn preview_request(message: usize, line: usize, column: usize) -> ConversationPreviewRequest {
    ConversationPreviewRequest {
        generation: 1,
        session: "x".into(),
        message,
        line,
        column,
        rows: 20,
        columns: 60,
        live: Arc::new(AtomicU64::new(1)),
    }
}

#[test]
fn the_preview_names_the_directory_and_shows_the_messages_around_the_row() {
    let messages: Vec<Message> = ["one", "two\nthe needle", "three"]
        .iter()
        .enumerate()
        .map(|(n, text)| Message {
            prompt: n % 2 == 0,
            text: (*text).into(),
        })
        .collect();
    let (text, line, column) = preview::text(&about(false), &messages, &preview_request(1, 1, 4));
    assert_eq!(text, "in /w/oso\n\n› one\n\n‹ two\n  the needle\n\n› three");
    assert_eq!(text.lines().nth(line).unwrap(), "  the needle");
    assert_eq!(&text.lines().nth(line).unwrap()[column..], "needle");
    let (text, _, _) = preview::text(&about(true), &messages, &preview_request(0, 0, 0));
    assert!(text.starts_with("in /w/oso (gone)\n"), "{text}");
}

#[test]
fn a_long_line_before_the_match_is_cut_so_the_match_shows() {
    let long = format!("{}needle", "x".repeat(60_000));
    let messages = vec![
        Message {
            prompt: true,
            text: "y".repeat(60_000).into(),
        },
        Message {
            prompt: false,
            text: long.into(),
        },
    ];
    let (text, line, column) =
        preview::text(&about(false), &messages, &preview_request(1, 0, 60_000));
    let shown = text.lines().nth(line).unwrap();
    assert!(
        shown
            .chars()
            .skip(column)
            .collect::<String>()
            .starts_with("needle"),
        "{shown:.40}"
    );
    assert!(
        text.chars().count() < 2000,
        "only the room before the match is kept"
    );
}

#[test]
fn a_place_names_a_worktree_with_its_repository() {
    assert_eq!(
        place(Path::new("/w/oso/.claude/worktrees/lsp-8b")),
        "oso/lsp-8b"
    );
    assert_eq!(
        place(Path::new("/w/oso/.claude/worktrees/lsp-8b/crates")),
        "oso/lsp-8b"
    );
    assert_eq!(place(Path::new("/Users/me/spaces/oso-2")), "oso-2");
    assert_eq!(place(Path::new("/")), "/");
}

#[test]
fn the_thread_searches_and_previews() {
    let scratch = Scratch::new("thread");
    let (older, _) = tree(&scratch);
    let (sent, reports) = mpsc::channel();
    let sent = Mutex::new(sent);
    let mut history = History::new(Some(scratch.0.clone()), move |report| {
        let _ = sent.lock().unwrap().send(report);
    });
    history.search(request("record", ConversationScope::all()));
    let Report::Found(found) = reports.recv_timeout(Duration::from_secs(10)).unwrap() else {
        panic!("rows first");
    };
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.end, Some(WalkEnd { cut: false }));
    history.preview(ConversationPreviewRequest {
        generation: 7,
        session: older.as_str().into(),
        message: 1,
        line: 0,
        column: 4,
        rows: 10,
        columns: 40,
        live: Arc::new(AtomicU64::new(7)),
    });
    let Report::Previewed(previewed) = reports.recv_timeout(Duration::from_secs(10)).unwrap()
    else {
        panic!("the preview");
    };
    assert_eq!(previewed.generation, 7);
    assert!(matches!(previewed.read, FilePreviewRead::Text(_)));
    let debug = format!("{found:?} {previewed:?}");
    assert!(!debug.contains("record"), "{debug}");
}
