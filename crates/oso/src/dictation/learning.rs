//! What dictation learns (ADR 0175, step 3): a record of the pairs, a misheard `wrong` and its
//! `right`, that fixes by Claude and the person's own rewrites of dictated text taught, and
//! the history each fix is then given. The host reads and writes the files, off the UI
//! thread, and watches what was dictated.
//!
//! Must not depend on: `app`, `sessions` or the editor; this sees strings and days alone.
//!
//! - **The record** keeps at most [`MAX_PAIRS`] pairs, each with how often it stood as a fix,
//!   as the person's rewrite of text a fix saw and of text typed plain, how often it was undone, the days it stood on (the latest
//!   [`MAX_DAYS`]) and the day it was last seen. One not seen for [`UNUSED_DAYS`] goes, and
//!   when the record is full the one seen longest ago makes room.
//! - **The history** a fix is given is only of pairs whose `wrong` is in the text: those that
//!   stood more often than they were undone, the most used first, then the latest, within
//!   [`MAX_HISTORY`] pairs and [`MAX_HISTORY_BYTES`]; and, as `rejected`, those undone at least
//!   as often as they stood. A pair whose `right` looks like a credential is never kept.
//! - **What a watch saw** becomes observations: the text as oso left it, the text as it was
//!   typed, the fixes made and the text now (`rewrites` finds the person's own).
//! - **TypeWhisper's dictionary** gets the pairs that stood often enough (`dictionary`, step 4).
//! - **The log** is JSON lines, the last [`MAX_LOG_LINES`] of the last [`UNUSED_DAYS`]: each
//!   observation, each history sent, as counts, and each change in TypeWhisper's dictionary.
//!   Learning is silent: only `dictation-learning-log` shows it (`view`).

use serde_json::{Value, json};

use super::dictionary::sync::Done;
use super::fix::{self, MAX_FIX_CHARS};

pub mod rewrites;
pub mod view;

/// The directory in oso's state directory the record and the log are in.
pub const DIRECTORY: &str = "dictation";
/// The record's file in [`DIRECTORY`].
pub const FILE: &str = "learning.json";
/// Where a write of the record goes first.
pub const PART: &str = "learning.json.part";
/// The log's file in [`DIRECTORY`].
pub const LOG: &str = "learning.log";
/// Where a write of the log goes first.
pub const LOG_PART: &str = "learning.log.part";
/// The most bytes of the record or the log read.
pub const MAX_BYTES: u64 = 256 * 1024;
/// The most pairs the record keeps.
pub const MAX_PAIRS: usize = 40;
/// How many days a pair is kept with nothing seen of it.
pub const UNUSED_DAYS: u32 = 30;
/// The most days a pair keeps of those it stood on.
pub const MAX_DAYS: usize = 8;
/// The most pairs of history a fix is given.
pub const MAX_HISTORY: usize = 50;
/// The most bytes of the pairs a fix is given, history and rejected together.
pub const MAX_HISTORY_BYTES: usize = 4 * 1024;
/// The most lines the log keeps.
pub const MAX_LOG_LINES: usize = 500;
/// The record's version.
const VERSION: u64 = 1;

/// Where a pair came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A fix by Claude.
    Fix,
    /// The person's own rewrite of dictated text that a fix saw.
    Edit,
    /// The person's own rewrite of text typed plain, which no fix saw: kept, never sent.
    Plain,
}

/// What became of a pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It still stood when the watch ended.
    Stood,
    /// The person took it back.
    Undone,
}

/// One thing a watch saw: `wrong` became `right`, or was put back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub wrong: String,
    pub right: String,
    pub source: Source,
    pub outcome: Outcome,
}

/// A pair the record keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pair {
    pub wrong: String,
    pub right: String,
    /// How often it stood as a fix.
    pub fixes: u32,
    /// How often it stood as the person's rewrite of text a fix saw.
    pub edits: u32,
    /// How often it stood as the person's rewrite of text typed plain.
    pub plain: u32,
    /// How often it was undone.
    pub undone: u32,
    /// The days it stood on, ascending, the latest [`MAX_DAYS`].
    pub days: Vec<u32>,
    /// The day it was last seen.
    pub last: u32,
}

impl Pair {
    /// How often it stood, of any source.
    pub fn stood(&self) -> u32 {
        self.sent_stood().saturating_add(self.plain)
    }

    /// How often it stood in text a fix saw: a pair learned only from text typed plain, which
    /// the person kept from Claude, is never sent (the author's choice on 2026-11-23).
    pub fn sent_stood(&self) -> u32 {
        self.fixes.saturating_add(self.edits)
    }
}

