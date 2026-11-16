//! Tests of the rows of a list of a server's places (ADR 0138, step 8b): the server's order
//! grouped by file, the paths shown, the lines of open buffers and of files read once, and no
//! text of a file that may hold secrets.
//!
//! Must not depend on: the app or the window system; these read files they make in a scratch
//! directory.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::Editor;
use oso_core::editor::{VisitColumn, XrefRows};
use oso_core::rope::Rope;

use super::{Home, ROW_TEXT_BYTES, Snapshot, Spot, Withheld, build, grouped, row_text, shown_path};

/// A directory of its own under the system's temporary one, removed at the end.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let name = format!("oso-xref-rows-{}-{test}", std::process::id());
        let path = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn file(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn file(path: &Path, line: usize) -> Spot {
    Spot::File {
        path: path.to_path_buf(),
        line,
        column: VisitColumn::Utf16(0),
    }
}

fn leads(root: &Path) -> Option<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for ancestor in root.ancestors() {
        if let Ok(target) = fs::read_link(ancestor) {
            paths.push(ancestor.parent()?.join(target));
        }
    }
    paths.push(fs::canonicalize(root).ok()?);
    Some(paths)
}

/// An editor and a buffer's id.
fn editor() -> (Editor, BufferId) {
    let mut editor = Editor::with_buffer(Buffer::new("a.rs"));
    let other = editor.add_buffer(Buffer::new("open.rs"));
    (editor, other)
}

/// The rows' texts, as the list shows them in `editor`.
fn texts(mut editor: Editor, rows: XrefRows) -> Vec<String> {
    editor.set_frame_size(oso_core::frame::FrameSize {
        rows: 40,
        columns: 160,
    });
    editor
        .open_xref_list(
            oso_core::editor::XrefKind::References,
            1,
            PathBuf::from("/"),
        )
        .unwrap();
    editor.xref_listed(rows);
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    (0..candidates.len())
        .map(|at| candidates.get(at).unwrap().text().to_owned())
        .collect()
}

#[test]
fn places_keep_the_server_s_order_grouped_by_file() {
    let (a, b) = (Path::new("/r/a.rs"), Path::new("/r/b.rs"));
    let spots = [file(a, 9), file(b, 1), file(a, 2), file(b, 5)];
    assert_eq!(grouped(&spots), [0, 2, 1, 3]);
}

#[test]
fn a_path_shows_from_the_root_or_whole_with_the_home_as_a_tilde() {
    let (root, home) = (Path::new("/w/oso"), Some(Path::new("/h")));
    assert_eq!(
        &*shown_path(Path::new("/w/oso/src/a.rs"), root, home),
        "src/a.rs"
    );
    assert_eq!(
        &*shown_path(Path::new("/h/.cargo/c.rs"), root, home),
        "~/.cargo/c.rs"
    );
    assert_eq!(
        &*shown_path(Path::new("/usr/lib/d.rs"), root, home),
        "/usr/lib/d.rs"
    );
}

#[test]
fn a_row_shows_its_line_from_its_first_non_blank_cut_at_a_character() {
    assert_eq!(&*row_text(b"\t  let x = 1;\r"), "let x = 1;");
    let long = "é".repeat(ROW_TEXT_BYTES);
    let shown = row_text(long.as_bytes());
    assert!(shown.ends_with('…'));
    assert!(shown.len() <= ROW_TEXT_BYTES + '…'.len_utf8());
    assert!(!shown.contains('\u{fffd}'), "never a split character");
}

/// A snapshot of `spots` under `root`, with no home, no buffer withheld and the whole budget.
fn snapshot(root: &Path, spots: Vec<Spot>, texts: HashMap<BufferId, Rope>) -> Snapshot {
    Snapshot {
        generation: 1,
        root: root.to_path_buf(),
        home: Home {
            spelled: None,
            real: None,
        },
        spots,
        texts,
        withheld: Vec::new(),
        read_max: super::READ_MAX,
        cut: false,
    }
}

fn open(path: PathBuf, buffer: BufferId, offset: usize, secret: bool) -> Spot {
    Spot::Open {
        path,
        buffer,
        offset,
        revision: 3,
        secret,
    }
}

