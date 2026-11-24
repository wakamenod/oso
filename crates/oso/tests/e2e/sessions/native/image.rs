//! `C-c C-y` in a native session (ADR 0142; ADR 0165, part 2f), with the fakes of the composer, the
//! request and the resume flows: from the composer and from the transcript, the clipboard's image is
//! kept in a file and its `@PATH` goes into the composer, not sent; while a request waits, whose
//! line it answers nothing of; stopped; with the composer hidden; and not into one that holds secret
//! text, for which no file is kept.
//!
//! Must not depend on: the app's internals. The clipboard is the one in memory, the only one the dev
//! socket's input reads.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use super::super::super::harness::App;
use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::compose::COMPOSE_FAKE;
use super::request::{Keyboard, asking, asks, request};
use super::{Flow, MESSAGE, end_turn, in_composer, joined, text};

fn echo(state: &Value) -> String {
    state["echo"].as_str().unwrap_or_default().to_owned()
}

/// Puts an image, `bytes` as a PNG, on the clipboard.
fn copy_png(app: &mut App, bytes: &[u8]) {
    app.call("clipboard", Some(json!({ "image": "png", "bytes": bytes })));
}

/// `C-c C-y` where point is, and the state once it said something.
fn yank(app: &mut App) -> Value {
    app.run_command("keyboard-quit", None).unwrap();
    let quit = echo(&app.state());
    app.send_keys("C-c C-y");
    app.wait_for("what the yank said", |state| {
        let said = echo(state);
        said != quit && !said.is_empty() && !said.starts_with("C-c")
    })
}

/// The images kept for the flow's sessions.
pub(super) fn kept(flow: &Flow) -> Vec<PathBuf> {
    let home = flow.scratch.path().join("home");
    let cache = match cfg!(target_os = "macos") {
        true => home.join("Library").join("Caches"),
        false => home.join(".cache"),
    };
    let mut kept: Vec<PathBuf> = (fs::read_dir(cache.join("oso").join("images")))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|session| fs::read_dir(session.path()).unwrap().flatten())
        .map(|file| file.path())
        .collect();
    kept.sort();
    kept
}

/// What the echo area says of the one image kept as `path`.
fn said(path: &std::path::Path) -> String {
    let name = path.file_name().unwrap().to_string_lossy();
    format!("Image saved as {name}: C-c C-c in its composer sends it")
}

/// The flow, its session's first message sent and answered, point in its empty composer.
fn started(name: &str) -> (Flow, App) {
    let flow = Flow::new(name);
    let script = COMPOSE_FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(flow.scratch.path().join("fake-claude"), script).unwrap();
    let init = r#"{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"m"}"#;
    flow.scratch
        .write("stream.jsonl", format!("{init}\n").as_bytes());
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.contains("restricted"))
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys(MESSAGE));
    app.send_keys("C-c C-c");
    assert_eq!(flow.wait_for_file("sent.1").trim(), MESSAGE);
    end_turn(&flow, &mut app, "release.1");
    (flow, app)
}

