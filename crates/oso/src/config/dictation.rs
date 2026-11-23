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
}

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
