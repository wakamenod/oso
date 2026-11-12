//! The tests of searching a project, on scratch directories and in memory.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::time::Duration;

use super::read::search_file;
use super::*;
use oso_core::project::{BLOCK, LineQuery, MAX_LINE, MAX_ROWS};
use std::sync::{Mutex, PoisonError};

/// The lines `query` finds in `text`, read as a file: each line's number and text.
fn lines_in(query: &str, text: &[u8]) -> Vec<(usize, String)> {
    let query = LineQuery::parse(query);
    let mut found = Vec::new();
    let mut hit = |line, bytes: &[u8], _| {
        found.push((line, String::from_utf8_lossy(bytes).into_owned()));
        true
    };
    search_file(text, &query, &|| true, &mut hit, &mut Vec::new());
    found
}

#[test]
fn lines_are_found_and_numbered_across_blocks() {
    let mut text = Vec::new();
    for line in 1..=40_000 {
        text.extend_from_slice(format!("line {line} of the file\n").as_bytes());
    }
    assert!(text.len() > 3 * BLOCK);
    let found = lines_in("39999 of", &text);
    assert_eq!(found, [(39_999, "line 39999 of the file".to_owned())]);
    let found = lines_in("^line\\ 1\\ ", &text);
    assert_eq!(found, [(1, "line 1 of the file".to_owned())]);
    // The last line, with no line break.
    assert_eq!(
        lines_in("end", b"a\nb\nthe end"),
        [(3, "the end".to_owned())]
    );
    assert_eq!(lines_in("two", b"one\r\ntwo\r\n"), [(2, "two".to_owned())]);
}

#[test]
fn a_line_longer_than_a_block_grows_it_and_past_its_bound_the_file_is_skipped() {
    let mut text = b"short\n".to_vec();
    text.extend(std::iter::repeat_n(b'a', 3 * BLOCK));
    text.extend_from_slice(b"needle\nafter needle\n");
    let found = lines_in("needle", &text);
    assert_eq!(found.len(), 2);
    assert_eq!((found[0].0, found[0].1.len()), (2, 3 * BLOCK + 6));
    assert_eq!(found[1], (3, "after needle".to_owned()));
    let mut text = b"needle first\n".to_vec();
    text.extend(std::iter::repeat_n(b'a', MAX_LINE + 1));
    text.extend_from_slice(b"\nneedle never\n");
    assert_eq!(lines_in("needle", &text), [(1, "needle first".to_owned())]);
}

#[test]
fn a_nul_stops_the_file_and_keeps_the_rows_before_it() {
    let found = lines_in("x", b"x one\nx two\nx\0three\nx four\n");
    assert_eq!(found, [(1, "x one".to_owned()), (2, "x two".to_owned())]);
    // In a later block too.
    let mut text = Vec::new();
    while text.len() < 2 * BLOCK {
        text.extend_from_slice(b"x row\n");
    }
    let before = text.len() / 6;
    text.extend_from_slice(b"\0x not\n");
    assert_eq!(lines_in("x", &text).len(), before);
}

#[test]
fn a_search_that_is_over_reads_no_more_blocks() {
    let text = "x\n".repeat(BLOCK);
    let query = LineQuery::parse("x");
    let reads = std::cell::Cell::new(0);
    search_file(
        text.as_bytes(),
        &query,
        &|| {
            reads.set(reads.get() + 1);
            reads.get() < 2
        },
        &mut |_, _, _| true,
        &mut Vec::new(),
    );
    assert_eq!(reads.get(), 2, "one block, then it stops");
}

