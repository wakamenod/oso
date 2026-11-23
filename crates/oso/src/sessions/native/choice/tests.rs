//! The model and effort a native session's person chose (ADR 0165, part 3c): which sent text
//! is asked, which `result` notes it, and what a fork and a new process keep.
//!
//! Must not depend on: the app or a process.

use proptest::prelude::*;

use super::{Choice, EFFORTS};
use crate::sessions::native::slash::SlashCommands;
use crate::sessions::native::wire::slash::SlashCommand;
use crate::sessions::native::wire::{Event, parse};

fn command(name: &str, builtin: bool) -> SlashCommand {
    SlashCommand {
        name: name.to_owned(),
        description: String::new(),
        hint: String::new(),
        aliases: Vec::new(),
        builtin,
    }
}

/// The CLI's own `/model` and `/effort`, as `initialize` lists them.
fn listed() -> SlashCommands {
    let mut commands = SlashCommands::default();
    commands.initialized(vec![command("model", true), command("effort", true)]);
    commands
}

/// A local command's `result`, as the CLI ends its turn (measured, CLI 2.1.330).
fn result(local: &str, reply: &str, error: bool) -> Event {
    let line = serde_json::json!({
        "type": "result",
        "subtype": "success",
        "is_error": error,
        "num_turns": 0,
        "local_command": local,
        "result": reply,
        "session_id": "s",
    });
    parse(line.to_string().as_bytes())
}

/// `text` sent, and its turn ended by `answer`: what the choice then holds, and whether the
/// effort changed.
fn after(choice: &mut Choice, text: &str, answer: &Event) -> bool {
    choice.sent(&listed(), text);
    choice.event(answer)
}

const SET_SONNET: &str = "Set model to Sonnet 5.5 for this session only";
const SET_LOW: &str = "Set effort level to low (this session only): Quick, straightforward implementation with minimal overhead";

#[test]
fn a_model_the_cli_set_is_noted_as_the_person_named_it() {
    let mut choice = Choice::default();
    assert!(!after(
        &mut choice,
        "/model sonnet",
        &result("model", SET_SONNET, false)
    ));
    assert_eq!(choice.model(), Some("sonnet"));
    // A reply that set nothing notes nothing (the CLI's 2.1.330 replies).
    for reply in [
        "Kept model as Sonnet 5.5",
        "Current model: Sonnet 5.5",
        "Model 'nope' not found",
    ] {
        after(&mut choice, "/model opus", &result("model", reply, false));
        assert_eq!(choice.model(), Some("sonnet"), "{reply}");
    }
    after(
        &mut choice,
        "/model opus",
        &result("model", SET_SONNET, true),
    );
    assert_eq!(choice.model(), Some("sonnet"));
    // Another local command's turn, or a message's, does not answer it.
    after(
        &mut choice,
        "/model opus",
        &result("effort", SET_SONNET, false),
    );
    assert_eq!(choice.model(), Some("sonnet"));
}

#[test]
fn only_a_model_that_can_go_on_a_command_line_is_asked() {
    for text in [
        "/model",
        "/model  ",
        "/model sonnet please",
        "/model -x",
        "/model son\u{202e}net",
        "/model ｓｏｎｎｅｔ",
    ] {
        let mut choice = Choice::default();
        after(&mut choice, text, &result("model", SET_SONNET, false));
        assert_eq!(choice.model(), None, "{text:?}");
    }
    let mut choice = Choice::default();
    after(
        &mut choice,
        "/model\tclaude-opus-5-5[1m]\n",
        &result("model", SET_SONNET, false),
    );
    assert_eq!(choice.model(), Some("claude-opus-5-5[1m]"));
}

#[test]
fn an_effort_is_one_of_the_levels_and_auto_clears_it() {
    let mut choice = Choice::default();
    assert!(after(
        &mut choice,
        "/effort LOW",
        &result("effort", SET_LOW, false)
    ));
    assert_eq!(choice.effort(), Some("low"));
    // The same again changes nothing shown.
    assert!(!after(
        &mut choice,
        "/effort low",
        &result("effort", SET_LOW, false)
    ));
    // A level the CLI capped, an unknown option, ultracode: the reply set no level asked.
    for (text, reply) in [
        (
            "/effort max",
            "Effort 'max' exceeds the cap for opus; set to 'xhigh' instead",
        ),
        (
            "/effort huge",
            "Unknown option: huge. Options: low, medium, high, auto",
        ),
        ("/effort ultracode on", "Ultracode on (this session only)"),
        ("/effort high", "Couldn't set effort level: no"),
        // A reply that names another level than asked.
        (
            "/effort high",
            "Set effort level to xhigh (this session only)",
        ),
        ("/effort high", "Set effort level to highest"),
    ] {
        assert!(
            !after(&mut choice, text, &result("effort", reply, false)),
            "{text}"
        );
        assert_eq!(choice.effort(), Some("low"), "{text}");
    }
    let auto = result(
        "effort",
        "Effort level set to auto (this session only)",
        false,
    );
    assert!(after(&mut choice, "/effort auto", &auto));
    assert_eq!(choice.effort(), None);
    assert_eq!(EFFORTS.len(), 5);
}

#[test]
fn a_command_not_the_clis_own_is_not_asked() {
    let mut commands = listed();
    // A project's skill named so, once the list says whose each is.
    commands.initialized(vec![command("model", false), command("effort", true)]);
    let mut choice = Choice::default();
    choice.sent(&commands, "/model sonnet");
    choice.event(&result("model", SET_SONNET, false));
    assert_eq!(choice.model(), None);
    // A text that is no command, or one that starts with a space.
    for text in ["model sonnet", " /model sonnet", "/models sonnet"] {
        after(&mut choice, text, &result("model", SET_SONNET, false));
        assert_eq!(choice.model(), None, "{text:?}");
    }
}

