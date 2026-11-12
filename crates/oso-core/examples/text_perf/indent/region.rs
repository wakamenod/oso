//! Indentation's region and small commands (ADR 0115, step 3), through the editor:
//! `indent-region` over whole files and at its bounds, `indent-rigidly`, `M-^` over a region,
//! Markdown's `TAB` and `M-RET` at 1000 cursors, and the cases the step's review asked for.
//!
//! Must not depend on: crate internals; it uses only the public API, and the timing
//! helpers of `text_perf`.

use std::time::Duration;

use oso_core::buffer::{Buffer, Mode};
use oso_core::editor::Editor;
use oso_core::indent::{LINE_COST, LINE_LOOKBACK};
use oso_core::rope::Rope;
use oso_core::selection::{Selection, SelectionSet};

use super::super::MB;
use super::{command_costs, line_ends, rust_editor, rust_file};

/// An editor showing `text` as Rust, with one region over all of it.
fn region_editor(text: &str) -> Editor {
    let mut editor = rust_editor(text, &[text.len()]);
    let whole = Selection {
        cursor: text.len(),
        mark: Some(0),
        goal: None,
    };
    editor.current_mut().0.selections = SelectionSet::from_parts(vec![whole], 0).expect("a region");
    editor
}

/// `indent-region` over whole Rust files, through the editor: one already right, which the
/// command only reads, and one at the margin, every line of which it changes.
pub(super) fn region_costs() {
    for size in [MB, 2 * MB, 3 * MB] {
        let text = rust_file(size);
        let rope = Rope::from(text.as_str());
        let needed = text.len() + rope.line_count() * LINE_COST + LINE_LOOKBACK;
        let flat: String = text
            .lines()
            .map(|line| format!("{}\n", line.trim_start()))
            .collect();
        for (label, text) in [("right", &text), ("flat", &flat)] {
            let (p50, p99) = command_costs(
                5,
                || region_editor(text),
                |editor| {
                    let done = editor.indent_region(None, 16 << 20);
                    if let Err(error) = done {
                        println!("  {error}");
                    }
                },
            );
            println!(
                "editor indent-region {:>2} MiB {label:<5} p50 {p50:>9.2?}  p99 {p99:>9.2?}  (needs {:.1} MiB)",
                size / MB,
                needed as f64 / MB as f64
            );
        }
    }
    // The most a command may do: about all the reading the limit allows, and the most lines
    // changed, short ones, then long ones.
    let right = rust_file(2 * MB);
    for (label, line) in [("short", "x;\n"), ("long", "let x = 1;\n")] {
        let flat = format!("fn f() {{\n{}}}\n", line.repeat(29_990));
        for (what, text) in [
            ("alone", flat.clone()),
            ("after 2 MiB", format!("{right}{flat}")),
        ] {
            let rope = Rope::from(text.as_str());
            let needed = text.len() + rope.line_count() * LINE_COST + LINE_LOOKBACK;
            let (p50, p99) = command_costs(
                5,
                || region_editor(&text),
                |editor| {
                    if let Err(error) = editor.indent_region(None, 16 << 20) {
                        println!("  {error}");
                    }
                },
            );
            println!(
                "editor indent-region 30K {label:<5} lines {what:<13} p50 {p50:>9.2?}  p99 {p99:>9.2?}  (needs {:.1} MiB)",
                needed as f64 / MB as f64
            );
        }
    }
}

/// `indent-rigidly` and `delete-indentation` over a region of nearly the most lines one command
/// changes, and Markdown's `TAB` and `M-RET` at 1000 cursors in a 2 MiB document.
pub(super) fn small_command_costs() {
    let lines = "    let x = 1;\n".repeat(29_000);
    let (p50, p99) = command_costs(
        5,
        || region_editor(&lines),
        |editor| {
            editor.indent_rigidly(1, 16 << 20).expect("fits");
        },
    );
    println!("editor indent-rigidly 29K lines     p50 {p50:>9.2?}  p99 {p99:>9.2?}");
    for joins in [10_000, 20_000, 29_000] {
        let lines = "    let x = 1;\n".repeat(joins + 1);
        let (p50, p99) = command_costs(
            5,
            || {
                let mut editor = region_editor(&lines);
                let set = &mut editor.current_mut().0.selections;
                set.set_mark_active(oso_core::selection::MarkActive::Active);
                editor
            },
            |editor| {
                editor.delete_indentation(false).expect("fits");
            },
        );
        println!(
            "editor M-^ over {:>2}K lines           p50 {p50:>9.2?}  p99 {p99:>9.2?}",
            joins / 1000
        );
    }
    let notes = "## A heading\n\n- an item\n  - a nested item with some text\n    more text\n\n```rust\nfn main() {}\n```\n\nA paragraph of prose.\n";
    let text = notes.repeat(2 * MB / notes.len());
    let rope = Rope::from(text.as_str());
    let ends = line_ends(&rope, 1000);
    let markdown = |at: &[usize]| {
        let mut buffer = Buffer::from_text("big.md", &text);
        buffer.set_mode(Mode::Markdown);
        let mut editor = Editor::with_buffer(buffer);
        let parts = at.iter().map(|&at| Selection::at(at)).collect();
        editor.current_mut().0.selections = SelectionSet::from_parts(parts, 0).expect("a cursor");
        editor
    };
    let (p50, p99) = command_costs(
        20,
        || markdown(&ends),
        |editor| {
            editor.indent_for_tab().expect("editable");
        },
    );
    println!("editor Markdown TAB at 1000 cursors p50 {p50:>9.2?}  p99 {p99:>9.2?}");
    let first = line_ends(&Rope::from(&text[..LINE_LOOKBACK]), 1000);
    let (p50, p99) = command_costs(
        20,
        || markdown(&first),
        |editor| {
            editor.indent_for_tab().expect("editable");
        },
    );
    println!("  the same in its first 64 KiB      p50 {p50:>9.2?}  p99 {p99:>9.2?}");
    let (p50, p99) = command_costs(
        20,
        || markdown(&first),
        |editor| {
            let _ = editor.markdown_insert_list_item("\n", 16 << 20);
        },
    );
    println!("editor M-RET at 1000 cursors, 64 KiB p50 {p50:>9.2?}  p99 {p99:>9.2?}");
}

