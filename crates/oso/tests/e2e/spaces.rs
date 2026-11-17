//! Issue spaces (ADR 0136). Step 1a: the sessions an earlier oso remembered, in two
//! repositories and a worktree of one, become two spaces of one repository each, the worktree
//! held by its repository's, with nothing started and no space directory made; the `spaces`
//! file keeps them, so that the next start reads the same spaces and makes none again; and
//! `space-switch` goes from one to the other. Step 1b: `space-new` makes a space and its
//! private directory, the space menu adds a repository by its path, renames, closes (stopping
//! an idle session) and reopens, a shared root is refused, and a restart keeps it all. Step 2:
//! the pane shows a space, its two repositories, their worktrees and its session; a quiet
//! space folds while another is in front, `RET` on its row enters it, `a` reads a repository,
//! and a closed space goes in `Archived spaces`.
//!
//! Must not depend on: the app's internals, or what the spaces' own tests cover: the join
//! rule, the file's format and the directory checks.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::harness::emergency_dir;
use super::worktrees::{Flow, keys};

/// oso's state directory for an instance started in `cwd`, as the harness gives it: beside
/// its emergency copies.
fn state_dir(cwd: &Path) -> PathBuf {
    emergency_dir(cwd)
        .parent()
        .expect("the emergency copies' directory has a parent")
        .to_path_buf()
}

/// A remembered session's line: its id from `n`, its name, and where it ran.
fn session_line(n: u8, name: &str, place: &Path) -> String {
    let id = format!("{n:08x}-0000-4000-8000-000000000000");
    let place = place.display();
    format!(
        r#"{{"id":"{id}","name":"{name}","directory":"{place}","root":"{place}","origin":"person","pinned":false,"archived":false,"changed":1794512815000}}"#
    )
}

