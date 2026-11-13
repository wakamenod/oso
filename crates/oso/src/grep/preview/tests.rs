//! Tests of the preview's read (ADR 0114): from the row's offset, around its line, within its
//! bounds, as the search reached the file, and never one that may hold secrets.
//!
//! Must not depend on: `app`, winit or the UI thread; these read files they make in a scratch
//! directory.

#[cfg(unix)]
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oso_core::editor::{ProjectPreviewRead, ProjectPreviewRequest};
use oso_core::project::MAX_LINE;

use super::super::tests::{Scratch, leads};
use super::{BACK, open, read_around, read_preview};

/// A read of `path` under `scratch` at `offset`, its line `line`, for `rows` rows.
fn read_of(
    scratch: &Scratch,
    path: &str,
    (offset, line): (u64, usize),
    rows: usize,
) -> ProjectPreviewRequest {
    ProjectPreviewRequest {
        generation: 1,
        root: scratch.0.clone(),
        path: path.into(),
        offset,
        line,
        column: 0,
        rows,
        live: Arc::new(AtomicU64::new(1)),
    }
}

/// What `request` finds, with no home directory to refuse.
fn preview(request: &ProjectPreviewRequest) -> Option<ProjectPreviewRead> {
    read_preview(request, (None, leads))
}

/// The lines `request` reads, and the number of the first.
fn read(scratch: &Scratch, request: &ProjectPreviewRequest) -> Option<(String, usize)> {
    let path = std::path::Path::new(&*request.path);
    let leads = leads(&scratch.0);
    let (file, _) = open((&scratch.0, leads.as_deref()), path, None, &|| true)?;
    let (bytes, first) = read_around(file, request, &|| true)?;
    Some((String::from_utf8(bytes).unwrap(), first))
}

fn is_text(read: Option<ProjectPreviewRead>) -> bool {
    matches!(read, Some(ProjectPreviewRead::Text(_)))
}

fn is_unreadable(read: Option<ProjectPreviewRead>) -> bool {
    matches!(read, Some(ProjectPreviewRead::Unreadable))
}

#[test]
fn it_reads_around_the_rows_line_from_its_offset() {
    let scratch = Scratch::new("around");
    let text: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    scratch.file("src/a.rs", text.as_bytes());
    let offset = text.find("line 10\n").unwrap() as u64;
    // Two rows before it, two from it.
    let request = read_of(&scratch, "src/a.rs", (offset, 10), 4);
    let (lines, first) = read(&scratch, &request).unwrap();
    assert_eq!(lines, "line 8\nline 9\nline 10\nline 11\n");
    assert_eq!(first, 8);
    // At the file's start there is nothing before.
    let request = read_of(&scratch, "src/a.rs", (0, 1), 4);
    assert_eq!(
        read(&scratch, &request).unwrap(),
        ("line 1\nline 2\n".into(), 1)
    );
    assert!(is_text(preview(&request)));
    // A file that changed since, shorter than the offset, is not read.
    let request = read_of(&scratch, "src/a.rs", (offset + 10_000, 10), 4);
    assert!(read(&scratch, &request).is_none());
}

#[test]
fn it_reads_at_most_its_bound() {
    let scratch = Scratch::new("bound");
    // A line before longer than what is read back, and one after longer than a whole read.
    let mut bytes = vec![b'a'; BACK + 10];
    bytes.push(b'\n');
    let offset = bytes.len() as u64;
    bytes.extend_from_slice(b"hit\n");
    bytes.extend(std::iter::repeat_n(b'b', MAX_LINE + 10));
    scratch.file("big.txt", &bytes);
    let request = read_of(&scratch, "big.txt", (offset, 2), 18);
    let (lines, first) = read(&scratch, &request).unwrap();
    assert!(
        lines.starts_with("hit\n"),
        "a part of a line before is left out"
    );
    assert_eq!(first, 2);
    assert!(lines.len() <= MAX_LINE);
    // Past the row's line, what an excerpt keeps of each of the 9 rows after it and a break.
    assert!(
        lines.len() <= "hit\n".len() + 9 * (4096 + 1),
        "past the line, only what its rows can show"
    );
}

