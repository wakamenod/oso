//! The `/add-dir` offer to a space's orchestrator (ADR 0136, step 3), with `git_agent`'s fake
//! `claude`, which logs what it reads: a repository trusted while the orchestrator runs is
//! offered, and `y` pastes `/add-dir PATH` into its terminal as it is, oso's own command, not
//! escaped as composed text is (ADR 0165, part 2f), and not submitted.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: which paths make
//! the command has its own tests.

use std::fs;

use super::git_agent::{END, FAKE, WORKING, pastes};
use super::spaces::{PrivateRoot, answer, space, space_menu, trust_restricted};
use super::worktrees::{Flow, asks, terminal};

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn y_to_the_add_dir_offer_pastes_the_command_as_it_is_not_submitted() {
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
    let orchestrator = "*claude: fix-login*";
    app.wait_for("the orchestrator, ready and recorded", |state| {
        space(state, "fix-login").is_some_and(|s| s["orchestrator"].is_string())
            && terminal(state, orchestrator).is_some_and(|terminal| {
                (terminal["rows"].as_array())
                    .is_some_and(|rows| rows.iter().any(|row| row == "fake ready"))
            })
    });
    // Judged working, so that it takes a paste.
    flow.scratch.write("screen", WORKING.as_bytes());
    app.wait_for("judged working", |state| {
        (state["sessions"]["sessions"].as_array()).is_some_and(|rows| {
            rows.iter()
                .any(|row| row["name"] == "fix-login" && row["state"] == "working")
        })
    });

    // A repository oso does not trust yet, added while it runs, then trusted: offered.
    space_menu(&mut app, "a");
    answer(&mut app, "Add repository:", other.to_str().unwrap());
    app.wait_for("its second repository, not trusted", |state| {
        space(state, "fix-login")
            .is_some_and(|s| s["members"].as_array().is_some_and(|m| m.len() == 2))
    });
    app.open_file(&other.join("notes.md"));
    trust_restricted(&mut app, "other");
    let state = app.wait_for("the offer", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Paste /add-dir"))
    });
    let question = asks(&state).unwrap();
    let real = fs::canonicalize(&other).unwrap();
    let command = [&real, &other]
        .map(|path| format!("/add-dir {}", path.display()))
        .into_iter()
        .find(|command| question.starts_with(&format!("Paste {command} into ")))
        .unwrap_or_else(|| panic!("{question}"));

    // `y`: the command as it is, with no space before its `/`, pasted and not submitted.
    app.send_keys("y");
    let read = pastes(&flow, 1);
    assert_eq!(
        read.rsplit("\u{1b}[200~").next(),
        Some(format!("{command}{END}").as_str()),
        "{read:?}"
    );
    assert!(!read.contains(" /add-dir"), "{read:?}");
    flow.exit_with(0);
}