/// The record: what dictation has learned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub pairs: Vec<Pair>,
}

/// What a fix is given of the record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    /// Pairs that stood, each with how often.
    pub accepted: Vec<(String, String, u32)>,
    /// Pairs the person undid, for the fix not to make.
    pub rejected: Vec<(String, String)>,
}

/// Whether `text` can be one side of a pair: at most [`MAX_FIX_CHARS`] characters, shown as
/// text, and, for a `wrong`, something.
fn holds(text: &str, wrong: bool) -> bool {
    (!wrong || !text.trim().is_empty())
        && text.chars().count() <= MAX_FIX_CHARS
        && text.chars().all(fix::shown_as_text)
        && !looks_like_a_credential(text)
}

/// The starts of tokens that are credentials whatever follows them.
const CREDENTIAL_STARTS: &[&str] = &[
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "sk-",
    "xoxa-",
    "xoxb-",
    "xoxp-",
    "xoxs-",
    "-----BEGIN",
];

/// How long a run of a token's characters holding both letters and digits is taken for a
/// credential: longer than any word a person says.
const CREDENTIAL_RUN: usize = 16;

/// Whether `text` looks like a credential pasted over a word: a run of ASCII letters, digits
/// and `_-+/=.` in it starts as a known token does, or is [`CREDENTIAL_RUN`] or more long with
/// both letters and digits. Such a pair would put the credential in later fixes' history.
pub fn looks_like_a_credential(text: &str) -> bool {
    let token = |c: char| c.is_ascii_alphanumeric() || "_-+/=.".contains(c);
    text.split(|c: char| !token(c)).any(|run| {
        CREDENTIAL_STARTS.iter().any(|start| run.starts_with(start))
            || (run.len() >= CREDENTIAL_RUN
                && run.bytes().any(|b| b.is_ascii_alphabetic())
                && run.bytes().any(|b| b.is_ascii_digit()))
    })
}

