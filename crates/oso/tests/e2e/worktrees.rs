//! Worktree sessions (ADR 0121, step 4), with a fake `claude` as step 1's flows have: a shell
//! script that `[sessions] claude` names, which writes the directory it runs in and its
//! arguments, each ended by a NUL, to a file, then exits with the status the flow gives it.
//! Each flow has a git repository of its own, with a commit, a local bare `origin` and its own
//! `HOME`, trusted with `trust-project` as a person trusts one. `C-x a w` makes a worktree from
//! a brief, a second session in one directory is asked about, `*Worktrees*` lists a worktree
//! and removes it only once its work is merged, and `[sessions] steer_worktrees` gives a session
//! the hook that denies Claude's own worktree tools. One flow times a listing of 20 worktrees.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the slug, the
//! brief's header, the checks, the merge test and the hook's pattern have their own tests.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
use serde_json::{Value, json};

use super::config::set_mode;
use super::harness::{App, Scratch};

/// The fake: where it runs and what it was given, then the exit the flow writes to `go`. It
/// also ends once the flow's directory is gone. `DIR` is the flow's scratch directory.
const FAKE: &str = r#"#!/bin/sh
dir='DIR'
printf '%s\0' "$(pwd -P)" "$@" > "$dir/tmp.$$"
mv "$dir/tmp.$$" "$dir/run.$$"
printf 'fake claude\n'
while [ ! -f "$dir/go" ]; do
  [ -d "$dir" ] || exit 0
  sleep 0.1
done
exit "$(cat "$dir/go")"
"#;

/// The text of the repository's `.env`, which git ignores and `.worktreeinclude` names.
const ENV: &str = "E2E_SETTING=1\n";

/// The branch the flows make a worktree on, and its slug.
const BRANCH: &str = "e2e-work";

/// The brief the flows send.
const BRIEF: &str = "Fix the parser";

/// The session buffer of the worktree on [`BRANCH`].
const SESSION: &str = "*claude: e2e-work*";

/// How long a fake may take to start, and a listing to show its rows.
const DEADLINE: Duration = Duration::from_secs(30);

/// One run of the fake: the directory it ran in, and its arguments.
type Run = (String, Vec<String>);

/// A flow's files: the scratch directory, the repository the instance starts in, its home and
/// its configuration.
struct Flow {
    scratch: Scratch,
    repo: PathBuf,
    home: PathBuf,
    config: PathBuf,
}

impl Flow {
    /// A committed repository, `repo`, with `.env` ignored and named by `.worktreeinclude`, and
    /// `main` pushed to a bare `origin`. `sessions` is added to the `[sessions]` table.
    fn new(test: &str, sessions: &str) -> Flow {
        let scratch = Scratch::new(test);
        let repo = scratch.path().join("repo");
        let home = scratch.path().join("home");
        fs::create_dir(&home).unwrap();
        scratch.write("repo/README.md", b"# e2e\n");
        scratch.write("repo/.gitignore", b".env\n");
        scratch.write("repo/.worktreeinclude", b".env\n");
        scratch.write("repo/.env", ENV.as_bytes());
        let fake = scratch.write(
            "fake-claude",
            FAKE.replace("DIR", &scratch.path().display().to_string())
                .as_bytes(),
        );
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let config = format!(
            "[terminal]\nshell = \"/bin/sh\"\nlogin = false\n\n[sessions]\nclaude = \"{}\"\n{sessions}",
            fake.display()
        );
        let config = scratch.write("config.toml", config.as_bytes());
        set_mode(&config, 0o644);
        let flow = Flow {
            scratch,
            repo,
            home,
            config,
        };
        let origin = flow.scratch.path().join("origin.git");
        let origin = origin.to_str().expect("scratch paths are UTF-8");
        flow.git(&flow.repo, &["init", "-q", "-b", "main"]);
        flow.git(&flow.repo, &["add", "-A"]);
        flow.git(&flow.repo, &["commit", "-q", "--no-gpg-sign", "-m", "base"]);
        flow.git(flow.scratch.path(), &["init", "-q", "--bare", origin]);
        flow.git(&flow.repo, &["remote", "add", "origin", origin]);
        flow.git(&flow.repo, &["push", "-q", "origin", "main"]);
        flow
    }

