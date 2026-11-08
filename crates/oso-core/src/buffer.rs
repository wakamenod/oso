//! Buffers: the text of a file, with its display model (ADR 0022,
//! ADR 0040).
//!
//! Must not depend on: `window`, `editor` or `command`, or file I/O; a buffer
//! does not know who shows it or who reads the file.
//!
//! A file buffer starts empty and read-only, and fills in as the loading
//! thread hands over snapshots ([`Buffer::apply_load`]). Buffers with
//! invalid UTF-8, and binary files, stay read-only and can never be saved.
//!
//! Edits are atomic (ADR 0053). They are made on clones of the rope and the
//! display map, both O(1) to clone, and committed by moves that cannot panic.
//! A panic in a command leaves a buffer before the edit or after it. The editor
//! maps cursors between the two steps, since a buffer does not know its windows.

use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::slice;

use crate::decode::{FileInfo, InvalidBytes};
use crate::display_map::{DisplayMap, DisplaySnapshot, TabWidth};
use crate::load::{self, LoadSnapshot, Progress};
use crate::rope::Rope;
use crate::secret;
use crate::selection::{OffsetEdit, SelectionSet};
pub use crate::undo::ContentId;
use crate::undo::{Batch, Direction, Group, History, Opening, Side, Splice};

mod baseline;
mod internal;
mod mode;
mod names;
mod naming;
mod recovery;
mod splice;

pub use crate::secret::Verdict;
pub use baseline::{Baseline, FileStamp};
pub use mode::Mode;
pub(crate) use names::distinct as distinct_names;
use splice::{SpliceText, apply, normalized, offset_edit};

/// Identifies a buffer in its [`Editor`](crate::editor::Editor).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BufferId(pub(crate) u64);

impl BufferId {
    /// The number behind the id, for showing it outside the editor.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Why a buffer cannot be edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReadOnly {
    /// The file is still loading.
    Loading,
    /// The file has invalid UTF-8, shown as `\xNN`. It can never be saved.
    InvalidUtf8,
    /// The file is binary (a NUL byte in its first 8 KB). It can never be
    /// saved.
    Binary,
    /// The editor keeps this buffer for itself, and only the editor changes it.
    Internal,
}

impl ReadOnly {
    /// `"loading"`, `"invalid-utf8"`, `"binary"` or `"internal"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::InvalidUtf8 => "invalid-utf8",
            Self::Binary => "binary",
            Self::Internal => "internal",
        }
    }
}

/// One replacement: the bytes in `range` become `text`.
///
/// `range` is clamped as in [`Rope::replace`]: to the text, to character
/// starts, and a reversed range becomes empty at its start. It has no `Debug`,
/// so that its text cannot reach a log.
#[derive(Clone)]
pub struct Replacement<'a> {
    pub range: Range<usize>,
    pub text: &'a str,
}

/// Why an edit, an undo or a redo did nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditError {
    /// The buffer cannot be edited, for this reason.
    ReadOnly(ReadOnly),
    /// No buffer has the id.
    NoSuchBuffer,
    /// The history has nothing more to undo.
    NoUndo,
    /// The history has nothing more to redo.
    NoRedo,
}

impl fmt::Display for EditError {
    /// The message Emacs shows, which never quotes the text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ReadOnly(_) => "Buffer is read-only",
            Self::NoSuchBuffer => "No such buffer",
            Self::NoUndo => "No further undo information",
            Self::NoRedo => "No further redo information",
        })
    }
}

impl std::error::Error for EditError {}

/// An edit made on clones, not yet in the buffer. Dropping it changes nothing.
pub(crate) struct StagedEdit {
    text: Rope,
    display: DisplayMap,
    edits: Vec<OffsetEdit>,
    undo: Batch,
}

impl StagedEdit {
    pub(crate) fn display(&self) -> DisplaySnapshot {
        self.display.snapshot()
    }

    /// What cursors follow, batch by batch.
    pub(crate) fn steps(&self) -> &[Vec<OffsetEdit>] {
        slice::from_ref(&self.edits)
    }
}

/// An undo or a redo made on clones, not yet in the buffer.
pub(crate) struct StagedCrossing {
    text: Rope,
    display: DisplayMap,
    steps: Vec<Vec<OffsetEdit>>,
    direction: Direction,
    crossed: Group,
}

impl StagedCrossing {
    pub(crate) fn display(&self) -> DisplaySnapshot {
        self.display.snapshot()
    }

    /// What cursors follow, batch by batch.
    pub(crate) fn steps(&self) -> &[Vec<OffsetEdit>] {
        &self.steps
    }

