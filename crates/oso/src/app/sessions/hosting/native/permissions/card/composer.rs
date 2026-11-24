//! A native session's card answered from the session's empty composer (ADR 0180): `y`, `n` and
//! `N` of a plan's card, and `n`, `N`, `o` and the digit of an option of a
//! question's, typed there with nothing written, answer the card in place of going in. No letter
//! typed there answers a call (ADR 0182).
//!
//! Must not depend on: how a key is dispatched, or how a frame is drawn; the app asks before it
//! hands a key to the dispatcher, which runs the card's command in place of typing.
//!
//! - Only where the card's own keys would answer now (`gate`): after the quiet moment since the
//!   card showed, since the composer was last empty and in front, and since the key before;
//!   never from a macro, nor of a card cut by its window. Where one would not, it is typed: it
//!   neither answers nor is lost.
//! - Never a plan's `a`, `A` and `e`, which grant a mode beyond the plan or open its edit and
//!   begin many a sentence, nor `RET`, the composer's line break: on the card only.
//! - Only the person's key: no text an input method commits, no repeat of a held key, and
//!   nothing replayed or held back for later.

use std::time::Instant;

use oso_core::buffer::Mode;
use oso_core::editor::NativeAction;
use oso_core::key::KeyPress;

use super::Card;
use crate::app::App;
use crate::sessions::native::card::Question;

/// Whether `question`'s card takes the character `c` typed in its session's empty composer.
fn takes(question: &Question, c: char) -> bool {
    match &question.card {
        Card::Plan { .. } => matches!(c, 'y' | 'n' | 'N'),
        Card::Questions(_) if matches!(c, 'n' | 'N' | 'o') => true,
        Card::Questions(questions) => {
            let number = c.to_digit(10).and_then(|digit| usize::try_from(digit).ok());
            let options = (questions.current()).map_or(0, |asked| asked.options.len());
            // One not on screen yet is typed.
            number.is_some_and(|n| (1..=options).contains(&n) && question.has_shown(n))
        }
    }
}

/// Whether `action` is one a card takes from its session's composer.
pub(super) fn takes_action(action: NativeAction) -> bool {
    matches!(
        action,
        NativeAction::Allow
            | NativeAction::Deny
            | NativeAction::Note
            | NativeAction::Choose(_)
            | NativeAction::Other
    )
}

impl App {
    /// Whether `key`, the person's next, answers the card of the session whose empty composer
    /// is selected, in place of being typed there.
    pub(in crate::app) fn composer_key_answers(&self, key: KeyPress) -> bool {
        let Some(c) = key.text() else {
            return false;
        };
        let selected = self.editor.window().buffer();
        let composer = (self.editor.buffer(selected)).is_some_and(|b| b.mode() == Mode::Composer);
        let Some(session) = composer.then(|| self.composer_session(selected)).flatten() else {
            return false;
        };
        (self.hosting.natives.permissions.asks.shown(session))
            .and_then(|ask| ask.question())
            .is_some_and(|question| takes(question, c))
            && self.composer_empty(session)
            && {
                let now = self.agent_person_typed().unwrap_or_else(Instant::now);
                (self.card_refusal(session, selected, self.agent_key_before(), now)).is_none()
            }
    }
}

#[cfg(test)]
mod tests {
    use super::takes;
    use crate::sessions::native::card::{Card, Question, Questions};
    use crate::sessions::native::wire::card::Asked;

    fn question(card: Card) -> Question {
        Question::new(card)
    }

    fn taken(question: &Question) -> String {
        "yYnNoOaAeE0123456789 \n"
            .chars()
            .filter(|&c| takes(question, c))
            .collect()
    }

    #[test]
    fn only_the_keys_that_answer_the_card_asked_are_taken_from_the_composer() {
        // Never what grants a mode after a plan, or edits it.
        let plan = question(Card::Plan {
            plan: String::new(),
            cut: false,
        });
        assert_eq!(taken(&plan), "ynN");
        let asked = Asked {
            question: "Which?".into(),
            header: String::new(),
            options: vec![("A".into(), String::new()), ("B".into(), String::new())],
            multi: false,
        };
        let mut which = question(Card::Questions(Questions::new(vec![asked])));
        assert_eq!(taken(&which), "nNo", "its options not on screen yet");
        let _ = which.shown(80, 40);
        which.on_screen();
        assert_eq!(taken(&which), "nNo12", "its two options");
    }
}
