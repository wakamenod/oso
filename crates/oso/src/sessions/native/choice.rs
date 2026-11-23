//! A native session's model and effort level as the person chose them (ADR 0165, part 3c):
//! `/model` and `/effort`, sent as text, are noted once the CLI says it took them, so that the
//! modeline shows the effort after the model and each later process starts with both.
//!
//! Must not depend on: threads, the stream or the UI; the caller gives what it sent and the
//! events.
//!
//! - No stream field says the effort, and `system/init` says the model only at the next turn
//!   (measured, CLI 2.1.330), so oso notes what it sent. A sent `/model` or `/effort`, the
//!   CLI's own (its list's, or its name while the list says not whose a command is), is
//!   asked; the `result` that ends its turn notes it when it names that local command, is no
//!   error, and its reply says it was set: `Set model to `, `Set effort level to `, or
//!   `Effort level set to auto`, which clears the effort. Any other reply, `Kept model as`,
//!   `Current model:`, a model not found, an unknown option, or a level the CLI capped,
//!   notes nothing. The replies are the CLI's (2.1.330): one that changes notes nothing, the
//!   safe side.
//! - One message is under way at a time (the queue sends the next after the turn's end), so
//!   one command is asked at a time, and any `result` ends it. A new process forgets it.
//! - What is noted goes on a command line (`--model=`, `--effort=`): a model is one word that
//!   reads as a model's id (`wire::status_line::Kind::Id`) and does not start with `-`; an
//!   effort level is one of [`EFFORTS`], which `--effort` takes, in lower case.
//! - A fork starts with its source's choice. Across an oso restart nothing is kept: the record
//!   holds no model.

use super::slash::{SlashCommands, ends_word};
use super::wire::Event;
use super::wire::status_line::{Kind, Model, readable};

/// The levels `--effort` takes (CLI 2.1.330's `--help`), which tier 1 completes over.
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// A `/model` reply that says the model was set.
const MODEL_SET: &str = "Set model to ";
/// A `/effort` reply that says a level was set, and one that says it was cleared.
const EFFORT_SET: &str = "Set effort level to ";
const EFFORT_AUTO: &str = "Effort level set to auto";

/// What a sent command asked, if its turn's `result` says it was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Asked {
    /// `/model`, with the model it named, if it named one that can be carried.
    Model(Option<String>),
    /// `/effort`, with what it asked, if it was a level or `auto`.
    Effort(Option<Effort>),
}

/// What a `/effort` asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effort {
    Level(&'static str),
    /// `auto` or `unset`: the model's own.
    Auto,
}

/// A native session's model and effort, as the person chose them here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choice {
    model: Option<String>,
    effort: Option<&'static str>,
    asked: Option<Asked>,
}

impl Choice {
    /// `text` was written to the session as the person's message, which `commands` says
    /// whether it runs as a command.
    pub fn sent(&mut self, commands: &SlashCommands, text: &str) {
        self.asked = match commands.cli_command(text) {
            Some(("model", rest)) => Some(Asked::Model(model(rest))),
            Some(("effort", rest)) => Some(Asked::Effort(effort(rest))),
            _ => None,
        };
    }

    /// Notes what `event` says of the command asked: whether the effort noted changed.
    pub fn event(&mut self, event: &Event) -> bool {
        let Event::Result(result) = event else {
            return false;
        };
        let Some(asked) = self.asked.take() else {
            return false;
        };
        if result.error || result.subtype != "success" {
            return false;
        }
        let local = result.local_command.as_deref();
        match asked {
            Asked::Model(Some(model)) if local == Some("model") => {
                if result.reply.starts_with(MODEL_SET) {
                    self.model = Some(model);
                }
                false
            }
            Asked::Effort(Some(effort)) if local == Some("effort") => {
                let was = self.effort;
                match effort {
                    Effort::Level(level) if result.reply.starts_with(EFFORT_SET) => {
                        self.effort = Some(level);
                    }
                    Effort::Auto if result.reply.starts_with(EFFORT_AUTO) => self.effort = None,
                    _ => {}
                }
                was != self.effort
            }
            _ => false,
        }
    }

    /// A new process starts: a command asked of the one before gets no answer.
    pub fn new_process(&mut self) {
        self.asked = None;
    }

    /// What a fork of the session starts with: its choice, and nothing asked.
    pub fn forked(&self) -> Self {
        Self {
            asked: None,
            ..self.clone()
        }
    }

    /// What a new process of the session starts with: `--model` and `--effort`.
    pub fn for_start(&self) -> (Option<String>, Option<&'static str>) {
        (self.model.clone(), self.effort)
    }

    /// The model noted, as the person named it: an alias or an id.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The effort level noted.
    pub fn effort(&self) -> Option<&'static str> {
        self.effort
    }
}

/// Which of `models` says the effort levels `Effort:` completes over: the one `chosen` names,
/// by its value or its id, else the one whose id is `shown`, the model shown; `None` for
/// [`EFFORTS`], as when no list came (tier 1), or the list names neither, or says no levels of
/// it.
pub fn effort_source(models: &[Model], chosen: Option<&str>, shown: Option<&str>) -> Option<usize> {
    let says = |name: &str| {
        let named = |model: &Model| model.value == name || model.resolved == name;
        (models.iter()).position(|model| named(model) && model.efforts.is_some())
    };
    chosen.and_then(says).or_else(|| shown.and_then(says))
}

/// The words of `rest`, what follows a command's name.
fn words(rest: &str) -> impl Iterator<Item = &str> {
    rest.split(ends_word).filter(|word| !word.is_empty())
}

/// The model `rest` names, if it is one word that can go on a command line.
fn model(rest: &str) -> Option<String> {
    let mut words = words(rest);
    let model = words.next()?;
    let carried = words.next().is_none() && readable(model, Kind::Id) && !model.starts_with('-');
    carried.then(|| model.to_owned())
}

/// What `rest` asks of `/effort`, if it is one word: a level `--effort` takes, or `auto`.
fn effort(rest: &str) -> Option<Effort> {
    let mut words = words(rest);
    let word = words.next()?.to_ascii_lowercase();
    if words.next().is_some() {
        return None;
    }
    match word.as_str() {
        "auto" | "unset" => Some(Effort::Auto),
        word => EFFORTS
            .iter()
            .find(|level| **level == word)
            .map(|l| Effort::Level(l)),
    }
}

#[cfg(test)]
mod tests;
