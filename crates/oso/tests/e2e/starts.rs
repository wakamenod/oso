//! What an agent's open starts in the running app (ADR 0118, ADR 0138 steps 1 and 3): the
//! person's open starts the project's language server at once, the agent's open of a file in
//! another project waits in the buffer for the person, the modeline says `waits: server`, the
//! person's first key there lets it go and starts that project's server, and a restricted
//! project holds nothing. The server is the fake one, named by the configuration as
//! rust-analyzer's command; `*Agent*` says what ran. What a save starts waits for ADR 0138's
//! step 4, which tells a server of a save. The dev socket's keys stand in for the person's.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The rules
//! themselves have unit tests in the editor.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::agent::{Agent, let_in};
use super::config::set_mode;
use super::harness::{App, Scratch};

/// How long the channel waits after the person's last key, and a moment more.
const AFTER_TYPING: Duration = Duration::from_millis(1200);

/// The line of the record of trusted projects that trusts `root`.
#[cfg(unix)]
pub(super) fn record_line(root: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(root).expect("the scratch project");
    let path = root.to_str().expect("a UTF-8 scratch path");
    format!("{path}\tdev={} ino={}\n", meta.dev(), meta.ino())
}

/// A configuration whose rust-analyzer is the fake server, answering `initialize`.
pub(super) fn fake_server_config(scratch: &Scratch) -> std::path::PathBuf {
    let script = scratch.write(
        "conf/fake-lsp.json",
        json!({"rules": [
            {"method": "initialize", "do": [{"reply": {"capabilities": {"positionEncoding": "utf-8"}}}]},
            {"method": "shutdown", "do": [{"reply": null}]},
        ]})
        .to_string()
        .as_bytes(),
    );
    let toml = format!(
        "[lsp.servers.rust-analyzer]\ncommand = [{:?}, {:?}]\n",
        env!("CARGO_BIN_EXE_fake-lsp-server"),
        script.to_str().expect("a UTF-8 scratch path"),
    );
    scratch.write("conf/config.toml", toml.as_bytes())
}

/// A project at `name` under the scratch directory, with a `.git` and `files`.
pub(super) fn project(scratch: &Scratch, name: &str, files: &[&str]) -> std::path::PathBuf {
    for file in files {
        scratch.write(&format!("{name}/src/{file}"), b"fn main() {}\n");
    }
    let root = scratch.path().join(name);
    std::fs::create_dir(root.join(".git")).unwrap();
    root
}

