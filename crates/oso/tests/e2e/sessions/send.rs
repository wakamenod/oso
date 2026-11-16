//! Sending to hosted agent sessions (ADR 0121, step 5), with a fake `claude` that turns
//! bracketed paste on, logs every byte it reads, and prints the screen a flow writes for it:
//! a region, a note and a place arrive as one bracketed paste with no Enter after it; a
//! dialog refuses a send; a buffer that may hold secrets sends its place only; a mention is
//! inserted or pasted; a message line notes the row and `C-x a f` shows its sender; and
//! `C-c C-y` pastes an `@` reference to the clipboard's image, kept in a private file (ADR 0142);
//! a session carried over a restart is judged and takes a send in the new instance.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: composing the
//! text, the refusals, the message rule and the completions have their own tests.

use std::fs;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Flow, asks, terminal};
use crate::harness::App;

/// The fake: raw mode with no echo, bracketed paste on, its input logged to `input.NAME`, and
/// `screen.NAME` printed each time a flow writes it. `DIR` is the flow's scratch directory.
const FAKE: &str = r#"#!/bin/sh
dir='DIR'
name=''
for a in "$@"; do
  case "$a" in --name=*) name=${a#--name=} ;; esac
done
stty raw -echo opost onlcr
printf '\033[?2004h'
printf 'fake ready %s\n' "$name"
exec 3<&0
cat <&3 > "$dir/input.$name" &
reader=$!
while [ ! -f "$dir/go" ]; do
  if [ -f "$dir/screen.$name" ]; then
    printf '\033[2J\033[H'
    cat "$dir/screen.$name"
    rm -f "$dir/screen.$name"
  fi
  sleep 0.1
done
kill "$reader" 2>/dev/null
exit "$(cat "$dir/go")"
"#;

const PERMISSION: &str = include_str!("../../../src/sessions/screens/permission-bash.txt");
const WORKING: &str = include_str!("../../../src/sessions/screens/working.txt");

const START: &str = "\u{1b}[200~";
const END: &str = "\u{1b}[201~";

fn flow(test: &str) -> Flow {
    let flow = Flow::new(test);
    let fake = FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(flow.path("fake-claude"), fake).unwrap();
    fs::write(flow.project.join("notes.md"), "one\ntwo\nthree\n").unwrap();
    fs::write(flow.project.join(".env"), "TOKEN=fixture\n").unwrap();
    flow
}

/// Has session `name`'s fake print `screen`.
fn print(flow: &Flow, name: &str, screen: &str) {
    let staged = flow.path(&format!("screen.{name}.new"));
    fs::write(&staged, screen).unwrap();
    fs::rename(staged, flow.path(&format!("screen.{name}"))).unwrap();
}

/// What session `name`'s fake has read.
fn input(flow: &Flow, name: &str) -> String {
    fs::read_to_string(flow.path(&format!("input.{name}"))).unwrap_or_default()
}

/// Waits until session `name`'s fake has read `pastes` whole bracketed pastes, and returns
/// what it read.
fn wait_for_pastes(flow: &Flow, name: &str, pastes: usize) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let read = input(flow, name);
        if read.matches(END).count() >= pastes {
            return read;
        }
        assert!(Instant::now() < deadline, "{pastes} pastes, read {read:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The `n`th bracketed paste in `read`, from 1, and what follows it up to the next.
fn paste(read: &str, n: usize) -> (&str, &str) {
    let mut rest = read;
    for _ in 1..n {
        rest = &rest[rest.find(END).unwrap() + END.len()..];
    }
    let start = rest.find(START).unwrap() + START.len();
    let end = rest.find(END).unwrap();
    let after = &rest[end + END.len()..];
    let after = &after[..after.find(START).unwrap_or(after.len())];
    (&rest[start..end], after)
}

/// The keys that type `text`.
fn keys(text: &str) -> String {
    let keys: Vec<String> = text
        .chars()
        .map(|c| match c {
            ' ' => "SPC".to_owned(),
            c => c.to_string(),
        })
        .collect();
    keys.join(" ")
}

fn echo(state: &Value) -> String {
    state["echo"].as_str().unwrap_or_default().to_owned()
}

/// The row of the session called `name` in the list.
fn row<'a>(state: &'a Value, name: &str) -> Option<&'a Value> {
    state["sessions"]["sessions"]
        .as_array()?
        .iter()
        .find(|session| session["name"] == name)
}

