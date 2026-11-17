//! The names and paths a rename shows (ADR 0138, step 14): the identifier at point, for a
//! server that checks no place first, and a buffer's path as the preview shows it.
//!
//! Must not depend on: the window system, the servers or the UI thread's state.

use std::path::Path;

/// The most bytes of a name, old or new.
pub(super) const NEW_NAME_MAX: usize = oso_lsp::limits::NEW_NAME_BYTES_MAX;

/// The identifier around `offset` of `text`: its letters, digits and `_`; `None` for none.
pub(super) fn identifier_at(text: &str, offset: usize) -> Option<String> {
    let offset = text.floor_char_boundary(offset.min(text.len()));
    // A combining mark joins the identifier it follows, as a whole cluster does in a popup's:
    // a script's own signs are alphabetic, and the diacritics are these blocks.
    let word = |c: char| c.is_alphanumeric() || c == '_' || combining(c);
    let start = (text[..offset].char_indices().rev())
        .take_while(|(_, c)| word(*c))
        .last()
        .map_or(offset, |(at, _)| at);
    let end = (text[offset..].char_indices())
        .find(|(_, c)| !word(*c))
        .map_or(text.len(), |(at, _)| offset + at);
    (start < end && end - start <= NEW_NAME_MAX).then(|| text[start..end].to_owned())
}

/// Whether `c` is a combining diacritic, which no letter test takes: the combining diacritical
/// marks, their extended and supplement blocks, those for symbols, and the half marks.
fn combining(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F
    )
}

/// `path` as the preview shows it: from `root`, spelled with `/`, else whole; `name` for a
/// buffer with no file.
pub(super) fn shown_path(path: Option<&Path>, root: &Path, name: &str) -> String {
    let Some(path) = path else {
        return name.to_owned();
    };
    match path.strip_prefix(root) {
        Ok(relative) => {
            let parts: Vec<String> = (relative.components())
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            parts.join("/")
        }
        Err(_) => path.to_string_lossy().into_owned(),
    }
}