#[test]
fn long_lines_after_the_rows_each_keep_what_an_excerpt_shows() {
    let scratch = Scratch::new("long-after");
    let mut bytes = b"hit\n".to_vec();
    for _ in 0..20 {
        bytes.extend(std::iter::repeat_n(b'c', 5000));
        bytes.push(b'\n');
    }
    scratch.file("long.txt", &bytes);
    let request = read_of(&scratch, "long.txt", (0, 1), 18);
    let (lines, _) = read(&scratch, &request).unwrap();
    let lines: Vec<&str> = lines.lines().collect();
    assert_eq!(lines.len(), 9, "the row's line and the 8 after it");
    assert!(lines[1..].iter().all(|line| line.len() == 4096 + 1));
}

#[test]
fn a_newer_read_stops_this_one() {
    let scratch = Scratch::new("stale");
    scratch.file("a.rs", b"one\n");
    let request = read_of(&scratch, "a.rs", (0, 1), 4);
    request.live.store(2, Ordering::Relaxed);
    assert!(preview(&request).is_none(), "nothing is sent");
}

#[test]
fn a_file_that_may_hold_secrets_is_never_read() {
    let scratch = Scratch::new("secret");
    scratch.file(".env", b"TOKEN=1\n");
    scratch.file("keys/id_rsa", b"key\n");
    for path in [".env", "keys/id_rsa"] {
        let request = read_of(&scratch, path, (0, 1), 4);
        assert!(matches!(
            preview(&request),
            Some(ProjectPreviewRead::Withheld)
        ));
    }
}

#[test]
fn a_path_that_leaves_the_root_or_a_root_that_holds_home_is_not_read() {
    let scratch = Scratch::new("leave");
    scratch.file("in/a.rs", b"one\n");
    let request = read_of(&scratch, "in/../in/a.rs", (0, 1), 4);
    assert!(is_unreadable(preview(&request)));
    let request = read_of(&scratch, "in/a.rs", (0, 1), 4);
    let home = scratch.0.join("in");
    assert!(is_unreadable(read_preview(&request, (Some(&home), leads))));
    assert!(is_text(preview(&request)));
}

#[cfg(unix)]
#[test]
fn a_link_on_the_way_or_at_the_end_is_not_followed() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("links");
    scratch.file("real/a.rs", b"one\n");
    symlink(scratch.0.join("real/a.rs"), scratch.0.join("link.rs")).unwrap();
    symlink(scratch.0.join("real"), scratch.0.join("dir")).unwrap();
    for path in ["link.rs", "dir/a.rs"] {
        let request = read_of(&scratch, path, (0, 1), 4);
        assert!(is_unreadable(preview(&request)), "{path}");
    }
    // A directory swapped for a link since the search.
    fs::rename(scratch.0.join("real"), scratch.0.join("moved")).unwrap();
    symlink(scratch.0.join("moved"), scratch.0.join("real")).unwrap();
    let request = read_of(&scratch, "real/a.rs", (0, 1), 4);
    assert!(is_unreadable(preview(&request)));
}

#[cfg(unix)]
#[test]
fn a_pipe_is_not_read() {
    use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
    let scratch = Scratch::new("pipe");
    let mkfifo = Executable::from_path("mkfifo", None).expect("mkfifo is on PATH");
    let status = spawn(&Spawn {
        executable: &mkfifo,
        args: &[scratch.0.join("a.rs").into_os_string()],
        cwd: None,
        project: None,
        inherit: &[],
        stdin: Source::Closed,
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        set: &[],
        terminal: None,
    })
    .and_then(|mut child| child.wait())
    .expect("cannot run mkfifo");
    assert!(status.success());
    let request = read_of(&scratch, "a.rs", (0, 1), 4);
    assert!(is_unreadable(preview(&request)));
}

