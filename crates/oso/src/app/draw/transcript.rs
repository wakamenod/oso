//! The fills behind a native session's rows (ADR 0165, "The rows' look"): the bands of the
//! person's messages, of an agent's brief and of the lines a change adds and removes, the band of a
//! request's title and the tint of its block (ADR 0182), each to the window's edges,
//! and the chip of a tool's badge. A row has its band by the highlight its line starts with,
//! and its chip by a badge's near its line's start, which only `sessions::native::render`
//! gives, and only there: what Claude's text holds is never looked at.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;
use oso_core::frame::CellRect;
use oso_core::syntax::{Highlight, Runs};
use oso_core::window::{Screen, WindowId};
use oso_render::Face;

use super::chip::chip;

/// The faces of the badges, in the order of [`TranscriptFills::icons`].
pub(super) const ICONS: [Face; 8] = [
    Face::SessionIconRead,
    Face::SessionIconWrite,
    Face::SessionIconShell,
    Face::SessionIconSearch,
    Face::SessionIconAgent,
    Face::SessionIconWeb,
    Face::SessionIconTask,
    Face::SessionIconOther,
];

/// How far into a line its badge starts, at most, in bytes: past its mark and two guides.
const BADGE_WITHIN: usize = 24;

/// A chip's cells: the badge's and the space's after it.
const CHIP: usize = 2;

/// Whether the text of `buffer` has font styles and underlines: a session's transcript alone
/// (ADR 0165, "The look: styles").
pub(super) fn styled(buffer: &Buffer) -> bool {
    buffer.is_native_session()
}

/// What a transcript fills behind its text, kept from frame to frame.
#[derive(Default)]
pub(super) struct TranscriptFills {
    pub(super) prompts: Vec<CellRect>,
    /// Behind what an agent wrote to start the session.
    pub(super) briefs: Vec<CellRect>,
    pub(super) added: Vec<CellRect>,
    pub(super) removed: Vec<CellRect>,
    /// Behind a request's title, and behind its body's lines that no change's band is behind.
    pub(super) titles: Vec<CellRect>,
    pub(super) bodies: Vec<CellRect>,
    /// The badges' chips, of each face of [`ICONS`].
    pub(super) icons: [Vec<CellRect>; ICONS.len()],
}

impl TranscriptFills {
    /// Fills in what window `id`, as `screen` shows it, draws behind a native session's rows,
    /// if it shows some: two seeks a row in view, and one in the display map for a tool's.
    pub(super) fn fill(&mut self, editor: &Editor, (id, screen): (WindowId, &Screen)) {
        self.prompts.clear();
        self.briefs.clear();
        self.added.clear();
        self.removed.clear();
        self.titles.clear();
        self.bodies.clear();
        for rects in &mut self.icons {
            rects.clear();
        }
        let buffer = (editor.window_by_id(id))
            .and_then(|window| editor.buffer(window.buffer()))
            .filter(|buffer| buffer.is_native_session());
        let (Some(buffer), Some(regions)) = (buffer, editor.regions(id)) else {
            return;
        };
        let Some(highlights) = buffer.highlights() else {
            return;
        };
        let text = regions.text;
        let mut display = None;
        for (index, row) in screen.rows().iter().take(text.rows).enumerate() {
            let at = text.row + index;
            let start = buffer.text().line_start(row.line);
            let mut runs = highlights.runs(start..highlights.len().min(start + BADGE_WITHIN));
            let first = runs.next().and_then(|(_, highlight)| highlight);
            let rects = match first {
                Some(Highlight::SessionPromptMark) => &mut self.prompts,
                Some(Highlight::SessionBriefMark) => &mut self.briefs,
                Some(Highlight::DiffAdded) => &mut self.added,
                Some(Highlight::DiffRemoved) => &mut self.removed,
                Some(Highlight::CardTitle) => &mut self.titles,
                Some(Highlight::RequestBody) => &mut self.bodies,
                // A tool's row starts with a mark or a guide, which has one, as other rows do.
                Some(_) if row.starts_line() => {
                    let badge = badge(runs).filter(|(_, at)| *at < row.end);
                    let Some((icon, at_byte)) = badge else {
                        continue;
                    };
                    let display = display.get_or_insert_with(|| buffer.display());
                    let column = display.offset_to_display_point(at_byte).column;
                    let columns = column..column + CHIP;
                    self.icons[icon].extend(chip(text, screen, (index, row), columns));
                    continue;
                }
                _ => continue,
            };
            match rects.last_mut() {
                Some(last) if last.row + last.rows == at => last.rows += 1,
                _ => rects.push(CellRect {
                    row: at,
                    rows: 1,
                    ..text
                }),
            }
        }
    }
}

