//! The tests of `[sessions]` in [`parse`](super::parse): `claude`'s path and `env`'s names, and
//! what is reported of bad ones (ADR 0121).
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use std::path::PathBuf;

use super::tests::{Fixture, messages};
use crate::sessions::tools::StartLimit;

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

#[test]
fn agents_start_without_asking_and_its_limit() {
    let f = Fixture::new();
    let loaded = f.parse("[sessions]\n");
    assert!(!loaded.settings.sessions.agents_start_without_asking);
    assert_eq!(
        loaded.settings.sessions.agents_start_limit(),
        StartLimit::Most(4),
        "the default"
    );
    let loaded =
        f.parse("[sessions]\nagents_start_without_asking = true\nagents_start_limit = 8\n");
    assert_eq!(messages(&loaded), Vec::<String>::new());
    assert!(loaded.settings.sessions.agents_start_without_asking);
    assert_eq!(
        loaded.settings.sessions.agents_start_limit(),
        StartLimit::Most(8)
    );
    // No limit but the loop guard (ADR 0144).
    let loaded = f.parse("[sessions]\nagents_start_limit = \"none\"\n");
    assert_eq!(messages(&loaded), Vec::<String>::new());
    assert_eq!(
        loaded.settings.sessions.agents_start_limit(),
        StartLimit::Unlimited
    );
    for (value, said) in [
        ("0", "not 0"),
        ("9", "not 9"),
        ("-1", "not -1"),
        ("2.5", "not a number"),
        ("\"4\"", "not \"4\""),
        ("\"None\"", "not \"None\""),
        ("true", "not true or false"),
    ] {
        let loaded = f.parse(&format!("[sessions]\nagents_start_limit = {value}\n"));
        let messages = messages(&loaded);
        assert_eq!(messages.len(), 1, "{value}");
        assert!(
            messages[0].ends_with(&format!(
                "[sessions] agents_start_limit takes a number from 1 to 8, or \"none\", {said}"
            )),
            "{}",
            messages[0]
        );
        assert_eq!(
            loaded.settings.sessions.agents_start_limit(),
            StartLimit::Most(4),
            "{value}"
        );
    }
    let loaded = f.parse("[sessions]\nagents_start_without_asking = 1\n");
    assert_eq!(messages(&loaded).len(), 1);
    assert!(!loaded.settings.sessions.agents_start_without_asking);
}

#[test]
fn pane_width_takes_24_to_80_columns_and_says_any_other() {
    let f = Fixture::new();
    assert_eq!(f.parse("[sessions]\n").settings.sessions.pane_width, None);
    for columns in [24, 44, 80] {
        let loaded = f.parse(&format!("[sessions]\npane_width = {columns}\n"));
        assert_eq!(messages(&loaded), Vec::<String>::new());
        assert_eq!(loaded.settings.sessions.pane_width, Some(columns));
    }
    for (value, said) in [
        ("23", "not 23"),
        ("81", "not 81"),
        ("-4", "not -4"),
        ("99999999999999999999", "not 9"),
        ("44.5", "not a number"),
        ("\"44\"", "not a string"),
    ] {
        let loaded = f.parse(&format!("[sessions]\npane_width = {value}\n"));
        let said_all = messages_of(&loaded);
        assert!(
            said_all
                .contains("[sessions] pane_width takes a whole number of columns from 24 to 80"),
            "{value}: {said_all}"
        );
        assert!(said_all.contains(said), "{value}: {said_all}");
        assert_eq!(loaded.settings.sessions.pane_width, None, "{value}");
    }
}
