//! The file's indentation settings (ADR 0115): `[indent] electric`, and each mode's indentation
//! in `[modes.<mode>]`: `indent-size`, `indent-style` and `tab-width`.
//!
//! Must not depend on: `app` or `dispatch`; applying a setting, and when, is the caller's.

use oso_core::indent::{self, Origin, Style};
use toml::Spanned;
use toml::de::DeValue;

use super::modes::{MODE_COUNT, MODES};
use super::problems::{Found, quoted};
use super::settings::switch;
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
    /// The setting these give, the compiled-in one's values where they give nothing. Where
    /// the compiled-in one indents a tab a level (Go's, ADR 0164), a tab width given alone
    /// sets the indent size too, so that a level stays one tab.
    pub fn over(self, compiled: indent::Settings) -> indent::Settings {
        let a_tab_a_level = compiled.style() == Style::Tabs
            && compiled.indent_size() == compiled.tab_width().get() as usize;
        let size = match (self.indent_size, self.tab_width) {
            (Some(size), _) => size,
            (None, Some(width)) if a_tab_a_level => width,
            (None, _) => u32::try_from(compiled.indent_size()).unwrap_or(4),
        };
        let style = self.style.unwrap_or(compiled.style());
        let width = self.tab_width.unwrap_or(compiled.tab_width().get());
        indent::Settings::new(size, style, width).unwrap_or(compiled)
    }

    /// Where each property of [`ModeIndent::over`]'s setting comes from: the file, or the
    /// compiled-in default.
    pub fn origins(self) -> indent::Origins {
        let from = |given: bool| {
            if given {
                Origin::Configuration
            } else {
                Origin::Default
            }
        };
        indent::Origins {
            indent_size: from(self.indent_size.is_some()),
            style: from(self.style.is_some()),
            tab_width: from(self.tab_width.is_some()),
        }
    }
}

/// `[modes]`: each mode's, in the order of [`MODES`].
pub type Modes = [ModeIndent; MODE_COUNT];

/// `[indent]`.
pub(super) fn indent(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Indent {
    let mut indent = Indent::default();
    let Some(entries) = table(value, "[indent]", found) else {
        return indent;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "electric" => indent.electric = switch(value, "[indent]", "electric", found),
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

/// A value for a message: a string quoted, anything else by its kind.
fn shown(value: &DeValue<'_>) -> String {
    match value {
        DeValue::String(text) => quoted(text).to_string(),
        DeValue::Integer(integer) => integer.as_str().to_owned(),
        other => type_name(other).to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Mode;
    use oso_core::indent::{Settings, Style};

    use super::ModeIndent;

    /// Go indents a tab a level: a tab width given alone keeps a level one tab, and an indent
    /// size given says otherwise (ADR 0164).
    #[test]
    fn a_tab_width_alone_keeps_a_tab_a_level() {
        let go = Mode::Go.default_indent();
        let width = ModeIndent {
            tab_width: Some(4),
            ..ModeIndent::default()
        };
        assert_eq!(width.over(go), Settings::new(4, Style::Tabs, 4).unwrap());
        let both = ModeIndent {
            indent_size: Some(8),
            tab_width: Some(4),
            ..ModeIndent::default()
        };
        assert_eq!(both.over(go), Settings::new(8, Style::Tabs, 4).unwrap());
        // Spaces a level do not follow the tab width.
        let html = Mode::Html.default_indent();
        assert_eq!(
            width.over(html),
            Settings::new(2, Style::Spaces, 4).unwrap()
        );
    }
}
