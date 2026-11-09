//! What `C-x b` and `C-x k` complete over: the buffers, most recent first, each with what
//! marginalia says of a buffer (ADR 0088).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use std::path::Path;
use std::sync::Arc;

use oso_core::buffer::Buffer;
use oso_core::command::BufferPurpose;
use oso_core::complete::Candidate;
use oso_core::editor::Editor;

/// The buffers a prompt read for `purpose` lists, in its order: for showing, the current buffer
/// last, so that `RET` goes back to the one before; for killing, the current first.
pub(super) fn candidates(editor: &Editor, purpose: BufferPurpose) -> Arc<[Candidate]> {
    let mut order = editor.buffers_by_recency();
    if purpose == BufferPurpose::Switch && !order.is_empty() {
        order.rotate_left(1);
    }
    order
        .into_iter()
        .filter_map(|id| editor.buffer(id))
        .map(|buffer| Candidate::new(buffer.name(), "", annotation(buffer, editor.home())))
        .collect()
}

/// `*` when modified, the size, the mode, and the file's path with the home directory as `~`.
fn annotation(buffer: &Buffer, home: Option<&Path>) -> String {
    let modified = if buffer.is_modified() { "*" } else { " " };
    let size = match buffer.loading() {
        Some(_) => "loading".to_owned(),
        None => size(buffer.text().len()),
    };
    let mode = buffer.mode().name();
    let path = buffer.path().map(|path| abbreviated(path, home));
    match path {
        Some(path) => format!("{modified} {size:>7}  {mode}  {path}"),
        None => format!("{modified} {size:>7}  {mode}"),
    }
}

/// A size as `ls -h` writes it: `812`, `1.2k`, `34M`.
pub(crate) fn size(bytes: usize) -> String {
    const UNITS: [&str; 4] = ["k", "M", "G", "T"];
    if bytes < 1024 {
        return bytes.to_string();
    }
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024 * 1024 && unit + 1 < UNITS.len() {
        value /= 1024;
        unit += 1;
    }
    // Tenths of the unit, shown only below ten.
    let tenths = value * 10 / 1024;
    if tenths < 100 {
        format!("{}.{}{}", tenths / 10, tenths % 10, UNITS[unit])
    } else {
        format!("{}{}", tenths / 10, UNITS[unit])
    }
}

/// `path` with the home directory written as `~`.
pub(super) fn abbreviated(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn sizes_read_as_ls_h_writes_them() {
        assert_eq!(size(812), "812");
        assert_eq!(size(1024), "1.0k");
        assert_eq!(size(1300), "1.2k");
        assert_eq!(size(34 * 1024 * 1024), "34M");
        assert_eq!(size(10 * 1024), "10k");
    }

    #[test]
    fn the_home_directory_is_written_as_a_tilde() {
        let home = PathBuf::from("/home/me");
        let at = |path: &str| abbreviated(Path::new(path), Some(&home));
        assert_eq!(at("/home/me/notes/a.md"), "~/notes/a.md");
        assert_eq!(at("/home/me"), "~");
        assert_eq!(at("/home/meadow/a.md"), "/home/meadow/a.md");
        assert_eq!(abbreviated(Path::new("/a"), None), "/a");
    }

    #[test]
    fn showing_lists_the_current_buffer_last_and_killing_first() {
        let mut editor = Editor::default();
        let other = editor.add_buffer(Buffer::from_text("other", "x"));
        editor.show(other);
        let names = |purpose| -> Vec<String> {
            candidates(&editor, purpose)
                .iter()
                .map(|candidate| candidate.text().to_owned())
                .collect()
        };
        assert_eq!(
            names(BufferPurpose::Switch),
            ["*scratch*", "*Messages*", "other"]
        );
        assert_eq!(
            names(BufferPurpose::Kill),
            ["other", "*scratch*", "*Messages*"]
        );
    }
}