    /// The cursors of the state this returns to, if the group has them.
    pub(crate) fn selections(&self) -> Option<&SelectionSet> {
        self.crossed.source.selections.as_ref()
    }
}

/// A buffer: text, its display model, and where it came from.
pub struct Buffer {
    name: String,
    path: Option<PathBuf>,
    text: Rope,
    /// Also holds the invalid bytes (ADR 0040).
    display: DisplayMap,
    info: FileInfo,
    /// The file on disk, as this buffer last read or wrote it, once the host has said.
    baseline: Option<Baseline>,
    /// A save found the file changed since then, and nothing has written it since.
    changed_on_disk: bool,
    /// The regular file that save found, which a save the person agrees to may replace.
    save_over: Option<FileStamp>,
    /// The editor keeps this buffer for itself.
    internal: bool,
    /// An internal buffer that takes typing, as a prompt's does (ADR 0072).
    takes_typing: bool,
    /// Edits keep no undo record, so that no copy of the text stays in one.
    no_history: bool,
    /// Which keys it takes.
    mode: Mode,
    /// An emergency copy of its file waits for an answer.
    copy_waits: bool,
    /// The file's own spelling, if something else may have been typed.
    own_spelling: Option<PathBuf>,
    /// Changes whenever the name or the path does, so that what is kept of them elsewhere,
    /// such as an emergency entry, can tell (ADR 0062, ADR 0087).
    naming: u32,
    /// A copy's text was put in, until its copy has gone.
    recovery: Option<recovery::RecoveryMark>,
    /// A save of this buffer is running.
    saving: bool,
    /// `Some` while the file loads.
    loading: Option<Progress>,
    /// Changes whenever the content does.
    revision: u64,
    /// Whether the text may hold secrets (ADR 0044, ADR 0051). Withheld from the moment
    /// a file is visited until the host knows better.
    verdict: Verdict,
    /// A kill that counts as secret was yanked into it (ADR 0074). Nothing that judges the
    /// text clears this: [`Buffer::verdict`] combines the two.
    yanked_secret: bool,
    /// The state of the content now.
    content: ContentId,
    /// The state the file on disk has.
    saved: ContentId,
    /// The number of the next new content id.
    next_content: u64,
    /// Undo and redo.
    history: History,
}

impl Buffer {
    /// An empty buffer that visits no file, such as `*scratch*`.
    pub fn new(name: impl Into<String>) -> Self {
        Self::from_text(name, "")
    }

    /// A buffer holding `text`.
    pub fn from_text(name: impl Into<String>, text: &str) -> Self {
        let text = Rope::from(text);
        Self {
            name: name.into(),
            path: None,
            display: DisplayMap::new(&text, TabWidth::default()),
            text,
            info: FileInfo::default(),
            baseline: None,
            changed_on_disk: false,
            save_over: None,
            internal: false,
            takes_typing: false,
            no_history: false,
            mode: Mode::Fundamental,
            copy_waits: false,
            own_spelling: None,
            naming: 0,
            recovery: None,
            saving: false,
            loading: None,
            revision: 0,
            verdict: Verdict::Ordinary,
            yanked_secret: false,
            content: ContentId(0),
            saved: ContentId(0),
            next_content: 1,
            history: History::default(),
        }
    }

    /// A buffer holding a file's `bytes`, decoded at once. For small
    /// content; files load in the background instead.
    pub fn from_bytes(name: impl Into<String>, bytes: &[u8]) -> Self {
        let mut buffer = Self::new(name);
        buffer.apply_load(load::decode_all(bytes, TabWidth::default()));
        buffer
    }

    /// An empty, read-only buffer for `path`, which a loading thread is
    /// about to fill.
    ///
    /// It is named after the file. Telling apart buffers of files with the
    /// same name (Emacs's uniquify) is left to the editor, which knows the
    /// other buffers.
    pub fn visiting(path: PathBuf) -> Self {
        let name = path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        Self {
            path: Some(path),
            loading: Some(Progress::default()),
            verdict: Verdict::Unjudged,
            ..Self::new(name)
        }
    }

    /// Replaces the content with what has loaded so far. O(1).
    pub fn apply_load(&mut self, snapshot: LoadSnapshot) {
        self.text = snapshot.text;
        self.display = snapshot.display;
        self.info = snapshot.info;
        self.loading = (!snapshot.done).then_some(snapshot.progress);
        self.revision += 1;
        self.content = self.new_content();
        self.saved = self.content;
        self.history.clear();
    }

