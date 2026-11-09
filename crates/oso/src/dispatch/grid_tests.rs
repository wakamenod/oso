//! Text-grid snapshots (ADR 0037, layer 3; ADR 0045): key sequences run
//! through the [`Dispatcher`], then the screen is compared as text.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on
//! every OS with no window.
//!
//! The tests live here because `oso` is a binary crate, so `tests/` cannot
//! reach the `Dispatcher`. The grid draws through the code the renderer uses
//! (`DisplayCluster::for_each_run`, `Window::cursor_point`,
//! `motion::cursor_box`, `frame::cursor_rect`), so a snapshot fails when
//! the cells on screen change. Pixels are not covered (ADR 0037, layer 4).
//!
//! A frame is its regions from the top (ADR 0057). Each row of the text area is
//! one line between `│`, the modeline's between `=` and the echo area's between
//! `"`. Below a row that needs it, a line between `:` marks its cells. See
//! `docs/dev/testing.md`. The modeline and the echo area are the spans the app draws,
//! laid out by `text_clusters`, as the renderer does. A preedit is read into its row
//! through `row_clusters` and placed by `motion::cursor_box`, as the renderer does,
//! and a line between `_` below its row marks its underline with `-` and the segment
//! being converted with `=` (ADR 0069). A line between `+` below a row marks with `*`
//! the cells a region takes, from `Editor::region_cells`, as the renderer fills them
//! (ADR 0071), and with `#` and `~` the current isearch match and the others in view,
//! from `Editor::isearch_cells` (ADR 0082).
//!
//! An open prompt's float comes after the frame, as the innermost prompt's cells from its
//! place: its ring as a box of `┌─┐`, and inside it the label, the answer and a message
//! (ADR 0072). Beneath it, the window's hollow cursor is marked `o` in place of `^`. That
//! the float hides what is under it is the renderer's, checked in its scene tests.

use oso_core::buffer::Buffer;
use std::ops::Range;

use oso_core::display_map::{Preedit, text_clusters};
use oso_core::editor::Preview;
use oso_core::editor::{Editor, Status};
use oso_core::frame::{self, CellRect, FrameSize};
use oso_core::key::KeySequence;
use oso_core::minibuffer::{self, Kind, Prompt, PromptLayout};
use oso_core::motion;
use oso_core::window::{Scroll, ViewSize};

use super::Dispatcher;

mod rows;
use rows::{Style, draw_row, draw_run};

/// The frame's screen: its regions from the top, one line per row and one per
/// marked row.
fn text_grid(editor: &Editor) -> String {
    let regions = editor.frame();
    let mut status = Status::default();
    editor.status_into(&mut status);
    let mut lines = Vec::new();
    draw_text_area(editor, regions.window.text, &mut lines);
    let modeline = [&status.warnings[..], &status.name[..], &status.state[..]];
    draw_spans(&modeline, regions.window.modeline, '=', &mut lines);
    let echo = [status.echo_warnings(), &status.echo[..]];
    draw_spans(&echo, regions.echo, '"', &mut lines);
    draw_float(editor, &mut lines);
    lines.join("\n")
}

/// The innermost prompt's float, if one is open: where it is, then its cells. The answer's
/// field shows as `[` and `]` in the padding beside it, and a key's chip as `[y]`. A list's
/// rows follow, the selected one with `>` beside it and what matched marked `*` below.
fn draw_float(editor: &Editor, lines: &mut Vec<String>) {
    let (Some(prompt), Some(layout)) = (editor.prompt(), editor.prompt_layout()) else {
        return;
    };
    let (outer, inner) = (layout.regions.outer, layout.regions.inner);
    lines.push(format!(
        "float at row {}, column {}: {} x {}",
        outer.row, outer.column, outer.rows, outer.columns
    ));
    let framed = layout.regions.inner != layout.regions.outer;
    let edge = if framed { '│' } else { '|' };
    let label = prompt.label();
    let rows = &layout.label;
    // Rows of padding above what it shows, as a completion's float has.
    for _ in outer.row..inner.row {
        lines.push(format!("{edge}{}{edge}", " ".repeat(outer.columns)));
    }
    for index in 0..inner.rows {
        let row = inner.row + index;
        // What goes on the row, by column from the inner cells' left edge.
        let mut parts: Vec<(usize, String, Vec<char>)> = Vec::new();
        let cells = layout.label_cells;
        if (cells.row..cells.row + cells.rows).contains(&row) {
            let at = row - cells.row;
            let cut = if at + 1 == rows.len() && layout.label_cut {
                minibuffer::CUT
            } else {
                ""
            };
            let text = rows.get(at).map_or("", |range| &label[range.clone()]);
            let (text, marks) = spans_row(&[text, cut], cells.columns);
            parts.push((cells.column - inner.column, text, marks));
        }
        if row == layout.input.row && layout.input.columns > 0 {
            let (text, marks) = draw_input(editor, layout.input);
            parts.push((layout.input.column - inner.column, text, marks));
        }
        if row == layout.count.row && layout.count.columns > 0 {
            parts.extend(draw_count(editor, prompt, layout));
        }
        if row == layout.message.row && layout.message.columns > 0 {
            let message = prompt.message().unwrap_or("");
            let (message, cut) = minibuffer::fit_message(message, layout.message.columns);
            let (text, marks) = spans_row(&[" [", message, cut, "]"], layout.message.columns);
            parts.push((layout.message.column - inner.column, text, marks));
        }
        draw_keys(prompt, layout, row, &mut parts);
        parts.sort_by_key(|part| part.0);
        let (mut text, mut marks) = (String::new(), Vec::new());
        for (column, part, part_marks) in parts {
            let gap = column.saturating_sub(marks.len());
            text.extend(std::iter::repeat_n(' ', gap));
            marks.extend(std::iter::repeat_n(' ', gap));
            text.push_str(&part);
            marks.extend(part_marks);
        }
        let gap = inner.columns.saturating_sub(marks.len());
        text.extend(std::iter::repeat_n(' ', gap));
        marks.extend(std::iter::repeat_n(' ', gap));
        // A field that spans the row shows by brackets in the padding beside it.
        let field = layout.field.columns == inner.columns && row == layout.field.row;
        let (open, close) = if field { ('[', ']') } else { (' ', ' ') };
        let side = (inner.column - outer.column).saturating_sub(1);
        let pad = " ".repeat(side);
        lines.push(format!("{edge}{pad}{open}{text}{close}{pad}{edge}"));
        if marks.iter().any(|&mark| mark != ' ') {
            let marks = marks.into_iter().collect::<String>();
            lines.push(format!(":{pad} {marks} {pad}:"));
        }
        if row == layout.input.row {
            lines.extend(draw_answer_region(editor, inner, &pad));
        }
    }
    draw_candidates(editor, edge, lines);
    let regions = layout.regions;
    if regions.candidates_padded() || regions.candidates.rows == 0 && regions.padded_below() {
        lines.push(format!("└{}┘", "─".repeat(outer.columns)));
    }
}

