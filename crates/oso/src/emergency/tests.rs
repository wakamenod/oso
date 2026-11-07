//! The emergency save with no window and no hook installed, as a hook is for the whole
//! process and tests share one (ADR 0062).
//!
//! Must not depend on: `app` or winit.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use oso_core::buffer::{Baseline, Buffer, BufferId, Replacement};
use oso_core::editor::Editor;

use super::{Emergency, Entry, LIMITS, Report, Why, directory_problem, write};
use crate::saver::Agreement;

mod hook;
mod seal;

/// The text published for `id`, if any.
pub(crate) fn published_text(emergency: &Emergency, id: BufferId) -> Option<String> {
    emergency
        .shared
        .state
        .lock()
        .unwrap()
        .entries
        .iter()
        .find(|entry| entry.buffer == id)
        .and_then(|entry| entry.text.as_ref().map(ToString::to_string))
}

/// Whether the entry for `id` says a save of it is running, if there is an entry.
pub(crate) fn published_saving(emergency: &Emergency, id: BufferId) -> Option<bool> {
    emergency
        .shared
        .state
        .lock()
        .unwrap()
        .entries
        .iter()
        .find(|entry| entry.buffer == id)
        .map(|entry| entry.saving)
}

/// Whether the command boundary is marked now.
pub(crate) fn in_command(emergency: &Emergency) -> bool {
    emergency.shared.in_command.load(Ordering::Relaxed)
}

/// A fresh directory for one test, removed when dropped. Its `copies` are in a directory
/// that does not exist yet.
pub(super) struct Scratch(pub(super) PathBuf);

impl Scratch {
    /// Named after the test: each runs on a thread with its name, or one at a time. Its mode
    /// is set, so that what is checked does not depend on the umask.
    pub(super) fn new() -> Self {
        let test = std::thread::current();
        let test = test.name().unwrap_or("test").replace("::", "-");
        let name = format!("oso-emergency-{}-{test}", std::process::id());
        let path = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(path)
    }

    pub(super) fn copies(&self) -> PathBuf {
        self.0.join("oso").join("emergency")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The names in `directory` with `extension`, sorted, and nothing if it does not exist.
fn names(directory: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|found| found == extension))
        .collect();
    paths.sort();
    paths
}

/// The text of each copy in `directory`, sorted.
pub(super) fn texts(directory: &Path) -> Vec<String> {
    let mut texts: Vec<String> = names(directory, "txt")
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect();
    texts.sort();
    texts
}

fn edit(editor: &mut Editor, id: BufferId, text: &str) {
    let end = editor.buffer(id).unwrap().text().len();
    let edit = Replacement {
        range: end..end,
        text,
    };
    editor.apply_edits(id, &[edit]).unwrap();
}

/// A file buffer, loaded and judged, with `text` typed into it.
pub(super) fn file_buffer(
    editor: &mut Editor,
    path: &str,
    resolved: Option<&[PathBuf]>,
    text: &str,
) -> BufferId {
    let mut buffer = Buffer::visiting(PathBuf::from(path));
    buffer.finish_empty();
    buffer.judge_secret(resolved);
    buffer.set_baseline(Baseline {
        location: PathBuf::from(path),
        file: None,
    });
    let id = editor.add_buffer(buffer);
    edit(editor, id, text);
    id
}

/// An app's worth of state writing to `directory`, published from `editor`.
pub(super) fn published(directory: &Path, editor: &Editor) -> Emergency {
    let emergency = Emergency::new(Some(directory.to_owned()));
    emergency.publisher().publish(editor);
    emergency
}

/// An editor with an edited `*scratch*`.
pub(super) fn scratch_editor(text: &str) -> Editor {
    let mut editor = Editor::default();
    let id = editor.window().buffer();
    edit(&mut editor, id, text);
    editor
}

#[test]
fn the_messages_are_not_written_with_the_buffers_at_risk() {
    let scratch = Scratch::new();
    let mut editor = scratch_editor("in scratch");
    editor.message("A MESSAGE");
    let emergency = published(&scratch.copies(), &editor);
    let report = emergency.shared.write_copies(Why::Panic);
    assert_eq!((report.wrote, report.failed, report.withheld), (1, 0, 0));
    assert_eq!(texts(&scratch.copies()), ["in scratch"]);
}

