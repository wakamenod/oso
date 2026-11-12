//! Recovering an emergency copy, from a real copy where a flow can make one (ADR 0064).
//!
//! Must not depend on: the app's internals, or logic a lower layer can test.

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use oso_platform::process::{Executable, Sink, Spawn, spawn};
use serde_json::Value;

use super::harness::{App, FAKE_SECRET, Scratch, emergency_dir};
use super::{FAKE_TEXT, read_copies, typing};

/// A stem no writer would use by chance.
const STEM: &str = "00000000000000aa-1790000000-77-0";

/// The rows the window shows, as one text.
fn visible(state: &Value) -> String {
    let rows = state["text"]["rows"].as_array();
    let rows = rows.map_or_else(Vec::new, |rows| {
        rows.iter().filter_map(Value::as_str).collect()
    });
    rows.join("\n")
}

/// Whether the modeline warns of a copy.
fn warns(state: &Value) -> bool {
    state["modeline"]
        .as_str()
        .is_some_and(|modeline| modeline.starts_with("[emergency copy]"))
}

fn echo(state: &Value) -> &str {
    state["echo"].as_str().unwrap_or("")
}

fn name(state: &Value) -> &str {
    state["buffer"]["name"].as_str().unwrap_or("")
}

/// Types into `file`, and ends the instance by a panic, which writes a copy.
fn crash_with_typed_text(scratch: &Scratch, file: &Path) {
    let mut app = App::start(Some(file), scratch.path());
    app.wait_until_ready();
    app.send_keys(&typing(FAKE_TEXT));
    let _ = app.exchange("dev-panic", None);
    assert!(!app.wait_for_exit().success());
    assert_eq!(read_copies(&emergency_dir(scratch.path())).len(), 1);
}

/// An instance on `file`, once it has offered its copy.
fn offered(scratch: &Scratch, file: &Path) -> App {
    let mut app = App::start(Some(file), scratch.path());
    app.wait_until_ready();
    app.wait_for("the warning of a copy", warns);
    app
}

/// `C-h r`, once the list has its rows.
fn open_list(app: &mut App, row: &str) {
    app.send_keys("C-h r");
    app.wait_for("the list with its row", |state| {
        name(state) == "*Emergency Copies*" && visible(state).contains(row)
    });
}

/// The private directory, for copies made by hand.
fn private_directory(scratch: &Scratch) -> PathBuf {
    let directory = emergency_dir(scratch.path());
    fs::create_dir_all(&directory).expect("cannot make the directory");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .expect("cannot set its mode");
    directory
}

/// A copy made by hand: its metadata as written here, and its text.
fn plant(directory: &Path, stem: &str, meta: &str, text: &[u8]) {
    fs::write(directory.join(format!("{stem}.meta")), meta).expect("cannot write metadata");
    fs::write(directory.join(format!("{stem}.txt")), text).expect("cannot write text");
}

/// Metadata of a copy of `path` holding `bytes` bytes.
fn meta_of(path: &str, bytes: usize) -> String {
    format!(
        "oso-emergency-copy 1\nname copy\npath {path}\nlocation {path}\nwithheld false\nbom false\nline-ending lf\nbytes {bytes}\nwritten 1790000000\nwhy panic\n"
    )
}