fn state_of(state: &Value, name: &str) -> Option<String> {
    row(state, name)?["state"].as_str().map(str::to_owned)
}

/// Starts session `name`'s fake with `claude`, and waits until the rules have judged its
/// screen, which a send waits for.
fn start_session(flow: &Flow, app: &mut App, buffer: &str, name: &str) {
    app.run_command("claude", None).unwrap();
    // A second session in one directory asks first (step 4).
    let state = app.wait_for("the fake or a question", |state| {
        asks(state).is_some() || terminal(state, buffer).is_some()
    });
    if asks(&state).is_some() {
        app.send_keys("y");
    }
    app.wait_for("the fake", |state| {
        terminal(state, buffer).is_some_and(|terminal| {
            terminal["rows"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row == &format!("fake ready {name}")))
        })
    });
    print(flow, name, WORKING);
    app.wait_for("judged working", |state| {
        state_of(state, name).as_deref() == Some("working")
    });
}

/// Opens the file at `path` and waits until it is loaded and, unless `secret`, checked and
/// found to hold no secret.
fn open(app: &mut App, path: &std::path::Path, secret: bool) {
    app.open_file(path);
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    app.wait_for("the file loaded", |state| {
        let buffer = &state["buffer"];
        buffer["name"] == name.as_str() && buffer["loading"].is_null() && buffer["secret"] == secret
    });
}

/// Lets every fake exit, waits until oso has seen each end, and quits.
fn end(flow: &Flow, app: &mut App, names: &[&str]) {
    flow.exit_with(0);
    app.wait_for("the sessions' ends", |state| {
        names
            .iter()
            .all(|name| state_of(state, name).is_some_and(|s| s == "stopped" || s == "failed"))
    });
    super::quit(app);
}

