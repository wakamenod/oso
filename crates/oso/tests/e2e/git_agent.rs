//! Asking the worktree's agent session from a commit (ADR 0127, step 6a), with a fake `claude`
//! that turns bracketed paste on, logs what it reads, and runs the `oso mcp` its
//! `--mcp-config` names: `C-c C-d` pastes a draft request with an id, `draft_set` with that id
//! puts the draft above the `#` lines and keeps the person's text as `#` lines, and is refused
//! a second time; a failing hook's message names `C-c C-f`, which pastes the failed run's
//! output, and `F` in the log does too.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the requests'
//! texts, the fence and the draft's place have their own tests.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::git_status::{has, rows_until};
use super::harness::App;
use super::review::rows;
use super::session_tools::{Bridge, run_in};
use super::typing;
use super::worktrees::{Flow, keys, terminal};

/// The fake: raw mode with no echo, bracketed paste on, what it reads logged to `input`, the
/// screen a flow writes to `screen` printed, and, once the flow wrote `bridge.PID`, that script
/// run as its child over the FIFO `in.PID`, writing to `out.PID`. `DIR` is the scratch
/// directory.
pub(super) const FAKE: &str = r#"#!/bin/sh
dir='DIR'
printf '%s\0' "$(pwd -P)" "$@" > "$dir/tmp.$$"
mkfifo "$dir/in.$$"
mv "$dir/tmp.$$" "$dir/run.$$"
stty raw -echo opost onlcr
printf '\033[?2004h'
printf 'fake ready\n'
exec 3<&0
cat <&3 > "$dir/input" &
reader=$!
bridged=
while [ ! -f "$dir/go" ]; do
  [ -d "$dir" ] || exit 0
  if [ -z "$bridged" ] && [ -f "$dir/bridge.$$" ]; then
    sh "$dir/bridge.$$" < "$dir/in.$$" > "$dir/out.$$" 2> "$dir/err.$$" &
    bridged=1
  fi
  if [ -f "$dir/screen" ]; then
    printf '\033[2J\033[H'
    cat "$dir/screen"
    rm -f "$dir/screen"
  fi
  sleep 0.1
done
kill "$reader" 2>/dev/null
exit "$(cat "$dir/go")"
"#;

const WORKING: &str = include_str!("../../src/sessions/screens/working.txt");

pub(super) const END: &str = "\u{1b}[201~";

/// How long the channel waits after the person's last key before it changes anything, and a
/// moment more.
const AFTER_TYPING: Duration = Duration::from_millis(1200);

/// What the fake has read once it holds `pastes` whole bracketed pastes.
pub(super) fn pastes(flow: &Flow, pastes: usize) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let read = fs::read_to_string(flow.scratch.path().join("input")).unwrap_or_default();
        if read.matches(END).count() >= pastes {
            return read;
        }
        assert!(Instant::now() < deadline, "{pastes} pastes, read {read:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub(super) fn echo(state: &Value) -> String {
    state["echo"].as_str().unwrap_or_default().to_owned()
}

/// The id the newest draft request in `read` names.
fn request_id(read: &str) -> String {
    let at = read.rfind("id: ").expect("a request id") + "id: ".len();
    read[at..at + 8].to_owned()
}

/// Shows the buffer `name` again with `C-x b`, as the person does after a paste showed the
/// session.
pub(super) fn back_to(app: &mut App, name: &str) {
    app.send_keys(&format!("C-x b {} RET", keys(name)));
    app.wait_for(name, |state| state["buffer"]["name"] == name);
}

