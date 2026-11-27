//! `[spaces]`: where the directories of issue spaces go (ADR 0136, Q1), how a new space is
//! named (ADR 0159), and what a space's orchestrator may answer for its native workers
//! (ADR 0186).
//!
//! Must not depend on: `app` or the file system; the app makes and checks the directories.

use toml::Spanned;
use toml::de::DeValue;

use super::paths::user_path;
use super::problems::{Found, cut, quoted};
use super::{table, type_name, unknown};
use crate::sessions::native::stand_in::Rules;

/// The longest `[spaces] root`, in bytes.
pub(super) const MAX_ROOT: usize = 1024;

/// The most tools `orchestrator-answers` lists, and patterns `orchestrator-answers-deny` does.
pub(super) const MAX_ANSWER_TOOLS: usize = 64;
pub(super) const MAX_DENY_PATTERNS: usize = 64;
/// The longest tool name, and pattern, in bytes.
pub(super) const MAX_TOOL_NAME: usize = 64;
pub(super) const MAX_DENY_PATTERN: usize = 1024;

/// What a space's directory goes under when `[spaces] root` says nothing.
pub const DEFAULT_ROOT: &str = "~/spaces";

/// How a new space is named (ADR 0159).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpacesNaming {
    /// The person's `claude` summarizes the description, oso's own names standing in where
    /// it gives none.
    #[default]
    Claude,
    /// oso's own names: the description's first ASCII words, else the repository's name.
    Local,
}

/// `[spaces]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spaces {
    /// `root`: the directory each new space's own directory is made in, as written: an
    /// absolute path, or one starting with `~/`. Else [`DEFAULT_ROOT`].
    pub root: Option<String>,
    /// `naming`: `"claude"`, the default, or `"local"`.
    pub naming: SpacesNaming,
    /// `orchestrator-answers`: the tools a space's orchestrator may allow its native workers'
    /// calls of (ADR 0186). None unless set, which lets it allow nothing.
    pub answers: Vec<String>,
    /// `orchestrator-answers-deny`: the Bash commands it never allows besides the defaults, as
    /// patterns, as written; the defaults are never taken away.
    pub answers_deny: Vec<String>,
    /// Either key was given a value oso does not take: no orchestrator answers until it is
    /// fixed, as the person's list may have meant to deny more.
    pub answers_refused: bool,
}

impl Spaces {
    /// The root in force, as written.
    pub fn root(&self) -> &str {
        self.root.as_deref().unwrap_or(DEFAULT_ROOT)
    }

    /// What a space's orchestrator may answer: nothing where a key was refused.
    pub fn answer_rules(&self) -> Rules {
        let tools = if self.answers_refused {
            &[][..]
        } else {
            &self.answers[..]
        };
        Rules::new(tools, &self.answers_deny).unwrap_or_default()
    }
}

/// `[spaces]`.
pub(super) fn spaces(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Spaces {
    let mut spaces = Spaces::default();
    let Some(entries) = table(value, "[spaces]", found) else {
        return spaces;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "root" => {
                let label = "[spaces] root";
                let root = user_path(value, label, "a directory's", MAX_ROOT, found);
                // A `..` would be refused when the directory is made; it is said here instead.
                let up = |root: &String| {
                    std::path::Path::new(root)
                        .components()
                        .any(|part| part == std::path::Component::ParentDir)
                };
                spaces.root = match root {
                    Some(root) if up(&root) => {
                        let at = value.span().start;
                        found.at(at, format_args!("{label} takes a path with no `..`"));
                        None
                    }
                    root => root,
                };
            }
            "naming" => match value.get_ref() {
                DeValue::String(text) if text == "claude" => spaces.naming = SpacesNaming::Claude,
                DeValue::String(text) if text == "local" => spaces.naming = SpacesNaming::Local,
                other => {
                    let what = match other {
                        DeValue::String(text) => quoted(cut(text.as_ref())).to_string(),
                        other => type_name(other).to_owned(),
                    };
                    found.at(
                        value.span().start,
                        format_args!("[spaces] naming takes \"claude\" or \"local\", not {what}"),
                    );
                    // Written to keep descriptions from a model, most likely: fail closed.
                    spaces.naming = SpacesNaming::Local;
                }
            },
            "orchestrator-answers" => match answer_tools(value, found) {
                Some(tools) => spaces.answers = tools,
                None => spaces.answers_refused = true,
            },
            "orchestrator-answers-deny" => match deny_patterns(value, found) {
                Some(patterns) => spaces.answers_deny = patterns,
                None => spaces.answers_refused = true,
            },
            other => unknown(found, name.span().start, other, "[spaces]"),
        }
    }
    spaces
}