/// A question's keys on `row`, as chips such as `[y]`, and their verbs, from the inner cells'
/// left.
fn draw_keys(
    prompt: &Prompt,
    layout: &PromptLayout,
    row: usize,
    parts: &mut Vec<(usize, String, Vec<char>)>,
) {
    let inner = layout.regions.inner;
    let keys = layout.keys.iter().zip(&layout.verbs).zip(minibuffer::KEYS);
    let verbs = match prompt.kind() {
        Kind::Question { verbs, .. } => verbs,
        _ => ["", ""],
    };
    for (((key, verb), shown), said) in keys.zip(verbs) {
        if row == key.row && key.columns > 0 {
            let chip = format!("[{}]", shown.trim());
            let (text, marks) = spans_row(&[&chip], key.columns);
            parts.push((key.column - inner.column, text, marks));
        }
        if row == verb.row && verb.columns > 0 {
            let (text, marks) = spans_row(&[" ", said], verb.columns);
            parts.push((verb.column - inner.column, text, marks));
        }
    }
}

/// A list's count, such as `3/57`, at the start of its cells, from the inner cells' left.
fn draw_count(
    editor: &Editor,
    prompt: &Prompt,
    layout: &PromptLayout,
) -> Option<(usize, String, Vec<char>)> {
    let candidates = prompt.candidates()?;
    let place = candidates.selected().map_or(0, |place| place + 1);
    let more = if editor.files_cut() { "+" } else { "" };
    let count = if editor.files_pending() {
        "…/…".to_owned()
    } else {
        format!("{place}/{}{more}", candidates.len())
    };
    let (text, marks) = spans_row(&[&count], layout.count.columns);
    Some((
        layout.count.column - layout.regions.inner.column,
        text,
        marks,
    ))
}

/// The line under the answer's row marking its region with `*`, if it shows one.
fn draw_answer_region(editor: &Editor, inner: CellRect, pad: &str) -> Option<String> {
    let mut region = Vec::new();
    editor.prompt_region_cells(&mut region);
    let mut marks = vec![' '; inner.columns];
    for cells in &region {
        let start = cells.column - inner.column;
        marks[start..start + cells.columns].fill('*');
    }
    let marks = marks.into_iter().collect::<String>();
    marks
        .contains('*')
        .then(|| format!("+{pad} {marks} {pad}+"))
}

/// A list's rows below the answer's padding, as the renderer draws them.
fn draw_candidates(editor: &Editor, edge: char, lines: &mut Vec<String>) {
    let (Some(prompt), Some(layout)) = (editor.prompt(), editor.prompt_layout()) else {
        return;
    };
    let (Some(candidates), rows) = (prompt.candidates(), layout.regions.candidates) else {
        return;
    };
    if rows.rows == 0 {
        return;
    }
    let outer = layout.regions.outer;
    let pad = " ".repeat((rows.column - outer.column).saturating_sub(1));
    // A row of padding between the answer and the list, where the float has one.
    let inner = layout.regions.inner;
    if rows.row > inner.row + inner.rows {
        lines.push(format!("{edge}{}{edge}", " ".repeat(outer.columns)));
    }
    let named = candidates
        .shown(rows.rows)
        .map(|(_, candidate)| (candidate.text(), candidate.binding()));
    let column = minibuffer::annotation_column(named, rows.columns);
    let mut highlights = Vec::new();
    for index in 0..rows.rows {
        let place = candidates.top() + index;
        let beside = preview_row(editor, layout, index);
        let Some(candidate) = candidates.get(place) else {
            let blank = " ".repeat(rows.columns);
            lines.push(format!("{edge}{pad} {blank}{beside} {pad}{edge}"));
            continue;
        };
        let (text, binding, note) = (
            candidate.text(),
            candidate.binding(),
            candidate.annotation(),
        );
        let row = minibuffer::candidate_row(text, binding, note, rows.columns, column);
        let shown = &text[..row.text];
        let mut spans = vec![shown.to_owned()];
        let mut used = minibuffer::cells_of(shown);
        if row.text_cut {
            spans.push(minibuffer::CUT.to_owned());
            used += 1;
        }
        if let Some(column) = row.binding {
            spans.push(" ".repeat(column - used) + "(" + binding + ")");
            used = column + minibuffer::cells_of(binding) + 2;
        }
        if let Some((column, bytes, cut)) = row.annotation {
            let cut = if cut { minibuffer::CUT } else { "" };
            spans.push(" ".repeat(column - used) + &note[..bytes] + cut);
        }
        let spans: Vec<&str> = spans.iter().map(String::as_str).collect();
        let (line, _) = spans_row(&spans, rows.columns);
        let side = if candidates.selected() == Some(place) {
            '>'
        } else {
            ' '
        };
        lines.push(format!("{edge}{pad}{side}{line}{beside} {pad}{edge}"));
        candidates.query().highlights(candidate, &mut highlights);
        let mut marks = vec![' '; rows.columns];
        for range in &highlights {
            let (start, end) = (range.start.min(shown.len()), range.end.min(shown.len()));
            let from = minibuffer::cells_of(&shown[..start]);
            let to = minibuffer::cells_of(&shown[..end]).min(rows.columns);
            marks[from..to].fill('*');
        }
        if marks.contains(&'*') {
            lines.push(format!(
                ":{pad} {} {pad}:",
                marks.into_iter().collect::<String>()
            ));
        }
    }
}

