//! `[dictation]`: how oso dictates through TypeWhisper (ADR 0175). Only the person's own
//! configuration says this; nothing is looked at on disk here.
//!
//! Must not depend on: `app` or the file system.

use std::path::PathBuf;
use std::time::Duration;

use toml::Spanned;
use toml::de::DeValue;

use super::paths::{program_path, user_path};
use super::problems::{Found, quoted};
use super::{table, type_name, unknown, whole_in};
use crate::dictation::api::is_uuid;
use crate::dictation::fix::Confidence;

/// The longest path `[dictation]` takes, in bytes.
const MAX_PATH: usize = 4096;

/// How long a dictation records at most unless `max_seconds` says.
pub const MAX_SECONDS: u32 = 120;

/// What `max_seconds` takes.
const SECONDS: std::ops::RangeInclusive<u32> = 5..=1800;

/// `[dictation]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dictation {
    /// `discovery`: TypeWhisper's discovery file, as written: absolute, or `~/` and a relative
    /// path. Unless set, the place TypeWhisper's release build keeps it.
    pub discovery: Option<String>,
    /// `curl`: the curl to run, by absolute path; `/usr/bin/curl` unless set.
    pub curl: Option<PathBuf>,
    /// `workflow`: the id of the TypeWhisper workflow to dictate in, a UUID.
    pub workflow: Option<String>,
    /// `max_seconds`: the longest a dictation records, from 5 to 1800 seconds.
    pub max_seconds: Option<u32>,
    /// `fix_model`: the model `claude -p` fixes a dictation with (step 2); `sonnet` unless set.
    pub fix_model: Option<String>,
    /// `fix_effort`: its effort, `low` unless set.
    pub fix_effort: Option<String>,
    /// `fix_context_lines`: the lines on each side of the text the fix sees, from 0 to 20; 3
    /// unless set.
    pub fix_context_lines: Option<u32>,
    /// `fix_min_confidence`: the least confidence a fix is applied with; the rest are only
    /// shown. `medium` unless set.
    pub fix_min_confidence: Option<Confidence>,
}

/// The model a fix runs with unless `fix_model` says.
pub const FIX_MODEL: &str = "sonnet";
/// The effort a fix runs with unless `fix_effort` says.
pub const FIX_EFFORT: &str = "low";
/// The lines on each side a fix sees unless `fix_context_lines` says.
pub const FIX_CONTEXT_LINES: u32 = 3;
/// What `fix_context_lines` takes.
const CONTEXT_LINES: std::ops::RangeInclusive<u32> = 0..=20;
/// The efforts `fix_effort` takes.
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// The longest model name `fix_model` takes.
const MAX_MODEL: usize = 64;

impl Dictation {
    /// The curl run: `curl`, else `/usr/bin/curl`.
    pub fn curl(&self) -> PathBuf {
        self.curl
            .clone()
            .unwrap_or_else(|| PathBuf::from("/usr/bin/curl"))
    }

    /// The longest a dictation records.
    pub fn max_recording(&self) -> Duration {
        Duration::from_secs(u64::from(self.max_seconds.unwrap_or(MAX_SECONDS)))
    }

    /// The model a fix runs with.
    pub fn fix_model(&self) -> &str {
        self.fix_model.as_deref().unwrap_or(FIX_MODEL)
    }

    /// The effort a fix runs with.
    pub fn fix_effort(&self) -> &str {
        self.fix_effort.as_deref().unwrap_or(FIX_EFFORT)
    }

    /// The lines on each side of the text a fix sees.
    pub fn fix_context_lines(&self) -> usize {
        self.fix_context_lines.unwrap_or(FIX_CONTEXT_LINES) as usize
    }

    /// The least confidence a fix is applied with.
    pub fn fix_min_confidence(&self) -> Confidence {
        self.fix_min_confidence.unwrap_or_default()
    }
}

