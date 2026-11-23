//! What oso keeps of the entries it added to TypeWhisper's dictionary (ADR 0175, step 4): the
//! entries, those let go, and when one was last used.
//!
//! Must not depend on: the calls, the files or the app; this sees strings and days alone.
//!
//! - **The list** keeps at most [`MAX_ENTRIES`] entries of one TypeWhisper, its server, that oso
//!   added and still owns. One whose call's outcome is not known is `unsure`.
//! - **An entry that is the person's now**, as they changed, disabled or removed it, is let go
//!   and never added again, until `dictation-forget-learned`; one oso removed, only once its pair
//!   is seen again.
//! - A list that is not whole is refused, as what oso added would be forgotten.

use serde_json::{Value, json};

use super::Kind;
use super::matching::Key;
use super::rules::{MAX_CHARS, kind_of, ready, taken_back, unfit};
use crate::dictation::learning::{Pair, Record, UNUSED_DAYS};

/// The list's file, in the learning's directory.
pub const FILE: &str = "typewhisper.json";
/// Where a write of the list goes first.
pub const PART: &str = "typewhisper.json.part";
/// The file whose lock the one instance of oso that changes the list holds.
pub const LOCK: &str = "typewhisper.lock";
/// The empty file that says a forget has yet to mark the list.
pub const DUE: &str = "typewhisper.forget";
/// The most entries oso keeps in one TypeWhisper's dictionary.
pub const MAX_ENTRIES: usize = 40;
/// The most entries the list holds, those that are to go and other servers' among them: past
/// them, none is added.
pub const MAX_LISTED: usize = 4 * MAX_ENTRIES;
/// The most entries the list remembers as let go, oso's own dropped first.
const MAX_LET_GO: usize = 200;
/// The most bytes of a server's name.
pub const MAX_SERVER: usize = 1024;
/// The list's version.
const VERSION: u64 = 1;

/// Where an entry is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// It stays.
    Live,
    /// It goes, once TypeWhisper answers.
    Removing,
    /// As [`Self::Removing`], as the person forgot what was learned: nothing of it is logged.
    Forgetting,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Removing => "removing",
            Self::Forgetting => "forgetting",
        }
    }

    fn of(name: &str) -> Option<Self> {
        [Self::Live, Self::Removing, Self::Forgetting]
            .into_iter()
            .find(|state| state.name() == name)
    }
}

/// An entry oso added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    pub wrong: String,
    pub right: String,
    /// Whether the call that adds it arrived is not known.
    pub unsure: bool,
    /// The day it was added.
    pub added: u32,
    /// The day a dictation last used it, or it was added.
    pub used: u32,
    pub state: State,
    /// The TypeWhisper it is in, as its discovery file's path.
    pub server: String,
}

impl Entry {
    /// What TypeWhisper knows it by.
    pub fn known_by(&self) -> &str {
        self.kind.known_by(&self.wrong, &self.right)
    }

    /// Whether `other` is the same entry of the list.
    pub fn same(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.known_by() == other.known_by()
            && self.server == other.server
    }
}

/// Who let an entry go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum By {
    /// The person changed, disabled or removed it in TypeWhisper.
    Person,
    /// oso removed it, or TypeWhisper refused it.
    Oso,
}

/// An entry let go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetGo {
    pub kind: Kind,
    pub wrong: String,
    pub right: String,
    pub by: By,
    pub day: u32,
}

/// What oso added to TypeWhisper's dictionary, and what it does not add again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct List {
    pub entries: Vec<Entry>,
    pub let_go: Vec<LetGo>,
}

