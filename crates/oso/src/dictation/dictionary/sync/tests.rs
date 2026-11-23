//! A pass over a fake TypeWhisper's dictionary: what it adds and removes, and what it leaves.
//!
//! Must not depend on: curl, files or the app.

use serde_json::{Value, json};

use super::*;
use crate::dictation::api::Answer;
use crate::dictation::discovery;
use crate::dictation::learning::Pair;

const DAY: u32 = 20_000;

/// What the person does to a [`Fake`].
type Act = fn(&mut Fake);

/// A TypeWhisper whose dictionary is kept here, and what it was asked.
#[derive(Default)]
struct Fake {
    terms: Vec<String>,
    /// Terms with a similarity the person gave.
    tuned: Vec<String>,
    corrections: Vec<(String, String, bool)>,
    /// Corrections the person disabled, which it does not give.
    disabled: Vec<(String, String, bool)>,
    changes: Vec<Call>,
    reads: usize,
    /// The answers to changes, the first first; then each is made.
    answers: Vec<Result<u16, Failure>>,
    /// Its dictionary is not loaded: it gives none, and refuses each change with a 500.
    unloaded: bool,
    /// What the person does once it was read that often.
    then: Option<(usize, Act)>,
}

impl Fake {
    fn make(&mut self, call: &Call) {
        match call {
            Call::PutTerm(term) => self.terms.push(term.clone()),
            Call::DeleteTerm(term) => self.terms.retain(|other| other != term),
            Call::PutCorrection {
                original,
                replacement,
            } => {
                self.disabled.retain(|(other, ..)| other != original);
                (self.corrections).push((original.clone(), replacement.clone(), true));
            }
            Call::DeleteCorrection(original) => {
                self.corrections.retain(|(other, ..)| other != original);
            }
            _ => panic!("not a change of the dictionary"),
        }
    }

    fn read(&mut self, call: &Call) -> Value {
        self.reads += 1;
        if let Some((_, act)) = self.then.take_if(|(at, _)| *at == self.reads) {
            act(self);
        }
        match call {
            Call::Terms if self.unloaded => json!({"terms": [], "term_entries": []}),
            Call::Corrections if self.unloaded => json!({"corrections": []}),
            Call::Terms => {
                let entries: Vec<Value> = (self.terms.iter())
                    .map(|term| {
                        let similarity = self.tuned.contains(term).then_some(0.5);
                        json!({"term": term, "ctc_min_similarity": similarity})
                    })
                    .collect();
                json!({"terms": self.terms, "term_entries": entries})
            }
            _ => {
                let all: Vec<Value> = (self.corrections.iter())
                    .map(|(original, replacement, case)| {
                        json!({
                            "original": original,
                            "replacement": replacement,
                            "caseSensitive": case,
                        })
                    })
                    .collect();
                json!({"corrections": all})
            }
        }
    }
}

impl Transport for Fake {
    fn call(&mut self, _: &Discovery, call: &Call) -> Result<Answer, Failure> {
        let answer = |status, body: Vec<u8>| Ok(Answer { status, body });
        match call {
            Call::Terms | Call::Corrections => {
                let body = self.read(call).to_string();
                answer(200, body.into_bytes())
            }
            _ if self.unloaded => answer(500, Vec::new()),
            Call::DeleteTerm(term) if term == PROBE => answer(200, Vec::new()),
            change => {
                self.changes.push(change.clone());
                let status = if self.answers.is_empty() {
                    200
                } else {
                    self.answers.remove(0)?
                };
                if status == 200 {
                    self.make(change);
                }
                answer(status, Vec::new())
            }
        }
    }

    fn call_and_go(&mut self, _: &Discovery, _: &Call) {
        panic!("a pass waits for each answer");
    }
}

/// A pass's parts that outlive it.
#[derive(Default)]
struct Ground {
    fake: Fake,
    list: List,
    blocked: Blocked,
    /// What was kept, in order.
    kept: Vec<List>,
}

impl Ground {
    /// Runs a pass on `today`, unless nothing is wanted, as the host does.
    fn pass(&mut self, record: &Record, today: u32) -> Outcome {
        if !wanted(&self.list, record, "here", today, &self.blocked) {
            return Outcome::default();
        }
        self.run(record, today, &|| false)
    }

