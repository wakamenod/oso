//! The windows on native sessions' transcripts across a restart (ADR 0165, part 4c-2): the
//! instance going names each session whose transcript a window shows, and the new one shows it
//! there again, at its end, with its composer selected if it was.
//!
//! Must not depend on: key presses or how a frame is drawn. The state's field is written and read
//! by the restart state's own module.
//!
//! - **Only by the record:** a session of that id which this instance's record has as a native one,
//!   with its buffer, running or not. An archived or forgotten one, one the record has in the TUI
//!   and one it does not have leave the window on what was behind the transcript.
//! - **Once:** as the state is taken, or, where the record is not read yet, as it is read. A
//!   session's resume then changes its rows in the same buffer.
//! - **Nothing is taken from the person:** a window they moved, or had show something else, or
//!   selected, since the state was taken stays theirs, and so does the selected one once they
//!   typed, or while they are in the middle of something. A search, a replace, a jump or a macro
//!   under way, which showing a buffer anywhere would end, leaves every window as it is.

use std::time::Instant;

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::Editor;
use oso_core::selection::SelectionSet;
use oso_core::window::{DockSlot, Scroll, Window, WindowId};

use crate::app::App;
use crate::carry;
use crate::carry::native_windows::{NativeWindows, Shown};

/// The windows a restart's state names, until they show their transcripts.
#[derive(Debug, Default)]
pub(super) struct Carried {
    windows: Vec<CarriedWindow>,
    /// When the state was taken.
    taken: Option<Instant>,
    /// The window selected as the state was taken.
    selected: Option<WindowId>,
    /// Its transcript's composer was selected.
    composer: bool,
}

/// A window that shows a transcript again, and where it was as the state was taken.
#[derive(Debug)]
struct CarriedWindow {
    id: WindowId,
    /// The session's id.
    session: String,
    buffer: BufferId,
    selections: SelectionSet,
    scroll: Scroll,
}

impl Carried {
    /// What `entries` name of a state whose windows are `ids` now, as `editor` has them, the
    /// `selected`th selected. One whose place is not among them is passed over.
    fn of(editor: &Editor, entries: &[NativeWindows], ids: &[WindowId], selected: usize) -> Self {
        let windows = (entries.iter())
            .flat_map(|entry| entry.windows.iter().map(move |at| (entry, *at)))
            .filter_map(|(entry, at)| {
                let window = editor.window_by_id(*ids.get(at)?)?;
                Some(CarriedWindow {
                    id: window.id(),
                    session: entry.id.clone(),
                    buffer: window.buffer(),
                    selections: window.selections.clone(),
                    scroll: window.scroll(),
                })
            })
            .collect();
        Self {
            windows,
            taken: Some(Instant::now()),
            selected: ids.get(selected).copied(),
            composer: (entries.iter())
                .any(|entry| entry.composer && entry.windows.contains(&selected)),
        }
    }

    /// Window `id` was moved by oso, not the person: where it is now is where it was.
    fn moved(&mut self, editor: &Editor, id: WindowId) {
        for carried in self.windows.iter_mut().filter(|carried| carried.id == id) {
            if let Some(window) = editor.window_by_id(id)
                && window.buffer() == carried.buffer
            {
                carried.selections = window.selections.clone();
                carried.scroll = window.scroll();
            }
        }
    }

    /// Whether the person, who typed last `at`, typed since the state was taken.
    fn used(&self, at: Option<Instant>) -> bool {
        (at.zip(self.taken)).is_some_and(|(typed, taken)| typed > taken)
    }

    /// Shows the transcript `transcript` finds of each window's session there, unless the window is
    /// the person's by now; `busy`, the selected one is. The composer is selected after, if it was,
    /// where its place holds it. Whether a window changed.
    fn show(
        self,
        editor: &mut Editor,
        busy: bool,
        transcript: impl Fn(&str) -> Option<BufferId>,
    ) -> bool {
        // Showing a buffer in any window ends it.
        if carry::under_way(editor).is_some() {
            return false;
        }
        let selected = editor.tree_window();
        let theirs = |editor: &Editor, carried: &CarriedWindow| {
            let taken = carried.id == selected && (busy || self.selected != Some(selected));
            taken
                || editor.window_by_id(carried.id).is_none_or(|window| {
                    window.buffer() != carried.buffer
                        || window.selections != carried.selections
                        || window.scroll() != carried.scroll
                })
        };
        let mut changed = false;
        let mut composer = None;
        for carried in &self.windows {
            let Some(buffer) = transcript(&carried.session).filter(|_| !theirs(editor, carried))
            else {
                continue;
            };
            editor.show_in(carried.id, buffer);
            changed = true;
            // Not while the side window is selected.
            if self.composer && carried.id == selected && editor.tiled().0.id() == selected {
                composer = editor.dock_window(buffer, DockSlot::Composer);
            }
        }
        if let Some(id) = composer.filter(|id| shows_composer(editor, *id)) {
            editor.select_window(id);
        }
        changed
    }
}

