//! Tests of an open menu's panes (ADR 0117): its panel and chips, a pane for each part that
//! overlaps no other, the marked default head, and no float under a prompt.
//!
//! Must not depend on: the GPU; panes are plain data.

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;
use oso_core::frame::{CellRect, FrameSize};
use oso_core::key::KeySequence;
use oso_core::keymap::Keymap;
use oso_core::menu::{Definition, Head, HeadAction, Menus, Note, Notes, Open, Stay};
use oso_render::{Face, Layer, PaneContent};

use super::super::{Cells, statuses_into, with_frame};
use crate::dispatch::Dispatcher;
use crate::{bindings, commands};

/// Head 1 noted, head 2 unavailable, head 0 the default.
fn describe(_: &Editor, _: &Open) -> Notes {
    Notes {
        line: Some("a line".to_owned()),
        ret_head: Some(0),
        heads: vec![
            (
                1,
                Note {
                    text: "12 files".to_owned(),
                    ..Note::default()
                },
            ),
            (
                2,
                Note {
                    unavailable: Some("Not now".to_owned()),
                    ..Note::default()
                },
            ),
        ],
    }
}

/// A dispatcher whose `C-c m` opens a menu of an exit head, a stay head, an unavailable head
/// and a switch, and an editor of 30 rows by 100 columns.
fn opened() -> (Dispatcher, Editor) {
    let registry = commands::registry();
    let id = |name| registry.lookup(name).unwrap();
    let head = |keys: &str, label: &str, action, stay| Head {
        keys: keys.parse().unwrap(),
        label: label.to_owned(),
        action,
        stay,
        count: None,
    };
    let mut menu = Definition::new("test", "Test").unwrap();
    menu.set_describe(describe);
    menu.add_group(Some("Move"), 1).unwrap();
    let heads = [
        head("e", "end", HeadAction::Command(id("end-of-buffer")), None),
        head(
            "f",
            "forward",
            HeadAction::Command(id("forward-char")),
            Some(Stay::Stay),
        ),
        head("b", "back", HeadAction::Command(id("backward-char")), None),
    ];
    for head in heads {
        menu.add_head(head).unwrap();
    }
    menu.add_group(Some("Switches"), 1).unwrap();
    menu.add_head(head(
        "- f",
        "files",
        HeadAction::Switch("files".into()),
        None,
    ))
    .unwrap();
    let mut menus = Menus::new();
    let test = menus.id("test").unwrap();
    menus.define(test, menu).unwrap();
    let global = bindings::global_keymap(&registry);
    let mut d = Dispatcher::with(registry, global);
    d.set_menus(Some(menus));
    let mut user = Keymap::new();
    user.bind_menu(&"C-c m".parse::<KeySequence>().unwrap().0, test)
        .unwrap();
    d.set_user_keymap(user);
    let mut editor = Editor::with_buffer(Buffer::from_text("a.txt", "abc"));
    editor.set_frame_size(FrameSize {
        rows: 30,
        columns: 100,
    });
    let keys: KeySequence = "C-c m - f".parse().unwrap();
    d.handle_keys(&mut editor, keys.0);
    (d, editor)
}

/// Each float pane's id, layer, region, and its rows' text and faces, and the panel's chips
/// and marked row.
type Drawn = (
    Vec<(u32, Layer, CellRect, Vec<Vec<(String, Face)>>)>,
    Vec<(CellRect, Face)>,
    Option<CellRect>,
);

