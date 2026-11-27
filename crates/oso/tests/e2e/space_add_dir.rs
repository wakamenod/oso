//! A repository given to a space's running orchestrator without asking (ADR 0185), with
//! `git_agent`'s fake `claude`, which logs what it reads and prints the screen a flow writes: a
//! repository trusted while the orchestrator runs is typed as `/add-dir PATH`, oso's own
//! command, not escaped as composed text is (ADR 0165, part 2f), only once its turn ended and
//! its input box is empty, then submitted once the box shows the command alone; a dialog after
//! it is said, never answered.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: which paths make
//! the command, and which screens hold it, have their own tests.

use std::fs;
use std::time::{Duration, Instant};

use super::git_agent::{END, FAKE, WORKING, echo, pastes};
use super::spaces::{PrivateRoot, answer, space, space_menu, trust_restricted};
use super::worktrees::{Flow, terminal};
use crate::harness::App;

const ORCHESTRATOR: &str = "*claude: fix-login*";

/// A rule of the input box, 20 columns, the fewest oso reads as a box's.
fn rule() -> String {
    "\u{2500}".repeat(20)
}

/// A screen with Claude Code's input box holding `input`, cut into rows of 16 characters
/// with the box's indent, so that it fits any window the flow shows it in, as the TUI wraps.
fn boxed(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut rows: Vec<String> = chars
        .chunks(16)
        .map(|chunk| chunk.iter().collect())
        .collect();
    if rows.is_empty() {
        rows.push(String::new());
    }
    let first = format!("\u{276f}\u{a0}{}", rows[0]);
    let rest: String = rows[1..].iter().map(|row| format!("  {row}\n")).collect();
    format!(
        "talk\n{}\n{first}\n{rest}{}\n  ? for shortcuts\n",
        rule(),
        rule()
    )
}

/// Shows `screen` in the fake's terminal, and waits until oso has a row `row` of it.
fn show(app: &mut App, flow: &Flow, screen: &str, row: &str) {
    flow.scratch.write("screen", screen.as_bytes());
    app.wait_for("the screen", |state| {
        terminal(state, ORCHESTRATOR).is_some_and(|terminal| {
            (terminal["rows"].as_array())
                .is_some_and(|rows| rows.iter().any(|shown| shown.as_str() == Some(row)))
        })
    });
}

/// What the fake has read so far.
fn read(flow: &Flow) -> String {
    fs::read_to_string(flow.scratch.path().join("input")).unwrap_or_default()
}

/// Waits until the fake's input ends with a submitted paste.
fn submitted(flow: &Flow) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let read = read(flow);
        if read.ends_with(&format!("{END}\r")) {
            return read;
        }
        assert!(Instant::now() < deadline, "no Enter, read {read:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_trusted_repository_is_typed_as_add_dir_and_sent_once_the_orchestrator_is_idle() {
    let spaces = PrivateRoot::new("space-add-dir");
    let config = format!("\n[spaces]\nroot = \"{}\"\n", spaces.0.display());
    let flow = Flow::with_fake("space-add-dir", &config, FAKE);
    let other = flow.scratch.path().join("other");
    fs::create_dir(&other).unwrap();
    flow.git(&other, &["init", "-q", "-b", "main"]);
    fs::write(other.join("notes.md"), "# other\n").unwrap();
    let (mut app, _) = flow.start();
    app.send_keys("C-c a O");
    answer(&mut app, "Describe the space:", "Fix login");
    app.wait_for("the orchestrator, ready and recorded", |state| {
        space(state, "fix-login").is_some_and(|s| s["orchestrator"].is_string())
            && terminal(state, ORCHESTRATOR).is_some_and(|terminal| {
                (terminal["rows"].as_array())
                    .is_some_and(|rows| rows.iter().any(|row| row == "fake ready"))
            })
    });
    // Working: its turn runs.
    flow.scratch.write("screen", WORKING.as_bytes());
    app.wait_for("judged working", |state| {
        (state["sessions"]["sessions"].as_array()).is_some_and(|rows| {
            rows.iter()
                .any(|row| row["name"] == "fix-login" && row["state"] == "working")
        })
    });

    // A repository oso does not trust yet, added while it runs, then trusted: nothing is
    // asked, and nothing is typed while its turn runs.
    space_menu(&mut app, "a");
    answer(&mut app, "Add repository:", other.to_str().unwrap());
    app.wait_for("its second repository, not trusted", |state| {
        space(state, "fix-login")
            .is_some_and(|s| s["members"].as_array().is_some_and(|m| m.len() == 2))
    });
    app.open_file(&other.join("notes.md"));
    trust_restricted(&mut app, "other");
    app.wait_for("other trusted", |state| {
        (state["modeline"].as_str()).is_some_and(|line| !line.contains("restricted"))
    });
    assert_eq!(
        app.state()["prompts"],
        serde_json::json!([]),
        "nothing is asked"
    );
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!read(&flow).contains("\u{1b}[200~"), "{:?}", read(&flow));
    let real = fs::canonicalize(&other).unwrap();
    let commands = [&real, &other].map(|path| format!("/add-dir {}", path.display()));

    // Its turn ended with a draft in its box: still nothing typed.
    show(
        &mut app,
        &flow,
        &boxed("my draft"),
        "\u{276f}\u{a0}my draft",
    );
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!read(&flow).contains("\u{1b}[200~"), "{:?}", read(&flow));

    // The box empty: the command as it is, with no space before its `/`, then, once the box
    // shows it alone, one Enter as a key of its own.
    show(&mut app, &flow, &boxed(""), "\u{276f}");
    let typed = pastes(&flow, 1);
    let pasted = (typed.rsplit("\u{1b}[200~").next()).and_then(|paste| paste.strip_suffix(END));
    let command = (commands.iter())
        .find(|command| pasted == Some(command.as_str()))
        .unwrap_or_else(|| panic!("{typed:?}"))
        .clone();
    assert!(!typed.contains(" /add-dir"), "{typed:?}");
    let first = format!(
        "\u{276f}\u{a0}{}",
        command.chars().take(16).collect::<String>()
    );
    show(&mut app, &flow, &boxed(&command), &first);
    let sent = submitted(&flow);
    assert_eq!(sent.matches('\r').count(), 1, "{sent:?}");
    app.wait_for("the send said", |state| {
        echo(state) == format!("Typed {command} into fix-login and sent it")
    });

    // Claude Code asks its own question of the folder: said, never answered.
    let dialog = include_str!("../../src/sessions/screens/start-trust.txt");
    flow.scratch.write("screen", dialog.as_bytes());
    let path = command.trim_start_matches("/add-dir ");
    app.wait_for("the question said", |state| {
        echo(state) == format!("fix-login asks you to trust {path}: answer it in fix-login")
    });
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(read(&flow), sent, "nothing more is typed");
    flow.exit_with(0);
}
