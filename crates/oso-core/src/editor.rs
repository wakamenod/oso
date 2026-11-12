//! The editor state that commands operate on.
//!
//! Must not depend on: `command`, `keymap`, or the host's window, threads or
//! I/O.
//!
//! The host owns one [`Editor`] and lends it to each command through
//! [`Ctx`](crate::command::Ctx). It holds the buffers and the windows that
//! show them, each window with its own selection set (ADR 0007), and the tree
//! the tiled ones are laid out by (ADR 0112, `windows`). New state is added
//! here, so commands keep their signature as it grows.
//!
//! Text changes here ([`Editor::apply_edits`], ADR 0053), because a buffer does
//! not know its windows. An edit is made on clones, every window's cursors are
//! worked out against the result, and only then does anything change, in moves
//! that cannot panic. An undo group spans a command: the dispatcher opens one
//! before it runs a command and closes it after, also when the command panics.
//!
//! Saving and the exit guard are in [`save`]: a command starts a save here, and the
//! host, which owns threads and files, takes the job and reports back. What the
//! modeline and the echo area say is in [`status`], and messages in [`messages`]. The
//! buffers that show emergency copies are in [`copies`], prompts in [`prompts`], kills and
//! yanks in [`kill`], isearch in [`isearch`], and files read again after they changed on disk
//! in [`reload`].
//!
//! Internal buffers (ADR 0063) are in the editor but not in [`Editor::buffers`], so
//! whatever goes through a user's work leaves them out without having to know them.

use crate::buffer::{Buffer, BufferId, ContentId, EditError, Replacement};
use crate::display_map::Preedit;
use crate::frame::{self, FrameLayout, FrameSize, Splits};
use crate::minibuffer::{Histories, Outcome, Prompt, PromptLayout};
use crate::project::Project;
use crate::selection::SelectionSet;
use crate::undo::Opening;
use crate::window::{ViewSize, Window, WindowId};
use follow::Selected;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

mod agent;
mod aids;
mod completion;
mod copies;
mod cursors;
mod excerpt;
mod files;
mod follow;
mod gutter;
mod isearch;
mod kill;
mod marks;
mod messages;
mod projects;
mod prompts;
mod reload;
mod replace;
mod save;
mod screen;
mod status;
mod switch;
mod tab;
mod themes;
mod trim;
mod undo_group;
mod view;
mod walks;
mod windows;
mod wraps;
mod write;
mod yank_pop;

pub use crate::search::{SearchJob, SearchReport};
pub use agent::AgentChannel;
pub use aids::{ViewSample, VisualAids};
pub use completion::CompletionStyle;
pub use copies::RecoveryAction;
pub use cursors::{CursorError, Toward};
pub use files::Looked;
pub use gutter::Gutter;
pub use isearch::{Isearch, MatchCells};
pub use kill::{ClipboardWrite, YankError};
pub use messages::row_text;
pub use projects::{ProjectRequest, TrustRefused};
pub use prompts::Quit;
pub use reload::{Leftover, ReloadOutcome};
pub use replace::{Replace, ReplaceAnswer, ReplaceError};
pub use save::{Elsewhere, ExitRefused, Over, SaveJob, SaveRefused};
pub use status::Status;
pub use switch::{KillRefused, Preview};
pub use themes::NoSuchTheme;
#[cfg(test)]
use trim::TRIM_ON_THE_UI_THREAD;
pub use trim::{TrimId, TrimJob, TrimOutcome, TrimReport};
pub use walks::{NotInProject, WalkEnd, WalkRequest};
pub use windows::{CannotResize, NoWindow, SoleWindow};
pub use yank_pop::YankPopError;

/// The name of the window an editor starts with; the next takes 1.
const FIRST_WINDOW: WindowId = WindowId::new(0);

/// The name of the buffer shown when no file is open.
pub const SCRATCH: &str = "*scratch*";

/// The name of the internal buffer that keeps every message.
pub(crate) const MESSAGES: &str = "*Messages*";

/// The undo group of the command running.
struct UndoScope {
    /// The buffer the selected window showed when it began, with the cursors.
    before: (BufferId, SelectionSet),
    /// The buffers this command edited, each with its newest group open.
    touched: Vec<BufferId>,
    /// The command allowed its edits to join the previous command's group.
    join: bool,
    /// Its host kept it a step of its own, whatever the command asks.
    apart: bool,
    /// The command undid or redid something.
    crossed: bool,
}

