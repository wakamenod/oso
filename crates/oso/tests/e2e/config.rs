//! The user configuration file under a running instance (ADR 0108): its keys apply, what is
//! wrong in it reaches `*Messages*` with its line, and a change to it applies without a restart.
//! Its settings apply at start and again only when they change, beside the theme remembered
//! from before and a restart's handoff.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. What the file may
//! say, and what is refused, are in the tests of `config`.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::cursor;
use super::handover::hand_over;
use super::harness::{App, Scratch, emergency_dir};

/// Gives `path` `mode`, whatever the umask, so that the file is read as the person's own.
fn set_mode(path: &Path, mode: u32) {
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

/// The theme the instance draws with.
fn theme(app: &mut App) -> String {
    let state = app.call("export-state", None);
    state["theme"].as_str().unwrap_or_default().to_owned()
}

/// Waits until the instance draws with `name`.
fn wait_for_theme(app: &mut App, name: &str) {
    let started = Instant::now();
    while theme(app) != name {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "never drew with {name}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// Waits until `*Messages*` has `line`.
fn wait_for_message(app: &mut App, line: &str) {
    let started = Instant::now();
    while !messages(app).contains(&json!(line)) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "*Messages* never had {line:?}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

/// A table this oso does not know, so that a look at the file says something.
const SAID: &str = "[later]\n";
/// What it says.
const SAID_ECHO: &str = "Configuration: 1 problem, see *Messages*";

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_files_settings_apply_at_start_and_again_only_when_they_change() {
    let settings = |line_numbers: bool, start: bool, default: &str| {
        format!(
            "[theme]\ndefault = \"{default}\"\npair = [\"dusk\", \"light\"]\n\n\
             [aids]\nline-numbers = {line_numbers}\n\n[agent-channel]\nstart = {start}\n"
        )
    };
    let (scratch, config) = with_config("config-settings", settings(true, true, "dusk").as_bytes());
    let mut app = App::start_with_config(None, scratch.path(), &config);
    app.wait_until_ready();
    let state = app.wait_for("the channel, started", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.starts_with("Agent  "))
    });
    assert_eq!(state["visual_aids"]["line_numbers"], true);
    assert_eq!(theme(&mut app), "dusk");
    wait_for_message(
        &mut app,
        "Agent channel on, as config.toml says: each agent asks you first",
    );

    // Choices made at run time stand while their settings do not change.
    app.run_command("global-whitespace-mode", Some(0))
        .expect("an aid toggles");
    app.run_command("toggle-theme", None)
        .expect("the theme toggles");
    assert_eq!(theme(&mut app), "light", "within the file's pair");
    replace(&config, &settings(false, false, "dusk"));
    let state = app.wait_for("the channel's change, said", |state| {
        state["echo"] == "config.toml: [agent-channel] start = false applies from the next start"
    });
    assert_eq!(state["visual_aids"]["line_numbers"], false, "it changed");
    assert_eq!(
        state["visual_aids"]["whitespace"], false,
        "the choice stood"
    );
    assert_eq!(theme(&mut app), "light", "the choice stood");
    assert!(
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.starts_with("Agent  ")),
        "a reload never stops the channel"
    );

    replace(&config, &settings(false, false, "dark"));
    wait_for_theme(&mut app, "dark");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_theme_shown_last_wins_until_the_configured_default_changes() {
    let file = |default: &str| format!("{SAID}[theme]\ndefault = \"{default}\"\n");
    let (scratch, config) = with_config("config-theme", file("light").as_bytes());
    let remembered = emergency_dir(scratch.path())
        .parent()
        .expect("the state directory")
        .join("theme");
    {
        let mut app = App::start_with_config(None, scratch.path(), &config);
        app.wait_until_ready();
        app.wait_for("the look", |state| state["echo"] == SAID_ECHO);
        assert_eq!(theme(&mut app), "light");
        app.run_command("toggle-theme", None)
            .expect("the theme toggles");
        assert_eq!(theme(&mut app), "dark");
        let started = Instant::now();
        while fs::read_to_string(&remembered).ok().as_deref() != Some("dark\nlight\n") {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "never remembered"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
    {
        let mut app = App::start_with_config(None, scratch.path(), &config);
        app.wait_until_ready();
        app.wait_for("the look", |state| state["echo"] == SAID_ECHO);
        assert_eq!(theme(&mut app), "dark", "the choice wins");
    }
    replace(&config, &file("dusk"));
    let mut app = App::start_with_config(None, scratch.path(), &config);
    app.wait_until_ready();
    app.wait_for("the look", |state| state["echo"] == SAID_ECHO);
    assert_eq!(theme(&mut app), "dusk", "the changed default wins");

    // A reload never starts the channel.
    replace(
        &config,
        &format!("{}[agent-channel]\nstart = true\n", file("dusk")),
    );
    wait_for_message(
        &mut app,
        "config.toml: [agent-channel] start = true applies from the next start",
    );
    assert!(
        app.state()["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.starts_with("Agent  "))
    );
}

#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_file_its_group_can_change_does_not_start_the_channel() {
    let (scratch, config) = with_config(
        "config-group",
        b"[aids]\nline-numbers = true\n\n[agent-channel]\nstart = true\n",
    );
    set_mode(&config, 0o664);
    let mut app = App::start_with_config(None, scratch.path(), &config);
    app.wait_until_ready();
    let state = app.wait_for("the warning", |state| state["echo"] == SAID_ECHO);
    assert_eq!(
        state["visual_aids"]["line_numbers"], true,
        "the rest applies"
    );
    wait_for_message(
        &mut app,
        "config.toml: [agent-channel] start not applied: other users in a group can change it",
    );
    assert!(
        app.state()["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.starts_with("Agent  "))
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_restarts_aids_win_over_the_files_which_become_the_baseline() {
    let file = |whitespace: bool| {
        format!("{SAID}[aids]\nline-numbers = true\nwhitespace = {whitespace}\n")
    };
    let (scratch, config) = with_config("config-handover", file(true).as_bytes());
    let mut old = App::start_with_config(None, scratch.path(), &config);
    old.wait_until_ready();
    old.wait_for("the look", |state| state["echo"] == SAID_ECHO);
    old.run_command("global-display-line-numbers-mode", Some(0))
        .expect("an aid toggles");
    let mut new = App::start_with_config(None, scratch.path(), &config);
    new.wait_until_ready();
    hand_over(&mut old, &mut new);
    // Whether the new instance's look came before the import or after, the handoff's switch
    // stands.
    wait_for_message(&mut new, "config.toml:1:2: unknown table [later], skipped");
    assert_eq!(new.state()["visual_aids"]["line_numbers"], false);

    replace(&config, &file(false));
    let state = new.wait_for("whitespace, changed", |state| {
        state["visual_aids"]["whitespace"] == false
    });
    assert_eq!(
        state["visual_aids"]["line_numbers"], false,
        "its setting did not change"
    );
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
