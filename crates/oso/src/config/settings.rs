//! The file's settings: the theme, the visual aids, the agent channel at launch (ADR 0108),
//! whitespace cleanup on save (ADR 0113), migemo in isearch and jumps, and jumps' keys and
//! timeout (ADR 0116), and how hosted sessions start (ADR 0121). Indentation's are in
//! `indentation` (ADR 0115), and terminals' in `terminal` (ADR 0120).
//! Each is `None` where the file leaves it out or gives a bad value, so the caller keeps its own.
//!
//! Must not depend on: `app` or `dispatch`; applying a setting, and when, is the caller's
//! (ADR 0108's "Settings and run-time choices").

use std::time::Duration;

use oso_core::jump::Keys;
use toml::Spanned;
use toml::de::DeValue;

use super::indentation::{Indent, Modes};
use super::paths::user_path;
use super::problems::{Found, cut, quoted};
use super::{table, type_name, unknown};
use crate::sessions::width;

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
    /// `[sessions]` (ADR 0121).
    pub sessions: Sessions,
    /// `[terminal]` (ADR 0120).
    pub terminal: super::Terminal,
    /// `[spaces]` (ADR 0136).
    pub spaces: super::spaces::Spaces,
    /// `[lsp]`, and the table of language servers it leads to (ADR 0138).
    pub lsp: super::lsp::Lsp,
    /// `[git]` (ADR 0127, step 1b).
    pub git: super::git::GitSettings,
}

/// `[sessions]`: how a hosted session's `claude` starts (ADR 0121).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sessions {
    /// `claude`: its executable, by absolute path; else the login shell finds it on `PATH`.
    pub claude: Option<std::path::PathBuf>,
    /// `env`: the names of more variables a session takes from oso's environment.
    pub env: Vec<String>,
    /// `steer_worktrees`: a session oso starts gets a hook that denies Claude's own worktree
    /// tools and points it to `session-worktree` (ADR 0121, Q15). Off unless set.
    pub steer_worktrees: bool,
    /// `spinner = false`: working marks in `*Spaces*` stand still (ADR 0131, Q14). They spin
    /// unless set.
    pub no_spinner: bool,
    /// `pane_width`: the columns of `*Spaces*`' pane, from 24 to 80; else 44 (ADR 0131, Q7).
    pub pane_width: Option<usize>,
    /// `[sessions.notify]`: which states ask for the person's attention (ADR 0121, step 3).
    pub notify: crate::sessions::attention::Toggles,
    /// `agents_start_without_asking`: a session the person started may start sessions with no
    /// question, in a trusted project (ADR 0135). Off unless set; applies from oso's start.
    pub agents_start_without_asking: bool,
    /// `agents_start_limit`: how many live sessions one caller may have started without asking,
    /// 1 to [`MAX_AGENTS_START_LIMIT`]; [`AGENTS_START_LIMIT`] unless set (ADR 0135).
    pub agents_start_limit: Option<u8>,
}

impl Sessions {
    /// How many live sessions one caller may have started without asking.
    pub fn agents_start_limit(&self) -> u8 {
        self.agents_start_limit.unwrap_or(AGENTS_START_LIMIT)
    }
}

/// `[sessions] agents_start_limit` unless set (ADR 0135).
pub const AGENTS_START_LIMIT: u8 = 4;
/// The most `[sessions] agents_start_limit` takes.
pub const MAX_AGENTS_START_LIMIT: u8 = 8;

/// The most names `[sessions] env` takes.
pub(super) const MAX_ENV_NAMES: usize = 64;
/// The longest name in `[sessions] env`, and the longest `[sessions] claude`, in bytes.
pub(super) const MAX_ENV_NAME: usize = 256;
pub(super) const MAX_CLAUDE_PATH: usize = 4096;

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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Search {
    /// `migemo`: whether a search starts with migemo on.
    pub migemo: Option<bool>,
    /// `migemo-dictionary`: the file migemo takes words from, as written: an absolute path,
    /// or one starting with `~/`.
    pub migemo_dictionary: Option<String>,
}

/// The longest `[search] migemo-dictionary` path, in bytes.
pub(super) const MAX_DICTIONARY_PATH: usize = 4096;

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
            "migemo-dictionary" => {
                let label = "[search] migemo-dictionary";
                search.migemo_dictionary =
                    user_path(value, label, "a file's", MAX_DICTIONARY_PATH, found);
            }
            other => unknown(found, name.span().start, other, "[search]"),
        }
    }
    search
}

/// `[sessions]`.
pub(super) fn sessions(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Sessions {
    let mut sessions = Sessions::default();
    let Some(entries) = table(value, "[sessions]", found) else {
        return sessions;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "claude" => sessions.claude = claude_path(value, found),
            "env" => sessions.env = env_names(value, "[sessions]", found),
            "steer_worktrees" => {
                if let Some(on) = switch(value, "[sessions]", "steer_worktrees", found) {
                    sessions.steer_worktrees = on;
                }
            }
            "spinner" => {
                if let Some(on) = switch(value, "[sessions]", "spinner", found) {
                    sessions.no_spinner = !on;
                }
            }
            "pane_width" => sessions.pane_width = pane_width(value, found),
            "notify" => notify(value, &mut sessions.notify, found),
            "agents_start_without_asking" => {
                let key = "agents_start_without_asking";
                if let Some(on) = switch(value, "[sessions]", key, found) {
                    sessions.agents_start_without_asking = on;
                }
            }
            "agents_start_limit" => sessions.agents_start_limit = agents_start_limit(value, found),
            other => unknown(found, name.span().start, other, "[sessions]"),
        }
    }
    sessions
}