    fn run(&mut self, record: &Record, today: u32, stop: &dyn Fn() -> bool) -> Outcome {
        let file = br#"{"port": 8978, "token": "abcdefghijklmnopqrstuvwxyz012345", "version": 1}"#;
        self.kept.clear();
        let kept = &mut self.kept;
        Pass {
            transport: &mut self.fake,
            discovery: &discovery::parse(file).unwrap(),
            server: "here",
            today,
            blocked: &mut self.blocked,
            keep: &mut |list| {
                kept.push(list.clone());
                true
            },
            stop,
        }
        .run(&mut self.list, record)
    }

    /// With the entries of `record` added.
    fn with(record: &Record) -> Self {
        let mut ground = Self::default();
        let out = ground.pass(record, DAY);
        assert!(
            out.done.iter().all(|done| done.what == What::Added),
            "{out:?}"
        );
        ground.fake.changes.clear();
        ground
    }
}

fn learned(pairs: &[(&str, &str)]) -> Record {
    Record {
        pairs: (pairs.iter())
            .map(|(wrong, right)| Pair {
                wrong: (*wrong).into(),
                right: (*right).into(),
                fixes: 3,
                days: vec![DAY - 1, DAY],
                last: DAY,
                ..Pair::default()
            })
            .collect(),
    }
}

fn whats(out: &Outcome) -> Vec<(What, &str)> {
    (out.done.iter())
        .map(|done| (done.what, done.wrong.as_str()))
        .collect()
}

fn both() -> Record {
    learned(&[("リアクト", "React"), ("機能", "昨日")])
}

#[test]
fn what_is_ready_is_added_once_and_is_in_the_list_first() {
    let mut ground = Ground::default();
    ground.fake.terms.push("oso".into());
    let out = ground.pass(&both(), DAY);
    assert_eq!(
        whats(&out),
        [(What::Added, "リアクト"), (What::Added, "機能")]
    );
    let first = &ground.kept[0].entries;
    assert!(first.len() == 2 && first.iter().all(|entry| entry.unsure));
    assert!(ground.list.entries.iter().all(|entry| !entry.unsure));
    assert_eq!(ground.kept.last(), Some(&ground.list));
    assert_eq!(ground.fake.terms, ["oso", "React"]);
    assert_eq!(
        ground.fake.corrections,
        [("機能".into(), "昨日".into(), true)]
    );
    let reads = ground.fake.reads;
    let _ = ground.pass(&both(), DAY);
    assert_eq!(ground.fake.reads, reads, "nothing more is asked");
}

#[test]
fn the_persons_own_entries_are_never_written_over_or_removed() {
    let mut ground = Ground::default();
    ground.fake.terms.push("react".into());
    (ground.fake.corrections).push(("機能".into(), "きのう".into(), false));
    let out = ground.pass(&both(), DAY);
    assert_eq!(
        whats(&out),
        [(What::Skipped, "リアクト"), (What::Skipped, "機能")]
    );
    assert!(ground.kept.is_empty(), "nothing to keep");
    let reads = ground.fake.reads;
    let _ = ground.pass(&both(), DAY);
    assert_eq!(ground.fake.reads, reads, "looked at once a day");
    ground.list.forget();
    let _ = ground.run(&Record::default(), DAY + 99, &|| false);
    assert!(ground.fake.changes.is_empty() && ground.list.is_empty());
}

#[test]
fn an_entry_the_person_changed_tuned_or_removed_is_theirs_for_good() {
    let change: [Act; 4] = [
        |fake| {
            fake.terms.clear();
            fake.corrections[0].1 = "きのう".into();
        },
        |fake| {
            fake.tuned.push("React".into());
            fake.corrections[0].2 = false;
        },
        |fake| {
            fake.terms = vec!["react".into()];
            fake.corrections.clear();
        },
        // Others beside oso's, which a removal could take instead.
        |fake| {
            fake.terms.push("Réact".into());
            (fake.corrections).push(("機能".into(), "きのう".into(), false));
        },
    ];
    for act in change {
        let mut ground = Ground::with(&both());
        act(&mut ground.fake);
        let mut seen = both();
        for pair in &mut seen.pairs {
            pair.last = DAY + 31;
        }
        // Old enough to go: oso removes none of it.
        let out = ground.pass(&Record::default(), DAY + 30);
        assert_eq!(
            whats(&out),
            [(What::Released, "リアクト"), (What::Released, "機能")]
        );
        let _ = ground.pass(&seen, DAY + 31);
        assert!(ground.fake.changes.is_empty() && ground.list.entries.is_empty());
    }
}

