//! The modes the configuration names, by the names its tables give them: `[modes.<name>]`'s
//! indentation (ADR 0115) and `[lsp.servers.<name>] modes` (ADR 0138). One list, so that a new
//! mode is added in one place.
//!
//! Must not depend on: `app` or `dispatch`; nothing here reads a file.

use oso_core::buffer::Mode;

/// The modes the configuration names, by name, Fundamental last.
pub const MODES: [(&str, Mode); 14] = [
    ("rust", Mode::Rust),
    ("markdown", Mode::Markdown),
    ("toml", Mode::Toml),
    ("go", Mode::Go),
    ("typescript", Mode::TypeScript),
    ("tsx", Mode::Tsx),
    ("javascript", Mode::JavaScript),
    ("html", Mode::Html),
    ("css", Mode::Css),
    ("json", Mode::Json),
    ("yaml", Mode::Yaml),
    ("latex", Mode::Latex),
    ("swift", Mode::Swift),
    ("fundamental", Mode::Fundamental),
];

/// How many there are, the length of each list kept in [`MODES`]' order.
pub const MODE_COUNT: usize = MODES.len();

/// The names a server can follow, for a message: every mode's but Fundamental's.
pub fn server_mode_names() -> String {
    let names: Vec<&str> = MODES
        .iter()
        .filter(|(_, mode)| *mode != Mode::Fundamental)
        .map(|(name, _)| *name)
        .collect();
    match names.split_last() {
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}
