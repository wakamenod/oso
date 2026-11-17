//! Pull requests in a running instance (ADR 0127, step 5a), against a stand-in `gh` a dev build
//! takes from `OSO_DEV_GH`:
//! the status's `PR:` line, `N v` showing the branch's pull request with its checks and review
//! threads, `N l` the open ones, `RET` there one of them, and every run of `gh` asked about the
//! remote's repository, outside the project.
//!
//! Must not depend on: the app's internals, a real `gh` or the network. The readers, the texts
//! and the thread's runs have their own tests.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::config::set_mode;
use super::git_status::project;
use super::harness::{App, Scratch};
use super::review::{git, rows};
use super::typing;

const VIEW: &str = r#"{"number":12,"title":"Add x","state":"OPEN","isDraft":false,
"reviewDecision":"APPROVED","url":"https://github.com/o/r/pull/12","headRefOid":"0000000",
"statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"}],
"author":{"login":"ann"},"baseRefName":"main","headRefName":"main","body":"Why",
"comments":[{"author":{"login":"bob"},"authorAssociation":"MEMBER","body":"Thanks"}]}"#;
const CHECKS: &str =
    r#"[{"name":"build","bucket":"pending","workflow":"CI","link":"https://x/1"}]"#;
const THREADS: &str = r#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"totalCount":1,
"nodes":[{"isResolved":false,"path":"a.txt","line":3,"comments":{"totalCount":1,"nodes":[
{"author":{"login":"cid"},"authorAssociation":"OWNER","body":"Why three?","diffHunk":"@@ -1 +1 @@\n+3"}]}}]}}}}}"#;
const LIST: &str = r#"[{"number":12,"title":"Add x","headRefName":"main","author":{"login":"ann"},
"isDraft":false,"reviewDecision":"APPROVED","statusCheckRollup":[],"url":"https://github.com/o/r/pull/12"}]"#;

/// A `gh` in `bin` that logs where it ran and what it was asked to `asked`, and answers from
/// the files beside it.
fn stand_in_gh(scratch: &Scratch) -> std::path::PathBuf {
    let bin = scratch.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    for (name, text) in [
        ("view", VIEW),
        ("checks", CHECKS),
        ("threads", THREADS),
        ("list", LIST),
    ] {
        fs::write(bin.join(format!("{name}.json")), text).unwrap();
    }
    let b = bin.display();
    let script = format!(
        "#!/bin/sh\nprintf '%s | %s\\n' \"$(pwd)\" \"$*\" >> '{b}/asked'\n\
         case \"$1 $2\" in\n\
         'pr view') cat '{b}/view.json' ;;\n\
         'pr checks') cat '{b}/checks.json'; exit 8 ;;\n\
         'api graphql') cat '{b}/threads.json' ;;\n\
         'pr list') cat '{b}/list.json' ;;\n\
         *) exit 1 ;;\nesac\n"
    );
    let gh = bin.join("gh");
    fs::write(&gh, script).unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn text(app: &mut App) -> String {
    rows(app, 40).join("\n")
}

fn wait_text(app: &mut App, what: &str, part: &str) -> String {
    for _ in 0..200 {
        let shown = text(app);
        if shown.contains(part) {
            return shown;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("no {what}: {}", text(app));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_status_shows_the_branch_s_pull_request_and_its_buffer_and_list_read_through_gh() {
    let scratch = Scratch::new("git-pr");
    let file = project(&scratch);
    let root = scratch.path().join("proj-git");
    git(
        &root,
        &["remote", "add", "origin", "https://github.com/o/r.git"],
    );
    let bin = stand_in_gh(&scratch);
    set_mode(scratch.path(), 0o700);
    let config = scratch.write("conf/config.toml", b"[lsp]\nstart = \"manual\"\n");
    set_mode(&scratch.path().join("conf"), 0o700);
    set_mode(&config, 0o644);
    // `PATH` is the platform's own: a dev build takes its stand-in `gh` by name.
    let gh = bin.join("gh");
    let env: [(&str, &OsStr); 1] = [("OSO_DEV_GH", gh.as_os_str())];
    let mut app = App::start_with_file_config_and_env(&file, scratch.path(), &config, &env);
    app.wait_until_ready();
    // The project is known before it is trusted, as the status's flows wait for.
    app.wait_for("the project", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("  proj-git  restricted  "))
    });
    app.send_keys(&format!("M-x {} RET y", typing("trust-project")));
    app.wait_for("the trust", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with("Trusted "))
    });
    app.send_keys("C-x g");
    app.wait_for("the status", |state| {
        state["buffer"]["name"] == "*git: proj-git*"
    });
    // The head's `PR:` line, stale as the stand-in's head is not the commit checked out.
    let shown = wait_text(
        &mut app,
        "the PR line",
        "PR:       #12 Add x  open, approved",
    );
    assert!(shown.contains("✓ 1 ✗ 0 ● 0  stale"), "{shown}");

    // `N v`: the branch's pull request, its checks, threads and conversation.
    app.send_keys("N v");
    app.wait_for("the pull request", |state| {
        state["buffer"]["name"] == "*pr: proj-git*"
    });
    let shown = wait_text(&mut app, "its checks", "● build");
    assert!(shown.starts_with("#12 Add x"), "{shown}");
    assert!(
        shown.contains("a.txt:3  unresolved") && shown.contains("@cid (OWNER)"),
        "{shown}"
    );
    assert!(
        shown.contains("Conversation (1)") && shown.contains("@bob (MEMBER)"),
        "{shown}"
    );

    // `N l` from the status: the open ones; `RET` on one shows it.
    app.send_keys("q C-x g");
    app.wait_for("the status again", |state| {
        state["buffer"]["name"] == "*git: proj-git*"
    });
    app.send_keys("N l");
    app.wait_for("the list", |state| {
        state["buffer"]["name"] == "*pull requests: proj-git*"
    });
    wait_text(&mut app, "the listed one", "#12    open   Add x");
    app.send_keys("M-< C-n RET");
    app.wait_for("the listed one's buffer", |state| {
        state["buffer"]["name"] == "*pr: #12*"
    });
    wait_text(&mut app, "its text", "● build");

    // Every run asked about the remote's repository, outside the project: each run's line
    // starts with where it ran; the GraphQL query's own lines follow its run's.
    let asked = fs::read_to_string(bin.join("asked")).unwrap();
    let runs: Vec<(&str, &str)> = (asked.lines())
        .filter_map(|line| line.split_once(" | "))
        .filter(|(cwd, _)| cwd.starts_with('/'))
        .collect();
    assert!(runs.len() >= 6, "{asked}");
    for (cwd, args) in runs {
        assert!(!Path::new(cwd).starts_with(&root), "{cwd} | {args}");
        let about = args.contains("--repo github.com/o/r") || args.starts_with("api graphql");
        assert!(about, "{cwd} | {args}");
    }
}