/// What goes right of the candidates on their row `index` beside a preview: the column that
/// divides them, then the previewed buffer's row from where its window was, its cursor as `o`,
/// or what is said of a buffer that may hold secrets on the first row (ADR 0088).
fn preview_row(editor: &Editor, layout: &PromptLayout, index: usize) -> String {
    let preview = layout.regions.preview;
    if preview.columns == 0 || index >= preview.rows {
        return String::new();
    }
    let text = match editor.preview() {
        Some(Preview::Buffer { id, top, cursor }) => {
            let Some(buffer) = editor.buffer(id) else {
                return String::new();
            };
            let display = buffer.display();
            let point = display.offset_to_display_point(cursor);
            let scroll = Scroll { top, left: 0 };
            let (place, cells) = motion::cursor_box(&display, point, None);
            let row = top + index;
            let cursor = frame::cursor_rect(preview, scroll, place, cells)
                .filter(|cells| cells.row == preview.row + index)
                .map_or(0..0, |cells| {
                    let start = cells.column - preview.column;
                    start..start + cells.columns
                });
            let style = Style {
                cursor: 'o',
                whitespace: false,
            };
            let (text, marks) =
                draw_row(&display, row, scroll, preview.columns, cursor, None, style);
            // The cursor's mark shows in the text, as the row of marks is the list's.
            text.chars()
                .zip(marks)
                .map(|(c, mark)| if mark == 'o' { 'o' } else { c })
                .collect()
        }
        Some(Preview::Withheld) if index == 0 => {
            let (text, _) = spans_row(&["may hold secrets"], preview.columns);
            text
        }
        _ => " ".repeat(preview.columns),
    };
    format!("│{text}")
}

/// The answer's cells in `area`, as the renderer draws them: the prompt's window, or one `*`
/// per character of a passphrase.
fn draw_input(editor: &Editor, area: CellRect) -> (String, Vec<char>) {
    let (window, buffer) = editor.current();
    let display = buffer.display();
    let scroll = window.scroll();
    let point = window.cursor_point(&display);
    if editor
        .prompt()
        .is_some_and(|prompt| prompt.kind() == Kind::Passphrase)
    {
        let cursor = window.selections.primary().cursor;
        let (stars, column) = minibuffer::mask(buffer.text(), cursor, area.columns);
        let mut marks = vec![' '; area.columns];
        if let Some(mark) = marks.get_mut(column) {
            *mark = '^';
        }
        let text = "*".repeat(stars) + &" ".repeat(area.columns - stars);
        return (text, marks);
    }
    let (place, cells) = motion::cursor_box(&display, point, None);
    let cursor = frame::cursor_rect(area, scroll, place, cells).map_or(0..0, |cells| {
        let start = cells.column - area.column;
        start..start + cells.columns
    });
    let style = Style {
        cursor: '^',
        whitespace: false,
    };
    draw_row(
        &display,
        point.row,
        scroll,
        area.columns,
        cursor,
        None,
        style,
    )
}

/// The tiled window's rows. Beneath a prompt, its cursor is hollow and it shows no preedit.
fn draw_text_area(editor: &Editor, area: CellRect, lines: &mut Vec<String>) {
    let (window, buffer) = editor.tiled();
    let beneath = editor.prompt().is_some();
    let mark = if beneath { 'o' } else { '^' };
    let display = buffer.display();
    let scroll = window.scroll();
    let point = window.cursor_point(&display);
    let preedit = editor.preedit().filter(|_| !beneath);
    // The cursor box as the renderer draws it.
    let (place, cells) = motion::cursor_box(&display, point, preedit);
    let cursor = frame::cursor_rect(area, scroll, place, cells);
    let mut region = Vec::new();
    editor.region_cells(&mut region);
    let mut found = oso_core::editor::MatchCells::default();
    editor.isearch_cells(&mut found);
    let (current, matches) = (found.current, found.others);
    let (mut trailing, band) = (Vec::new(), editor.current_line_cells());
    editor.trailing_cells(&mut trailing);
    let whitespace = editor.visual_aids().whitespace;
    for index in 0..area.rows {
        let row = scroll.top + index;
        let cursor = cursor
            .filter(|cells| cells.row == area.row + index)
            .map_or(0..0, |cells| {
                let start = cells.column - area.column;
                start..start + cells.columns
            });
        let spliced = preedit
            .filter(|_| row == point.row)
            .map(|preedit| (point.column, preedit));
        let style = Style {
            cursor: mark,
            whitespace,
        };
        let (text, marks) = draw_row(&display, row, scroll, area.columns, cursor, spliced, style);
        lines.push(format!("│{text}│"));
        if marks.iter().any(|&mark| mark != ' ') {
            let marks: String = marks.into_iter().collect();
            lines.push(format!(":{marks}:"));
        }
        let mut marks = vec![' '; area.columns];
        // In the renderer's order, so a region shows over a match, and both over the band.
        let band = band.as_slice();
        let fills = [
            (band, 'l'),
            (&trailing[..], 't'),
            (&current, '#'),
            (&matches, '~'),
        ];
        for (cells, mark) in fills.into_iter().chain([(&region[..], '*')]) {
            for cells in cells.iter().filter(|cells| cells.row == area.row + index) {
                let start = cells.column - area.column;
                marks[start..start + cells.columns].fill(mark);
            }
        }
        if marks.iter().any(|&mark| mark != ' ') {
            lines.push(format!("+{}+", marks.into_iter().collect::<String>()));
        }
        if let Some((at, preedit)) = spliced {
            lines.push(underline(at, preedit, scroll.left, area.columns));
        }
    }
}

/// The line under a row with `preedit` at column `at`: `-` under its cells and `=`
/// under the segment being converted, as the renderer's thin and thick underlines.
fn underline(at: usize, preedit: &Preedit, left: usize, columns: usize) -> String {
    let mut marks = vec![' '; columns];
    let mut mark = |cells: Range<usize>, mark: char| {
        for column in cells {
            if let Some(cell) = column.checked_sub(left).and_then(|i| marks.get_mut(i)) {
                *cell = mark;
            }
        }
    };
    let (all, segment) = preedit.underlines(at);
    mark(all, '-');
    if let Some(segment) = segment {
        mark(segment, '=');
    }
    format!("_{}_", marks.into_iter().collect::<String>())
}

