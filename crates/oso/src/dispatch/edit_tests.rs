//! Scenario tests of the editing commands, with their keys, through the
//! [`Dispatcher`] (ADR 0037), which holds the undo groups.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every
//! OS with no window.

use oso_core::buffer::{Buffer, Replacement};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Effects};
use oso_core::display_map::DisplayPoint;
use oso_core::editor::Editor;
use oso_core::key::KeySequence;
use oso_core::keymap::Keymap;
use oso_core::motion;
use oso_core::selection::{Selection, SelectionSet};
use oso_core::window::{Scroll, ViewSize};

use super::Dispatcher;
use crate::{bindings, commands};

const REDRAW: Effects = Effects {
    redraw: true,
    exit: false,
    prefix: None,
};
const NOTHING: Effects = Effects {
    redraw: false,
    exit: false,
    prefix: None,
};

fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, keys: &str) -> Effects {
    let keys: KeySequence = keys.parse().unwrap();
    dispatcher.handle_keys(editor, keys.0)
}

fn with_buffer(buffer: Buffer) -> Editor {
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(10, 20));
    editor
}

/// An editor showing `text`, with the line ending of a file holding it.
fn editor(text: &str) -> Editor {
    with_buffer(Buffer::from_bytes("test", text.as_bytes()))
}

/// A dispatcher and an editor showing `text`, with a cursor at each of `cursors`.
fn at(text: &str, cursors: &[usize]) -> (Dispatcher, Editor) {
    let mut editor = editor(text);
    place(&mut editor, cursors);
    (Dispatcher::new(), editor)
}

