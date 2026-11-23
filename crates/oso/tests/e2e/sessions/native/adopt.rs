//! A conversation started outside oso, adopted under native's default (ADR 0165, part 4e),
//! with resume's fake `claude -p`: at a trusted project's root it resumes natively in a new
//! buffer, with an "Adopted" row and the composer selected; at a restricted project's root, or
//! below a trusted one's, it runs in a terminal.
//!
//! Must not depend on: the app's internals.

use std::fs;

use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::super::adopt::{adopt, transcript};
use super::resume::RESUME_FAKE;
use super::{Flow, in_composer, text};

/// Conversations that ran at the project's root, and one that ran below it.
const RESTRICTED: &str = "9abc0123-0000-4000-8000-00000000eeee";
const AT_ROOT: &str = "1234abcd-0000-4000-8000-00000000cccc";
const BELOW: &str = "5678cdef-0000-4000-8000-00000000dddd";

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_adopted_conversation_is_native_at_a_trusted_root_and_a_terminal_elsewhere() {
    let flow = Flow::new("native-adopt");
    let fake = flow.scratch.path().join("fake-claude");
    let script = RESUME_FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(&fake, script).unwrap();
    let init = r#"{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"m"}"#;
    flow.scratch
        .write("stream.jsonl", format!("{init}\n").as_bytes());
    // The native run's first message answers at once.
    flow.scratch.write("release.2.1", b"");
    let home = flow.scratch.path().join("home");
    let root = flow.project.canonicalize().unwrap();
    let below = root.join("sub");
    fs::create_dir(&below).unwrap();
    transcript(&home, RESTRICTED, &root, Some("restricted-root"));
    transcript(&home, AT_ROOT, &root, Some("at-root"));
    transcript(&home, BELOW, &below, Some("below-root"));
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));

    // At the root of a project not trusted: Claude Code's TUI, in a terminal (Q5).
    adopt(&mut app, "restricted-root");
    let args = flow.wait_for_file("run.1");
    assert!(
        args.contains(&format!("arg --resume={RESTRICTED}")),
        "{args}"
    );
    assert!(!args.contains("--input-format"), "{args}");
    // Back to the file, whose project is trusted now.
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        !(state["modeline"].as_str()).is_some_and(|modeline| modeline.contains("restricted"))
    });

    // At the root: native, as no `mode` says otherwise, in a buffer of its own.
    adopt(&mut app, "at-root");
    let args = flow.wait_for_file("run.2");
    assert!(args.contains(&format!("arg --resume={AT_ROOT}")), "{args}");
    assert!(args.contains("arg --input-format=stream-json"), "{args}");
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys("go on"));
    app.send_keys("C-c C-c");
    assert_eq!(flow.wait_for_file("sent.2.1").trim(), "go on");
    app.send_keys("C-c C-o");
    let state = app.wait_for("the reply", |state| text(state).contains("reply 2.1"));
    assert_eq!(state["buffer"]["name"], "*claude: at-root*");
    assert!(text(&state).contains("Adopted."), "{}", text(&state));

    // Below the root: in a terminal too.
    adopt(&mut app, "below-root");
    let args = flow.wait_for_file("run.3");
    assert!(args.contains(&format!("arg --resume={BELOW}")), "{args}");
    assert!(!args.contains("--input-format"), "{args}");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
