//! A buffer's indentation: EditorConfig over the configuration over the default, kept through
//! a configuration's change and a mode's, with the tab width it is shown with.
//!
//! Must not depend on: `command` or `keymap`; the host's calls are written out here.

use std::path::PathBuf;

use crate::buffer::{Baseline, Buffer, BufferId, Mode};
use crate::display_map::TabWidth;
use crate::editor::Editor;
use crate::editorconfig::{Applied, Budget, File, Properties};
use crate::indent::{Origin, Origins, Settings, Style};
use crate::load::decode_all;

/// What `text`, an `.editorconfig` beside the file, gives a file `a.rs`.
fn applied(text: &str) -> Applied {
    let mut properties = Properties::default();
    properties.apply(&File::parse(text, &mut Budget::default()), 0, "a.rs");
    Applied {
        properties,
        files: vec![PathBuf::from("/p/.editorconfig")],
    }
}

/// An editor visiting `/p/a.rs`, shown with tabs `tab_width` wide.
fn visiting(tab_width: u32) -> (Editor, BufferId) {
    let mut buffer = Buffer::visiting(PathBuf::from("/p/a.rs"));
    buffer.apply_load(decode_all(b"fn a() {}\n", TabWidth::new(tab_width)));
    buffer.set_baseline(Baseline {
        location: PathBuf::from("/p/a.rs"),
        file: None,
    });
    let editor = Editor::with_buffer(buffer);
    let id = editor.window().buffer();
    (editor, id)
}

/// Each mode two spaces wide from the configuration, with tabs `tab_width` wide from it too.
fn configure(editor: &mut Editor, tab_width: u32) {
    let two = Settings::new(2, Style::Spaces, tab_width).unwrap();
    let origins = Origins {
        indent_size: Origin::Configuration,
        style: Origin::Configuration,
        tab_width: Origin::Configuration,
    };
    let modes = [Mode::Rust, Mode::Markdown, Mode::Toml, Mode::Fundamental];
    editor.set_mode_indents(modes.map(|mode| (mode, two, origins)));
}

fn of(editor: &Editor, id: BufferId) -> (Settings, Origins) {
    let buffer = editor.buffer(id).unwrap();
    (buffer.indent_settings(), buffer.indent_origins())
}

#[test]
fn editorconfig_wins_over_the_configuration_property_by_property() {
    let (mut editor, id) = visiting(4);
    configure(&mut editor, 4);
    editor.set_editorconfig(
        id,
        Some(applied("[*.rs]\nindent_style = tab\ntab_width = 4\n")),
    );
    let (settings, origins) = of(&editor, id);
    assert_eq!(settings, Settings::new(4, Style::Tabs, 4).unwrap());
    assert_eq!(
        origins,
        Origins {
            indent_size: Origin::EditorConfig,
            style: Origin::EditorConfig,
            tab_width: Origin::EditorConfig,
        },
        "indent_style = tab makes a level a tab"
    );
}

#[test]
fn a_configurations_change_keeps_what_editorconfig_gives() {
    let (mut editor, id) = visiting(8);
    editor.set_editorconfig(id, Some(applied("[*]\nindent_style = tab\n")));
    configure(&mut editor, 8);
    let (settings, origins) = of(&editor, id);
    assert_eq!(settings.style(), Style::Tabs);
    assert_eq!(settings.indent_size(), 8, "a tab of the width shown");
    assert_eq!(origins.style, Origin::EditorConfig);
    assert_eq!(origins.tab_width, Origin::Configuration);
}

#[test]
fn a_tab_width_other_than_the_one_shown_waits_for_the_next_visit() {
    let (mut editor, id) = visiting(8);
    editor.set_editorconfig(id, Some(applied("[*]\nindent_size = tab\ntab_width = 3\n")));
    let (settings, origins) = of(&editor, id);
    assert_eq!(settings, Settings::new(8, Style::Spaces, 8).unwrap());
    assert_eq!(origins.tab_width, Origin::Visited);
    assert_eq!(origins.indent_size, Origin::EditorConfig);
    configure(&mut editor, 4);
    assert_eq!(of(&editor, id).1.tab_width, Origin::Visited);
}

#[test]
fn with_no_editorconfig_a_buffer_takes_the_configuration() {
    let (mut editor, id) = visiting(4);
    configure(&mut editor, 4);
    editor.set_editorconfig(id, Some(applied("[*.md]\nindent_size = 3\n")));
    let (settings, origins) = of(&editor, id);
    assert_eq!(settings, Settings::new(2, Style::Spaces, 4).unwrap());
    assert_eq!(origins.indent_size, Origin::Configuration);
    editor.set_editorconfig(id, None);
    assert_eq!(editor.buffer(id).unwrap().editorconfig(), None);
    assert_eq!(
        of(&editor, id).0,
        Settings::new(2, Style::Spaces, 4).unwrap()
    );
}

#[test]
fn the_defaults_say_so() {
    let (editor, id) = visiting(8);
    assert_eq!(of(&editor, id), (Settings::default(), Origins::default()));
    assert_eq!(Origins::default().tab_width, Origin::Default);
}

#[test]
fn an_internal_buffer_takes_nothing() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a", ""));
    let id = editor.add_buffer(Buffer::internal("*x*"));
    editor.set_editorconfig(id, Some(applied("[*]\nindent_size = 3\n")));
    assert_eq!(
        editor.buffer(id).unwrap().indent_settings(),
        Settings::default()
    );
}

#[test]
fn a_new_name_asks_for_its_editorconfig_once() {
    let (mut editor, id) = visiting(8);
    editor.request_editorconfig(id);
    editor.request_editorconfig(id);
    let requests = editor.take_editorconfig_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].buffer, id);
    assert_eq!(requests[0].location, PathBuf::from("/p/a.rs"));
    assert!(editor.take_editorconfig_requests().is_empty());
}
