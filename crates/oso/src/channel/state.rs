//! What the agent channel shows an agent (ADR 0085): a small state of its own, with what
//! `get-state` withholds around a buffer that may hold secrets withheld the same way
//! (ADR 0044, ADR 0058, ADR 0063, ADR 0079), and the commands it may run. A buffer withheld
//! from agents gives neither its text nor its positions either (ADR 0121).
//!
//! Must not depend on: sockets, threads or `app`; these are plain functions of the editor.
//!
//! It never holds keys in progress, a composition, what is typed in a prompt, the kill
//! ring, the clipboard, secure input or timings.

use std::ops::Range;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::command::{ArgSpec, BufferPurpose, Command, Reach};
use oso_core::display_map::{DisplayPoint, DisplaySnapshot};
use oso_core::editor::{Editor, Status};
use oso_core::minibuffer::Kind;
use oso_core::rope::Rope;
use oso_core::sum_tree::Bias;
use oso_core::window::Window;
use serde_json::{Value, json};

/// The state's version. Bump it on incompatible changes.
pub const STATE_VERSION: u32 = 1;

/// The most text of the selected window's rows the state holds, in bytes.
pub const MAX_VISIBLE_TEXT: usize = 64 * 1024;

/// The most selections of one window the state lists; the rest are counted.
pub const MAX_SELECTIONS: usize = 256;

/// The most buffers one state lists; `buffer_offset` asks for the rest, so that a state
/// always fits a response line.
pub const MAX_BUFFERS: usize = 100;

/// Whether the channel may not read `buffer`'s text or where its cursors are: it may hold
/// secrets, or it is withheld from agents (ADR 0121).
pub fn unreadable(buffer: &Buffer) -> bool {
    buffer.is_secret() || buffer.withheld_from_agents()
}

/// The state: every buffer the person has, every tiled window, the selected window's rows,
/// the echo area, the prompts open, the outermost first, and `menu`, the name of the menu the
/// person has open, or null (ADR 0117).
pub fn state(editor: &Editor, menu: Option<&str>, buffer_offset: usize) -> Value {
    let shows_secret = editor.shows_secret();
    let (window, buffer) = editor.tiled();
    let mut state = json!({
        "version": STATE_VERSION,
        // A buffer the channel is still opening is not there yet for the agent: its open
        // checks it first (ADR 0094).
        "buffers": editor
            .buffers()
            .filter(|(_, buffer)| !buffer.agent_pending())
            .skip(buffer_offset)
            .take(MAX_BUFFERS)
            .map(|(id, buffer)| buffer_state(id, buffer, shows_secret))
            .collect::<Vec<_>>(),
        "buffer_offset": buffer_offset,
        "buffer_count": editor.buffers().filter(|(_, b)| !b.agent_pending()).count(),
        "windows": editor
            .frame()
            .windows()
            .iter()
            .filter_map(|(id, regions)| Some((editor.window_by_id(*id)?, regions)))
            .map(|(window, regions)| window_state(editor, window, regions))
            .collect::<Vec<_>>(),
        "text": (!unreadable(buffer)).then(|| visible_text(window, buffer.text(), &buffer.display())),
        "undoing": undoing_state(editor),
        // A menu's name is the configuration's, never a buffer's text.
        "menu": menu,
        "prompts": editor.prompts().iter().map(|prompt| {
            let mut state = json!({
                "kind": kind(prompt.kind()),
                "by": if prompt.from_agent() { "agent" } else { "person" },
            });
            // Only the questions the agent caused say what they ask; a person's prompt
            // may be about what they type.
            if prompt.from_agent() && !prompt.around_secret() {
                state["label"] = json!(prompt.label());
            }
            state
        }).collect::<Vec<_>>(),
    });
    // Keys in progress show in the echo area, and say what and when the person types.
    if !shows_secret
        && !editor.echo_withheld()
        && !editor.keys_withheld()
        && !editor.shows_keys_in_progress()
    {
        let mut status = Status::default();
        editor.status_into(editor.tiled().0.id(), &mut status);
        state["echo"] = json!(status.echo);
    }
    state
}

/// The `undoing` field of `get-state`: null, or the undo or the redo of a large step under way,
/// which way and its share done (ADR 0119). Around a buffer that may hold secrets, only that
/// one is under way.
pub fn undoing_state(editor: &Editor) -> Value {
    let Some(crossing) = editor.crossing() else {
        return Value::Null;
    };
    if editor.shows_secret() || editor.current().1.is_secret() {
        return json!({ "active": true });
    }
    json!({ "redo": crossing.redo(), "done": crossing.done() })
}

