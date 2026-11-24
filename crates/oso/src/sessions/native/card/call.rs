//! The card of an `AskUserQuestion` and of an `ExitPlanMode` (ADR 0165, step 2e-2), from what the
//! request carries, and what `RET` shows of a plan whole. Another call has no card (ADR 0182).
//!
//! Must not depend on: the app or the UI; plain functions over a call's input.
//!
//! - An `AskUserQuestion` whose questions cannot be read has no card: it is denied, so that no
//!   answers of the model's own pass as the person's.

use serde_json::Value;

use super::{Card, Question, Questions};
use crate::sessions::native::wire::card as read;

/// What the card of a call of `tool` with the request's `input` asks; `None` for an
/// `AskUserQuestion` whose questions cannot be read, and for a call that has no card.
pub fn question(tool: &str, input: Option<&Value>) -> Option<Question> {
    let card = match tool {
        "AskUserQuestion" => Card::Questions(Questions::new(read::questions(input?)?)),
        "ExitPlanMode" => {
            let (plan, cut) = read::plan(input?)?;
            Card::Plan { plan, cut }
        }
        _ => return None,
    };
    Some(Question::new(card))
}

/// What `RET` shows of `card` of session `name` whole: a plan, as the request gives it. Made when it
/// is asked for.
pub fn details(name: &str, card: &Card) -> String {
    match card {
        Card::Plan { plan, .. } => {
            format!("Session {name}'s plan, as its request gives it:\n\n{plan}\n")
        }
        Card::Questions(_) => String::new(),
    }
}