#[test]
fn a_forget_says_what_the_person_changed() {
    let mut ground = Ground::with(&both());
    ground.fake.corrections[0].1 = "きのう".into();
    ground.list.forget();
    let out = ground.run(&Record::default(), DAY, &|| false);
    assert_eq!((out.kept, out.done.len()), (1, 0), "and nothing is logged");
    assert_eq!(ground.fake.changes, [Call::DeleteTerm("React".into())]);
    assert!(ground.list.is_empty());
}

#[test]
fn what_the_person_does_during_a_pass_is_seen_before_each_change() {
    // Two reads at the start, then one before the change.
    let mut ground = Ground::with(&learned(&[("機能", "昨日")]));
    ground.fake.reads = 0;
    ground.fake.then = Some((3, |fake| fake.corrections[0].1 = "きのう".into()));
    let out = ground.pass(&Record::default(), DAY + 30);
    assert_eq!(whats(&out), [(What::Released, "機能")]);
    assert!(ground.fake.changes.is_empty());

    let mut ground = Ground::default();
    ground.fake.then = Some((3, |fake| fake.terms.push("REACT".into())));
    let out = ground.pass(&learned(&[("リアクト", "React")]), DAY);
    assert_eq!(whats(&out), [(What::Skipped, "リアクト")]);
    assert!(ground.fake.changes.is_empty() && ground.list.entries.is_empty());
}

#[test]
fn an_empty_dictionary_is_believed_only_once_it_is_loaded() {
    for state in [State::Live, State::Removing, State::Forgetting] {
        let mut ground = Ground::with(&both());
        for entry in &mut ground.list.entries {
            entry.state = state;
        }
        let before = ground.list.clone();
        ground.fake.unloaded = true;
        let out = ground.run(&both(), DAY + 30, &|| false);
        assert!(out.stuck && out.done.is_empty());
        assert_eq!(ground.list, before);
        ground.fake.unloaded = false;
        ground.fake.terms.clear();
        ground.fake.corrections.clear();
        let _ = ground.run(&Record::default(), DAY + 30, &|| false);
        assert!(ground.list.entries.is_empty());
    }
}

/// The limit the author accepted: TypeWhisper gives no disabled entry.
#[test]
fn a_disabled_entry_of_the_same_word_is_written_over() {
    let mut ground = Ground::default();
    (ground.fake.disabled).push(("機能".into(), "きのう".into(), false));
    let _ = ground.pass(&learned(&[("機能", "昨日")]), DAY);
    assert!(ground.fake.disabled.is_empty());
    // But one oso added, saw disabled and let go is not enabled again.
    let mut ground = Ground::with(&learned(&[("機能", "昨日")]));
    let theirs = ground.fake.corrections.remove(0);
    ground.fake.disabled.push(theirs);
    let _ = ground.pass(&Record::default(), DAY + 30);
    let _ = ground.pass(&learned(&[("機能", "帰納")]), DAY + 30);
    assert_eq!(ground.fake.disabled.len(), 1);
    assert!(ground.fake.changes.is_empty());
}

#[test]
fn what_no_dictation_used_for_a_month_goes() {
    let mut ground = Ground::with(&both());
    ground.list.entries[1].used = DAY + 5;
    let none = Record::default();
    let _ = ground.pass(&none, DAY + 29);
    assert!(ground.fake.changes.is_empty());
    let out = ground.pass(&none, DAY + 30);
    assert_eq!(whats(&out), [(What::Removed, "リアクト")]);
    assert_eq!(ground.kept[0].entries[0].state, State::Removing);
    assert!(ground.fake.terms.is_empty());
    assert_eq!(ground.fake.corrections.len(), 1);
    assert_eq!(ground.list.entries.len(), 1);
}

