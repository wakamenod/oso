//! The session tools from an issue space's orchestrator (ADR 0136, step 4): its fake `claude`
//! runs the `oso mcp` its `--mcp-config` names, as in `session_tools`. `space_status` gives the
//! space and its repositories with their trust; `session_list` holds the space's sessions
//! only; `start_worktree_session` needs `repo`, refuses a repository oso does not trust, and
//! with a trusted one asks the person and starts a worker in a worktree of it, told its space,
//! which then belongs to the space. `session_start` and `worktree_list` name the repository too.
//! `with_dirs` gives the worker another trusted repository of the space to read, and refuses a
//! directory in none (ADR 0136, step 4b). A renamed space's orchestrator keeps its role.
//! `session_resume` resumes a stopped worker of the space in its own buffer without asking, as
//! the configuration allows, the resumed one counted toward the limit; past it, it asks and `n`
//! refuses it for a minute; a conversation that runs in another process is refused, not forked,
//! and the caller, an unknown name and a running session are refused (ADR 0153).
//! `session_rename` renames a worker of the space in oso's record and its buffer, never the
//! orchestrator, and takes no name another session or the space has; its next resume passes
//! the new name; the person's `C-c a R` opens holding the name and renames it again, and is
//! refused on the orchestrator (ADR 0155). Reviewers the orchestrator starts with
//! `session_start` in the implementer's worktree get no shared-directory question: one starts
//! without asking, and past the limit the one question says another session runs there
//! (ADR 0162).
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: which repository
//! `repo` names and the scope have their own tests.

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::session_tools::{Bridge, FAKE, Run, answer, run_in, runs};
use super::sessions::live_elsewhere;
use super::spaces::{PrivateRoot, answer as reply_with, space, trust_restricted};
use super::worktrees::{DEADLINE, Flow, asks, keys, terminal};

/// A space of the flow's repository and an untrusted `other` one, made with `C-c a O`, which
/// starts its orchestrator: the instance, the orchestrator's bridge and `other`.
pub(super) fn orchestrated(
    flow: &Flow,
    spaces: &PrivateRoot,
) -> (crate::harness::App, Bridge, PathBuf) {
    let other = flow.scratch.path().join("other");
    fs::create_dir(&other).unwrap();
    flow.git(&other, &["init", "-q", "-b", "main"]);
    let (mut app, _) = flow.start();
    app.send_keys("C-c a O");
    reply_with(&mut app, "Describe the space:", "Fix login");
    // `space-new` starts its orchestrator (ADR 0159) once its directory is made.
    app.wait_for("the space's directory", |state| {
        space(state, "fix-login").is_some_and(|s| s["directory"].is_string())
    });
    let run = run_in(flow, &spaces.0.join("fix-login"));
    app.send_keys("C-c a S");
    app.send_keys("a");
    reply_with(&mut app, "Add repository:", other.to_str().unwrap());
    app.wait_for("its second repository, not trusted", |state| {
        space(state, "fix-login")
            .is_some_and(|s| s["members"].as_array().is_some_and(|m| m.len() == 2))
    });
    let bridge = Bridge::start(flow, &run);
    (app, bridge, other)
}