/// Prints a case's p50 and p99.
fn report(label: &str, (p50, p99): (Duration, Duration)) {
    println!("{label:<44} p50 {p50:>9.2?}  p99 {p99:>9.2?}");
}

/// Says why a command changed nothing, if it did not.
fn quiet(done: Result<(), oso_core::editor::IndentError>) {
    if let Err(error) = done {
        println!("  {error}");
    }
}

/// The cases the review of step 3 asked for: tab indentation already right, a blank line after
/// every line, and 1000 regions apart.
pub(super) fn reviewed_costs() {
    let tabbed: String = rust_file(2 * MB)
        .lines()
        .map(|line| {
            let spaces = line.len() - line.trim_start().len();
            let (tabs, rest) = (spaces / 8, spaces % 8);
            format!(
                "{}{}{}\n",
                "\t".repeat(tabs),
                " ".repeat(rest),
                line.trim_start()
            )
        })
        .collect();
    let tabs = oso_core::indent::Settings::new(4, oso_core::indent::Style::Tabs, 8).expect("valid");
    report(
        "indent-region, 2 MiB of tabs already right",
        command_costs(
            5,
            || {
                let mut editor = region_editor(&tabbed);
                let id = editor.window().buffer();
                editor
                    .buffer_mut(id)
                    .expect("shown")
                    .set_indent_settings(tabs);
                editor
            },
            |editor| quiet(editor.indent_region(None, 16 << 20)),
        ),
    );
    let gaps = format!("fn f() {{\n{}}}\n", "    x;\n\n".repeat(70_000));
    report(
        "indent-region, 70K lines each before a blank",
        command_costs(
            5,
            || region_editor(&gaps),
            |editor| {
                quiet(editor.indent_region(None, 16 << 20));
            },
        ),
    );
    let apart = rust_file(5 * MB);
    let rope = Rope::from(apart.as_str());
    let rows = rope.line_count();
    let regions: Vec<Selection> = (0..1000)
        .map(|index| {
            let row = index * rows / 1000;
            Selection {
                cursor: rope.line_start(row + 30),
                mark: Some(rope.line_start(row)),
                goal: None,
            }
        })
        .collect();
    let apart_editor = || {
        let mut editor = rust_editor(&apart, &[0]);
        editor.current_mut().0.selections =
            SelectionSet::from_parts(regions.clone(), 0).expect("regions");
        editor
    };
    report(
        "indent-region, 1000 regions of 30 lines",
        command_costs(5, apart_editor, |editor| {
            quiet(editor.indent_region(None, 16 << 20))
        }),
    );
    report(
        "indent-rigidly, 1000 regions of 30 lines",
        command_costs(5, apart_editor, |editor| {
            quiet(editor.indent_rigidly(1, 16 << 20))
        }),
    );
}

/// Deep columns up to the insert limit, and `M-^` over long lines, for the review of step 3.
pub(super) fn reviewed_edit_costs() {
    let lines = "x\n".repeat(29_000);
    report(
        "indent-region to column 559, 29K lines",
        command_costs(
            5,
            || region_editor(&lines),
            |editor| {
                quiet(editor.indent_region(Some(559), 16 << 20));
            },
        ),
    );
    for (joins, width) in [(15_000, 2 * MB / 15_000), (20_000, 150), (10_000, 400)] {
        let long = format!("{}\n", "y".repeat(width)).repeat(joins + 1);
        report(
            &format!("M-^ over {}K lines of {width} bytes", joins / 1000),
            command_costs(
                5,
                || {
                    let mut editor = region_editor(&long);
                    let set = &mut editor.current_mut().0.selections;
                    set.set_mark_active(oso_core::selection::MarkActive::Active);
                    editor
                },
                |editor| quiet(editor.delete_indentation(false)),
            ),
        );
    }
    for column in [140, 280] {
        report(
            &format!("indent-region to column {column}, 29K lines"),
            command_costs(
                5,
                || region_editor(&lines),
                |editor| {
                    quiet(editor.indent_region(Some(column), 16 << 20));
                },
            ),
        );
    }
}