/// `order`, a restart's state's, which names no transcript (ADR 0171), with those `editor`'s windows
/// show: before all, or while the selected window shows something else, after the first.
pub(in crate::app) fn with_transcripts_shown(
    editor: &Editor,
    mut order: Vec<BufferId>,
) -> Vec<BufferId> {
    let transcript = |window: &Window| {
        (editor.buffer(window.buffer()))
            .filter(|buffer| buffer.is_native_session())
            .map(|_| window.buffer())
    };
    let selected = editor.window_by_id(editor.tree_window());
    let mut at = usize::from(selected.and_then(transcript).is_none()).min(order.len());
    for window in selected.into_iter().chain(carry::tiled_windows(editor)) {
        if let Some(buffer) = transcript(window).filter(|buffer| !order.contains(buffer)) {
            order.insert(at, buffer);
            at += 1;
        }
    }
    order
}

/// Whether docked window `id` is a composer, laid out.
fn shows_composer(editor: &Editor, id: WindowId) -> bool {
    let holds = editor
        .window_by_id(id)
        .and_then(|window| editor.buffer(window.buffer()))
        .is_some_and(|buffer| buffer.mode() == Mode::Composer);
    holds
        && editor
            .regions(id)
            .is_some_and(|regions| regions.text.rows > 0 && regions.text.columns > 0)
}

impl super::NativeSessions {
    /// Window `id` was put back where a restart's state had it, as its file came.
    pub(in crate::app) fn carried_window_moved(&mut self, editor: &Editor, id: WindowId) {
        self.resuming.windows.moved(editor, id);
    }
}

impl App {
    /// The native sessions' transcripts, for a restart to show again in the windows that show
    /// them. Not one set aside while its session runs in the TUI.
    pub(in crate::app) fn native_transcripts(&self) -> Vec<Shown> {
        (self.hosting.natives.map.iter())
            .filter_map(|(key, native)| {
                let session = self.hosting.table.get(*key)?;
                Some(Shown {
                    id: session.id.as_str().to_owned(),
                    transcript: native.buffer,
                })
            })
            .collect()
    }

    /// A restart's state names `entries` of its windows, `ids`, the `selected`th selected: each
    /// shows its transcript now, if the record is read, else as it is read.
    pub(in crate::app) fn native_windows_carried(
        &mut self,
        entries: &[NativeWindows],
        ids: &[WindowId],
        selected: usize,
    ) {
        self.hosting.natives.resuming.windows = Carried::of(&self.editor, entries, ids, selected);
        self.show_native_windows_carried();
    }