fn transcript(app: &mut App) {
    app.send_keys("C-c C-o");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_clipboards_image_goes_into_the_composer_from_it_and_from_the_transcript() {
    let (flow, mut app) = started("native-image");

    // With none on the clipboard, it says so and the draft stays.
    app.send_keys(&keys("look at"));
    let state = yank(&mut app);
    assert_eq!(echo(&state), "No image on the clipboard");
    assert_eq!(joined(&state), "look at");

    // At point, not sent, with point after it.
    let png = [0x89, b'P', b'N', b'G', 1, 2, 3];
    copy_png(&mut app, &png);
    let state = yank(&mut app);
    let first = kept(&flow);
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(fs::read(&first[0]).unwrap(), png);
    assert_eq!(echo(&state), said(&first[0]));
    assert!(in_composer(&state), "{state}");
    app.send_keys(&keys("now"));
    let path = first[0].display().to_string();
    let state = app.state();
    assert_eq!(joined(&state), format!("look at @{path} now"));
    assert!(!flow.scratch.path().join("sent.2").exists(), "not sent");
    app.send_keys("C-c C-c");
    assert_eq!(
        flow.wait_for_file("sent.2").trim(),
        format!("look at @{path} now")
    );
    end_turn(&flow, &mut app, "release.2");

    // From the transcript: into the composer, which is selected, and the echo area says so.
    transcript(&mut app);
    copy_png(&mut app, &[7]);
    let state = yank(&mut app);
    let both = kept(&flow);
    assert_eq!(both.len(), 2, "{both:?}");
    let second = both.iter().find(|path| **path != first[0]).unwrap();
    assert_eq!(echo(&state), said(second));
    assert!(in_composer(&state), "{state}");
    assert_eq!(joined(&state), format!(" @{} ", second.display()));
    app.send_keys("C-x h C-w");
    // The kill took the image's place on the clipboard.
    copy_png(&mut app, &[8]);

    // A composer whose window is too short for it takes it, and the transcript keeps the keys.
    transcript(&mut app);
    app.send_keys("C-x 2");
    app.run_command("shrink-window", Some(40)).unwrap();
    app.send_keys("i");
    let state = app.state();
    assert!(
        echo(&state).starts_with("The composer is hidden"),
        "{state}"
    );
    let state = yank(&mut app);
    assert_eq!(state["buffer"]["name"], "*claude: proj*", "{state}");
    assert_eq!(kept(&flow).len(), 3);
    assert!(echo(&state).starts_with("Image saved as "), "{state}");
    app.send_keys("C-x 1 i");
    let state = app.wait_for("the composer", in_composer);
    assert!(joined(&state).starts_with(" @"), "{state}");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_secret_composer_takes_no_image_and_none_is_kept() {
    let (flow, mut app) = started("native-image-secret");
    transcript(&mut app);
    let env = flow.scratch.write(".env", b"KEY=abc\n");
    app.open_file(&env);
    app.wait_for("the secret file", |state| state["buffer"]["secret"] == true);
    app.send_keys("C-k");
    app.send_keys(&format!("C-x b {} RET", keys("*claude: proj*")));
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    app.send_keys("C-y");
    app.wait_for("the secret text", |state| {
        in_composer(state) && state["buffer"]["secret"] == true
    });

    // Refused before the clipboard is read, which holds no image yet; from its transcript too. The
    // state says no echo while the secret is shown: `*Messages*` has it.
    app.send_keys("C-c C-y");
    transcript(&mut app);
    app.send_keys("C-c C-y");
    std::thread::sleep(Duration::from_millis(300));
    let state = app.state();
    assert_eq!(state["buffer"]["name"], "*claude: proj*", "{state}");
    app.send_keys(&format!("C-x b {} RET", keys("*Messages*")));
    let state = app.wait_for("the messages", |state| {
        state["buffer"]["name"] == "*Messages*"
    });
    let said = text(&state);
    assert!(said.contains("The composer holds secret text"), "{said}");
    assert!(!said.contains("No image on the clipboard"), "{said}");
    // Nor with one there.
    copy_png(&mut app, &[1, 2]);
    app.send_keys(&format!("C-x b {} RET", keys("*claude: proj*")));
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    app.send_keys("C-c C-y");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(kept(&flow), Vec::<PathBuf>::new());
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_image_goes_into_the_composer_while_a_request_waits_and_answers_nothing() {
    let (flow, mut app) = asking("native-image-request");
    asks(&flow, &mut app, "ask", Keyboard::Japanese);
    request(&mut app, "the request");
    copy_png(&mut app, &[1, 2, 3]);
    // From the transcript, the request still waiting.
    let state = yank(&mut app);
    let first = kept(&flow);
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(echo(&state), said(&first[0]));
    assert!(in_composer(&state), "{state}");
    assert!(!state["request"].is_null(), "{state}");
    // From the composer, after what is there; a letter after it is text.
    let state = yank(&mut app);
    assert_eq!(kept(&flow).len(), 2);
    assert!(!state["request"].is_null(), "{state}");
    app.send_keys("y");
    let state = app.state();
    let shown = joined(&state);
    assert_eq!(shown.matches('@').count(), 2, "{shown}");
    assert!(shown.ends_with(" y"), "{shown}");
    assert!(!state["request"].is_null(), "{state}");
    // On the request's line it is no key.
    app.send_keys("C-c C-j");
    app.wait_for("point on the line", super::request::on_line);
    app.send_keys("C-c C-y");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(kept(&flow).len(), 2);
    super::request::until(&mut app, "some of it read", |state| {
        state["request"]["may_ask"] == true
    });
    super::request::answer(&mut app, "n");
    // Its answer leads back to the draft, which holds both.
    let state = app.wait_for("the draft", in_composer);
    assert!(state["request"].is_null(), "{state}");
    assert_eq!(joined(&state).matches('@').count(), 2, "{state}");
    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_stopped_sessions_composer_takes_an_image() {
    let flow = Flow::new("native-image-stopped");
    let mut app = super::resume::spoken_and_stopped(&flow);
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    copy_png(&mut app, &[9]);
    let state = yank(&mut app);
    let kept = kept(&flow);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(echo(&state), said(&kept[0]));
    assert_eq!(joined(&state), format!(" @{} ", kept[0].display()));
    assert!(!text(&state).is_empty());
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