/// All editor state that commands read and change.
///
/// Invariant: every window shows a buffer that exists.
pub struct Editor {
    buffers: Vec<(BufferId, Buffer)>,
    windows: Vec<Window>,
    selected: usize,
    /// The tiled windows, as a tree (ADR 0112).
    splits: Splits,
    /// The frame's regions. Every window's size is its text area.
    frame: FrameLayout,
    next_id: u64,
    /// The number the next window takes.
    next_window: u64,
    /// The tiled windows in the order they were selected, the most recent first (ADR 0112).
    recent: Vec<WindowId>,
    /// What is left of the command's wrap budget while no window's wrap holds it (ADR 0112).
    wrap_pool: usize,
    /// The tiled windows other than the selected one whose cursors or tops an edit moved in
    /// the command, which follow their cursors when it ends (ADR 0112).
    moved_windows: Vec<WindowId>,
    undo_scope: Option<UndoScope>,
    /// The previous command allowed the next one to join its undo group.
    join_ready: bool,
    /// The internal buffer that keeps every message.
    messages_buffer: BufferId,
    /// While a command runs, whether it has taken a buffer that may hold secrets off the
    /// screen. This holds whatever then becomes of that buffer.
    command_left_secret: Option<bool>,
    /// A file a write to a new place found, which the next `C-x C-w` of it asks to write over
    /// (ADR 0087).
    write_over_asked: Option<write::WriteOver>,
    /// What the prompt that reads a path keeps beside its candidates (ADR 0087).
    file_list: files::FileList,
    /// What the prompt that lists a project's files keeps beside its candidates (ADR 0114).
    project_files: walks::ProjectFiles,
    /// What a prompt that completes waits for from the worker (ADR 0096).
    completion_work: completion::CompletionWork,
    /// Files commands asked to visit, with the most bytes each may read, for the host.
    visits: Vec<(std::path::PathBuf, u64)>,
    /// A file a load stopped before reading as too large, with its size, which `C-x C-f` asks
    /// about next (ADR 0087).
    large: Option<(std::path::PathBuf, u64)>,
    /// The directory oso started in.
    start_directory: std::path::PathBuf,
    /// The marks the last `C-.` listed (ADR 0088).
    mark_list: Option<marks::MarkList>,
    /// The ring's entries a `M-y` list shows (ADR 0091).
    kill_list: Option<yank_pop::KillList>,
    /// The home directory, which `~` stands for, as the host read it at startup.
    home: Option<std::path::PathBuf>,
    /// The buffer a question about killing it with unsaved text was asked of (ADR 0088).
    kill_asked: Option<BufferId>,
    /// What killed buffers held, for the host to free off the UI thread.
    freed: Vec<Buffer>,
    /// The internal buffer that lists the emergency copies, once it has been asked for.
    copies_buffer: Option<BufferId>,
    /// What a person asked of the emergency copies, for the host.
    copy_actions: Vec<RecoveryAction>,
    /// The echo area's line, until the next key.
    echo: Option<messages::Echo>,
    /// A prefix argument and a prefix key being typed, for the echo area. The dispatcher
    /// holds them, and says so here after each key, so that a frame reads only the editor.
    keys_in_progress: String,
    /// The keys in progress were typed, or stayed, while a buffer that may hold secrets was
    /// shown, so the control socket leaves them out until they end.
    keys_withheld: bool,
    /// The composition was open while such a buffer was shown, so the control socket leaves it
    /// out until it ends.
    preedit_withheld: bool,
    /// Saves the host has yet to start.
    save_jobs: Vec<SaveJob>,
    /// The buffers the last refused exit put at risk, until a question about it is answered
    /// or a save changes what is at risk.
    exit_attempt: Option<Vec<BufferId>>,
    /// What an input method is composing, shown at the selected window's primary
    /// cursor and in no buffer (ADR 0069).
    preedit: Option<Preedit>,
    /// The open prompts, the outermost first. Their windows come after the tiled ones,
    /// and the innermost's is selected (ADR 0072).
    prompts: Vec<Prompt>,
    /// The answers given to prompts, in memory only.
    histories: Histories,
    /// How the last prompt to end ended, until the dispatcher takes it.
    outcome: Option<Outcome>,
    /// Where the innermost prompt's parts go, worked out when it changes.
    prompt_layout: Option<PromptLayout>,
    /// The preview's rows of the buffer it shows, while that buffer wraps (ADR 0100).
    preview_wrap: Option<wraps::PreviewWrap>,
    /// The number of the next prompt's id.
    next_prompt: u64,
    /// The command running got a passphrase, so what it says stays out of the log.
    passphrase_command: bool,
    /// The buffer a question about saving over its changed file was asked of, so that `y`
    /// saves that buffer and no other.
    save_over_asked: Option<BufferId>,
    /// The kill ring, and whether the command before killed (ADR 0074).
    kills: kill::Kills,
    /// What the editor knows of the agent channel (ADR 0085).
    agent: agent::Agent,
    /// The isearch under way in the selected window (ADR 0082).
    isearch: Option<Isearch>,
    /// The generation of the search job that may run, which a worker checks to stop.
    search_live: Arc<AtomicU64>,
    /// A job for a worker, until the host takes it.
    search_job: Option<SearchJob>,
    /// The query a window keeps for the keys that add cursors (ADR 0091).
    marking: Option<cursors::Marking>,
    /// Buffers a person asked to read again from their files, for the host (ADR 0083).
    reverts: Vec<(BufferId, ContentId)>,
    /// The projects buffers are in, which `ProjectId` indexes (ADR 0114).
    projects: Vec<Project>,
    /// Files whose project the host should find.
    project_requests: Vec<ProjectRequest>,
    /// The record of trusted projects, and what of it waits to be written.
    trust: projects::Trust,
    themes: themes::Themes,
    aids: VisualAids,
    /// Whether buffers are highlighted (`global-font-lock-mode`, ADR 0111).
    font_lock: bool,
    /// Whitespace cleanup: the setting, and the scans under way (ADR 0113).
    trims: trim::Trims,
    /// query-replace's session, and the last pairs given (ADR 0113).
    replacing: replace::Replacing,
}