#[test]
fn a_panic_writes_each_buffer_at_risk_exactly_and_with_where_it_belongs() {
    let scratch = Scratch::new();
    let mut editor = scratch_editor("in scratch, no final newline");
    let plain = PathBuf::from("/p/notes.txt");
    file_buffer(
        &mut editor,
        "/p/notes.txt",
        Some(std::slice::from_ref(&plain)),
        "日本語\r\n\ttabs\r\nno end",
    );
    // Not edited: not at risk.
    editor.add_buffer(Buffer::from_text("untouched", "abc"));

    let emergency = published(&scratch.copies(), &editor);
    let report = emergency.shared.write_copies(Why::Panic);
    assert_eq!((report.wrote, report.failed, report.withheld), (2, 0, 0));
    assert_eq!(
        texts(&scratch.copies()),
        ["in scratch, no final newline", "日本語\r\n\ttabs\r\nno end"],
    );
    let metas: Vec<String> = names(&scratch.copies(), "meta")
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect();
    assert_eq!(metas.len(), 2);
    assert!(metas.iter().any(|meta| {
        meta.contains("\nname notes.txt\npath /p/notes.txt\nlocation /p/notes.txt\nwithheld false\nbom false\n")
            && meta.contains("\nwhy panic\n")
    }));
    assert!(
        metas
            .iter()
            .any(|meta| meta.contains("\nname *scratch*\nwithheld false\nbom false\n"))
    );
    assert!(
        metas
            .iter()
            .all(|meta| meta.starts_with("oso-emergency-copy 1\n"))
    );
}

#[test]
fn the_choice_of_what_gets_a_copy() {
    let scratch = Scratch::new();
    let mut editor = Editor::default();
    let plain = PathBuf::from("/p/plain.txt");
    let key = PathBuf::from("/h/.ssh/id_rsa");
    let ordinary = Some(std::slice::from_ref(&plain));
    file_buffer(&mut editor, "/p/plain.txt", ordinary, "plain");
    file_buffer(&mut editor, "/p/lost.txt", None, "unresolved");
    file_buffer(&mut editor, "/p/.env", ordinary, "secret by name");
    file_buffer(
        &mut editor,
        "/p/.env.unresolved",
        None,
        "secret and unresolved",
    );
    file_buffer(
        &mut editor,
        "/p/link.txt",
        Some(std::slice::from_ref(&key)),
        "secret by link",
    );
    let id = editor.window().buffer();
    edit(&mut editor, id, "pathless");

    let emergency = published(&scratch.copies(), &editor);
    let report = emergency.shared.write_copies(Why::Panic);
    assert_eq!((report.wrote, report.failed, report.withheld), (3, 0, 3));
    assert_eq!(
        texts(&scratch.copies()),
        ["pathless", "plain", "unresolved"]
    );
    let everything = fs::read_dir(scratch.copies()).unwrap();
    for entry in everything {
        let bytes = fs::read(entry.unwrap().path()).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("secret"), "no trace of a secret buffer");
        assert!(!text.contains(".env"), "nor of its path");
    }
    let shown = report.to_string();
    assert!(shown.contains("wrote 3 emergency copies in"), "{shown}");
    assert!(
        shown.contains(&scratch.copies().display().to_string()),
        "{shown}"
    );
    assert!(
        shown.contains("no copy of 3 unsaved buffers that may"),
        "{shown}"
    );
}

#[test]
fn a_secret_buffers_text_is_never_published() {
    let mut editor = Editor::default();
    let id = file_buffer(&mut editor, "/p/.env", Some(&[]), "secret");
    let emergency = Emergency::new(None);
    emergency.publisher().publish(&editor);
    assert_eq!(published_text(&emergency, id), None);
    let entries = &emergency.shared.state.lock().unwrap().entries;
    assert_eq!(entries.len(), 1, "that it is at risk is known");
}

