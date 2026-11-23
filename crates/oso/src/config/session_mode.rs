//! `[sessions] mode`: whether a new session in a trusted project runs native, `claude -p` shown
//! in oso's own rows (ADR 0165), the default since part 4e, or hosts Claude Code's TUI.
//!
//! Must not depend on: `app` or the file system; a plain reading of one value.

use toml::Spanned;
use toml::de::DeValue;

use super::problems::{Found, cut, quoted};
use super::type_name;

/// `[sessions] mode`: whether sessions host the TUI (ADR 0165). Anything but `"native"` or
/// `"tui"` is said, and the default, native, holds (part 4e).
pub(super) fn tui(value: &Spanned<DeValue<'_>>, found: &mut Found) -> bool {
    match value.get_ref() {
        DeValue::String(text) if text == "native" => false,
        DeValue::String(text) if text == "tui" => true,
        other => {
            let what = match other {
                DeValue::String(text) => quoted(cut(text.as_ref())).to_string(),
                other => type_name(other).to_owned(),
            };
            found.at(
                value.span().start,
                format_args!("[sessions] mode takes \"native\" or \"tui\", not {what}"),
            );
            false
        }
    }
}
