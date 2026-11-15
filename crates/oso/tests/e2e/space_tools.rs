//! The session tools from an issue space's orchestrator (ADR 0136, step 4): its fake `claude`
//! runs the `oso mcp` its `--mcp-config` names, as in `session_tools`. `space_status` gives the
//! space and its repositories with their trust; `session_list` holds the space's sessions
//! only; `start_worktree_session` needs `repo`, refuses a repository oso does not trust, and
//! with a trusted one asks the person and starts a worker in a worktree of it, told its space,
//! which then belongs to the space. `session_start` and `worktree_list` name the repository too.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: which repository
//! `repo` names and the scope have their own tests.

use std::fs;

use serde_json::{Value, json};

use super::session_tools::{Bridge, FAKE, answer, run_in};
use super::spaces::{PrivateRoot, answer as reply_with, space};
use super::worktrees::Flow;

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_orchestrator_works_through_the_repositories_of_its_space() {
    // The space's directory needs a private way; the scratch may be in a shared /tmp.
    let spaces = PrivateRoot::new("space-tools");
    let config = format!("\n[spaces]\nroot = \"{}\"\n", spaces.0.display());
    let flow = Flow::with_fake("space-tools", &config, FAKE);
    let other = flow.scratch.path().join("other");
    fs::create_dir(&other).unwrap();
    flow.git(&other, &["init", "-q", "-b", "main"]);
    let (mut app, _) = flow.start();
    app.send_keys("C-x a O");
    reply_with(&mut app, "New space for issue:", "Fix login");
    app.wait_for("the space's directory", |state| {
        space(state, "fix-login").is_some_and(|s| s["directory"].is_string())
    });
    app.send_keys("C-x a S");
    app.send_keys("a");
    reply_with(&mut app, "Add repository:", other.to_str().unwrap());
    app.wait_for("its second repository, not trusted", |state| {
        space(state, "fix-login")
            .is_some_and(|s| s["members"].as_array().is_some_and(|m| m.len() == 2))
    });
    app.send_keys("C-x a S");
    app.send_keys("o");
    let run = run_in(&flow, &spaces.0.join("fix-login"));
    let mut bridge = Bridge::start(&flow, &run);

    // Its space, its repositories with their trust, and the sessions of the space alone.
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
    let question = answer(&mut app, "n");
    assert!(question.contains("repo"), "{question}");
    let (said, error) = bridge.result(id);
    assert!(error, "{said}");
    let (listed, error) = bridge.tool("worktree_list", json!({ "repo": "repo" }));
    assert!(!error, "{listed}");

    // A trusted one: the person is asked, and a worker starts in a worktree of it.
    let id = bridge.call("start_worktree_session", start(Some("repo")));
    let question = answer(&mut app, "y");
    assert!(question.contains("fix/login"), "{question}");
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
    assert!(brief.contains("SendMessage (to: fix-login)"), "{brief}");
    let (status, _) = bridge.tool("space_status", json!({}));
    let sessions = status["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2, "the worker is the space's: {status}");
}