    /// Once the record of sessions is read: the windows a restart's state names show their
    /// transcripts, and none is shown later.
    pub(in crate::app) fn show_native_windows_carried(&mut self) {
        if !self.hosting.adopting.recalled {
            return;
        }
        let carried = std::mem::take(&mut self.hosting.natives.resuming.windows);
        if carried.windows.is_empty() {
            return;
        }
        let busy = self.person_in_the_middle() || carried.used(self.agent_person_typed());
        let (table, natives) = (&self.hosting.table, &self.hosting.natives);
        let transcript = |id: &str| {
            let session = table.iter().find(|session| session.id.as_str() == id)?;
            let native = natives.map.get(&session.key).filter(|_| session.native)?;
            Some(native.buffer)
        };
        if carried.show(&mut self.editor, busy, transcript) {
            self.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Buffer;
    use oso_core::frame::{FrameSize, Way};

    use super::*;

    const ID: &str = "0123abcd-0000-4000-8000-000000000001";

    /// A new instance as a state of three windows was taken: each on a file, the third selected.
    struct New {
        editor: Editor,
        ids: Vec<WindowId>,
        transcript: BufferId,
    }

    fn new() -> New {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes", "one\ntwo\nthree\n"));
        editor.set_frame_size(FrameSize {
            rows: 40,
            columns: 120,
        });
        editor.split_window(Way::Across, true).unwrap();
        editor.split_window(Way::Down, true).unwrap();
        let ids: Vec<_> = (carry::tiled_windows(&editor).iter())
            .map(|window| window.id())
            .collect();
        assert_eq!(editor.tree_window(), ids[2]);
        let transcript = editor.add_native_session_buffer("*claude: s*");
        editor.replace_native_rows(transcript, 0..0, "first\nlast\n", &[]);
        let mut composer = Buffer::from_text("*compose: s*", "");
        composer.set_mode(Mode::Composer);
        let composer = editor.add_buffer(composer);
        editor.set_dock(transcript, DockSlot::Composer, Some(composer));
        New {
            editor,
            ids,
            transcript,
        }
    }

    impl New {
        /// The state named `windows` for the session, with its composer selected or not.
        fn carried(&self, windows: &[usize], composer: bool) -> Carried {
            let entry = NativeWindows {
                id: ID.to_owned(),
                windows: windows.to_vec(),
                composer,
            };
            Carried::of(&self.editor, &[entry], &self.ids, 2)
        }

        /// Shows what `carried` says, with the session in the record.
        fn show(&mut self, carried: Carried, busy: bool) {
            let transcript = self.transcript;
            carried.show(&mut self.editor, busy, |id| {
                (id == ID).then_some(transcript)
            });
        }

        /// The places of the windows on the transcript.
        fn on_transcript(&self) -> Vec<usize> {
            (0..self.ids.len())
                .filter(|at| {
                    (self.editor.window_by_id(self.ids[*at]))
                        .is_some_and(|window| window.buffer() == self.transcript)
                })
                .collect()
        }

        fn selected(&self) -> WindowId {
            self.editor.tiled().0.id()
        }

        fn composer(&self) -> WindowId {
            self.editor
                .dock_window(self.transcript, DockSlot::Composer)
                .unwrap()
        }
    }

    #[test]
    fn each_window_shows_its_transcript_at_its_end_and_follows_it() {
        let mut new = new();
        let carried = new.carried(&[0, 2], false);
        new.show(carried, false);
        assert_eq!(new.on_transcript(), [0, 2]);
        assert_eq!(new.selected(), new.ids[2], "the transcript");
        let end = new.editor.buffer(new.transcript).unwrap().text().len();
        new.editor
            .replace_native_rows(new.transcript, end..end, "more\n", &[]);
        for at in [0, 2] {
            let window = new.editor.window_by_id(new.ids[at]).unwrap();
            assert_eq!(window.selections.primary().cursor, end + 5, "window {at}");
        }
    }

    #[test]
    fn the_transcripts_shown_are_among_the_first_in_the_buffers_order() {
        let mut new = new();
        let notes = new.editor.window().buffer();
        let file = new.editor.add_buffer(Buffer::from_text("file", ""));
        let order = vec![notes, file];
        assert_eq!(with_transcripts_shown(&new.editor, order.clone()), order);
        let other = new.editor.add_native_session_buffer("*claude: t*");
        new.editor.show_in(new.ids[0], other);
        assert_eq!(
            with_transcripts_shown(&new.editor, order.clone()),
            [notes, other, file],
            "the selected window's file is first"
        );
        assert_eq!(with_transcripts_shown(&new.editor, Vec::new()), [other]);
        let carried = new.carried(&[1, 2], true);
        new.show(carried, false);
        assert_eq!(new.selected(), new.composer());
        let order = with_transcripts_shown(&new.editor, order);
        assert_eq!(order, [new.transcript, other, notes, file]);
        new.editor.restore_buffer_order(&order);
        let first = new.editor.buffers_by_recency()[..4].to_vec();
        assert_eq!(first, order);
    }

    #[test]
    fn the_composer_is_selected_if_it_was_and_shows() {
        let mut shown = new();
        let carried = shown.carried(&[2], true);
        shown.show(carried, false);
        assert_eq!(shown.selected(), shown.composer());
        // Only in the state's selected window.
        let mut other = new();
        let carried = other.carried(&[0], true);
        other.show(carried, false);
        assert_eq!(other.on_transcript(), [0]);
        assert_eq!(other.selected(), other.ids[2]);
        // With the place holding something else, or no room for it, the transcript is.
        let mut place = new();
        let line = place.editor.add_buffer(Buffer::from_text("*request*", ""));
        place
            .editor
            .set_dock(place.transcript, DockSlot::Composer, Some(line));
        let carried = place.carried(&[2], true);
        place.show(carried, false);
        assert_eq!(place.selected(), place.ids[2]);
        let mut short = new();
        short.editor.set_frame_size(FrameSize {
            rows: 8,
            columns: 120,
        });
        let carried = short.carried(&[2], true);
        short.show(carried, false);
        assert_eq!(short.on_transcript(), [2]);
        assert_eq!(short.selected(), short.ids[2]);
    }

    #[test]
    fn a_session_the_record_does_not_have_leaves_its_windows() {
        let mut new = new();
        let carried = new.carried(&[0, 2], true);
        assert!(!carried.show(&mut new.editor, false, |_| None));
        assert_eq!(new.on_transcript(), [] as [usize; 0]);
        assert_eq!(new.selected(), new.ids[2]);
    }

    #[test]
    fn a_window_the_person_moved_changed_or_lost_stays_theirs() {
        let mut new = new();
        new.editor.select_window(new.ids[0]);
        let gone = new.editor.split_window(Way::Down, false).unwrap();
        new.editor.select_window(new.ids[2]);
        new.ids.push(gone);
        let carried = new.carried(&[0, 1, 2, 3], false);
        new.editor.select_window(gone);
        new.editor.delete_window().unwrap();
        let selections = SelectionSet::new(4);
        new.editor
            .restore_positions_in(new.ids[0], selections, Scroll::default());
        let other = new.editor.add_buffer(Buffer::from_text("other", ""));
        new.editor.show_in(new.ids[1], other);
        new.editor.select_window(new.ids[2]);
        new.show(carried, false);
        assert_eq!(new.on_transcript(), [2]);
    }

    #[test]
    fn the_window_the_person_went_to_or_is_busy_in_stays_theirs() {
        let mut went = new();
        let carried = went.carried(&[0, 2], true);
        went.editor.select_window(went.ids[0]);
        went.show(carried, false);
        assert_eq!(went.on_transcript(), [2], "not under their cursor");
        assert_eq!(went.selected(), went.ids[0], "and the composer is not");
        let mut busy = new();
        let carried = busy.carried(&[0, 2], true);
        busy.show(carried, true);
        assert_eq!(busy.on_transcript(), [0]);
        assert_eq!(busy.selected(), busy.ids[2]);
    }

    #[test]
    fn a_search_or_a_replace_under_way_is_not_ended() {
        let mut new = new();
        let carried = new.carried(&[0], false);
        new.editor.isearch_begin(false, false);
        assert!(new.editor.isearch().is_some());
        new.show(carried, true);
        assert!(new.editor.isearch().is_some(), "in a window beside it too");
        assert_eq!(new.on_transcript(), [] as [usize; 0]);
    }

    #[test]
    fn a_key_since_the_state_was_taken_makes_the_selected_window_the_persons() {
        let new = new();
        let carried = new.carried(&[0, 2], true);
        let taken = carried.taken.unwrap();
        assert!(!carried.used(None));
        assert!(!carried.used(Some(taken)), "a key before it");
        assert!(carried.used(Some(taken + std::time::Duration::from_millis(1))));
        assert!(!Carried::default().used(Some(taken)), "nothing was carried");
    }

    #[test]
    fn the_side_window_stays_selected() {
        let mut new = new();
        let carried = new.carried(&[2], true);
        let list = new.editor.add_internal_buffer("*Spaces*", Mode::Sessions);
        let side = new.editor.show_side_window(list, 32);
        new.editor.select_window(side);
        new.show(carried, false);
        assert_eq!(new.on_transcript(), [2]);
        assert_eq!(new.selected(), side);
    }

    #[test]
    fn a_position_put_back_as_its_file_came_is_not_the_persons() {
        let mut new = new();
        let mut carried = new.carried(&[2], false);
        let selections = SelectionSet::new(4);
        new.editor
            .restore_positions_in(new.ids[2], selections, Scroll::default());
        carried.moved(&new.editor, new.ids[2]);
        new.show(carried, false);
        assert_eq!(new.on_transcript(), [2]);
    }

    #[test]
    fn places_past_the_windows_taken_are_passed_over() {
        let mut new = new();
        // A layout not taken left the state's first window, and the selected one's place is not it.
        let ids = [new.ids[0]];
        let entry = |windows: &[usize]| NativeWindows {
            id: ID.to_owned(),
            windows: windows.to_vec(),
            composer: true,
        };
        let carried = Carried::of(&new.editor, &[entry(&[0, 2])], &ids, 2);
        new.show(carried, false);
        assert_eq!(new.on_transcript(), [0]);
        assert_ne!(new.selected(), new.composer());
    }
}