/// `[dictation]`.
pub(super) fn dictation(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Dictation {
    const TABLE: &str = "[dictation]";
    let mut dictation = Dictation::default();
    let Some(entries) = table(value, TABLE, found) else {
        return dictation;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "discovery" => {
                let label = "[dictation] discovery";
                dictation.discovery = user_path(value, label, "a file's", MAX_PATH, found);
            }
            "curl" => dictation.curl = curl(value, found),
            "workflow" => dictation.workflow = workflow(value, found),
            "max_seconds" => {
                let takes = format!(
                    "[dictation] max_seconds takes a whole number of seconds from {} to {}",
                    SECONDS.start(),
                    SECONDS.end()
                );
                dictation.max_seconds = whole_in(value, found, SECONDS, &takes);
            }
            "fix_model" => dictation.fix_model = fix_model(value, found),
            "fix_effort" => dictation.fix_effort = fix_effort(value, found),
            "fix_context_lines" => {
                let takes = format!(
                    "[dictation] fix_context_lines takes a whole number of lines from {} to {}",
                    CONTEXT_LINES.start(),
                    CONTEXT_LINES.end()
                );
                dictation.fix_context_lines = whole_in(value, found, CONTEXT_LINES, &takes);
            }
            "fix_min_confidence" => dictation.fix_min_confidence = min_confidence(value, found),
            other => unknown(found, name.span().start, other, TABLE),
        }
    }
    dictation
}

/// `[dictation] curl`: an absolute path on this computer. The dictation's thread checks the
/// file.
fn curl(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<PathBuf> {
    program_path(value, "[dictation] curl", "curl", MAX_PATH, found)
}

/// `[dictation] workflow`: a UUID, as TypeWhisper names its workflows.
fn workflow(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<String> {
    let at = value.span().start;
    match value.get_ref() {
        DeValue::String(id) if is_uuid(id) => Some(id.as_ref().to_owned()),
        DeValue::String(id) => {
            found.at(
                at,
                format_args!(
                    "[dictation] workflow takes a workflow's id, a UUID, not {}",
                    quoted(id)
                ),
            );
            None
        }
        other => {
            let kind = type_name(other);
            found.at(
                at,
                format_args!("[dictation] workflow takes a workflow's id, a UUID, not {kind}"),
            );
            None
        }
    }
}

/// A string `value` that `holds`, else what `takes` says and what it was.
fn string_that(
    value: &Spanned<DeValue<'_>>,
    found: &mut Found,
    takes: &str,
    holds: impl Fn(&str) -> bool,
) -> Option<String> {
    let at = value.span().start;
    match value.get_ref() {
        DeValue::String(text) if holds(text) => Some(text.as_ref().to_owned()),
        DeValue::String(text) => {
            found.at(at, format_args!("{takes}, not {}", quoted(text)));
            None
        }
        other => {
            found.at(at, format_args!("{takes}, not {}", type_name(other)));
            None
        }
    }
}

/// `[dictation] fix_model`: a model's name or alias, such as `sonnet` or `claude-sonnet-5-5`.
/// It goes to `claude` as one `--model=` argument, so only a name's characters are taken.
fn fix_model(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<String> {
    let takes = "[dictation] fix_model takes a model's name, such as \"sonnet\"";
    string_that(value, found, takes, |name| {
        !name.is_empty()
            && name.len() <= MAX_MODEL
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'[' | b']'))
    })
}

/// `[dictation] fix_effort`: one of `claude`'s efforts.
fn fix_effort(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<String> {
    let takes = "[dictation] fix_effort takes low, medium, high, xhigh or max";
    string_that(value, found, takes, |name| EFFORTS.contains(&name))
}

/// `[dictation] fix_min_confidence`: `low`, `medium` or `high`.
fn min_confidence(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<Confidence> {
    let takes = "[dictation] fix_min_confidence takes low, medium or high";
    string_that(value, found, takes, |name| Confidence::of(name).is_some())
        .and_then(|name| Confidence::of(&name))
}
