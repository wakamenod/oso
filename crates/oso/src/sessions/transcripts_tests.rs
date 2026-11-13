//! Tests of the transcripts reader: which files are opened, what of them is read, and its
//! bounds and cache, on transcripts written to a scratch directory.
//!
//! Must not depend on: `app` or `dispatch`; or on the person's own transcripts.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{Cache, HEAD, MAX_LISTED, RECENT, TAIL, list, title, top_level_string, whole_lines};
use crate::sessions::identity::SessionId;

/// A directory of its own under the system's temporary one, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("oso-transcripts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// Writes `text` as `project/name`, making the project's directory.
    fn write(&self, project: &str, name: &str, text: &str) -> PathBuf {
        let dir = self.0.join(project);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn id(n: u8) -> SessionId {
    SessionId::from_random([n; 16])
}

fn user_line(cwd: &str) -> String {
    format!(
        "{{\"parentUuid\":null,\"message\":{{\"role\":\"user\",\"content\":\"hi\"}},\"cwd\":\"{cwd}\",\"sessionId\":\"x\"}}"
    )
}

fn listed(dir: &Path, cache: &mut Cache) -> Vec<(String, PathBuf, Option<String>)> {
    list(dir, SystemTime::now(), |_| false, cache)
        .into_iter()
        .map(|c| (c.id.to_string(), c.directory, c.title))
        .collect()
}

#[test]
fn a_top_level_key_is_found_and_nested_ones_and_text_are_not() {
    let line = br#"{"message":{"cwd":"/nested","text":"\"cwd\":\"/in-text\""},"cwd":"/p/a\u00e9","x":[{"cwd":"/arr"}]}"#;
    assert_eq!(top_level_string(line, "cwd").as_deref(), Some("/p/a\u{e9}"));
    let nested_only = br#"{"message":{"cwd":"/nested"},"a":"\"cwd\":\"/t\""}"#;
    assert_eq!(top_level_string(nested_only, "cwd"), None);
    assert_eq!(top_level_string(br#"["cwd","/x"]"#, "cwd"), None);
    assert_eq!(top_level_string(br#"{"cwd":3}"#, "cwd"), None);
    assert_eq!(top_level_string(br#"{"cwd":"/unterminated"#, "cwd"), None);
    // A value equal to the key is not taken as the key.
    assert_eq!(
        top_level_string(br#"{"a":"cwd","cwd":"/r"}"#, "cwd").as_deref(),
        Some("/r")
    );
}

#[test]
fn a_title_is_the_best_kind_and_the_last_of_it() {
    let lines: Vec<&[u8]> = vec![
        br#"{"type":"ai-title","aiTitle":"made by claude","sessionId":"x"}"#,
        br#"{"type":"agent-name","agentName":"agent","sessionId":"x"}"#,
        br#"{"type":"custom-title","customTitle":"first","sessionId":"x"}"#,
        br#"{"type":"custom-title","customTitle":"renamed","sessionId":"x"}"#,
    ];
    assert_eq!(title(lines.iter().copied()).as_deref(), Some("renamed"));
    let ai: Vec<&[u8]> = vec![br#"{"type":"ai-title","aiTitle":"made by claude"}"#];
    assert_eq!(title(ai.into_iter()).as_deref(), Some("made by claude"));
    // A message whose text holds a title line's words does not start as one.
    let text: Vec<&[u8]> = vec![
        br#"{"message":"{\"type\":\"custom-title\",\"customTitle\":\"spoof\"}","customTitle":"no"}"#,
        br#"{"type":"user","customTitle":"no"}"#,
    ];
    assert_eq!(title(text.into_iter()), None);
    let empty: Vec<&[u8]> = vec![br#"{"type":"custom-title","customTitle":"  "}"#];
    assert_eq!(title(empty.into_iter()), None);
}

#[test]
fn only_whole_lines_are_read() {
    assert_eq!(whole_lines(b"a\nb\nc", true, true), [&b"a"[..], b"b", b"c"]);
    assert_eq!(whole_lines(b"a\nb\nc", true, false), [&b"a"[..], b"b"]);
    assert_eq!(whole_lines(b"a\nb\nc", false, true), [&b"b"[..], b"c"]);
    assert!(whole_lines(b"", true, true).is_empty());
}

#[test]
fn a_conversation_is_its_uuid_file_with_its_directory_and_name() {
    let scratch = Scratch::new("listed");
    let text = format!(
        "{{\"type\":\"custom-title\",\"customTitle\":\"fix tests\",\"sessionId\":\"x\"}}\n{}\n",
        user_line("/p/oso")
    );
    scratch.write("-p-oso", &format!("{}.jsonl", id(1)), &text);
    // Not a session id as oso writes one, a key file, a subagent's, and no directory.
    scratch.write(
        "-p-oso",
        &format!("{}.jsonl", id(0xab).as_str().to_uppercase()),
        &text,
    );
    scratch.write("-p-oso", "notes.jsonl", &text);
    scratch.write("-p-oso", &format!("{}.key", id(3)), &text);
    scratch.write(
        &format!("-p-oso/{}/subagents", id(1)),
        &format!("{}.jsonl", id(4)),
        &text,
    );
    scratch.write(
        "-p-oso",
        &format!("{}.jsonl", id(5)),
        "{\"type\":\"user\"}\n",
    );
    // A relative directory is none.
    scratch.write("-p-rel", &format!("{}.jsonl", id(6)), &user_line("p/rel"));
    let found = listed(&scratch.0, &mut Cache::default());
    assert_eq!(
        found,
        [(
            id(1).to_string(),
            PathBuf::from("/p/oso"),
            Some("fix tests".to_owned())
        )]
    );
}

#[test]
fn a_link_is_not_followed() {
    let scratch = Scratch::new("link");
    let real = scratch.write("elsewhere", "real.txt", &user_line("/p/oso"));
    let dir = scratch.0.join("-p-oso");
    std::fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(&real, dir.join(format!("{}.jsonl", id(1)))).unwrap();
    assert!(listed(&scratch.0, &mut Cache::default()).is_empty());
}

#[test]
fn a_late_rename_is_found_in_the_tail_and_the_middle_is_not_read() {
    let scratch = Scratch::new("tail");
    let filler = format!(
        "{{\"type\":\"user\",\"message\":\"{}\"}}\n",
        "x".repeat(1000)
    );
    let mut text = user_line("/p/oso") + "\n";
    // A rename in the middle, beyond the head and before the tail, is not seen.
    while (text.len() as u64) < HEAD + 10_000 {
        text.push_str(&filler);
    }
    text.push_str("{\"type\":\"custom-title\",\"customTitle\":\"middle\"}\n");
    while (text.len() as u64) < HEAD + TAIL + 50_000 {
        text.push_str(&filler);
    }
    let mut late = text.clone();
    late.push_str("{\"type\":\"custom-title\",\"customTitle\":\"late\"}\n");
    late.push_str(&filler);
    scratch.write("-p-oso", &format!("{}.jsonl", id(1)), &text);
    scratch.write("-p-oso", &format!("{}.jsonl", id(2)), &late);
    let mut found = listed(&scratch.0, &mut Cache::default());
    found.sort();
    let titles: Vec<Option<&str>> = found.iter().map(|(_, _, t)| t.as_deref()).collect();
    let (first, second) = if id(1) < id(2) { (0, 1) } else { (1, 0) };
    assert_eq!(titles[first], None);
    assert_eq!(titles[second], Some("late"));
}

#[test]
fn old_ones_and_left_out_ones_are_not_listed_and_at_most_so_many_are() {
    let scratch = Scratch::new("cut");
    for n in 0..(MAX_LISTED as u8 + 5) {
        scratch.write("-p-oso", &format!("{}.jsonl", id(n)), &user_line("/p/oso"));
    }
    let all = list(
        &scratch.0,
        SystemTime::now(),
        |_| false,
        &mut Cache::default(),
    );
    assert_eq!(all.len(), MAX_LISTED);
    let left = list(
        &scratch.0,
        SystemTime::now(),
        |found| *found == id(0),
        &mut Cache::default(),
    );
    assert!(left.iter().all(|c| c.id != id(0)));
    let later = SystemTime::now() + RECENT + Duration::from_secs(60);
    assert!(list(&scratch.0, later, |_| false, &mut Cache::default()).is_empty());
}

#[test]
fn an_unchanged_file_is_not_read_again() {
    let scratch = Scratch::new("cache");
    let path = scratch.write("-p-oso", &format!("{}.jsonl", id(1)), &user_line("/p/oso"));
    let mut cache = Cache::default();
    assert_eq!(listed(&scratch.0, &mut cache).len(), 1);
    // Unreadable now, but unchanged by length and time: the cache answers.
    let meta = std::fs::metadata(&path).unwrap();
    std::fs::write(&path, user_line("/p/xyz")).unwrap();
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(meta.modified().unwrap()).unwrap();
    assert_eq!(listed(&scratch.0, &mut cache)[0].1, PathBuf::from("/p/oso"));
    // Changed: read again.
    std::fs::write(&path, user_line("/p/other")).unwrap();
    assert_eq!(
        listed(&scratch.0, &mut cache)[0].1,
        PathBuf::from("/p/other")
    );
}
