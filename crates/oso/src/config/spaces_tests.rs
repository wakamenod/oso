//! The tests of `[spaces]` in [`parse`](super::parse): `root`, its default, `naming`, and what is said of a
//! bad one (ADR 0136).
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use std::path::{Path, PathBuf};

use super::paths::expand;
use super::spaces::DEFAULT_ROOT;
use super::tests::{Fixture, messages};

/// The start of an absolute path here.
const ABSOLUTE: &str = if cfg!(windows) { "C:/" } else { "/" };

#[test]
fn root_is_absolute_or_under_home_and_defaults_to_spaces() {
    let f = Fixture::new();
    let loaded = f.parse("[spaces]\n");
    assert_eq!(loaded.settings.spaces.root, None);
    assert_eq!(loaded.settings.spaces.root(), DEFAULT_ROOT);
    let absolute = if cfg!(windows) {
        "C:/work/spaces"
    } else {
        "/work/spaces"
    };
    for written in ["~/work/spaces", absolute] {
        let loaded = f.parse(&format!("[spaces]\nroot = \"{written}\"\n"));
        assert_eq!(messages(&loaded), Vec::<String>::new());
        assert_eq!(loaded.settings.spaces.root(), written);
    }
}

#[test]
fn a_bad_root_is_said_and_the_default_stays() {
    let f = Fixture::new();
    let long = format!("{ABSOLUTE}{}", "a".repeat(1024));
    for (value, said) in [
        (
            "\"spaces\"",
            "takes an absolute path, or one starting with ~/",
        ),
        ("\"//host/spaces\"", "takes a path on this computer"),
        (
            &format!("\"{long}\"") as &str,
            "takes a path of at most 1024 bytes",
        ),
        ("3", "takes a directory's path, not a number"),
        ("\"~/a/../spaces\"", "takes a path with no `..`"),
    ] {
        let loaded = f.parse(&format!("[spaces]\nroot = {value}\n"));
        let messages = messages(&loaded);
        assert_eq!(messages.len(), 1, "{value}");
        assert!(messages[0].contains(said), "{}", messages[0]);
        assert_eq!(loaded.settings.spaces.root(), DEFAULT_ROOT);
    }
    let loaded = f.parse("[spaces]\nplace = 1\n");
    assert_eq!(messages(&loaded).len(), 1);
}

#[test]
fn a_root_under_home_needs_a_home() {
    let home = PathBuf::from(format!("{ABSOLUTE}h"));
    assert_eq!(expand("~/spaces", Some(&home)), Some(home.join("spaces")));
    assert_eq!(expand("~/spaces", None), None);
    assert_eq!(expand("~/spaces", Some(Path::new("relative"))), None);
    let written = format!("{ABSOLUTE}w/s");
    assert_eq!(expand(&written, None), Some(PathBuf::from(&written)));
}

#[test]
fn naming_is_claude_unless_local_and_anything_else_is_said_and_taken_as_local() {
    use super::spaces::SpacesNaming as Naming;
    let f = Fixture::new();
    assert_eq!(f.parse("[spaces]\n").settings.spaces.naming, Naming::Claude);
    let local = f.parse("[spaces]\nnaming = \"local\"\n");
    assert_eq!(messages(&local), Vec::<String>::new());
    assert_eq!(local.settings.spaces.naming, Naming::Local);
    for value in ["\"gpt\"", "true"] {
        let loaded = f.parse(&format!("[spaces]\nnaming = {value}\n"));
        let messages = messages(&loaded);
        assert_eq!(messages.len(), 1, "{value}");
        assert!(
            messages[0].contains("\"claude\" or \"local\""),
            "{}",
            messages[0]
        );
        assert_eq!(loaded.settings.spaces.naming, Naming::Local, "fails closed");
    }
}

#[test]
fn orchestrator_answers_are_off_unless_tools_are_listed() {
    let f = Fixture::new();
    let none = f.parse("[spaces]\n").settings.spaces;
    assert!(none.answers.is_empty());
    assert!(none.answers_deny.is_empty());
    let rules = none.answer_rules();
    assert!(rules.tools.is_empty(), "off by default");
    assert_eq!(
        rules.deny.len(),
        crate::sessions::native::stand_in::DEFAULT_DENY.len()
    );
    let loaded = f.parse(
        "[spaces]\norchestrator-answers = [\"Read\", \"Edit\", \"Read\"]\n\
         orchestrator-answers-deny = ['\\bmake\\b']\n",
    );
    assert_eq!(messages(&loaded), Vec::<String>::new());
    let rules = loaded.settings.spaces.answer_rules();
    assert_eq!(rules.tools, ["Read", "Edit"]);
    // The person's patterns add to the defaults, which cannot be taken away.
    let defaults = crate::sessions::native::stand_in::DEFAULT_DENY.len();
    assert_eq!(rules.deny.len(), defaults + 1);
    assert!(rules.deny[defaults].is_match("make all"));
    assert!(
        rules
            .deny
            .iter()
            .any(|pattern| pattern.is_match("git push"))
    );
    let empty =
        f.parse("[spaces]\norchestrator-answers = [\"Bash\"]\norchestrator-answers-deny = []\n");
    assert_eq!(messages(&empty), Vec::<String>::new());
    assert_eq!(empty.settings.spaces.answer_rules().deny.len(), defaults);
}

#[test]
fn a_bad_orchestrator_answers_key_is_said_and_answers_stay_off() {
    let f = Fixture::new();
    for (written, said) in [
        (
            "orchestrator-answers = \"Bash\"",
            "takes a list of tools' names",
        ),
        ("orchestrator-answers = [1]", "takes a list of tools' names"),
        (
            "orchestrator-answers = [\"mcp__oso__open\"]",
            "takes no MCP tool",
        ),
        ("orchestrator-answers = [\"Bash \"]", "of ASCII letters"),
        ("orchestrator-answers-deny = ['(']", "cannot read"),
        ("orchestrator-answers-deny = ['']", "takes patterns of 1 to"),
        (
            "orchestrator-answers-deny = true",
            "takes a list of patterns",
        ),
    ] {
        // A bad deny list turns off the tools listed beside it.
        let tools = match written.starts_with("orchestrator-answers-deny") {
            true => "orchestrator-answers = [\"Read\"]\n",
            false => "",
        };
        let loaded = f.parse(&format!("[spaces]\n{tools}{written}\n"));
        let messages = messages(&loaded);
        assert_eq!(messages.len(), 1, "{written}: {messages:?}");
        assert!(messages[0].contains(said), "{}", messages[0]);
        let spaces = &loaded.settings.spaces;
        assert!(spaces.answers_refused, "{written}");
        assert!(spaces.answer_rules().tools.is_empty(), "{written}");
    }
    let long = format!(
        "[spaces]\norchestrator-answers = [{}]\n",
        vec!["\"Read\""; 65].join(", ")
    );
    assert!(messages(&f.parse(&long))[0].contains("takes at most 64"));
}
