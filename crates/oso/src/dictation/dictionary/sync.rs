//! One pass over TypeWhisper's dictionary (ADR 0175, step 4): its terms and corrections are
//! read, what oso added is checked against them, then what is to go goes and what is ready is
//! added. The host gives the transport, keeps the list when asked, and logs what was done.
//!
//! Must not depend on: `app`, the files or how a call is made; the tests fake the transport.
//!
//! - **Nothing is changed that was not read just before**: an entry is removed only while
//!   TypeWhisper gives it exactly as oso added it, and added only beside nothing like it.
//! - **The list is kept before TypeWhisper is asked to change**, so that a pass cut short never
//!   leaves an entry oso added out of the list. One whose call's outcome is not known stays in it
//!   as unsure, until TypeWhisper gives it, or on a later day does not.
//! - A call with no answer ends the pass: nothing is tried twice. So does a stop, as a dictation
//!   or a forget came, and [`MAX_TIME`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::list::{By, Entry, List, MAX_ENTRIES, MAX_LISTED, State};
use super::rules::clashes;
use super::{Dictionary, Held, Kind, corrections, terms};
use crate::dictation::api::{Call, Failure, Transport};
use crate::dictation::discovery::Discovery;
use crate::dictation::learning::Record;

/// How long after its start a pass still starts a change.
pub const MAX_TIME: Duration = Duration::from_secs(10);

/// A term nobody has: TypeWhisper answers its removal only once its dictionary is loaded.
const PROBE: &str = "oso-dictation-loaded-probe-5c1f0e9a";

/// What a pass did, for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Done {
    pub what: What,
    pub kind: Kind,
    pub wrong: String,
    pub right: String,
    /// Why, unless it was added.
    pub why: &'static str,
}

/// What became of an entry or a pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    /// It was added to TypeWhisper's dictionary.
    Added,
    /// oso removed it from there.
    Removed,
    /// It is the person's now, or gone by them.
    Released,
    /// It was seen often enough, and is not promoted.
    Skipped,
}

impl What {
    /// As the log writes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Released => "released",
            Self::Skipped => "skipped",
        }
    }
}

const THEIRS: &str = "you changed or removed it in TypeWhisper";

/// What came of a pass.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub done: Vec<Done>,
    /// How many entries a forget leaves in TypeWhisper, as the person changed them.
    pub kept: usize,
    /// TypeWhisper did not answer as it should: a pass soon after would come to the same.
    pub stuck: bool,
}

impl Outcome {
    fn note(&mut self, entry: &Entry, what: What, why: &'static str) {
        self.done.push(Done {
            what,
            kind: entry.kind,
            wrong: entry.wrong.clone(),
            right: entry.right.clone(),
            why,
        });
    }
}

/// The pairs a pass could not promote, each with the day: looked at again only on another day,
/// so that one that never goes costs no calls each time, and is logged once.
pub type Blocked = HashMap<(String, String), u32>;

/// One pass.
pub struct Pass<'a> {
    pub transport: &'a mut dyn Transport,
    pub discovery: &'a Discovery,
    /// The TypeWhisper it is over, as its discovery file's path.
    pub server: &'a str,
    pub today: u32,
    pub blocked: &'a mut Blocked,
    /// Keeps the list: `false` if it could not, and the pass ends.
    pub keep: &'a mut dyn FnMut(&List) -> bool,
    /// Whether the pass is to end.
    pub stop: &'a dyn Fn() -> bool,
}

/// Whether a pass over `server` would have something to ask TypeWhisper for.
pub fn wanted(list: &List, record: &Record, server: &str, today: u32, blocked: &Blocked) -> bool {
    let waits = |entry: &Entry| {
        entry.state != State::Live
            || (entry.unsure && entry.added < today)
            || List::over(entry, record, today).is_some()
    };
    (list.entries.iter()).any(|entry| entry.server == server && waits(entry))
        || (list.candidates(record, server).iter()).any(|(pair, _, unfit)| {
            let key = (pair.wrong.clone(), pair.right.clone());
            unfit.is_none() && blocked.get(&key) != Some(&today)
        })
}

/// Why a change did not go through.
enum Failed {
    /// It never reached TypeWhisper.
    Unsent,
    /// It may have been made, or not.
    Unknown,
    /// TypeWhisper refused it.
    Refused,
}