/// A region of at most one row, between `edge`s: `spans`, one after the other, up to
/// its last column.
fn draw_spans(spans: &[&str], area: CellRect, edge: char, lines: &mut Vec<String>) {
    if area.rows == 0 {
        return;
    }
    let (text, marks) = spans_row(spans, area.columns);
    lines.push(format!("{edge}{text}{edge}"));
    if marks.iter().any(|&mark| mark != ' ') {
        let marks: String = marks.into_iter().collect();
        lines.push(format!(":{marks}:"));
    }
}

/// `spans`, one after the other, in a row of `columns` cells, and a mark for each.
fn spans_row(spans: &[&str], columns: usize) -> (String, Vec<char>) {
    let mut text = String::new();
    let mut marks = vec![' '; columns];
    let mut drawn = 0;
    let mut next = 0;
    for span in spans {
        for cluster in text_clusters(span, next) {
            next = cluster.column + cluster.cells;
            cluster.for_each_run(|run| {
                let end = (run.column + run.cells).min(columns);
                if run.column < end {
                    draw_run(&run, run.column, &mut marks[run.column..end], &mut text);
                    drawn = end;
                }
            });
        }
    }
    text.extend(std::iter::repeat_n(' ', columns - drawn));
    (text, marks)
}

/// How `bytes` reads in a snapshot's description: no raw invisible characters
/// or invalid bytes.
fn describe(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.utf8_chunks() {
        out.extend(chunk.valid().chars().flat_map(char::escape_debug));
        for byte in chunk.invalid() {
            out.push_str(&format!("\\x{byte:02X}"));
        }
    }
    out
}

/// Turns the visual aids off, so that a scenario shows only the aids it turns on
/// (ADR 0097).
fn aids_off(editor: &mut Editor) {
    editor.set_visual_aids(oso_core::editor::VisualAids {
        whitespace: false,
        current_line: false,
        flash: false,
        line_numbers: false,
    });
}

/// An editor showing `bytes` in a window of `rows` by `columns`, with the visual aids off.
fn open(bytes: &[u8], (rows, columns): (usize, usize)) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_bytes("test", bytes));
    aids_off(&mut editor);
    editor.set_view_size(ViewSize::new(rows, columns));
    editor
}

/// Compares `frames` of a scenario on `bytes` with the snapshot `name`.
fn assert_frames(name: &str, bytes: &[u8], frames: &[String]) {
    let mut settings = insta::Settings::clone_current();
    settings.set_prepend_module_to_snapshot(false);
    settings.set_omit_expression(true);
    settings.set_description(format!("text: {}", describe(bytes)));
    settings.bind(|| insta::assert_snapshot!(name, frames.join("\n\n")));
}

/// Opens `text` in a window of `size` (rows, columns) and checks one frame per
/// entry of `steps`: the keys to press first, on top of the earlier ones.
fn check(name: &str, text: impl AsRef<[u8]>, size: (usize, usize), steps: &[&str]) {
    check_bytes(name, text.as_ref(), size, steps);
}

fn check_bytes(name: &str, bytes: &[u8], size: (usize, usize), steps: &[&str]) {
    check_editor(name, bytes, open(bytes, size), steps);
}

/// As [`check`], with the whole frame given in rows and columns, so that the rows
/// it reserves come out of them.
fn check_frame(name: &str, text: &str, (rows, columns): (usize, usize), steps: &[&str]) {
    let mut editor = Editor::with_buffer(Buffer::from_bytes("test", text.as_bytes()));
    aids_off(&mut editor);
    editor.set_frame_size(FrameSize { rows, columns });
    check_editor(name, text.as_bytes(), editor, steps);
}

fn check_editor(name: &str, bytes: &[u8], mut editor: Editor, steps: &[&str]) {
    let mut dispatcher = Dispatcher::new();
    let mut frames = Vec::new();
    for &keys in steps {
        if !keys.is_empty() {
            let sequence: KeySequence = keys.parse().unwrap();
            dispatcher.handle_keys(&mut editor, sequence.0);
            assert!(dispatcher.pending().is_empty(), "{keys} left a prefix");
        }
        // isearch's count, as its worker would give it before the frame.
        while let Some(job) = editor.take_isearch_job() {
            let mut reports = Vec::new();
            job.run(|report| reports.push(report));
            for report in reports {
                editor.isearch_reported(report);
            }
        }
        let keys = if keys.is_empty() { "none" } else { keys };
        frames.push(format!("keys: {keys}\n{}", text_grid(&editor)));
    }
    assert_frames(name, bytes, &frames);
}

// `cfg(test)` again so that the size limit counts the grid and its helpers, not
// the scenarios, which grow with every visible feature (ADR 0045).
#[cfg(test)]
mod scenarios {
    use super::*;

    #[test]
    fn full_width_and_half_width() {
        check(
            "widths_japanese",
            "日本語abc\nｱｲｳ半角\nＡＢＣ全角\n",
            (4, 16),
            &[""],
        );
    }

    #[test]
    fn emoji_and_combining_marks_take_their_cluster_width() {
        let text = "👨\u{200d}👩\u{200d}👧|family\n🇯🇵|flag\n🍣|sushi\n1\u{fe0f}\u{20e3}|keycap\n\
                    か\u{3099}|voiced\ne\u{301}|acute\n\u{301}|lone mark";
        check("widths_emoji_and_marks", text, (7, 16), &[""]);
    }

    #[test]
    fn tabs_reach_the_next_stop() {
        let text = "a\tb\n\tc\n日本\td\n1234567\te\n12345678\tf";
        check("tabs", text, (5, 24), &["", "C-n C-f C-f"]);
    }

    #[test]
    fn line_breaks_and_lone_carriage_returns() {
        check(
            "line_endings",
            "crlf\r\nlone\rcr\nlf\r\n\r\nend\r",
            (6, 12),
            &["", "C-n C-f C-f C-f C-f"],
        );
    }

    #[test]
    fn control_characters_are_escapes() {
        check(
            "controls",
            "bell\x07 del\x7f nul\0\nc1\u{85} esc\x1b[31m",
            (2, 24),
            &[""],
        );
    }

