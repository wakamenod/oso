//! What an agent's open or save starts in the running app (ADR 0118, ADR 0138 step 1): it waits
//! in the buffer for the person, the modeline says `waits: server`, the person's first key there
//! or their next save lets it go, and a restricted project holds nothing. The dev build's
//! stand-in (`OSO_DEV_STARTS=1`) stands for a language server, and says in `*Agent*` what it ran.
//! The dev socket's keys stand in for the person's.
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
fn record_line(root: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(root).expect("the scratch project");
    let path = root.to_str().expect("a UTF-8 scratch path");
    format!("{path}\tdev={} ino={}\n", meta.dev(), meta.ino())
}

/// A project at `name` under the scratch directory, with a `.git` and `files`.
fn project(scratch: &Scratch, name: &str, files: &[&str]) -> std::path::PathBuf {
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
fn what_an_agents_open_or_save_starts_waits_for_the_person_and_a_restricted_project_holds_nothing()
{
    let scratch = Scratch::new("starts");
    set_mode(scratch.path(), 0o700);
    // The configuration directory is apart from the projects: the agent never saves there.
    let config = scratch.write("conf/config.toml", b"");
    set_mode(&scratch.path().join("conf"), 0o700);
    set_mode(&config, 0o644);
    let trusted = project(&scratch, "proj-starts", &["main.rs", "lib.rs"]);
    project(&scratch, "proj-free", &["free.rs"]);
    let record = scratch.write("conf/trusted-projects", record_line(&trusted).as_bytes());
    set_mode(&record, 0o600);
    let main = trusted.join("src/main.rs");
    let stand_in = std::ffi::OsStr::new("1");
    // The person visits main.rs: what opening it starts runs at once.
    let mut app = App::start_with_file_config_and_env(
        &main,
        scratch.path(),
        &config,
        &[("OSO_DEV_STARTS", stand_in)],
    );
    app.wait_until_ready();
    let mut agent = let_in(&mut app);
    wait_until(
        &mut app,
        &mut agent,
        "the person's open ran",
        |app, agent| agent_log(app, agent).contains("stand-in ran server-start in main.rs"),
    );
    assert_eq!(waits(&mut agent, "main.rs"), json!([]));
    an_agents_open_waits_for_the_persons_key(&mut app, &mut agent, &trusted.join("src/lib.rs"));
    an_agents_save_waits_for_the_persons_save(&mut app, &mut agent, &main);
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
        modeline.contains("proj-starts  waits: server  "),
        "{modeline}"
    );
    assert!(!agent_log(app, agent).contains("ran server-start in lib.rs"));
    app.send_keys("C-f");
    wait_until(app, agent, "lib.rs's open let go", |_, agent| {
        waits(agent, "lib.rs") == json!([])
    });
    let log = agent_log(app, agent);
    assert!(log.contains("the person let go: server in lib.rs"), "{log}");
    assert!(log.contains("stand-in ran server-start in lib.rs"), "{log}");
    thread::sleep(AFTER_TYPING);
}

/// The agent's save of `main`, which the person visited, starts nothing either, and waits for
/// the person's next save: `C-x C-s` in the unmodified buffer.
fn an_agents_save_waits_for_the_persons_save(app: &mut App, agent: &mut Agent, main: &Path) {
    let opened = agent.call("open", Some(json!({ "path": main }))).unwrap();
    assert_eq!(opened.get("waits"), None, "already open: {opened}");
    let inserted = agent.call(
        "run",
        Some(json!({ "name": "insert", "arg": "// a\n", "reach": "change" })),
    );
    assert!(inserted.is_ok(), "{inserted:?}");
    let saved = agent.run("save-buffer", None, "change");
    let waiting = saved.as_ref().ok().and_then(|reply| reply.get("waiting"));
    assert!(waiting.is_none(), "main.rs is the person's: {saved:?}");
    let saved = saved.unwrap();
    assert_eq!(
        saved["waits"], "server waits for the person in main.rs",
        "{saved}"
    );
    assert_eq!(waits(agent, "main.rs"), json!(["server"]));
    assert!(!agent_log(app, agent).contains("ran server-save"));
    app.send_keys("C-x C-s");
    wait_until(app, agent, "main.rs's save let go", |_, agent| {
        waits(agent, "main.rs") == json!([])
    });
    let log = agent_log(app, agent);
    assert!(
        log.contains("the person let go: server in main.rs"),
        "{log}"
    );
    assert!(log.contains("stand-in ran server-save in main.rs"), "{log}");
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
