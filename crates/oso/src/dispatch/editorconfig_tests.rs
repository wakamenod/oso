//! Scenario tests of what EditorConfig gives a buffer, through the [`Dispatcher`] (ADR 0037,
//! ADR 0115): the keys indenting by it, over the configuration, through a mode command and a
//! configuration's change, and `describe-indentation`.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use std::path::PathBuf;

use oso_core::buffer::{Baseline, Buffer, BufferId, Mode};
use oso_core::command::{Args, Invocation};
use oso_core::display_map::TabWidth;
use oso_core::editor::{Editor, ProjectRequest, Status};
use oso_core::editorconfig::{Applied, Budget, File, Properties};
use oso_core::indent::{Origin, Origins, Settings, Style};
use oso_core::key::KeySequence;
use oso_core::load::decode_all;
use oso_core::project::{Found, RootIdentity};
use oso_core::selection::{Selection, SelectionSet};
use oso_core::window::{Scroll, ViewSize};

use super::Dispatcher;

/// The project the file is in.
const ROOT: &str = "/p";

/// A dispatcher and an editor visiting `/p/src/a.rs`, shown with tabs `tab_width` wide,
/// holding `text` with the cursor at its `|`, in the project at [`ROOT`].
fn visiting(text: &str, tab_width: u32) -> (Dispatcher, Editor, BufferId) {
    let location = PathBuf::from("/p/src/a.rs");
    let mut buffer = Buffer::visiting(location.clone());
    let plain = text.replace('|', "");
    buffer.apply_load(decode_all(plain.as_bytes(), TabWidth::new(tab_width)));
    buffer.set_baseline(Baseline {
        location: location.clone(),
        file: None,
    });
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(10, 40));
    let id = editor.window().buffer();
    let request = ProjectRequest {
        buffer: id,
        location,
    };
    let found = Found {
        root: RootIdentity {
            path: PathBuf::from(ROOT),
            identity: None,
        },
        worktree_of: None,
        checked_main: None,
    };
    editor.project_found(&request, Some(found));
    let at = text.find('|').unwrap();
    let set = SelectionSet::from_parts(vec![Selection::at(at)], 0).unwrap();
    editor.restore_positions(set, Scroll::default());
    (Dispatcher::new(), editor, id)
}

/// What the files `(path, text)`, nearest first, give `/p/src/a.rs`.
fn applied(files: &[(&str, &str)]) -> Applied {
    let mut budget = Budget::default();
    let parsed: Vec<File> = files
        .iter()
        .map(|(_, text)| File::parse(text, &mut budget))
        .collect();
    let mut properties = Properties::default();
    for (source, (path, _)) in files.iter().enumerate().rev() {
        let dir = PathBuf::from(path).parent().unwrap().to_path_buf();
        let relative = PathBuf::from("/p/src/a.rs");
        let relative = relative
            .strip_prefix(&dir)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        properties.apply(&parsed[source], source, &relative);
    }
    Applied {
        properties,
        files: files.iter().map(|(path, _)| PathBuf::from(path)).collect(),
    }
}

/// Each mode two spaces wide from the configuration.
fn configure_two_spaces(editor: &mut Editor) {
    let two = Settings::new(2, Style::Spaces, 8).unwrap();
    let origins = Origins {
        indent_size: Origin::Configuration,
        style: Origin::Configuration,
        tab_width: Origin::Default,
    };
    let modes = [Mode::Rust, Mode::Markdown, Mode::Toml, Mode::Fundamental];
    editor.set_mode_indents(modes.map(|mode| (mode, two, origins)));
}

fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, keys: &str) {
    let keys: KeySequence = keys.parse().unwrap();
    dispatcher.handle_keys(editor, keys.0);
}

fn run(dispatcher: &mut Dispatcher, editor: &mut Editor, name: &str) {
    let id = dispatcher.registry.lookup(name).unwrap();
    let step = super::Step::MayJoin;
    let args = Args::with_prefix(None);
    let (_, result) = dispatcher.execute(editor, id, args, Invocation::default(), step);
    result.unwrap();
}

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

#[test]
fn the_keys_indent_with_tabs_when_editorconfig_says_so_over_the_configuration() {
    let (mut dispatcher, mut editor, id) = visiting("fn f() {|}\n", 4);
    configure_two_spaces(&mut editor);
    let tabs = applied(&[(
        "/p/.editorconfig",
        "root = true\n[*.rs]\nindent_style = tab\ntab_width = 4\n",
    )]);
    editor.set_editorconfig(id, Some(tabs));
    send(&mut dispatcher, &mut editor, "RET");
    assert_eq!(text(&editor), "fn f() {\n\t\n}\n");
}