    /// Ends loading with no content, for a file that does not exist yet.
    pub fn finish_empty(&mut self) {
        self.loading = None;
    }

    /// Decides whether the file may hold secrets, from where its path really
    /// leads: each link's target on the way and the canonical path, or `None`
    /// if the path could not be resolved, which dev tooling counts as secret. The
    /// host resolves the path off the UI thread, before any content arrives.
    pub fn judge_secret(&mut self, resolved: Option<&[PathBuf]>) {
        self.verdict = secret::judge(self.path.as_deref(), resolved);
    }

    /// Decides whether the file may hold secrets when its path could not be resolved.
    /// `seen` holds what the walk passed before it failed, whose names count too.
    pub fn judge_unresolved(&mut self, seen: &[PathBuf]) {
        self.verdict = secret::judge_unresolved(self.path.as_deref(), seen);
    }

    /// Counts the text as secret from now on, as a kill that counts as secret was yanked
    /// into it, for dev tooling and the emergency save alike. Whatever judges the text later
    /// keeps it so, and a passphrase stays a passphrase. Returns whether a buffer that was
    /// not withheld now is.
    pub(crate) fn treat_as_secret(&mut self) -> bool {
        let was_withheld = self.verdict().is_withheld();
        self.yanked_secret = true;
        !was_withheld
    }

    /// Whether the text may hold secrets, and why.
    pub fn verdict(&self) -> Verdict {
        match self.verdict {
            // These say more than a yank does, and a passphrase's rules are its own.
            Verdict::SecretName | Verdict::Passphrase => self.verdict,
            _ if self.yanked_secret => Verdict::Yanked,
            verdict => verdict,
        }
    }

    /// Whether the file may hold secrets (ADR 0028, ADR 0044). A file's text
    /// must not leave the editor through dev tooling while this is true. An
    /// emergency save asks [`Verdict::may_be_copied`] instead (ADR 0051).
    pub fn is_secret(&self) -> bool {
        self.verdict().is_withheld()
    }

    /// A number that changes whenever the content does, so that caches of
    /// it (shaped rows) know when to check it again.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The state of the content now. A safe save reads this with the text it
    /// writes, and gives it to [`Buffer::mark_saved`] when the write is done.
    pub fn content_id(&self) -> ContentId {
        self.content
    }

    /// Records that the file on disk now holds the state `id`. Edits made
    /// since that state leave the buffer modified.
    pub fn mark_saved(&mut self, id: ContentId) {
        self.saved = id;
    }

    /// The file on disk, as this buffer last read or wrote it, once the host has said.
    pub fn baseline(&self) -> Option<&Baseline> {
        self.baseline.as_ref()
    }

    /// Records where the file is and what was there, when it is opened to load. What a
    /// save found changed was measured against the old one, so nothing is saved over now.
    pub fn set_baseline(&mut self, baseline: Baseline) {
        self.baseline = Some(baseline);
        self.save_over = None;
    }

    /// Records the file a save has just put in place.
    pub(crate) fn set_written(&mut self, stamp: FileStamp) {
        if let Some(baseline) = &mut self.baseline {
            baseline.file = Some(stamp);
        }
        self.note_written(stamp);
    }

    /// Whether a save found the file changed since this buffer read it or wrote it.
    pub(crate) fn changed_on_disk(&self) -> bool {
        self.changed_on_disk
    }

    /// Records whether a save found the file changed. A file that has not changed has
    /// nothing to save over.
    pub(crate) fn set_changed_on_disk(&mut self, changed: bool) {
        self.changed_on_disk = changed;
        if !changed {
            self.save_over = None;
        }
    }

    /// The regular file a save found changed on disk, which a save the person agrees to
    /// may replace, if it is still that file (ADR 0072).
    pub(crate) fn save_over(&self) -> Option<FileStamp> {
        self.save_over.filter(|_| self.changed_on_disk)
    }

    pub(crate) fn set_save_over(&mut self, found: FileStamp) {
        self.save_over = Some(found);
    }

    pub(crate) fn forget_save_over(&mut self) {
        self.save_over = None;
    }

    /// Whether a save of this buffer is running. Until it ends, the disk may hold
    /// either the old state or the one being written.
    pub fn is_saving(&self) -> bool {
        self.saving
    }

    pub(crate) fn set_saving(&mut self, saving: bool) {
        self.saving = saving;
    }

