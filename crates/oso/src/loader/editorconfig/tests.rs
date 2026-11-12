//! The tests of reading the EditorConfig files above a file, on scratch directories.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::fs;

use oso_core::editorconfig::{IndentSize, IndentStyle, VISIT_SECTIONS};
use oso_core::indent::Style;

use super::*;

/// A scratch directory with no links in its path, removed at the end of the test. Each test
/// passes it as the home directory, so that no file above it counts.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let name = format!("oso-editorconfig-test-{}-{test}", std::process::id());
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

    /// What the files give the file at `relative`, with this directory as home.
    fn read(&self, relative: &str) -> Found {
        read(&self.0.join(relative), Some(&self.0))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn size(found: &Found) -> Option<(IndentSize, usize)> {
    found.applied.as_ref()?.properties.indent_size()
}

#[test]
fn the_nearest_file_wins_and_each_value_names_its_file() {
    let scratch = Scratch::new("nearest");
    let far = scratch.file(
        ".editorconfig",
        "[*]\nindent_size = 8\nindent_style = tab\n",
    );
    let near = scratch.file("a/.editorconfig", "[*.rs]\nindent_size = 2\n");
    let found = scratch.read("a/b/x.rs");
    let applied = found.applied.as_ref().unwrap();
    assert_eq!(applied.files, [near, far]);
    assert_eq!(size(&found), Some((IndentSize::Columns(2), 0)));
    assert_eq!(
        applied.properties.indent_style(),
        Some((IndentStyle::Tab, 1))
    );
    assert!(found.notes.is_empty());
}

#[test]
fn a_root_file_ends_the_walk() {
    let scratch = Scratch::new("root");
    scratch.file(".editorconfig", "[*]\nindent_style = tab\n");
    let near = scratch.file("a/.editorconfig", "root = true\n[*]\nindent_size = 2\n");
    let found = scratch.read("a/x.rs");
    let applied = found.applied.unwrap();
    assert_eq!(applied.files, [near]);
    assert_eq!(applied.properties.indent_style(), None);
}

#[test]
fn the_walk_ends_after_the_home_directory() {
    let scratch = Scratch::new("home");
    scratch.file(".editorconfig", "[*]\nindent_size = 8\n");
    let home_file = scratch.file("home/.editorconfig", "[*]\nindent_style = tab\n");
    let location = scratch.0.join("home/p/x.rs");
    let found = read(&location, Some(&scratch.0.join("home")));
    let applied = found.applied.unwrap();
    assert_eq!(
        applied.files,
        [home_file],
        "home's own file counts, not the one above"
    );
}

#[test]
fn globs_are_read_from_each_files_directory() {
    let scratch = Scratch::new("relative");
    scratch.file(
        ".editorconfig",
        "[a/*.rs]\nindent_size = 3\n[/x.rs]\nindent_size = 5\n",
    );
    assert_eq!(
        size(&scratch.read("a/x.rs")),
        Some((IndentSize::Columns(3), 0))
    );
    assert_eq!(
        size(&scratch.read("x.rs")),
        Some((IndentSize::Columns(5), 0))
    );
    assert_eq!(size(&scratch.read("b/a/x.rs")), None);
}

#[test]
fn a_new_files_missing_directories_are_passed_over() {
    let scratch = Scratch::new("new");
    scratch.file("a/.editorconfig", "[*]\nindent_size = 2\n");
    let found = scratch.read("a/not/yet/x.rs");
    assert_eq!(size(&found), Some((IndentSize::Columns(2), 0)));
}

#[test]
fn with_no_file_nothing_applies() {
    let scratch = Scratch::new("none");
    let found = scratch.read("a/x.rs");
    assert!(found.applied.is_none());
    assert!(found.notes.is_empty());
}

#[cfg(unix)]
#[test]
fn a_link_is_not_read_and_said() {
    let scratch = Scratch::new("link");
    let target = scratch.file("elsewhere/config", "[*]\nindent_size = 2\n");
    fs::create_dir_all(scratch.0.join("a")).unwrap();
    std::os::unix::fs::symlink(&target, scratch.0.join("a/.editorconfig")).unwrap();
    let found = scratch.read("a/x.rs");
    assert!(found.applied.is_none());
    assert_eq!(found.notes.len(), 1);
    assert!(
        found.notes[0].ends_with("is not read: it is a link, or not a regular file"),
        "{:?}",
        found.notes
    );
}

#[cfg(unix)]
#[test]
fn a_link_on_the_way_up_ends_the_walk() {
    let scratch = Scratch::new("link-dir");
    scratch.file(".editorconfig", "[*]\nindent_size = 2\n");
    fs::create_dir_all(scratch.0.join("real")).unwrap();
    std::os::unix::fs::symlink(scratch.0.join("real"), scratch.0.join("link")).unwrap();
    let found = read(&scratch.0.join("link/x.rs"), Some(&scratch.0));
    assert!(
        found.applied.is_none(),
        "the walk goes from where a path leads"
    );
}

#[cfg(unix)]
#[test]
fn a_file_every_user_may_write_is_not_read() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("writable");
    let file = scratch.file(".editorconfig", "[*]\nindent_size = 2\n");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o666)).unwrap();
    let found = scratch.read("x.rs");
    assert!(found.applied.is_none());
    assert!(found.notes[0].ends_with("another user owns it or can change it"));
    fs::set_permissions(&file, fs::Permissions::from_mode(0o664)).unwrap();
    assert!(
        scratch.read("x.rs").applied.is_some(),
        "a group may write it, as the configuration file"
    );
}

