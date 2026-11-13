//! The tests of `[sessions]` in [`parse`](super::parse): `claude`'s path and `env`'s names, and
//! what is reported of bad ones (ADR 0121).
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use std::path::PathBuf;

use super::tests::{Fixture, messages};

#[test]
fn claude_is_an_absolute_path() {
    let f = Fixture::new();
    let absolute = if cfg!(windows) {
        "C:/bin/claude"
    } else {
        "/opt/bin/claude"
    };
    let loaded = f.parse(&format!("[sessions]\nclaude = \"{absolute}\"\n"));
    assert_eq!(messages(&loaded), Vec::<String>::new());
    assert_eq!(
        loaded.settings.sessions.claude,
        Some(PathBuf::from(absolute))
    );
    for (value, said) in [
        ("\"claude\"", "takes an absolute path on this computer"),
        (
            "\"//host/claude\"",
            "takes an absolute path on this computer",
        ),
        ("3", "takes the absolute path of claude, not a number"),
    ] {
        let loaded = f.parse(&format!("[sessions]\nclaude = {value}\n"));
        let messages = messages(&loaded);
        assert_eq!(messages.len(), 1, "{value}");
        assert!(messages[0].ends_with(said), "{}", messages[0]);
        assert_eq!(loaded.settings.sessions.claude, None);
    }
}

#[test]
fn env_names_are_kept_and_refused_ones_said() {
    let f = Fixture::new();
    let loaded = f.parse(
        "[sessions]\nenv = [\"HTTPS_PROXY\", \"PATH\", \"LD_PRELOAD\", \"dyld_x\", \"A=B\", \"\", \"HTTPS_PROXY\", \"NODE_OPTIONS\"]\n",
    );
    assert_eq!(
        loaded.settings.sessions.env,
        ["HTTPS_PROXY", "NODE_OPTIONS"]
    );
    let messages = messages(&loaded);
    assert_eq!(messages.len(), 5, "{messages:?}");
    assert!(
        messages
            .iter()
            .all(|m| m.contains("[sessions] env cannot pass"))
    );
    let loaded = f.parse("[sessions]\nenv = \"HTTPS_PROXY\"\n");
    assert!(messages_of(&loaded).contains("takes a list of variables' names"));
    let many: Vec<String> = (0..=super::settings::MAX_ENV_NAMES)
        .map(|n| format!("\"V{n}\""))
        .collect();
    let loaded = f.parse(&format!("[sessions]\nenv = [{}]\n", many.join(", ")));
    assert!(messages_of(&loaded).contains("at most 64 names"));
    assert!(loaded.settings.sessions.env.is_empty());
    let loaded = f.parse("[sessions]\nshell = \"/bin/sh\"\n");
    assert!(
        messages_of(&loaded).contains("shell"),
        "an unknown key is said"
    );
}

#[test]
fn notify_toggles_default_and_are_set_one_by_one() {
    let f = Fixture::new();
    let loaded = f.parse("");
    let defaults = loaded.settings.sessions.notify;
    assert_eq!(
        (
            defaults.needs_input,
            defaults.done,
            defaults.failed,
            defaults.ready_for_review
        ),
        (true, true, true, false)
    );
    let loaded = f.parse("[sessions.notify]\ndone = false\nready_for_review = true\n");
    assert_eq!(messages(&loaded), Vec::<String>::new());
    let notify = loaded.settings.sessions.notify;
    assert!(notify.needs_input && !notify.done && notify.failed && notify.ready_for_review);
}

#[test]
fn a_wrong_notify_toggle_keeps_its_default_and_is_said() {
    let f = Fixture::new();
    let loaded = f.parse("[sessions.notify]\nfailed = \"no\"\nbanner = true\ndone = false\n");
    let said = messages_of(&loaded);
    assert!(
        said.contains("[sessions.notify] failed takes true or false, not a string"),
        "{said}"
    );
    assert!(said.contains("banner"), "{said}");
    let notify = loaded.settings.sessions.notify;
    assert!(notify.failed && !notify.done);
    let loaded = f.parse("[sessions]\nnotify = 3\n");
    assert!(messages_of(&loaded).contains("[sessions.notify]"));
    assert_eq!(loaded.settings.sessions.notify, Default::default());
}

fn messages_of(loaded: &super::Loaded) -> String {
    messages(loaded).join("\n")
}

#[test]
fn steering_worktrees_is_off_until_set() {
    let f = Fixture::new();
    assert!(!f.parse("[sessions]\n").settings.sessions.steer_worktrees);
    let loaded = f.parse("[sessions]\nsteer_worktrees = true\n");
    assert_eq!(messages(&loaded), Vec::<String>::new());
    assert!(loaded.settings.sessions.steer_worktrees);
    let loaded = f.parse("[sessions]\nsteer_worktrees = \"yes\"\n");
    let messages = messages(&loaded);
    assert_eq!(messages.len(), 1);
    assert!(
        messages[0].ends_with("[sessions] steer_worktrees takes true or false, not a string"),
        "{}",
        messages[0]
    );
    assert!(!loaded.settings.sessions.steer_worktrees);
}