impl Pass<'_> {
    /// Runs it over `list` and what `record` taught.
    pub fn run(mut self, list: &mut List, record: &Record) -> Outcome {
        let mut out = Outcome::default();
        let Some(mut dictionary) = self.read() else {
            out.stuck = true;
            return out;
        };
        // A dictionary with nothing in it, though oso added to it, may be one not loaded yet.
        let mine = list.entries.iter().any(|entry| entry.server == self.server);
        if dictionary.is_empty() && mine && !self.loaded() {
            out.stuck = true;
            return out;
        }
        let today = self.today;
        self.blocked.retain(|_, day| *day == today);
        let mut kept = list.clone();
        for entry in self.of_server(list) {
            self.check(list, &entry, &dictionary, &mut out);
        }
        let mut going = self.going(list, record);
        let mut added = Vec::new();
        if !(self.stop)() {
            added = self.choose(list, record, &mut dictionary, &mut going, &mut out);
        }
        if !self.keep_changed(list, &mut kept) {
            // Nothing was asked, and nothing is said of what is not kept.
            return Outcome::default();
        }
        let until = Instant::now() + MAX_TIME;
        let ended = self.remove(list, &going, until, &mut out);
        self.add(list, &added, until, ended, &mut out);
        list.age(today);
        self.keep_changed(list, &mut kept);
        out
    }

    /// Keeps `list` if it is not as it was `kept` last.
    fn keep_changed(&mut self, list: &List, kept: &mut List) -> bool {
        if list == kept {
            return true;
        }
        kept.clone_from(list);
        (self.keep)(list)
    }

    fn of_server(&self, list: &List) -> Vec<Entry> {
        (list.entries.iter())
            .filter(|entry| entry.server == self.server)
            .cloned()
            .collect()
    }

    /// TypeWhisper's dictionary, if both its halves come.
    fn read(&mut self) -> Option<Dictionary> {
        Some(Dictionary {
            terms: terms(&self.ask(&Call::Terms)?)?,
            corrections: corrections(&self.ask(&Call::Corrections)?)?,
        })
    }

    /// The half of it `kind` is in, as it is now.
    fn read_half(&mut self, kind: Kind) -> Option<Dictionary> {
        let mut half = Dictionary::default();
        match kind {
            Kind::Term => half.terms = terms(&self.ask(&Call::Terms)?)?,
            Kind::Correction => half.corrections = corrections(&self.ask(&Call::Corrections)?)?,
        }
        Some(half)
    }

    fn ask(&mut self, call: &Call) -> Option<Vec<u8>> {
        match self.transport.call(self.discovery, call) {
            Ok(answer) if answer.status == 200 => Some(answer.body),
            _ => None,
        }
    }

    /// Whether TypeWhisper's dictionary is loaded: it answers the removal of a term nobody has.
    fn loaded(&mut self) -> bool {
        self.ask(&Call::DeleteTerm(PROBE.to_owned())).is_some()
    }

    /// Asks for a change.
    fn change(&mut self, call: &Call) -> Result<(), Failed> {
        match self.transport.call(self.discovery, call) {
            Ok(answer) if answer.status == 200 => Ok(()),
            Ok(answer) if (400..500).contains(&answer.status) => Err(Failed::Refused),
            Err(Failure::NotRunning) => Err(Failed::Unsent),
            _ => Err(Failed::Unknown),
        }
    }

    /// Checks `entry` of the list against `dictionary`: one it does not hold exactly leaves the
    /// list, unless the call that adds it may still arrive. Says whether it stays.
    fn check(
        &self,
        list: &mut List,
        entry: &Entry,
        dictionary: &Dictionary,
        out: &mut Outcome,
    ) -> bool {
        let held = dictionary.holds(entry);
        match held {
            Held::Exactly => {
                for other in list.entries.iter_mut().filter(|other| entry.same(other)) {
                    other.unsure = false;
                }
                return true;
            }
            Held::No if entry.unsure && entry.added >= self.today => return true,
            _ => {}
        }
        list.drop_entry(entry);
        match (entry.state, held) {
            // It never arrived.
            (State::Live | State::Removing, Held::No) if entry.unsure => {}
            (State::Removing, Held::No) => {
                list.note_let_go(entry, By::Oso, self.today);
                out.note(entry, What::Removed, "gone");
            }
            (State::Live | State::Removing, _) => {
                list.note_let_go(entry, By::Person, self.today);
                out.note(entry, What::Released, THEIRS);
            }
            (State::Forgetting, Held::No) => {}
            (State::Forgetting, _) => out.kept += 1,
        }
        false
    }

    /// Marks what is to go, and why: what was marked before, and what is over now.
    fn going(&self, list: &mut List, record: &Record) -> Vec<(Entry, &'static str)> {
        let mut going = Vec::new();
        let today = self.today;
        for entry in (list.entries.iter_mut()).filter(|entry| entry.server == self.server) {
            let why = match entry.state {
                State::Forgetting => Some("forgotten"),
                State::Removing => Some("to go since before"),
                State::Live if entry.unsure => None,
                State::Live => List::over(entry, record, today),
            };
            if let Some(why) = why {
                if entry.state == State::Live {
                    entry.state = State::Removing;
                }
                going.push((entry.clone(), why));
            }
        }
        going
    }

    /// Chooses what is added, each put in the list as unsure and in `dictionary`, within
    /// [`MAX_ENTRIES`] that stay: when full, the least recently used joins `going`.
    fn choose(
        &mut self,
        list: &mut List,
        record: &Record,
        dictionary: &mut Dictionary,
        going: &mut Vec<(Entry, &'static str)>,
        out: &mut Outcome,
    ) -> Vec<Entry> {
        let mut added: Vec<Entry> = Vec::new();
        for (pair, kind, unfit) in list.candidates(record, self.server) {
            let key = (pair.wrong.clone(), pair.right.clone());
            if self.blocked.get(&key) == Some(&self.today) {
                continue;
            }
            let entry = Entry {
                kind,
                wrong: pair.wrong.clone(),
                right: pair.right.clone(),
                unsure: true,
                added: self.today,
                used: self.today,
                state: State::Live,
                server: self.server.to_owned(),
            };
            let clash = unfit.or_else(|| clashes((&pair.wrong, &pair.right), kind, dictionary));
            if let Some(why) = clash {
                self.blocked.insert(key, self.today);
                out.note(&entry, What::Skipped, why);
                continue;
            }
            let full = list.staying(self.server) >= MAX_ENTRIES;
            let room = list.makes_room(self.server, self.today);
            if list.entries.len() >= MAX_LISTED || (full && room.is_none()) {
                // Not asked again today, as room is only made by another day.
                self.blocked.insert(key, self.today);
                continue;
            }
            if let Some(at) = room.filter(|_| full) {
                list.entries[at].state = State::Removing;
                going.push((list.entries[at].clone(), "made room for another"));
            }
            dictionary.add(&entry);
            list.entries.push(entry.clone());
            added.push(entry);
        }
        added
    }

    /// Whether nothing more is asked: the pass is stopped, or its time is over.
    fn over(&self, until: Instant) -> bool {
        Instant::now() >= until || (self.stop)()
    }

    /// Removes `going` from TypeWhisper and the list; `true` once nothing more is asked.
    fn remove(
        &mut self,
        list: &mut List,
        going: &[(Entry, &'static str)],
        until: Instant,
        out: &mut Outcome,
    ) -> bool {
        for (entry, why) in going {
            if self.over(until) {
                return true;
            }
            let Some(now) = self.read_half(entry.kind) else {
                out.stuck = true;
                return true;
            };
            if now.holds(entry) != Held::Exactly {
                self.check(list, entry, &now, out);
                continue;
            }
            let call = match entry.kind {
                Kind::Term => Call::DeleteTerm(entry.right.clone()),
                Kind::Correction => Call::DeleteCorrection(entry.wrong.clone()),
            };
            match self.change(&call) {
                Ok(()) => {
                    list.drop_entry(entry);
                    if entry.state != State::Forgetting {
                        list.note_let_go(entry, By::Oso, self.today);
                        out.note(entry, What::Removed, why);
                    }
                }
                // It stays to go; the others go on.
                Err(Failed::Refused) => out.stuck = true,
                Err(Failed::Unsent | Failed::Unknown) => {
                    out.stuck = true;
                    return true;
                }
            }
        }
        false
    }

    /// Adds `added` to TypeWhisper, or, once `ended`, takes it back out of the list. One whose
    /// outcome is not known stays in it as unsure; one refused is not asked again until its pair
    /// is seen again.
    fn add(
        &mut self,
        list: &mut List,
        added: &[Entry],
        until: Instant,
        mut ended: bool,
        out: &mut Outcome,
    ) {
        for entry in added {
            ended = ended || self.over(until);
            let now = if ended {
                None
            } else {
                self.read_half(entry.kind)
            };
            let Some(now) = now else {
                out.stuck |= !ended;
                ended = true;
                list.drop_entry(entry);
                continue;
            };
            if let Some(why) = clashes((&entry.wrong, &entry.right), entry.kind, &now) {
                let key = (entry.wrong.clone(), entry.right.clone());
                self.blocked.insert(key, self.today);
                list.drop_entry(entry);
                out.note(entry, What::Skipped, why);
                continue;
            }
            let call = match entry.kind {
                Kind::Term => Call::PutTerm(entry.right.clone()),
                Kind::Correction => Call::PutCorrection {
                    original: entry.wrong.clone(),
                    replacement: entry.right.clone(),
                },
            };
            match self.change(&call) {
                Ok(()) => {
                    for other in list.entries.iter_mut().filter(|other| entry.same(other)) {
                        other.unsure = false;
                    }
                    out.note(entry, What::Added, "");
                }
                Err(Failed::Unknown) => {
                    out.stuck = true;
                    ended = true;
                }
                Err(Failed::Unsent) => {
                    out.stuck = true;
                    ended = true;
                    list.drop_entry(entry);
                }
                Err(Failed::Refused) => {
                    list.drop_entry(entry);
                    list.note_let_go(entry, By::Oso, self.today);
                    out.note(entry, What::Skipped, "TypeWhisper refused it");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