/// `C-x a m` to session `buffer`, then the note `note` if one is asked.
fn send_to(app: &mut App, buffer: &str, note: Option<&str>) -> Value {
    let before = echo(&app.state());
    app.send_keys("C-x a m");
    app.wait_for("the session prompt", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Send to session"))
    });
    app.send_keys(&format!("{} RET", keys(buffer)));
    if let Some(note) = note {
        app.wait_for("the note prompt", |state| {
            asks(state).is_some_and(|prompt| prompt.starts_with("Note"))
        });
        let typed = if note.is_empty() {
            "RET".to_owned()
        } else {
            format!("{} RET", keys(note))
        };
        app.send_keys(&typed);
    }
    app.wait_for("what the send said", |state| {
        asks(state).is_none() && echo(state) != before && !echo(state).is_empty()
    })
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_send_is_one_bracketed_paste_not_submitted_and_a_dialog_refuses_it() {
    let flow = flow("sessions-send");
    let mut app = flow.start();
    let session = "*claude: proj*";
    start_session(&flow, &mut app, session, "proj");

    // A region: its place and its text in a fence, with no Enter after it, and the session
    // shown.
    open(&mut app, &flow.project.join("notes.md"), false);
    app.send_keys("C-x h");
    let state = send_to(&mut app, session, None);
    assert_eq!(echo(&state), "Pasted into proj: press Enter there");
    assert_eq!(state["buffer"]["name"], session);
    let read = wait_for_pastes(&flow, "proj", 1);
    assert_eq!(
        paste(&read, 1),
        ("notes.md:1-3\n```\none\ntwo\nthree\n```", "")
    );

    // No region: a note, as written.
    open(&mut app, &flow.project.join("notes.md"), false);
    app.send_keys("C-g");
    send_to(&mut app, session, Some("look at this"));
    let read = wait_for_pastes(&flow, "proj", 2);
    assert_eq!(paste(&read, 2), ("look at this", ""));

    // `C-u` with no region: the line at point, as its place only.
    open(&mut app, &flow.project.join("notes.md"), false);
    app.send_keys("C-g C-n C-u");
    send_to(&mut app, session, Some(""));
    let read = wait_for_pastes(&flow, "proj", 3);
    assert_eq!(paste(&read, 3).0, "notes.md:2");

    // A buffer that may hold secrets: its place only, and the echo area says so.
    open(&mut app, &flow.project.join(".env"), true);
    // The state withholds the prompt while the buffer is shown; the send shows the session.
    app.send_keys(&format!("C-x h C-x a m {} RET", keys(session)));
    let state = app.wait_for("the send", |state| {
        state["buffer"]["name"] == session && echo(state).contains("may hold secrets")
    });
    assert_eq!(
        echo(&state),
        "Pasted into proj: press Enter there. Only the place went, as the buffer may hold secrets"
    );
    let read = wait_for_pastes(&flow, "proj", 4);
    assert_eq!(paste(&read, 4).0, ".env:1");
    assert!(!read.contains("fixture"), "{read:?}");

    // A dialog: refused, and nothing reaches it.
    print(&flow, "proj", PERMISSION);
    app.wait_for("needs input", |state| {
        state_of(state, "proj").is_some_and(|s| s.starts_with("needs input"))
    });
    open(&mut app, &flow.project.join("notes.md"), false);
    let state = send_to(&mut app, session, Some("no"));
    assert_eq!(
        echo(&state),
        "proj is asking for permission; answer it first"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(input(&flow, "proj").matches(END).count(), 4);

    mention(&flow, &mut app, session);
    end(&flow, &mut app, &["proj"]);
}

/// A session carried over a restart is judged in the new instance, which tells its states and
/// takes a send: its screens went unjudged once, and every send was refused as not judged yet.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_session_carried_over_a_restart_is_judged_and_takes_a_send() {
    let flow = flow("sessions-send-restart");
    let mut old = flow.start();
    let session = "*claude: proj*";
    start_session(&flow, &mut old, session, "proj");
    let mut new = flow.start();
    crate::terminal::restart::hand_over(&mut old, &mut new);
    new.wait_for("the session carried", |state| {
        terminal(state, session).is_some_and(|terminal| terminal["running"] == true)
    });
    print(&flow, "proj", PERMISSION);
    new.wait_for("needs input", |state| {
        state_of(state, "proj").is_some_and(|s| s.starts_with("needs input"))
    });
    print(&flow, "proj", WORKING);
    new.wait_for("judged working", |state| {
        state_of(state, "proj").as_deref() == Some("working")
    });
    open(&mut new, &flow.project.join("notes.md"), false);
    let state = send_to(&mut new, session, Some("after the restart"));
    assert_eq!(echo(&state), "Pasted into proj: press Enter there");
    let read = wait_for_pastes(&flow, "proj", 1);
    assert_eq!(paste(&read, 1), ("after the restart", ""));
    end(&flow, &mut new, &["proj"]);
}