#[test]
fn a_long_rows_line_is_kept_only_as_far_as_its_match_needs() {
    let scratch = Scratch::new("long-hit");
    let mut bytes = b"before\n".to_vec();
    let offset = bytes.len() as u64;
    bytes.extend_from_slice(b"needle ");
    bytes.extend(std::iter::repeat_n(b'x', 4 * 1024 * 1024));
    bytes.extend_from_slice(b"\nafter\n");
    scratch.file("long.txt", &bytes);
    let request = read_of(&scratch, "long.txt", (offset, 2), 4);
    let (lines, first) = read(&scratch, &request).unwrap();
    assert_eq!(first, 1);
    assert!(lines.starts_with("before\nneedle x"));
    assert!(
        lines.ends_with("\nafter\n"),
        "the lines after it still come"
    );
    assert!(lines.len() < 16 * 1024, "{}", lines.len());
}

#[cfg(unix)]
#[test]
fn a_root_that_leads_through_a_place_that_may_hold_secrets_is_never_read() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("secret-root");
    scratch.file("real/keys/config", b"Host x\n");
    // A root that is a link into such a place, and one that leads there through a link of its
    // own whose real place is named like any other.
    symlink(scratch.0.join("real/keys"), scratch.0.join("real/.ssh")).unwrap();
    fs::create_dir(scratch.0.join("proj")).unwrap();
    symlink("../real/.ssh", scratch.0.join("proj/docs")).unwrap();
    symlink(scratch.0.join("real/.ssh"), scratch.0.join("direct")).unwrap();
    for root in ["proj/docs", "direct"] {
        let request = ProjectPreviewRequest {
            root: scratch.0.join(root),
            ..read_of(&scratch, "config", (0, 1), 4)
        };
        assert!(
            matches!(preview(&request), Some(ProjectPreviewRead::Withheld)),
            "{root}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_root_opened_that_is_not_where_its_path_was_judged_to_lead_is_not_read() {
    let scratch = Scratch::new("swapped-root");
    scratch.file("a.rs", b"one\n");
    let elsewhere = Scratch::new("swapped-root-elsewhere");
    let path = std::path::Path::new("a.rs");
    // What the root's path was judged to lead to is another directory now.
    let judged = vec![elsewhere.0.clone()];
    assert!(open((&scratch.0, Some(&judged)), path, None, &|| true).is_none());
    assert!(open((&scratch.0, None), path, None, &|| true).is_none());
    let leads = leads(&scratch.0);
    assert!(open((&scratch.0, leads.as_deref()), path, None, &|| true).is_some());
}

#[cfg(unix)]
#[test]
fn a_place_judged_that_a_link_now_stands_in_the_way_of_is_not_read() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("judged-link");
    scratch.file("real/a.rs", b"one\n");
    symlink(scratch.0.join("real"), scratch.0.join("now-a-link")).unwrap();
    let root = scratch.0.join("now-a-link");
    let path = std::path::Path::new("a.rs");
    // The place was judged as a directory of that name, which a link has taken the place of:
    // the root opens through the link, but the place judged is not reached through it.
    let judged = vec![root.clone()];
    assert!(open((&root, Some(&judged)), path, None, &|| true).is_none());
    let judged = vec![scratch.0.join("real")];
    assert!(open((&root, Some(&judged)), path, None, &|| true).is_some());
}

#[cfg(unix)]
#[test]
fn a_root_spelled_through_the_systems_links_is_read() {
    let scratch = Scratch::new("spelled");
    scratch.file("a.rs", b"one\n");
    // The temporary directory as the system spells it, through `/var` on macOS; the place
    // judged is its real one, which has no link in it.
    let root = std::env::temp_dir().join(scratch.0.file_name().unwrap());
    let leads = leads(&root);
    let path = std::path::Path::new("a.rs");
    assert!(open((&root, leads.as_deref()), path, None, &|| true).is_some());
}