impl Default for Editor {
    /// An editor showing an empty `*scratch*` buffer.
    fn default() -> Self {
        Self::with_buffer(Buffer::new(SCRATCH))
    }
}

impl Editor {
    /// An editor whose one window shows `buffer`.
    pub fn with_buffer(buffer: Buffer) -> Self {
        let mut editor = Self {
            buffers: Vec::new(),
            windows: Vec::new(),
            selected: 0,
            splits: Splits::new(FIRST_WINDOW),
            frame: frame::layout(
                FrameSize::holding(ViewSize::default()),
                &Splits::new(FIRST_WINDOW),
                FIRST_WINDOW,
                |_| 0,
            ),
            next_id: 0,
            next_window: 1,
            recent: vec![FIRST_WINDOW],
            wrap_pool: crate::display_map::wrap_map::COMMAND_BUDGET,
            moved_windows: Vec::new(),
            undo_scope: None,
            join_ready: false,
            messages_buffer: BufferId(0),
            command_left_secret: None,
            home: None,
            mark_list: None,
            kill_list: None,
            file_list: files::FileList::default(),
            project_files: walks::ProjectFiles::default(),
            completion_work: completion::CompletionWork::default(),
            visits: Vec::new(),
            large: None,
            start_directory: std::path::PathBuf::from("/"),
            write_over_asked: None,
            kill_asked: None,
            freed: Vec::new(),
            copies_buffer: None,
            copy_actions: Vec::new(),
            echo: None,
            keys_in_progress: String::new(),
            keys_withheld: false,
            preedit_withheld: false,
            save_jobs: Vec::new(),
            exit_attempt: None,
            preedit: None,
            prompts: Vec::new(),
            histories: Histories::default(),
            outcome: None,
            prompt_layout: None,
            preview_wrap: None,
            next_prompt: 0,
            passphrase_command: false,
            save_over_asked: None,
            kills: kill::Kills::default(),
            agent: agent::Agent::default(),
            isearch: None,
            search_live: Arc::new(AtomicU64::new(0)),
            search_job: None,
            marking: None,
            reverts: Vec::new(),
            projects: Vec::new(),
            project_requests: Vec::new(),
            trust: projects::Trust::default(),
            themes: themes::Themes::default(),
            aids: VisualAids::default(),
            font_lock: true,
            trims: trim::Trims::default(),
            replacing: replace::Replacing::default(),
        };
        let id = editor.add_buffer(buffer);
        editor.windows.push(Window::new(FIRST_WINDOW, id));
        editor.messages_buffer = editor.add_buffer(Buffer::internal(MESSAGES));
        editor
    }

    /// Adds a buffer, shown in no window yet.
    pub fn add_buffer(&mut self, buffer: Buffer) -> BufferId {
        let id = BufferId(self.next_id);
        self.next_id += 1;
        let name = buffer
            .path()
            .map_or_else(|| buffer.name().to_owned(), crate::buffer::file_base);
        self.buffers.push((id, buffer));
        self.name_files_like(&name);
        id
    }

    /// The buffer with `id`, if it exists.
    pub fn buffer(&self, id: BufferId) -> Option<&Buffer> {
        self.buffers
            .iter()
            .find_map(|(other, buffer)| (*other == id).then_some(buffer))
    }

    /// The buffer with `id`, if it exists, to change.
    pub fn buffer_mut(&mut self, id: BufferId) -> Option<&mut Buffer> {
        self.buffers
            .iter_mut()
            .find_map(|(other, buffer)| (*other == id).then_some(buffer))
    }

    /// Every buffer of the user's, in the order they were added. This leaves out
    /// internal buffers, which hold no work to save, hand over or count at an exit.
    pub fn buffers(&self) -> impl Iterator<Item = (BufferId, &Buffer)> {
        self.buffers
            .iter()
            .filter(|(_, buffer)| !buffer.is_internal())
            .map(|(id, buffer)| (*id, buffer))
    }

    /// Shows `buffer` in the selected window, where the window was in it the last time, or from
    /// its start, or in the window beneath an open prompt, whose own window always shows its own
    /// buffer (ADR 0072). Does nothing for an id that does not exist.
    pub fn show(&mut self, buffer: BufferId) {
        if self.buffer(buffer).is_some() {
            self.window_moves_on();
            let index = self.tiled_index();
            self.window_shows(index, buffer);
        }
    }