/// Makes `path` look long left behind, so that no writer can be at work on it.
fn make_old(path: &Path) {
    let file = File::options()
        .write(true)
        .open(path)
        .expect("cannot open it");
    let long_ago = SystemTime::now() - Duration::from_secs(24 * 60 * 60);
    file.set_modified(long_ago).expect("cannot set its time");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_copy_from_a_real_panic_is_offered_recovered_and_goes_once_it_is_saved() {
    let scratch = Scratch::new("recover");
    let notes = scratch.write("notes.txt", b"on disk\n");
    crash_with_typed_text(&scratch, &notes);
    let directory = emergency_dir(scratch.path());

    let mut app = offered(&scratch, &notes);
    let state = app.state();
    assert!(
        echo(&state).contains("notes.txt has an emergency copy"),
        "{}",
        echo(&state)
    );
    assert_eq!(state["buffer"]["modified"], false, "only offered");
    assert_eq!(fs::read(&notes).expect("the file"), b"on disk\n");

    open_list(&mut app, "file unchanged");
    app.send_keys("r");
    let state = app.wait_for("the copy", |state| {
        name(state) == "*copy of notes.txt*" && state["buffer"]["loading"].is_null()
    });
    assert!(visible(&state).contains(&format!("{FAKE_TEXT}on disk")));
    assert!(echo(&state).contains("Press r again"));
    assert_eq!(fs::read(&notes).expect("the file"), b"on disk\n");

    app.send_keys("r");
    let state = app.state();
    assert_eq!(name(&state), "notes.txt");
    assert_eq!(state["buffer"]["modified"], true);
    assert!(visible(&state).contains(&format!("{FAKE_TEXT}on disk")));
    assert!(!warns(&state), "it has been answered");
    assert_eq!(
        fs::read(&notes).expect("the file"),
        b"on disk\n",
        "not until saved"
    );
    assert_eq!(
        read_copies(&directory).len(),
        1,
        "the copy stays until then"
    );

    app.send_keys("C-x C-s");
    app.wait_for("the save", |state| state["buffer"]["modified"] == false);
    let saved = fs::read_to_string(&notes).expect("the file");
    assert!(saved.starts_with(FAKE_TEXT), "the text reached the file");
    app.wait_for("the copy to go", |state| {
        echo(state).contains("Deleted the emergency copy")
    });
    assert!(read_copies(&directory).is_empty());

    let log = app.stop_and_read_log();
    assert!(!log.contains(FAKE_TEXT), "the log quotes the text");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn undoing_a_recovery_keeps_the_copy_through_a_save() {
    let scratch = Scratch::new("recover-undo");
    let notes = scratch.write("notes.txt", b"on disk\n");
    crash_with_typed_text(&scratch, &notes);
    let directory = emergency_dir(scratch.path());
    let mut app = offered(&scratch, &notes);
    open_list(&mut app, "file unchanged");
    app.send_keys("r");
    app.wait_for("the copy", |state| {
        name(state) == "*copy of notes.txt*" && state["buffer"]["loading"].is_null()
    });
    app.send_keys("r");
    app.send_keys("C-z");
    let state = app.state();
    assert_eq!(state["buffer"]["modified"], false);
    assert!(visible(&state).contains("on disk"));
    assert!(!visible(&state).contains(FAKE_TEXT));
    // A new edit, saved: the person turned the copy down by undoing, and has not deleted it.
    app.send_keys("x C-x C-s");
    app.wait_for("the save", |state| state["buffer"]["modified"] == false);
    assert_eq!(read_copies(&directory).len(), 1);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_end_nobody_agreed_to_is_offered_and_a_confirmed_exit_keeps_it_for_next_time() {
    let scratch = Scratch::new("recover-exit");
    let notes = scratch.write("notes.txt", b"on disk\n");
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    app.send_keys(&typing(FAKE_TEXT));
    app.call("dev-end-unasked", None);
    assert!(app.wait_for_exit().success());
    drop(app);
    let directory = emergency_dir(scratch.path());
    assert_eq!(read_copies(&directory).len(), 1);

    let mut app = offered(&scratch, &notes);
    open_list(&mut app, "exit");
    app.send_keys("r");
    app.wait_for("the copy", |state| {
        name(state) == "*copy of notes.txt*" && state["buffer"]["loading"].is_null()
    });
    app.send_keys("r");
    assert_eq!(app.state()["buffer"]["modified"], true);
    // Leaving with the recovered text unsaved, having agreed to lose it.
    app.send_keys("C-x C-c");
    let _ = app.exchange("send-keys", Some(serde_json::json!({ "keys": "y" })));
    assert!(app.wait_for_exit().success());
    drop(app);
    assert_eq!(read_copies(&directory).len(), 1, "it is offered again");
    let mut app = offered(&scratch, &notes);
    assert!(warns(&app.state()));
    let _ = app.stop_and_read_log();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn declining_takes_two_keys_and_removes_both_files() {
    let scratch = Scratch::new("recover-decline");
    let notes = scratch.write("notes.txt", b"on disk\n");
    crash_with_typed_text(&scratch, &notes);
    let directory = emergency_dir(scratch.path());
    let mut app = offered(&scratch, &notes);
    open_list(&mut app, "file unchanged");
    app.send_keys("d");
    assert!(echo(&app.state()).contains("Press d again"));
    assert_eq!(read_copies(&directory).len(), 1, "one d keeps it");
    app.send_keys("C-n C-p d");
    assert_eq!(read_copies(&directory).len(), 1, "another key cancelled it");
    app.send_keys("d");
    app.wait_for("the row to go", |state| {
        visible(state).starts_with("0 emergency copies.")
    });
    assert!(read_copies(&directory).is_empty());
    app.send_keys("q");
    let state = app.state();
    assert_eq!(name(&state), "notes.txt");
    assert!(!warns(&state));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_file_changed_since_is_said_and_compared_and_saving_replaces_it() {
    let scratch = Scratch::new("recover-changed");
    let notes = scratch.write("notes.txt", b"on disk\n");
    crash_with_typed_text(&scratch, &notes);
    fs::write(&notes, b"changed by another program\n").expect("cannot change it");

    let mut app = offered(&scratch, &notes);
    open_list(&mut app, "file changed");
    app.send_keys("=");
    let state = app.wait_for("the compare", |state| name(state) == "*compare notes.txt*");
    let shown = visible(&state);
    assert!(shown.contains("-changed by another program"), "{shown}");
    assert!(shown.contains(&format!("+{FAKE_TEXT}on disk")), "{shown}");
    app.send_keys("r");
    app.wait_for("the copy", |state| {
        name(state) == "*copy of notes.txt*" && state["buffer"]["loading"].is_null()
    });
    app.send_keys("r");
    assert!(echo(&app.state()).contains("replaces what is there now"));
    app.send_keys("C-x C-s");
    app.wait_for("the save", |state| state["buffer"]["modified"] == false);
    let saved = fs::read_to_string(&notes).expect("the file");
    assert!(saved.starts_with(FAKE_TEXT));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_file_that_has_gone_comes_back_private() {
    use std::os::unix::fs::MetadataExt;
    let scratch = Scratch::new("recover-gone");
    let notes = scratch.write("notes.txt", b"on disk\n");
    crash_with_typed_text(&scratch, &notes);
    fs::remove_file(&notes).expect("cannot remove it");

    let mut app = offered(&scratch, &notes);
    open_list(&mut app, "file gone");
    app.send_keys("r");
    app.wait_for("the copy", |state| {
        name(state) == "*copy of notes.txt*" && state["buffer"]["loading"].is_null()
    });
    app.send_keys("r");
    assert!(echo(&app.state()).contains("mode 0600"));
    assert!(!notes.exists(), "not until saved");
    app.send_keys("C-x C-s");
    app.wait_for("the save", |state| state["buffer"]["modified"] == false);
    let metadata = fs::metadata(&notes).expect("it came back");
    assert_eq!(metadata.mode() & 0o777, 0o600);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_copy_of_scratch_is_listed_and_goes_in_an_empty_scratch() {
    let scratch = Scratch::new("recover-scratch");
    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys(&typing(FAKE_TEXT));
    let _ = app.exchange("dev-panic", None);
    assert!(!app.wait_for_exit().success());
    drop(app);

    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.wait_for("the warning of a copy", warns);
    open_list(&mut app, "no file");
    app.send_keys("r");
    app.wait_for("the copy", |state| {
        name(state) == "*copy of *scratch**" && state["buffer"]["loading"].is_null()
    });
    app.send_keys("r");
    let state = app.state();
    assert_eq!(name(&state), "*scratch*");
    assert!(visible(&state).contains(FAKE_TEXT));
    let log = app.stop_and_read_log();
    assert!(!log.contains(FAKE_TEXT), "the log quotes the text");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_copy_that_would_count_as_secret_now_is_offered_and_withheld() {
    let scratch = Scratch::new("recover-secret");
    let directory = private_directory(&scratch);
    let path = scratch.path().join(".env");
    let path = path.to_str().expect("scratch paths are UTF-8");
    plant(
        &directory,
        STEM,
        &meta_of(path, FAKE_SECRET.len()),
        FAKE_SECRET.as_bytes(),
    );

    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    open_list(&mut app, "not open");
    assert!(!visible(&app.state()).contains(FAKE_SECRET));
    app.send_keys("v");
    let state = app.wait_for("the copy, withheld", |state| {
        state["buffer"]["secret"] == true && state["buffer"]["loading"].is_null()
    });
    assert_eq!(state["text"], Value::Null, "no text");
    assert!(state.get("cursor").is_none(), "and no position");
    let params = serde_json::json!({ "start_row": 0, "rows": 5 });
    let refused = app.try_call("get-text", Some(params)).unwrap_err();
    assert!(refused.contains("secret"), "{refused}");
    assert!(app.try_call("screenshot", None).is_err());
    let log = app.stop_and_read_log();
    super::assert_no_secret(&log, "the log");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_hostile_directory_costs_nothing_and_changes_nothing() {
    let scratch = Scratch::new("recover-hostile");
    let directory = private_directory(&scratch);
    let victim = scratch.write("victim.txt", b"keep this\n");
    let opened = scratch.write("opened.txt", b"opened\n");
    let victim_path = victim.to_str().expect("scratch paths are UTF-8");

    // A copy naming another file, with far more text than it says.
    plant(
        &directory,
        "00000000000000a1-1790000001-77-0",
        &meta_of(victim_path, 3),
        &vec![b'x'; 1 << 20],
    );
    // Metadata over the limit, and metadata that means nothing.
    plant(
        &directory,
        "00000000000000a2-1790000002-77-0",
        &"x".repeat(1 << 20),
        b"text",
    );
    plant(
        &directory,
        "00000000000000a3-1790000003-77-0",
        "oso-emergency-copy 1\npath /a%zz\nbytes 1\n",
        b"t",
    );
    // A text with no metadata, a link for a text, and a real pipe for each file of a copy.
    let lone = directory.join("00000000000000a4-1790000004-77-0.txt");
    fs::write(&lone, b"lone").expect("cannot write it");
    make_old(&lone);
    plant(
        &directory,
        "00000000000000a5-1790000005-77-0",
        &meta_of(victim_path, 10),
        b"",
    );
    let linked = directory.join("00000000000000a5-1790000005-77-0.txt");
    fs::remove_file(&linked).expect("cannot remove it");
    std::os::unix::fs::symlink(&victim, &linked).expect("cannot link it");
    for extension in ["meta", "txt"] {
        let pipe = directory.join(format!("00000000000000a6-1790000006-77-0.{extension}"));
        let mkfifo = Executable::from_path("mkfifo", None).expect("mkfifo is on PATH");
        let status = spawn(&Spawn {
            executable: &mkfifo,
            args: &[pipe.into_os_string()],
            cwd: None,
            project: None,
            inherit: &[],
            stdout: Sink::Null,
            stderr: Sink::Null,
            own_process_group: false,
            set: &[],
            terminal: None,
        })
        .and_then(|mut child| child.wait())
        .expect("cannot run mkfifo");
        assert!(status.success());
    }
    // Names of no form this knows, which must stay as they are.
    fs::write(directory.join("something-else"), b"other").expect("cannot write it");

    let mut app = App::start(Some(&opened), scratch.path());
    app.wait_until_ready();
    app.send_keys("C-h r");
    let state = app.wait_for("the rows", |state| {
        visible(state).contains("damaged") && visible(state).contains("unknown")
    });
    assert!(!visible(&state).contains("keep this"));
    // The whole row: the window cuts it, and the path it claims comes last.
    let params = serde_json::json!({ "start_row": 0, "rows": 20 });
    let rows = app.call("get-text", Some(params))["rows"].to_string();
    assert!(rows.contains(victim_path), "the path it claims is shown");

    // Answering the row that names another file does nothing to that file.
    app.send_keys("r d d");
    assert_eq!(fs::read(&victim).expect("the victim"), b"keep this\n");
    assert_eq!(fs::read(&opened).expect("the file"), b"opened\n");
    assert!(directory.join("something-else").exists());
    let names = app.state();
    assert_eq!(names["buffer"]["modified"], false);
    let _ = app.stop_and_read_log();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_copy_equal_to_its_file_goes_and_is_not_offered() {
    let scratch = Scratch::new("recover-equal");
    let directory = private_directory(&scratch);
    let notes = scratch.write("notes.txt", b"the same\n");
    let path = notes.to_str().expect("scratch paths are UTF-8");
    plant(&directory, STEM, &meta_of(path, 9), b"the same\n");
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    let state = app.wait_for("the row in the messages", |state| {
        echo(state).contains("its file holds the same text")
    });
    assert!(!warns(&state));
    assert!(read_copies(&directory).is_empty());
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn what_a_save_left_is_listed_and_deleted_and_a_young_one_is_left_alone() {
    let scratch = Scratch::new("recover-left");
    let notes = scratch.write("notes.txt", b"on disk\n");
    let old = scratch.write(".oso-save-77-123-0.tmp", FAKE_TEXT.as_bytes());
    make_old(&old);
    let young = scratch.write(".oso-save-77-456-0.tmp", b"being written");

    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    open_list(&mut app, "left by a save");
    let state = app.state();
    assert!(!visible(&state).contains(".oso-save-77-456-0.tmp"));
    app.send_keys("v");
    let state = app.wait_for("the look, withheld", |state| {
        state["buffer"]["secret"] == true && state["buffer"]["loading"].is_null()
    });
    assert_eq!(state["text"], Value::Null, "no text over the socket");
    app.send_keys("q");
    app.send_keys("d d");
    app.wait_for("the row to go", |state| {
        !visible(state).contains("left by a save")
    });
    assert!(!old.exists());
    assert!(young.exists());
    let log = app.stop_and_read_log();
    assert!(!log.contains(FAKE_TEXT), "the log quotes the text");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_list_of_a_directory_with_nothing_in_it_says_so_and_a_refused_one_is_not_read() {
    let scratch = Scratch::new("recover-refused");
    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys("C-h r");
    app.wait_for("an empty list", |state| {
        visible(state).starts_with("0 emergency copies.")
    });
    let _ = app.stop_and_read_log();
    drop(app);

    let directory = private_directory(&scratch);
    plant(&directory, STEM, &meta_of("/a", 4), FAKE_TEXT.as_bytes());
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("cannot loosen it");
    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys("C-h r");
    let state = app.wait_for("the reason", |state| visible(state).contains("Not read:"));
    assert!(visible(&state).starts_with("0 emergency copies."));
    let _ = app.stop_and_read_log();
}