#[test]
fn what_the_person_took_back_goes() {
    let mut record = learned(&[("機能", "昨日")]);
    let mut ground = Ground::with(&record);
    record.pairs.push(Pair {
        wrong: "昨日".into(),
        right: "機能".into(),
        edits: 1,
        days: vec![DAY + 1],
        last: DAY + 1,
        ..Pair::default()
    });
    let out = ground.pass(&record, DAY + 1);
    assert_eq!(whats(&out), [(What::Removed, "機能")]);
    assert!(ground.fake.corrections.is_empty());
    let out = ground.pass(&record, DAY + 2);
    assert!(out.done.is_empty() && ground.list.entries.is_empty());
}

#[test]
fn past_the_cap_the_least_recently_used_makes_room_and_a_full_list_asks_once() {
    let words: Vec<(String, String)> = (0..=MAX_ENTRIES)
        .map(|n| (format!("語{n}語"), format!("word-{n}")))
        .collect();
    let all: Vec<(&str, &str)> = (words.iter())
        .map(|(wrong, right)| (wrong.as_str(), right.as_str()))
        .collect();
    let mut ground = Ground::with(&learned(&all[..MAX_ENTRIES]));
    assert_eq!(ground.list.entries.len(), MAX_ENTRIES);
    // All used today: none makes room, and it is asked once.
    let last = learned(&all[MAX_ENTRIES..]);
    let out = ground.pass(&last, DAY);
    assert!(out.done.is_empty() && ground.kept.is_empty());
    let reads = ground.fake.reads;
    let _ = ground.pass(&last, DAY);
    assert_eq!(ground.fake.reads, reads);
    for entry in &mut ground.list.entries {
        entry.used = DAY + 1;
    }
    ground.list.entries[7].used = DAY;
    let out = ground.pass(&last, DAY + 2);
    assert_eq!(
        whats(&out),
        [(What::Removed, "語7語"), (What::Added, "語40語")]
    );
    assert_eq!(ground.list.entries.len(), MAX_ENTRIES);
    assert_eq!(ground.fake.corrections.len(), MAX_ENTRIES);
    // Another server's entries take no room here.
    for entry in &mut ground.list.entries {
        entry.server = "elsewhere".into();
    }
    let out = ground.pass(&learned(&[("リアクト", "React")]), DAY + 2);
    assert_eq!(whats(&out), [(What::Added, "リアクト")]);
}

#[test]
fn a_change_with_no_answer_ends_the_pass_and_stays_unsure_for_the_day() {
    let record = learned(&[("リアクト", "React"), ("機能", "昨日"), ("帰納", "木能")]);
    let mut ground = Ground::default();
    ground.fake.answers = vec![Err(Failure::NoAnswer)];
    let out = ground.pass(&record, DAY);
    assert!(out.stuck && out.done.is_empty());
    assert_eq!(
        ground.fake.changes.len(),
        1,
        "nothing after it, nothing twice"
    );
    let [entry] = &ground.list.entries[..] else {
        panic!("what was never asked is not listed");
    };
    assert!(entry.unsure);
    // Not there yet, the same day: it may still come.
    let none = Record::default();
    ground.fake.terms.push("oso".into());
    let _ = ground.run(&none, DAY, &|| false);
    assert_eq!(ground.list.entries.len(), 1);
    // A forget the same day waits for it too, and removes it once it came.
    let mut forgotten = Ground {
        list: ground.list.clone(),
        ..Ground::default()
    };
    forgotten.list.forget();
    forgotten.fake.terms.push("oso".into());
    let _ = forgotten.run(&none, DAY, &|| false);
    assert_eq!(forgotten.list.entries.len(), 1);
    forgotten.fake.terms.push("React".into());
    let _ = forgotten.run(&none, DAY, &|| false);
    assert!(forgotten.list.is_empty());
    assert_eq!(forgotten.fake.terms, ["oso"]);
    // It arrived, or on a later day did not.
    let mut arrived = Ground {
        list: ground.list.clone(),
        ..Ground::default()
    };
    arrived.fake.terms.push("React".into());
    let _ = arrived.run(&none, DAY, &|| false);
    assert!(!arrived.list.entries[0].unsure);
    let out = ground.pass(&none, DAY + 1);
    assert!(out.done.is_empty() && ground.list.is_empty());
}

#[test]
fn a_change_never_sent_or_refused_is_not_listed() {
    let one = learned(&[("リアクト", "React")]);
    for (answer, again) in [(Err(Failure::NotRunning), 2), (Ok(400), 1)] {
        let mut ground = Ground::default();
        ground.fake.answers = vec![answer];
        let _ = ground.pass(&one, DAY);
        assert!(ground.list.entries.is_empty());
        let _ = ground.pass(&one, DAY);
        assert_eq!(ground.fake.changes.len(), again);
    }
}

