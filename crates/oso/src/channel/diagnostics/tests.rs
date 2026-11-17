//! Tests of the agent channel's `diagnostics` (ADR 0138, step 11): a buffer's, its project's
//! with the servers' reports of files no buffer holds, the order, the bound and `more`, the
//! columns, the messages as the agent gets them, and the buffers it refuses.
//!
//! Must not depend on: sockets, threads or `app`.

use std::path::{Path, PathBuf};

use oso_core::buffer::{Baseline, Buffer, BufferId};
use oso_core::diagnostic::{Diagnostics, New, Severity, Tags};
use oso_core::editor::Editor;
use oso_core::project::{Found, RootIdentity};
use oso_lsp::position::{Encoding, Position};
use oso_lsp::protocol::{Published, Reported};
use serde_json::Value;

use std::sync::Arc;

use super::{ANSWER_BYTES, FileReport, MAX_ENTRIES, MESSAGE_BYTES, server_text, start};

/// A judge of files that takes those named `conf.ts` to lead to a secret file, as a link may.
fn secret_file(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "conf.ts")
}

/// The answer to `of`, with `files` as the project's servers' reports.
fn diagnostics(
    editor: &Editor,
    files: &[(&str, &Published, Encoding)],
    of: DiagnosticsOf,
) -> Result<Value, String> {
    let reports = |_: &Path| {
        files
            .iter()
            .map(|(path, report, encoding)| FileReport {
                path: Arc::from(Path::new(path)),
                report: Arc::new((*report).clone()),
                encoding: *encoding,
                server: Arc::from("fake"),
            })
            .collect()
    };
    start(editor, &reports, secret_file, of).map(|job| job.run())
}
use crate::channel::protocol::DiagnosticsOf;

/// A buffer visiting `path`, holding `text`, judged ordinary.
fn visiting(path: &str, text: &str) -> Buffer {
    let path = PathBuf::from(path);
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(oso_core::load::decode_all(
        text.as_bytes(),
        oso_core::display_map::TabWidth::default(),
    ));
    buffer.judge_secret(Some(std::slice::from_ref(&path)));
    buffer.set_baseline(Baseline {
        location: path,
        file: None,
    });
    buffer.set_followed(true);
    buffer
}

/// Puts buffer `id` in the project at `/w/oso`.
fn in_project(editor: &mut Editor, id: BufferId) {
    editor.request_project(id);
    let request = editor.take_project_requests().pop().unwrap();
    let found = Found {
        root: RootIdentity {
            path: PathBuf::from("/w/oso"),
            identity: cfg!(unix).then_some((1, 7)),
        },
        worktree_of: None,
        checked_main: None,
    };
    editor.project_found(&request, Some(found));
}

/// Gives buffer `id` errors at the offsets `at`, and a hint, from the server `fake`.
fn report(editor: &mut Editor, id: BufferId, at: &[(usize, Severity, &str)]) {
    let buffer = editor.buffer_mut(id).unwrap();
    let new = at.iter().map(|&(start, severity, message)| New {
        range: start..start + 1,
        severity,
        tags: Tags::default(),
        source: "rustc",
        code: "E0308",
        message,
    });
    let set = Diagnostics::new("fake", new, buffer.text().len());
    let revision = buffer.revision();
    assert!(buffer.take_diagnostics(revision, set));
}

fn file_report(at: &[(u32, u32, Severity)]) -> Published {
    Published {
        uri: String::new(),
        version: None,
        diagnostics: at
            .iter()
            .map(|&(line, character, severity)| Reported {
                start: Position { line, character },
                end: Position { line, character },
                severity,
                tags: Tags::default(),
                source: "clippy".into(),
                code: String::new(),
                message: format!("on {line}"),
                code_number: false,
                data: None,
            })
            .collect(),
    }
}

fn places(answer: &Value) -> Vec<(String, u64, u64)> {
    answer["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let path = entry["path"].as_str().unwrap();
            let name = Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (
                name,
                entry["line"].as_u64().unwrap(),
                entry["column"].as_u64().unwrap(),
            )
        })
        .collect()
}