fn place(editor: &mut Editor, cursors: &[usize]) {
    let selections = cursors
        .iter()
        .map(|cursor| Selection::at(*cursor))
        .collect();
    let set = SelectionSet::from_parts(selections, 0).unwrap();
    editor.restore_positions(set, Scroll::default());
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

fn cursor(editor: &Editor) -> usize {
    editor.window().selections.primary().cursor
}

fn cursors(editor: &Editor) -> Vec<usize> {
    let selections = editor.window().selections.iter();
    selections.map(|selection| selection.cursor).collect()
}

/// Runs a command with a numeric argument and input, as a key press after `C-u`
/// does.
fn run(
    dispatcher: &mut Dispatcher,
    editor: &mut Editor,
    name: &str,
    count: isize,
    input: &str,
) -> (Effects, Result<(), String>) {
    let id = dispatcher.registry.lookup(name).unwrap();
    let (effects, result) = dispatcher.execute(editor, id, Args::numeric(count), input, None);
    (effects, result.map_err(|e| e.to_string()))
}

/// A message of the last command, for checking what it signaled.
fn last_message(editor: &Editor) -> Option<&str> {
    editor.messages().next_back()
}

// `cfg(test)` again so that the size limit counts these helpers, not the scenarios,
// which grow with every command (ADR 0045).
#[cfg(test)]
mod scenarios {
    use super::*;

    // --- Typing ---

    #[test]
    fn keys_that_type_insert_their_character_after_which_the_cursor_stays() {
        let (mut dispatcher, mut editor) = at("xy", &[1]);
        assert_eq!(send(&mut dispatcher, &mut editor, "a"), REDRAW);
        assert_eq!((text(&editor).as_str(), cursor(&editor)), ("xay", 2));
        send(&mut dispatcher, &mut editor, "B ! SPC");
        assert_eq!((text(&editor).as_str(), cursor(&editor)), ("xaB! y", 5));
        send(&mut dispatcher, &mut editor, "日 🍣");
        assert_eq!(text(&editor), "xaB! 日🍣y");
        assert_eq!(cursor(&editor), 5 + 3 + 4);
        assert!(editor.current().1.is_modified());
    }

    #[test]
    fn keys_with_control_meta_or_super_and_keys_under_a_prefix_type_nothing() {
        let (mut dispatcher, mut editor) = at("", &[0]);
        for keys in ["C-q", "M-q", "s-q", "C-M-q", "C-x q", "<f35>"] {
            assert_eq!(send(&mut dispatcher, &mut editor, keys), NOTHING, "{keys}");
        }
        assert_eq!(text(&editor), "");
        assert!(!editor.current().1.is_modified());
    }

    #[test]
    fn a_mark_stays_before_the_cursor_also_when_it_joins_what_follows() {
        // After its base: the cluster grows, and the cursor is past it.
        let (mut dispatcher, mut editor) = at("か", &[3]);
        send(&mut dispatcher, &mut editor, "\u{3099}");
        assert_eq!(cursor(&editor), "か\u{3099}".len());
        send(&mut dispatcher, &mut editor, "x");
        assert_eq!(text(&editor), "か\u{3099}x");

        // A joiner before an emoji takes it into one cluster, and the next key goes
        // after that cluster, so what was typed stays in order.
        let (mut dispatcher, mut editor) = at("👩", &[0]);
        send(&mut dispatcher, &mut editor, "👨 \u{200d}");
        assert_eq!(cursor(&editor), "👨\u{200d}👩".len());
        send(&mut dispatcher, &mut editor, "x");
        assert_eq!(text(&editor), "👨\u{200d}👩x");

        // Text that joins nothing keeps the cursor just after it.
        let (mut dispatcher, mut editor) = at("👩", &[0]);
        send(&mut dispatcher, &mut editor, "a");
        assert_eq!(cursor(&editor), 1);
    }

    #[test]
    fn typing_at_every_cursor_keeps_each_after_its_text() {
        let (mut dispatcher, mut editor) = at("ab", &[0, 1, 2]);
        assert_eq!(send(&mut dispatcher, &mut editor, "x"), REDRAW);
        assert_eq!(text(&editor), "xaxbx");
        assert_eq!(cursors(&editor), [1, 3, 5]);
        send(&mut dispatcher, &mut editor, "語");
        assert_eq!(text(&editor), "x語ax語bx語");
        assert_eq!(cursors(&editor), [4, 9, 14]);
    }

    #[test]
    fn the_window_follows_what_is_typed() {
        let (mut dispatcher, mut editor) = at("", &[0]);
        for _ in 0..25 {
            send(&mut dispatcher, &mut editor, "a");
        }
        assert_eq!(editor.window().scroll().left, 6);
        for _ in 0..12 {
            send(&mut dispatcher, &mut editor, "RET");
        }
        assert_eq!(editor.window().scroll(), Scroll { top: 3, left: 0 });
    }

    #[test]
    fn a_numeric_argument_repeats_what_is_typed() {
        let (mut dispatcher, mut editor) = at("xy", &[1]);
        let (effects, result) = run(&mut dispatcher, &mut editor, "self-insert-command", 3, "語");
        assert_eq!((effects, result), (REDRAW, Ok(())));
        assert_eq!(text(&editor), "x語語語y");
        assert_eq!(cursor(&editor), 10);
        let (effects, result) = run(&mut dispatcher, &mut editor, "self-insert-command", 0, "語");
        assert_eq!((effects, result), (NOTHING, Ok(())), "nothing to draw");
        for (count, message) in [
            (-1, "Negative repetition argument -1"),
            (
                isize::MIN,
                "Negative repetition argument -9223372036854775808",
            ),
            (isize::MAX, "Too much text to insert"),
        ] {
            let (effects, result) = run(
                &mut dispatcher,
                &mut editor,
                "self-insert-command",
                count,
                "a",
            );
            assert_eq!(effects, NOTHING, "{count}");
            assert_eq!(result, Err(format!("self-insert-command: {message}")));
        }
        assert_eq!(text(&editor), "x語語語y", "what failed changed nothing");
    }

    #[test]
    fn the_limit_on_inserted_text_counts_every_cursor_and_byte() {
        const LIMIT: isize = 16 * 1024 * 1024;
        let too_much = Err("self-insert-command: Too much text to insert".to_owned());
        let insert = |editor: &mut Editor, count, input| {
            let mut dispatcher = Dispatcher::new();
            run(&mut dispatcher, editor, "self-insert-command", count, input)
        };
        let (_, mut editor) = at("ab", &[0, 1]);
        assert_eq!(
            insert(&mut editor, LIMIT / 2 + 1, "x"),
            (NOTHING, too_much.clone())
        );
        assert_eq!(text(&editor), "ab", "two cursors each over half of it");
        assert_eq!(
            insert(&mut editor, LIMIT / 6 + 1, "語"),
            (NOTHING, too_much)
        );
        assert_eq!(text(&editor), "ab", "three bytes each");
        assert_eq!(insert(&mut editor, LIMIT / 2, "x"), (REDRAW, Ok(())));
        assert_eq!(editor.current().1.text().len(), 2 + LIMIT as usize);
    }

    #[test]
    fn self_insert_with_no_text_signals() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("");
        let (effects, result) = dispatcher.run_command(&mut editor, "self-insert-command", None);
        assert_eq!(effects, NOTHING);
        assert_eq!(result, Err("self-insert-command: No text to insert".into()));
        assert_eq!(text(&editor), "");
    }

    #[test]
    fn a_keymap_with_no_text_command_inserts_nothing_by_any_path() {
        let registry = commands::registry();
        let menu = Keymap::from_table(&registry, &[("RET", "newline")]).unwrap();
        let mut dispatcher = Dispatcher::with(registry, menu);
        let mut editor = editor("");
        assert_eq!(send(&mut dispatcher, &mut editor, "a"), NOTHING);
        assert_eq!(dispatcher.insert_text(&mut editor, "日本語"), NOTHING);
        assert_eq!(text(&editor), "");
        send(&mut dispatcher, &mut editor, "RET");
        assert_eq!(text(&editor), "\n");
    }

    #[test]
    fn insert_text_runs_the_same_command_at_every_cursor() {
        let (mut dispatcher, mut editor) = at("ab", &[1, 2]);
        assert_eq!(dispatcher.insert_text(&mut editor, "日本語"), REDRAW);
        assert_eq!(text(&editor), "a日本語b日本語");
        assert_eq!(cursors(&editor), [10, 20]);
        assert_eq!(dispatcher.insert_text(&mut editor, ""), NOTHING, "no text");
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), "ab", "one step");
        assert_eq!(cursors(&editor), [1, 2]);

        // In a read-only buffer it signals as a key does.
        let mut editor = with_buffer(Buffer::from_bytes("t", b"caf\xE9"));
        assert_eq!(dispatcher.insert_text(&mut editor, "日本語"), NOTHING);
        assert_eq!(last_message(&editor), Some("Buffer is read-only"));
    }

    #[test]
    fn insert_text_from_a_key_with_several_characters_is_one_step() {
        let (mut dispatcher, mut editor) = at("", &[0]);
        send(&mut dispatcher, &mut editor, "a");
        assert_eq!(dispatcher.insert_text(&mut editor, "لا"), REDRAW);
        assert_eq!(text(&editor), "aلا");
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), "a");
    }

    #[test]
    fn insert_text_after_a_prefix_is_an_undefined_sequence() {
        let (mut dispatcher, mut editor) = at("", &[0]);
        send(&mut dispatcher, &mut editor, "C-x");
        assert_eq!(dispatcher.insert_text(&mut editor, "日本語"), NOTHING);
        assert!(dispatcher.pending().is_empty());
        assert_eq!(text(&editor), "");
        dispatcher.insert_text(&mut editor, "日本語");
        assert_eq!(text(&editor), "日本語", "the next text types");
    }

    // --- RET ---

    #[test]
    fn ret_inserts_the_files_line_ending() {
        let (mut dispatcher, mut editor) = at("ab", &[1]);
        send(&mut dispatcher, &mut editor, "RET");
        assert_eq!(text(&editor), "a\nb");
        assert_eq!(cursor(&editor), 2);

        let (mut dispatcher, mut editor) = at("ab\r\ncd\n", &[1]);
        assert_eq!(send(&mut dispatcher, &mut editor, "RET"), REDRAW);
        assert_eq!(text(&editor), "a\r\nb\r\ncd\n", "the first line break's");
        assert_eq!(cursor(&editor), 3);
        let (window, buffer) = editor.current();
        assert_eq!(
            window.cursor_point(&buffer.display()),
            DisplayPoint::new(1, 0)
        );
        send(&mut dispatcher, &mut editor, "DEL");
        assert_eq!(text(&editor), "ab\r\ncd\n", "as one character");
        assert_eq!(cursor(&editor), 1);

        // At every cursor, and with a count.
        let (mut dispatcher, mut editor) = at("ab\r\n", &[0, 1]);
        let (_, result) = run(&mut dispatcher, &mut editor, "newline", 2, "");
        assert_eq!(result, Ok(()));
        assert_eq!(text(&editor), "\r\n\r\na\r\n\r\nb\r\n");
        assert_eq!(cursors(&editor), [4, 9]);
    }

    #[test]
    fn ret_takes_counts_as_typing_does() {
        let (mut dispatcher, mut editor) = at("", &[0]);
        let (effects, result) = run(&mut dispatcher, &mut editor, "newline", 0, "");
        assert_eq!((effects, result), (NOTHING, Ok(())));
        let (effects, result) = run(&mut dispatcher, &mut editor, "newline", isize::MIN, "");
        assert_eq!(effects, NOTHING);
        assert!(result.unwrap_err().contains("Negative repetition"));
        assert_eq!(text(&editor), "");
        assert!(!editor.current().1.is_modified());
    }

    // --- DEL and C-d ---

    /// Full width, a combining mark, a ZWJ family, a flag, a tab, a letter, CRLF.
    const CLUSTERS: [&str; 7] = ["日", "か\u{3099}", "👨‍👩‍👧", "🇯🇵", "\t", "x", "\r\n"];

    #[test]
    fn del_removes_one_character_on_screen() {
        let all = CLUSTERS.concat();
        let (mut dispatcher, mut editor) = at(&all, &[all.len()]);
        for kept in (0..CLUSTERS.len()).rev() {
            assert_eq!(send(&mut dispatcher, &mut editor, "DEL"), REDRAW);
            assert_eq!(text(&editor), CLUSTERS[..kept].concat());
            assert_eq!(cursor(&editor), text(&editor).len());
        }
        assert_eq!(send(&mut dispatcher, &mut editor, "DEL"), NOTHING);
        assert_eq!(last_message(&editor), Some("Beginning of buffer"));
    }

    #[test]
    fn c_d_removes_one_character_on_screen() {
        let all = CLUSTERS.concat();
        for key in ["C-d", "<delete>"] {
            let (mut dispatcher, mut editor) = at(&all, &[0]);
            for start in 1..=CLUSTERS.len() {
                assert_eq!(send(&mut dispatcher, &mut editor, key), REDRAW);
                assert_eq!(text(&editor), CLUSTERS[start..].concat());
                assert_eq!(cursor(&editor), 0);
            }
            assert_eq!(send(&mut dispatcher, &mut editor, key), NOTHING);
            assert_eq!(last_message(&editor), Some("End of buffer"));
        }
    }

    #[test]
    fn a_count_deletes_that_many_or_nothing() {
        let (mut dispatcher, mut editor) = at("a日本語cd", &[10]);
        let mut del = |name, count| run(&mut dispatcher, &mut editor, name, count, "");
        assert_eq!(del("delete-char", 0), (NOTHING, Ok(())));
        assert_eq!(del("delete-backward-char", 0), (NOTHING, Ok(())));
        assert_eq!(del("delete-backward-char", 2), (REDRAW, Ok(())));
        assert_eq!(del("delete-char", 1), (REDRAW, Ok(())));
        for (name, count, edge) in [
            ("delete-char", 2, "End of buffer"),
            ("delete-char", isize::MAX, "End of buffer"),
            ("delete-char", -3, "Beginning of buffer"),
            ("delete-char", isize::MIN, "Beginning of buffer"),
            ("delete-backward-char", 3, "Beginning of buffer"),
            ("delete-backward-char", isize::MAX, "Beginning of buffer"),
            ("delete-backward-char", -2, "End of buffer"),
            ("delete-backward-char", isize::MIN, "End of buffer"),
        ] {
            let (effects, result) = del(name, count);
            assert_eq!(effects, NOTHING, "{name} {count}");
            assert_eq!(result, Err(format!("{name}: {edge}")), "{count}");
        }
        assert_eq!(del("delete-char", -1), (REDRAW, Ok(())));
        assert_eq!(del("delete-backward-char", -1), (REDRAW, Ok(())));
        assert_eq!(text(&editor), "a");
        assert_eq!(cursor(&editor), 1);
    }

    #[test]
    fn deleting_at_several_cursors() {
        let (mut dispatcher, mut editor) = at("abc def", &[1, 5]);
        assert_eq!(send(&mut dispatcher, &mut editor, "DEL"), REDRAW);
        assert_eq!(text(&editor), "bc ef");
        assert_eq!(cursors(&editor), [0, 3]);

        // Clusters side by side: both go, and the cursors meet.
        let (mut dispatcher, mut editor) = at("日本語", &[3, 6]);
        send(&mut dispatcher, &mut editor, "DEL");
        assert_eq!(text(&editor), "語");
        assert_eq!(cursors(&editor), [0]);

        // Ranges that overlap become their union.
        let (mut dispatcher, mut editor) = at("abcdef", &[1, 2]);
        let (_, result) = run(&mut dispatcher, &mut editor, "delete-char", 2, "");
        assert_eq!(result, Ok(()));
        assert_eq!(text(&editor), "aef");
        assert_eq!(cursors(&editor), [1]);
    }

    #[test]
    fn a_cursor_at_the_edge_signals_and_the_others_still_delete() {
        let (mut dispatcher, mut editor) = at("ab", &[0, 2]);
        assert_eq!(send(&mut dispatcher, &mut editor, "DEL"), REDRAW);
        assert_eq!(text(&editor), "a");
        assert_eq!(cursors(&editor), [0, 1]);
        assert_eq!(last_message(&editor), Some("Beginning of buffer"));

        let (mut dispatcher, mut editor) = at("ab", &[0, 2]);
        assert_eq!(send(&mut dispatcher, &mut editor, "C-d"), REDRAW);
        assert_eq!(text(&editor), "b");
        assert_eq!(cursors(&editor), [0, 1]);
        assert_eq!(last_message(&editor), Some("End of buffer"));
    }

    #[test]
    fn deleting_what_keeps_a_cluster_apart_joins_it() {
        let (mut dispatcher, mut editor) = at("\rx\n", &[1]);
        send(&mut dispatcher, &mut editor, "C-d");
        assert_eq!(text(&editor), "\r\n");
        assert_eq!(
            cursors(&editor),
            [0],
            "a cursor is never inside a line break"
        );
        let (window, buffer) = editor.current();
        assert_eq!(buffer.display().row_count(), 2);
        assert_eq!(
            window.cursor_point(&buffer.display()),
            DisplayPoint::new(0, 0)
        );
    }

    // --- TAB ---

    #[test]
    fn tab_inserts_spaces_up_to_the_next_stop_at_every_cursor() {
        let (mut dispatcher, mut editor) = at("abc\n\n12345678", &[3, 4, 13]);
        assert_eq!(send(&mut dispatcher, &mut editor, "TAB"), REDRAW);
        assert_eq!(
            text(&editor),
            format!(
                "abc{}\n{}\n12345678{}",
                " ".repeat(5),
                " ".repeat(8),
                " ".repeat(8)
            )
        );
        let (window, buffer) = editor.current();
        let display = buffer.display();
        let columns: Vec<usize> = window
            .selections
            .iter()
            .map(|selection| display.offset_to_display_point(selection.cursor).column)
            .collect();
        assert_eq!(columns, [8, 8, 16]);
    }

    #[test]
    fn tab_counts_what_earlier_cursors_on_the_row_insert() {
        let columns = |editor: &Editor| -> Vec<usize> {
            let (window, buffer) = editor.current();
            let display = buffer.display();
            let selections = window.selections.iter();
            selections
                .map(|selection| display.offset_to_display_point(selection.cursor).column)
                .collect()
        };
        let cases: [(&str, &[usize], &[usize]); 5] = [
            ("abcdef", &[1, 3], &[8, 16]),
            ("abcdef", &[0, 1, 2], &[8, 16, 24]),
            // Wide characters take their cells, whatever their bytes.
            ("日本語", &[3, 9], &[8, 16]),
            ("日本語の文章", &[0, 6, 18], &[8, 16, 32]),
            // A tab between them still reaches a stop, from where it now starts.
            ("a\tb", &[1, 3], &[8, 24]),
        ];
        for (start, places, expected) in cases {
            let (mut dispatcher, mut editor) = at(start, places);
            send(&mut dispatcher, &mut editor, "TAB");
            assert_eq!(columns(&editor), expected, "{start:?} at {places:?}");
        }

        // Exactly the spaces each needs.
        let (mut dispatcher, mut editor) = at("abcdef", &[1, 3]);
        send(&mut dispatcher, &mut editor, "TAB");
        let spaces = |n| " ".repeat(n);
        assert_eq!(text(&editor), format!("a{}bc{}def", spaces(7), spaces(6)));
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), "abcdef", "one step");
        assert_eq!(cursors(&editor), [1, 3]);

        // A cursor on another row starts from its own column.
        let (mut dispatcher, mut editor) = at("ab\ncd", &[1, 4]);
        send(&mut dispatcher, &mut editor, "TAB");
        assert_eq!(columns(&editor), [8, 8]);
    }

    #[test]
    fn tab_counts_columns_not_bytes() {
        let wide = "日本";
        let (mut dispatcher, mut editor) = at(wide, &[wide.len()]);
        send(&mut dispatcher, &mut editor, "TAB");
        assert_eq!(text(&editor), format!("{wide}{}", " ".repeat(4)));

        let tabbed = "a\t";
        let (mut dispatcher, mut editor) = at(tabbed, &[tabbed.len()]);
        send(&mut dispatcher, &mut editor, "TAB");
        assert_eq!(text(&editor), format!("{tabbed}{}", " ".repeat(8)));

        let (mut dispatcher, mut editor) = at("", &[0]);
        send(&mut dispatcher, &mut editor, "TAB DEL");
        assert_eq!(text(&editor), " ".repeat(7), "only spaces, one by one");
    }

    // --- Read-only buffers ---

    #[test]
    fn a_read_only_buffer_signals_and_changes_nothing() {
        let invalid = || Buffer::from_bytes("t", b"caf\xE9\n");
        let binary = || Buffer::from_bytes("t", b"a\0b\n");
        let loading = || {
            let path = std::path::PathBuf::from("/test/a.txt");
            let mut buffer = Buffer::visiting(path.clone());
            buffer.judge_secret(Some(&[path]));
            buffer
        };
        let buffers: [&dyn Fn() -> Buffer; 3] = [&invalid, &binary, &loading];
        for buffer in buffers {
            for keys in ["a", "RET", "DEL", "C-d", "TAB", "C-z", "C-M-z"] {
                let mut dispatcher = Dispatcher::new();
                let mut editor = with_buffer(buffer());
                let before = text(&editor);
                assert_eq!(send(&mut dispatcher, &mut editor, keys), NOTHING, "{keys}");
                assert_eq!(text(&editor), before);
                assert!(!editor.current().1.is_modified());
                assert_eq!(cursor(&editor), 0);
                assert_eq!(last_message(&editor), Some("Buffer is read-only"), "{keys}");
            }
        }
    }

    // --- Undo ---

    #[test]
    fn one_command_is_one_step_across_all_cursors() {
        let (mut dispatcher, mut editor) = at("ab", &[0, 1, 2]);
        send(&mut dispatcher, &mut editor, "x RET");
        assert_eq!(text(&editor), "x\nax\nbx\n");
        let typed = cursors(&editor);

        assert_eq!(send(&mut dispatcher, &mut editor, "C-z"), REDRAW);
        assert_eq!(text(&editor), "xaxbx");
        assert_eq!(cursors(&editor), [1, 3, 5]);
        assert_eq!(send(&mut dispatcher, &mut editor, "C-z"), REDRAW);
        assert_eq!(text(&editor), "ab");
        assert_eq!(cursors(&editor), [0, 1, 2]);

        assert_eq!(send(&mut dispatcher, &mut editor, "C-z"), NOTHING);
        assert_eq!(last_message(&editor), Some("No further undo information"));

        send(&mut dispatcher, &mut editor, "C-M-z C-M-z");
        assert_eq!(text(&editor), "x\nax\nbx\n");
        assert_eq!(cursors(&editor), typed);
        assert_eq!(send(&mut dispatcher, &mut editor, "C-M-z"), NOTHING);
        assert_eq!(last_message(&editor), Some("No further redo information"));
    }

    #[test]
    fn deletions_undo_back_to_the_text_and_the_cursors() {
        let all = CLUSTERS.concat();
        let (mut dispatcher, mut editor) = at(&all, &[all.len()]);
        send(&mut dispatcher, &mut editor, "DEL DEL DEL");
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), CLUSTERS[..5].concat());
        send(&mut dispatcher, &mut editor, "C-z C-z");
        assert_eq!(text(&editor), all);
        assert_eq!(cursor(&editor), all.len());
    }

    #[test]
    fn typed_characters_join_until_motion_or_another_edit() {
        let (mut dispatcher, mut editor) = at("", &[0]);
        send(&mut dispatcher, &mut editor, "a b");
        send(&mut dispatcher, &mut editor, "C-b C-f");
        send(&mut dispatcher, &mut editor, "c d");
        send(&mut dispatcher, &mut editor, "RET");
        send(&mut dispatcher, &mut editor, "e");
        send(&mut dispatcher, &mut editor, "DEL");
        send(&mut dispatcher, &mut editor, "f g");
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(
            text(&editor),
            "abcd\n",
            "f g; e's deletion is a step of its own"
        );
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), "abcd\ne");
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), "abcd\n");
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), "abcd");
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), "ab");
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), "");
    }

    #[test]
    fn twenty_typed_characters_join_at_most() {
        let (mut dispatcher, mut editor) = at("", &[0]);
        for _ in 0..45 {
            send(&mut dispatcher, &mut editor, "a");
        }
        for left in [40, 20, 0] {
            send(&mut dispatcher, &mut editor, "C-z");
            assert_eq!(text(&editor).len(), left);
        }
    }

    #[test]
    fn only_a_single_character_joins() {
        let (mut dispatcher, mut editor) = at("", &[0]);
        let typed = |editor: &mut Editor, count| {
            let mut dispatcher = Dispatcher::new();
            let _ = run(&mut dispatcher, editor, "self-insert-command", count, "a");
        };
        // As in Emacs, a count over 1 joins nothing and nothing joins it.
        send(&mut dispatcher, &mut editor, "b");
        typed(&mut editor, 2);
        send(&mut dispatcher, &mut editor, "c");
        typed(&mut editor, 1);
        assert_eq!(text(&editor), "baaca");
        // Text of several characters is a step of its own, at either side.
        dispatcher.insert_text(&mut editor, "日本");
        send(&mut dispatcher, &mut editor, "d");
        dispatcher.insert_text(&mut editor, "語");
        assert_eq!(text(&editor), "baaca日本d語");
        for expected in ["baaca日本", "baaca", "baa", "b", ""] {
            send(&mut dispatcher, &mut editor, "C-z");
            assert_eq!(text(&editor), expected);
        }
    }

    #[test]
    fn a_redo_is_lost_to_an_edit_and_joins_nothing() {
        let (mut dispatcher, mut editor) = at("", &[0]);
        send(&mut dispatcher, &mut editor, "a C-z b");
        assert_eq!(text(&editor), "b");
        assert_eq!(send(&mut dispatcher, &mut editor, "C-M-z"), NOTHING);
        send(&mut dispatcher, &mut editor, "C-z C-M-z c C-z");
        assert_eq!(text(&editor), "b", "c did not join across the undo");
    }

    #[test]
    fn counts_cross_several_steps_and_stop_at_the_end_of_the_history() {
        let (mut dispatcher, mut editor) = at("", &[0]);
        for _ in 0..3 {
            let _ = run(&mut dispatcher, &mut editor, "newline", 1, "");
        }
        let mut cross =
            |editor: &mut Editor, name, count| run(&mut dispatcher, editor, name, count, "");
        assert_eq!(cross(&mut editor, "undo", 0), (NOTHING, Ok(())));
        assert_eq!(cross(&mut editor, "undo", 2), (REDRAW, Ok(())));
        assert_eq!(cross(&mut editor, "undo-redo", 1), (REDRAW, Ok(())));
        assert_eq!(
            cross(&mut editor, "undo", -1),
            (REDRAW, Ok(())),
            "a negative count redoes"
        );
        assert_eq!(cross(&mut editor, "undo-redo", -2), (REDRAW, Ok(())));
        let (effects, result) = cross(&mut editor, "undo", isize::MAX);
        assert_eq!(effects, REDRAW, "as far as it could");
        assert_eq!(result, Err("undo: No further undo information".into()));
        assert_eq!(text(&editor), "");
        let (effects, result) = cross(&mut editor, "undo", isize::MIN);
        assert_eq!(effects, REDRAW);
        assert_eq!(result, Err("undo: No further redo information".into()));
        assert_eq!(text(&editor), "\n\n\n");
        let (effects, result) = cross(&mut editor, "undo-redo", isize::MIN);
        assert_eq!(effects, REDRAW);
        assert_eq!(result, Err("undo-redo: No further undo information".into()));
        assert_eq!(cross(&mut editor, "undo-redo", isize::MAX).0, REDRAW);
        assert_eq!(text(&editor), "\n\n\n");
    }

    #[test]
    fn undo_back_to_the_saved_state_is_unmodified() {
        let (mut dispatcher, mut editor) = at("x", &[1]);
        send(&mut dispatcher, &mut editor, "a");
        assert!(editor.current().1.is_modified());
        send(&mut dispatcher, &mut editor, "C-z");
        assert!(!editor.current().1.is_modified());
        send(&mut dispatcher, &mut editor, "C-M-z");
        assert!(editor.current().1.is_modified());

        // Saved part way: that is now the state to return to.
        let (id, _) = editor.buffers().next().unwrap();
        let saved = editor.current().1.content_id();
        editor.buffer_mut(id).unwrap().mark_saved(saved);
        assert!(!editor.current().1.is_modified());
        send(&mut dispatcher, &mut editor, "RET");
        assert!(editor.current().1.is_modified());
        send(&mut dispatcher, &mut editor, "C-z");
        assert!(!editor.current().1.is_modified());
        send(&mut dispatcher, &mut editor, "C-z");
        assert!(editor.current().1.is_modified(), "before the save");
    }

    fn edit_and_panic(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
        let buffer = ctx.editor.window().buffer();
        let edit = Replacement {
            range: 0..0,
            text: "X",
        };
        ctx.editor
            .apply_edits(buffer, std::slice::from_ref(&edit))?;
        ctx.editor.apply_edits(buffer, &[edit])?;
        panic!("deliberate panic in a test command");
    }

    #[test]
    fn what_a_panicking_command_applied_is_one_step_and_nothing_joins_it() {
        let mut registry = commands::registry();
        registry
            .register(Command {
                name: "edit-and-panic",
                doc: "Edit, then panic, to test the command boundary.",
                args: ArgSpec::None,
                run: edit_and_panic,
            })
            .unwrap();
        let mut global = bindings::global_keymap(&registry);
        let keys: KeySequence = "C-c p".parse().unwrap();
        global
            .bind(&keys.0, registry.lookup("edit-and-panic").unwrap())
            .unwrap();
        let mut dispatcher = Dispatcher::with(registry, global);
        let mut editor = editor("");
        send(&mut dispatcher, &mut editor, "a");
        assert_eq!(send(&mut dispatcher, &mut editor, "C-c p"), REDRAW);
        assert_eq!(text(&editor), "XXa");
        assert_eq!(last_message(&editor), Some("edit-and-panic panicked"));
        send(&mut dispatcher, &mut editor, "b");
        assert_eq!(text(&editor), "XXab");

        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), "XXa", "b did not join it");
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), "a", "both of its edits, in one step");
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), "");
    }

    // --- Messages ---

    #[test]
    fn a_failed_command_leaves_a_message_that_quotes_no_text() {
        let (mut dispatcher, mut editor) = at("SECRET-TEXT", &[0]);
        send(&mut dispatcher, &mut editor, "DEL");
        assert_eq!(
            editor.messages().collect::<Vec<_>>(),
            ["Beginning of buffer"]
        );
        send(&mut dispatcher, &mut editor, "C-f");
        assert_eq!(
            editor.messages().count(),
            1,
            "a command that worked leaves none"
        );
        assert_eq!(send(&mut dispatcher, &mut editor, "C-x C-g"), NOTHING);
        assert_eq!(editor.messages().count(), 1, "no command ran");
    }

    // --- Many keys ---

    const KEYS: [&str; 13] = [
        "a", "語", "\u{301}", "\u{200d}", "🍣", "RET", "TAB", "DEL", "C-d", "C-z", "C-M-z", "C-f",
        "C-b",
    ];
    const STARTS: [&str; 6] = [
        "",
        "ab",
        "a語\tb\n\u{301}",
        "\r\nx\r\n",
        "e\u{301}🇯🇵\n\n",
        "\u{301}",
    ];

    /// Whatever is typed, cursors stay where cursors can be, and undoing it all returns
    /// to the text the buffer started with, unmodified. The keys are pseudo-random, from
    /// a fixed seed, since this crate has no `proptest`.
    #[test]
    fn undo_returns_to_the_start_and_cursors_stay_on_clusters() {
        let mut seed: u64 = 1;
        let mut below = |bound: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((seed >> 33) as usize) % bound
        };
        for round in 0..300 {
            let start = STARTS[round % STARTS.len()];
            let mut dispatcher = Dispatcher::new();
            let mut editor = editor(start);
            // Up to three cursors, spread over the display clusters of the start.
            let places: Vec<usize> = {
                let display = editor.current().1.display();
                let mut places = vec![0];
                while let Some(next) = motion::next_cluster(&display, places[places.len() - 1]) {
                    places.push(next);
                }
                let step = places.len().div_ceil(1 + below(3));
                places.into_iter().step_by(step).collect()
            };
            place(&mut editor, &places);
            for _ in 0..below(40) {
                let key = KEYS[below(KEYS.len())];
                send(&mut dispatcher, &mut editor, key);
                let (window, buffer) = editor.current();
                let display = buffer.display();
                for selection in window.selections.iter() {
                    let cursor = selection.cursor;
                    assert_eq!(motion::cluster_start(&display, cursor), cursor, "{key}");
                }
            }
            while send(&mut dispatcher, &mut editor, "C-z").redraw {}
            assert_eq!(text(&editor), start, "round {round}");
            assert!(!editor.current().1.is_modified(), "round {round}");
        }
    }
}