    /// Every window.
    pub fn windows(&self) -> impl ExactSizeIterator<Item = &Window> {
        self.windows.iter()
    }

    /// The window named `id`, if there is one.
    pub fn window_by_id(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|window| window.id() == id)
    }

    /// A new window showing `buffer`, with a name no window had before.
    pub(super) fn new_window(&mut self, buffer: BufferId) -> Window {
        let id = WindowId::new(self.next_window);
        self.next_window += 1;
        Window::new(id, buffer)
    }

    /// The position of the selected window in [`Editor::windows`].
    pub fn selected_window(&self) -> usize {
        self.selected
    }

    /// Whether any window shows a buffer that may hold secrets (ADR 0044).
    pub fn shows_secret(&self) -> bool {
        self.windows
            .iter()
            .any(|window| self.shown(window).is_secret())
    }

    /// The selected window.
    pub fn window(&self) -> &Window {
        &self.windows[self.selected]
    }

    /// The selected window and the buffer it shows.
    pub fn current(&self) -> (&Window, &Buffer) {
        let window = &self.windows[self.selected];
        (window, self.shown(window))
    }

    /// The selected window, to change, and the buffer it shows.
    pub fn current_mut(&mut self) -> (&mut Window, &Buffer) {
        self.window_mut(self.selected)
    }

    /// The window at `index`, to change, and the buffer it shows.
    #[expect(
        clippy::expect_used,
        reason = "every window shows a buffer that exists: kill_buffer moves windows off a buffer before it goes"
    )]
    fn window_mut(&mut self, index: usize) -> (&mut Window, &Buffer) {
        let window = &mut self.windows[index];
        let id = window.buffer();
        let buffer = self
            .buffers
            .iter()
            .find_map(|(other, buffer)| (*other == id).then_some(buffer))
            .expect("every window shows a buffer that exists");
        (window, buffer)
    }

    /// The buffer `window` shows.
    #[expect(
        clippy::expect_used,
        reason = "every window shows a buffer that exists: kill_buffer moves windows off a buffer before it goes"
    )]
    pub fn shown(&self, window: &Window) -> &Buffer {
        self.buffer(window.buffer())
            .expect("every window shows a buffer that exists")
    }

    /// Applies `edits` to the buffer `id` as one step, and moves the cursors of
    /// every window showing it. All of it happens, or none of it does.
    ///
    /// `edits` are usually one per cursor, in any order. They are applied in
    /// descending offset order, so all their ranges are offsets in the text as
    /// it is now. Ranges that overlap, and insertions at one offset, become
    /// their union, replaced by their texts joined in offset order; ranges that
    /// only touch stay apart. Empty edits do nothing.
    ///
    /// In the selected window, a cursor at the start of an edit or inside its
    /// replaced text ends after the new text, so a command that inserts at each
    /// cursor need not move them, though it may move them afterward. In other
    /// windows such a cursor stays before the new text, as an Emacs marker.
    /// Cursors are then clipped to a display cluster, since a combining mark can
    /// move the boundaries: to its start, or to its end for a selected cursor
    /// just after new text that joined what follows, so typed text stays before
    /// the cursor. Cursors that met merge.
    ///
    /// # Errors
    ///
    /// [`EditError::ReadOnly`] for a read-only buffer, and
    /// [`EditError::NoSuchBuffer`]. Nothing changes.
    pub fn apply_edits(
        &mut self,
        id: BufferId,
        edits: &[Replacement<'_>],
    ) -> Result<(), EditError> {
        let buffer = self.buffer(id).ok_or(EditError::NoSuchBuffer)?;
        let Some(staged) = buffer.stage_edits(edits)? else {
            return Ok(());
        };
        let opening = self.opening(id, buffer);
        let moved = self.moved_selections(
            id,
            &staged.display(),
            staged.steps(),
            staged.changed(),
            Selected::Edited,
        );
        let after = moved
            .iter()
            .find_map(|(index, set)| (*index == self.selected).then(|| set.clone()));
        let views = self.moved_views(id, &staged.display(), staged.steps(), staged.rewrap());

        // Nothing past this point can panic part way.
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.commit_edit(staged, opening, after);
        }
        for (index, set) in moved {
            self.windows[index].selections = set;
        }
        self.commit_views(id, views);
        self.note_edited(id);
        // The line count may have another digit (ADR 0097).
        self.fit_gutter();
        Ok(())
    }

    /// `id` has been edited: the open undo group touches it, or with none, nothing joins.
    fn note_edited(&mut self, id: BufferId) {
        match &mut self.undo_scope {
            Some(scope) if !scope.touched.contains(&id) => scope.touched.push(id),
            Some(_) => {}
            None => self.join_ready = false,
        }
    }

    /// How an edit of `id` relates to its newest undo group.
    fn opening(&self, id: BufferId, buffer: &Buffer) -> Opening {
        let Some(scope) = &self.undo_scope else {
            return Opening::New(self.selected_selections(id).cloned());
        };
        if scope.touched.contains(&id) {
            return Opening::Continue;
        }
        let before = if scope.before.0 == id {
            Some(&scope.before.1)
        } else {
            self.selected_selections(id)
        };
        if scope.join && self.join_ready && buffer.can_join_undo(before) {
            Opening::Join
        } else {
            Opening::New(before.cloned())
        }
    }

    /// The selected window's cursors, if it shows `id`.
    fn selected_selections(&self, id: BufferId) -> Option<&SelectionSet> {
        let window = &self.windows[self.selected];
        (window.buffer() == id).then_some(&window.selections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_map::TabWidth;
    use crate::frame::{PixelGrid, PixelRect};
    use crate::load::Builder;
    use crate::selection::Selection;
    use crate::window::Scroll;
    use proptest::prelude::*;
    use std::path::PathBuf;

    fn lines(n: usize) -> String {
        (0..n).map(|i| format!("line {i:03}\n")).collect()
    }

    /// An editor on `n` lines, plus the empty last row after them, with the cursor
    /// on `row`, in a window of `rows` rows.
    fn editor_on_row(n: usize, row: usize, rows: usize) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(n)));
        editor.set_view_size(ViewSize::new(rows, 40));
        editor.current_mut().0.selections = SelectionSet::new(row * "line 000\n".len());
        editor.follow_cursor();
        editor
    }

    /// Resizes to `rows` rows. Returns whether the view moved and the top row.
    fn resize(editor: &mut Editor, rows: usize) -> (bool, usize) {
        let moved = editor.set_view_size(ViewSize::new(rows, 40));
        (moved, editor.top_row())
    }

    #[test]
    fn growing_at_the_end_shows_the_text_again() {
        let mut editor = editor_on_row(100, 100, 10);
        assert_eq!(editor.top_row(), 91);
        let cursor = editor.window().selections.primary().cursor;
        assert_eq!(resize(&mut editor, 1), (true, 100), "only the cursor's row");
        assert_eq!(resize(&mut editor, 10), (true, 91), "the screen it had");
        assert_eq!(resize(&mut editor, 25), (true, 76));
        assert_eq!(resize(&mut editor, 200), (true, 0), "the whole buffer");
        assert_eq!(resize(&mut editor, 300), (false, 0), "nothing above it");
        assert_eq!(editor.window().selections.primary().cursor, cursor);
    }

    #[test]
    fn growing_a_row_at_a_time_keeps_the_last_row_at_the_bottom() {
        let mut editor = editor_on_row(100, 100, 1);
        for rows in 2..=10 {
            assert_eq!(resize(&mut editor, rows), (true, 101 - rows));
        }
    }

    #[test]
    fn growing_in_the_middle_moves_nothing_while_text_fills_the_window() {
        let mut editor = editor_on_row(100, 50, 10);
        assert_eq!(resize(&mut editor, 1), (true, 50));
        assert_eq!(resize(&mut editor, 10), (false, 50));
        assert_eq!(
            resize(&mut editor, 51),
            (false, 50),
            "the last row just fits"
        );
        assert_eq!(resize(&mut editor, 52), (true, 49), "one empty row");
    }

    #[test]
    fn growing_near_the_end_fills_only_the_empty_rows() {
        let mut editor = editor_on_row(100, 95, 3);
        assert_eq!(editor.top_row(), 93);
        assert_eq!(resize(&mut editor, 8), (false, 93), "rows 93 to 100");
        assert_eq!(resize(&mut editor, 10), (true, 91));

        let mut editor = editor_on_row(100, 95, 3);
        assert_eq!(resize(&mut editor, 10), (true, 91), "2 of the 7 rows added");
    }

    #[test]
    fn growing_at_the_top_moves_nothing() {
        let mut editor = editor_on_row(100, 0, 10);
        assert_eq!(resize(&mut editor, 1), (false, 0));
        assert_eq!(resize(&mut editor, 200), (false, 0));
    }

    #[test]
    fn a_buffer_shorter_than_the_window_comes_back_from_its_top() {
        let mut editor = editor_on_row(3, 3, 10);
        assert_eq!(editor.top_row(), 0);
        assert_eq!(resize(&mut editor, 1), (true, 3));
        assert_eq!(resize(&mut editor, 2), (true, 2));
        assert_eq!(resize(&mut editor, 10), (true, 0));
    }

    #[test]
    fn a_view_past_the_end_stays_until_the_window_gets_taller() {
        let mut editor = editor_on_row(100, 0, 10);
        let display = editor.current().1.display();
        editor.current_mut().0.set_top(98, &display);
        assert_eq!(resize(&mut editor, 10), (false, 98), "the same size");
        let wider = editor.set_view_size(ViewSize::new(10, 80));
        assert!(!wider, "more columns are not more rows");
        assert_eq!(resize(&mut editor, 3), (false, 98), "shorter");
        assert_eq!(resize(&mut editor, 3), (false, 98));
        assert_eq!(resize(&mut editor, 4), (true, 97), "taller");
    }

    #[test]
    fn a_view_past_the_end_moves_by_the_rows_added() {
        let mut editor = editor_on_row(100, 0, 10);
        let display = editor.current().1.display();
        editor.current_mut().0.set_top(98, &display);
        assert_eq!(resize(&mut editor, 11), (true, 97), "a row for a row");
        assert_eq!(resize(&mut editor, 12), (true, 96));
        assert_eq!(resize(&mut editor, 20), (true, 88));
        assert_eq!(resize(&mut editor, 200), (true, 0), "all that remains");
    }

    #[test]
    fn growing_fills_with_what_a_loading_buffer_has_so_far() {
        let mut builder = Builder::new(None, TabWidth::default());
        builder.push(lines(20).as_bytes());
        let mut editor = Editor::default();
        editor.set_view_size(ViewSize::new(10, 40));
        let file = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/b")));
        editor.show(file);
        assert_eq!(resize(&mut editor, 1), (false, 0), "nothing has loaded");
        assert_eq!(resize(&mut editor, 10), (false, 0));

        let buffer = editor.buffer_mut(file).unwrap();
        buffer.apply_load(builder.snapshot());
        assert!(buffer.loading().is_some());
        editor.current_mut().0.selections = SelectionSet::new(20 * "line 000\n".len());
        assert_eq!(resize(&mut editor, 1), (true, 20));
        assert_eq!(resize(&mut editor, 10), (true, 11));

        builder.push(lines(20).as_bytes());
        builder.finish();
        editor
            .buffer_mut(file)
            .unwrap()
            .apply_load(builder.snapshot());
        assert_eq!(resize(&mut editor, 12), (false, 11), "more text below now");
    }

    #[test]
    fn growing_leaves_other_cursors_where_they_are() {
        let mut editor = editor_on_row(100, 100, 1);
        let row = "line 000\n".len();
        let cursors = vec![Selection::at(0), Selection::at(100 * row)];
        let selections = SelectionSet::from_parts(cursors, 1).unwrap();
        editor.current_mut().0.selections = selections.clone();
        assert_eq!(resize(&mut editor, 10), (true, 91));
        assert_eq!(editor.window().selections, selections);
    }

    proptest! {
        #[test]
        fn prop_a_resize_shows_the_cursor_and_growing_leaves_no_empty_rows(
            n in 0usize..60,
            row in 0usize..61,
            sizes in proptest::collection::vec(1usize..80, 1..12),
        ) {
            let mut editor = editor_on_row(n, row.min(n), 10);
            let mut before = 10;
            for rows in sizes {
                let old = editor.top_row();
                let (moved, top) = resize(&mut editor, rows);
                prop_assert_eq!(moved, top != old);
                prop_assert!(top <= row.min(n) && row.min(n) < top + rows);
                if rows > before {
                    prop_assert!(top == 0 || top + rows <= n + 1, "empty rows below");
                }
                if rows == before {
                    prop_assert_eq!(top, old);
                }
                before = rows;
            }
        }
    }

    #[test]
    fn restoring_positions_follows_the_cursor() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(100)));
        editor.set_view_size(ViewSize::new(10, 20));
        let saved = Scroll {
            start: 40 * "line 000\n".len(),
            left: 1,
        };
        editor.restore_positions(SelectionSet::new(45 * "line 000\n".len() + 1), saved);
        assert_eq!(editor.window().scroll(), saved, "the cursor is in view");
        editor.restore_positions(SelectionSet::new(0), saved);
        assert_eq!(editor.window().scroll(), Scroll::default(), "back to it");
        assert_eq!(editor.window().selections.primary().cursor, 0);
    }

    #[test]
    fn the_sideways_wheel_goes_as_far_as_the_rows_in_view_need_and_leaves_the_cursors() {
        let long = "x".repeat(50);
        let text = format!("ab\n{long}\n{}\nab\n", "y".repeat(100));
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &text));
        editor.set_view_size(ViewSize::new(2, 20));
        assert!(editor.scroll_view_sideways(editor.tiled().0.id(), 6));
        assert_eq!(editor.window().scroll().left, 6);
        assert!(editor.scroll_view_sideways(editor.tiled().0.id(), 1000));
        assert_eq!(editor.window().scroll().left, 31, "the widest row in view");
        assert!(!editor.scroll_view_sideways(editor.tiled().0.id(), 1));
        assert_eq!(editor.window().selections.primary().cursor, 0);
        assert!(editor.follow_cursor(), "the next command brings it back");
        assert_eq!(editor.window().scroll().left, 0);
    }

    #[test]
    fn a_frame_gives_the_windows_its_text_area() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(100)));
        assert_eq!(editor.window().size(), ViewSize::default());
        assert_eq!(editor.tiled_regions().text.view_size(), ViewSize::default());

        let frame = FrameSize {
            rows: 12,
            columns: 40,
        };
        editor.set_frame_size(frame);
        assert_eq!(*editor.frame(), frame::single(frame, 0));
        assert_eq!(editor.window().size(), ViewSize::new(10, 40));
    }

    #[test]
    fn a_smaller_frame_scrolls_to_keep_the_cursor_above_the_reserved_rows() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(100)));
        editor.set_frame_size(FrameSize {
            rows: 12,
            columns: 40,
        });
        editor.current_mut().0.selections = SelectionSet::new(9 * "line 000\n".len());
        assert!(!editor.follow_cursor(), "the last row of the text area");
        assert!(editor.set_frame_size(FrameSize {
            rows: 11,
            columns: 40,
        }));
        assert_eq!(editor.top_row(), 1);
    }

    #[test]
    fn a_frame_too_small_for_a_row_still_has_a_view_to_scroll() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(100)));
        editor.set_frame_size(FrameSize {
            rows: 0,
            columns: 0,
        });
        assert_eq!(editor.window().size(), ViewSize::new(1, 1));
        assert!(editor.scroll_view(editor.tiled().0.id(), 3));
        let grid = PixelGrid::new(10.0, 20.0);
        assert_eq!(editor.cursor_area(grid), None, "nothing is shown");
    }

    #[test]
    fn the_cursor_area_is_the_cursors_cells_in_pixels() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", "ab\n日本\n"));
        editor.set_view_size(ViewSize::new(10, 20));
        let grid = PixelGrid::new(10.0, 20.0);
        let area = |x, y, width| PixelRect {
            x,
            y,
            width,
            height: 20.0,
        };
        assert_eq!(editor.cursor_area(grid), Some(area(8.0, 8.0, 10.0)));
        editor.current_mut().0.selections = SelectionSet::new("ab\n".len());
        assert_eq!(
            editor.cursor_area(grid),
            Some(area(8.0, 28.0, 20.0)),
            "wide"
        );
        editor.current_mut().0.selections = SelectionSet::new("ab\n日".len());
        assert_eq!(editor.cursor_area(grid), Some(area(28.0, 28.0, 20.0)));
    }

    #[test]
    fn the_cursor_area_moves_with_the_view() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(100)));
        editor.set_view_size(ViewSize::new(10, 20));
        editor.current_mut().0.selections = SelectionSet::new(5 * "line 000\n".len());
        editor.follow_cursor();
        let grid = PixelGrid::new(10.0, 20.0);
        let before = editor.cursor_area(grid).map(|area| area.y);
        assert_eq!(before, Some(8.0 + 5.0 * 20.0));
        assert!(editor.scroll_view(editor.tiled().0.id(), 2));
        let after = editor.cursor_area(grid).map(|area| area.y);
        assert_eq!(after, Some(8.0 + 3.0 * 20.0));
    }

    #[test]
    fn the_composition_is_placed_where_it_is_drawn_after_a_sideways_scroll() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &"a".repeat(18)));
        editor.set_view_size(ViewSize::new(10, 20));
        editor.current_mut().0.selections = SelectionSet::new(18);
        editor.follow_cursor();
        let grid = PixelGrid::new(10.0, 20.0);
        let before = editor.cursor_area(grid).unwrap();
        let mut positions = vec![1.0];
        assert_eq!(
            editor.preedit_layout(grid, &mut positions),
            None,
            "none composed"
        );
        assert!(positions.is_empty());
        // Too wide to show beside the cursor box: the window scrolls sideways (ADR 0069).
        editor.set_preedit(Some(Preedit::new("かかかかか".into(), None)));
        let first = editor.preedit_layout(grid, &mut positions).unwrap();
        let left = editor.window().scroll().left;
        assert!(left > 0, "scrolled sideways");
        assert_eq!(
            first.x,
            before.x - left as f32 * 10.0,
            "moved with the text"
        );
        assert_eq!((first.y, first.height), (before.y, before.height));
        let text = grid.rect(editor.tiled_regions().text);
        assert_eq!(positions.len(), 6, "each character and the end");
        for position in &positions {
            let x = first.x + position;
            assert!(
                text.x <= x && x <= text.x + text.width,
                "inside the text area"
            );
        }
        assert!(positions.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn a_composition_whose_row_is_out_of_view_gets_no_place() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(100)));
        editor.set_view_size(ViewSize::new(10, 20));
        editor.set_preedit(Some(Preedit::new("か".into(), None)));
        assert!(editor.scroll_view(editor.tiled().0.id(), 30));
        let mut positions = vec![1.0];
        let grid = PixelGrid::new(10.0, 20.0);
        assert_eq!(editor.preedit_layout(grid, &mut positions), None);
        assert!(positions.is_empty());
    }

    #[test]
    fn the_wheel_leaves_the_cursor_where_a_composition_is() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(100)));
        editor.set_view_size(ViewSize::new(10, 20));
        editor.set_preedit(Some(Preedit::new("か".into(), None)));
        assert!(editor.scroll_view(editor.tiled().0.id(), 30));
        assert_eq!(editor.top_row(), 30);
        assert_eq!(
            editor.window().selections.primary().cursor,
            0,
            "the cursor stays"
        );
        editor.set_preedit(None);
        assert_eq!(editor.top_row(), 0, "the view comes back to it");
        assert!(editor.scroll_view(editor.tiled().0.id(), 30));
        assert_ne!(
            editor.window().selections.primary().cursor,
            0,
            "with none, it moves"
        );
    }

    #[test]
    fn the_view_follows_the_preedit_to_the_cursor_box() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", "abcdefgh"));
        editor.set_view_size(ViewSize::new(3, 10));
        editor.current_mut().0.selections = SelectionSet::new(6);
        editor.follow_cursor();
        assert_eq!(editor.window().scroll().left, 0);
        // At column 6, four cells of preedit and the box on `g` need 11 columns.
        let preedit = Preedit::new("かな".into(), None);
        assert!(editor.set_preedit(Some(preedit)));
        assert_eq!(
            editor.window().scroll().left,
            1,
            "from 6 to the box's end at 11"
        );
        // Wider than the window, it shows the box.
        let long = Preedit::new("かなかなかなか".into(), None);
        assert!(editor.set_preedit(Some(long)));
        assert_eq!(editor.window().scroll().left, 11);
        assert!(editor.preedit().is_some());
        editor.set_preedit(Some(Preedit::new(String::new(), None)));
        assert!(editor.preedit().is_none(), "no text is no preedit");
    }

    #[test]
    fn starts_with_scratch() {
        let editor = Editor::default();
        let (window, buffer) = editor.current();
        assert_eq!(buffer.name(), SCRATCH);
        assert_eq!(window.selections.primary().cursor, 0);
    }

    #[test]
    fn killing_a_shown_buffer_shows_scratch() {
        let mut editor = Editor::default();
        let file = editor.add_buffer(Buffer::visiting(PathBuf::from("/a")));
        editor.show(file);
        assert_eq!(editor.current().1.name(), "a");
        editor.kill_buffer(file);
        assert_eq!(editor.current().1.name(), SCRATCH);
        assert_eq!(editor.buffers().count(), 1);
        // Killing scratch itself makes a new one.
        let (scratch, _) = editor.buffers().next().unwrap();
        editor.kill_buffer(scratch);
        assert_eq!(editor.current().1.name(), SCRATCH);
    }

    #[test]
    fn a_file_named_like_scratch_is_not_scratch() {
        let mut editor = Editor::default();
        let (scratch, _) = editor.buffers().next().unwrap();
        let other = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/b")));
        editor.show(other);
        editor.kill_buffer(scratch);
        let file = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/*scratch*")));
        editor.kill_buffer(other);
        assert_ne!(editor.current().0.buffer(), file);
        assert_eq!(editor.current().1.path(), None);
    }

    #[test]
    fn buffers_can_be_changed_by_id() {
        let mut editor = Editor::default();
        let file = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/b")));
        editor.buffer_mut(file).unwrap().finish_empty();
        assert_eq!(editor.buffer(file).unwrap().loading(), None);
        assert_eq!(editor.buffer(file).unwrap().name(), "b");
        assert!(editor.buffer_mut(BufferId(u64::MAX)).is_none());
        assert_eq!(editor.selected_window(), 0);
    }

    #[test]
    fn shows_secret_follows_what_the_windows_show() {
        let mut editor = Editor::default();
        assert!(!editor.shows_secret());
        let file = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/.env")));
        assert!(!editor.shows_secret(), "not shown yet");
        editor.show(file);
        assert!(editor.shows_secret());
        let (scratch, _) = editor.buffers().next().unwrap();
        editor.show(scratch);
        assert!(!editor.shows_secret(), "switched away");
    }

    #[test]
    fn the_view_follows_the_cursor_and_the_wheel_moves_the_cursor() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(100)));
        editor.set_view_size(ViewSize::new(10, 20));
        editor.windows[0].selections = crate::selection::SelectionSet::new(15 * 9);
        assert!(editor.follow_cursor());
        assert_eq!(editor.top_row(), 6);
        assert!(!editor.follow_cursor());

        assert!(editor.scroll_view(editor.tiled().0.id(), 20));
        assert_eq!(editor.top_row(), 26);
        let cursor = editor.window().selections.primary().cursor;
        assert_eq!(cursor, 26 * 9, "moved to the start of the top row");
        assert!(editor.scroll_view(editor.tiled().0.id(), -3));
        assert_eq!(editor.window().selections.primary().cursor, 26 * 9);
        assert!(editor.scroll_view(editor.tiled().0.id(), -100));
        assert_eq!(editor.top_row(), 0);
        assert!(!editor.scroll_view(editor.tiled().0.id(), -1));
    }
}

#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod highlight_tests;
#[cfg(test)]
mod isearch_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod prompt_tests;
#[cfg(test)]
mod reload_tests;
#[cfg(test)]
mod save_tests;
#[cfg(test)]
mod test_buffers;
#[cfg(test)]
mod trim_tests;
#[cfg(test)]
mod undo_tests;
#[cfg(test)]
mod wrap_tests;