    /// The keys that run `command` from `M-x`.
    fn m_x(command: &str) -> String {
        let typed: Vec<String> = command.chars().map(String::from).collect();
        format!("M-x {} RET", typed.join(" "))
    }

    #[test]
    fn whitespace_marks_tabs_and_spaces_and_fills_trailing_runs_but_the_cursors() {
        let text = "a\tb\u{3000}c\u{a0}d  \ne\t \nf  \n";
        let on = m_x("global-whitespace-mode");
        check("whitespace_marks", text, (4, 24), &["", &on, "C-n"]);
    }

    #[test]
    fn the_current_line_band_follows_the_primary_cursor() {
        let on = m_x("global-hl-line-mode");
        check(
            "current_line_band",
            "one\ntwo\nthree\n",
            (4, 12),
            &[&on, "C-n"],
        );
    }

    #[test]
    fn invisible_and_bidi_characters_are_boxed() {
        let text =
            "ok\u{202e}evil\nz\u{200b}w\u{200d}j\n\u{2066}iso\u{2069}\nE\u{e0041}tag\na\u{feff}b";
        check("invisible_and_bidi", text, (5, 24), &[""]);
    }

    #[test]
    fn characters_past_a_clusters_limits_are_boxed() {
        let at_limit = format!("e{}", "\u{301}".repeat(8));
        let past_limit = format!("o{}", "\u{301}".repeat(10));
        let text = format!("{at_limit}x\n{past_limit}x\n");
        let steps = ["", "C-n C-f", "C-f", "C-f", "C-f"];
        check("past_cluster_limits", &text, (3, 24), &steps);
    }

    #[test]
    fn invalid_bytes_are_escapes() {
        check(
            "invalid_bytes",
            b"a\xFFb\n\xE3\x81x\n\xC0\xAF\n",
            (4, 16),
            &["", "C-f"],
        );
    }

    #[test]
    fn escapes_differ_from_text_that_reads_the_same() {
        check(
            "escapes_and_lookalikes",
            b"^M \\xFF U+202E\n\r \xFF \xE2\x80\xAE\n".as_slice(),
            (2, 20),
            &[""],
        );
    }

    #[test]
    fn the_cursor_box_covers_its_whole_cluster() {
        let text = "日a👨\u{200d}👩\u{200d}👧🇯🇵か\u{3099}\tb\r\u{202e}";
        let steps = [
            "", "C-f", "C-f", "C-f", "C-f", "C-f", "C-f", "C-f", "C-f", "C-f",
        ];
        check("cursor_box", text, (1, 32), &steps);
        check("cursor_on_invalid_byte", b"a\xFF", (1, 8), &["", "C-f"]);
    }

    #[test]
    fn page_keys_scroll_by_a_screen() {
        let text: String = (0..12).map(|i| format!("line {i:02}\n")).collect();
        let steps = ["", "C-v", "C-v", "C-v", "M-v", "M-v", "M-v", "M->", "M-<"];
        check("page_scrolling", text, (5, 12), &steps);
    }

    #[test]
    fn the_cursor_and_the_view_stop_above_the_reserved_rows() {
        let text: String = (0..12).map(|i| format!("line {i:02}\n")).collect();
        let steps = ["", "C-n C-n C-n", "C-n", "C-v", "M->", "M-<"];
        check_frame("frame_reserved_rows", &text, (6, 12), &steps);
    }

    #[test]
    fn the_sideways_wheel_can_leave_the_cursor_on_a_cluster_cut_at_the_left_edge() {
        let text = "日本語日本語日本語\n";
        let mut editor = open(text.as_bytes(), (1, 10));
        let mut frames = vec![format!("wheel: none\n{}", text_grid(&editor))];
        editor.scroll_view_sideways(1);
        frames.push(format!("sideways: 1\n{}", text_grid(&editor)));
        editor.scroll_view_sideways(1);
        frames.push(format!("sideways: 1, out of view\n{}", text_grid(&editor)));
        assert_frames("cursor_cut_at_the_left_edge", text.as_bytes(), &frames);
    }

    #[test]
    fn a_prompt_hangs_from_the_top_and_its_answer_scrolls_sideways() {
        let text = "# Notes\n\nAgenda\n- budget\n- hiring\n- office\n";
        let steps = [
            "M-g g",
            "4 2",
            "C-a C-b",
            "C-e 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2",
            "C-g",
        ];
        check_frame("prompt_goto_line", text, (10, 64), &steps);
    }

    #[test]
    fn a_question_has_its_keys_on_a_row_and_breaks_when_it_is_long() {
        let text = "notes\n";
        check_frame("prompt_question", text, (8, 64), &["x C-x C-c", "a"]);
        check_frame("prompt_long_question", text, (8, 30), &["x C-x C-c"]);
    }

    #[test]
    fn a_nested_prompt_shows_alone_with_its_depth() {
        check_frame(
            "prompt_nested",
            "notes\n",
            (8, 64),
            &["x M-g g 4 C-x C-c", "C-g"],
        );
    }

    #[test]
    fn a_short_frame_gives_up_the_padding_then_the_gap_then_shares_a_row() {
        check_frame("prompt_in_5_rows", "notes\n", (5, 64), &["M-g g 1"]);
        check_frame("prompt_in_4_rows", "notes\n", (4, 64), &["M-g g 1"]);
        check_frame(
            "prompt_in_3_rows",
            "notes\n",
            (3, 64),
            &["M-g g 1", "C-g x C-x C-c"],
        );
    }

    // A dev build lists its own commands too, so these run in a build without them.
    #[cfg(not(feature = "dev"))]
    #[test]
    fn m_x_lists_commands_below_the_answer_with_their_keys_and_summaries() {
        let text = "# Notes\n\nAgenda\n- budget\n";
        let steps = [
            "M-x",
            "f o r w",
            "C-n",
            "SPC , w",
            "C-a C-SPC C-e",
            "C-g C-g",
        ];
        check_frame("complete_m_x", text, (30, 100), &steps);
    }