impl Record {
    /// The record in `bytes`, as far as it holds, with what is past `today`'s age gone: a pair
    /// that does not hold is left out, and so is a second of the same pair. Nothing from an
    /// answer of another shape.
    pub fn parse(bytes: &[u8], today: u32) -> Self {
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            return Self::default();
        };
        if value.get("version").and_then(Value::as_u64) != Some(VERSION) {
            return Self::default();
        }
        let mut record = Self::default();
        let listed = value.get("pairs").and_then(Value::as_array);
        // A record oso wrote holds at most [`MAX_PAIRS`]: past a few times that, it is not
        // oso's, and the rest is left out, so that reading one costs little.
        let listed = listed.into_iter().flatten().take(MAX_PAIRS * 4);
        for pair in listed.filter_map(|pair| pair_of(pair, today)) {
            if record.find(&pair.wrong, &pair.right).is_none() {
                record.pairs.push(pair);
            }
        }
        record.age(today);
        record
    }

    /// The record as its file holds it.
    pub fn to_text(&self) -> String {
        let pairs: Vec<Value> = self
            .pairs
            .iter()
            .map(|pair| {
                json!({
                    "wrong": pair.wrong,
                    "right": pair.right,
                    "fixes": pair.fixes,
                    "edits": pair.edits,
                    "plain": pair.plain,
                    "undone": pair.undone,
                    "days": pair.days,
                    "last": pair.last,
                })
            })
            .collect();
        let mut text = json!({ "version": VERSION, "pairs": pairs }).to_string();
        text.push('\n');
        text
    }

    /// Where the pair of `wrong` and `right` is, if it is there.
    pub fn find(&self, wrong: &str, right: &str) -> Option<usize> {
        self.pairs
            .iter()
            .position(|pair| pair.wrong == wrong && pair.right == right)
    }

    /// Takes in what a watch saw on `today`: each pair is counted, made if new, and seen
    /// today. Then what is past its age goes, and the record keeps [`MAX_PAIRS`], those seen
    /// longest ago going first.
    pub fn observe(&mut self, seen: &[Observation], today: u32) {
        for observation in seen {
            if observation.wrong == observation.right
                || !holds(&observation.wrong, true)
                || !holds(&observation.right, false)
            {
                continue;
            }
            let at = self
                .find(&observation.wrong, &observation.right)
                .unwrap_or_else(|| {
                    self.pairs.push(Pair {
                        wrong: observation.wrong.clone(),
                        right: observation.right.clone(),
                        fixes: 0,
                        edits: 0,
                        plain: 0,
                        undone: 0,
                        days: Vec::new(),
                        last: today,
                    });
                    self.pairs.len() - 1
                });
            let pair = &mut self.pairs[at];
            pair.last = pair.last.max(today);
            match (observation.outcome, observation.source) {
                (Outcome::Undone, _) => pair.undone = pair.undone.saturating_add(1),
                (Outcome::Stood, source) => {
                    match source {
                        Source::Fix => pair.fixes = pair.fixes.saturating_add(1),
                        Source::Edit => pair.edits = pair.edits.saturating_add(1),
                        Source::Plain => pair.plain = pair.plain.saturating_add(1),
                    }
                    if !pair.days.contains(&today) {
                        pair.days.push(today);
                        pair.days.sort_unstable();
                    }
                    let extra = pair.days.len().saturating_sub(MAX_DAYS);
                    pair.days.drain(..extra);
                }
            }
        }
        self.age(today);
    }

    /// Drops the pairs not seen for [`UNUSED_DAYS`] by `today`, then those seen longest ago,
    /// and of those the least used, past [`MAX_PAIRS`].
    pub fn age(&mut self, today: u32) {
        self.pairs
            .retain(|pair| today.saturating_sub(pair.last) < UNUSED_DAYS);
        if self.pairs.len() <= MAX_PAIRS {
            return;
        }
        // Kept: the latest seen, then the most used, then the first added, in their order.
        let mut order: Vec<usize> = (0..self.pairs.len()).collect();
        order.sort_by_key(|&at| {
            let pair = &self.pairs[at];
            std::cmp::Reverse((pair.last, pair.stood(), std::cmp::Reverse(at)))
        });
        let mut keep = vec![false; self.pairs.len()];
        for &at in &order[..MAX_PAIRS] {
            keep[at] = true;
        }
        let mut kept = keep.into_iter();
        self.pairs.retain(|_| kept.next().unwrap_or(false));
    }

    /// What a fix of `target` is given: see the module's comment.
    pub fn history(&self, target: &str) -> History {
        let mut ranked: Vec<&Pair> = (self.pairs.iter())
            .filter(|pair| target.contains(pair.wrong.as_str()))
            .collect();
        ranked.sort_by_key(|pair| {
            (
                std::cmp::Reverse(pair.sent_stood()),
                std::cmp::Reverse(pair.last),
            )
        });
        let mut history = History::default();
        let mut bytes = 0;
        for pair in ranked {
            let size = pair.wrong.len() + pair.right.len();
            if bytes + size > MAX_HISTORY_BYTES {
                continue;
            }
            if pair.sent_stood() > pair.undone {
                if history.accepted.len() == MAX_HISTORY {
                    continue;
                }
                history
                    .accepted
                    .push((pair.wrong.clone(), pair.right.clone(), pair.sent_stood()));
            } else if pair.undone > 0 {
                history
                    .rejected
                    .push((pair.wrong.clone(), pair.right.clone()));
            } else {
                continue;
            }
            bytes += size;
        }
        history
    }
}

/// `value` as a pair of the record, if it holds as one, with no day after `today`, as a clock
/// set back or a record not oso's could give, which would keep it from ever ageing.
fn pair_of(value: &Value, today: u32) -> Option<Pair> {
    let text = |key: &str| value.get(key).and_then(Value::as_str);
    let count = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_u64)
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    let day = |value: &Value| {
        value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(|day| day.min(today))
    };
    let (wrong, right) = (text("wrong")?, text("right")?);
    if wrong == right || !holds(wrong, true) || !holds(right, false) {
        return None;
    }
    let mut days: Vec<u32> = value
        .get("days")
        .and_then(Value::as_array)?
        .iter()
        .take(MAX_DAYS * 4)
        .filter_map(day)
        .collect();
    days.sort_unstable();
    days.dedup();
    let extra = days.len().saturating_sub(MAX_DAYS);
    days.drain(..extra);
    Some(Pair {
        wrong: wrong.to_owned(),
        right: right.to_owned(),
        fixes: count("fixes")?,
        edits: count("edits")?,
        plain: count("plain")?,
        undone: count("undone")?,
        days,
        last: value.get("last").and_then(day)?,
    })
}

/// What a watch of one dictation saw when it ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Watched {
    /// The text as it was typed.
    pub typed: String,
    /// The text as oso left it: fixed, or as typed.
    pub left: String,
    /// The fixes made, each `wrong` and `right`.
    pub fixed: Vec<(String, String)>,
    /// The text when the watch ended.
    pub current: String,
    /// A fix saw the text: the person's rewrites of it may be sent with later fixes.
    pub sent: bool,
}