/// `space_status` names the space and its repositories with their trust, and `session_list`
/// holds the orchestrator alone.
fn assert_its_space(bridge: &mut Bridge) {
    let (status, error) = bridge.tool("space_status", json!({}));
    assert!(!error, "{status}");
    assert_eq!(status["space"], "fix-login");
    assert_eq!(status["you_orchestrate"], true);
    let repositories: Vec<(&str, bool)> = status["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["name"].as_str().unwrap(), r["trusted"] == true))
        .collect();
    assert_eq!(repositories, [("repo", true), ("other", false)]);
    let (listed, _) = bridge.tool("session_list", json!({}));
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "{listed}");
    assert_eq!(sessions[0]["you"], true);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_orchestrator_works_through_the_repositories_of_its_space() {
    // The space's directory needs a private way; the scratch may be in a shared /tmp.
    let spaces = PrivateRoot::new("space-tools");
    let config = format!("\n[spaces]\nroot = \"{}\"\n", spaces.0.display());
    let flow = Flow::with_fake("space-tools", &config, FAKE);
    let (mut app, mut bridge, other) = orchestrated(&flow, &spaces);
    assert_its_space(&mut bridge);

    // A start names a repository of the space, which oso must trust.
    let start = |repo: Option<&str>| {
        let mut call = json!({ "branch": "fix/login", "brief": "Fix the login error." });
        if let Some(repo) = repo {
            call["repo"] = Value::from(repo);
        }
        call
    };
    let (said, error) = bridge.tool("start_worktree_session", start(None));
    assert!(error && said.to_string().contains("\\\"repo\\\""), "{said}");
    let (said, error) = bridge.tool("start_worktree_session", start(Some("other")));
    assert!(error && said.to_string().contains("not trusted"), "{said}");
    // A session at a repository's main checkout is asked about after git looked at it; `n`.
    let id = bridge.call("session_start", json!({ "repo": "repo" }));
    let question = answer(&mut app, &bridge, "n");
    assert!(question.contains("repo"), "{question}");
    let (said, error) = bridge.result(id);
    assert!(error, "{said}");
    let (listed, error) = bridge.tool("worktree_list", json!({ "repo": "repo" }));
    assert!(!error, "{listed}");

    // A directory to read must be in a repository of the space.
    let mut outside = start(Some("repo"));
    outside["with_dirs"] = json!([flow.scratch.path().display().to_string()]);
    let (said, error) = bridge.tool("start_worktree_session", outside);
    assert!(
        error
            && said
                .to_string()
                .contains("neither a repository of your space"),
        "{said}"
    );

    // The other repository trusted now; its `/add-dir` offer is answered `n`.
    app.open_file(&other.join("notes.md"));
    trust_restricted(&mut app, "other");
    app.wait_for("the offer", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Paste /add-dir"))
    });
    app.send_keys("n");

    // A trusted one: the person is asked, and a worker starts in a worktree of it, also
    // reading the other repository.
    let other_real = fs::canonicalize(&other).unwrap();
    let mut call = start(Some("repo"));
    call["with_dirs"] = json!([other_real.display().to_string()]);
    let id = bridge.call("start_worktree_session", call);
    let question = answer(&mut app, &bridge, "y");
    assert!(question.contains("fix/login"), "{question}");
    assert!(question.contains("It also reads"), "{question}");
    let (started, error) = bridge.result(id);
    assert!(!error, "{started}");
    let worktrees = flow.repo.join(".claude/worktrees");
    let worktree = fs::read_dir(&worktrees)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .next()
        .expect("the worker's worktree");
    let worker = run_in(&flow, &worktree);
    let brief = worker.args.last().expect("the worker's brief");
    assert!(brief.contains(", in space fix-login."), "{brief}");
    assert!(brief.contains("You can also read: "), "{brief}");
    let add = format!("--add-dir={}", other_real.display());
    assert!(worker.args.contains(&add), "{:?}", worker.args);
    assert!(brief.contains("SendMessage (to: fix-login)"), "{brief}");
    let (status, _) = bridge.tool("space_status", json!({}));
    let sessions = status["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2, "the worker is the space's: {status}");

    // Renamed, the space keeps its slug and its directory, and the orchestrator there its
    // role; agents see the new display name (ADR 0159).
    super::spaces::rename_space(&mut app, "Fix login", "Login fix");
    app.wait_for("the new name", |state| {
        space(state, "fix-login").is_some_and(|s| s["display"] == "Login fix")
    });
    let (status, error) = bridge.tool("space_status", json!({}));
    assert!(!error, "{status}");
    assert_eq!(status["space"], "fix-login");
    assert_eq!(status["display_name"], "Login fix");
    assert_eq!(status["description"], "Fix login");
    assert_eq!(status["orchestrator"], "fix-login");
    assert_eq!(status["you_orchestrate"], true, "{status}");
}

/// The state `session_list` gives session `name`, as `bridge` sees it.
fn state_of(bridge: &mut Bridge, name: &str) -> String {
    let (state, error) = bridge.tool("session_state", json!({ "session": name }));
    assert!(!error, "{state}");
    state["state"].as_str().unwrap_or_default().to_owned()
}

/// The fake's run `run` exits with status 0, and session `name` is stopped then.
fn stop(flow: &Flow, bridge: &mut Bridge, run: &Run, name: &str) {
    flow.scratch.write(&format!("go.{}", run.pid), b"0");
    let deadline = Instant::now() + DEADLINE;
    while state_of(bridge, name) != "stopped" {
        assert!(Instant::now() < deadline, "{name} did not stop");
        thread::sleep(Duration::from_millis(100));
    }
}

/// The fake's run that resumed conversation `id`, once it is there.
fn resumed_run(flow: &Flow, id: &str) -> Run {
    let resume = format!("--resume={id}");
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(run) = runs(flow)
            .into_iter()
            .find(|run| run.args.contains(&resume))
        {
            return run;
        }
        assert!(Instant::now() < deadline, "nothing resumed {id}");
        thread::sleep(Duration::from_millis(100));
    }
}