impl List {
    /// The list in `bytes`, with no day after `today`: `None` if it is not one, which a write
    /// must then not replace.
    pub fn parse(bytes: &[u8], today: u32) -> Option<Self> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        if value.get("version").and_then(Value::as_u64) != Some(VERSION) {
            return None;
        }
        let all = |key: &str, most: usize| {
            let all = value.get(key)?.as_array()?;
            (all.len() <= most).then_some(all)
        };
        let list = Self {
            entries: (all("entries", MAX_LISTED)?.iter())
                .map(|entry| entry_of(entry, today))
                .collect::<Option<_>>()?,
            let_go: (all("let_go", MAX_LET_GO)?.iter())
                .map(|past| let_go_of(past, today))
                .collect::<Option<_>>()?,
        };
        let twice = (list.entries.iter().enumerate())
            .any(|(at, entry)| list.entries[..at].iter().any(|other| entry.same(other)));
        (!twice).then_some(list)
    }

    /// The list as its file holds it.
    pub fn to_text(&self) -> String {
        let entries: Vec<Value> = (self.entries.iter())
            .map(|entry| {
                json!({
                    "kind": entry.kind.name(),
                    "wrong": entry.wrong,
                    "right": entry.right,
                    "unsure": entry.unsure,
                    "added": entry.added,
                    "used": entry.used,
                    "state": entry.state.name(),
                    "server": entry.server,
                })
            })
            .collect();
        let let_go: Vec<Value> = (self.let_go.iter())
            .map(|past| {
                json!({
                    "kind": past.kind.name(),
                    "wrong": past.wrong,
                    "right": past.right,
                    "person": past.by == By::Person,
                    "day": past.day,
                })
            })
            .collect();
        let mut text = json!({
            "version": VERSION,
            "entries": entries,
            "let_go": let_go,
        })
        .to_string();
        text.push('\n');
        text
    }

    /// Whether nothing is in it.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.let_go.is_empty()
    }

    /// Notes on `today` which entries a dictation used: a term in its `text`, or a correction
    /// whose `wrong` is in what was recognised, `raw`, if TypeWhisper gave that, and whose
    /// `right` is in `text`. Says whether a day changed.
    pub fn note_used(&mut self, text: &str, raw: Option<&str>, today: u32) -> bool {
        let mut changed = false;
        let mut folded = None;
        for entry in self.entries.iter_mut().filter(|entry| entry.used < today) {
            let used = match entry.kind {
                Kind::Term => folded
                    .get_or_insert_with(|| text.to_lowercase())
                    .contains(&entry.right.to_lowercase()),
                Kind::Correction => {
                    text.contains(&entry.right) && raw.is_none_or(|raw| raw.contains(&entry.wrong))
                }
            };
            if used {
                entry.used = today;
                changed = true;
            }
        }
        changed
    }

    /// Marks every entry to go, and forgets those let go.
    pub fn forget(&mut self) {
        self.let_go.clear();
        for entry in &mut self.entries {
            entry.state = State::Forgetting;
        }
    }

    /// Whether `pair`, as `kind`, was let go and may not be added now.
    fn may_not_return(&self, pair: &Pair, kind: Kind) -> bool {
        let known_by = kind.known_by(&pair.wrong, &pair.right);
        (self.let_go.iter()).any(|past| match past.by {
            By::Person => {
                past.kind == kind
                    && Key::of(kind.known_by(&past.wrong, &past.right)).like(&Key::of(known_by))
            }
            By::Oso => {
                past.wrong == pair.wrong && past.right == pair.right && pair.last <= past.day
            }
        })
    }

    /// Remembers that `entry` was let go.
    pub fn note_let_go(&mut self, entry: &Entry, by: By, day: u32) {
        (self.let_go).retain(|past| {
            past.kind != entry.kind || past.wrong != entry.wrong || past.right != entry.right
        });
        self.let_go.push(LetGo {
            kind: entry.kind,
            wrong: entry.wrong.clone(),
            right: entry.right.clone(),
            by,
            day,
        });
        while self.let_go.len() > MAX_LET_GO {
            let at = self.let_go.iter().position(|past| past.by == By::Oso);
            self.let_go.remove(at.unwrap_or(0));
        }
    }

    /// Forgets what oso itself let go long enough ago that its pair is gone from the record.
    pub fn age(&mut self, today: u32) {
        (self.let_go)
            .retain(|past| past.by == By::Person || today.saturating_sub(past.day) < UNUSED_DAYS);
    }

    /// Takes `entry` out of the list.
    pub fn drop_entry(&mut self, entry: &Entry) {
        self.entries.retain(|other| !entry.same(other));
    }

    /// Why `entry` is to go, by its age or by what `record` says, if it is.
    pub fn over(entry: &Entry, record: &Record, today: u32) -> Option<&'static str> {
        if today.saturating_sub(entry.used) >= UNUSED_DAYS {
            return Some("not used for 30 days");
        }
        let pair = record.find(&entry.wrong, &entry.right).map_or_else(
            || Pair {
                wrong: entry.wrong.clone(),
                right: entry.right.clone(),
                ..Pair::default()
            },
            |at| record.pairs[at].clone(),
        );
        taken_back(&pair, record, entry.added).then_some("you took it back")
    }

    /// The pairs of `record` seen often enough, not in the list for `server` and not let go, the
    /// most used first, then the latest, each with what it would be and why its own text keeps
    /// it from being added.
    pub fn candidates<'r>(
        &self,
        record: &'r Record,
        server: &str,
    ) -> Vec<(&'r Pair, Kind, Option<&'static str>)> {
        let mut pairs: Vec<(&Pair, Kind)> = (record.pairs.iter())
            .filter(|pair| ready(pair))
            .map(|pair| (pair, kind_of(pair)))
            .filter(|(pair, kind)| !self.may_not_return(pair, *kind))
            .filter(|(pair, kind)| {
                !(self.entries.iter()).any(|entry| {
                    entry.server == server
                        && entry.kind == *kind
                        && entry.known_by() == kind.known_by(&pair.wrong, &pair.right)
                })
            })
            .collect();
        pairs.sort_by_key(|(pair, _)| std::cmp::Reverse((pair.stood(), pair.last)));
        pairs
            .into_iter()
            .map(|(pair, kind)| (pair, kind, unfit(pair, record)))
            .collect()
    }

    /// The entry of `server` that makes room on `today` for another: the least recently used of
    /// those that stay, if a dictation did not use it today.
    pub fn makes_room(&self, server: &str, today: u32) -> Option<usize> {
        (self.entries.iter().enumerate())
            .filter(|(_, entry)| {
                entry.server == server
                    && entry.state == State::Live
                    && !entry.unsure
                    && entry.used < today
            })
            .min_by_key(|(_, entry)| (entry.used, entry.added))
            .map(|(at, _)| at)
    }

    /// How many entries of `server` stay.
    pub fn staying(&self, server: &str) -> usize {
        (self.entries.iter())
            .filter(|entry| entry.server == server && entry.state == State::Live)
            .count()
    }
}