#[test]
fn an_unmodified_or_killed_or_saved_buffer_is_taken_back_and_only_changes_touch_the_state() {
    let mut editor = Editor::default();
    let id = file_buffer(&mut editor, "/p/a.txt", Some(&[]), "one");
    let emergency = Emergency::new(None);
    let publisher = emergency.publisher();
    publisher.publish(&editor);
    assert_eq!(published_text(&emergency, id).as_deref(), Some("one"));

    edit(&mut editor, id, " two");
    publisher.publish(&editor);
    assert_eq!(published_text(&emergency, id).as_deref(), Some("one two"));

    let saved = editor.buffer(id).unwrap().content_id();
    editor.buffer_mut(id).unwrap().mark_saved(saved);
    publisher.publish(&editor);
    assert_eq!(published_text(&emergency, id), None);

    edit(&mut editor, id, " three");
    publisher.publish(&editor);
    assert_eq!(
        published_text(&emergency, id).as_deref(),
        Some("one two three")
    );
    editor.kill_buffer(id);
    publisher.publish(&editor);
    assert_eq!(published_text(&emergency, id), None);
    assert!(emergency.shared.state.lock().unwrap().entries.is_empty());
}

#[test]
fn a_save_running_keeps_the_buffer_at_risk_even_if_it_looks_unmodified() {
    let scratch = Scratch::new();
    let mut editor = Editor::default();
    let id = file_buffer(&mut editor, "/p/a.txt", Some(&[]), "typed\n");
    editor.show(id);
    editor.start_save().unwrap();
    // The state the save is writing is on disk only when it reports.
    let content = editor.buffer(id).unwrap().content_id();
    editor.buffer_mut(id).unwrap().mark_saved(content);
    assert!(!editor.buffer(id).unwrap().is_modified());
    let emergency = published(&scratch.copies(), &editor);
    assert!(published_text(&emergency, id).is_some());

    emergency
        .agreement_cell()
        .set(Agreement::ModifiedButNotSaves);
    let report = emergency.shared.write_copies(Why::Exit);
    assert_eq!(
        report.wrote, 1,
        "the end may stop the save, and nobody can tell"
    );
}

#[test]
fn what_was_agreed_to_decides_what_an_exit_writes() {
    let scratch = Scratch::new();
    let editor = scratch_editor("unsaved");
    let emergency = published(&scratch.copies(), &editor);

    emergency.agreement_cell().set(Agreement::Everything);
    let report = emergency.shared.write_copies(Why::Exit);
    assert!(report.is_empty(), "a confirmed exit, or the socket's");
    assert!(!scratch.copies().exists(), "no directory either");

    emergency
        .agreement_cell()
        .set(Agreement::ModifiedButNotSaves);
    let report = emergency.shared.write_copies(Why::Exit);
    assert!(report.is_empty(), "this one is not being saved");

    emergency.agreement_cell().set(Agreement::Nothing);
    let report = emergency.shared.write_copies(Why::Exit);
    assert_eq!(report.wrote, 1, "the OS ended it");
    assert_eq!(texts(&scratch.copies()), ["unsaved"]);
}

#[test]
fn a_state_on_disk_is_not_written_again_but_a_new_one_keeps_the_first() {
    let scratch = Scratch::new();
    let mut editor = scratch_editor("first");
    let emergency = Emergency::new(Some(scratch.copies()));
    let publisher = emergency.publisher();
    publisher.publish(&editor);

    assert_eq!(emergency.shared.write_copies(Why::Panic).wrote, 1);
    assert_eq!(
        emergency.shared.write_copies(Why::Exit).wrote,
        0,
        "an exit after a panic, or a second panic"
    );

    let id = editor.window().buffer();
    edit(&mut editor, id, " and second");
    publisher.publish(&editor);
    assert_eq!(emergency.shared.write_copies(Why::Panic).wrote, 1);
    assert_eq!(texts(&scratch.copies()), ["first", "first and second"]);
    assert_eq!(names(&scratch.copies(), "meta").len(), 2);
}

