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
use oso_core::editor::{DiagnosticRowAt, VisitColumn, XrefRows};
use oso_core::rope::Rope;

use super::{ROW_TEXT_BYTES, Snapshot, Spot, build, grouped, row_text, shown_path};

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
    let separator = std::path::MAIN_SEPARATOR;
    assert_eq!(
        &*shown_path(Path::new("/h/.cargo/c.rs"), root, home),
        format!("~{separator}.cargo/c.rs")
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

#[test]
fn rows_read_open_buffers_and_files_once_and_nothing_of_a_secret_one() {
    let scratch = Scratch::new("build");
    let a = scratch.file("src/a.rs", "fn a() {}\n    fn b() {}\nfn c() {}\n");
    let secret = scratch.file(".env", "TOKEN=1\n");
    let open = scratch.0.join("src/open.rs");
    let (editor, buffer) = editor();
    let mut texts_by_buffer = HashMap::new();
    texts_by_buffer.insert(buffer, Rope::from("one\n  two\n"));
    let snapshot = Snapshot {
        generation: 1,
        root: scratch.0.clone(),
        home: (None, None),
        spots: vec![
            file(&a, 2),
            Spot::Open {
                path: open,
                buffer,
                offset: 6,
                revision: 3,
                secret: false,
            },
            file(&secret, 1),
            file(&a, 3),
        ],
        texts: texts_by_buffer,
        cut: false,
    };
    let rows = build(&snapshot, leads, &|| true).unwrap();
    assert_eq!(
        texts(editor, rows),
        [
            "src/a.rs:2: fn b() {}",
            "src/a.rs:3: fn c() {}",
            "src/open.rs:2: two",
            ".env:1:"
        ]
    );
    assert!(build(&snapshot, leads, &|| false).is_none(), "a stale list");
    let _ = DiagnosticRowAt::File {
        column: VisitColumn::Chars(0),
    };
}