    #[test]
    fn c_x_b_previews_the_selected_buffer_but_never_one_that_may_hold_secrets() {
        let mut editor = Editor::with_buffer(Buffer::from_text("plan.md", "budget\nhiring\n"));
        aids_off(&mut editor);
        editor.set_frame_size(FrameSize {
            rows: 24,
            columns: 100,
        });
        let path = std::path::PathBuf::from("/tmp/.env");
        let mut secret = Buffer::visiting(path.clone());
        secret.finish_empty();
        secret.judge_secret(Some(&[path]));
        let secret = editor.add_buffer(secret);
        editor.show(secret);
        let notes = editor.add_buffer(Buffer::from_text("test", "notes\n"));
        editor.show(notes);
        check_editor(
            "buffers_preview",
            b"notes\n",
            editor,
            &["C-x b", "C-n", "C-g"],
        );
    }

    #[test]
    fn c_dot_previews_the_text_around_each_mark() {
        let text = "# Notes\n\nAgenda\n- budget\n- hiring\n";
        let steps = [
            "M-> C-SPC C-SPC M-< C-n C-n C-SPC C-SPC",
            "C-.",
            "C-n",
            "RET",
        ];
        check_frame("marks_c_dot", text, (24, 100), &steps);
    }

    #[test]
    fn c_x_b_lists_buffers_with_what_they_are_and_c_p_selects_what_is_typed() {
        let steps = ["C-x b", "t e", "C-p", "C-g"];
        check_frame("buffers_c_x_b", "notes\n", (24, 100), &steps);
    }

    // A dev build lists its own commands too, so these run in a build without them.
    #[cfg(not(feature = "dev"))]
    #[test]
    fn a_narrow_list_cuts_the_summary_then_drops_it_then_the_key() {
        check_frame("complete_narrow", "notes\n", (12, 44), &["M-x b u f"]);
    }

    // A dev build lists its own commands too, so these run in a build without them.
    #[cfg(not(feature = "dev"))]
    #[test]
    fn a_short_frame_gives_up_candidate_rows_first() {
        check_frame("complete_in_9_rows", "notes\n", (9, 80), &["M-x", "C-v"]);
        check_frame("complete_in_7_rows", "notes\n", (7, 80), &["M-x"]);
        check_frame("complete_in_3_rows", "notes\n", (3, 80), &["M-x"]);
    }

    #[cfg(feature = "dev")]
    #[test]
    fn a_passphrase_shows_one_star_per_character() {
        check_frame(
            "prompt_passphrase",
            "notes\n",
            (8, 64),
            &["<f34> s e c r e t"],
        );
    }

    #[test]
    fn a_small_frame_gives_up_the_reserved_rows_before_the_text() {
        let text = "one\ntwo\nthree\n";
        check_frame("frame_of_3_rows", text, (3, 8), &["", "C-n", "M->"]);
        check_frame("frame_of_2_rows", text, (2, 8), &["", "C-n"]);
        check_frame("frame_of_1_row", text, (1, 8), &["", "C-n"]);
    }

    #[test]
    fn the_view_follows_the_cursor_by_the_fewest_rows() {
        let text: String = (0..8).map(|i| format!("line {i}\n")).collect();
        check(
            "follow_rows",
            text,
            (3, 8),
            &["", "C-n C-n", "C-n", "C-p C-p C-p"],
        );
    }

    #[test]
    fn a_long_line_scrolls_sideways() {
        let text = "0123456789abcdefghijklmnopqrstuvwxyz\nshort\n";
        check(
            "horizontal_scrolling",
            text,
            (3, 12),
            &["", "C-e", "C-b C-b", "C-a", "C-e C-n", "C-p C-e"],
        );
    }

    #[test]
    fn edges_cut_wide_clusters_escapes_and_tabs() {
        let wide = format!("a{}", "あ".repeat(20));
        check("edges_wide", wide, (1, 10), &["", "C-e"]);
        check("edges_label", "\u{202e}\u{202e}", (1, 8), &["", "C-e"]);
        check("edges_tab", "abc\tdef", (1, 5), &["", "C-e"]);
        check(
            "edges_narrower_than_a_cluster",
            "日本",
            (1, 1),
            &["", "C-f"],
        );
    }

    #[test]
    fn an_empty_buffer_shows_only_the_cursor() {
        check("empty_buffer", "", (3, 8), &[""]);
    }

    #[test]
    fn typing_draws_what_was_typed_with_the_cursor_after_it() {
        let steps = [
            "",
            "a b SPC c",
            "日 本",
            "RET 🍣",
            "DEL",
            "C-a TAB",
            "C-z C-z",
            "C-M-z",
        ];
        check("typing", "", (3, 16), &steps);
    }

    #[test]
    fn typing_in_a_line_pushes_its_text_and_a_line_break_its_rows() {
        let steps = ["", "C-f x", "RET", "C-d", "C-z", "C-z", "C-z"];
        check("typing_in_a_line", "ab\ncd\n", (4, 8), &steps);
    }

    #[test]
    fn a_crlf_file_breaks_lines_as_it_does() {
        let steps = ["", "C-f RET", "DEL"];
        check("typing_crlf", "ab\r\ncd\r\n", (3, 8), &steps);
    }

    #[test]
    fn a_typed_mark_joins_its_cluster_and_del_takes_them_together() {
        let steps = ["", "か", "\u{3099}", "x", "DEL", "DEL"];
        check("typing_marks", "", (1, 8), &steps);
    }

    #[test]
    fn typing_past_the_edge_scrolls_sideways() {
        let steps = ["", "a b c d e f", "語", "DEL DEL DEL DEL DEL"];
        check("typing_scrolls_sideways", "012", (1, 8), &steps);
    }

    #[test]
    fn a_window_that_shrinks_and_grows_at_the_end_shows_the_text_again() {
        let text: String = (0..8).map(|i| format!("line {i}\n")).collect();
        let mut editor = open(text.as_bytes(), (4, 8));
        let columns = 8;
        let mut dispatcher = Dispatcher::new();
        dispatcher.handle_keys(&mut editor, "M->".parse::<KeySequence>().unwrap().0);
        let mut frames = vec![format!("keys: M->\n{}", text_grid(&editor))];
        for rows in [2, 6] {
            editor.set_frame_size(FrameSize { rows, columns });
            frames.push(format!("frame rows: {rows}\n{}", text_grid(&editor)));
        }
        assert_frames("frame_grows_at_the_end", text.as_bytes(), &frames);
    }