/// Begins undoing a step of 10,000 replacements in the selected window's buffer, which takes
/// more than one slice, for tests elsewhere that need an undo under way.
#[cfg(test)]
pub(crate) fn begin_a_large_undo(editor: &mut Editor) {
    let id = editor.current().0.buffer();
    let text = "ab ".repeat(10_000);
    editor.begin_undo_group();
    editor
        .apply_edits(
            id,
            &[oso_core::buffer::Replacement {
                range: 0..0,
                text: &text,
            }],
        )
        .expect("an insertion");
    let edits: Vec<_> = (0..10_000)
        .map(|at| oso_core::buffer::Replacement {
            range: at * 3..at * 3 + 2,
            text: "c",
        })
        .collect();
    editor.end_undo_group();
    editor.begin_undo_group();
    editor.apply_edits(id, &edits).expect("the replacements");
    editor.end_undo_group();
    editor.begin_crossing(1).expect("an undo");
    assert!(editor.crossing().is_some(), "more than one slice");
}

/// A buffer's metadata, which says nothing that follows what a person does in a buffer that
/// may hold secrets: whether it wraps, which a person toggles there, is withheld while one is
/// shown (ADR 0079).
fn buffer_state(id: BufferId, buffer: &Buffer, shows_secret: bool) -> Value {
    let shown = !(buffer.is_secret() && shows_secret);
    json!({
        "id": id.get(),
        "name": buffer.name(),
        "path": buffer.path().map(|path| path.to_string_lossy()),
        "secret": buffer.is_secret(),
        "read_only": buffer.read_only().is_some(),
        "loading": buffer.loading().is_some(),
        "wrapped": shown.then(|| buffer.wraps()),
    })
}

/// A tiled window, in the tree's order (ADR 0112): its id, its cells, gutter, text and
/// modeline together, so that a secret buffer's gutter says nothing of its lines (ADR 0097),
/// and its positions only if what it shows may hold no secrets. The selected one is the
/// selected tiled window, also beneath a prompt. New fields keep the state's version 1
/// (ADR 0085).
fn window_state(
    editor: &Editor,
    window: &Window,
    regions: &oso_core::frame::WindowRegions,
) -> Value {
    let shown = editor.shown(window);
    let area = regions.area();
    let mut state = json!({
        "id": window.id().get(),
        "area": {
            "row": area.row,
            "column": area.column,
            "rows": area.rows + regions.modeline.rows,
            "columns": area.columns,
        },
        "buffer": window.buffer().get(),
        "selected": window.id() == editor.tiled().0.id(),
    });
    if !unreadable(shown) {
        let display = shown.display();
        let point = |offset| {
            let point = display.offset_to_display_point(offset);
            json!({ "row": point.row, "column": point.column })
        };
        state["first_row"] = json!(first_line(window, shown.text(), &display));
        state["selection_count"] = json!(window.selections.len());
        state["selections"] = window
            .selections
            .iter()
            .take(MAX_SELECTIONS)
            .map(|selection| {
                json!({
                    "cursor": point(selection.cursor),
                    "mark": selection.mark.map(point),
                })
            })
            .collect();
        state["region"] = json!(window.selections.mark_active().is_active());
    }
    state
}

/// What a prompt reads, as the dev socket's state and the channel's name it.
pub const fn kind(kind: Kind) -> &'static str {
    match kind {
        Kind::Number { .. } => "number",
        Kind::Text { .. } => "text",
        Kind::Passphrase => "passphrase",
        Kind::Complete { .. } => "complete",
        Kind::Question { .. } => "question",
    }
}

/// The rows the window shows in full, at most [`MAX_VISIBLE_TEXT`].
fn visible_text(window: &Window, text: &Rope, display: &DisplaySnapshot) -> Value {
    let (rows, truncated) = visible_rows(window, text, display, MAX_VISIBLE_TEXT);
    let first = first_line(window, text, display);
    json!({ "first_row": first, "rows": rows, "truncated": truncated })
}

/// The logical line that holds the window's top. The agent's rows are logical lines, so that
/// its view does not depend on a window's width (ADR 0100).
fn first_line(window: &Window, text: &Rope, display: &DisplaySnapshot) -> usize {
    let start = window.scroll().start.min(text.len());
    display.offset_to_display_point(start).row
}

