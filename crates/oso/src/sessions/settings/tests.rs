//! Tests of a session's `--settings`: the hook it installs, Remote Control turned off in it, and
//! quoting checked by a real `/bin/sh`.
//!
//! Must not depend on: Claude Code or a spawn of its own; the shell starts as every child of
//! oso does.

use std::path::Path;

use serde_json::{Value, json};

use super::{Parts, settings};

#[test]
fn settings_with_nothing_on_are_none() {
    assert_eq!(settings(Path::new("/bin/oso"), &Parts::default()), Ok(None));
}

#[test]
fn settings_install_the_hook() {
    let parts = Parts {
        steer_worktrees: true,
        ..Parts::default()
    };
    let text = settings(Path::new("/Apps/it's oso/oso"), &parts)
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value,
        json!({"hooks": {"PreToolUse": [{
            "matcher": "EnterWorktree|Bash",
            "hooks": [{"type": "command", "command": r"'/Apps/it'\''s oso/oso' hook deny-worktree"}],
        }]}})
    );
}

/// Turning Remote Control off names no path, so it needs none in UTF-8.
#[cfg(unix)]
#[test]
fn settings_turn_remote_control_off_whatever_osos_path() {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(b"/bin/\xffoso"));
    let parts = Parts {
        disable_remote_control: true,
        ..Parts::default()
    };
    assert_eq!(
        settings(path, &parts),
        Ok(Some(r#"{"disableRemoteControl":true}"#.to_owned()))
    );
}

#[cfg(unix)]
#[test]
fn settings_need_a_utf8_path() {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(b"/bin/\xffoso"));
    let parts = Parts {
        steer_worktrees: true,
        disable_remote_control: true,
    };
    assert_eq!(settings(path, &parts), Err(super::SettingsError::NotUtf8));
    assert_eq!(
        super::SettingsError::NotUtf8.to_string(),
        "oso's own path is not UTF-8, so a hook cannot name it"
    );
}

#[cfg(unix)]
#[test]
fn quoted_text_reaches_the_shell_as_it_is() {
    use std::ffi::OsString;

    use oso_platform::process::{self, Executable, Sink, Source, Spawn};

    use super::shell_quote;

    let sh = Executable::absolute("/bin/sh").unwrap();
    for text in [
        "/Apps/it's oso/oso",
        "a  b",
        "$(touch pwned) `id` $HOME",
        "line\nbreak",
        "'",
        "''\\'",
        "",
    ] {
        let script = format!("printf %s {}", shell_quote(text));
        let args = [OsString::from("-c"), OsString::from(script)];
        let child = process::spawn(&Spawn {
            executable: &sh,
            args: &args,
            cwd: Some(&std::env::temp_dir()),
            project: None,
            inherit: &[],
            stdin: Source::Closed,
            stdout: Sink::Piped,
            stderr: Sink::Null,
            own_process_group: false,
            own_session: false,
            set: &[],
            terminal: None,
            pass_fd: None,
            args_logged: true,
        })
        .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), text, "{text:?}");
    }
}