fn drawn(d: &Dispatcher, editor: &Editor) -> Drawn {
    let mut statuses = Vec::new();
    statuses_into(editor, &mut statuses);
    let menu = d.menu();
    with_frame(
        editor,
        menu.as_ref(),
        &statuses,
        &mut Cells::default(),
        |panes| {
            let mut chips = Vec::new();
            let mut marked = None;
            let floats = panes
                .iter()
                .filter(|pane| pane.layer != Layer::TILED)
                .map(|pane| {
                    if let Some(panel) = pane.panel.as_ref().filter(|_| pane.id.0 == super::PANEL) {
                        chips.extend_from_slice(panel.keys);
                        marked = panel.marked;
                        assert!(panel.border, "a completion's panel");
                    }
                    let rows = match &pane.content {
                        PaneContent::Lines(lines) => lines
                            .iter()
                            .map(|spans| {
                                spans
                                    .iter()
                                    .map(|span| (span.text.to_owned(), span.face))
                                    .collect()
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    (pane.id.0, pane.layer, pane.region, rows)
                })
                .collect();
            (floats, chips, marked)
        },
    )
}

#[test]
fn a_menus_float_has_its_panel_its_parts_each_in_a_pane_and_chips_by_kind() {
    let (d, editor) = opened();
    let (floats, chips, marked) = drawn(&d, &editor);
    let content: Vec<_> = floats
        .iter()
        .filter(|(_, layer, ..)| *layer == Layer::FLOAT_CONTENT)
        .collect();
    // Title, line, two columns and `q quit`.
    assert_eq!(content.len(), 5, "{content:?}");
    for (i, a) in content.iter().enumerate() {
        for b in &content[i + 1..] {
            let (a, b) = (a.2, b.2);
            let apart = a.row + a.rows <= b.row
                || b.row + b.rows <= a.row
                || a.column + a.columns <= b.column
                || b.column + b.columns <= a.column;
            assert!(apart, "{a:?} overlaps {b:?}");
        }
    }
    let move_column = content
        .iter()
        .find(|(.., rows)| rows.first().is_some_and(|row| row[0].0 == "Move"))
        .unwrap();
    let rows = &move_column.3;
    assert_eq!(rows[1][0].1, Face::MenuNote, "the rule");
    let faces = |row: &Vec<(String, Face)>| -> Vec<Face> {
        row.iter()
            .filter(|(t, _)| !t.trim().is_empty())
            .map(|(_, f)| *f)
            .collect()
    };
    assert_eq!(faces(&rows[2]), [Face::MenuKey, Face::Text]);
    assert_eq!(
        faces(&rows[3]),
        [Face::MenuKeyStay, Face::Text, Face::MenuNote]
    );
    assert_eq!(rows[3].last().unwrap().0, "12 files");
    assert_eq!(
        faces(&rows[4]),
        [Face::MenuNote, Face::MenuNote],
        "unavailable: dim"
    );
    // Chips: `e`, `f`, the switch's key and its `[x]`, and `q`; none for the unavailable `b`.
    let kinds: Vec<Face> = chips.iter().map(|(_, face)| *face).collect();
    assert_eq!(
        kinds,
        [
            Face::MenuKey,
            Face::MenuKeyStay,
            Face::MenuKeySwitch,
            Face::MenuKeySwitch,
            Face::MenuKey
        ]
    );
    // The default head's row is marked.
    let first_row = CellRect {
        rows: 1,
        ..move_column.2
    };
    assert_eq!(marked.map(|m| m.row), Some(first_row.row + 2));
}

#[test]
fn no_float_is_drawn_under_a_prompt_nor_once_the_menu_closes() {
    let (mut d, mut editor) = opened();
    let keys: KeySequence = "C-g".parse().unwrap();
    d.handle_keys(&mut editor, keys.0);
    assert!(drawn(&d, &editor).0.is_empty());
    let (d, mut editor) = opened();
    editor
        .open_prompt("Say: ", oso_core::minibuffer::Kind::Text { history: None })
        .unwrap();
    let (floats, chips, _) = drawn(&d, &editor);
    assert!(chips.is_empty());
    assert!(floats.iter().all(|(id, ..)| *id < super::MENU_PANES));
}

#[test]
fn a_label_of_controls_and_invisible_characters_stays_in_its_columns_pane() {
    let crafted = "\u{202e}ab\u{200b}\n\u{1b}[31m\u{2066}cd";
    let registry = commands::registry();
    let id = registry.lookup("forward-char").unwrap();
    let head = |keys: &str, label: &str| Head {
        keys: keys.parse().unwrap(),
        label: label.to_owned(),
        action: HeadAction::Command(id),
        stay: None,
        count: None,
    };
    let mut menu = Definition::new("odd", "Odd").unwrap();
    menu.add_group(Some("One"), 1).unwrap();
    menu.add_head(head("a", crafted)).unwrap();
    menu.add_group(Some("Two"), 1).unwrap();
    menu.add_head(head("b", "plain")).unwrap();
    let mut menus = Menus::new();
    let odd = menus.id("odd").unwrap();
    menus.define(odd, menu).unwrap();
    let global = bindings::global_keymap(&registry);
    let mut d = Dispatcher::with(registry, global);
    d.set_menus(Some(menus));
    let mut user = Keymap::new();
    user.bind_menu(&"C-c o".parse::<KeySequence>().unwrap().0, odd)
        .unwrap();
    d.set_user_keymap(user);
    let mut editor = Editor::with_buffer(Buffer::from_text("a.txt", "abc"));
    editor.set_frame_size(FrameSize {
        rows: 30,
        columns: 100,
    });
    d.handle_keys(&mut editor, "C-c o".parse::<KeySequence>().unwrap().0);
    let (floats, ..) = drawn(&d, &editor);
    let content: Vec<_> = floats
        .iter()
        .filter(|(_, layer, ..)| *layer == Layer::FLOAT_CONTENT)
        .collect();
    for (i, a) in content.iter().enumerate() {
        for b in &content[i + 1..] {
            let (a, b) = (a.2, b.2);
            let apart = a.row + a.rows <= b.row
                || b.row + b.rows <= a.row
                || a.column + a.columns <= b.column
                || b.column + b.columns <= a.column;
            assert!(apart, "{a:?} overlaps {b:?}");
        }
    }
    // The label goes to the renderer whole, as a span of its column's pane only, which boxes
    // what it cannot show and cuts what passes the pane's region.
    let holding: Vec<_> = content
        .iter()
        .filter(|(.., rows)| rows.iter().flatten().any(|(text, _)| text == crafted))
        .collect();
    assert_eq!(holding.len(), 1, "{content:?}");
    assert_eq!(holding[0].3[0][0].0, "One");
}
