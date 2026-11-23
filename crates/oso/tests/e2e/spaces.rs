//! Issue spaces (ADR 0136). Step 1a: the sessions an earlier oso remembered, in two
//! repositories and a worktree of one, become two spaces of one repository each, the worktree
//! held by its repository's, with nothing started and no space directory made; the `spaces`
//! file keeps them, so that the next start reads the same spaces and makes none again; and
//! `space-switch` goes from one to the other. Step 1b and ADR 0159: `space-new`, from a
//! session's buffer, makes a space and its private directory and starts its orchestrator, the
//! session staying in its own space and in no window of the new one; the space menu adds a
//! repository by its path, renames the display name alone, closes (stopping an idle session)
//! and reopens; a shared root is refused, the space stays, and `o` starts its orchestrator
//! once the root is fixed; the modeline names the space in front; and a restart keeps it all.
//! Step 2:
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

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_start_archives_the_sessions_a_gone_worktree_left_under_its_main_checkout() {
    let flow = Flow::new("spaces-stranded", "");
    let repo = fs::canonicalize(&flow.repo).unwrap();
    // What an earlier oso remembered: a session at the main checkout, one in a worktree that
    // was removed, and one in a directory gone from under no repository.
    let gone = repo.join(".claude/worktrees/gone");
    let elsewhere = fs::canonicalize(flow.scratch.path()).unwrap().join("moved");
    let state = state_dir(&flow.repo);
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let in_repo = |n: u8, name: &str, place: &Path| {
        let line = session_line(n, name, place);
        line.replace(
            &format!(r#""root":"{}""#, place.display()),
            &format!(r#""root":"{}""#, repo.display()),
        )
    };
    let lines = [
        in_repo(1, "primary", &repo),
        in_repo(2, "stranded", &gone),
        session_line(3, "elsewhere", &elsewhere),
    ];
    let record = state.join("sessions");
    fs::write(&record, lines.join("\n") + "\n").unwrap();
    fs::set_permissions(&record, fs::Permissions::from_mode(0o600)).unwrap();

    let mut app = flow.start_again();
    let found = app.wait_for("the stranded session archived", |state| {
        archived(state, "stranded") == Some(true)
    });
    assert_eq!(archived(&found, "primary"), Some(false), "{found}");
    assert_eq!(archived(&found, "elsewhere"), Some(false), "{found}");
    app.send_keys(&format!("C-x b {} RET", keys("*Messages*")));
    app.wait_for("*Messages*", |state| {
        state["buffer"]["name"] == "*Messages*"
    });
    let line = "Archived 1 session: its directory is gone from under a repository";
    super::worktrees::text_until(&mut app, "the archive's line", |lines| {
        lines.iter().any(|text| text == line)
    });
    // The record keeps it archived, so that a restart neither brings it back nor counts it.
    wait_for_sessions(&flow, "the archived session", |sessions| {
        sessions
            .iter()
            .any(|s| s["name"] == "stranded" && s["archived"] == true)
    });
    let _ = app.exchange("exit", None);
    app.wait_for_exit();

    let mut again = flow.start_again();
    let back = again.wait_for("the recalled sessions", |state| {
        archived(state, "primary").is_some()
    });
    assert_eq!(archived(&back, "stranded"), Some(true), "{back}");
    again.send_keys(&format!("C-x b {} RET", keys("*Messages*")));
    again.wait_for("*Messages*", |state| {
        state["buffer"]["name"] == "*Messages*"
    });
    let lines = super::worktrees::text_until(&mut again, "*Messages*", |lines| !lines.is_empty());
    assert!(
        !lines.iter().any(|text| text.starts_with("Archived ")),
        "{lines:?}"
    );
}

/// Whether the session named `name` is archived, in the dev state.
fn archived(state: &Value, name: &str) -> Option<bool> {
    super::worktrees::session_archived(state, name)
}

/// Waits until the sessions record in the flow's state directory holds what `holds` looks for.
fn wait_for_sessions(flow: &Flow, what: &str, holds: impl Fn(&[Value]) -> bool) {
    let place = state_dir(&flow.repo).join("sessions");
    let started = Instant::now();
    loop {
        let sessions: Vec<Value> = fs::read_to_string(&place)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        if holds(&sessions) {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no {what} in the sessions record after 30s"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
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

/// `space-switch` to the space `name`: a prompt takes the list as it was when it opened, which
/// after a start may be before the host listed the spaces, and then keeps it, so one that does
/// not offer `name` is closed and opened again.
pub(super) fn switch_space(app: &mut crate::harness::App, name: &str) {
    let started = Instant::now();
    loop {
        app.send_keys("C-c a j");
        let state = app.wait_for("the space prompt", |state| {
            super::worktrees::asks(state).is_some_and(|prompt| prompt.starts_with("Space:"))
        });
        let offered = state["prompts"][0]["completion"]["rows"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row.as_str() == Some(name)));
        if offered {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "no space {name} is offered: {state}"
        );
        app.send_keys("C-g");
        std::thread::sleep(Duration::from_millis(100));
    }
    app.send_keys(&format!("{} RET", keys(name)));
    app.wait_for("the space in front", |state| {
        state["sessions"]["space"] == name
    });
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

/// `space-rename` from the space menu: its prompt holds `from`, the space's name now, which
/// the person replaces with `to` (ADR 0159).
pub(super) fn rename_space(app: &mut crate::harness::App, from: &str, to: &str) {
    space_menu(app, "n");
    let state = app.wait_for("the rename's prompt", |state| {
        super::worktrees::asks(state).is_some_and(|prompt| prompt.starts_with("Rename space to:"))
    });
    assert_eq!(state["prompts"][0]["text"], from, "{}", state["prompts"]);
    app.send_keys(&format!("C-a C-k {} RET", keys(to)));
}

pub(super) fn answer(app: &mut crate::harness::App, label: &str, text: &str) {
    app.wait_for(label, |state| {
        super::worktrees::asks(state).is_some_and(|prompt| prompt.starts_with(label))
    });
    app.send_keys(&format!("{} RET", keys(text)));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn claude_names_a_new_space_and_a_name_that_does_not_hold_gives_way() {
    let root = PrivateRoot::new("spaces-naming-root");
    let config = format!("\n[spaces]\nroot = \"{}\"\n", root.0.display());
    let flow = Flow::new("spaces-naming", &config);
    let answer_with = |display: &str, slug: &str| {
        let names = serde_json::json!({ "display_name": display, "slug": slug });
        let text = serde_json::json!({ "is_error": false, "structured_output": names });
        flow.scratch
            .write("naming-answer", text.to_string().as_bytes());
    };
    let (mut app, _) = flow.start();

    // Named by `claude` from the description, given on its input, never as an argument, run
    // with no tool in an empty directory of oso's state; then the directory and the
    // orchestrator take the slug.
    answer_with("ログイン 🔧", "login-fix");
    app.send_keys("C-c a O");
    answer(
        &mut app,
        "Describe the space:",
        "Fix the login when the SSO token expires",
    );
    app.wait_for(
        "the space named, its directory made and its orchestrator started",
        |state| {
            space(state, "login-fix").is_some_and(|s| {
                s["display"] == "ログイン 🔧"
                    && s["directory"].is_string()
                    && s["orchestrator"].is_string()
            })
        },
    );
    let runs = flow.wait_for_runs(1);
    let (cwd, _) = run_named(&runs, "login-fix").expect("the orchestrator's run");
    assert_eq!(
        PathBuf::from(cwd),
        fs::canonicalize(root.0.join("login-fix")).unwrap()
    );
    let read = |name: &str| fs::read_to_string(flow.scratch.path().join(name)).unwrap();
    let args = read("naming-args");
    for wanted in [
        "--print",
        "--safe-mode",
        "--tools=",
        "--no-session-persistence",
    ] {
        assert!(args.lines().any(|arg| arg == wanted), "{wanted}: {args}");
    }
    assert!(!args.contains("SSO token"), "{args}");
    assert!(read("naming-in").contains("Fix the login when the SSO token expires"));
    // A directory of its own, named after the space, under oso's state directory's own.
    let cwd = PathBuf::from(read("naming-cwd").trim_end());
    let parent = cwd.parent().and_then(Path::file_name);
    assert_eq!(
        parent.and_then(|name| name.to_str()),
        Some("space-naming"),
        "{cwd:?}"
    );

    // A slug that is none and a display name with a bidi control: oso's own slug, and the
    // name cleaned.
    show_readme(&mut app);
    answer_with("a\u{202e}b", "../../x");
    app.send_keys("C-c a O");
    answer(&mut app, "Describe the space:", "Second try");
    app.wait_for("the fallback slug and the cleaned name", |state| {
        space(state, "second-try")
            .is_some_and(|s| s["display"] == "ab" && s["directory"].is_string())
    });
    flow.exit_with(0);
}

/// Enters the first space and closes it once its sessions, its orchestrator here, are idle:
/// they are stopped and the space archived.
fn close_with_its_idle_session(app: &mut crate::harness::App) {
    let theirs = |state: &Value| -> Vec<Value> {
        let all = state["sessions"]["sessions"].as_array().cloned();
        let all = all.unwrap_or_default().into_iter();
        all.filter(|s| s["space"] == "Fix login error").collect()
    };
    app.send_keys(&format!("C-c a j {} RET", keys("Fix login error")));
    app.wait_for("the first space, its sessions idle", |state| {
        let sessions = theirs(state);
        state["sessions"]["space"] == "fix-login-error"
            && !sessions.is_empty()
            && sessions.iter().all(|s| s["state"] == "idle")
    });
    space_menu(app, "c");
    app.wait_for("the sessions stopped and the space closed", |state| {
        space(state, "fix-login-error").is_some_and(|s| s["archived"] == true)
            && theirs(state).iter().all(|s| s["state"] == "stopped")
    });
}

/// From a session's buffer, `space-new` makes a space for "Fix login error" of the flow's
/// repository, with its directory under `root`, and starts its orchestrator there: the session
/// stays in its own space and in no window of the new one, and the modeline names the new one.
fn new_space_from_a_session(flow: &Flow, app: &mut crate::harness::App, root: &Path) {
    // A session of the repository, shown when the space is made.
    app.send_keys("C-c a c");
    app.wait_for("the session shown, in its repository's space", |state| {
        state["buffer"]["name"] == "*claude: repo*" && state["sessions"]["space"] == "repo"
    });

    // A space for what the person describes, of the selected buffer's repository, made with
    // its directory and its orchestrator started there: the session shown before stays in its
    // own space, and in no window of the new one.
    app.send_keys("C-c a O");
    answer(app, "Describe the space:", "Fix login error");
    let state = app.wait_for(
        "the new space, its directory and its orchestrator",
        |state| {
            space(state, "fix-login-error")
                .is_some_and(|s| s["directory"].is_string() && s["orchestrator"].is_string())
                && state["sessions"]["space"] == "fix-login-error"
                && state["buffer"]["name"] == "*claude: fix-login-error*"
        },
    );
    let made = space(&state, "fix-login-error").unwrap();
    let repo = fs::canonicalize(&flow.repo).unwrap().display().to_string();
    assert_eq!(made["members"], serde_json::json!([repo]));
    assert_eq!(made["description"], "Fix login error");
    assert_eq!(made["display"], "Fix login error");
    let directory = root.join("fix-login-error");
    assert_eq!(made["directory"], directory.display().to_string());
    let mode = fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    assert_eq!(
        session(&state, "repo").map(|s| s["space"].clone()),
        Some(Value::from("repo")),
        "the session stays in its space: {}",
        list(&state)
    );
    let runs = flow.wait_for_runs(2);
    let (cwd, _) = run_named(&runs, "fix-login-error").expect("the orchestrator's run");
    assert_eq!(PathBuf::from(cwd), fs::canonicalize(&directory).unwrap());
    assert!(
        state["modeline"]
            .as_str()
            .is_some_and(|m| m.contains("Fix login error  ")),
        "{}",
        state["modeline"]
    );
}

/// The session of the list named `name`, as the dev state gives it.
fn session(state: &Value, name: &str) -> Option<Value> {
    let sessions = state["sessions"]["sessions"].as_array()?;
    sessions.iter().find(|s| s["name"] == name).cloned()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_space_commands_make_change_close_and_reopen_a_space() {
    // The orchestrator's directory must have a private way: see the orchestrator's flow.
    let root = PrivateRoot::new("spaces-commands-root");
    let config = format!("\n[spaces]\nroot = \"{}\"\n", root.0.display());
    let flow = Flow::new("spaces-commands", &config);
    let other = flow.scratch.path().join("other");
    fs::create_dir(&other).unwrap();
    flow.git(&other, &["init", "-q", "-b", "main"]);
    let (mut app, _) = flow.start();

    new_space_from_a_session(&flow, &mut app, &root.0);

    // Another repository, by its path.
    space_menu(&mut app, "a");
    answer(&mut app, "Add repository:", other.to_str().unwrap());
    app.wait_for("the second repository", |state| {
        space(state, "fix-login-error")
            .is_some_and(|s| s["members"].as_array().is_some_and(|m| m.len() == 2))
    });

    // A root others can write: the space is made, its directory refused and said why, and no
    // orchestrator starts; `o` starts it once the root is fixed.
    fs::set_permissions(&root.0, fs::Permissions::from_mode(0o770)).unwrap();
    // The new space's window shows its orchestrator, in no repository: a file of one is
    // selected, as `space-new` takes the selected buffer's repository.
    show_readme(&mut app);
    app.send_keys("C-c a O");
    answer(&mut app, "Describe the space:", "Second");
    let state = app.wait_for("the refusal", |state| echo(state).contains("[spaces] root"));
    let second = space(&state, "second").unwrap();
    assert!(second["directory"].is_null() && second["orchestrator"].is_null());
    fs::set_permissions(&root.0, fs::Permissions::from_mode(0o700)).unwrap();
    space_menu(&mut app, "o");
    app.wait_for("the orchestrator started", |state| {
        space(state, "second").is_some_and(|s| s["orchestrator"].is_string())
            && session(state, "second").is_some_and(|s| s["state"] == "idle")
    });

    // Renamed, the space keeps its slug, its directory and its orchestrator's name.
    rename_space(&mut app, "Second", "Third 名前 🔧");
    // The pane and the modeline show it once the views are made again.
    let state = app.wait_for("the new name", |state| {
        space(state, "second").is_some_and(|s| s["display"] == "Third 名前 🔧")
            && list(state).contains("Third 名前 🔧")
            && (state["modeline"].as_str()).is_some_and(|m| m.contains("Third 名前 🔧"))
    });
    assert_eq!(state["sessions"]["space"], "second");

    // Closing the space in front enters the one entered last; reopening brings it back.
    space_menu(&mut app, "c");
    app.wait_for("the space closed", |state| {
        space(state, "second").is_some_and(|s| s["archived"] == true)
            && state["sessions"]["space"] == "fix-login-error"
    });
    space_menu(&mut app, "R");
    answer_listed(&mut app, "Reopen space:", "Third 名前 🔧");
    app.wait_for("the space open again", |state| {
        space(state, "second").is_some_and(|s| s["archived"] == false)
            && state["sessions"]["space"] == "second"
    });

    close_with_its_idle_session(&mut app);
    wait_for_record(&flow, "the closed and renamed spaces", |spaces| {
        stored(spaces, "fix-login-error").is_some_and(|s| s["archived"] == true)
            && stored(spaces, "second")
                .is_some_and(|s| s["archived"] == false && s["display"] == "Third 名前 🔧")
    });
    let _ = app.exchange("exit", None);
    app.wait_for_exit();

    // All of it is kept.
    let mut again = flow.start_again();
    again.wait_for("the kept spaces", |state| {
        space(state, "fix-login-error").is_some_and(|s| s["archived"] == true)
            && space(state, "second")
                .is_some_and(|s| s["archived"] == false && s["display"] == "Third 名前 🔧")
    });
    flow.exit_with(0);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_space_closed_just_before_quitting_is_kept() {
    let flow = Flow::new("spaces-quit", "");
    let (mut app, _) = flow.start();
    app.send_keys("C-c a O");
    answer(&mut app, "Describe the space:", "Close then quit");
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

/// Shows the buffer of the flow's `README.md` again in the selected window, by its id: a space
/// `space-new` made shows its orchestrator's terminal, which takes keys such as `C-x`, and is in
/// no repository, while `space-new` and `claude` take the selected buffer's.
pub(super) fn show_readme(app: &mut crate::harness::App) {
    let state = app.state();
    let id = (state["buffers"].as_array().into_iter().flatten())
        .find(|buffer| {
            buffer["name"]
                .as_str()
                .is_some_and(|n| n.starts_with("README.md"))
        })
        .map(|buffer| buffer["id"].clone())
        .expect("the README's buffer");
    app.show_buffer(&id);
    app.wait_for("the README shown", |state| {
        state["buffer"]["name"]
            .as_str()
            .is_some_and(|name| name.starts_with("README.md"))
    });
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

    // A space of two repositories, with a session in the first. The new space shows its
    // orchestrator, which is in no repository: the session starts from a file of the first.
    app.send_keys("C-c a O");
    answer(&mut app, "Describe the space:", "Fix login");
    app.wait_for("the space", |state| space(state, "fix-login").is_some());
    space_menu(&mut app, "a");
    answer(&mut app, "Add repository:", other.to_str().unwrap());
    app.wait_for("its second repository", |state| {
        space(state, "fix-login")
            .is_some_and(|s| s["members"].as_array().is_some_and(|m| m.len() == 2))
    });
    show_readme(&mut app);
    app.send_keys("C-c a c");
    let state = app.wait_for("space, repositories, worktree and session", |state| {
        line_starting(state, " ▾ Fix login ").is_some()
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
    show_readme(&mut app);
    app.send_keys("C-c a O");
    answer(&mut app, "Describe the space:", "Second");
    let state = app.wait_for("the first folded", |state| {
        state["sessions"]["space"] == "second" && line_starting(state, " ▸ Fix login ").is_some()
    });
    app.send_keys("C-c a s");
    app.wait_for("the list selected", |state| {
        state["buffer"]["name"] == "*Spaces*"
    });
    let row = line_starting(&state, " ▸ Fix login ").unwrap();
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
    app.send_keys(&format!("C-c a j {} RET", keys("Second")));
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
    answer(&mut app, "Describe the space:", "Fix login");

    // `space-new` starts it in the space's directory, given the trusted repository, and told
    // what the space is for.
    let runs = flow.wait_for_runs(1);
    let (cwd, args) = run_named(&runs, "fix-login").expect("the orchestrator's run");
    let directory = fs::canonicalize(spaces.0.join("fix-login")).unwrap();
    assert_eq!(PathBuf::from(cwd), directory);
    let repo = fs::canonicalize(&flow.repo).unwrap();
    let other_real = fs::canonicalize(&other).unwrap();
    let add = |path: &Path| format!("--add-dir={}", path.display());
    assert!(args.contains(&add(&repo)), "{args:?}");
    let prompt = args
        .iter()
        .find(|arg| arg.starts_with("--append-system-prompt="))
        .expect("the orchestrator's text");
    assert!(prompt.contains("You are the orchestrator of an issue space"));
    assert!(prompt.contains("Reviews of a worker's change:"), "{prompt}");
    assert!(prompt.contains("# This space: fix-login"));
    assert!(prompt.contains("can edit too): Fix login\n"), "{prompt}");

    // A repository oso does not trust yet, added while it runs, is not given.
    space_menu(&mut app, "a");
    answer(&mut app, "Add repository:", other.to_str().unwrap());
    app.wait_for("its second repository, not trusted", |state| {
        space(state, "fix-login")
            .is_some_and(|s| s["members"].as_array().is_some_and(|m| m.len() == 2))
    });
    // `o` shows the one that runs, and starts no other.
    space_menu(&mut app, "o");
    app.wait_for("the one that runs", |state| {
        echo(state).starts_with("The space's orchestrator runs")
    });
    assert_eq!(flow.wait_for_runs(1).len(), 1, "no second start");
    let state = app.wait_for("the orchestrator recorded and listed", |state| {
        space(state, "fix-login").is_some_and(|s| s["orchestrator"].is_string())
            && list(state)
                .lines()
                .any(|row| row.starts_with("  ") && row.contains("orchestrator"))
    });
    assert!(
        line_starting(&state, " ▾ Fix login ").is_some(),
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