    /// Starts an instance in the repository with `README.md` shown, and trusts the project as
    /// a person does. Returns it and `README.md`'s buffer id.
    fn start(&self) -> (App, Value) {
        let mut app = App::start_with_config_and_env(
            &self.repo,
            &self.config,
            &[("HOME", self.home.as_os_str())],
        );
        app.wait_until_ready();
        app.open_file(&self.repo.join("README.md"));
        app.wait_until_ready();
        app.wait_for("the project", |state| {
            state["modeline"]
                .as_str()
                .is_some_and(|modeline| modeline.contains("  repo  restricted  "))
        });
        app.send_keys(&format!("M-x {} RET y", keys("trust-project")));
        let state = app.wait_for("the trust", |state| {
            state["echo"]
                .as_str()
                .is_some_and(|echo| echo.starts_with("Trusted "))
        });
        let readme = state["buffer"]["id"].clone();
        (app, readme)
    }

    /// Runs git in `dir` with `args`, which must succeed, with a fixed identity and kept from
    /// the person's configuration and hooks. Returns what it printed.
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let git = Executable::from_path("git", None).expect("git is on PATH");
        let hooks = self.scratch.path().join("hooks");
        fs::create_dir_all(&hooks).expect("cannot make the hooks directory");
        let mut hooks_path = OsString::from("core.hooksPath=");
        hooks_path.push(&hooks);
        let identity = ["user.name=e2e", "user.email=e2e@example.invalid"];
        let args: Vec<OsString> = ["-c".into(), hooks_path]
            .into_iter()
            .chain(identity.iter().flat_map(|set| ["-c".into(), set.into()]))
            .chain(args.iter().map(Into::into))
            .collect();
        let set = [
            ("GIT_CONFIG_GLOBAL", OsStr::new("/dev/null")),
            ("GIT_CONFIG_NOSYSTEM", OsStr::new("1")),
        ];
        let output = spawn(&Spawn {
            executable: &git,
            args: &args,
            cwd: Some(dir),
            project: None,
            inherit: &["HOME"],
            stdin: Source::Closed,
            stdout: Sink::Piped,
            stderr: Sink::Piped,
            own_process_group: false,
            set: &set,
            terminal: None,
        })
        .and_then(Child::wait_with_output)
        .expect("cannot run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// The fake's next run exits with `code` once it is there.
    fn exit_with(&self, code: u8) {
        self.scratch.write("go", code.to_string().as_bytes());
    }

    /// Each run of the fake so far.
    fn runs(&self) -> Vec<Run> {
        let mut runs: Vec<Run> = fs::read_dir(self.scratch.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("run."))
            .filter_map(|entry| {
                let text = fs::read_to_string(entry.path()).ok()?;
                let mut fields = text.split('\0').map(str::to_owned).collect::<Vec<_>>();
                // Each field ends with a NUL, so the last split is empty.
                fields.pop();
                let cwd = fields.first()?.clone();
                Some((cwd, fields.split_off(1)))
            })
            .collect();
        runs.sort();
        runs
    }

    /// Waits for the fake's `count`th run, and returns the runs.
    fn wait_for_runs(&self, count: usize) -> Vec<Run> {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let runs = self.runs();
            if runs.len() >= count {
                return runs;
            }
            assert!(
                Instant::now() < deadline,
                "the fake ran {} times",
                runs.len()
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Where the worktree on [`BRANCH`] goes.
    fn place(&self) -> PathBuf {
        self.repo.join(".claude/worktrees").join(BRANCH)
    }
}

/// The keys that type `text`.
fn keys(text: &str) -> String {
    let keys: Vec<String> = text
        .chars()
        .map(|c| match c {
            ' ' => "SPC".to_owned(),
            c => c.to_string(),
        })
        .collect();
    keys.join(" ")
}

/// What the open prompt or question says, if one is open.
fn asks(state: &Value) -> Option<String> {
    state["prompts"].as_array()?.first()?["prompt"]
        .as_str()
        .map(str::to_owned)
}

/// The terminal of the buffer named `name`.
fn terminal<'a>(state: &'a Value, name: &str) -> Option<&'a Value> {
    state["terminals"]
        .as_array()?
        .iter()
        .find(|terminal| terminal["name"] == name)
}

/// The names of the instance's buffers.
fn buffer_names(state: &Value) -> Vec<String> {
    state["buffers"]
        .as_array()
        .map(|buffers| {
            buffers
                .iter()
                .filter_map(|buffer| buffer["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The selected buffer's lines, as many as a listing takes.
fn text(app: &mut App) -> Vec<String> {
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 400 })));
    text["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The selected buffer's lines once `done` holds of them.
fn text_until(app: &mut App, what: &str, done: impl Fn(&[String]) -> bool) -> Vec<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let lines = text(app);
        if done(&lines) {
            return lines;
        }
        assert!(Instant::now() < deadline, "no {what}: {lines:?}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// Whether a listing's line is a row's first: a worktree's path and branch.
fn is_row(line: &str) -> bool {
    !line.starts_with(' ') && line.contains(".claude/worktrees/")
}

/// `C-x a w` on [`BRANCH`] with [`BRIEF`], sent with `C-c C-c`. Returns the fake's run.
fn make_worktree(flow: &Flow, app: &mut App) -> Run {
    app.send_keys("C-x a w");
    app.wait_for("the branch prompt", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Worktree branch"))
    });
    app.send_keys(&format!("{} RET", keys(BRANCH)));
    app.wait_for("*Brief*", |state| state["buffer"]["name"] == "*Brief*");
    app.send_keys(&keys(BRIEF));
    app.send_keys("C-c C-c");
    let mut runs = flow.wait_for_runs(1);
    app.wait_for("the session's terminal", |state| {
        terminal(state, SESSION).is_some()
    });
    runs.remove(0)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_brief_makes_a_worktree_with_the_included_files_and_starts_its_session() {
    let flow = Flow::new("worktrees-create", "");
    let (mut app, _) = flow.start();
    let (cwd, args) = make_worktree(&flow, &mut app);

    let place = flow.place();
    assert!(place.is_dir(), "no worktree at {}", place.display());
    assert_eq!(
        fs::read_to_string(place.join(".env")).ok().as_deref(),
        Some(ENV),
        ".worktreeinclude's file was not copied"
    );
    let branch = flow.git(&place, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(branch.trim(), BRANCH);

    // The fake ran in the worktree, with the brief after its header as one argument.
    let place = fs::canonicalize(&place).unwrap();
    assert_eq!(Path::new(&cwd), place);
    let end = args.iter().position(|arg| arg == "--");
    let end = end.unwrap_or_else(|| panic!("no -- in {args:?}"));
    assert_eq!(args.len(), end + 2, "one argument after --: {args:?}");
    let prompt = &args[end + 1];
    assert!(
        prompt.starts_with("You are in a git worktree of") && prompt.ends_with(BRIEF),
        "{prompt}"
    );
    let state = app.state();
    assert!(
        buffer_names(&state)
            .iter()
            .any(|name| name.starts_with("*claude: ")),
        "{:?}",
        buffer_names(&state)
    );
    flow.exit_with(0);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_second_session_in_one_directory_is_asked_about_and_n_starts_none() {
    let flow = Flow::new("worktrees-shared", "");
    let (mut app, readme) = flow.start();
    app.send_keys("C-x a c");
    flow.wait_for_runs(1);
    app.wait_for("the session's terminal", |state| {
        terminal(state, "*claude: repo*").is_some()
    });

    // From the repository's root again, where the first one runs.
    app.show_buffer(&readme);
    app.send_keys("C-x a c");
    let state = app.wait_for("the question", |state| asks(state).is_some());
    let question = asks(&state).unwrap_or_default();
    assert!(
        question.starts_with("Another session runs in this directory"),
        "{question}"
    );
    app.send_keys("n");
    let state = app.state();
    assert_eq!(state["prompts"], json!([]));
    // Nothing should start: give a start the time it would take.
    thread::sleep(Duration::from_secs(1));
    assert_eq!(flow.runs().len(), 1, "a second fake ran");
    let state = app.state();
    assert_eq!(state["terminals"].as_array().map(Vec::len), Some(1));
    flow.exit_with(0);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_worktree_is_kept_until_its_work_is_merged_then_removed_with_its_branch_kept() {
    let flow = Flow::new("worktrees-remove", "");
    let (mut app, readme) = flow.start();
    // The fake exits at once, so the session stops.
    flow.exit_with(0);
    make_worktree(&flow, &mut app);
    app.wait_for("the session's stop", |state| {
        terminal(state, SESSION)
            .is_some_and(|terminal| terminal["ended"] == "stopped: session-resume to continue")
    });
    let place = flow.place();
    let row = format!(".claude/worktrees/{BRANCH}  {BRANCH}");

    app.show_buffer(&readme);
    app.send_keys(&format!("M-x {} RET", keys("worktree-list")));
    app.wait_for("*Worktrees*", |state| {
        state["buffer"]["name"] == "*Worktrees*"
    });
    let lines = text_until(&mut app, "the row", |lines| lines.contains(&row));
    assert!(
        lines.iter().any(|line| line.contains("not merged")),
        "{lines:?}"
    );

    // An untracked file keeps it.
    let note = flow
        .scratch
        .write("repo/.claude/worktrees/e2e-work/note.txt", b"note\n");
    app.send_keys("M-< C-n C-n D");
    let state = app.wait_for("the refusal", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains(" kept: "))
    });
    let echo = state["echo"].as_str().unwrap_or_default();
    assert!(
        echo.ends_with("kept: 0 changed and 1 untracked files are not committed"),
        "{echo}"
    );
    assert!(place.is_dir(), "the worktree went");

    // Its work committed, merged into main and pushed.
    fs::remove_file(note).unwrap();
    flow.scratch
        .write("repo/.claude/worktrees/e2e-work/work.txt", b"work\n");
    flow.git(&place, &["add", "work.txt"]);
    flow.git(&place, &["commit", "-q", "--no-gpg-sign", "-m", "work"]);
    flow.git(&flow.repo, &["merge", "-q", "--ff-only", BRANCH]);
    flow.git(&flow.repo, &["push", "-q", "origin", "main"]);
    app.send_keys("g");
    text_until(&mut app, "the merge", |lines| {
        lines
            .iter()
            .any(|line| line.contains("merged (in origin/main)"))
    });

    // The question names what git ignores there; `y` removes it and keeps the branch.
    app.send_keys("M-< C-n C-n D");
    let state = app.wait_for("the question", |state| asks(state).is_some());
    let question = asks(&state).unwrap_or_default();
    assert!(
        question.starts_with("Remove worktree ") && question.contains(".env"),
        "{question}"
    );
    app.send_keys("y");
    app.wait_for("the removal", |state| {
        !place.exists()
            && state["echo"]
                .as_str()
                .is_some_and(|echo| echo.starts_with("Removed worktree "))
    });
    let listed = flow.git(&flow.repo, &["branch", "--list", BRANCH]);
    assert!(listed.contains(BRANCH), "the branch went");
    let lines = text_until(&mut app, "the empty list", |lines| {
        lines.iter().any(|line| line == "No linked worktrees.")
    });
    assert!(!lines.iter().any(|line| is_row(line)), "{lines:?}");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn steer_worktrees_gives_a_session_the_hook_that_denies_enter_worktree() {
    let flow = Flow::new("worktrees-steer", "steer_worktrees = true\n");
    let (mut app, _) = flow.start();
    app.send_keys("C-x a c");
    let (_, args) = flow.wait_for_runs(1).remove(0);
    let settings: Vec<&str> = args
        .iter()
        .filter_map(|arg| arg.strip_prefix("--settings="))
        .collect();
    assert_eq!(settings.len(), 1, "{args:?}");
    let settings: Value = serde_json::from_str(settings[0]).expect("--settings is JSON");
    let command = settings["hooks"]["PreToolUse"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
        .filter_map(|hook| hook["command"].as_str())
        .find(|command| command.ends_with("hook deny-worktree"));
    let command = command.unwrap_or_else(|| panic!("no PreToolUse hook: {settings}"));

    // Run as Claude Code runs a hook's command: by a shell, the call's JSON on stdin.
    let (status, stdout) = run_hook(command, r#"{"tool_name":"EnterWorktree","tool_input":{}}"#);
    assert!(
        stdout.contains(r#""permissionDecision":"deny""#),
        "{stdout}"
    );
    assert_eq!(status.code(), Some(0));
    flow.exit_with(0);
}

/// Runs a hook's `command` with `/bin/sh -c`, `input` on its standard input. Returns its exit
/// status and what it printed.
fn run_hook(command: &str, input: &str) -> (ExitStatus, String) {
    let sh = Executable::absolute("/bin/sh").expect("/bin/sh is there");
    let args: Vec<OsString> = vec!["-c".into(), command.into()];
    let mut child = spawn(&Spawn {
        executable: &sh,
        args: &args,
        cwd: None,
        project: None,
        inherit: &[],
        stdin: Source::Piped,
        stdout: Sink::Piped,
        stderr: Sink::Null,
        own_process_group: false,
        set: &[],
        terminal: None,
    })
    .expect("cannot start the hook");
    // Dropped after the write, which closes it.
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .expect("cannot write the hook's input");
    let output = child.wait_with_output().expect("cannot wait for the hook");
    (
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

/// A measurement for the coordinator: `worktree-list` over 20 linked worktrees, none of whose
/// branches was pushed, so no row asks `gh`. Prints the time to the first row and to all, and
/// bounds the first generously. Run with `--nocapture` to see them.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn twenty_worktrees_are_listed_and_the_time_is_printed() {
    const COUNT: usize = 20;
    let flow = Flow::new("worktrees-timing", "");
    for n in 0..COUNT {
        let branch = format!("wt-{n:02}");
        let path = format!(".claude/worktrees/{branch}");
        flow.git(&flow.repo, &["worktree", "add", "-q", "-b", &branch, &path]);
    }
    let (mut app, _) = flow.start();
    let started = Instant::now();
    app.send_keys(&format!("M-x {} RET", keys("worktree-list")));
    let mut first = None;
    let all = loop {
        let lines = text(&mut app);
        let rows = lines.iter().filter(|line| is_row(line)).count();
        if rows > 0 && first.is_none() {
            first = Some(started.elapsed());
        }
        let pending = lines
            .iter()
            .any(|line| line.contains("more being looked at"));
        if rows == COUNT && !pending {
            break started.elapsed();
        }
        if started.elapsed() >= DEADLINE {
            let state = app.state();
            panic!("{rows} rows: {lines:?}; state: {state}");
        }
        thread::sleep(Duration::from_millis(5));
    };
    let first = first.unwrap_or(all);
    report(&format!(
        "worktree-list over {COUNT} worktrees: first row {first:?}, all rows {all:?}"
    ));
    assert!(
        first < Duration::from_secs(5),
        "the first row took {first:?}"
    );
}

/// Prints a measurement, which `cargo test` shows with `--nocapture`.
#[expect(
    clippy::disallowed_macros,
    reason = "a measurement for the coordinator, which `cargo test` captures"
)]
fn report(line: &str) {
    eprintln!("{line}");
}
