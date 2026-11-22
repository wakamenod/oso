//! The card of a call (ADR 0165, step 2e-2): its title and Claude's lines, from what the
//! session's stream says of the call and the request says the same of, and what `RET` shows of
//! it whole.
//!
//! Must not depend on: the app or the UI; plain functions over a call and its input.
//!
//! - A title says when the card does not show all the call does, a long tool name cut before
//!   that warning, never the warning: for a call that does more than read, a command with more
//!   than its description and limit, or holding a control character, and an edit with more than
//!   its file and strings.
//! - An `AskUserQuestion` whose questions cannot be read has no card: it is denied, so that no
//!   answers of the model's own pass as the person's.

use std::path::Path;

use serde_json::Value;

use super::{Card, Question, Questions};
use crate::git::printable;
use crate::sessions::native::render::{self, Face};
use crate::sessions::native::rows::Tool;
use crate::sessions::native::wire::card as read;
use crate::sessions::native::wire::tools::{MAX_INPUT, only_keys, pretty};

/// The most bytes of a call's input `RET` shows.
const MAX_SHOWN: usize = 4 * MAX_INPUT;
/// The most characters of a title before what it says of the rest.
const MAX_TITLE: usize = 160;
/// What a title says when the card does not show all the call does.
pub const NOT_ALL: &str = " (not all of it is shown: RET shows it)";

/// The question about the call `tool` of session `name`, which runs in `directory`, with the
/// request's `input`; `None` for an `AskUserQuestion` whose questions cannot be read.
pub fn question(
    name: &str,
    tool: &Tool,
    directory: &Path,
    input: Option<&Value>,
) -> Option<Question> {
    let card = match (tool.name.as_str(), input) {
        ("AskUserQuestion", Some(input)) => {
            Card::Questions(Questions::new(read::questions(input)?))
        }
        ("AskUserQuestion", None) => return None,
        ("ExitPlanMode", Some(input)) => match read::plan(input) {
            Some((plan, cut)) => Card::Plan { plan, cut },
            None => call(name, tool, directory, Some(input)),
        },
        _ => call(name, tool, directory, input),
    };
    Some(Question {
        card,
        details: None,
        whole: false,
    })
}

/// What `RET` shows of the call `tool` of session `name` whole, with the request's `input`:
/// its input as JSON, or a plan as the request gives it. Made when it is asked for.
pub fn details(name: &str, card: &Card, tool: &str, input: Option<&Value>) -> String {
    if let Card::Plan { plan, .. } = card {
        return format!("Session {name}'s plan, as its request gives it:\n\n{plan}\n");
    }
    let whole = input.map_or_else(
        || {
            format!(
                "Its input is over {} KiB as JSON and is not shown here.",
                MAX_INPUT >> 10
            )
        },
        |input| pretty(input, MAX_SHOWN),
    );
    format!("Session {name} asks to run {tool}:\n\n{whole}\n")
}

/// The card of a call that runs: its tool and target, what it runs or changes, and why.
fn call(name: &str, tool: &Tool, directory: &Path, input: Option<&Value>) -> Card {
    let target = render::target(tool, directory);
    let field = |key: &str| {
        input
            .and_then(|input| input.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let mut lines = Vec::new();
    let mut odd = false;
    let title = match tool.name.as_str() {
        "Bash" => {
            if let Some(command) = field("command") {
                // A control character, but a line break or a tab, shows as nothing like it.
                odd = command
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\t'));
                lines.push((format!("$ {command}"), Face::Target));
            }
            format!("Bash wants to run, in session {name}")
        }
        "Edit" | "MultiEdit" | "Write" => {
            lines.push((target.clone(), Face::Target));
            if let Some(input) = input {
                let (diff, cut) = read::diff(&tool.name, input);
                odd = cut;
                lines.extend(diff.into_iter().map(|(mark, line)| {
                    let face = if mark == '-' {
                        Face::Removed
                    } else {
                        Face::Added
                    };
                    (format!("{mark}{line}"), face)
                }));
            }
            format!("{} wants to change a file, in session {name}", tool.name)
        }
        other => {
            if !target.is_empty() {
                lines.push((target.clone(), Face::Target));
            }
            format!("{other} wants to run, in session {name}")
        }
    };
    if let Some(why) = field("description").filter(|why| !why.is_empty()) {
        lines.push((format!("Why: {why}"), Face::Target));
    }
    let all =
        !odd && !target.ends_with('…') && input.is_some_and(|input| shows_all(&tool.name, input));
    let title = printable(&title, MAX_TITLE);
    let title = if all {
        title
    } else {
        format!("{title}{NOT_ALL}")
    };
    Card::Call { title, lines }
}

/// Whether what a card shows of a call of `name` is all its `input` does: for the calls that
/// read, a command with nothing but its description and limit, and an edit with nothing but
/// its file and strings.
fn shows_all(name: &str, input: &Value) -> bool {
    match name {
        "Read" | "Glob" | "Grep" | "WebSearch" | "WebFetch" => true,
        "Bash" => only_keys(input, &["command", "description", "timeout"]),
        "Edit" => only_keys(input, &["file_path", "old_string", "new_string"]),
        "Write" => only_keys(input, &["file_path", "content"]),
        _ => false,
    }
}
