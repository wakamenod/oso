//! The control socket's `clipboard` method (ADR 0074, ADR 0142): what it asks of the clipboard
//! in memory, and how its parameters are read. Its `Debug` gives lengths only.
//!
//! Must not depend on: the app or the clipboard's threads; the app carries the op out.

use serde_json::{Map, Value};

/// What `clipboard` does with the clipboard in memory. Its `Debug` gives lengths only.
#[derive(Clone, PartialEq, Eq)]
pub enum ClipboardOp {
    /// Report what it holds.
    Get,
    /// Put text there, marked concealed or not.
    Set { text: String, concealed: bool },
    /// Put an image there in place of any text, marked concealed or not (ADR 0142).
    SetImage {
        kind: oso_platform::clipboard::ImageKind,
        bytes: Vec<u8>,
        concealed: bool,
    },
}

impl std::fmt::Debug for ClipboardOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Get => f.write_str("Get"),
            Self::Set { text, concealed } => f
                .debug_struct("Set")
                .field("len", &text.len())
                .field("concealed", concealed)
                .finish(),
            Self::SetImage {
                kind,
                bytes,
                concealed,
            } => f
                .debug_struct("SetImage")
                .field("kind", kind)
                .field("len", &bytes.len())
                .field("concealed", concealed)
                .finish(),
        }
    }
}

/// `clipboard` with no parameters reads, and with `text`, and `concealed` if it is to
/// be marked so, sets. With `image`, a type (`png`, `jpeg`, `gif`, `webp` or `tiff`), and
/// `bytes`, an array of byte values, it puts an image there.
pub(super) fn parse_clipboard(params: &Map<String, Value>) -> Result<ClipboardOp, String> {
    use oso_platform::clipboard::ImageKind;
    let concealed = match params.get("concealed") {
        None => false,
        Some(Value::Bool(concealed)) => *concealed,
        Some(_) => return Err("clipboard's \"concealed\" must be a boolean".into()),
    };
    if let Some(image) = params.get("image") {
        const USAGE: &str = "clipboard's \"image\" is one of png, jpeg, gif, webp and tiff, with \
                             \"bytes\", an array of byte values";
        let kind = match image.as_str() {
            Some("png") => ImageKind::Png,
            Some("jpeg") => ImageKind::Jpeg,
            Some("gif") => ImageKind::Gif,
            Some("webp") => ImageKind::Webp,
            Some("tiff") => ImageKind::Tiff,
            _ => return Err(USAGE.into()),
        };
        let bytes = params
            .get("bytes")
            .and_then(Value::as_array)
            .and_then(|values| {
                values
                    .iter()
                    .map(|value| value.as_u64().and_then(|byte| u8::try_from(byte).ok()))
                    .collect::<Option<Vec<u8>>>()
            })
            .ok_or(USAGE)?;
        return Ok(ClipboardOp::SetImage {
            kind,
            bytes,
            concealed,
        });
    }
    match params.get("text") {
        None if params.is_empty() => Ok(ClipboardOp::Get),
        Some(Value::String(text)) => Ok(ClipboardOp::Set {
            text: text.clone(),
            concealed,
        }),
        _ => Err("clipboard takes a string \"text\", and \"concealed\", or nothing".into()),
    }
}