fn day_in(value: &Value, key: &str, today: u32) -> Option<u32> {
    let day = u32::try_from(value.get(key)?.as_u64()?).ok()?;
    Some(day.min(today))
}

/// Both sides of `value`. What they hold was judged when they were added, and is not again: an
/// entry is not left out for a rule made since.
fn sides_of(value: &Value) -> Option<(String, String)> {
    let side = |key: &str| {
        let text = value.get(key)?.as_str()?;
        let holds = !text.trim().is_empty() && text.chars().count() <= MAX_CHARS;
        holds.then(|| text.to_owned())
    };
    Some((side("wrong")?, side("right")?))
}

fn entry_of(value: &Value, today: u32) -> Option<Entry> {
    let (wrong, right) = sides_of(value)?;
    let server = value.get("server")?.as_str()?;
    if server.len() > MAX_SERVER {
        return None;
    }
    Some(Entry {
        kind: Kind::of(value.get("kind")?.as_str()?)?,
        wrong,
        right,
        unsure: value.get("unsure")?.as_bool()?,
        added: day_in(value, "added", today)?,
        used: day_in(value, "used", today)?,
        state: State::of(value.get("state")?.as_str()?)?,
        server: server.to_owned(),
    })
}

fn let_go_of(value: &Value, today: u32) -> Option<LetGo> {
    let (wrong, right) = sides_of(value)?;
    let person = value.get("person")?.as_bool()?;
    Some(LetGo {
        kind: Kind::of(value.get("kind")?.as_str()?)?,
        wrong,
        right,
        by: if person { By::Person } else { By::Oso },
        day: day_in(value, "day", today)?,
    })
}

#[cfg(test)]
mod tests;