#[test]
fn any_result_ends_what_was_asked_and_a_new_process_forgets_it() {
    let mut choice = Choice::default();
    choice.sent(&listed(), "/model sonnet");
    choice.event(&result("", "hello", false));
    choice.event(&result("model", SET_SONNET, false));
    assert_eq!(choice.model(), None);
    choice.sent(&listed(), "/model sonnet");
    choice.new_process();
    choice.event(&result("model", SET_SONNET, false));
    assert_eq!(choice.model(), None);
    // A message sent after it replaces it.
    choice.sent(&listed(), "/model sonnet");
    choice.sent(&listed(), "hello");
    choice.event(&result("model", SET_SONNET, false));
    assert_eq!(choice.model(), None);
}

#[test]
fn a_fork_starts_with_the_choice_and_nothing_asked() {
    let mut choice = Choice::default();
    after(
        &mut choice,
        "/model haiku",
        &result("model", SET_SONNET, false),
    );
    after(
        &mut choice,
        "/effort high",
        &result("effort", "Set effort level to high", false),
    );
    choice.sent(&listed(), "/model opus");
    let mut fork = choice.forked();
    assert_eq!((fork.model(), fork.effort()), (Some("haiku"), Some("high")));
    fork.event(&result("model", SET_SONNET, false));
    assert_eq!(fork.model(), Some("haiku"));
}

proptest! {
    /// Whatever is sent and whatever the CLI replies, what is noted can go on a command line
    /// as one argument's value.
    #[test]
    fn what_is_noted_is_always_one_plain_word(text in "/(model|effort)[ -~\u{a0}\u{202e}]{0,40}") {
        let mut choice = Choice::default();
        after(&mut choice, &text, &result("model", SET_SONNET, false));
        after(&mut choice, &text, &result("effort", SET_LOW, false));
        if let Some(model) = choice.model() {
            prop_assert!(!model.starts_with('-'));
            prop_assert!(model.bytes().all(|b| b.is_ascii_graphic()));
        }
        if let Some(effort) = choice.effort() {
            prop_assert!(EFFORTS.contains(&effort));
        }
    }
}

#[test]
fn the_levels_are_the_chosen_models_else_the_shown_ones_else_the_five() {
    use super::effort_source;
    use crate::sessions::native::wire::status_line::Model;
    let model = |value: &str, resolved: &str, efforts: Option<&[&str]>| Model {
        value: value.to_owned(),
        resolved: resolved.to_owned(),
        display: value.to_owned(),
        description: String::new(),
        efforts: efforts.map(|levels| levels.iter().map(|l| (*l).to_owned()).collect()),
    };
    let models = [
        model("opus", "claude-opus-5-5", Some(&["low", "max"])),
        model("haiku", "claude-haiku-5-5", Some(&[])),
        model("old", "claude-old-1", None),
    ];
    let shown = Some("claude-opus-5-5");
    assert_eq!(effort_source(&models, Some("haiku"), shown), Some(1));
    assert_eq!(
        effort_source(&models, Some("claude-haiku-5-5"), shown),
        Some(1)
    );
    assert_eq!(effort_source(&models, Some("unlisted"), shown), Some(0));
    assert_eq!(effort_source(&models, None, shown), Some(0));
    // A model that says no levels, or none listed: the five.
    assert_eq!(effort_source(&models, Some("old"), None), None);
    assert_eq!(effort_source(&[], Some("opus"), shown), None);
}

#[test]
fn the_recorded_models_say_their_descriptions_and_levels() {
    const STATUS: &str = include_str!("../fixtures/2.1.330/status.jsonl");
    let answer = (STATUS.lines().map(|line| parse(line.as_bytes())))
        .find_map(|event| match event {
            Event::Answer(answer) => answer.models,
            _ => None,
        })
        .unwrap();
    let opus = answer.iter().find(|model| model.value == "opus").unwrap();
    assert_eq!(opus.description, "For complex work and everyday tasks");
    assert_eq!(
        opus.efforts.as_deref(),
        Some(&EFFORTS.map(str::to_owned)[..])
    );
}

#[test]
fn a_models_levels_are_words_and_its_description_one_line() {
    let line = r#"{"type":"control_response","response":{"subtype":"success","request_id":"oso-0","response":{"models":[
        {"value":"a","resolvedModel":"claude-a","displayName":"A","description":"one\ntwo","supportedEffortLevels":["low","h·igh","","max"]},
        {"value":"b","resolvedModel":"claude-b","displayName":"B","supportsEffort":false,"supportedEffortLevels":["low"]},
        {"value":"c","resolvedModel":"claude-c","displayName":"C"}]}}}"#;
    let line = line.replace('\n', "");
    let Event::Answer(answer) = parse(line.as_bytes()) else {
        panic!("not an answer");
    };
    let models = answer.models.unwrap();
    assert_eq!(models[0].description, "one");
    let levels = |at: usize| models[at].efforts.clone();
    assert_eq!(levels(0), Some(vec!["low".to_owned(), "max".to_owned()]));
    assert_eq!(levels(1), Some(Vec::new()), "a model that takes no effort");
    assert_eq!(levels(2), None, "a model that says nothing of it");
}

#[test]
fn a_result_says_its_local_command_and_its_replys_first_line() {
    let Event::Result(result) = result("model", "Set model to Sonnet 5.5\nmore", false) else {
        panic!("not a result");
    };
    assert_eq!(result.local_command.as_deref(), Some("model"));
    assert_eq!(result.reply, "Set model to Sonnet 5.5");
    let Event::Result(result) = parse(br#"{"type":"result","result":"hi"}"#) else {
        panic!("not a result");
    };
    assert_eq!(result.local_command, None);
}
