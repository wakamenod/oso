//! query-replace's two prompts, before it begins (ADR 0113): the string to replace, offering
//! the last pair as the default, then its replacement.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::command::{Args, CommandId, Effects, ExecuteError, Invocation, Prefix};
use oso_core::editor::Editor;
use oso_core::minibuffer::{self, Kind};

use super::super::{Dispatcher, Step};
use super::{Reading, Then};

/// Which of query-replace's two prompts a command waits for (ADR 0113).
#[derive(Clone)]
pub(super) enum ReplaceStage {
    /// The string to replace.
    From { regexp: bool },
    /// Its replacement, after this string.
    To { regexp: bool, from: String },
}

/// The most cells of the string to replace that the second prompt's label shows.
const REPLACE_LABEL_CELLS: usize = 40;

/// The history query-replace's prompts share, one for each kind.
fn replace_history(regexp: bool) -> &'static str {
    if regexp {
        "query-replace-regexp"
    } else {
        "query-replace"
    }
}

impl Dispatcher {
    /// Opens query-replace's first prompt, offering the last pair as the default, unless the
    /// selected buffer is read-only, which it says before anything is read (ADR 0113).
    pub(super) fn read_replace(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        regexp: bool,
    ) -> (Effects, Result<(), ExecuteError>) {
        let redraw = Effects {
            redraw: true,
            ..Effects::default()
        };
        if let Err(refused) = editor.replace_may_begin() {
            editor.message(refused.to_string());
            return (redraw, Ok(()));
        }
        let kind = if regexp { " regexp" } else { "" };
        let label = match editor.last_replace(regexp) {
            Some((from, to)) => {
                let (from, from_cut) = minibuffer::cut_to(from, REPLACE_LABEL_CELLS);
                let (to, to_cut) = minibuffer::cut_to(to, REPLACE_LABEL_CELLS);
                format!("Query replace{kind} (default {from}{from_cut} → {to}{to_cut}): ")
            }
            None => format!("Query replace{kind}: "),
        };
        let stage = ReplaceStage::From { regexp };
        self.open_replace_prompt(editor, id, prefix, label, stage);
        if regexp {
            // A regexp that does not compile keeps the prompt open, saying why (ADR 0113).
            editor.prompt_takes_regexp();
        }
        (redraw, Ok(()))
    }

    fn open_replace_prompt(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        label: String,
        stage: ReplaceStage,
    ) {
        let regexp = match &stage {
            ReplaceStage::From { regexp } | ReplaceStage::To { regexp, .. } => *regexp,
        };
        let kind = Kind::Text {
            history: Some(replace_history(regexp)),
        };
        match editor.open_prompt(label, kind) {
            Ok(prompt) => self.reading.push(Reading {
                id,
                prefix,
                prompt,
                passphrase: false,
                then: Then::Replace(stage),
                switches: Vec::new(),
            }),
            Err(error) => editor.prompt_says(error.to_string()),
        }
    }

    /// Takes the answer of one of query-replace's prompts: the string opens the second, and
    /// the replacement, or an empty string, runs the command.
    pub(super) fn replace_answered(
        &mut self,
        editor: &mut Editor,
        reading: &Reading,
        stage: &ReplaceStage,
        text: &str,
    ) -> Effects {
        let (from, to) = match stage {
            ReplaceStage::From { regexp } if !text.is_empty() => {
                let kind = if *regexp { " regexp" } else { "" };
                let (shown, cut) = minibuffer::cut_to(text, REPLACE_LABEL_CELLS);
                let label = format!("Query replace{kind} {shown}{cut} with: ");
                let stage = ReplaceStage::To {
                    regexp: *regexp,
                    from: text.to_owned(),
                };
                self.open_replace_prompt(editor, reading.id, reading.prefix, label, stage);
                return Effects {
                    redraw: true,
                    ..Effects::default()
                };
            }
            ReplaceStage::From { .. } => ("", ""),
            ReplaceStage::To { from, .. } => (from.as_str(), text),
        };
        let invocation = Invocation {
            answer: Some(from),
            second: Some(to),
            ..Invocation::default()
        };
        let args = Args::with_prefix(reading.prefix);
        self.execute(editor, reading.id, args, invocation, Step::MayJoin)
            .0
    }
}