    /// Whether the content differs from the state last saved. Undoing back to
    /// that state makes the buffer unmodified again. If the history no longer
    /// reaches it, the buffer stays modified until it is saved.
    pub fn is_modified(&self) -> bool {
        self.content != self.saved
    }

    fn new_content(&mut self) -> ContentId {
        let id = ContentId(self.next_content);
        self.next_content += 1;
        id
    }

    /// Makes `edits` on clones, unless the buffer is read-only. `None` if all
    /// are empty.
    pub(crate) fn stage_edits(
        &self,
        edits: &[Replacement<'_>],
    ) -> Result<Option<StagedEdit>, EditError> {
        if let Some(reason) = self.read_only() {
            return Err(EditError::ReadOnly(reason));
        }
        let splices = normalized(&self.text, edits);
        Ok((!splices.is_empty()).then(|| self.stage_splices(&splices)))
    }

    fn stage_splices<T: SpliceText>(&self, splices: &[Splice<T>]) -> StagedEdit {
        let mut text = self.text.clone();
        let mut display = self.display.clone();
        let undo = apply(splices, &mut text, &mut display);
        StagedEdit {
            text,
            display,
            edits: splices.iter().map(offset_edit).collect(),
            undo,
        }
    }

    /// Puts a staged edit in the buffer, with its undo record and the cursors
    /// `after` it. Nothing here can panic part way.
    pub(crate) fn commit_edit(
        &mut self,
        staged: StagedEdit,
        opening: Opening,
        after: Option<SelectionSet>,
    ) {
        let before = self.content;
        self.content = self.new_content();
        self.text = staged.text;
        self.display = staged.display;
        self.revision += 1;
        if self.no_history {
            return;
        }
        let after = Side {
            content: self.content,
            selections: after,
        };
        self.history.record(staged.undo, opening, before, after);
    }

    /// The group the next undo or redo crosses, if the buffer allows one.
    pub(crate) fn next_crossing(&self, direction: Direction) -> Result<&Group, EditError> {
        if let Some(reason) = self.read_only() {
            return Err(EditError::ReadOnly(reason));
        }
        self.history.next(direction).ok_or(match direction {
            Direction::Undo => EditError::NoUndo,
            Direction::Redo => EditError::NoRedo,
        })
    }

    /// Makes the next undo or redo on clones, if there is one.
    pub(crate) fn stage_crossing(&self, direction: Direction) -> Result<StagedCrossing, EditError> {
        let group = self.next_crossing(direction)?;
        let mut text = self.text.clone();
        let mut display = self.display.clone();
        let mut steps = Vec::with_capacity(group.batches.len());
        let mut batches = Vec::with_capacity(group.batches.len());
        for batch in group.batches.iter().rev() {
            steps.push(batch.iter().map(offset_edit).collect());
            batches.push(apply(batch, &mut text, &mut display));
        }
        Ok(StagedCrossing {
            text,
            display,
            steps,
            direction,
            crossed: group.crossed(batches),
        })
    }

    /// Puts a staged undo or redo in the buffer, and restores the content id
    /// of the state it returns to.
    pub(crate) fn commit_crossing(&mut self, staged: StagedCrossing) {
        self.content = staged.crossed.source.content;
        self.text = staged.text;
        self.display = staged.display;
        self.revision += 1;
        self.history.cross(staged.direction, staged.crossed);
        self.note_crossing();
    }

    /// Whether a command that starts with cursors `selections` may join the
    /// newest undo group.
    pub(crate) fn can_join_undo(&self, selections: Option<&SelectionSet>) -> bool {
        self.history.can_join(self.content, selections)
    }

    /// Sets the cursors the newest undo group ends with.
    pub(crate) fn set_undo_after(&mut self, selections: SelectionSet) {
        self.history.set_after(selections);
    }

    #[cfg(test)]
    pub(crate) fn set_undo_limit(&mut self, limit: usize) {
        self.history = History::new(limit);
    }

    #[cfg(test)]
    pub(crate) fn undo_lens(&self) -> (usize, usize) {
        self.history.lens()
    }

    /// The buffer's name, such as `main.rs` or `*scratch*`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The file the buffer visits, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The text.
    pub fn text(&self) -> &Rope {
        &self.text
    }

    /// A snapshot of the display model. O(1).
    pub fn display(&self) -> DisplaySnapshot {
        self.display.snapshot()
    }

    /// Whether the text holds bidirectional controls, for the modeline's warning. O(1).
    pub(crate) fn has_bidi_controls(&self) -> bool {
        self.display.snapshot().has_bidi_controls()
    }

    /// The invalid bytes of the file, each shown as `\xNN`.
    pub fn invalid_bytes(&self) -> &InvalidBytes {
        self.display.invalid_bytes()
    }

    /// How many cells apart the tab stops are.
    pub fn tab_width(&self) -> TabWidth {
        self.display.tab_width()
    }

    /// The BOM and line ending of the file, and whether it is binary.
    pub fn file_info(&self) -> FileInfo {
        self.info
    }

    /// How far the file has loaded, while it loads.
    pub fn loading(&self) -> Option<Progress> {
        self.loading
    }

    /// Why the buffer cannot be edited, if it cannot. A reason that lasts
    /// wins over loading.
    pub fn read_only(&self) -> Option<ReadOnly> {
        if self.internal && !self.takes_typing {
            Some(ReadOnly::Internal)
        } else if self.info.binary {
            Some(ReadOnly::Binary)
        } else if !self.invalid_bytes().is_empty() {
            Some(ReadOnly::InvalidUtf8)
        } else if self.loading.is_some() {
            Some(ReadOnly::Loading)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::LineEnding;
    use crate::load::Builder;

    #[test]
    fn ids_and_reasons_read_the_same_outside_the_editor() {
        assert_eq!(BufferId(7).get(), 7);
        assert_eq!(ReadOnly::Loading.as_str(), "loading");
        assert_eq!(ReadOnly::InvalidUtf8.as_str(), "invalid-utf8");
        assert_eq!(ReadOnly::Binary.as_str(), "binary");
        assert_eq!(ReadOnly::Internal.as_str(), "internal");
    }

    #[test]
    fn every_load_changes_the_revision_and_the_file_info() {
        let mut buffer = Buffer::visiting(PathBuf::from("/tmp/a.txt"));
        let start = buffer.revision();
        let mut builder = Builder::new(None, TabWidth::default());
        builder.push(b"a\r\n");
        buffer.apply_load(builder.snapshot());
        let first = buffer.revision();
        assert_ne!(first, start);
        buffer.apply_load(builder.snapshot());
        assert_ne!(buffer.revision(), first);
        assert_eq!(buffer.file_info().line_ending, LineEnding::Crlf);
    }

    #[test]
    fn a_loading_buffer_is_read_only_until_done() {
        let mut buffer = Buffer::visiting(PathBuf::from("/tmp/a.txt"));
        assert_eq!(buffer.read_only(), Some(ReadOnly::Loading));
        assert_eq!(buffer.name(), "a.txt");
        let mut builder = Builder::new(Some(6), TabWidth::default());
        builder.push(b"abc");
        buffer.apply_load(builder.snapshot());
        assert_eq!(buffer.text().to_string(), "abc");
        assert_eq!(
            buffer.loading(),
            Some(Progress {
                read: 3,
                total: Some(6)
            })
        );
        builder.push(b"def");
        builder.finish();
        buffer.apply_load(builder.snapshot());
        assert_eq!(buffer.read_only(), None);
        assert_eq!(buffer.loading(), None);
        assert_eq!(buffer.display().row_cells(0), 6);
    }

    #[test]
    fn invalid_and_binary_files_stay_read_only() {
        let buffer = Buffer::from_bytes("bad", b"a\xff");
        assert_eq!(buffer.read_only(), Some(ReadOnly::InvalidUtf8));
        assert_eq!(buffer.invalid_bytes().len(), 1);
        let binary = Buffer::from_bytes("bin", b"a\0\xff");
        assert_eq!(binary.read_only(), Some(ReadOnly::Binary));
        assert_eq!(Buffer::from_bytes("ok", b"ok").read_only(), None);
    }

    #[test]
    fn a_missing_file_ends_as_an_empty_editable_buffer() {
        let mut buffer = Buffer::visiting(PathBuf::from("/tmp/new.txt"));
        buffer.finish_empty();
        assert_eq!(buffer.read_only(), None);
        assert!(buffer.text().is_empty());
        assert_eq!(buffer.path(), Some(Path::new("/tmp/new.txt")));
    }

    #[test]
    fn a_visited_file_is_secret_until_its_path_is_resolved() {
        let plain = PathBuf::from("/tmp/notes.txt");
        let key = PathBuf::from("/home/me/.ssh/id_rsa");
        let mut buffer = Buffer::visiting(plain.clone());
        assert!(buffer.is_secret(), "fails closed while unknown");
        buffer.judge_secret(Some(std::slice::from_ref(&plain)));
        assert!(!buffer.is_secret());
        buffer.judge_secret(Some(&[key.clone(), plain.clone()]));
        assert!(buffer.is_secret(), "a link through a secret");
        buffer.judge_secret(None);
        assert!(buffer.is_secret(), "a path that cannot be resolved");

        let mut named = Buffer::visiting(PathBuf::from("/tmp/.env"));
        named.judge_secret(Some(&[plain]));
        assert!(named.is_secret(), "named like a secret, whatever it is");
    }

    fn edit(range: Range<usize>, text: &str) -> Replacement<'_> {
        Replacement { range, text }
    }

    /// `edits` normalized against `text`, as `(start, len, text)`.
    fn batch(text: &str, edits: &[Replacement<'_>]) -> Vec<(usize, usize, String)> {
        normalized(&Rope::from(text), edits)
            .into_iter()
            .map(|splice| (splice.start, splice.len, splice.text.into_owned()))
            .collect()
    }

    fn one(start: usize, len: usize, text: &str) -> (usize, usize, String) {
        (start, len, text.to_owned())
    }

    #[test]
    fn edits_are_sorted_and_clamped() {
        assert_eq!(
            batch("abcdef", &[edit(4..5, "x"), edit(1..2, "y")]),
            [one(1, 1, "y"), one(4, 1, "x")]
        );
        assert_eq!(batch("abc", &[edit(2..99, "x")]), [one(2, 1, "x")], "past");
        assert_eq!(batch("abc", &[edit(99..99, "x")]), [one(3, 0, "x")]);
        assert_eq!(
            batch("abc", &[edit(usize::MAX..usize::MAX, "x")]),
            [one(3, 0, "x")]
        );
        assert_eq!(batch("abc", &[edit(0..usize::MAX, "x")]), [one(0, 3, "x")]);
        #[expect(clippy::reversed_empty_ranges, reason = "what the test is about")]
        let reversed = edit(2..1, "x");
        assert_eq!(batch("abc", &[reversed]), [one(2, 0, "x")], "reversed");
        #[expect(clippy::reversed_empty_ranges, reason = "what the test is about")]
        let reversed = edit(usize::MAX..0, "x");
        assert_eq!(batch("abc", &[reversed]), [one(3, 0, "x")], "reversed");
        // Inside 日, which takes bytes 1..4.
        assert_eq!(batch("a日b", &[edit(2..3, "x")]), [one(1, 0, "x")]);
        assert_eq!(batch("a日b", &[edit(2..4, "x")]), [one(1, 3, "x")]);
    }

    #[test]
    fn empty_edits_are_dropped() {
        assert_eq!(batch("abc", &[edit(1..1, "")]), []);
        assert_eq!(batch("abc", &[edit(2..2, ""), edit(9..9, "")]), []);
        assert_eq!(batch("abc", &[]), []);
        // Empty once clamped: `2..3` inside 日.
        assert_eq!(batch("a日b", &[edit(2..3, "")]), []);
    }

    #[test]
    fn overlapping_edits_become_their_union_with_their_texts_in_order() {
        assert_eq!(
            batch("abcdefgh", &[edit(4..7, "2"), edit(1..5, "1")]),
            [one(1, 6, "12")],
            "partly"
        );
        assert_eq!(
            batch("abcdefgh", &[edit(1..7, "1"), edit(3..4, "2")]),
            [one(1, 6, "12")],
            "one inside the other"
        );
        assert_eq!(
            batch("abcdefgh", &[edit(1..7, "1"), edit(3..3, "2")]),
            [one(1, 6, "12")],
            "an insertion inside"
        );
        assert_eq!(
            batch(
                "abcdefgh",
                &[edit(1..3, "a"), edit(2..5, "b"), edit(4..6, "c")]
            ),
            [one(1, 5, "abc")],
            "a chain"
        );
        assert_eq!(
            batch("abcdefgh", &[edit(2..2, "x"), edit(2..2, "y")]),
            [one(2, 0, "xy")],
            "insertions at one offset, in the order given"
        );
    }

    #[test]
    fn edits_that_only_touch_stay_apart() {
        assert_eq!(
            batch("abcdefgh", &[edit(3..5, "2"), edit(1..3, "1")]),
            [one(1, 2, "1"), one(3, 2, "2")],
            "end to start"
        );
        assert_eq!(
            batch("abcdefgh", &[edit(1..3, "1"), edit(3..3, "2")]),
            [one(1, 2, "1"), one(3, 0, "2")],
            "an insertion where a range ends"
        );
        assert_eq!(
            batch("abcdefgh", &[edit(3..5, "2"), edit(3..3, "1")]),
            [one(3, 0, "1"), one(3, 2, "2")],
            "an insertion where a range starts goes first"
        );
    }

    /// Applies `edits` to a rope, checks the display map, and returns the
    /// text and the batch that undoes them.
    fn applied(text: &str, edits: &[Replacement<'_>]) -> (String, Batch) {
        let mut rope = Rope::from(text);
        let mut display = DisplayMap::new(&rope, TabWidth::default());
        let undo = apply(&normalized(&rope, edits), &mut rope, &mut display);
        let fresh = DisplayMap::new(&rope, TabWidth::default()).snapshot();
        assert_eq!(display.snapshot().max_point(), fresh.max_point());
        rope.check_invariants();
        (rope.to_string(), undo)
    }

    #[test]
    fn a_batch_reports_what_undoes_it_in_the_offsets_after_it() {
        let (text, undo) = applied("abcdefgh", &[edit(5..7, "XXX"), edit(1..3, "")]);
        assert_eq!(text, "adeXXXh");
        let undo: Vec<_> = undo
            .iter()
            .map(|s| (s.start, s.len, s.text.as_ref().unwrap().to_string()))
            .collect();
        assert_eq!(undo, [one(1, 0, "bc"), one(3, 3, "fg")]);
    }

    #[test]
    fn undoing_a_batch_gives_back_the_text_and_the_batch() {
        let start = "日本語\r\ne\u{301}\tz";
        let forward = [edit(0..3, "x"), edit(9..11, "👍"), edit(14..14, "\u{301}")];
        let (after, undo) = applied(start, &forward);
        let mut rope = Rope::from(after.as_str());
        let mut display = DisplayMap::new(&rope, TabWidth::default());
        let redo = apply(&undo, &mut rope, &mut display);
        assert_eq!(rope.to_string(), start);
        let again = apply(&redo, &mut rope, &mut display);
        assert_eq!(rope.to_string(), after);
        assert_eq!(again.len(), undo.len());
    }

    #[test]
    fn staging_leaves_the_buffer_as_it_was_until_the_commit() {
        let mut buffer = Buffer::from_text("t", "abc");
        let (revision, content) = (buffer.revision(), buffer.content_id());
        let staged = buffer.stage_edits(&[edit(1..1, "x")]).unwrap().unwrap();
        assert_eq!(buffer.text().to_string(), "abc");
        assert_eq!(buffer.revision(), revision);
        assert_eq!(buffer.content_id(), content);
        assert_eq!(buffer.undo_lens(), (0, 0));
        assert_eq!(staged.display().max_point().column, 4);
        assert_eq!(staged.steps().len(), 1);
        drop(staged);
        assert_eq!(buffer.display().max_point().column, 3);

        let staged = buffer.stage_edits(&[edit(1..1, "x")]).unwrap().unwrap();
        buffer.commit_edit(staged, Opening::New(None), None);
        assert_eq!(buffer.text().to_string(), "axbc");
        assert_eq!(buffer.display().max_point().column, 4);
        assert_eq!(buffer.revision(), revision + 1);
        assert_ne!(buffer.content_id(), content);
        assert!(buffer.is_modified());
        assert_eq!(buffer.undo_lens(), (1, 0));
    }

    /// Text that panics when it is put in a rope, after `good` replacements.
    struct PanicsAfter<'a> {
        good: &'a std::cell::Cell<usize>,
    }

    impl SpliceText for PanicsAfter<'_> {
        fn byte_len(&self) -> usize {
            1
        }

        fn replace_in(&self, rope: &mut Rope, range: Range<usize>) -> crate::rope::RopeEdit {
            let good = self.good.get();
            assert!(good > 0, "a bug part way through a batch");
            self.good.set(good - 1);
            rope.replace(range, "x")
        }
    }

    #[test]
    fn a_panic_part_way_through_a_batch_changes_nothing() {
        let buffer = Buffer::from_text("t", "abcdef");
        let good = std::cell::Cell::new(1);
        let splices: Vec<_> = [1, 3, 5]
            .map(|start| Splice {
                start,
                len: 1,
                text: PanicsAfter { good: &good },
            })
            .into_iter()
            .collect();
        let staged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            buffer.stage_splices(&splices)
        }));
        assert!(staged.is_err());
        assert_eq!(good.get(), 0, "the panic came after one replacement");
        assert_eq!(buffer.text().to_string(), "abcdef");
        assert_eq!(buffer.display().max_point().column, 6);
        assert_eq!(buffer.revision(), 0);
        assert!(!buffer.is_modified());
        assert_eq!(buffer.undo_lens(), (0, 0));
    }

    #[test]
    fn empty_edits_stage_nothing() {
        let buffer = Buffer::from_text("t", "abc");
        assert!(buffer.stage_edits(&[edit(1..1, "")]).unwrap().is_none());
        assert!(buffer.stage_edits(&[]).unwrap().is_none());
    }

    #[test]
    fn read_only_buffers_refuse_edits_for_their_reason() {
        let bad = Buffer::from_bytes("bad", b"a\xff");
        let refused = |buffer: &Buffer| buffer.stage_edits(&[edit(0..0, "x")]).err();
        assert_eq!(
            refused(&bad),
            Some(EditError::ReadOnly(ReadOnly::InvalidUtf8))
        );
        assert_eq!(
            refused(&Buffer::from_bytes("bin", b"a\0")),
            Some(EditError::ReadOnly(ReadOnly::Binary))
        );
        assert_eq!(
            refused(&Buffer::visiting(PathBuf::from("/tmp/a"))),
            Some(EditError::ReadOnly(ReadOnly::Loading))
        );
        assert_eq!(
            bad.stage_crossing(Direction::Undo).err(),
            Some(EditError::ReadOnly(ReadOnly::InvalidUtf8))
        );
    }

    #[test]
    fn a_load_is_a_new_saved_state_with_no_history() {
        let mut buffer = Buffer::from_text("t", "abc");
        let staged = buffer.stage_edits(&[edit(0..0, "x")]).unwrap().unwrap();
        buffer.commit_edit(staged, Opening::New(None), None);
        assert!(buffer.is_modified());
        let before = buffer.content_id();
        let mut builder = Builder::new(None, TabWidth::default());
        builder.push(b"new");
        builder.finish();
        buffer.apply_load(builder.snapshot());
        assert!(!buffer.is_modified());
        assert_ne!(buffer.content_id(), before);
        assert_eq!(buffer.undo_lens(), (0, 0));
    }

    #[test]
    fn saving_marks_the_state_that_was_written() {
        let mut buffer = Buffer::from_text("t", "abc");
        assert!(!buffer.is_modified());
        let staged = buffer.stage_edits(&[edit(0..0, "x")]).unwrap().unwrap();
        buffer.commit_edit(staged, Opening::New(None), None);
        let written = buffer.content_id();
        // An edit while the save runs.
        let staged = buffer.stage_edits(&[edit(0..0, "y")]).unwrap().unwrap();
        buffer.commit_edit(staged, Opening::New(None), None);
        buffer.mark_saved(written);
        assert!(buffer.is_modified(), "the edit since is not on disk");
        buffer.mark_saved(buffer.content_id());
        assert!(!buffer.is_modified());
    }

    #[test]
    fn errors_read_as_emacs_says_them() {
        let read_only = EditError::ReadOnly(ReadOnly::Loading);
        assert_eq!(read_only.to_string(), "Buffer is read-only");
        assert_eq!(EditError::NoSuchBuffer.to_string(), "No such buffer");
        assert_eq!(EditError::NoUndo.to_string(), "No further undo information");
        assert_eq!(EditError::NoRedo.to_string(), "No further redo information");
    }

    #[test]
    fn a_verdict_tells_an_unresolved_path_from_a_secret_name() {
        let mut plain = Buffer::visiting(PathBuf::from("/tmp/notes.txt"));
        assert_eq!(plain.verdict(), Verdict::Unjudged);
        plain.judge_secret(None);
        assert_eq!(plain.verdict(), Verdict::Unresolved);
        assert!(plain.is_secret(), "dev tooling still withholds it");
        assert!(plain.verdict().may_be_copied());
        plain.judge_unresolved(&[PathBuf::from("/h/.aws/config")]);
        assert_eq!(plain.verdict(), Verdict::SecretName, "seen on the way");

        let mut named = Buffer::visiting(PathBuf::from("/tmp/.env"));
        named.judge_secret(None);
        assert_eq!(named.verdict(), Verdict::SecretName);
        assert!(!named.verdict().may_be_copied());

        let mut scratch = Buffer::new("*scratch*");
        scratch.judge_unresolved(&[]);
        assert_eq!(scratch.verdict(), Verdict::Ordinary);
    }

    #[test]
    fn a_buffer_without_a_file_is_not_secret() {
        let mut buffer = Buffer::new("*scratch*");
        assert!(!buffer.is_secret());
        buffer.judge_secret(None);
        assert!(!buffer.is_secret());
    }
}
