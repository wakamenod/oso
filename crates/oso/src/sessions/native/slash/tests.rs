//! A native session's slash commands (ADR 0165, step 2d): which list counts, which text runs a
//! command, what is refused and what completion offers; and the lists as the wire reads them.
//!
//! Must not depend on: the app; the tests give what the CLI listed.

use super::{CLEAR_REFUSED, SlashCommands};
use crate::sessions::native::wire::slash::{MAX_COMMANDS, SlashCommand};
use crate::sessions::native::wire::{self, Event, MAX_NAME};

fn command(name: &str, aliases: &[&str]) -> SlashCommand {
    SlashCommand {
        name: name.to_owned(),
        description: format!("{name}'s description"),
        hint: String::new(),
        aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
        builtin: true,
    }
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn offered(commands: &SlashCommands) -> Vec<&str> {
    commands.offered().map(|c| c.name.as_str()).collect()
}

fn skill(name: &str) -> SlashCommand {
    SlashCommand {
        builtin: false,
        ..command(name, &[])
    }
}

#[test]
fn initializes_list_wins_and_system_init_names_stand_in_until_it_comes() {
    let mut commands = SlashCommands::default();
    commands.system_init(names(&["model", "pdf"]), Vec::new());
    assert_eq!(offered(&commands), ["model", "pdf"]);
    let version = commands.version();
    commands.system_init(names(&["model", "pdf"]), Vec::new());
    assert_eq!(commands.version(), version, "nothing changed");
    commands.initialized(vec![command("model", &[]), command("pdf", &["skills:pdf"])]);
    assert!(commands.version() > version);
    commands.system_init(names(&["model", "skills:pdf"]), Vec::new());
    assert_eq!(
        offered(&commands),
        ["model", "pdf"],
        "initialize's names stay"
    );
    assert!(commands.runs("/skills:pdf"), "an alias runs it");

    // A new process keeps the list until its own comes; on tier 1, its names.
    commands.new_process();
    assert_eq!(offered(&commands), ["model", "pdf"]);
    commands.system_init(names(&["model", "context"]), Vec::new());
    assert_eq!(offered(&commands), ["model", "context"]);
}

#[test]
fn only_a_listed_command_at_the_very_start_runs() {
    let mut commands = SlashCommands::default();
    commands.initialized(vec![command("model", &[]), command("context", &[])]);
    assert!(commands.runs("/model haiku"));
    assert!(commands.runs("/context"));
    assert!(commands.runs("/context\nmore"));
    // Measured: these are messages to the CLI.
    for text in [
        " /context",
        "\n/context",
        "\u{feff}/context",
        "/CONTEXT",
        "/Context",
    ] {
        assert!(!commands.runs(text), "{text:?}");
    }
    assert!(!commands.runs("/tmp/x is a path"));
    assert!(!commands.runs("/notacommand"));
    assert!(!commands.runs("/"));
    assert!(!commands.runs("context"));
}

#[test]
fn clear_and_its_aliases_are_refused_and_not_offered() {
    let mut commands = SlashCommands::default();
    // Before any list, and as the CLI splits its words (measured: a no-break space ends one).
    for text in [
        "/clear",
        "/reset",
        "/new now",
        "/clear\u{a0}x",
        "/clear\u{feff}x",
    ] {
        assert_eq!(commands.refused(text), Some(CLEAR_REFUSED), "{text:?}");
    }
    commands.initialized(vec![
        command("clear", &["reset", "new", "wipe"]),
        command("model", &[]),
    ]);
    assert_eq!(
        commands.refused("/wipe"),
        Some(CLEAR_REFUSED),
        "a listed alias"
    );
    assert_eq!(commands.refused("/model"), None);
    // Measured: these are messages, and clear nothing.
    for text in [
        " /clear",
        "\n/clear",
        "\u{feff}/clear",
        "/CLEAR",
        "/Clear",
        "clear",
    ] {
        assert_eq!(commands.refused(text), None, "{text:?}");
    }
    assert_eq!(offered(&commands), ["model"]);

    // A project's skill named `new` is no clear once the list says whose it is.
    commands.initialized(vec![command("clear", &["reset"]), skill("new")]);
    assert_eq!(commands.refused("/new"), None);
    assert_eq!(commands.refused("/reset"), Some(CLEAR_REFUSED));
    assert_eq!(offered(&commands), ["new"]);
}

#[test]
fn a_terminals_own_commands_go_if_typed_but_are_not_offered() {
    let mut commands = SlashCommands::default();
    commands.initialized(vec![command("doctor", &[]), command("model", &[])]);
    assert_eq!(
        offered(&commands),
        ["model"],
        "the measured ones, before system/init"
    );
    commands.system_init(names(&["doctor", "model"]), names(&["model"]));
    assert_eq!(
        offered(&commands),
        ["doctor"],
        "system/init's, once it lists them"
    );
    assert!(commands.runs("/model"));
    assert_eq!(commands.refused("/model"), None);
}

#[test]
fn the_lists_are_read_as_the_wire_has_them_and_cut() {
    let long = "d".repeat(MAX_NAME * 2);
    let mut listed: Vec<serde_json::Value> = vec![
        serde_json::json!({"name": "model", "description": "Set the model\nsecond line", "argumentHint": "<model>\nmore", "builtin": true}),
        serde_json::json!({"name": "pdf", "description": long, "aliases": ["skills:pdf", "has space", ""]}),
        serde_json::json!({"name": "bad name"}),
        serde_json::json!({"name": "ctl\u{7}"}),
        serde_json::json!({"name": "bidi\u{202e}"}),
        serde_json::json!({"name": "zero\u{200b}width"}),
        serde_json::json!({"name": "n".repeat(MAX_NAME + 1)}),
        serde_json::json!({"description": "no name"}),
    ];
    listed.extend((0..MAX_COMMANDS).map(|n| serde_json::json!({"name": format!("c{n}")})));
    let line = serde_json::json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": "oso-0", "response": {"commands": listed}},
    })
    .to_string();
    let Event::Answer(answer) = wire::parse(line.as_bytes()) else {
        panic!("an answer");
    };
    let commands = answer.commands.unwrap();
    assert_eq!(commands.len(), MAX_COMMANDS);
    assert_eq!(commands[0].name, "model");
    assert_eq!(commands[0].description, "Set the model");
    assert_eq!(commands[0].hint, "<model>");
    assert!(commands[0].builtin);
    assert_eq!(commands[1].aliases, ["skills:pdf"]);
    assert!(commands[1].description.len() <= MAX_NAME);
    assert!(!commands[1].builtin);
    assert_eq!(
        commands[2].name, "c0",
        "names that cannot be typed are no command's"
    );

    let line = serde_json::json!({
        "type": "system", "subtype": "init",
        "slash_commands": ["model", "/context", "bad name"],
        "terminal_slash_commands": ["doctor"],
    })
    .to_string();
    let Event::Init(init) = wire::parse(line.as_bytes()) else {
        panic!("an init");
    };
    assert_eq!(init.slash_commands, ["model", "context"]);
    assert_eq!(init.terminal_commands, ["doctor"]);

    let line = r#"{"type":"control_response","response":{"subtype":"success","request_id":"oso-1","response":{"still_queued":[]}}}"#;
    let Event::Answer(answer) = wire::parse(line.as_bytes()) else {
        panic!("an answer");
    };
    assert_eq!(answer.commands, None, "an interrupt's answer lists none");
}

#[test]
fn a_skill_listed_first_under_clears_name_shadows_no_refusal() {
    let mut commands = SlashCommands::default();
    commands.initialized(vec![skill("clear"), command("clear", &["reset"])]);
    assert_eq!(commands.refused("/clear"), Some(CLEAR_REFUSED));
    assert_eq!(commands.refused("/reset"), Some(CLEAR_REFUSED));
    assert!(commands.says_whose());
}

#[test]
fn a_list_that_marks_no_command_the_clis_own_refuses_clears_names() {
    let mut commands = SlashCommands::default();
    commands.initialized(vec![skill("clear"), skill("new"), skill("model")]);
    assert!(
        !commands.says_whose(),
        "a CLI that marks none says nothing of whose"
    );
    for text in ["/clear", "/new", "/reset"] {
        assert_eq!(commands.refused(text), Some(CLEAR_REFUSED), "{text}");
    }
    assert_eq!(offered(&commands), ["model"]);
}
