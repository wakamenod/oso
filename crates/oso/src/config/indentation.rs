//! The file's indentation settings (ADR 0115): `[indent] electric`, and each mode's indentation
//! in `[modes.<mode>]`: `indent-size`, `indent-style` and `tab-width`.
//!
//! Must not depend on: `app` or `dispatch`; applying a setting, and when, is the caller's.

use oso_core::buffer::Mode;
use oso_core::indent::{self, Style};
use toml::Spanned;
use toml::de::DeValue;

use super::problems::{Found, quoted};
use super::{table, type_name, unknown};

/// `[indent]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Indent {
    /// `electric`: whether `RET` and typed closers indent, at start (`electric-indent-mode`).
    pub electric: Option<bool>,
}

/// One mode's `[modes.<mode>]`: each `None` where the file leaves it out or gives a bad value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModeIndent {
    pub indent_size: Option<u32>,
    pub style: Option<Style>,
    pub tab_width: Option<u32>,
}

impl ModeIndent {
    /// The setting these give, the compiled-in one's values where they give nothing.
    pub fn over(self, compiled: indent::Settings) -> indent::Settings {
        let size = self
            .indent_size
            .unwrap_or(u32::try_from(compiled.indent_size()).unwrap_or(4));
        let style = self.style.unwrap_or(compiled.style());
        let width = self.tab_width.unwrap_or(compiled.tab_width().get());
        indent::Settings::new(size, style, width).unwrap_or(compiled)
    }
}

/// The modes `[modes]` can set, by the name their table has.
pub const MODES: [(&str, Mode); 4] = [
    ("rust", Mode::Rust),
    ("markdown", Mode::Markdown),
    ("toml", Mode::Toml),
    ("fundamental", Mode::Fundamental),
];

/// `[modes]`: each mode's, in the order of [`MODES`].
pub type Modes = [ModeIndent; 4];

/// `[indent]`.
pub(super) fn indent(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Indent {
    let mut indent = Indent::default();
    let Some(entries) = table(value, "[indent]", found) else {
        return indent;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "electric" => indent.electric = switch(value, found),
            other => unknown(found, name.span().start, other, "[indent]"),
        }
    }
    indent
}

/// `[modes]`, whose tables are the modes'.
pub(super) fn modes(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Modes {
    let mut modes = Modes::default();
    let Some(entries) = table(value, "[modes]", found) else {
        return modes;
    };
    for (name, value) in entries {
        let key = name.get_ref().as_ref();
        let Some(index) = MODES.iter().position(|(mode, _)| *mode == key) else {
            unknown(found, name.span().start, key, "[modes]");
            continue;
        };
        modes[index] = mode(value, &format!("[modes.{key}]"), found);
    }
    modes
}

/// One mode's table, called `table_name` in messages.
fn mode(value: &Spanned<DeValue<'_>>, table_name: &str, found: &mut Found) -> ModeIndent {
    let mut indent = ModeIndent::default();
    let Some(entries) = table(value, table_name, found) else {
        return indent;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "indent-size" => indent.indent_size = width(value, table_name, "indent-size", found),
            "tab-width" => indent.tab_width = width(value, table_name, "tab-width", found),
            "indent-style" => indent.style = style(value, table_name, found),
            other => unknown(found, name.span().start, other, table_name),
        }
    }
    indent
}

/// A width from 1 to 32, as EditorConfig's are taken (ADR 0115).
fn width(value: &Spanned<DeValue<'_>>, table: &str, key: &str, found: &mut Found) -> Option<u32> {
    let parsed = match value.get_ref() {
        DeValue::Integer(integer) if integer.radix() == 10 => {
            integer.as_str().replace('_', "").parse::<u32>().ok()
        }
        _ => None,
    };
    let valid = parsed.filter(|width| (1..=indent::Settings::MAX).contains(width));
    if valid.is_none() {
        found.at(
            value.span().start,
            format_args!(
                "{table} {key} takes a number from 1 to {}, not {}",
                indent::Settings::MAX,
                shown(value.get_ref())
            ),
        );
    }
    valid
}

/// `"space"` or `"tab"`.
fn style(value: &Spanned<DeValue<'_>>, table: &str, found: &mut Found) -> Option<Style> {
    match value.get_ref() {
        DeValue::String(name) if name == "space" => Some(Style::Spaces),
        DeValue::String(name) if name == "tab" => Some(Style::Tabs),
        other => {
            found.at(
                value.span().start,
                format_args!(
                    "{table} indent-style takes \"space\" or \"tab\", not {}",
                    shown(other)
                ),
            );
            None
        }
    }
}

/// `true` or `false`.
fn switch(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<bool> {
    if let DeValue::Boolean(on) = value.get_ref() {
        return Some(*on);
    }
    found.at(
        value.span().start,
        format_args!(
            "[indent] electric takes true or false, not {}",
            type_name(value.get_ref())
        ),
    );
    None
}

/// A value for a message: a string quoted, anything else by its kind.
fn shown(value: &DeValue<'_>) -> String {
    match value {
        DeValue::String(text) => quoted(text).to_string(),
        DeValue::Integer(integer) => integer.as_str().to_owned(),
        other => type_name(other).to_owned(),
    }
}
