//! What a native session's permission card is drawn at (ADR 0170, ADR 0180): the width of the
//! window that carries it, and for a question's or a plan's card, which grows to what it says, the
//! rows that window has for it.
//!
//! Must not depend on: key presses or how a frame is drawn; it reads the frame's layout.
//!
//! A card no window shows keeps the size it was drawn at: nothing is drawn again for a session
//! the person is not in.

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;
use oso_core::frame::WindowRegions;
use oso_core::window::{DockSlot, WindowId};

use crate::app::App;
use crate::sessions::native::card::{self, Question};
use crate::sessions::worker::SessionKey;

/// What a card is drawn at while no window shows its session.
pub(super) const UNSHOWN: (usize, usize) = (80, card::UNSHOWN_ROWS);
/// The rows of the transcript a card that grows leaves.
const TRANSCRIPT: usize = 4;
/// The most rows a card that grows takes: what its dock gives it.
const MOST: usize = 60;

/// The rows a composer has around its text, and a session's footer: a card that grows leaves them,
/// whether they show now or gave way to it, so that its room is the same either way.
const AROUND: [usize; 2] = [2, 1];

/// The rows a card that grows may take of a window that gives its transcript `transcript` rows
/// now, the card `card`, and `around`, which of a composer's padding and the footer show.
fn room(transcript: usize, card: usize, around: [bool; 2]) -> usize {
    let hidden: usize = (AROUND.iter().zip(around))
        .map(|(rows, shows)| if shows { 0 } else { *rows })
        .sum();
    (transcript + card)
        .saturating_sub(TRANSCRIPT + hidden)
        .min(MOST)
}

/// The rows a card that grows may take of `carrier`, which carries what is docked under `transcript`
/// and shows `card` rows of it.
fn room_of(
    editor: &Editor,
    transcript: BufferId,
    (id, carrier): (WindowId, &WindowRegions),
    card: usize,
) -> usize {
    let frame = editor.frame();
    let composer = editor.dock_window(transcript, DockSlot::Composer);
    // With no composer none is missing.
    let padded = composer.is_none_or(|id| frame.padding(id).is_some());
    room(
        carrier.text.rows,
        card,
        [padded, frame.footer(id).is_some()],
    )
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
        let (id, carrier) = (self.editor.frame().windows().iter()).find_map(|(id, regions)| {
            let window = self.editor.window_by_id(*id)?;
            let holds = inside.is_none_or(|inside| {
                (regions.text.row..regions.modeline.row).contains(&inside.row)
                    && (regions.gutter.column..=regions.text.column).contains(&inside.column)
            });
            let its = window.docked().is_none() && window.buffer() == native.buffer && holds;
            its.then_some((*id, regions))
        })?;
        if carrier.text.columns == 0 {
            return None;
        }
        let question = self.hosting.natives.permissions.asks.shown(key);
        let grows = (question.and_then(|ask| ask.question())).is_some_and(Question::grows);
        let rows = if grows {
            let card = shown.map_or(0, |shown| shown.rows);
            room_of(&self.editor, native.buffer, (id, carrier), card)
        } else {
            0
        };
        Some((carrier.gutter.columns + carrier.text.columns, rows))
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::{BufferId, Mode};
    use oso_core::editor::Editor;
    use oso_core::frame::FrameSize;
    use oso_core::window::DockSlot;

    use super::super::SETTLES;
    use super::{MOST, TRANSCRIPT, room, room_of};

    #[test]
    fn a_card_takes_all_but_four_rows_of_its_window_at_most_sixty_and_stays_so_once_drawn() {
        let shown = [true, true];
        assert_eq!(room(35, 0, shown), 35 - TRANSCRIPT);
        // Drawn, it has the rows it took: its room is the same.
        for card in [5, 12, 31] {
            assert_eq!(room(35 - card, card, shown), room(35, 0, shown));
        }
        assert_eq!(room(200, 0, shown), MOST);
        assert_eq!(room(3, 0, shown), 0);
        // What gave way to it is not its own: its room is as with them.
        assert_eq!(room(36, 0, [true, false]), room(35, 0, shown));
        assert_eq!(room(38, 0, [false, false]), room(35, 0, shown));
    }

    /// A session whose card grows to its room, with a strip and a draft.
    struct Setup {
        editor: Editor,
        host: BufferId,
        card: BufferId,
        composer: BufferId,
    }

    fn lines(rows: usize) -> String {
        vec!["x"; rows].join("\n")
    }

    impl Setup {
        fn new(rows: usize, strip: usize) -> Self {
            let mut editor = Editor::default();
            let host = editor.add_native_session_buffer("*claude: s*");
            editor.show(host);
            let mut dock = |slot, mode, text: &str| {
                let id = editor.add_internal_buffer(format!("*{slot:?}*"), mode);
                editor.set_internal_text(id, text);
                editor.set_dock(host, slot, Some(id));
                id
            };
            dock(DockSlot::Strip, Mode::Todo, &lines(strip));
            let card = dock(DockSlot::Card, Mode::Card, "x");
            let composer = dock(DockSlot::Composer, Mode::Composer, "");
            editor.set_footer(host, Some(String::new()));
            editor.set_frame_size(FrameSize { rows, columns: 80 });
            Self {
                editor,
                host,
                card,
                composer,
            }
        }

        /// The card drawn again until its room is what it was drawn at: how often, and its rows.
        fn settle(&mut self) -> (usize, usize) {
            let mut drawn = 0;
            let mut at = self.editor.buffer(self.card).unwrap().text().line_count();
            loop {
                let id = self.editor.window().id();
                let regions = *self.editor.regions(id).unwrap();
                let card = self.editor.dock_window(self.host, DockSlot::Card).unwrap();
                let rows = self.editor.regions(card).map_or(0, |r| r.text.rows);
                let now = room_of(&self.editor, self.host, (id, &regions), rows).max(1);
                if now == at {
                    return (drawn, at);
                }
                drawn += 1;
                assert!(drawn < 20, "it goes on: {at} and {now}");
                at = now;
                self.editor.set_internal_text(self.card, &lines(now));
            }
        }
    }

    /// However the draft and the window change, the card's rows settle within the times it is drawn
    /// again, and what is around the composer is there once they did, as before.
    #[test]
    fn a_card_that_grows_settles_beside_a_composers_padding_and_the_footer() {
        for strip in [1, 6] {
            let mut setup = Setup::new(45, strip);
            let mut most = setup.settle().0;
            for (rows, draft) in [
                (45, 4),
                (45, 10),
                (45, 1),
                (24, 1),
                (24, 10),
                (60, 10),
                (60, 1),
            ] {
                let size = FrameSize { rows, columns: 80 };
                setup
                    .editor
                    .set_internal_text(setup.composer, &lines(draft));
                setup.editor.set_frame_size(size);
                let (drawn, card) = setup.settle();
                most = most.max(drawn);
                let id = setup.editor.window().id();
                let composer = setup.editor.dock_window(setup.host, DockSlot::Composer);
                let frame = setup.editor.frame();
                if card > 1 {
                    assert!(frame.padding(composer.unwrap()).is_some(), "{rows} {draft}");
                    assert!(frame.footer(id).is_some(), "{rows} {draft}");
                }
            }
            assert!(most < SETTLES, "{strip}: {most}");
        }
    }
}