#[test]
fn a_buffer_s_own_come_by_place_with_columns_in_characters_and_no_hints() {
    let mut editor = Editor::with_buffer(visiting("/w/oso/a.rs", "let é = 1;\nfoo();\n"));
    let id = editor.tiled().0.buffer();
    let foo = "let é = 1;\n".len();
    report(
        &mut editor,
        id,
        &[
            (foo, Severity::Error, "not found"),
            ("let é = ".len(), Severity::Warning, "unused\nmore"),
            (0, Severity::Hint, "a hint"),
        ],
    );
    let of = DiagnosticsOf {
        buffer: None,
        project: false,
    };
    let answer = diagnostics(&editor, &[], of).unwrap();
    assert_eq!(
        places(&answer),
        [("a.rs".into(), 1, 9), ("a.rs".into(), 2, 1)],
        "`é` is one character"
    );
    let first = &answer["entries"][0];
    assert_eq!(first["severity"], "warning");
    assert_eq!(first["message"], "unused\nmore", "line breaks kept");
    assert_eq!(
        (&first["source"], &first["server"]),
        (&Value::from("rustc"), &Value::from("fake"))
    );
    assert!(first.get("column_unit").is_none());
    assert_eq!(answer["more"], 0);
}

#[test]
fn a_project_s_add_other_buffers_and_files_by_path_with_the_server_s_columns() {
    let mut editor = Editor::with_buffer(visiting("/w/oso/z.rs", "x\n"));
    let first = editor.tiled().0.buffer();
    let other = editor.add_buffer(visiting("/w/oso/b.rs", "y\n"));
    in_project(&mut editor, first);
    report(&mut editor, first, &[(0, Severity::Error, "z")]);
    report(&mut editor, other, &[(0, Severity::Error, "b")]);
    let (a, outside) = (
        file_report(&[(4, 2, Severity::Warning)]),
        file_report(&[(0, 0, Severity::Error)]),
    );
    let files = [
        ("/w/oso/src/a.rs", &a, Encoding::Utf16),
        ("/w/elsewhere/x.rs", &outside, Encoding::Utf8),
    ];
    let of = DiagnosticsOf {
        buffer: None,
        project: true,
    };
    let answer = diagnostics(&editor, &files, of).unwrap();
    assert_eq!(
        places(&answer),
        [
            ("z.rs".into(), 1, 1),
            ("b.rs".into(), 1, 1),
            ("a.rs".into(), 5, 3)
        ]
    );
    assert_eq!(answer["entries"][2]["column_unit"], "utf-16");
    assert_eq!(answer["project"], "/w/oso");
    // Of a buffer in no project, a project's is refused.
    let lone = Editor::with_buffer(visiting("/w/lone.rs", "x\n"));
    assert!(diagnostics(&lone, &[], of).is_err());
}

#[test]
fn past_the_bound_the_rest_are_counted() {
    let text = "x\n".repeat(MAX_ENTRIES + 20);
    let mut editor = Editor::with_buffer(visiting("/w/oso/a.rs", &text));
    let id = editor.tiled().0.buffer();
    let at: Vec<_> = (0..MAX_ENTRIES + 20)
        .map(|n| (2 * n, Severity::Error, "e"))
        .collect();
    report(&mut editor, id, &at);
    let of = DiagnosticsOf {
        buffer: Some(id.get()),
        project: false,
    };
    let answer = diagnostics(&editor, &[], of).unwrap();
    assert_eq!(answer["entries"].as_array().unwrap().len(), MAX_ENTRIES);
    assert_eq!(answer["more"], 20);
}

#[test]
fn a_buffer_that_may_hold_secrets_or_is_withheld_gives_none() {
    let mut editor = Editor::with_buffer(visiting("/w/oso/.env", "KEY=1\n"));
    let id = editor.tiled().0.buffer();
    let of = DiagnosticsOf {
        buffer: Some(id.get()),
        project: false,
    };
    assert!(
        diagnostics(&editor, &[], of)
            .unwrap_err()
            .contains("secrets")
    );
    let withheld = editor.add_buffer(visiting("/w/oso/w.rs", "x\n"));
    editor.buffer_mut(withheld).unwrap().withhold_from_agents();
    let of = DiagnosticsOf {
        buffer: Some(withheld.get()),
        project: false,
    };
    assert!(
        diagnostics(&editor, &[], of)
            .unwrap_err()
            .contains("only the person")
    );
    let none = DiagnosticsOf {
        buffer: Some(999),
        project: false,
    };
    assert!(diagnostics(&editor, &[], none).is_err());
}