    #[test]
    fn the_wheel_moves_the_view_and_takes_the_cursor_along() {
        let text: String = (0..8).map(|i| format!("line {i}\n")).collect();
        let mut editor = open(text.as_bytes(), (3, 8));
        let mut frames = vec![format!("wheel: none\n{}", text_grid(&editor))];
        for rows in [2, 100, -1, -100] {
            editor.scroll_view(rows);
            frames.push(format!("wheel: {rows}\n{}", text_grid(&editor)));
        }
        assert_frames("wheel", text.as_bytes(), &frames);
    }

    #[test]
    fn the_wheel_takes_a_cursor_it_moves_back_into_view_sideways() {
        let text: String = (0..8)
            .map(|i| format!("line {i} of some length\n"))
            .collect();
        let mut dispatcher = Dispatcher::new();
        let mut editor = open(text.as_bytes(), (3, 8));
        let keys: KeySequence = "C-n C-e".parse().unwrap();
        dispatcher.handle_keys(&mut editor, keys.0);
        let mut frames = vec![format!("keys: C-n C-e\n{}", text_grid(&editor))];
        editor.scroll_view(1);
        frames.push(format!(
            "wheel: 1, the cursor stays\n{}",
            text_grid(&editor)
        ));
        editor.scroll_view(1);
        frames.push(format!("wheel: 1, it moves\n{}", text_grid(&editor)));
        assert_frames("wheel_with_the_view_sideways", text.as_bytes(), &frames);
    }

    #[test]
    fn the_sideways_wheel_moves_the_view_and_the_next_key_comes_back() {
        let text = "日本語の文章で書いている長い行です\nabc\n";
        let mut dispatcher = Dispatcher::new();
        let mut editor = open(text.as_bytes(), (3, 12));
        let mut frames = vec![format!("wheel: none\n{}", text_grid(&editor))];
        for columns in [5, 100, -3] {
            editor.scroll_view_sideways(columns);
            frames.push(format!("sideways: {columns}\n{}", text_grid(&editor)));
        }
        let keys: KeySequence = "C-f".parse().unwrap();
        dispatcher.handle_keys(&mut editor, keys.0);
        frames.push(format!("keys: C-f\n{}", text_grid(&editor)));
        assert_frames("wheel_sideways", text.as_bytes(), &frames);
    }

    #[test]
    fn word_motion_stops_on_whole_clusters_and_deletes_them() {
        let steps = ["", "M-f", "M-f", "M-f", "M-b", "M-d", "C-z", "M-DEL"];
        check(
            "word_motion",
            "か\u{3099}き語の文章で😀 end",
            (1, 24),
            &steps,
        );
    }

    #[test]
    fn word_motion_reaches_a_past_limit_box_and_passes_it() {
        let marks = "\u{301}".repeat(10);
        let steps = ["", "M-f", "C-b", "C-b", "M-b", "M-f", "M-DEL"];
        check(
            "word_motion_past_limit",
            format!("a{marks}b c"),
            (1, 32),
            &steps,
        );
    }

    #[test]
    fn c_a_and_c_e_toggle_where_the_cursor_goes() {
        let steps = ["", "C-a", "C-a", "C-e", "C-e", "C-a"];
        check("mwim", "\t  code  \n", (2, 20), &steps);
    }

    #[test]
    fn line_motion_stays_at_line_ends() {
        let steps = ["", "C-e", "C-n", "C-n", "C-n", "C-p", "C-a"];
        check("track_eol", "short\n\nmuch longer\n", (4, 16), &steps);
    }

    #[test]
    fn a_count_repeats_what_a_key_does_on_screen() {
        let steps = [
            "",
            "C-u x",
            "C-u 3 y",
            "C-u 2 C-u 5",
            "M-- C-d",
            "C-u C-b",
            "M-2 DEL",
        ];
        check("prefix_argument", "", (2, 20), &steps);
    }

    #[test]
    fn c_l_cycles_where_the_cursors_row_goes() {
        let text: String = (0..20).map(|i| format!("line {i}\n")).collect();
        let steps = ["", "C-u 8 C-n", "C-l", "C-l", "C-l", "M-1 C-l", "C-u C-l"];
        check("recenter", text, (5, 8), &steps);
    }

    #[test]
    fn the_modeline_says_where_the_cursor_is_and_the_echo_area_what_failed() {
        check_frame(
            "status_modeline_and_echo",
            "ab\ncd\n",
            (5, 40),
            &["", "C-n C-f", "M-> C-f", "C-x C-z", "C-u 5", "C-g"],
        );
    }

    #[test]
    fn a_small_frame_keeps_its_warnings_as_long_as_it_can() {
        let text = "a\u{202e}b\n";
        let mut frames = Vec::new();
        let mut editor = Editor::with_buffer(Buffer::from_bytes("test", text.as_bytes()));
        aids_off(&mut editor);
        editor.message("Wrote test");
        for rows in [4, 3, 2, 1, 0] {
            editor.set_frame_size(FrameSize { rows, columns: 40 });
            frames.push(format!("frame rows: {rows}\n{}", text_grid(&editor)));
        }
        assert_frames("status_small_frame", text.as_bytes(), &frames);
    }

    #[test]
    fn a_long_name_cannot_push_a_warning_off_the_row() {
        let name = format!("{}.md", "very-long-name-".repeat(10));
        let mut editor = Editor::with_buffer(Buffer::from_text(name, "\u{2066}\n"));
        aids_off(&mut editor);
        editor.set_frame_size(FrameSize {
            rows: 4,
            columns: 50,
        });
        let frames = [text_grid(&editor)];
        assert_frames("status_long_name", "\u{2066}\n".as_bytes(), &frames);
    }

    #[test]
    fn a_name_from_disk_is_escaped_on_the_modeline() {
        let name = "a\nb\u{202e}\u{1b}c\tいe\u{301}.txt";
        let mut editor = Editor::with_buffer(Buffer::from_text(name, ""));
        aids_off(&mut editor);
        editor.message(format!("Wrote {name}"));
        editor.set_frame_size(FrameSize {
            rows: 3,
            columns: 56,
        });
        assert_frames(
            "status_escaped_name",
            name.as_bytes(),
            &[text_grid(&editor)],
        );
    }