#[test]
fn a_failure_or_a_panic_on_one_buffer_does_not_stop_the_next() {
    let scratch = Scratch::new();
    let mut editor = scratch_editor("scratch");
    file_buffer(&mut editor, "/p/fails.txt", Some(&[]), "fails");
    file_buffer(&mut editor, "/p/panics.txt", Some(&[]), "panics");
    file_buffer(&mut editor, "/p/last.txt", Some(&[]), "last");
    let emergency = published(&scratch.copies(), &editor);
    let report = emergency
        .shared
        .write_copies_with(LIMITS, |dir, entry, ticks| {
            match entry.identity.name.as_str() {
                "fails.txt" => Err(io::Error::other("no room")),
                "panics.txt" => panic!("deliberate panic in a test's writer"),
                _ => write::write_copy(dir, entry, Why::Panic, std::time::SystemTime::now(), ticks),
            }
        });
    assert_eq!((report.wrote, report.failed), (2, 2));
    assert_eq!(texts(&scratch.copies()), ["last", "scratch"]);
    assert_eq!(report.problem.as_deref(), Some("no room"));
}

#[test]
fn an_unsafe_directory_gets_nothing_and_says_so() {
    let scratch = Scratch::new();
    let editor = scratch_editor("text");
    fs::create_dir_all(scratch.copies()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(scratch.copies(), fs::Permissions::from_mode(0o755)).unwrap();
        let emergency = published(&scratch.copies(), &editor);
        let report = emergency.shared.write_copies(Why::Panic);
        assert_eq!((report.wrote, report.failed), (0, 1));
        let shown = report.to_string();
        assert!(shown.contains("refused: it"), "{shown}");
        assert!(shown.contains("could not write 1"), "{shown}");
        assert!(texts(&scratch.copies()).is_empty());
    }
    let emergency = Emergency::new(None);
    emergency.publisher().publish(&editor);
    let report = emergency.shared.write_copies(Why::Panic);
    assert_eq!(report.failed, 1);
    assert!(report.to_string().contains("no place"));
}

#[test]
fn a_directory_that_would_be_refused_is_found_out_before_any_crash_and_nothing_is_created() {
    assert!(
        directory_problem(None).is_some_and(|problem| problem.contains("no place")),
        "no place could be told"
    );
    let scratch = Scratch::new();
    assert_eq!(directory_problem(Some(&scratch.copies())), None);
    assert!(!scratch.copies().exists(), "nothing is created");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(scratch.copies()).unwrap();
        fs::set_permissions(scratch.copies(), fs::Permissions::from_mode(0o755)).unwrap();
        let problem = directory_problem(Some(&scratch.copies())).unwrap();
        assert!(
            problem.contains("No emergency copy can be written"),
            "{problem}"
        );
        assert!(problem.contains("refused: it"), "{problem}");
    }
}

#[cfg(unix)]
#[test]
fn copies_and_their_directory_are_private_and_no_temporary_file_stays() {
    use std::os::unix::fs::MetadataExt;
    let scratch = Scratch::new();
    let emergency = published(&scratch.copies(), &scratch_editor("text"));
    assert_eq!(emergency.shared.write_copies(Why::Panic).wrote, 1);
    let mode = |path: &Path| fs::symlink_metadata(path).unwrap().mode() & 0o7777;
    assert_eq!(mode(&scratch.copies()), 0o700);
    let entries: Vec<PathBuf> = fs::read_dir(scratch.copies())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(
        entries.len(),
        2,
        "a text and its metadata, and nothing else"
    );
    for path in entries {
        assert_eq!(mode(&path), 0o600, "{}", path.display());
    }
}

#[cfg(unix)]
#[test]
fn a_link_planted_where_a_copy_would_go_is_stepped_around_and_not_followed() {
    use crate::emergency::Identity;
    use oso_core::decode::LineEnding;
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new();
    let identity = Identity {
        name: "n".to_owned(),
        path: None,
        location: None,
        withheld: false,
        bom: false,
        line_ending: LineEnding::Lf,
    };
    let first = write::free_stem(&scratch.0, &identity, 7).unwrap();
    let target = scratch.0.join("target");
    symlink(&target, scratch.0.join(format!("{first}.meta"))).unwrap();
    let second = write::free_stem(&scratch.0, &identity, 7).unwrap();
    assert_ne!(first, second);
    assert!(!target.exists());
}

