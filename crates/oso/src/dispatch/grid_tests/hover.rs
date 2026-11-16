//! Text-grid snapshots of a server's description at point (ADR 0138, step 10): its float right
//! below the cursor's row, Markdown highlighted, wrapped to its width; a page further on
//! `C-v`; and gone with the next key, which moves the cursor as ever.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::buffer::{Buffer, ServerMark};
use oso_core::editor::Editor;
use oso_core::frame::FrameSize;
use oso_core::window::Screen;

use super::rows::{Style, blank_row, draw_row, screen_row};
use super::{aids_off, check_editor};

/// The description's float, if one shows: where it is, then its rows between `┊` edges, with
/// no cursor.
pub(super) fn draw_hover(editor: &Editor, lines: &mut Vec<String>) {
    let (Some(regions), Some(shown)) = (editor.hover_layout(), editor.hover()) else {
        return;
    };
    let Some(buffer) = editor.buffer(shown.buffer()) else {
        return;
    };
    let (outer, inner) = (regions.outer, regions.inner);
    lines.push(format!(
        "description at row {}, column {}: {} x {}",
        outer.row, outer.column, outer.rows, outer.columns
    ));
    let mut screen = Screen::default();
    editor.hover_screen(&mut screen);
    let display = buffer.display();
    let style = Style {
        cursor: ' ',
        whitespace: false,
    };
    for index in 0..inner.rows {
        let (text, _) = match screen_row(&screen, index, inner.columns) {
            Some(row) => draw_row(&display, &row, 0..0, None, style),
            None => blank_row(inner.columns),
        };
        lines.push(format!("┊{text}┊"));
    }
}

#[test]
fn a_description_shows_below_the_cursor_scrolls_and_goes() {
    let text = "fn main() {\n    run();\n}\n";
    let mut editor = Editor::with_buffer(Buffer::from_text("t.rs", text));
    aids_off(&mut editor);
    editor.set_frame_size(FrameSize {
        rows: 16,
        columns: 50,
    });
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.set_followed(true);
    buffer.set_server(Some(ServerMark::Ready));
    let mut description = String::from(
        "```rust\nfn main()\n```\n\nThe program's *entry*, which runs first and a little more than fits.\n",
    );
    for n in 1..=12 {
        description.push_str(&format!("\nLine {n}."));
    }
    let at = editor.ask_at();
    assert!(editor.show_hover(at, "fake", &description, true));
    check_editor("hover", text.as_bytes(), editor, &["", "C-v", "C-f"]);
}