/// How many runs of the fake resumed conversation `id`.
fn resumes_of(flow: &Flow, id: &str) -> usize {
    let resume = format!("--resume={id}");
    runs(flow)
        .iter()
        .filter(|run| run.args.contains(&resume))
        .count()
}

/// A worker named `name` in a new worktree on `branch` of the space's repository, started as
/// the call says; its id and its run.
fn worker(
    flow: &Flow,
    bridge: &mut Bridge,
    name: &str,
    asked: Option<&mut crate::harness::App>,
) -> (String, Run) {
    let call = json!({ "branch": name, "brief": "Work.", "name": name, "repo": "repo" });
    let started = match asked {
        None => {
            let (started, error) = bridge.tool("start_worktree_session", call);
            assert!(!error, "{started}");
            assert_eq!(started["asked"], false, "{started}");
            started
        }
        Some(app) => {
            let id = bridge.call("start_worktree_session", call);
            answer(app, bridge, "y");
            let (started, error) = bridge.result(id);
            assert!(!error, "{started}");
            assert_eq!(started["asked"], true, "{started}");
            started
        }
    };
    let id = started["session"]["id"].as_str().unwrap().to_owned();
    let run = run_in(flow, &flow.repo.join(".claude/worktrees").join(name));
    (id, run)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_orchestrator_resumes_the_stopped_sessions_of_its_space() {
    let spaces = PrivateRoot::new("space-resume");
    let config = format!(
        "agents_start_without_asking = true\nagents_start_limit = 1\n\n[spaces]\nroot = \"{}\"\n",
        spaces.0.display()
    );
    let flow = Flow::with_fake("space-resume", &config, FAKE);
    let (mut app, mut bridge, _) = orchestrated(&flow, &spaces);

    // One worker starts without asking, the limit's one; the next asks, and the person allows it.
    let (a, run_a) = worker(&flow, &mut bridge, "wa", None);
    let (b, run_b) = worker(&flow, &mut bridge, "wb", Some(&mut app));
    stop(&flow, &mut bridge, &run_a, "wa");
    stop(&flow, &mut bridge, &run_b, "wb");

    // What no resume takes: the caller, a name none has, a session that runs.
    let (said, error) = bridge.tool("session_resume", json!({ "session": "fix-login" }));
    assert!(
        error && said.to_string().contains("your own session"),
        "{said}"
    );
    let (said, error) = bridge.tool("session_resume", json!({ "session": "nobody" }));
    assert!(error && said.to_string().contains("no session"), "{said}");

    // Nothing runs that the caller started without asking: wb resumes without asking, in its
    // own buffer, in its worktree, and is no new session.
    let (resumed, error) = bridge.tool("session_resume", json!({ "session": "wb" }));
    assert!(!error, "{resumed}");
    assert_eq!(resumed["asked"], false, "{resumed}");
    assert_eq!(resumed["session"]["id"], b.as_str());
    let run = resumed_run(&flow, &b);
    assert_eq!(
        run.cwd,
        fs::canonicalize(flow.repo.join(".claude/worktrees/wb")).unwrap()
    );
    assert_eq!(app.state()["prompts"], json!([]), "nobody was asked");
    let state = app.wait_for("wb running in its buffer", |state| {
        terminal(state, "*claude: wb*").is_some()
    });
    assert!(
        terminal(&state, "*claude: wb<2>*").is_none(),
        "no second buffer"
    );
    let (listed, _) = bridge.tool("session_list", json!({}));
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 3, "{listed}");

    // wb counts toward the limit of 1 while it runs: wa's resume asks, and `n` refuses it,
    // then for a minute without asking.
    let id = bridge.call("session_resume", json!({ "session": a }));
    let question = answer(&mut app, &bridge, "n");
    assert!(question.contains("resume session \"wa\""), "{question}");
    let (said, error) = bridge.result(id);
    assert!(error && said.to_string().contains("refused"), "{said}");
    let (said, error) = bridge.tool("session_resume", json!({ "session": "wa" }));
    assert!(error && said.to_string().contains("a moment ago"), "{said}");
    assert_eq!(resumes_of(&flow, &a), 0);
    let (said, error) = bridge.tool("session_resume", json!({ "session": "wb" }));
    assert!(error && said.to_string().contains("runs already"), "{said}");

    // wb stopped, its conversation run by another process: refused, with no fork offered.
    stop(&flow, &mut bridge, &run, "wb");
    let mut other = live_elsewhere(&flow.home, &b, "wb");
    let (said, error) = bridge.tool("session_resume", json!({ "session": "wb" }));
    let pid = other.id().to_string();
    assert!(error && said.to_string().contains(&pid), "{said}");
    assert!(said.to_string().contains("stop it there first"), "{said}");
    assert_eq!(app.state()["prompts"], json!([]), "no fork is offered");
    assert_eq!(resumes_of(&flow, &b), 1, "nothing ran");
    assert_eq!(state_of(&mut bridge, "wb"), "stopped");
    let _ = other.kill();
    let _ = other.wait();
    flow.exit_with(0);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_orchestrator_starts_reviewers_where_its_implementer_runs_with_no_shared_question() {
    let spaces = PrivateRoot::new("space-review");
    let config = format!(
        "agents_start_without_asking = true\nagents_start_limit = 2\n\n[spaces]\nroot = \"{}\"\n",
        spaces.0.display()
    );
    let flow = Flow::with_fake("space-review", &config, FAKE);
    let (mut app, mut bridge, _) = orchestrated(&flow, &spaces);
    let (_, implementer) = worker(&flow, &mut bridge, "impl", None);
    let worktree = implementer.cwd.display().to_string();
    let review = |name: &str| json!({ "repo": "repo", "directory": worktree, "name": name, "brief": "Review." });

    // Below the limit, a reviewer starts in the implementer's worktree with no question at all:
    // an agent's start never asks "Start here anyway?" (ADR 0162).
    let (started, error) = bridge.tool("session_start", review("review-a"));
    assert!(!error, "{started}");
    assert_eq!(started["asked"], false, "{started}");
    assert_eq!(app.state()["prompts"], json!([]), "nobody was asked");

    // At the limit, the one question says another session runs there, and `y` starts it with no
    // second question.
    let id = bridge.call("session_start", review("review-b"));
    let question = answer(&mut app, &bridge, "y");
    assert!(
        question.ends_with("? Another session runs there. (y or n)"),
        "{question}"
    );
    let (started, error) = bridge.result(id);
    assert!(!error, "{started}");
    assert_eq!(started["asked"], true, "{started}");
    // The fake writes its run once it runs, which may be after the start's reply.
    let deadline = Instant::now() + DEADLINE;
    let in_worktree = || {
        runs(&flow)
            .iter()
            .filter(|run| run.cwd == implementer.cwd)
            .count()
    };
    while in_worktree() < 3 {
        assert!(
            Instant::now() < deadline,
            "the implementer and both reviewers"
        );
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(in_worktree(), 3, "the implementer and both reviewers");
    assert_eq!(app.state()["prompts"], json!([]), "no second question");
    let (status, _) = bridge.tool("space_status", json!({}));
    let sessions = status["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 4, "the reviewers are the space's: {status}");
    flow.exit_with(0);
}

/// The name of the session whose id is `id`, and the one Claude Code knows it by, as
/// `session_state` gives them.
fn names_of(bridge: &mut Bridge, id: &str) -> (String, String) {
    let (state, error) = bridge.tool("session_state", json!({ "session": id }));
    assert!(!error, "{state}");
    let text = |field: &str| state[field].as_str().unwrap_or_default().to_owned();
    (text("name"), text("messaging_name"))
}

fn name_of(bridge: &mut Bridge, id: &str) -> String {
    names_of(bridge, id).0
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_orchestrator_and_the_person_rename_the_sessions_of_a_space() {
    let spaces = PrivateRoot::new("space-rename");
    let config = format!(
        "agents_start_without_asking = true\nagents_start_limit = 2\n\n[spaces]\nroot = \"{}\"\n",
        spaces.0.display()
    );
    let flow = Flow::with_fake("space-rename", &config, FAKE);
    let (mut app, mut bridge, _) = orchestrated(&flow, &spaces);
    let (id, run) = worker(&flow, &mut bridge, "wa", None);
    // As an agent does, it tries again while the person has just typed (ADR 0085).
    let rename = |bridge: &mut Bridge, session: &str, name: &str| {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let call = json!({ "session": session, "name": name });
            let (said, error) = bridge.tool("session_rename", call);
            if !(error && said.to_string().contains("try again in a moment"))
                || Instant::now() > deadline
            {
                return (said, error);
            }
            thread::sleep(Duration::from_millis(200));
        }
    };

    // Never the orchestrator, whose name is the space's; no name the space or another session
    // has, case aside; only a name as it is kept.
    let (said, error) = rename(&mut bridge, "fix-login", "lead");
    assert!(error && said.to_string().contains("orchestrator"), "{said}");
    // The orchestrator holds the space's name too, so either refusal says it.
    let (said, error) = rename(&mut bridge, "wa", "Fix-Login");
    assert!(
        error && said.to_string().contains("has that name"),
        "{said}"
    );
    let (said, error) = rename(&mut bridge, "wa", "fix-login-was-1");
    assert!(error && said.to_string().contains("earlier"), "{said}");
    let (said, error) = rename(&mut bridge, "wa", "api v2");
    assert!(
        error && said.to_string().contains("\\\"api-v2\\\""),
        "{said}"
    );
    assert_eq!(name_of(&mut bridge, &id), "wa");

    // Renamed: the record and the buffer at once, nobody asked; its claude keeps its name until
    // it is resumed, which the reply and the echo area say.
    let (renamed, error) = rename(&mut bridge, "wa", "api-v2");
    assert!(!error, "{renamed}");
    assert_eq!(renamed["was"], "wa");
    assert_eq!(renamed["messaging_name"], "wa", "{renamed}");
    let note = renamed["note"].as_str().unwrap_or_default();
    assert!(note.contains("/rename api-v2"), "{renamed}");
    let state = app.wait_for("its buffer renamed", |state| {
        terminal(state, "*claude: api-v2*").is_some()
    });
    assert!(terminal(&state, "*claude: wa*").is_none());
    assert_eq!(state["prompts"], json!([]), "nobody was asked");
    assert!(
        (state["echo"].as_str().unwrap_or_default()).contains("renamed session \"wa\""),
        "{}",
        state["echo"]
    );
    // Its claude still answers to the name it was started with, which no other session takes.
    assert_eq!(names_of(&mut bridge, &id), ("api-v2".into(), "wa".into()));
    let _ = worker(&flow, &mut bridge, "wb", None);
    let (said, error) = rename(&mut bridge, "wb", "WA");
    assert!(
        error && said.to_string().contains("another session"),
        "{said}"
    );
    let (said, error) = rename(&mut bridge, &id, "api-v2");
    assert!(error && said.to_string().contains("already"), "{said}");

    // Its next resume passes the new name.
    stop(&flow, &mut bridge, &run, "api-v2");
    let (resumed, error) = bridge.tool("session_resume", json!({ "session": "api-v2" }));
    assert!(!error, "{resumed}");
    let run = resumed_run(&flow, &id);
    assert!(
        run.args.contains(&"--name=api-v2".to_owned()),
        "{:?}",
        run.args
    );
    assert_eq!(
        names_of(&mut bridge, &id),
        ("api-v2".into(), "api-v2".into())
    );

    the_person_renames(&mut app, &mut bridge, &id);
    flow.exit_with(0);
}

/// The person's `C-c a R` in session `id`'s buffer, `*claude: api-v2*`, opens holding its name
/// and renames it again; on the orchestrator it is refused, with no prompt.
fn the_person_renames(app: &mut crate::harness::App, bridge: &mut Bridge, id: &str) {
    // The person's `C-c a R` in its buffer opens holding its name, and renames it again.
    app.send_keys(&format!("C-x b {} RET", keys("*claude: api-v2*")));
    app.send_keys("C-c a R");
    let state = app.wait_for("the rename's prompt", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Rename session to:"))
    });
    assert_eq!(
        state["prompts"][0]["text"], "api-v2",
        "{}",
        state["prompts"]
    );
    app.send_keys(&format!("C-a C-k {} RET", keys("api v3")));
    app.wait_for("its buffer renamed by the person", |state| {
        terminal(state, "*claude: api-v3*").is_some()
    });
    assert_eq!(name_of(bridge, id), "api-v3");

    // On the orchestrator, the person's rename is refused, with no prompt.
    app.send_keys(&format!("C-x b {} RET", keys("*claude: fix-login*")));
    app.send_keys("C-c a R");
    let state = app.wait_for("the refusal", |state| {
        (state["echo"].as_str().unwrap_or_default()).contains("orchestrator")
    });
    assert_eq!(state["prompts"], json!([]));
}