/// `[sessions] agents_start_limit`: a whole number from 1 to [`MAX_AGENTS_START_LIMIT`].
fn agents_start_limit(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<u8> {
    let takes =
        format!("[sessions] agents_start_limit takes a number from 1 to {MAX_AGENTS_START_LIMIT}");
    whole_in(value, found, 1..=MAX_AGENTS_START_LIMIT, &takes)
}

/// `[sessions] pane_width`: a whole number of columns within [`width::WIDTHS`].
fn pane_width(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<usize> {
    let takes = format!(
        "[sessions] pane_width takes a whole number of columns from {} to {}",
        width::WIDTHS.start(),
        width::WIDTHS.end()
    );
    whole_in(value, found, width::WIDTHS, &takes)
}

/// A whole number within `range`; any other value is refused with `takes` and what it was.
fn whole_in<T>(
    value: &Spanned<DeValue<'_>>,
    found: &mut Found,
    range: std::ops::RangeInclusive<T>,
    takes: &str,
) -> Option<T>
where
    T: TryFrom<i64> + PartialOrd,
{
    let refuse = |found: &mut Found, what: &dyn std::fmt::Display| {
        found.at(value.span().start, format_args!("{takes}, not {what}"));
        None
    };
    let DeValue::Integer(n) = value.get_ref() else {
        return refuse(found, &type_name(value.get_ref()));
    };
    let number = i64::from_str_radix(n.as_str(), n.radix())
        .ok()
        .and_then(|n| T::try_from(n).ok())
        .filter(|number| range.contains(number));
    match number {
        Some(number) => Some(number),
        None => refuse(found, &cut(n)),
    }
}

/// `[sessions.notify]`: a switch for each kind; one left out or wrong keeps its default.
fn notify(
    value: &Spanned<DeValue<'_>>,
    toggles: &mut crate::sessions::attention::Toggles,
    found: &mut Found,
) {
    const TABLE: &str = "[sessions.notify]";
    let Some(entries) = table(value, TABLE, found) else {
        return;
    };
    for (name, value) in entries {
        let key = name.get_ref().as_ref();
        let toggle = match key {
            "needs_input" => &mut toggles.needs_input,
            "done" => &mut toggles.done,
            "failed" => &mut toggles.failed,
            "ready_for_review" => &mut toggles.ready_for_review,
            other => {
                unknown(found, name.span().start, other, TABLE);
                continue;
            }
        };
        if let Some(on) = switch(value, TABLE, key, found) {
            *toggle = on;
        }
    }
}

/// `[sessions] claude`: an absolute path on this computer, of at most [`MAX_CLAUDE_PATH`]
/// bytes. Nothing is looked at on disk here; the session's start thread checks the file.
fn claude_path(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<std::path::PathBuf> {
    let at = value.span().start;
    let DeValue::String(path) = value.get_ref() else {
        found.at(
            at,
            format_args!(
                "[sessions] claude takes the absolute path of claude, not {}",
                type_name(value.get_ref())
            ),
        );
        return None;
    };
    let elsewhere = path.starts_with("\\\\") || path.starts_with("//");
    let why = if !std::path::Path::new(path.as_ref()).is_absolute() || elsewhere {
        "takes an absolute path on this computer".to_owned()
    } else if path.len() > MAX_CLAUDE_PATH {
        format!("takes a path of at most {MAX_CLAUDE_PATH} bytes")
    } else if path.contains('\0') {
        "takes a path with no NUL character".to_owned()
    } else {
        return Some(std::path::PathBuf::from(path.as_ref()));
    };
    found.at(at, format_args!("[sessions] claude {why}"));
    None
}

/// `env` of `table`, as `[sessions] env`: at most [`MAX_ENV_NAMES`] variables' names, each
/// one oso may pass on: not empty, with no `=` or NUL, and not `PATH`, `LD_*` or `DYLD_*`.
/// A name refused is said
/// and left out; the others stay.
pub(super) fn env_names(
    value: &Spanned<DeValue<'_>>,
    table: &str,
    found: &mut Found,
) -> Vec<String> {
    let DeValue::Array(names) = value.get_ref() else {
        found.at(
            value.span().start,
            format_args!(
                "{table} env takes a list of variables' names, such as [\"HTTPS_PROXY\"], not {}",
                type_name(value.get_ref())
            ),
        );
        return Vec::new();
    };
    if names.len() > MAX_ENV_NAMES {
        found.at(
            value.span().start,
            format_args!("{table} env takes at most {MAX_ENV_NAMES} names"),
        );
        return Vec::new();
    }
    let mut kept = Vec::new();
    for name in names {
        let DeValue::String(text) = name.get_ref() else {
            found.at(
                name.span().start,
                format_args!("{table} env takes names, not {}", type_name(name.get_ref())),
            );
            continue;
        };
        let upper = text.to_ascii_uppercase();
        let refused = text.is_empty()
            || text.len() > MAX_ENV_NAME
            || text.contains(['=', '\0'])
            || upper == "PATH"
            || upper.starts_with("LD_")
            || upper.starts_with("DYLD_");
        if refused {
            found.at(
                name.span().start,
                format_args!(
                    "{table} env cannot pass {}: oso sets PATH itself, and never passes the loader's variables",
                    quoted(&cut(text.as_ref()))
                ),
            );
            continue;
        }
        if !kept.iter().any(|kept: &String| kept == text.as_ref()) {
            kept.push(text.as_ref().to_owned());
        }
    }
    kept
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
