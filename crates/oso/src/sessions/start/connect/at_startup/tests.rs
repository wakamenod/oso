//! Tests of the reading of `remoteControlAtStartup`, over temporary directories: which files
//! turn it on, which off, and that what cannot be read is off.
//!
//! Must not depend on: the person's own settings: every file is the test's.

use std::fs;
use std::path::PathBuf;

use super::{Files, MAX_BYTES, Reading, project_off};

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("oso-at-startup-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("managed")).unwrap();
        fs::create_dir_all(dir.join("config")).unwrap();
        Self(dir)
    }

    fn files(&self) -> Files {
        Files {
            user: Some(self.0.join("config/settings.json")),
            managed: self.0.join("managed"),
        }
    }

    fn user(&self, text: &str) {
        fs::write(self.0.join("config/settings.json"), text).unwrap();
    }

    fn managed(&self, text: &str) {
        fs::write(self.0.join("managed/managed-settings.json"), text).unwrap();
    }

    fn drop_in(&self, name: &str, text: &str) {
        let dir = self.0.join("managed/managed-settings.d");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), text).unwrap();
    }
}

#[test]
fn the_person_s_true_turns_it_on_and_nothing_else_of_theirs_does() {
    let scratch = Scratch::new("user");
    assert_eq!(scratch.files().read(), Reading::Off, "no file");
    for (text, reading) in [
        (r#"{"remoteControlAtStartup": true}"#, Reading::On),
        (r#"{"remoteControlAtStartup": false}"#, Reading::Off),
        (r#"{"model": "haiku"}"#, Reading::Off),
        (
            r#"{"remoteControl": {"remoteControlAtStartup": true}}"#,
            Reading::Off,
        ),
        ("{}", Reading::Off),
    ] {
        scratch.user(text);
        assert_eq!(scratch.files().read(), reading, "{text}");
    }
    let none = Files {
        user: None,
        ..scratch.files()
    };
    assert_eq!(none.read(), Reading::Off, "no configuration directory");
}

#[test]
fn what_cannot_be_read_is_off_and_says_which_file() {
    let scratch = Scratch::new("unread");
    for text in [
        r#"{"remoteControlAtStartup": "true"}"#,
        r#"{"remoteControlAtStartup": 1}"#,
        r#"{"remoteControlAtStartup": true,}"#,
        r#"[{"remoteControlAtStartup": true}]"#,
        "// a comment\n{\"remoteControlAtStartup\": true}",
        "",
    ] {
        scratch.user(text);
        let Reading::Unread(why) = scratch.files().read() else {
            panic!("{text} read");
        };
        assert!(why.contains("settings.json"), "{why}");
        assert!(!why.contains('"'), "quotes nothing: {why}");
    }
    let big = format!(
        r#"{{"remoteControlAtStartup": true, "x": "{}"}}"#,
        "a".repeat(usize::try_from(MAX_BYTES).unwrap())
    );
    scratch.user(&big);
    assert!(matches!(scratch.files().read(), Reading::Unread(_)));
    fs::write(scratch.0.join("config/settings.json"), b"{\"a\": \"\xff\"}").unwrap();
    assert!(matches!(scratch.files().read(), Reading::Unread(_)));
    fs::remove_file(scratch.0.join("config/settings.json")).unwrap();
    fs::create_dir(scratch.0.join("config/settings.json")).unwrap();
    assert!(
        matches!(scratch.files().read(), Reading::Unread(_)),
        "a directory"
    );
}

/// A pipe in a file's place is not opened, so that no writer can hold the thread that reads.
#[cfg(unix)]
#[test]
fn a_pipe_is_refused_and_never_waited_for() {
    use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
    let scratch = Scratch::new("pipe");
    let fifo = scratch.0.join("config/settings.json");
    let mkfifo = Executable::from_path("mkfifo", None).expect("mkfifo is on PATH");
    let status = spawn(&Spawn {
        executable: &mkfifo,
        args: &[fifo.clone().into_os_string()],
        cwd: None,
        project: None,
        inherit: &[],
        stdin: Source::Closed,
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        own_session: false,
        set: &[],
        terminal: None,
        pass_fd: None,
        args_logged: true,
    })
    .and_then(|mut child| child.wait())
    .expect("cannot run mkfifo");
    assert!(status.success());
    assert!(matches!(scratch.files().read(), Reading::Unread(_)));
    let project = scratch.0.join("project");
    fs::create_dir_all(project.join(".claude")).unwrap();
    fs::rename(&fifo, project.join(".claude/settings.json")).unwrap();
    assert!(project_off(&project).is_err());
}

#[test]
fn a_managed_value_comes_before_the_person_s() {
    let scratch = Scratch::new("managed");
    scratch.user(r#"{"remoteControlAtStartup": true}"#);
    scratch.managed(r#"{"remoteControlAtStartup": false}"#);
    assert_eq!(scratch.files().read(), Reading::Off);
    scratch.managed(r#"{"permissions": {}}"#);
    assert_eq!(scratch.files().read(), Reading::On);
    scratch.user("{}");
    scratch.managed(r#"{"remoteControlAtStartup": true}"#);
    assert_eq!(scratch.files().read(), Reading::On);
    // A false in any managed document wins.
    scratch.drop_in("20-off.json", r#"{"remoteControlAtStartup": false}"#);
    assert_eq!(scratch.files().read(), Reading::Off);
    scratch.drop_in("20-off.json", "{}");
    scratch.drop_in("10-bad.json", "{");
    assert!(matches!(scratch.files().read(), Reading::Unread(why) if why.contains("10-bad.json")));
    scratch.drop_in("10-bad.json", "{}");
    scratch.drop_in("notes.txt", "{");
    scratch.drop_in(".hidden.json", "{");
    assert_eq!(scratch.files().read(), Reading::On, "only *.json documents");
    let paths = scratch.files().paths();
    assert!(paths.iter().any(|path| path.ends_with("10-bad.json")));
    assert!(
        paths
            .iter()
            .any(|path| path.ends_with("config/settings.json"))
    );
}

#[test]
fn a_file_is_named_escaped() {
    let scratch = Scratch::new("escaped");
    // Windows takes no control character in a name, so there the bidi mark alone.
    let name = match cfg!(windows) {
        true => "project\u{202e}",
        false => "pro\u{1b}[31mject\u{202e}",
    };
    let project = scratch.0.join(name);
    fs::create_dir_all(project.join(".claude")).unwrap();
    fs::write(project.join(".claude/settings.json"), "{").unwrap();
    let why = project_off(&project).unwrap_err();
    assert!(
        !why.contains('\u{1b}') && !why.contains('\u{202e}'),
        "{why:?}"
    );
}

#[test]
fn a_project_s_false_turns_it_off_and_its_true_does_nothing() {
    let scratch = Scratch::new("project");
    let project = scratch.0.join("project");
    fs::create_dir_all(project.join(".claude")).unwrap();
    assert_eq!(project_off(&project), Ok(false));
    for name in ["settings.json", "settings.local.json"] {
        let file = project.join(".claude").join(name);
        fs::write(&file, r#"{"remoteControlAtStartup": true}"#).unwrap();
        assert_eq!(project_off(&project), Ok(false), "{name} true");
        fs::write(&file, r#"{"remoteControlAtStartup": false}"#).unwrap();
        assert_eq!(project_off(&project), Ok(true), "{name} false");
        fs::write(&file, "{").unwrap();
        assert!(project_off(&project).is_err(), "{name} unread");
        fs::remove_file(&file).unwrap();
    }
}
