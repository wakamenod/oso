//! Tests of the preview's read (ADR 0114): from the row's offset, around its line, within its
//! bounds, as the search reached the file, and never one that may hold secrets.
//!
//! Must not depend on: `app`, winit or the UI thread; these read files they make in a scratch
//! directory.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oso_core::editor::{ProjectPreviewRead, ProjectPreviewRequest};
use oso_core::project::MAX_LINE;

use super::{BACK, open, read_around, read_preview};

/// A scratch directory with no links in its path, removed at the end of the test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let name = format!("oso-preview-test-{}-{test}", std::process::id());
        let path = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn file(&self, relative: &str, content: &[u8]) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
    }

    /// A read of `path` at `offset`, its line `line`, for `rows` rows.
    fn request(
        &self,
        path: &str,
        (offset, line): (u64, usize),
        rows: usize,
    ) -> ProjectPreviewRequest {
        ProjectPreviewRequest {
            generation: 1,
            root: self.0.clone(),
            path: path.into(),
            offset,
            line,
            column: 0,
            rows,
            live: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The lines `request` reads, and the number of the first.
fn read(scratch: &Scratch, request: &ProjectPreviewRequest) -> Option<(String, usize)> {
    let path = std::path::Path::new(&*request.path);
    let (file, _) = open(&scratch.0, path, None, &|| true)?;
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
    let request = scratch.request("src/a.rs", (offset, 10), 4);
    let (lines, first) = read(&scratch, &request).unwrap();
    assert_eq!(lines, "line 8\nline 9\nline 10\nline 11\n");
    assert_eq!(first, 8);
    // At the file's start there is nothing before.
    let request = scratch.request("src/a.rs", (0, 1), 4);
    assert_eq!(
        read(&scratch, &request).unwrap(),
        ("line 1\nline 2\n".into(), 1)
    );
    assert!(is_text(read_preview(&request, None)));
    // A file that changed since, shorter than the offset, is not read.
    let request = scratch.request("src/a.rs", (offset + 10_000, 10), 4);
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
    let request = scratch.request("big.txt", (offset, 2), 18);
    let (lines, first) = read(&scratch, &request).unwrap();
    assert!(
        lines.starts_with("hit\n"),
        "a part of a line before is left out"
    );
    assert_eq!(first, 2);
    assert!(lines.len() <= MAX_LINE);
    assert!(
        lines.len() <= 2 * 64 * 1024,
        "past the line, a part read and what its rows can show"
    );
}

#[test]
fn a_newer_read_stops_this_one() {
    let scratch = Scratch::new("stale");
    scratch.file("a.rs", b"one\n");
    let request = scratch.request("a.rs", (0, 1), 4);
    request.live.store(2, Ordering::Relaxed);
    assert!(read_preview(&request, None).is_none(), "nothing is sent");
}

#[test]
fn a_file_that_may_hold_secrets_is_never_read() {
    let scratch = Scratch::new("secret");
    scratch.file(".env", b"TOKEN=1\n");
    scratch.file("keys/id_rsa", b"key\n");
    for path in [".env", "keys/id_rsa"] {
        let request = scratch.request(path, (0, 1), 4);
        assert!(matches!(
            read_preview(&request, None),
            Some(ProjectPreviewRead::Withheld)
        ));
    }
}

#[test]
fn a_path_that_leaves_the_root_or_a_root_that_holds_home_is_not_read() {
    let scratch = Scratch::new("leave");
    scratch.file("in/a.rs", b"one\n");
    let request = scratch.request("in/../in/a.rs", (0, 1), 4);
    assert!(is_unreadable(read_preview(&request, None)));
    let request = scratch.request("in/a.rs", (0, 1), 4);
    let home = scratch.0.join("in");
    assert!(is_unreadable(read_preview(&request, Some(&home))));
    assert!(is_text(read_preview(&request, None)));
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
        let request = scratch.request(path, (0, 1), 4);
        assert!(is_unreadable(read_preview(&request, None)), "{path}");
    }
    // A directory swapped for a link since the search.
    fs::rename(scratch.0.join("real"), scratch.0.join("moved")).unwrap();
    symlink(scratch.0.join("moved"), scratch.0.join("real")).unwrap();
    let request = scratch.request("real/a.rs", (0, 1), 4);
    assert!(is_unreadable(read_preview(&request, None)));
}

#[cfg(all(unix, feature = "dev"))]
#[test]
fn a_pipe_is_not_read() {
    use oso_platform::process::{Executable, Sink, Spawn, spawn};
    let scratch = Scratch::new("pipe");
    let mkfifo = Executable::from_path("mkfifo", None).expect("mkfifo is on PATH");
    let status = spawn(&Spawn {
        executable: &mkfifo,
        args: &[scratch.0.join("a.rs").into_os_string()],
        cwd: None,
        project: None,
        inherit: &[],
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
    })
    .and_then(|mut child| child.wait())
    .expect("cannot run mkfifo");
    assert!(status.success());
    let request = scratch.request("a.rs", (0, 1), 4);
    assert!(is_unreadable(read_preview(&request, None)));
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
    let request = scratch.request("long.txt", (offset, 2), 4);
    let (lines, first) = read(&scratch, &request).unwrap();
    assert_eq!(first, 1);
    assert!(lines.starts_with("before\nneedle x"));
    assert!(
        lines.ends_with("\nafter\n"),
        "the lines after it still come"
    );
    assert!(lines.len() < 16 * 1024, "{}", lines.len());
}
