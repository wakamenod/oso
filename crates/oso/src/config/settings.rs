//! The file's settings: the theme, the visual aids, the agent channel at launch (ADR 0108) and
//! whitespace cleanup on save (ADR 0113). Indentation's are in `indentation` (ADR 0115).
//! Each is `None` where the file leaves it out or gives a bad value, so the caller keeps its own.
//!
//! Must not depend on: `app` or `dispatch`; applying a setting, and when, is the caller's
//! (ADR 0108's "Settings and run-time choices").

use toml::Spanned;
use toml::de::DeValue;

use super::indentation::{Indent, Modes};
use super::problems::{Found, quoted};
use super::{table, type_name, unknown};

/// Every setting the file holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// `[theme]`.
    pub theme: ThemeSetting,
    /// `[aids]`.
    pub aids: Aids,
    /// `[agent-channel] start`: whether the channel starts at launch (ADR 0085).
    pub agent_channel_start: Option<bool>,
    /// `[save]`.
    pub save: Save,
    /// `[indent]` (ADR 0115).
    pub indent: Indent,
    /// `[modes.<mode>]`'s indentation (ADR 0115).
    pub modes: Modes,
}

/// `[save]`: what a save does besides writing (ADR 0113).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Save {
    /// `trim-whitespace`: clean the lines edited since the file was read or saved.
    pub trim_whitespace: Option<bool>,
}

/// `[theme]`: built-in themes, by name (ADR 0089).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeSetting {
    /// `default`: the theme to start in, until a choice made at run time.
    pub default: Option<String>,
    /// `pair`: the two themes `toggle-theme` switches between.
    pub pair: Option<[String; 2]>,
}

/// `[aids]`: the visual aids at start (ADR 0097).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Aids {
    pub whitespace: Option<bool>,
    pub current_line: Option<bool>,
    pub flash: Option<bool>,
    pub line_numbers: Option<bool>,
}

/// `[theme]`, with each name one of `themes`.
pub(super) fn theme(
    value: &Spanned<DeValue<'_>>,
    themes: &[&str],
    found: &mut Found,
) -> ThemeSetting {
    let mut theme = ThemeSetting::default();
    let Some(entries) = table(value, "[theme]", found) else {
        return theme;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "default" => theme.default = theme_name(value, "default", themes, found),
            "pair" => theme.pair = pair(value, themes, found),
            other => unknown(found, name.span().start, other, "[theme]"),
        }
    }
    theme
}

/// The two themes of `[theme] pair`, different and known.
fn pair(value: &Spanned<DeValue<'_>>, themes: &[&str], found: &mut Found) -> Option<[String; 2]> {
    let refuse = |found: &mut Found| {
        found.at(
            value.span().start,
            format_args!(
                "[theme] pair takes a list of two themes' names, such as [\"dusk\", \"light\"]"
            ),
        );
        None
    };
    let DeValue::Array(names) = value.get_ref() else {
        return refuse(found);
    };
    let [first, second] = &names[..] else {
        return refuse(found);
    };
    let first = theme_name(first, "pair", themes, found);
    let second = theme_name(second, "pair", themes, found);
    let (first, second) = (first?, second?);
    if first == second {
        found.at(
            value.span().start,
            format_args!("[theme] pair names {} twice", quoted(&first)),
        );
        return None;
    }
    Some([first, second])
}

/// A built-in theme's name, given for `[theme] key`.
fn theme_name(
    value: &Spanned<DeValue<'_>>,
    key: &str,
    themes: &[&str],
    found: &mut Found,
) -> Option<String> {
    let at = value.span().start;
    let DeValue::String(name) = value.get_ref() else {
        found.at(
            at,
            format_args!(
                "[theme] {key} takes a theme's name, not {}",
                type_name(value.get_ref())
            ),
        );
        return None;
    };
    if !themes.contains(&name.as_ref()) {
        found.at(
            at,
            format_args!("[theme] {key}: no built-in theme is named {}", quoted(name)),
        );
        return None;
    }
    Some(name.as_ref().to_owned())
}

/// `[aids]`.
pub(super) fn aids(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Aids {
    let mut aids = Aids::default();
    let Some(entries) = table(value, "[aids]", found) else {
        return aids;
    };
    for (name, value) in entries {
        let key = name.get_ref().as_ref();
        let slot = match key {
            "whitespace" => &mut aids.whitespace,
            "current-line" => &mut aids.current_line,
            "flash" => &mut aids.flash,
            "line-numbers" => &mut aids.line_numbers,
            other => {
                unknown(found, name.span().start, other, "[aids]");
                continue;
            }
        };
        *slot = switch(value, "[aids]", key, found);
    }
    aids
}

/// `[agent-channel] start`.
pub(super) fn agent_channel(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<bool> {
    let entries = table(value, "[agent-channel]", found)?;
    let mut start = None;
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "start" => start = switch(value, "[agent-channel]", "start", found),
            other => unknown(found, name.span().start, other, "[agent-channel]"),
        }
    }
    start
}

/// `[save]`.
pub(super) fn save(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Save {
    let mut save = Save::default();
    let Some(entries) = table(value, "[save]", found) else {
        return save;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "trim-whitespace" => {
                save.trim_whitespace = switch(value, "[save]", "trim-whitespace", found);
            }
            other => unknown(found, name.span().start, other, "[save]"),
        }
    }
    save
}

/// A setting that is `true` or `false`.
pub(super) fn switch(
    value: &Spanned<DeValue<'_>>,
    table: &str,
    key: &str,
    found: &mut Found,
) -> Option<bool> {
    if let DeValue::Boolean(on) = value.get_ref() {
        return Some(*on);
    }
    found.at(
        value.span().start,
        format_args!(
            "{table} {key} takes true or false, not {}",
            type_name(value.get_ref())
        ),
    );
    None
}