/// An instance in `flow`'s repository, its README changed, with a session at its root judged
/// working, which a paste waits for, and that session's bridge.
pub(super) fn hosted(flow: &Flow) -> (App, Bridge) {
    for (name, value) in [
        ("user.name", "e2e"),
        ("user.email", "e2e@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".git/hooks"),
    ] {
        flow.git(&flow.repo, &["config", name, value]);
    }
    fs::write(flow.repo.join("README.md"), "# e2e\nchanged\n").unwrap();
    let (mut app, _) = flow.start();
    app.send_keys("C-c a c");
    let run = run_in(flow, &flow.repo);
    app.wait_for("the fake", |state| {
        terminal(state, "*claude: repo*").is_some_and(|terminal| {
            (terminal["rows"].as_array()).is_some_and(|rows| rows.iter().any(|r| r == "fake ready"))
        })
    });
    let bridge = Bridge::start(flow, &run);
    flow.scratch.write("screen", WORKING.as_bytes());
    app.wait_for("judged working", |state| {
        (state["sessions"]["sessions"].as_array()).is_some_and(|rows| {
            rows.iter()
                .any(|row| row["name"] == "repo" && row["state"] == "working")
        })
    });
    (app, bridge)
}

/// Stages the README in the status and opens a commit's message.
fn open_message(app: &mut App) {
    app.send_keys("C-x g");
    app.wait_for("the status", |state| {
        state["buffer"]["name"] == "*git: repo*"
    });
    rows_until(app, |rows| rows.iter().any(|row| row.contains("README.md")));
    app.send_keys("S");
    rows_until(app, |rows| has(rows, "Staged changes (1)"));
    app.send_keys("c c");
    app.wait_for("the message", |state| {
        state["buffer"]["name"] == "*commit: repo*"
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_session_drafts_the_message_and_is_asked_to_fix_a_failed_run() {
    let flow = Flow::with_fake("git-agent", "", FAKE);
    let (mut app, mut bridge) = hosted(&flow);
    open_message(&mut app);
    app.send_keys(&format!("{} SPC {}", typing("my"), typing("words")));

    // `C-c C-d` pastes the request, not submitted, and shows the session.
    app.send_keys("C-c C-d");
    let read = pastes(&flow, 1);
    assert!(
        read.contains("Draft a commit message for the staged changes in "),
        "{read:?}"
    );
    assert!(read.contains("- base"), "{read:?}");
    assert!(read.ends_with(END), "{read:?}");
    let id = request_id(&read);
    back_to(&mut app, "*commit: repo*");
    std::thread::sleep(AFTER_TYPING);
    let draft = json!({ "kind": "commit", "id": id, "text": "Change the README\n\nIt says more." });
    let (said, error) = bridge.tool("draft_set", draft.clone());
    assert!(!error, "{said}");
    assert_eq!(said["committed"], false);
    let shown = rows(&mut app, 8);
    assert_eq!(
        shown[..3],
        ["Change the README", "", "It says more."],
        "{shown:?}"
    );
    assert!(
        has(&shown, "# your text before the draft from repo:"),
        "{shown:?}"
    );
    assert!(has(&shown, "# my words"), "{shown:?}");
    // Once only.
    let (said, error) = bridge.tool("draft_set", draft);
    assert!(error, "{said}");

    // A failing hook names `C-c C-f`, which pastes the failed run's output.
    let hook = flow.repo.join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        "#!/bin/sh\necho 'lint: README.md:2 is bad'\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    app.send_keys("C-c C-c");
    app.wait_for("the failure", |state| {
        echo(state)
            == "commit failed (hook pre-commit): $ shows the output; C-c C-f asks the session to fix it"
    });
    app.send_keys("C-c C-f");
    let read = pastes(&flow, 2);
    let fix = &read[read.find(END).unwrap()..];
    assert!(read.ends_with(END), "{read:?}");
    assert!(fix.contains("`git commit` failed in "), "{fix:?}");
    assert!(fix.contains("(hook pre-commit)"), "{fix:?}");
    assert!(fix.contains("lint: README.md:2 is bad"), "{fix:?}");
    assert!(fix.contains("Do not commit, do not skip hooks"), "{fix:?}");
    // `F` in the log asks the same, while the message is open.
    back_to(&mut app, "*git log: repo*");
    app.send_keys("F");
    let read = pastes(&flow, 3);
    assert!(
        read.matches("lint: README.md:2 is bad").count() >= 2,
        "{read:?}"
    );
    flow.exit_with(0);
}