/// `C-x a @` inserts the name in a file's buffer, and pastes `@NAME ` in a session's terminal.
fn mention(flow: &Flow, app: &mut App, session: &str) {
    app.send_keys("M-<");
    print(flow, "proj", WORKING);
    app.wait_for("working", |state| {
        state_of(state, "proj").as_deref() == Some("working")
    });
    app.send_keys("C-x a @");
    app.wait_for("the mention prompt", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Mention session"))
    });
    app.send_keys("p r o j RET");
    app.wait_for("the prompt closed", |state| asks(state).is_none());
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 4 })));
    assert_eq!(text["rows"][0], "@projone", "{text}");
    // Undone, so that quitting asks about no unsaved file.
    app.run_command("undo", None).unwrap();
    app.wait_for("notes.md unmodified", |state| {
        state["buffer"]["modified"] == false
    });
    app.send_keys("C-x a b");
    app.send_keys(&format!("{} RET", keys(session)));
    app.wait_for("the session shown", |state| {
        state["buffer"]["name"] == session
    });
    app.send_keys("C-x a @");
    app.wait_for("the mention prompt", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Mention session"))
    });
    app.send_keys("p r o j RET");
    let read = wait_for_pastes(flow, "proj", 5);
    assert_eq!(paste(&read, 5), ("@proj ", ""));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_message_line_notes_the_row_until_seen_and_c_x_a_f_shows_its_sender() {
    let flow = flow("sessions-message");
    let mut app = flow.start();
    start_session(&flow, &mut app, "*claude: proj*", "proj");
    start_session(&flow, &mut app, "*claude: proj-2*", "proj-2");

    // Away from proj, a message from proj-2 shows on its screen.
    open(&mut app, &flow.project.join("notes.md"), false);
    print(
        &flow,
        "proj",
        "› Message from @proj-2: fixture ping (ctrl+o to expand)\n",
    );
    app.wait_for("the row's note", |state| {
        row(state, "proj").is_some_and(|row| {
            row["recap"]
                .as_str()
                .is_some_and(|recap| recap.starts_with("message from proj-2"))
        })
    });

    // From proj's terminal, `C-x a f` shows the sender.
    app.send_keys("C-x a b");
    app.send_keys(&format!("{} RET", keys("*claude: proj*")));
    app.wait_for("proj shown", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    app.send_keys("C-x a f");
    app.wait_for("the sender shown", |state| {
        state["buffer"]["name"] == "*claude: proj-2*"
    });

    end(&flow, &mut app, &["proj", "proj-2"]);
}

/// `C-c C-y` in the selected session, and the state once the echo area has said something new.
fn yank_image(app: &mut App) -> Value {
    let before = echo(&app.state());
    app.send_keys("C-c C-y");
    app.wait_for("what the yank said", |state| {
        echo(state) != before && !echo(state).is_empty()
    })
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_c_c_y_pastes_a_reference_to_the_clipboard_s_image_kept_in_a_private_file() {
    use std::os::unix::fs::PermissionsExt;
    let flow = flow("sessions-yank-image");
    let mut app = flow.start();
    let session = "*claude: proj*";
    start_session(&flow, &mut app, session, "proj");
    app.wait_for("the session selected", |state| {
        state["buffer"]["name"] == session
    });

    let state = yank_image(&mut app);
    assert_eq!(echo(&state), "No image on the clipboard");

    // A PNG goes as it came, by path, with a space each side and no Enter.
    let png: Vec<u8> = vec![0x89, b'P', b'N', b'G', 1, 2, 3];
    app.call(
        "clipboard",
        Some(json!({ "image": "png", "bytes": png.clone() })),
    );
    let state = yank_image(&mut app);
    let cache = if cfg!(target_os = "macos") {
        flow.home.join("Library").join("Caches")
    } else {
        flow.home.join(".cache")
    };
    let images = cache.join("oso").join("images");
    let read = wait_for_pastes(&flow, "proj", 1);
    let (pasted, after) = paste(&read, 1);
    assert_eq!(after, "", "not submitted");
    let path = pasted
        .strip_prefix(" @")
        .and_then(|path| path.strip_suffix(' '))
        .unwrap_or_else(|| panic!("{pasted:?}"));
    let path = std::path::Path::new(path);
    assert!(path.starts_with(&images), "{path:?}");
    assert_eq!(path.extension().unwrap(), "png");
    assert_eq!(fs::read(path).unwrap(), png);
    let mode = |path: &std::path::Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    // The echo area, which the agent channel reads, names the file and not its directory.
    assert_eq!(
        echo(&state),
        format!(
            "Image saved as {}: press Enter there",
            path.file_name().unwrap().to_string_lossy()
        )
    );

    // An image the app that copied it marked secret is not read.
    app.call(
        "clipboard",
        Some(json!({ "image": "png", "bytes": [1], "concealed": true })),
    );
    let state = yank_image(&mut app);
    assert_eq!(
        echo(&state),
        "The app that copied the image marked it secret; nothing is attached"
    );

    // A dialog refuses it before anything is kept, as it refuses a send.
    app.call("clipboard", Some(json!({ "image": "gif", "bytes": [7] })));
    print(&flow, "proj", PERMISSION);
    app.wait_for("needs input", |state| {
        state_of(state, "proj").is_some_and(|s| s.starts_with("needs input"))
    });
    let state = yank_image(&mut app);
    assert_eq!(
        echo(&state),
        "proj is asking for permission; answer it first"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(input(&flow, "proj").matches(END).count(), 1);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);

    // Archived, the session's images go.
    app.send_keys("C-c A");
    let directory = path.parent().unwrap().to_path_buf();
    let deadline = Instant::now() + Duration::from_secs(10);
    while directory.exists() {
        assert!(Instant::now() < deadline, "{directory:?} is still there");
        std::thread::sleep(Duration::from_millis(50));
    }
    end(&flow, &mut app, &["proj"]);
}