#[test]
fn a_removal_that_failed_is_made_by_a_later_pass_and_a_refused_one_holds_up_no_other() {
    let mut ground = Ground::with(&both());
    ground.list.forget();
    ground.fake.answers = vec![Err(Failure::NoAnswer)];
    let out = ground.run(&Record::default(), DAY, &|| false);
    assert!(out.stuck);
    assert_eq!(ground.list.entries.len(), 2);
    ground.fake.answers = vec![Ok(400)];
    let out = ground.run(&Record::default(), DAY, &|| false);
    assert!(out.stuck && ground.list.entries.len() == 1);
    let out = ground.run(&Record::default(), DAY, &|| false);
    assert!(!out.stuck && ground.list.is_empty() && ground.fake.terms.is_empty());

    // One that had arrived, though its answer was lost, is not added again at once.
    let mut ground = Ground::with(&both());
    ground.list.entries[0].state = State::Removing;
    ground.fake.terms.clear();
    let out = ground.pass(&both(), DAY);
    assert_eq!(whats(&out), [(What::Removed, "リアクト")]);
    assert!(ground.fake.terms.is_empty());
}

#[test]
fn a_stop_starts_no_change_and_lists_nothing_unasked() {
    let mut ground = Ground::default();
    let out = ground.run(&both(), DAY, &|| true);
    assert!(out.done.is_empty() && ground.list.is_empty());
    assert!(ground.fake.changes.is_empty());
    // Between two: the first is made.
    let asked = std::cell::Cell::new(0);
    let _ = ground.run(&both(), DAY, &|| {
        asked.set(asked.get() + 1);
        asked.get() > 2
    });
    assert_eq!(ground.fake.changes.len(), 1);
    assert_eq!(ground.list.entries.len(), 1);
}

#[test]
fn a_list_that_cannot_be_kept_asks_for_no_change() {
    let mut ground = Ground::default();
    let file = br#"{"port": 8978, "token": "abcdefghijklmnopqrstuvwxyz012345", "version": 1}"#;
    let out = Pass {
        transport: &mut ground.fake,
        discovery: &discovery::parse(file).unwrap(),
        server: "here",
        today: DAY,
        blocked: &mut ground.blocked,
        keep: &mut |_| false,
        stop: &|| false,
    }
    .run(&mut ground.list, &both());
    assert_eq!(out, Outcome::default());
    assert!(ground.fake.changes.is_empty());
}

#[test]
fn another_typewhispers_entries_are_left_alone() {
    let mut ground = Ground::with(&learned(&[("リアクト", "React")]));
    ground.list.entries[0].server = "elsewhere".into();
    ground.fake.terms.clear();
    let before = ground.list.clone();
    let none = Record::default();
    assert!(!wanted(
        &ground.list,
        &none,
        "here",
        DAY + 99,
        &Blocked::new()
    ));
    let _ = ground.run(&none, DAY + 99, &|| false);
    assert!(ground.list == before && ground.fake.changes.is_empty());
}

#[test]
fn a_crafted_dictionary_costs_little_or_is_not_read() {
    let mut ground = Ground::with(&both());
    ground.fake.terms = vec!["a".into(); 20_000];
    assert!(ground.run(&both(), DAY, &|| false).stuck, "too many");
    ground.fake.terms = vec!["a".repeat(2000)];
    assert!(ground.run(&both(), DAY, &|| false).stuck, "too long");
    let words: Vec<String> = (0..MAX_ENTRIES)
        .map(|n| format!("{}{n:04}", "x".repeat(36)))
        .collect();
    let pairs: Vec<(&str, &str)> = (words.iter()).map(|word| (word.as_str(), "y")).collect();
    let mut ground = Ground::with(&learned(&pairs));
    ground
        .fake
        .corrections
        .extend((0..9_900).map(|n| (format!("{}{n:06}", "x".repeat(34)), "z".repeat(900), false)));
    let began = Instant::now();
    let _ = ground.run(&learned(&pairs), DAY, &|| false);
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "{:?}",
        began.elapsed()
    );
}
