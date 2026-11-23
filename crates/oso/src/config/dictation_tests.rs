//! The tests of `[dictation]` in [`parse`](super::parse): each entry, the defaults, and what is
//! said of a bad one (ADR 0175).
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use std::path::PathBuf;
use std::time::Duration;

use super::tests::{Fixture, messages};

const ID: &str = "0F3C2B1A-1234-4ABC-9DEF-0123456789AB";

#[test]
fn nothing_set_gives_the_system_curl_and_two_minutes() {
    let f = Fixture::new();
    let loaded = f.parse("[dictation]\n");
    let dictation = &loaded.settings.dictation;
    assert_eq!(dictation, &super::Dictation::default());
    assert_eq!(dictation.curl(), PathBuf::from("/usr/bin/curl"));
    assert_eq!(dictation.max_recording(), Duration::from_secs(120));
}

#[test]
fn each_entry_is_taken_as_written() {
    let f = Fixture::new();
    let absolute = if cfg!(windows) {
        "C:/bin/curl"
    } else {
        "/opt/bin/curl"
    };
    let text = format!(
        "[dictation]\ndiscovery = \"~/Library/Application Support/TypeWhisper-Dev/api-discovery.json\"\n\
         curl = \"{absolute}\"\nworkflow = \"{ID}\"\nmax_seconds = 600\n"
    );
    let loaded = f.parse(&text);
    assert_eq!(messages(&loaded), Vec::<String>::new());
    let dictation = &loaded.settings.dictation;
    assert_eq!(
        dictation.discovery.as_deref(),
        Some("~/Library/Application Support/TypeWhisper-Dev/api-discovery.json")
    );
    assert_eq!(dictation.curl(), PathBuf::from(absolute));
    assert_eq!(dictation.workflow.as_deref(), Some(ID));
    assert_eq!(dictation.max_recording(), Duration::from_secs(600));
}

#[test]
fn a_bad_entry_is_said_and_left_out() {
    let f = Fixture::new();
    for (entry, said) in [
        (
            "discovery = \"api-discovery.json\"",
            "takes an absolute path",
        ),
        ("curl = \"curl\"", "[dictation] curl takes an absolute path"),
        ("curl = 1", "takes the absolute path of curl, not a number"),
        ("workflow = \"not a uuid\"", "takes a workflow's id, a UUID"),
        (
            "workflow = true",
            "takes a workflow's id, a UUID, not true or false",
        ),
        ("max_seconds = 4", "from 5 to 1800"),
        ("max_seconds = 1801", "from 5 to 1800"),
        ("max_seconds = \"60\"", "from 5 to 1800"),
        ("microphone = \"built-in\"", "microphone"),
        (
            "fix_model = \"sonnet --tools=Bash\"",
            "fix_model takes a model's name",
        ),
        ("fix_model = \"\"", "fix_model takes a model's name"),
        ("fix_effort = \"none\"", "fix_effort takes low, medium"),
        ("fix_context_lines = 21", "from 0 to 20"),
        ("fix_min_confidence = \"sure\"", "takes low, medium or high"),
        (
            "fix_min_confidence = 1",
            "takes low, medium or high, not a number",
        ),
    ] {
        let loaded = f.parse(&format!("[dictation]\n{entry}\n"));
        let messages = messages(&loaded);
        assert_eq!(messages.len(), 1, "{entry}");
        assert!(messages[0].contains(said), "{entry}: {}", messages[0]);
        assert_eq!(
            loaded.settings.dictation,
            super::Dictation::default(),
            "{entry}"
        );
    }
}

#[test]
fn the_fix_takes_sonnet_at_low_effort_three_lines_and_medium_unless_set() {
    let f = Fixture::new();
    let loaded = f.parse("[dictation]\n");
    let dictation = &loaded.settings.dictation;
    assert_eq!(dictation.fix_model(), "sonnet");
    assert_eq!(dictation.fix_effort(), "low");
    assert_eq!(dictation.fix_context_lines(), 3);
    assert_eq!(
        dictation.fix_min_confidence(),
        crate::dictation::fix::Confidence::Medium
    );

    let loaded = f.parse(
        "[dictation]\nfix_model = \"claude-haiku-5-5\"\nfix_effort = \"medium\"\n\
         fix_context_lines = 0\nfix_min_confidence = \"low\"\n",
    );
    assert_eq!(messages(&loaded), Vec::<String>::new());
    let dictation = &loaded.settings.dictation;
    assert_eq!(dictation.fix_model(), "claude-haiku-5-5");
    assert_eq!(dictation.fix_effort(), "medium");
    assert_eq!(dictation.fix_context_lines(), 0);
    assert_eq!(
        dictation.fix_min_confidence(),
        crate::dictation::fix::Confidence::Low
    );
}
