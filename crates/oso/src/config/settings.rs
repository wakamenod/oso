//! The file's settings: the theme, the visual aids, the agent channel at launch (ADR 0108),
//! whitespace cleanup on save (ADR 0113), migemo in isearch and jumps, and jumps' keys and
//! timeout (ADR 0116). Indentation's are in `indentation` (ADR 0115).
//! Each is `None` where the file leaves it out or gives a bad value, so the caller keeps its own.
//!
//! Must not depend on: `app` or `dispatch`; applying a setting, and when, is the caller's
//! (ADR 0108's "Settings and run-time choices").

use std::time::Duration;

use oso_core::jump::Keys;
use toml::Spanned;
use toml::de::DeValue;

use super::indentation::{Indent, Modes};
use super::problems::{Found, cut, quoted};
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
    /// `[search]` (ADR 0116).
    pub search: Search,
    /// `[jump]`.
    pub jump: Jump,
    /// `[indent]` (ADR 0115).
    pub indent: Indent,
    /// `[modes.<mode>]`'s indentation (ADR 0115).
    pub modes: Modes,
}

/// `[jump]`: how jumps label and read (ADR 0116).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Jump {
    /// `keys`: the keys labels are typed with.
    pub keys: Option<Keys>,
    /// `timeout`: how long a jump reads its string after the last key.
    pub timeout: Option<Duration>,
}

/// `[save]`: what a save does besides writing (ADR 0113).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Save {
    /// `trim-whitespace`: clean the lines edited since the file was read or saved.
    pub trim_whitespace: Option<bool>,
}

/// `[search]`: how isearch and jumps search (ADR 0116).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Search {
    /// `migemo`: whether a search starts with migemo on.
    pub migemo: Option<bool>,
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

/// `[search]`.
pub(super) fn search(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Search {
    let mut search = Search::default();
    let Some(entries) = table(value, "[search]", found) else {
        return search;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "migemo" => search.migemo = switch(value, "[search]", "migemo", found),
            other => unknown(found, name.span().start, other, "[search]"),
        }
    }
    search
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

/// `[jump]`.
pub(super) fn jump(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Jump {
    let mut jump = Jump::default();
    let Some(entries) = table(value, "[jump]", found) else {
        return jump;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "keys" => jump.keys = jump_keys(value, found),
            "timeout" => jump.timeout = timeout(value, found),
            other => unknown(found, name.span().start, other, "[jump]"),
        }
    }
    jump
}

/// `[jump] keys`: 2 to 32 distinct printable ASCII characters, none of them an action's key.
fn jump_keys(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<Keys> {
    let at = value.span().start;
    let DeValue::String(keys) = value.get_ref() else {
        found.at(
            at,
            format_args!(
                "[jump] keys takes a string of keys, not {}",
                type_name(value.get_ref())
            ),
        );
        return None;
    };
    Keys::new(keys)
        .map_err(|error| found.at(at, format_args!("[jump] keys {}", cut(error))))
        .ok()
}

/// The shortest and longest `[jump] timeout`, in seconds (ADR 0116).
const TIMEOUT_SECONDS: std::ops::RangeInclusive<f64> = 0.1..=5.0;

/// `[jump] timeout`: seconds, a whole number or not, from 0.1 to 5.
fn timeout(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<Duration> {
    let refuse = |found: &mut Found, what: &dyn std::fmt::Display| {
        found.at(
            value.span().start,
            format_args!("[jump] timeout takes a number of seconds from 0.1 to 5, not {what}"),
        );
        None
    };
    let (seconds, spelled): (f64, &dyn std::fmt::Display) = match value.get_ref() {
        // One too large for an `i32` is out of range all the same.
        DeValue::Integer(n) => (
            i64::from_str_radix(n.as_str(), n.radix())
                .ok()
                .and_then(|n| i32::try_from(n).ok())
                .map_or(f64::INFINITY, f64::from),
            n,
        ),
        DeValue::Float(n) => (n.as_str().parse().unwrap_or(f64::NAN), n),
        other => return refuse(found, &type_name(other)),
    };
    if !TIMEOUT_SECONDS.contains(&seconds) {
        // The file's own spelling of the number, cut.
        return refuse(found, &cut(spelled));
    }
    Some(Duration::from_secs_f64(seconds))
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