/// The rows of `*Agent*`. The agent shows it by name and then the buffer shown before: its
/// commands let nothing go, so reading the log changes nothing it checks.
fn agent_log(app: &mut App, agent: &mut Agent) -> String {
    let back = app.state()["buffer"]["name"].as_str().unwrap().to_owned();
    show(agent, "*Agent*");
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 2000 })));
    show(agent, &back);
    text["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Has the agent show the buffer called `name`, once the person's last key is far enough back.
fn show(agent: &mut Agent, name: &str) {
    let started = Instant::now();
    loop {
        let params = json!({ "name": "switch-to-buffer", "arg": name, "reach": "look" });
        match agent.call("run", Some(params)) {
            Ok(_) => return,
            Err(e) if started.elapsed() < Duration::from_secs(5) => {
                let _ = e;
                thread::sleep(Duration::from_millis(200));
            }
            Err(e) => panic!("the agent cannot show {name}: {e}"),
        }
    }
}

/// What `state` says waits in the buffer called `name`.
fn waits(agent: &mut Agent, name: &str) -> Value {
    let state = agent.call("state", None).unwrap();
    state["buffers"]
        .as_array()
        .and_then(|buffers| buffers.iter().find(|b| b["name"] == name))
        .map_or(Value::Null, |buffer| buffer["waits"].clone())
}

/// Waits until `done` holds of what the person's app and the agent see, for at most 10 s.
fn wait_until(
    app: &mut App,
    agent: &mut Agent,
    what: &str,
    done: impl Fn(&mut App, &mut Agent) -> bool,
) {
    let started = Instant::now();
    while !done(app, agent) {
        if started.elapsed().as_secs() >= 10 {
            let log = agent_log(app, agent);
            let state = app.state();
            panic!(
                "never: {what}\n*Agent*:\n{log}\nbuffers: {}",
                state["buffers"]
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn what_an_agents_open_starts_waits_for_the_person_and_a_restricted_project_holds_nothing() {
    let scratch = Scratch::new("starts");
    set_mode(scratch.path(), 0o700);
    // The configuration directory is apart from the projects: the agent never saves there.
    let config = fake_server_config(&scratch);
    set_mode(&scratch.path().join("conf"), 0o700);
    set_mode(&config, 0o644);
    let trusted = project(&scratch, "proj-starts", &["main.rs"]);
    let other = project(&scratch, "proj-other", &["lib.rs"]);
    project(&scratch, "proj-free", &["free.rs"]);
    let lines = record_line(&trusted) + &record_line(&other);
    let record = scratch.write("conf/trusted-projects", lines.as_bytes());
    set_mode(&record, 0o600);
    let main = trusted.join("src/main.rs");
    // The person visits main.rs: what opening it starts runs at once.
    let mut app = App::start_with_file_config_and_env(&main, scratch.path(), &config, &[]);
    app.wait_until_ready();
    let mut agent = let_in(&mut app);
    wait_until(
        &mut app,
        &mut agent,
        "the person's open ran",
        |app, agent| agent_log(app, agent).contains("ran server-start in main.rs"),
    );
    assert_eq!(waits(&mut agent, "main.rs"), json!([]));
    app.wait_for("the server is ready", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("proj-starts  lsp  "))
    });
    an_agents_open_waits_for_the_persons_key(&mut app, &mut agent, &other.join("src/lib.rs"));
    a_restricted_project_holds_nothing(
        &mut app,
        &mut agent,
        &scratch.path().join("proj-free/src/free.rs"),
    );
}

/// The agent's open of `lib` starts nothing, the buffer says so, and the person's first key
/// there lets it go.
fn an_agents_open_waits_for_the_persons_key(app: &mut App, agent: &mut Agent, lib: &Path) {
    let opened = agent.call("open", Some(json!({ "path": lib }))).unwrap();
    assert_eq!(opened["name"], "lib.rs");
    wait_until(app, agent, "lib.rs's open waits", |_, agent| {
        waits(agent, "lib.rs") == json!(["server"])
    });
    if let Some(note) = opened.get("waits") {
        assert_eq!(note, "server waits for the person in lib.rs");
    }
    let modeline = app.state()["modeline"].as_str().unwrap().to_owned();
    assert!(
        modeline.contains("proj-other  waits: server  "),
        "{modeline}"
    );
    assert!(!agent_log(app, agent).contains("ran server-start in lib.rs"));
    app.send_keys("C-f");
    wait_until(app, agent, "lib.rs's open let go", |_, agent| {
        waits(agent, "lib.rs") == json!([])
    });
    let log = agent_log(app, agent);
    assert!(log.contains("the person let go: server in lib.rs"), "{log}");
    assert!(log.contains("ran server-start in lib.rs"), "{log}");
    app.wait_for("lib.rs's server is ready", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("proj-other  lsp  "))
    });
    thread::sleep(AFTER_TYPING);
}

/// The agent's open of `free`, in a project that is not trusted, holds nothing and runs nothing.
fn a_restricted_project_holds_nothing(app: &mut App, agent: &mut Agent, free: &Path) {
    let opened = agent.call("open", Some(json!({ "path": free }))).unwrap();
    assert_eq!(opened.get("waits"), None, "{opened}");
    let state = app.wait_for("free.rs restricted", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("proj-free  restricted  "))
    });
    let modeline = state["modeline"].as_str().unwrap();
    assert!(!modeline.contains("waits"), "{modeline}");
    assert_eq!(waits(agent, "free.rs"), json!([]));
    app.send_keys("C-f");
    thread::sleep(Duration::from_millis(300));
    let log = agent_log(app, agent);
    assert!(
        !log.contains("in free.rs"),
        "nothing ran or waited there: {log}"
    );
}
