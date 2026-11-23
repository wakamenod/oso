//! The pages of a question's or a plan's card (ADR 0180): turned with `SPC` and `DEL`, and which
//! of a question's options have been on screen, as only those may be chosen.
//!
//! Must not depend on: key presses or how a frame is drawn; the card's keys ask, and this carries
//! out.

use crate::app::App;
use crate::sessions::worker::SessionKey;

impl App {
    /// Turns session `key`'s card `pages` pages on, back if negative.
    pub(super) fn turn_card(&mut self, key: SessionKey, pages: isize) {
        let asks = &mut self.hosting.natives.permissions.asks;
        let Some(question) = asks.shown_mut(key).and_then(|ask| ask.question.as_mut()) else {
            return;
        };
        let grows = question.grows();
        if question.turn(pages) {
            self.draw_card(key);
            self.request_redraw();
        } else if grows {
            self.editor.message("No more rows that way");
        } else {
            self.editor
                .message("RET shows all of it or goes back; C-v and M-v scroll it");
        }
    }

    /// Whether the card of session `key`'s question turned to option `number`, which has not
    /// been on screen, in place of choosing it.
    pub(super) fn turned_to(&mut self, key: SessionKey, number: u8) -> bool {
        let number = usize::from(number);
        let asks = &mut self.hosting.natives.permissions.asks;
        let Some(question) = asks.shown_mut(key).and_then(|ask| ask.question.as_mut()) else {
            return false;
        };
        if question.has_shown(number) || !question.offers(number) {
            return false;
        }
        question.go_to(number);
        self.draw_card(key);
        self.editor.message(format!(
            "Option {number} was not on screen yet: type its number again"
        ));
        self.request_redraw();
        true
    }

    /// After each sync: what each card in a window that shows all of its page says is on screen.
    pub(super) fn note_cards_on_screen(&mut self) {
        for at in 0..self.hosting.natives.permissions.drawn.len() {
            let (key, _) = self.hosting.natives.permissions.drawn[at];
            if !self.card_shows_all(key) {
                continue;
            }
            let asks = &mut self.hosting.natives.permissions.asks;
            if let Some(question) = asks.shown_mut(key).and_then(|ask| ask.question.as_mut()) {
                question.on_screen();
            }
        }
    }
}