/// `[spaces] orchestrator-answers`: tools' names, never an MCP tool's; `None`, said, for any
/// other value.
fn answer_tools(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<Vec<String>> {
    let label = "[spaces] orchestrator-answers";
    let names = strings(
        value,
        label,
        "tools' names, such as [\"Read\", \"Edit\"]",
        MAX_ANSWER_TOOLS,
        found,
    )?;
    let mut tools = Vec::with_capacity(names.len());
    for (name, at) in names {
        let plain = !name.is_empty()
            && name.len() <= MAX_TOOL_NAME
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        let said = if !plain {
            "takes tools' names of ASCII letters, digits, _ and -"
        } else if name.starts_with("mcp__") {
            "takes no MCP tool: only the person answers those"
        } else {
            if !tools.contains(&name) {
                tools.push(name);
            }
            continue;
        };
        found.at(
            at,
            format_args!("{label} {said}, not {}", quoted(cut(&name))),
        );
        return None;
    }
    Some(tools)
}

/// `[spaces] orchestrator-answers-deny`: patterns of the `regex` crate; `None`, said, for any
/// that does not compile, or any other value.
fn deny_patterns(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<Vec<String>> {
    let label = "[spaces] orchestrator-answers-deny";
    let patterns = strings(
        value,
        label,
        "patterns, such as ['\\bmake\\b']",
        MAX_DENY_PATTERNS,
        found,
    )?;
    let mut kept = Vec::with_capacity(patterns.len());
    for (pattern, at) in patterns {
        let said = if pattern.is_empty() || pattern.len() > MAX_DENY_PATTERN {
            format!("takes patterns of 1 to {MAX_DENY_PATTERN} bytes")
        } else if let Err(e) = Rules::check(&pattern) {
            let e = e.to_string();
            let first = e.lines().last().unwrap_or_default().trim().to_owned();
            format!("cannot read {}: {}", quoted(cut(&pattern)), cut(&first))
        } else {
            kept.push(pattern);
            continue;
        };
        found.at(
            at,
            format_args!("{label} {said}; no orchestrator answers until it is fixed"),
        );
        return None;
    }
    Some(kept)
}

/// A list of at most `most` strings for `label`, each with where it is; `None`, said, for any
/// other value.
fn strings(
    value: &Spanned<DeValue<'_>>,
    label: &str,
    takes: &str,
    most: usize,
    found: &mut Found,
) -> Option<Vec<(String, usize)>> {
    let DeValue::Array(items) = value.get_ref() else {
        let what = type_name(value.get_ref());
        found.at(
            value.span().start,
            format_args!("{label} takes a list of {takes}, not {what}"),
        );
        return None;
    };
    if items.len() > most {
        found.at(
            value.span().start,
            format_args!("{label} takes at most {most}"),
        );
        return None;
    }
    let mut kept = Vec::with_capacity(items.len());
    for item in items {
        let DeValue::String(text) = item.get_ref() else {
            let what = type_name(item.get_ref());
            found.at(
                item.span().start,
                format_args!("{label} takes a list of {takes}, not {what}"),
            );
            return None;
        };
        kept.push((text.as_ref().to_owned(), item.span().start));
    }
    Some(kept)
}