/// A scratch directory with no links in its path, removed at the end of the test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let name = format!("oso-grep-test-{}-{test}", std::process::id());
        let path = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn file(&self, relative: &str, content: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A searcher, the rows it sends, and the live generation its requests share.
struct Harness {
    grep: Grep,
    found: mpsc::Receiver<Found>,
    live: Arc<AtomicU64>,
}

impl Harness {
    fn new() -> Self {
        let (send, found) = mpsc::channel();
        let send = Mutex::new(send);
        let grep = Grep::new(None, move |found| {
            send.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send(found)
                .is_ok()
        });
        Self {
            grep,
            found,
            live: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Searches `root` for `input` as the search `generation`, narrowing `narrow`.
    fn search(&mut self, root: &Scratch, input: &str, generation: u64, narrow: Option<u64>) {
        self.live.store(generation, Ordering::Relaxed);
        self.grep.search(SearchRequest {
            generation,
            root: root.0.clone(),
            query: Arc::new(LineQuery::parse(input)),
            live: Arc::clone(&self.live),
            narrow,
        });
    }

    /// The rows of the search `generation` until it ends, as `path:line: text`, sorted, and
    /// whether it was cut.
    fn rows(&self, generation: u64) -> (Vec<String>, bool) {
        let mut rows = Vec::new();
        loop {
            let found = self.found.recv_timeout(Duration::from_secs(10)).unwrap();
            if found.generation != generation {
                continue;
            }
            rows.extend(found.rows.into_iter().map(|row| {
                let text = if row.secret { "(secret)" } else { &row.text };
                format!("{}:{}:{}: {text}", row.path, row.line, row.column)
            }));
            if let Some(end) = found.end {
                rows.sort();
                return (rows, end.cut);
            }
        }
    }
}

#[test]
fn a_project_is_searched_as_it_is_walked_and_a_files_rows_come_in_order() {
    let scratch = Scratch::new("search");
    scratch.file(
        "src/main.rs",
        "fn main() {\n    needle();\n}\n// needle again\n",
    );
    scratch.file("src/lib.rs", "pub fn needle() {}\n");
    scratch.file("ignored/x.rs", "needle\n");
    scratch.file(".ignore", "ignored/\n");
    scratch.file(".git/HEAD", "needle\n");
    let mut harness = Harness::new();
    harness.search(&scratch, "needle", 1, None);
    let (rows, cut) = harness.rows(1);
    assert_eq!(
        rows,
        [
            "src/lib.rs:1:7: pub fn needle() {}",
            "src/main.rs:2:4:     needle();",
            "src/main.rs:4:3: // needle again",
        ]
    );
    assert!(!cut);
}

#[cfg(unix)]
#[test]
fn a_link_is_not_searched_and_a_secret_files_rows_carry_no_text() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("search-links");
    let outside = Scratch::new("search-links-outside");
    outside.file("id_rsa", "needle outside\n");
    scratch.file("a.txt", "needle here\n");
    scratch.file(".env", "TOKEN=needle\n");
    symlink(outside.0.join("id_rsa"), scratch.0.join("key")).unwrap();
    symlink("a.txt", scratch.0.join("to-a")).unwrap();
    let mut harness = Harness::new();
    harness.search(&scratch, "needle", 1, None);
    let (rows, _) = harness.rows(1);
    assert_eq!(rows, [".env:1:6: (secret)", "a.txt:1:0: needle here"]);
}

#[test]
fn a_search_stops_at_its_rows_and_says_it_was_cut() {
    let scratch = Scratch::new("search-rows");
    for file in 0..3 {
        scratch.file(&format!("f{file}.txt"), &"x row\n".repeat(MAX_ROWS / 2));
    }
    let mut harness = Harness::new();
    harness.search(&scratch, "row", 1, None);
    let (rows, cut) = harness.rows(1);
    assert_eq!((rows.len(), cut), (MAX_ROWS, true));
}

#[test]
fn a_newer_search_stops_the_one_before_and_a_narrowing_one_looks_at_its_lines() {
    let scratch = Scratch::new("search-narrow");
    scratch.file("a.txt", "foo one\nfoobar two\nbar three\n");
    let mut harness = Harness::new();
    harness.search(&scratch, "foo", 1, None);
    let (rows, _) = harness.rows(1);
    assert_eq!(rows.len(), 2);
    // The file changes, so a search that walked again would find the new line.
    scratch.file("a.txt", "foo one\nfoobar two\nfoobar new\n");
    harness.search(&scratch, "foobar", 2, Some(1));
    let (rows, cut) = harness.rows(2);
    assert_eq!(
        (rows, cut),
        (vec!["a.txt:2:0: foobar two".to_owned()], false)
    );
    // One that does not narrow, or narrows a search not kept, walks again.
    harness.search(&scratch, "foobar", 3, Some(1));
    assert_eq!(harness.rows(3).0.len(), 2);
    harness.search(&scratch, "new", 4, None);
    assert_eq!(harness.rows(4).0, ["a.txt:3:7: foobar new"]);
    // A superseded request sends nothing, and the newest still ends.
    harness.live.store(6, Ordering::Relaxed);
    harness.grep.search(SearchRequest {
        generation: 5,
        root: scratch.0.clone(),
        query: Arc::new(LineQuery::parse("foo")),
        live: Arc::clone(&harness.live),
        narrow: None,
    });
    harness.search(&scratch, "one", 6, None);
    let mut generations = Vec::new();
    loop {
        let found = harness.found.recv_timeout(Duration::from_secs(10)).unwrap();
        generations.push(found.generation);
        if found.end.is_some() {
            break;
        }
    }
    assert!(generations.iter().all(|&generation| generation == 6));
}

#[test]
fn debug_of_a_batch_names_no_row() {
    let found = Found {
        generation: 3,
        rows: vec![SearchRow {
            path: "secret/path.txt".into(),
            line: 1,
            column: 0,
            text: "hunter2".into(),
            secret: false,
        }],
        end: None,
    };
    let debug = format!("{found:?}");
    assert!(!debug.contains("hunter2") && !debug.contains("secret/path"));
}

#[test]
#[ignore = "a timing: run by hand in a release build, with OSO_WALK_TREE set to a tree"]
fn a_search_of_a_large_tree_takes_about_as_long_as_rg() {
    let Some(tree) = std::env::var_os("OSO_WALK_TREE") else {
        return;
    };
    let tree = PathBuf::from(tree);
    // The fastest of three, warm: on the author's Mac, over 46,234 files and 932 MB, this took
    // 0.61 s and `rg --no-config --hidden -j8` 0.55-0.58 s; `fn \w+_new\(` 0.64 s and 0.57 s.
    let fastest = (0..3)
        .map(|run| {
            let mut harness = Harness::new();
            let started = std::time::Instant::now();
            harness.live.store(run + 1, Ordering::Relaxed);
            harness.grep.search(SearchRequest {
                generation: run + 1,
                root: tree.clone(),
                query: Arc::new(LineQuery::parse("AKIA[0-9A-Z]{16}")),
                live: Arc::clone(&harness.live),
                narrow: None,
            });
            let (rows, cut) = harness.rows(run + 1);
            (started.elapsed(), rows.len(), cut)
        })
        .min();
    let (took, rows, cut) = fastest.unwrap();
    assert!(
        took < Duration::from_millis(900),
        "{rows} rows (cut: {cut}) in {took:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_root_that_holds_home_by_its_real_place_is_never_searched() {
    let scratch = Scratch::new("search-home");
    scratch.file("home/me/notes.txt", "needle\n");
    std::os::unix::fs::symlink(scratch.0.join("home"), scratch.0.join("to-home")).unwrap();
    let (send, found) = mpsc::channel();
    let send = Mutex::new(send);
    let mut grep = Grep::new(Some(scratch.0.join("home/me")), move |found| {
        send.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(found)
            .is_ok()
    });
    let live = Arc::new(AtomicU64::new(1));
    grep.search(SearchRequest {
        generation: 1,
        root: scratch.0.join("to-home"),
        query: Arc::new(LineQuery::parse("needle")),
        live: Arc::clone(&live),
        narrow: None,
    });
    let ended = found.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(ended.rows.is_empty());
    assert_eq!(ended.end, Some(WalkEnd { cut: false }));
}

#[cfg(unix)]
#[test]
fn a_root_given_through_a_link_judges_its_files_where_they_lead_too() {
    let scratch = Scratch::new("search-real-root");
    scratch.file("keys/.aws/config", "key = needle\n");
    std::os::unix::fs::symlink(scratch.0.join("keys/.aws"), scratch.0.join("plain")).unwrap();
    let mut harness = Harness::new();
    harness.live.store(1, Ordering::Relaxed);
    harness.grep.search(SearchRequest {
        generation: 1,
        root: scratch.0.join("plain"),
        query: Arc::new(LineQuery::parse("needle")),
        live: Arc::clone(&harness.live),
        narrow: None,
    });
    assert_eq!(harness.rows(1).0, ["config:1:6: (secret)"]);
}

#[test]
fn the_lines_kept_never_pass_their_bound_while_a_file_is_read() {
    let request = SearchRequest {
        generation: 1,
        root: PathBuf::from("/"),
        query: Arc::new(LineQuery::parse("x")),
        live: Arc::new(AtomicU64::new(1)),
        narrow: None,
    };
    let search = files::Search::new(request);
    assert!(search.may_keep(files::MAX_KEPT - 10));
    assert!(search.may_keep(10));
    assert!(!search.may_keep(1), "past the bound");
    assert!(!search.may_keep(0), "and nothing after it");
    assert!(search.kept_over.load(Ordering::Relaxed));
}