/// The spaces the dev state lists: each one's name, repositories and worktrees.
fn spaces(state: &Value) -> Vec<(String, Vec<String>, Vec<String>)> {
    let texts = |value: &Value| -> Vec<String> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    state["sessions"]["spaces"]
        .as_array()
        .map(|spaces| {
            spaces
                .iter()
                .map(|space| {
                    let name = space["name"].as_str().unwrap_or_default().to_owned();
                    (name, texts(&space["members"]), texts(&space["worktrees"]))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ids of the spaces the dev state lists.
fn space_ids(state: &Value) -> Vec<String> {
    state["sessions"]["spaces"]
        .as_array()
        .map(|spaces| {
            spaces
                .iter()
                .filter_map(|space| space["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Waits for `path` to exist, as the worker writes it.
fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} was not written",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn remembered_sessions_become_spaces_of_one_repository_that_a_restart_keeps() {
    let flow = Flow::new("spaces-migrate", "");
    // A worktree of the repository, as oso makes them, and another repository.
    let side = flow.repo.join(".claude/worktrees/side");
    fs::create_dir_all(side.parent().unwrap()).unwrap();
    let side_text = side.to_str().unwrap();
    flow.git(
        &flow.repo,
        &["worktree", "add", "-q", "-b", "side", side_text],
    );
    let other = flow.scratch.path().join("other");
    fs::create_dir(&other).unwrap();
    flow.git(&other, &["init", "-q", "-b", "main"]);
    let canonical = |path: &Path| fs::canonicalize(path).unwrap();
    let (repo, side, other) = (canonical(&flow.repo), canonical(&side), canonical(&other));

    // What an earlier oso remembered: no `spaces` file yet.
    let state = state_dir(&flow.repo);
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let lines = [
        session_line(1, "repo", &repo),
        session_line(2, "side", &side),
        session_line(3, "other", &other),
    ];
    let record = state.join("sessions");
    fs::write(&record, lines.join("\n") + "\n").unwrap();
    fs::set_permissions(&record, fs::Permissions::from_mode(0o600)).unwrap();

    let (mut app, _) = flow.start();
    let found = app.wait_for("two spaces from the sessions", |state| {
        spaces(state).len() == 2
    });
    let text = |path: &Path| path.display().to_string();
    let mut listed = spaces(&found);
    listed.sort();
    assert_eq!(
        listed,
        [
            ("other".to_owned(), vec![text(&other)], vec![]),
            ("repo".to_owned(), vec![text(&repo)], vec![text(&side)]),
        ]
    );
    let sessions = found["sessions"]["sessions"].as_array().unwrap();
    assert!(
        sessions.iter().all(|s| s["state"] == "stopped"),
        "nothing started: {sessions:?}"
    );
    assert!(
        !flow.home.join("spaces").exists(),
        "no space directory made"
    );
    wait_for_file(&state.join("spaces"));
    let ids = {
        let mut ids = space_ids(&found);
        ids.sort();
        ids
    };

    // `space-switch` to the other repository's space, and back.
    for name in ["other", "repo"] {
        app.send_keys(&format!("C-c a j {} RET", keys(name)));
        app.wait_for("the space in front", |state| {
            state["sessions"]["space"] == name
        });
    }
    let _ = app.exchange("exit", None);
    app.wait_for_exit();

    // The next start reads the same spaces, and makes none again.
    let mut again = flow.start_again();
    let back = again.wait_for("the kept spaces", |state| spaces(state).len() == 2);
    let mut back_ids = space_ids(&back);
    back_ids.sort();
    assert_eq!(back_ids, ids);
    flow.exit_with(0);
}

/// The space of this name in the dev state, if there is one.
pub(super) fn space<'a>(state: &'a Value, name: &str) -> Option<&'a Value> {
    state["sessions"]["spaces"]
        .as_array()?
        .iter()
        .find(|space| space["name"] == name)
}

/// What the echo area says.
fn echo(state: &Value) -> String {
    state["echo"].as_str().unwrap_or_default().to_owned()
}

/// Answers the prompt now open with `text`, once it says `label`.
/// Trusts the project of the buffer just opened, `name`, once the projects thread found it
/// restricted: before that, `trust-project` would answer "Not in a project" and the `y` would
/// go in as text.
pub(super) fn trust_restricted(app: &mut crate::harness::App, name: &str) {
    app.wait_until_ready();
    let shown = format!("  {name}  restricted  ");
    app.wait_for("the buffer's project", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains(&shown))
    });
    app.send_keys(&format!("M-x {} RET y", keys("trust-project")));
}

/// Opens the space menu, waits until it is open, and runs its head `key`: a key sent before the
/// menu opens would reach the buffer instead.
pub(super) fn space_menu(app: &mut crate::harness::App, key: &str) {
    app.send_keys("C-c a S");
    app.wait_for("the space menu", |state| state["menu"] == "space");
    app.send_keys(key);
}

/// Answers the prompt labelled `label` with `text` once its list offers `text`: the host gives
/// a listed prompt its list as the spaces change, which may come after the prompt opens.
fn answer_listed(app: &mut crate::harness::App, label: &str, text: &str) {
    app.wait_for(&format!("{label} offering {text}"), |state| {
        super::worktrees::asks(state).is_some_and(|prompt| prompt.starts_with(label))
            && state["prompts"][0]["completion"]["rows"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row.as_str() == Some(text)))
    });
    app.send_keys(&format!("{} RET", keys(text)));
}

/// Waits until the spaces record on disk holds what `holds` looks for, in whichever state
/// directory the instance keeps it: the flow's own (`OSO_STATE_DIR`, ADR 0146), else its
/// `HOME`'s. The sessions worker writes it after each change, and an exit does not wait for
/// that write, so a flow that starts again checks the record first.
fn wait_for_record(flow: &Flow, what: &str, holds: impl Fn(&[Value]) -> bool) {
    let places = [
        flow.repo.join("state").join("spaces"),
        flow.home.join("Library/Application Support/oso/spaces"),
        flow.home.join(".local/state/oso/spaces"),
    ];
    let started = Instant::now();
    loop {
        for place in &places {
            let Ok(text) = fs::read_to_string(place) else {
                continue;
            };
            let spaces: Vec<Value> = text
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            if holds(&spaces) {
                return;
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no {what} in the spaces record after 30s"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The space named `name` in a record's spaces.
fn stored<'a>(spaces: &'a [Value], name: &str) -> Option<&'a Value> {
    spaces.iter().find(|space| space["name"] == name)
}

pub(super) fn answer(app: &mut crate::harness::App, label: &str, text: &str) {
    app.wait_for(label, |state| {
        super::worktrees::asks(state).is_some_and(|prompt| prompt.starts_with(label))
    });
    app.send_keys(&format!("{} RET", keys(text)));
}

/// Enters the first space, starts a session there, and closes the space once the session is
/// idle: the session is stopped and the space archived.
fn close_with_its_idle_session(app: &mut crate::harness::App) {
    // A session of the first space, idle, is stopped when the space closes.
    app.send_keys(&format!("C-c a j {} RET", keys("fix-login-error")));
    app.wait_for("the first space", |state| {
        state["sessions"]["space"] == "fix-login-error"
    });
    app.send_keys("C-c a c");
    app.wait_for("its session", |state| {
        state["sessions"]["sessions"].as_array().is_some_and(|s| {
            s.iter()
                .any(|s| s["space"] == "fix-login-error" && s["state"] == "idle")
        })
    });
    space_menu(app, "c");
    app.wait_for("the session stopped and the space closed", |state| {
        space(state, "fix-login-error").is_some_and(|s| s["archived"] == true)
            && state["sessions"]["sessions"]
                .as_array()
                .is_some_and(|s| s.iter().all(|s| s["state"] == "stopped"))
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_space_commands_make_change_close_and_reopen_a_space() {
    let flow = Flow::new("spaces-commands", "");
    let other = flow.scratch.path().join("other");
    fs::create_dir(&other).unwrap();
    flow.git(&other, &["init", "-q", "-b", "main"]);
    let (mut app, _) = flow.start();
    let root = flow.home.join("spaces");

    // A space for an issue, of the selected buffer's repository, made with its directory.
    app.send_keys("C-c a O");
    answer(&mut app, "New space for issue:", "Fix login error");
    let state = app.wait_for("the new space and its directory", |state| {
        space(state, "fix-login-error").is_some_and(|s| s["directory"].is_string())
            && state["sessions"]["space"] == "fix-login-error"
    });
    let made = space(&state, "fix-login-error").unwrap();
    let repo = fs::canonicalize(&flow.repo).unwrap().display().to_string();
    assert_eq!(made["members"], serde_json::json!([repo]));
    assert_eq!(made["title"], "Fix login error");
    let directory = root.join("fix-login-error");
    assert_eq!(made["directory"], directory.display().to_string());
    let mode = fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);

    // Another repository, by its path.
    space_menu(&mut app, "a");
    answer(&mut app, "Add repository:", other.to_str().unwrap());
    app.wait_for("the second repository", |state| {
        space(state, "fix-login-error")
            .is_some_and(|s| s["members"].as_array().is_some_and(|m| m.len() == 2))
    });

    // A root others can write: the space is made, its directory refused and said why.
    fs::set_permissions(&root, fs::Permissions::from_mode(0o770)).unwrap();
    app.send_keys("C-c a O");
    answer(&mut app, "New space for issue:", "Second");
    let state = app.wait_for("the refusal", |state| echo(state).contains("[spaces] root"));
    assert!(space(&state, "second").is_some_and(|s| s["directory"].is_null()));
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();

    // Closing the space in front enters the one entered last; reopening brings it back.
    space_menu(&mut app, "c");
    app.wait_for("the space closed", |state| {
        space(state, "second").is_some_and(|s| s["archived"] == true)
            && state["sessions"]["space"] == "fix-login-error"
    });
    space_menu(&mut app, "R");
    answer_listed(&mut app, "Reopen space:", "second");
    app.wait_for("the space open again", |state| {
        space(state, "second").is_some_and(|s| s["archived"] == false)
            && state["sessions"]["space"] == "second"
    });

    // Renaming makes the name from the title.
    space_menu(&mut app, "n");
    answer(&mut app, "Space title:", "Third title");
    app.wait_for("the new name", |state| {
        space(state, "third-title").is_some()
    });

    close_with_its_idle_session(&mut app);
    wait_for_record(&flow, "the closed and renamed spaces", |spaces| {
        stored(spaces, "fix-login-error").is_some_and(|s| s["archived"] == true)
            && stored(spaces, "third-title").is_some_and(|s| s["archived"] == false)
    });
    let _ = app.exchange("exit", None);
    app.wait_for_exit();

    // All of it is kept.
    let mut again = flow.start_again();
    again.wait_for("the kept spaces", |state| {
        space(state, "fix-login-error").is_some_and(|s| s["archived"] == true)
            && space(state, "third-title").is_some_and(|s| s["archived"] == false)
    });
    flow.exit_with(0);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_space_closed_just_before_quitting_is_kept() {
    let flow = Flow::new("spaces-quit", "");
    let (mut app, _) = flow.start();
    app.send_keys("C-c a O");
    answer(&mut app, "New space for issue:", "Close then quit");
    app.wait_for("the space", |state| {
        space(state, "close-then-quit").is_some()
    });
    app.send_keys("C-c a S");
    app.wait_for("the space menu", |state| state["menu"] == "space");
    app.send_keys("c");
    app.wait_for("the space closed", |state| {
        space(state, "close-then-quit").is_some_and(|s| s["archived"] == true)
    });
    // At once: the exit waits for the record's write itself (ADR 0121).
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
    let mut again = flow.start_again();
    again.wait_for("the closed space kept", |state| {
        space(state, "close-then-quit").is_some_and(|s| s["archived"] == true)
    });
    flow.exit_with(0);
}

/// The list's text, as the dev state gives it.
pub(super) fn list(state: &Value) -> String {
    state["sessions"]["list"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The line of the list that starts with `start`, indent and all.
fn line_starting(state: &Value, start: &str) -> Option<usize> {
    list(state).lines().position(|row| row.starts_with(start))
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_pane_shows_spaces_their_repositories_worktrees_and_sessions() {
    let flow = Flow::new("spaces-pane", "");
    let other = flow.scratch.path().join("other");
    fs::create_dir(&other).unwrap();
    flow.git(&other, &["init", "-q", "-b", "main"]);
    let (mut app, _) = flow.start();

    // A space of two repositories, with a session in the first.
    app.send_keys("C-c a O");
    answer(&mut app, "New space for issue:", "Fix login");
    app.wait_for("the space", |state| space(state, "fix-login").is_some());
    space_menu(&mut app, "a");
    answer(&mut app, "Add repository:", other.to_str().unwrap());
    app.wait_for("its second repository", |state| {
        space(state, "fix-login")
            .is_some_and(|s| s["members"].as_array().is_some_and(|m| m.len() == 2))
    });
    app.send_keys("C-c a c");
    let state = app.wait_for("space, repositories, worktree and session", |state| {
        line_starting(state, " ▾ fix-login ").is_some()
            && line_starting(state, "   ▾ repo/").is_some()
            && line_starting(state, "   ▾ other/").is_some()
            && list(state)
                .lines()
                .any(|row| row.starts_with("    ") && row.contains("main  primary"))
            && list(state)
                .lines()
                .any(|row| row.starts_with("      ") && row.contains("repo"))
    });
    assert!(
        line_starting(&state, "   ▾ repo/") < line_starting(&state, "   ▾ other/"),
        "{}",
        list(&state)
    );

    // Another space in front: the first, quiet, is folded, and RET on its row enters it.
    app.send_keys("C-c a O");
    answer(&mut app, "New space for issue:", "Second");
    let state = app.wait_for("the first folded", |state| {
        state["sessions"]["space"] == "second" && line_starting(state, " ▸ fix-login ").is_some()
    });
    app.send_keys("C-c a s");
    app.wait_for("the list selected", |state| {
        state["buffer"]["name"] == "*Spaces*"
    });
    let row = line_starting(&state, " ▸ fix-login ").unwrap();
    super::sessions_tree::point_at(&mut app, row);
    app.send_keys("RET");
    app.wait_for("the first space entered", |state| {
        state["sessions"]["space"] == "fix-login"
    });

    // `a` in the list reads a repository for the space in front.
    app.send_keys("C-c a s");
    app.wait_for("the list selected", |state| {
        state["buffer"]["name"] == "*Spaces*"
    });
    app.send_keys("a");
    app.wait_for("the repository prompt", |state| {
        super::worktrees::asks(state).is_some_and(|prompt| prompt.starts_with("Add repository:"))
    });
    app.send_keys("C-g");

    // A closed space goes in `Archived spaces`.
    app.send_keys(&format!("C-c a j {} RET", keys("second")));
    app.wait_for("the second space", |state| {
        state["sessions"]["space"] == "second"
    });
    space_menu(&mut app, "c");
    app.wait_for("the closed space listed", |state| {
        line_starting(state, " ▸ Archived spaces ─").is_some()
    });
    flow.exit_with(0);
}

/// The fake's run whose arguments hold `name`'s `--name`.
fn run_named<'a>(
    runs: &'a [(String, Vec<String>)],
    name: &str,
) -> Option<&'a (String, Vec<String>)> {
    let flag = format!("--name={name}");
    runs.iter().rev().find(|(_, args)| args.contains(&flag))
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_orchestrator_starts_in_the_spaces_directory_with_its_trusted_repositories() {
    // The orchestrator's directory must have a private way, and the scratch is in the system's
    // temporary directory, shared on Linux: the spaces' root is in a private one instead.
    let spaces = PrivateRoot::new("spaces-orchestrator");
    let config = format!("\n[spaces]\nroot = \"{}\"\n", spaces.0.display());
    let flow = Flow::new("spaces-orchestrator", &config);
    let other = flow.scratch.path().join("other");
    fs::create_dir(&other).unwrap();
    flow.git(&other, &["init", "-q", "-b", "main"]);
    fs::write(other.join("notes.md"), "# other\n").unwrap();
    let (mut app, _) = flow.start();
    app.send_keys("C-c a O");
    answer(&mut app, "New space for issue:", "Fix login");
    app.wait_for("the space's directory", |state| {
        space(state, "fix-login").is_some_and(|s| s["directory"].is_string())
    });
    space_menu(&mut app, "a");
    answer(&mut app, "Add repository:", other.to_str().unwrap());
    app.wait_for("its second repository, not trusted", |state| {
        space(state, "fix-login")
            .is_some_and(|s| s["members"].as_array().is_some_and(|m| m.len() == 2))
    });

    // `o` starts it in the space's directory, given the trusted repository alone.
    space_menu(&mut app, "o");
    let runs = flow.wait_for_runs(1);
    let (cwd, args) = run_named(&runs, "fix-login").expect("the orchestrator's run");
    let directory = fs::canonicalize(spaces.0.join("fix-login")).unwrap();
    assert_eq!(PathBuf::from(cwd), directory);
    let repo = fs::canonicalize(&flow.repo).unwrap();
    let other_real = fs::canonicalize(&other).unwrap();
    let add = |path: &Path| format!("--add-dir={}", path.display());
    assert!(args.contains(&add(&repo)), "{args:?}");
    assert!(!args.contains(&add(&other_real)), "{args:?}");
    let prompt = args
        .iter()
        .find(|arg| arg.starts_with("--append-system-prompt="))
        .expect("the orchestrator's text");
    assert!(prompt.contains("You are the orchestrator of an issue space"));
    assert!(prompt.contains("# This space: fix-login"));
    assert!(prompt.contains("not trusted in oso yet"), "{prompt}");
    let state = app.wait_for("the orchestrator recorded and listed", |state| {
        space(state, "fix-login").is_some_and(|s| s["orchestrator"].is_string())
            && list(state)
                .lines()
                .any(|row| row.starts_with("  ") && row.contains("orchestrator"))
    });
    assert!(
        line_starting(&state, " ▾ fix-login ").is_some(),
        "{}",
        list(&state)
    );

    // The other repository trusted now: its `/add-dir` is offered, not pasted unasked.
    app.open_file(&other.join("notes.md"));
    trust_restricted(&mut app, "other");
    app.wait_for("the offer", |state| {
        super::worktrees::asks(state).is_some_and(|prompt| prompt.starts_with("Paste /add-dir"))
    });
    app.send_keys("n");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();

    // Started again, it is back stopped; resumed, it is given both repositories now.
    let mut again = flow.start_again();
    again.wait_for("the orchestrator listed", |state| {
        list(state).lines().any(|row| row.contains("orchestrator"))
    });
    again.send_keys(&format!("C-c a b {} RET", keys("*claude: fix-login*")));
    again.wait_for("its buffer", |state| {
        state["buffer"]["name"] == "*claude: fix-login*"
    });
    again.send_keys("C-c a r");
    let runs = flow.wait_for_runs(2);
    let (_, args) = runs
        .iter()
        .rev()
        .find(|(_, args)| args.iter().any(|arg| arg.starts_with("--resume=")))
        .expect("the resume");
    assert!(
        args.contains(&add(&repo)) && args.contains(&add(&other_real)),
        "{args:?}"
    );
    flow.exit_with(0);
}

/// A directory for a space's root whose whole way is private and inside no repository: the
/// first of the temporary, runtime and home directories that is; removed when dropped.
pub(super) struct PrivateRoot(pub(super) PathBuf);

impl PrivateRoot {
    pub(super) fn new(test: &str) -> Self {
        let candidates = [
            Some(std::env::temp_dir()),
            std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        ];
        let base = (candidates.into_iter().flatten())
            .filter_map(|dir| fs::canonicalize(dir).ok())
            .find(|dir| {
                dir.ancestors().all(|above| {
                    let private = fs::metadata(above)
                        .is_ok_and(|meta| meta.permissions().mode() & 0o022 == 0);
                    private && !above.join(".git").exists()
                })
            })
            .expect("no private directory outside a repository");
        let path = base.join(format!("oso-e2e-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}

impl Drop for PrivateRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