#[cfg(unix)]
#[test]
fn a_symlink_for_the_directory_is_refused() {
    let scratch = Scratch::new();
    let real = scratch.0.join("real");
    fs::create_dir(&real).unwrap();
    fs::create_dir(scratch.0.join("oso")).unwrap();
    std::os::unix::fs::symlink(&real, scratch.copies()).unwrap();
    let emergency = published(&scratch.copies(), &scratch_editor("text"));
    let report = emergency.shared.write_copies(Why::Panic);
    assert_eq!(report.wrote, 0);
    assert!(report.to_string().contains("symlink"));
    assert_eq!(fs::read_dir(real).unwrap().count(), 0);
}

#[test]
fn a_report_holds_nothing_when_nothing_happened_and_says_a_singular_as_one() {
    assert!(Report::default().is_empty());
    assert_eq!(Report::default().to_string(), "");
    let report = Report {
        directory: Some(PathBuf::from("/d")),
        wrote: 1,
        failed: 1,
        withheld: 1,
        problem: None,
    };
    let shown = report.to_string();
    assert!(shown.contains("wrote 1 emergency copy in /d"), "{shown}");
    assert!(
        shown.contains("could not write 1 emergency copy"),
        "{shown}"
    );
    assert!(shown.contains("1 unsaved buffer that may"), "{shown}");
}

#[test]
fn a_copys_name_comes_from_a_stable_hash_of_where_it_belongs() {
    assert_eq!(write::fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(write::fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    let scratch = Scratch::new();
    let mut editor = Editor::default();
    file_buffer(&mut editor, "/p/a b.txt", Some(&[]), "x");
    let emergency = published(&scratch.copies(), &editor);
    emergency.shared.write_copies(Why::Panic);
    let path = &names(&scratch.copies(), "txt")[0];
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let hash = format!("{:016x}-", write::fnv1a(b"/p/a b.txt"));
    assert!(name.starts_with(&hash), "{name}");
}

#[test]
fn metadata_is_one_line_of_ascii_per_key_whatever_a_path_holds() {
    let mut editor = Editor::default();
    let id = file_buffer(&mut editor, "/p/日本 語\nline\u{1b}[2J.txt", Some(&[]), "x");
    let emergency = Emergency::new(None);
    emergency.publisher().publish(&editor);
    let state = emergency.shared.state.lock().unwrap();
    let entry: &Entry = state
        .entries
        .iter()
        .find(|entry| entry.buffer == id)
        .unwrap();
    let meta = write::meta_text(&entry.identity, entry.file, 1, Why::Exit, 7);
    assert!(meta.is_ascii());
    assert!(!meta.contains('\u{1b}'));
    assert_eq!(meta.lines().count(), 10);
    assert!(meta.ends_with("bytes 1\nwritten 7\nwhy exit\n"), "{meta}");
    let path_line = meta.lines().find(|line| line.starts_with("path ")).unwrap();
    assert!(path_line.contains("%E6%97%A5"), "{path_line}");
    assert!(path_line.contains("%20"), "{path_line}");
    assert!(path_line.contains("%0A"), "{path_line}");
}

#[test]
fn metadata_says_what_the_file_was_so_that_a_change_since_can_be_told() {
    use oso_core::buffer::FileStamp;
    use std::time::{Duration, SystemTime};
    let mut editor = Editor::default();
    let id = file_buffer(&mut editor, "/p/a.txt", Some(&[]), "x");
    let stamp = FileStamp {
        // Whole hundreds of nanoseconds, all that Windows keeps.
        modified: SystemTime::UNIX_EPOCH + Duration::new(5, 700),
        len: 3,
        mode: 0o644,
    };
    let location = PathBuf::from("/p/a.txt");
    editor.buffer_mut(id).unwrap().set_baseline(Baseline {
        location,
        file: Some(stamp),
    });
    let emergency = Emergency::new(None);
    emergency.publisher().publish(&editor);
    let state = emergency.shared.state.lock().unwrap();
    let entry = state
        .entries
        .iter()
        .find(|entry| entry.buffer == id)
        .unwrap();
    let meta = write::meta_text(&entry.identity, entry.file, 1, Why::Panic, 7);
    assert!(
        meta.contains("\nfile-bytes 3\nfile-modified 5.000000700\nbytes 1\n"),
        "{meta}"
    );
}
