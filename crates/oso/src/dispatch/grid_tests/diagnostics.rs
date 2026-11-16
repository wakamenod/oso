//! Text-grid snapshots of a language server's diagnostics (ADR 0138, step 7): the sign column,
//! each severity's wavy line, the most severe where they meet, faded and struck code, an empty
//! range's cell, the line numbers beside the signs, and the ranges following an edit.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::buffer::{Buffer, ServerMark};
use oso_core::diagnostic::{Diagnostics, New, Severity, Tags};
use oso_core::editor::Editor;
use oso_core::frame::FrameSize;

use super::{aids_off, check_editor};

/// What a server reports: a range, a severity and the tags.
type Reported = (std::ops::Range<usize>, Severity, Tags);

/// An editor on `text` in a frame of `rows` by `columns`, which a server follows and has
/// reported `entries` for.
fn followed(text: &str, (rows, columns): (usize, usize), entries: &[Reported]) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t.rs", text));
    aids_off(&mut editor);
    editor.set_frame_size(FrameSize { rows, columns });
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.set_followed(true);
    buffer.set_server(Some(ServerMark::Ready));
    let new = entries.iter().map(|(range, severity, tags)| New {
        range: range.clone(),
        severity: *severity,
        tags: *tags,
        source: "",
        code: "",
        message: "m",
    });
    let set = Diagnostics::new("fake", new, text.len());
    let revision = buffer.revision();
    assert!(buffer.take_diagnostics(revision, set));
    editor.fit_gutter();
    editor
}

const PLAIN: Tags = Tags {
    unnecessary: false,
    deprecated: false,
};

#[test]
fn diagnostics_draw_lines_signs_and_counts() {
    let text = "let x: u8 = 300\nuse std::fs;\nold();\nfn f() {}\n";
    let entries = [
        // A warning over `x: u8 = 300` with an error on `300`, and the missing `;` at the
        // line's end.
        (4..15, Severity::Warning, PLAIN),
        (12..15, Severity::Error, PLAIN),
        (15..15, Severity::Error, PLAIN),
        // An unused import, a hint: faded, with no line and no sign.
        (
            16..28,
            Severity::Hint,
            Tags {
                unnecessary: true,
                deprecated: false,
            },
        ),
        // A deprecated call, a warning.
        (
            29..32,
            Severity::Warning,
            Tags {
                unnecessary: false,
                deprecated: true,
            },
        ),
        // Information, on `f`.
        (39..40, Severity::Information, PLAIN),
    ];
    let editor = followed(text, (7, 40), &entries);
    let on = "M-x g l o b a l - d i s p l a y - l i n e - n u m b e r s - m o d e RET";
    // Typing before the error moves it, and its sign stays on its line.
    check_editor(
        "diagnostics",
        text.as_bytes(),
        editor,
        &["", on, "C-f C-f C-f C-f a"],
    );
}