#[test]
fn tab_reindents_spaces_into_a_tab() {
    let (mut dispatcher, mut editor, id) = visiting("fn f() {\n  |x\n}\n", 4);
    let tabs = applied(&[(
        "/p/.editorconfig",
        "[*]\nindent_style = tab\nindent_size = tab\n",
    )]);
    editor.set_editorconfig(id, Some(tabs));
    send(&mut dispatcher, &mut editor, "TAB");
    assert_eq!(text(&editor), "fn f() {\n\tx\n}\n");
}

#[test]
fn a_configurations_change_and_a_mode_command_keep_what_editorconfig_gives() {
    let (mut dispatcher, mut editor, id) = visiting("fn f() {\n|x\n}\n", 8);
    let three = applied(&[(
        "/p/src/.editorconfig",
        "[*]\nindent_size = 3\ntab_width = 8\n",
    )]);
    editor.set_editorconfig(id, Some(three));
    configure_two_spaces(&mut editor);
    send(&mut dispatcher, &mut editor, "TAB");
    assert_eq!(text(&editor), "fn f() {\n   x\n}\n");
    run(&mut dispatcher, &mut editor, "fundamental-mode");
    run(&mut dispatcher, &mut editor, "rust-mode");
    assert_eq!(
        editor.buffer(id).unwrap().indent_settings().indent_size(),
        3
    );
}

#[test]
fn describe_indentation_names_each_properties_origin_and_files_in_the_project() {
    let (mut dispatcher, mut editor, id) = visiting("|x\n", 8);
    configure_two_spaces(&mut editor);
    let files = applied(&[
        ("/p/src/.editorconfig", "[*.rs]\nindent_style = tab\n"),
        ("/p/.editorconfig", "root = true\n[*]\ntab_width = 8\n"),
    ]);
    editor.set_editorconfig(id, Some(files));
    run(&mut dispatcher, &mut editor, "describe-indentation");
    assert_eq!(
        echo(&editor),
        "Rust mode: indent size 8 (EditorConfig src/.editorconfig), tabs (EditorConfig \
         src/.editorconfig), tab width 8 (EditorConfig .editorconfig)"
    );
}

#[test]
fn describe_indentation_names_no_file_outside_the_project() {
    let (mut dispatcher, mut editor, id) = visiting("|x\n", 8);
    configure_two_spaces(&mut editor);
    let home = applied(&[("/.editorconfig", "[*]\nindent_style = tab\n")]);
    editor.set_editorconfig(id, Some(home));
    run(&mut dispatcher, &mut editor, "describe-indentation");
    let said = echo(&editor);
    assert_eq!(
        said,
        "Rust mode: indent size 8 (EditorConfig outside the project), tabs (EditorConfig \
         outside the project), tab width 8 (the default)"
    );
    assert!(!said.contains('/'));
}

#[test]
fn describe_indentation_says_the_configuration_and_the_default() {
    let (mut dispatcher, mut editor, _) = visiting("|x\n", 8);
    run(&mut dispatcher, &mut editor, "describe-indentation");
    assert_eq!(
        echo(&editor),
        "Rust mode: indent size 4 (the default), spaces (the default), tab width 8 (the default)"
    );
    configure_two_spaces(&mut editor);
    run(&mut dispatcher, &mut editor, "describe-indentation");
    assert_eq!(
        echo(&editor),
        "Rust mode: indent size 2 (the configuration), spaces (the configuration), tab width 8 \
         (the default)"
    );
}

#[test]
fn a_tab_width_read_after_the_visit_waits_for_the_next_one() {
    let (mut dispatcher, mut editor, id) = visiting("|x\n", 8);
    let four = applied(&[(
        "/p/.editorconfig",
        "[*]\nindent_style = tab\ntab_width = 4\n",
    )]);
    editor.set_editorconfig(id, Some(four));
    run(&mut dispatcher, &mut editor, "describe-indentation");
    assert_eq!(
        echo(&editor),
        "Rust mode: indent size 8 (EditorConfig .editorconfig), tabs (EditorConfig \
         .editorconfig), tab width 8 (as visited; another applies at the next visit)"
    );
}

#[test]
fn describe_indentation_only_looks() {
    let dispatcher = Dispatcher::new();
    let id = dispatcher.registry.lookup("describe-indentation").unwrap();
    assert_eq!(
        dispatcher.registry.get(id).unwrap().reach,
        oso_core::command::Reach::Look
    );
}
