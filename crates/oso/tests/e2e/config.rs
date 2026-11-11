//! The user configuration file under a running instance (ADR 0108): its keys apply, what is
//! wrong in it reaches `*Messages*` with its line, and a change to it applies without a restart.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. What the file may
//! say, and what is refused, are in the tests of `config`.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use super::cursor;
use super::harness::{App, Scratch};

/// Gives `path` `mode`, whatever the umask, so that the file is read as the person's own.
pub(super) fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .expect("the scratch directory is ours");
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

/// A scratch directory only this user can change, holding a configuration file with `text`.
fn with_config(test: &str, text: &[u8]) -> (Scratch, std::path::PathBuf) {
    let scratch = Scratch::new(test);
    set_mode(scratch.path(), 0o700);
    let config = scratch.write("config.toml", text);
    set_mode(&config, 0o644);
    (scratch, config)
}

/// Replaces `path` with `text` as most editors do: a new file renamed over it.
fn replace(path: &Path, text: &str) {
    let new = path.with_extension("new");
    fs::write(&new, text).expect("the scratch directory is writable");
    set_mode(&new, 0o644);
    fs::rename(&new, path).expect("the scratch directory is writable");
}

/// The rows of `*Messages*`, shown for a moment.
fn messages(app: &mut App) -> Vec<Value> {
    app.send_keys("C-x b * M e s s a g e s * RET");
    let rows = app.call("get-text", Some(json!({ "start_row": 0, "rows": 100 })));
    app.send_keys("C-x b RET");
    rows["rows"].as_array().cloned().unwrap_or_default()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_files_keys_apply_its_problems_are_said_and_a_change_applies_at_once() {
    let (scratch, config) = with_config(
        "config-keys",
        b"[keys.global]\n\"C-c e\" = \"end-of-buffer\"\n\"C-c x\" = \"no-such-command\"\n",
    );
    let file = scratch.write("notes.txt", b"one\ntwo\nthree\n");
    let mut app = App::start_with_config(Some(&file), scratch.path(), &config);
    app.wait_until_ready();
    let state = app.wait_for("the problem, said", |state| {
        state["echo"] == "Configuration: 1 problem, see *Messages*"
    });
    assert_eq!(cursor(&state), (0, 0));
    app.send_keys("C-c e");
    assert_eq!(cursor(&app.state()), (3, 0), "the user's key ran");
    let said = "config.toml:3:11: \"C-c x\": no command is named \"no-such-command\"";
    assert!(
        messages(&mut app).contains(&json!(said)),
        "*Messages* has it"
    );

    replace(
        &config,
        "[keys.global]\n\"C-c e\" = \"beginning-of-buffer\"\n",
    );
    app.wait_for("the good load, said", |state| {
        state["echo"] == "Configuration loaded"
    });
    app.send_keys("C-c e");
    assert_eq!(cursor(&app.state()), (0, 0), "the changed key ran");

    // A file that does not parse leaves the keys in force.
    replace(&config, "[keys.global\n");
    app.wait_for("the syntax error, said", |state| {
        state["echo"] == "Configuration not loaded: 1 problem, see *Messages*"
    });
    app.send_keys("M->");
    assert_eq!(cursor(&app.state()), (3, 0));
    app.send_keys("C-c e");
    assert_eq!(cursor(&app.state()), (0, 0), "the keys in force stayed");
}

#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_file_other_users_can_change_is_not_read() {
    use std::os::unix::fs::PermissionsExt;
    let (scratch, config) = with_config(
        "config-refused",
        b"[keys.global]\n\"C-c e\" = \"end-of-buffer\"\n",
    );
    fs::set_permissions(&config, fs::Permissions::from_mode(0o646))
        .expect("the scratch directory is ours");
    let mut app = App::start_with_config(None, scratch.path(), &config);
    app.wait_until_ready();
    app.wait_for("the refusal, said", |state| {
        state["echo"] == "Configuration not read: other users can change it"
    });
    app.send_keys("C-c e");
    assert_eq!(app.state()["echo"], "C-c e is undefined");
}