    #[test]
    fn a_secret_buffers_modeline_says_no_position() {
        let path = std::path::PathBuf::from("/tmp/.env");
        let mut buffer = Buffer::visiting(path.clone());
        let text = b"KEY=value\n";
        buffer.apply_load(oso_core::load::decode_all(text, Default::default()));
        buffer.judge_secret(Some(&[path]));
        let mut editor = Editor::with_buffer(buffer);
        aids_off(&mut editor);
        editor.set_frame_size(FrameSize {
            rows: 4,
            columns: 40,
        });
        assert_frames("status_secret_buffer", text, &[text_grid(&editor)]);
    }

    #[test]
    fn a_file_with_an_emergency_copy_says_so_first() {
        let path = std::path::PathBuf::from("/tmp/notes.txt");
        let mut buffer = Buffer::visiting(path.clone());
        let text = b"on disk\n";
        buffer.apply_load(oso_core::load::decode_all(text, Default::default()));
        buffer.judge_secret(Some(&[path]));
        buffer.set_copy_waits(true);
        let mut editor = Editor::with_buffer(buffer);
        aids_off(&mut editor);
        editor.set_frame_size(FrameSize {
            rows: 4,
            columns: 60,
        });
        let full = text_grid(&editor);
        editor.set_frame_size(FrameSize {
            rows: 2,
            columns: 60,
        });
        assert_frames("status_emergency_copy", text, &[full, text_grid(&editor)]);
    }

    #[test]
    fn the_list_of_emergency_copies_shows_each_row_in_one_row() {
        let rows = concat!(
            "2 emergency copies.  v look  = compare  r recover  d delete  g refresh  q back\n\n",
            "notes.txt            12 min ago  panic    1234 B  file unchanged   /home/me/notes.txt\n",
            "a\\nb\\u{202e}.txt     2 days ago  ?          88 B  not open         /home/me/a\\nb\\u{202e}.txt\n",
        );
        let mut editor = Editor::default();
        aids_off(&mut editor);
        editor.set_frame_size(FrameSize {
            rows: 6,
            columns: 110,
        });
        let mut dispatcher = Dispatcher::new();
        let keys: KeySequence = "C-h r".parse().unwrap();
        dispatcher.handle_keys(&mut editor, keys.0);
        let list = oso_core::load::decode_all(rows.as_bytes(), Default::default());
        editor.set_copies_text(list, 2, None);
        assert_frames(
            "emergency_copies_list",
            rows.as_bytes(),
            &[text_grid(&editor)],
        );
    }

    #[test]
    fn a_preedit_pushes_its_row_and_the_box_and_view_follow_it() {
        let text = "ab日c\nnext\n";
        let mut editor = open(text.as_bytes(), (2, 12));
        let mut dispatcher = Dispatcher::new();
        let keys: KeySequence = "C-f C-f".parse().unwrap();
        dispatcher.handle_keys(&mut editor, keys.0);
        let mut frames = Vec::new();
        let mut show = |editor: &mut Editor, step: &str, text: &str, range| {
            editor.set_preedit(Some(Preedit::new(text.into(), range)));
            frames.push(format!("{step}\n{}", text_grid(editor)));
        };
        show(&mut editor, "typing", "にほ", Some(6..6));
        show(&mut editor, "caret inside", "にほ", Some(3..3));
        show(
            &mut editor,
            "converting the second clause",
            "日本語",
            Some(6..9),
        );
        show(
            &mut editor,
            "wider than the window",
            "にほんごにゅうりょく",
            None,
        );
        editor.set_preedit(None);
        frames.push(format!("cleared\n{}", text_grid(&editor)));
        assert_frames("preedit_in_its_row", text.as_bytes(), &frames);
    }

    #[test]
    fn a_region_takes_its_cells_and_its_line_breaks_to_the_edge() {
        check_frame(
            "region_across_rows",
            "ab日本c\n\nline three\n",
            (6, 12),
            &[
                "C-f C-SPC",
                "C-n C-n C-f",
                "C-x C-x",
                "C-SPC C-SPC",
                "C-x h",
                "C-g",
            ],
        );
    }

    #[test]
    fn isearch_highlights_its_matches_and_says_its_line() {
        check_frame(
            "isearch_matches",
            "foo bar foo\nbaz foo\nFoo\n",
            (6, 30),
            &[
                "C-s f o",
                "C-s",
                "C-s C-s C-s",
                "C-s",
                "RET",
                "C-s O",
                "M-c",
                "C-g",
            ],
        );
    }

    #[test]
    fn shift_selects_until_a_motion_without_it() {
        check(
            "region_by_shift",
            "one two three\nfour\n",
            (3, 16),
            &[
                "S-<right> S-<right>",
                "M-F",
                "C-N",
                "C-f",
                "C-SPC S-<right> C-f",
            ],
        );
    }

    #[test]
    fn a_region_is_cut_at_the_window_and_scrolls_with_it() {
        let long = format!("{}|end\nnext\n", "x".repeat(30));
        check(
            "region_scrolled_sideways",
            &long,
            (2, 10),
            &["C-SPC C-e", "C-n"],
        );
    }

    #[test]
    fn a_preedit_pushes_a_region_after_it() {
        let text = "abcdef\n";
        let mut editor = open(text.as_bytes(), (1, 14));
        let mut dispatcher = Dispatcher::new();
        let mut frames = Vec::new();
        for (keys, step) in [("C-e C-SPC C-a C-f", "after"), ("C-SPC C-e", "before")] {
            let keys: KeySequence = keys.parse().unwrap();
            dispatcher.handle_keys(&mut editor, keys.0);
            editor.set_preedit(Some(Preedit::new("かな".into(), None)));
            frames.push(format!(
                "a region {step} the preedit\n{}",
                text_grid(&editor)
            ));
            editor.set_preedit(None);
        }
        assert_frames("region_and_preedit", text.as_bytes(), &frames);
    }

    #[test]
    fn the_messages_buffer_shows_what_was_said() {
        check_frame(
            "status_messages_buffer",
            "ab\n",
            (5, 40),
            &["C-b", "C-h e", "C-h e"],
        );
    }
}
