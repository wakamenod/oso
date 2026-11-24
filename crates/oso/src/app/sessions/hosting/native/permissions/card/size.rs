//! What a native session's permission card is drawn at (ADR 0170, ADR 0180): the width of the
//! window that carries it, and for a question's or a plan's card, which grows to what it says, the
//! rows that window has for it.
//!
//! Must not depend on: key presses or how a frame is drawn; it reads the frame's layout.
//!
//! A card no window shows keeps the size it was drawn at: nothing is drawn again for a session
//! the person is not in.

use oso_core::window::DockSlot;

use crate::app::App;
use crate::sessions::native::card::{self, Question};
use crate::sessions::worker::SessionKey;

/// What a card is drawn at while no window shows its session.
pub(super) const UNSHOWN: (usize, usize) = (80, card::UNSHOWN_ROWS);
/// The rows of the transcript a card that grows leaves.
const TRANSCRIPT: usize = 4;
/// The most rows a card that grows takes: what its dock gives it.
const MOST: usize = 60;

/// The rows a card that grows may take of a window that gives its transcript `transcript` rows
/// now and the card `card`.
fn room(transcript: usize, card: usize) -> usize {
    (transcript + card).saturating_sub(TRANSCRIPT).min(MOST)
}

impl App {
    /// What session `key`'s card is drawn at now: its width, and the rows it has if it grows, else
    /// none; `None` while no window carries the session's docks.
    pub(super) fn card_size(&self, key: SessionKey) -> Option<(usize, usize)> {
        let native = self.hosting.natives.map.get(&key)?;
        let dock = |slot| {
            let window = self.editor.dock_window(native.buffer, slot)?;
            let text = self.editor.regions(window)?.text;
            (text.rows > 0 && text.columns > 0).then_some(text)
        };
        let shown = dock(DockSlot::Card);
        // A dock is inside the rows of the window that carries it, from its left edge.
        let inside = shown.or_else(|| dock(DockSlot::Composer));
        let carrier = (self.editor.frame().windows().iter()).find_map(|(id, regions)| {
            let window = self.editor.window_by_id(*id)?;
            let holds = inside.is_none_or(|inside| {
                (regions.text.row..regions.modeline.row).contains(&inside.row)
                    && (regions.gutter.column..=regions.text.column).contains(&inside.column)
            });
            let its = window.docked().is_none() && window.buffer() == native.buffer && holds;
            its.then_some(regions)
        })?;
        if carrier.text.columns == 0 {
            return None;
        }
        let question = self.hosting.natives.permissions.asks.shown(key);
        let grows = (question.and_then(|ask| ask.question())).is_some_and(Question::grows);
        let rows = if grows {
            room(carrier.text.rows, shown.map_or(0, |shown| shown.rows))
        } else {
            0
        };
        Some((carrier.gutter.columns + carrier.text.columns, rows))
    }
}

#[cfg(test)]
mod tests {
    use super::{MOST, TRANSCRIPT, room};

    #[test]
    fn a_card_takes_all_but_four_rows_of_its_window_at_most_sixty_and_stays_so_once_drawn() {
        assert_eq!(room(35, 0), 35 - TRANSCRIPT);
        // Drawn, it has the rows it took: its room is the same.
        for card in [5, 12, 31] {
            assert_eq!(room(35 - card, card), room(35, 0));
        }
        assert_eq!(room(200, 0), MOST);
        assert_eq!(room(3, 0), 0);
    }
}