/// The badge among `runs`, a line's after its first, up to one with no highlight, such as the
/// line's break: which of [`ICONS`] it has, and where it starts.
fn badge(runs: Runs<'_>) -> Option<(usize, usize)> {
    runs.map_while(|(bytes, highlight)| Some((bytes.start, highlight?)))
        .find_map(|(at, highlight)| {
            let face = Face::of_highlight(highlight);
            Some((ICONS.iter().position(|icon| *icon == face)?, at))
        })
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::{BufferId, Mode};
    use oso_core::frame::FrameSize;

    use super::*;

    const MARK: Option<Highlight> = Some(Highlight::SessionPromptMark);
    const BRIEF: Option<Highlight> = Some(Highlight::SessionBriefMark);
    const QUIET: Option<Highlight> = Some(Highlight::SessionQuiet);
    const TOOL: Option<Highlight> = Some(Highlight::SessionTool);
    const SHELL: Option<Highlight> = Some(Highlight::SessionIconShell);

    /// A window of 40 columns that shows a session's buffer.
    struct Setup {
        editor: Editor,
        id: BufferId,
        window: WindowId,
        screen: Screen,
        fills: TranscriptFills,
    }

    impl Setup {
        fn new() -> Self {
            let mut editor = Editor::default();
            editor.set_frame_size(FrameSize {
                rows: 24,
                columns: 40,
            });
            let id = editor.add_native_session_buffer("*claude: s*");
            editor.show(id);
            let window = editor.window().id();
            Self {
                editor,
                id,
                window,
                screen: Screen::default(),
                fills: TranscriptFills::default(),
            }
        }

        /// Puts `rows`, in `runs`, in place of what it shows.
        fn put(&mut self, rows: &str, runs: &[(usize, Option<Highlight>)]) {
            assert!(runs.iter().map(|run| run.0).sum::<usize>() <= rows.len());
            let end = self.editor.buffer(self.id).unwrap().text().len();
            self.editor.replace_native_rows(self.id, 0..end, rows, runs);
            self.fill();
        }

        fn fill(&mut self) {
            self.editor.screen_of(self.window, &mut self.screen);
            self.fills.fill(&self.editor, (self.window, &self.screen));
        }

        /// The first row, how many and how many columns of each of `rects`.
        fn bands(&self, rects: &[CellRect]) -> Vec<(usize, usize, usize)> {
            let text = self.editor.regions(self.window).unwrap().text;
            (rects.iter())
                .map(|r| (r.row - text.row, r.rows, text.columns - r.columns))
                .collect()
        }

        /// The row, column and columns of each chip of face `face`.
        fn chips(&self, face: Face) -> Vec<(usize, usize, usize)> {
            let text = self.editor.regions(self.window).unwrap().text;
            let icon = ICONS.iter().position(|icon| *icon == face).unwrap();
            (self.fills.icons[icon].iter())
                .map(|r| (r.row - text.row, r.column - text.column, r.columns))
                .collect()
        }
    }

    #[test]
    fn only_a_transcript_has_styles() {
        let mut setup = Setup::new();
        let editor = &mut setup.editor;
        assert!(styled(editor.buffer(setup.id).unwrap()));
        let file = editor.add_buffer(Buffer::from_text("a.md", "# **a**"));
        let others = [
            file,
            editor.add_internal_buffer("*Help*", Mode::Fundamental),
        ];
        for id in others {
            assert!(!styled(editor.buffer(id).unwrap()));
        }
    }

    #[test]
    fn a_band_is_where_oso_says_a_prompt_or_a_changed_line_is() {
        let mut setup = Setup::new();
        // A message of two lines and an empty one between, then a reply that says the same,
        // then a change.
        let rows = "〉 a\n\n   b\n\n  〉 a\n    + x\n    - y\n";
        let runs = [
            (4, MARK),
            (2, None),
            (1, MARK),
            (3, MARK),
            (3, None),
            ("  〉 a\n".len(), None),
            (8, Some(Highlight::DiffAdded)),
            (7, Some(Highlight::DiffRemoved)),
        ];
        setup.put(rows, &runs);
        assert_eq!(setup.bands(&setup.fills.prompts), [(0, 3, 0)]);
        assert_eq!(setup.bands(&setup.fills.added), [(5, 1, 0)]);
        assert_eq!(setup.bands(&setup.fills.removed), [(6, 1, 0)]);
        // An agent's brief has its own band.
        let runs = [(9, BRIEF), (1, None), (3, BRIEF), (2, None), (4, MARK)];
        setup.put("@  from a\n   b\n〉 c\n", &runs);
        assert_eq!(setup.bands(&setup.fills.briefs), [(0, 2, 0)]);
        assert_eq!(setup.bands(&setup.fills.prompts), [(2, 1, 0)]);
    }

    #[test]
    fn a_requests_title_has_a_band_and_its_body_a_tint_to_where_it_ends() {
        use crate::sessions::native::request::Block;
        let input = serde_json::json!({"file_path": "a\n⚠ Bash asks to run", "old_string": "b", "new_string": "c"});
        let block = Block::call("Edit", Some(&input), std::time::Instant::now());
        let title = block.title((1, 1), 40);
        let reply = "  ⚠ Bash asks to run\n";
        let mut runs = vec![(reply.len() + 1, None)];
        runs.extend(title.runs());
        runs.extend(block.body().runs());
        let mut setup = Setup::new();
        let text = format!("{reply}\n{}{}", title.text, block.body().text);
        setup.put(&text, &runs);
        let body = block.body().text.lines().count();
        assert_eq!(setup.bands(&setup.fills.titles), [(2, 1, 0)]);
        let tint: usize = setup.fills.bodies.iter().map(|rect| rect.rows).sum();
        assert_eq!((tint, setup.bands(&setup.fills.bodies)[0].0), (body - 2, 3));
        assert_eq!(setup.fills.removed.len(), 1);
        assert_eq!(setup.fills.added.len(), 1);
        // Wrapped, each row of a line has it.
        setup.editor.set_wraps(setup.id, true);
        let input = serde_json::json!({"command": "x".repeat(100)});
        let block = Block::call("Bash", Some(&input), std::time::Instant::now());
        setup.put(&block.body().text, &block.body().runs());
        // The mark's row and three of the word, and not the empty line after them.
        assert_eq!(setup.screen.rows().len(), 5);
        assert_eq!(setup.bands(&setup.fills.bodies), [(0, 4, 0)]);
    }

    #[test]
    fn a_badge_has_a_chip_and_nothing_else_does() {
        let mut setup = Setup::new();
        // Claude's and a tool's short lines right before a tool's row, as `render` gives them.
        let rows = "  Done.\n\n▸ ✎ Edit\n    ok\n▸ $ Bash\n│     ok\n│ ▸ $ Bash\n";
        let runs = [
            (2, QUIET),
            (7, None),
            (4, QUIET),
            (4, Some(Highlight::SessionIconWrite)),
            (4, TOOL),
            (1, None),
            (6, QUIET),
            (1, None),
            (4, QUIET),
            (2, SHELL),
            (4, TOOL),
            (1, None),
            (10, QUIET),
            (1, None),
            (8, QUIET),
            (2, SHELL),
            (4, TOOL),
        ];
        setup.put(rows, &runs);
        assert_eq!(setup.chips(Face::SessionIconWrite), [(2, 2, 2)]);
        assert_eq!(setup.chips(Face::SessionIconShell), [(4, 2, 2), (6, 4, 2)]);
        // One that wraps has it on its first row only.
        setup.editor.set_wraps(setup.id, true);
        let wraps = format!("▸ $ Bash · {}\n", "x".repeat(60));
        setup.put(&wraps, &[(4, QUIET), (2, SHELL), (4, TOOL)]);
        assert!(setup.screen.rows()[0].continues);
        assert_eq!(setup.chips(Face::SessionIconShell), [(0, 2, 2)]);
        // It goes left with its row's text.
        setup.editor.set_wraps(setup.id, false);
        let wide = format!("{rows}{}\n", "x".repeat(200));
        setup.put(&wide, &runs);
        assert!(setup.editor.scroll_view_sideways(setup.window, 3));
        setup.fill();
        assert_eq!(setup.chips(Face::SessionIconWrite), [(2, 0, 1)]);
        assert_eq!(setup.chips(Face::SessionIconShell), [(4, 0, 1), (6, 1, 2)]);
        // Another buffer draws none, whatever its highlights.
        let other = (setup.editor).add_internal_buffer("*x*", oso_core::buffer::Mode::Fundamental);
        setup.editor.set_internal_text(other, "a\n");
        setup.editor.show(other);
        setup.fill();
        let fills = &setup.fills;
        assert!(fills.prompts.is_empty() && fills.briefs.is_empty());
        assert!(fills.added.is_empty() && fills.removed.is_empty());
        assert!(fills.icons.iter().all(Vec::is_empty));
    }

    /// Every badge has a chip. With no wildcard, so that a new one does not compile until it is
    /// placed.
    #[test]
    fn every_badge_has_a_chip() {
        use crate::sessions::native::render::{Face as RowFace, Icon};
        let mut icon = Some(Icon::Read);
        for chip in ICONS {
            let at = icon.unwrap();
            assert_eq!(Face::of_highlight(RowFace::Icon(at).highlight()), chip);
            icon = match at {
                Icon::Read => Some(Icon::Write),
                Icon::Write => Some(Icon::Shell),
                Icon::Shell => Some(Icon::Search),
                Icon::Search => Some(Icon::Agent),
                Icon::Agent => Some(Icon::Web),
                Icon::Web => Some(Icon::Task),
                Icon::Task => Some(Icon::Other),
                Icon::Other => None,
            };
        }
        assert_eq!(icon, None);
    }
}