/// The rows the window shows in full, from its top row and left column, cut at its right
/// edge, and whether `budget` bytes cut them short. A wide character across either edge is
/// included. The dev socket's state shares it (ADR 0044).
pub fn visible_rows(
    window: &Window,
    text: &Rope,
    display: &DisplaySnapshot,
    budget: usize,
) -> (Vec<String>, bool) {
    let size = window.size();
    let scroll = window.scroll();
    let top = first_line(window, text, display);
    let end_row = top.saturating_add(size.rows).min(display.row_count());
    let right = scroll.left.saturating_add(size.columns);
    let mut budget = budget;
    let mut rows = Vec::new();
    for row in top..end_row {
        let start =
            display.display_point_to_offset(DisplayPoint::new(row, scroll.left), Bias::Left);
        let end = display.display_point_to_offset(DisplayPoint::new(row, right), Bias::Right);
        if end - start > budget {
            rows.push(slice(text, start..start + budget));
            return (rows, true);
        }
        budget -= end - start;
        rows.push(slice(text, start..end));
    }
    (rows, false)
}

/// The text of `range`, as one string.
pub fn slice(text: &Rope, range: Range<usize>) -> String {
    text.chunks_in_range(range).collect()
}

/// The commands an agent may run: name, the first line of the docstring, what argument it
/// takes, and its reach.
pub fn commands<'a>(commands: impl Iterator<Item = &'a Command>) -> Value {
    commands
        .filter(|command| command.reach != Reach::Person)
        .map(|command| {
            json!({
                "name": command.name,
                "doc": command.doc.lines().next().unwrap_or_default(),
                "arg": match command.args {
                    ArgSpec::None => "none",
                    ArgSpec::Count => "count",
                    ArgSpec::Number { .. } => "number",
                    ArgSpec::Text { .. } => "text",
                    ArgSpec::TextOnPrefix { .. } => "optional text",
                    // With no prefix it reads nothing, and the channel gives no prefix.
                    ArgSpec::DirectoryAndTextOnPrefix { .. } => "none",
                    // The name of a buffer `C-x b` lists (ADR 0094).
                    ArgSpec::Buffer {
                        purpose: BufferPurpose::Switch,
                        ..
                    } => "buffer",
                    // A kill's buffer, a mark or a path is read in a prompt, which the channel
                    // never opens.
                    ArgSpec::Passphrase { .. }
                    | ArgSpec::CountAndClipboard
                    | ArgSpec::Command { .. }
                    | ArgSpec::Theme { .. }
                    | ArgSpec::Listed { .. }
                    | ArgSpec::Menu { .. }
                    | ArgSpec::Buffer { .. }
                    | ArgSpec::Mark { .. }
                    | ArgSpec::File { .. }
                    | ArgSpec::ProjectFile { .. }
                    | ArgSpec::ProjectSearch { .. }
                    | ArgSpec::KillEntry { .. }
                    | ArgSpec::Replace { .. }
                    | ArgSpec::Key => "person",
                },
                "reach": if command.reach == Reach::Change { "change" } else { "look" },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{MAX_VISIBLE_TEXT, begin_a_large_undo, commands, state};
    use oso_core::buffer::Buffer;
    use oso_core::editor::Editor;
    use oso_core::window::ViewSize;
    use serde_json::json;
    use std::path::PathBuf;

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", text));
        editor.set_view_size(ViewSize::new(10, 40));
        editor
    }

    #[test]
    fn a_file_the_channel_is_still_opening_is_not_listed() {
        let mut editor = editor("one");
        let mut opening = Buffer::visiting(PathBuf::from("/tmp/plain.txt"));
        opening.set_agent_pending(true);
        editor.add_buffer(opening);
        let state = state(&editor, None, 0);
        let names: Vec<_> = state["buffers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|buffer| buffer["name"].clone())
            .collect();
        assert!(!names.contains(&json!("plain.txt")), "{names:?}");
        assert_eq!(state["buffer_count"], names.len());
    }

    #[test]
    fn says_how_far_an_undo_under_way_is_but_not_around_a_secret() {
        let mut editor = editor("");
        assert_eq!(state(&editor, None, 0)["undoing"], json!(null));
        begin_a_large_undo(&mut editor);
        let undoing = &state(&editor, None, 0)["undoing"];
        assert_eq!(undoing["redo"], false);
        assert!(
            undoing["done"]
                .as_f64()
                .is_some_and(|done| done > 0.0 && done < 1.0)
        );
        editor.cancel_crossing();
        let path = PathBuf::from("/tmp/.env");
        let mut secret = Buffer::visiting(path.clone());
        secret.apply_load(oso_core::load::decode_all(
            b"",
            oso_core::display_map::TabWidth::default(),
        ));
        secret.judge_secret(Some(&[path]));
        let secret = editor.add_buffer(secret);
        editor.show(secret);
        begin_a_large_undo(&mut editor);
        assert_eq!(
            state(&editor, None, 0)["undoing"],
            json!({ "active": true })
        );
    }

    #[test]
    fn shows_buffers_windows_and_the_rows_in_view() {
        let state = state(&editor("one\ntwo"), None, 0);
        assert_eq!(state["version"], 1);
        assert_eq!(state["buffers"][0]["name"], "notes.md");
        assert_eq!(state["buffers"][0]["secret"], false);
        assert_eq!(state["windows"][0]["selected"], true);
        assert_eq!(
            state["windows"][0]["selections"][0]["cursor"],
            json!({ "row": 0, "column": 0 })
        );
        assert_eq!(state["text"]["rows"], json!(["one", "two"]));
        assert_eq!(state["prompts"], json!([]));
        for left_out in ["kill_ring", "keys", "ime", "modeline", "surface"] {
            assert!(state.get(left_out).is_none(), "{left_out}");
        }
    }

    #[test]
    fn split_windows_come_in_the_trees_order_with_their_ids_and_cells_and_version_1() {
        let lines = |n: usize| "x\n".repeat(n);
        let shown = |n: usize| {
            let path = PathBuf::from("/tmp/.env");
            let mut secret = Buffer::visiting(path.clone());
            secret.apply_load(oso_core::load::decode_all(
                lines(n).as_bytes(),
                oso_core::display_map::TabWidth::default(),
            ));
            secret.judge_secret(Some(&[path]));
            let mut editor = editor("one\ntwo");
            editor.set_frame_size(oso_core::frame::FrameSize {
                rows: 20,
                columns: 41,
            });
            editor.set_visual_aids(oso_core::editor::VisualAids {
                line_numbers: true,
                ..oso_core::editor::VisualAids::default()
            });
            editor
                .split_window(oso_core::frame::Way::Across, false)
                .unwrap();
            let secret = editor.add_buffer(secret);
            editor.show(secret);
            editor.other_window(1);
            state(&editor, None, 0)
        };
        let state = shown(3);
        assert_eq!(state["version"], 1, "new fields keep the version");
        let windows = state["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(
            (windows[0]["id"].clone(), windows[1]["id"].clone()),
            (json!(0), json!(1))
        );
        assert_eq!(windows[0]["selected"], false, "the secret one, at the left");
        assert_eq!(windows[1]["selected"], true);
        assert_eq!(
            windows[0]["area"],
            json!({ "row": 0, "column": 0, "rows": 19, "columns": 20 })
        );
        assert_eq!(windows[1]["area"]["column"], 21, "past the divider");
        for withheld in ["first_row", "selections", "selection_count", "region"] {
            assert!(windows[0].get(withheld).is_none(), "{withheld}");
            assert!(windows[1].get(withheld).is_some(), "{withheld}");
        }
        // Its cells say nothing of how many lines it has.
        assert_eq!(shown(2000)["windows"][0]["area"], windows[0]["area"]);
    }

    #[test]
    fn a_buffer_that_may_hold_secrets_gives_no_text_or_positions() {
        let mut editor = Editor::with_buffer(Buffer::visiting(PathBuf::from("/tmp/.env")));
        editor.set_view_size(ViewSize::new(10, 40));
        assert!(editor.shows_secret());
        let state = state(&editor, None, 0);
        assert_eq!(state["text"], json!(null));
        assert_eq!(state["buffers"][0]["secret"], true);
        assert!(state["windows"][0].get("selections").is_none());
        assert!(state["windows"][0].get("first_row").is_none());
        assert!(state.get("echo").is_none());
    }

    #[test]
    fn a_wrapped_buffer_that_may_hold_secrets_gives_no_row_or_whether_it_wraps() {
        let path = PathBuf::from("/tmp/.env");
        let mut buffer = Buffer::visiting(path.clone());
        buffer.apply_load(oso_core::load::decode_all(
            "KEY ".repeat(300).as_bytes(),
            Default::default(),
        ));
        buffer.judge_secret(Some(&[path]));
        let mut editor = Editor::with_buffer(buffer);
        editor.set_view_size(ViewSize::new(10, 40));
        let id = editor.window().buffer();
        assert!(editor.set_wraps(id, true));
        let state = state(&editor, None, 0);
        assert_eq!(state["text"], json!(null));
        assert!(state["windows"][0].get("first_row").is_none());
        assert_eq!(state["buffers"][0]["wrapped"], json!(null));
    }

    #[test]
    fn a_wrapped_buffer_says_so_and_its_first_row_is_a_logical_line() {
        let mut editor = editor(&format!("{}\nnext\n", "word ".repeat(100)));
        let id = editor.window().buffer();
        assert!(editor.set_wraps(id, true));
        assert!(editor.scroll_lines(3).is_ok());
        let state = state(&editor, None, 0);
        assert_eq!(state["buffers"][0]["wrapped"], true);
        assert_eq!(state["windows"][0]["first_row"], 0, "still the long line");
        assert_eq!(state["text"]["first_row"], 0);
    }

    #[test]
    fn the_state_names_the_open_menu_or_none() {
        let editor = editor("one");
        assert_eq!(state(&editor, None, 0)["menu"], json!(null));
        assert_eq!(state(&editor, Some("toggles"), 0)["menu"], "toggles");
    }

    #[test]
    fn keys_in_progress_never_show() {
        let mut editor = editor("ab");
        editor.message("Saved");
        assert_eq!(state(&editor, None, 0)["echo"], "Saved");
        editor.set_keys_in_progress("C-u 5");
        assert!(state(&editor, None, 0).get("echo").is_none());
    }

    #[test]
    fn buffers_come_a_page_at_a_time() {
        let mut editor = editor("");
        for n in 0..150 {
            editor.add_buffer(Buffer::from_text(format!("b{n}"), ""));
        }
        let first = state(&editor, None, 0);
        let count = first["buffer_count"].as_u64().unwrap();
        assert!(count > 150);
        assert_eq!(
            first["buffers"].as_array().unwrap().len(),
            super::MAX_BUFFERS
        );
        let rest = state(&editor, None, super::MAX_BUFFERS);
        let left = usize::try_from(count).unwrap() - super::MAX_BUFFERS;
        assert_eq!(rest["buffers"].as_array().unwrap().len(), left);
        assert_eq!(rest["buffer_offset"], super::MAX_BUFFERS);
    }

    #[test]
    fn the_text_stops_at_its_limit() {
        let long = "a".repeat(MAX_VISIBLE_TEXT * 2);
        let mut editor = editor(&format!("{long}\n{long}"));
        editor.set_view_size(ViewSize::new(10, MAX_VISIBLE_TEXT * 2));
        let state = state(&editor, None, 0);
        assert_eq!(state["text"]["truncated"], true);
        let rows = state["text"]["rows"].as_array().unwrap();
        assert!(
            rows.iter()
                .map(|row| row.as_str().unwrap().len())
                .sum::<usize>()
                <= MAX_VISIBLE_TEXT
        );
    }

    #[test]
    fn a_persons_prompt_says_only_its_kind_and_an_agents_its_label() {
        let mut editor = editor("");
        editor.ask("Exit?", "save-buffers-kill-terminal").unwrap();
        editor.for_agent(|editor| editor.ask("Save over notes.md?", "save-buffer").unwrap());
        let prompts = state(&editor, None, 0)["prompts"].clone();
        assert_eq!(prompts[0], json!({ "kind": "question", "by": "person" }));
        assert_eq!(prompts[1]["by"], "agent");
        assert!(
            prompts[1]["label"]
                .as_str()
                .unwrap()
                .contains("Agent: Save over notes.md?")
        );
    }

    #[test]
    fn lists_no_command_that_needs_a_person() {
        use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
        fn run(_: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
            Ok(())
        }
        let command = |name, args, reach| Command {
            name,
            doc: "First line.\n\nMore.",
            args,
            reach,
            run,
            state: None,
        };
        let all = [
            command("forward-word", ArgSpec::Count, Reach::Look),
            command("save-buffer", ArgSpec::None, Reach::Change),
            command("yank", ArgSpec::CountAndClipboard, Reach::Person),
        ];
        let listed = commands(all.iter());
        assert_eq!(
            listed,
            json!([
                { "name": "forward-word", "doc": "First line.", "arg": "count", "reach": "look" },
                { "name": "save-buffer", "doc": "First line.", "arg": "none", "reach": "change" },
            ])
        );
    }
}