#[test]
fn rows_read_open_buffers_and_files_once_and_nothing_of_a_secret_one() {
    let scratch = Scratch::new("build");
    let a = scratch.file("src/a.rs", "fn a() {}\n    fn b() {}\nfn c() {}\n");
    let secret = scratch.file(".env", "TOKEN=1\n");
    let (editor, buffer) = editor();
    let mut texts_by_buffer = HashMap::new();
    texts_by_buffer.insert(buffer, Rope::from("one\n  two\n"));
    let spots = vec![
        file(&a, 2),
        open(scratch.0.join("src/open.rs"), buffer, 6, false),
        file(&secret, 1),
        file(&a, 3),
        open(scratch.0.join("src/held.rs"), buffer, 0, true),
    ];
    let snapshot = snapshot(&scratch.0, spots, texts_by_buffer);
    let rows = build(&snapshot, leads, &|| true).unwrap();
    assert_eq!(
        texts(editor, rows),
        [
            "src/a.rs:2: fn b() {}",
            "src/a.rs:3: fn c() {}",
            "src/open.rs:2: two",
            ".env:1:",
            "src/held.rs:1:"
        ]
    );
    assert!(build(&snapshot, leads, &|| false).is_none(), "a stale list");
}

#[test]
fn the_file_of_an_open_buffer_that_withholds_is_never_read() {
    let scratch = Scratch::new("withheld");
    let by_path = scratch.file("src/config.rs", "let key = 1;\n");
    let linked = scratch.file("src/linked.rs", "let key = 2;\n");
    let mut snapshot = snapshot(
        &scratch.0,
        vec![file(&by_path, 1), file(&linked, 1)],
        HashMap::new(),
    );
    snapshot.withheld.push(Withheld {
        path: Some(by_path.clone()),
        identity: None,
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(&linked).unwrap();
        snapshot.withheld.push(Withheld {
            path: None,
            identity: Some((meta.dev(), meta.ino())),
        });
    }
    let (editor, _) = editor();
    let rows = build(&snapshot, leads, &|| true).unwrap();
    let linked_row = if cfg!(unix) {
        "src/linked.rs:1:"
    } else {
        "src/linked.rs:1: let key = 2;"
    };
    assert_eq!(texts(editor, rows), ["src/config.rs:1:", linked_row]);
}

#[test]
fn a_file_right_in_the_home_directory_or_past_the_budget_gives_no_text() {
    let scratch = Scratch::new("bounds");
    let home = scratch.0.join("home");
    let in_home = scratch.file("home/notes.rs", "fn n() {}\n");
    let first = scratch.file("proj/a.rs", "fn a() {}\n");
    let second = scratch.file("proj/b.rs", "fn b() {}\n");
    let root = scratch.0.join("proj");
    let mut snapshot = snapshot(
        &root,
        vec![file(&in_home, 1), file(&first, 1), file(&second, 1)],
        HashMap::new(),
    );
    snapshot.home = Home {
        spelled: Some(home.clone()),
        real: Some(home),
    };
    // Enough for the first file of the root only.
    snapshot.read_max = 12;
    let (editor, _) = editor();
    let rows = build(&snapshot, leads, &|| true).unwrap();
    assert_eq!(
        texts(editor, rows),
        ["~/notes.rs:1:", "a.rs:1: fn a() {}", "b.rs:1:"]
    );
}

#[cfg(unix)]
#[test]
fn a_place_whose_path_is_not_utf_8_or_too_long_gives_no_row() {
    use std::os::unix::ffi::OsStrExt;
    let odd = PathBuf::from(std::ffi::OsStr::from_bytes(b"/w/\xff.rs"));
    let long = PathBuf::from(format!("/w/{}.rs", "a".repeat(5000)));
    let snapshot = snapshot(
        Path::new("/w"),
        vec![
            file(&odd, 1),
            file(&long, 1),
            file(Path::new("/w/ok.rs"), 1),
        ],
        HashMap::new(),
    );
    let (editor, _) = editor();
    let rows = build(&snapshot, leads, &|| true).unwrap();
    assert_eq!(texts(editor, rows), ["ok.rs:1:"]);
}

#[cfg(windows)]
#[test]
fn a_path_under_the_root_or_the_home_is_spelled_with_slashes_on_windows() {
    let (root, home) = (Path::new(r"C:\w\oso"), Some(Path::new(r"C:\h")));
    assert_eq!(
        &*shown_path(Path::new(r"C:\w\oso\src\a.rs"), root, home),
        "src/a.rs"
    );
    assert_eq!(
        &*shown_path(Path::new(r"C:\h\.cargo\c.rs"), root, home),
        "~/.cargo/c.rs"
    );
}