#[test]
fn a_message_is_cut_and_its_controls_written_out_but_its_line_breaks() {
    assert_eq!(
        server_text("a\u{202e}b\r\nc\u{7}", true),
        "a\\u{202e}b\\u{d}\nc\\u{7}"
    );
    let long = "é".repeat(MESSAGE_BYTES);
    assert!(server_text(&long, true).len() <= MESSAGE_BYTES);
}

#[test]
fn an_answer_holds_no_more_than_its_bytes_and_counts_the_rest() {
    let text = "x\n".repeat(MAX_ENTRIES);
    let mut editor = Editor::with_buffer(visiting("/w/oso/a.rs", &text));
    let id = editor.tiled().0.buffer();
    let long = "\u{1}".repeat(MESSAGE_BYTES);
    let at: Vec<_> = (0..MAX_ENTRIES)
        .map(|n| (2 * n, Severity::Error, long.as_str()))
        .collect();
    report(&mut editor, id, &at);
    let of = DiagnosticsOf {
        buffer: None,
        project: false,
    };
    let answer = diagnostics(&editor, &[], of).unwrap();
    let made = answer["entries"].as_array().unwrap().len();
    assert!(made < MAX_ENTRIES, "{made}");
    assert_eq!(answer["more"], MAX_ENTRIES - made);
    assert!(answer.to_string().len() < 1 << 20, "it fits a reply line");
    assert!(answer.to_string().len() > ANSWER_BYTES / 2);
}

#[test]
fn a_project_s_leave_out_buffers_withheld_and_their_files_reports_and_escape_paths() {
    let mut editor = Editor::with_buffer(visiting("/w/oso/a.rs", "x\n"));
    let first = editor.tiled().0.buffer();
    in_project(&mut editor, first);
    // Withheld from agents, with a set of its own and a server's report of its file.
    let withheld = editor.add_buffer(visiting("/w/oso/w.rs", "y\n"));
    report(&mut editor, withheld, &[(0, Severity::Error, "quoted")]);
    editor.buffer_mut(withheld).unwrap().withhold_from_agents();
    let of_withheld = file_report(&[(0, 0, Severity::Error)]);
    // A server names a path and writes a source and a code to steer the agent.
    let mut crafted = file_report(&[(0, 0, Severity::Error)]);
    crafted.diagnostics[0].source = "clip\u{202e}py".into();
    crafted.diagnostics[0].code = "E\u{e0041}1".into();
    let files = [
        ("/w/oso/w.rs", &of_withheld, Encoding::Utf8),
        ("/w/oso/b\u{202e}sr.rs", &crafted, Encoding::Utf8),
    ];
    let of = DiagnosticsOf {
        buffer: None,
        project: true,
    };
    let answer = diagnostics(&editor, &files, of).unwrap();
    let entries = answer["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{answer}");
    let entry = &entries[0];
    assert_eq!(entry["path"], "/w/oso/b\\u{202e}sr.rs");
    assert_eq!(
        (&entry["source"], &entry["code"]),
        (
            &Value::from("clip\\u{202e}py"),
            &Value::from("E\\u{e0041}1")
        )
    );
    assert!(!answer.to_string().contains("quoted"));
    assert_eq!(answer["more"], 0, "what is left out is not counted either");
}

#[test]
fn a_file_whose_judge_says_it_may_hold_secrets_gives_nothing() {
    let mut editor = Editor::with_buffer(visiting("/w/oso/a.rs", "x\n"));
    let first = editor.tiled().0.buffer();
    in_project(&mut editor, first);
    let (linked, plain) = (
        file_report(&[(0, 0, Severity::Error)]),
        file_report(&[(1, 0, Severity::Error)]),
    );
    let files = [
        ("/w/oso/src/conf.ts", &linked, Encoding::Utf16),
        ("/w/oso/src/plain.ts", &plain, Encoding::Utf16),
    ];
    let of = DiagnosticsOf {
        buffer: None,
        project: true,
    };
    let answer = diagnostics(&editor, &files, of).unwrap();
    assert_eq!(places(&answer), [("plain.ts".into(), 2, 1)]);
    assert_eq!(answer["more"], 0);
}