/// What `watched` teaches: each fix stood if the text is still as oso left it, or still holds
/// its `right`, and was undone if the text is back as it was typed, or the person put its
/// `wrong` back; and the person's own small rewrites of what oso left stood as edits. Each
/// pair once.
pub fn observations(watched: &Watched) -> Vec<Observation> {
    let mut seen: Vec<Observation> = Vec::new();
    let mut note = |wrong: &str, right: &str, source, outcome| {
        let new = seen
            .iter()
            .all(|other| other.wrong != wrong || other.right != right);
        if new {
            seen.push(Observation {
                wrong: wrong.to_owned(),
                right: right.to_owned(),
                source,
                outcome,
            });
        }
    };
    if watched.current == watched.left {
        for (wrong, right) in &watched.fixed {
            note(wrong, right, Source::Fix, Outcome::Stood);
        }
        return seen;
    }
    if !watched.fixed.is_empty() && watched.current == watched.typed {
        for (wrong, right) in &watched.fixed {
            note(wrong, right, Source::Fix, Outcome::Undone);
        }
        return seen;
    }
    let mut rewritten = rewrites::rewrites(&watched.left, &watched.current);
    for (wrong, right) in &watched.fixed {
        let back = rewritten
            .iter()
            .position(|(from, to)| from == right && to == wrong);
        if let Some(back) = back {
            rewritten.remove(back);
            note(wrong, right, Source::Fix, Outcome::Undone);
        } else if watched.current.contains(right.as_str()) {
            note(wrong, right, Source::Fix, Outcome::Stood);
        }
    }
    for (wrong, right) in &rewritten {
        let source = if watched.sent {
            Source::Edit
        } else {
            Source::Plain
        };
        note(wrong, right, source, Outcome::Stood);
    }
    seen
}

/// The log's line for `observation`, at `at` seconds since the epoch.
pub fn log_observed(observation: &Observation, at: u64) -> String {
    json!({
        "at": at,
        "event": "observed",
        "source": match observation.source {
            Source::Fix => "fix",
            Source::Edit => "edit",
            Source::Plain => "plain",
        },
        "outcome": match observation.outcome {
            Outcome::Stood => "stood",
            Outcome::Undone => "undone",
        },
        "wrong": observation.wrong,
        "right": observation.right,
    })
    .to_string()
}

/// The log's line for a history sent with a fix, as counts, at `at`.
pub fn log_history(history: &History, at: u64) -> String {
    json!({
        "at": at,
        "event": "history",
        "accepted": history.accepted.len(),
        "rejected": history.rejected.len(),
    })
    .to_string()
}

/// The log's line for what a pass over TypeWhisper's dictionary did, at `at`.
pub fn log_done(done: &Done, at: u64) -> String {
    json!({
        "at": at,
        "event": done.what.name(),
        "as": done.kind.name(),
        "wrong": done.wrong,
        "right": done.right,
        "why": done.why,
    })
    .to_string()
}

/// The log `old` with `lines` after it, its last [`MAX_LOG_LINES`] lines within
/// [`MAX_BYTES`], none from more than [`UNUSED_DAYS`] before `at`, seconds since the epoch. A
/// line of the old log that is not whole, or says no time, is dropped.
pub fn append_log(old: &str, lines: &[String], at: u64) -> String {
    let oldest = at.saturating_sub(u64::from(UNUSED_DAYS) * 86_400);
    let recent = |line: &&str| {
        serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|value| value.get("at").and_then(Value::as_u64))
            .is_some_and(|written| written >= oldest)
    };
    let all: Vec<&str> = old
        .lines()
        .filter(recent)
        .chain(lines.iter().map(String::as_str))
        .collect();
    let most = usize::try_from(MAX_BYTES).unwrap_or(usize::MAX);
    let mut kept = Vec::new();
    let mut bytes = 0;
    for line in all.iter().rev().take(MAX_LOG_LINES) {
        if bytes + line.len() + 1 > most {
            break;
        }
        bytes += line.len() + 1;
        kept.push(*line);
    }
    let mut text = String::with_capacity(bytes);
    for line in kept.iter().rev() {
        text.push_str(line);
        text.push('\n');
    }
    text
}

/// The day `secs` seconds since the epoch is in, counted from the epoch.
pub fn day_of(secs: u64) -> u32 {
    u32::try_from(secs / 86_400).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