#[test]
fn a_file_too_large_or_not_utf8_is_not_read() {
    let scratch = Scratch::new("large");
    let mut text = "[*]\nindent_size = 2\n".to_owned();
    text.push_str(&"#".repeat(MAX_FILE + 1 - text.len()));
    scratch.file(".editorconfig", &text);
    let found = scratch.read("x.rs");
    assert!(found.applied.is_none());
    assert!(found.notes[0].ends_with("it is larger than 64 KiB"));
    text.pop();
    scratch.file(".editorconfig", &text);
    assert!(scratch.read("x.rs").applied.is_some());
    fs::write(
        scratch.0.join(".editorconfig"),
        b"[*]\nindent_size = \xff\n",
    )
    .unwrap();
    let found = scratch.read("x.rs");
    assert!(found.applied.is_none());
    assert!(found.notes[0].ends_with("it is not UTF-8 text"));
}

#[test]
fn the_files_of_a_visit_share_one_budget() {
    let scratch = Scratch::new("budget");
    let sections =
        |count: usize| -> String { (0..count).map(|_| "[*]\nindent_size = 3\n").collect() };
    scratch.file(
        ".editorconfig",
        &format!("[*]\ntab_width = 4\n{}", sections(1)),
    );
    let near = scratch.file("a/.editorconfig", &sections(VISIT_SECTIONS));
    let found = scratch.read("a/x.rs");
    assert_eq!(
        found.notes,
        [format!(
            "EditorConfig {}: line 1: a visit compiles at most 128 sections and 2 KiB of \
             globs; this section and the rest are skipped",
            scratch.0.join(".editorconfig").display()
        )]
    );
    let applied = found.applied.unwrap();
    assert_eq!(applied.files[0], near);
    assert_eq!(
        applied.properties.tab_width(),
        Some((3, 0)),
        "derived from the near size"
    );
}

#[test]
fn the_tab_width_is_editorconfigs_or_the_configured_one() {
    let scratch = Scratch::new("tab-width");
    let configured = Settings::new(2, Style::Spaces, 8).unwrap();
    assert_eq!(scratch.read("x.rs").tab_width(configured), TabWidth::new(8));
    scratch.file(".editorconfig", "[*]\ntab_width = 4\n");
    assert_eq!(scratch.read("x.rs").tab_width(configured), TabWidth::new(4));
}

/// Visits `path` with home at `home`, and gives the last update with what EditorConfig gave,
/// taken before the first update, as the UI takes it.
fn visit(path: PathBuf, home: &Path) -> (Option<crate::loader::LoadUpdate>, Option<Found>) {
    use crate::loader::{LoadUpdate, UNASKED, Visit, spawn};
    use std::sync::mpsc;
    use std::time::Duration;

    let (tx, rx) = mpsc::channel();
    let id = oso_core::editor::Editor::default()
        .buffers()
        .next()
        .unwrap()
        .0;
    let visit = Visit {
        configured: Settings::default(),
        editorconfig: true,
        home: Some(home.to_path_buf()),
    };
    let handle = spawn(id, path, UNASKED, visit, move |_| tx.send(()).is_ok()).unwrap();
    let mut found = None;
    while rx.recv_timeout(Duration::from_secs(10)).is_ok() {
        found = found.or_else(|| handle.take_editorconfig());
        let Some(update) = handle.take() else {
            continue;
        };
        assert!(
            found.is_some(),
            "EditorConfig comes before the first update"
        );
        if matches!(&update, LoadUpdate::Failed(_))
            || matches!(&update, LoadUpdate::Snapshot(s) if s.done())
        {
            return (Some(update), found);
        }
    }
    (None, found)
}

#[test]
fn a_visit_builds_the_display_map_with_editorconfigs_tab_width() {
    let scratch = Scratch::new("visit");
    scratch.file(".editorconfig", "[*.txt]\ntab_width = 4\n");
    let path = scratch.file("x.txt", "\ta\n");
    let (update, found) = visit(path, &scratch.0);
    let Some(crate::loader::LoadUpdate::Snapshot(snapshot)) = update else {
        panic!("the file loads");
    };
    let mut buffer = oso_core::buffer::Buffer::visiting(scratch.0.join("x.txt"));
    buffer.apply_load(*snapshot);
    assert_eq!(buffer.tab_width(), TabWidth::new(4));
    assert_eq!(
        found.unwrap().applied.unwrap().properties.tab_width(),
        Some((4, 0))
    );
}

#[test]
fn a_new_file_takes_what_editorconfig_gives_where_it_will_be() {
    let scratch = Scratch::new("visit-new");
    scratch.file(".editorconfig", "[*]\nindent_size = 2\n");
    let (update, found) = visit(scratch.0.join("new.rs"), &scratch.0);
    assert!(matches!(
        update,
        Some(crate::loader::LoadUpdate::Failed(
            crate::loader::LoadError::NotFound
        ))
    ));
    assert_eq!(size(&found.unwrap()), Some((IndentSize::Columns(2), 0)));
}
