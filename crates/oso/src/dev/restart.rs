//! The order of a restart: ask the old instance, start the new one, then export,
//! import, and only then stop the old one (ADR 0023, ADR 0047, ADR 0055). The
//! export comes as late as possible, so that input the old window takes while
//! the new one starts is not lost.
//!
//! Must not depend on: `supervisor`, `ctl`, sockets or processes; the supervisor
//! supplies them behind [`Instance`], so that the order can be tested without
//! any of them.
//!
//! An instance goes only when it says so itself: it answers on its UI thread,
//! with the text it holds at that moment. Nothing here can end an instance that
//! has not agreed, since [`Instance`] has no way to.

use std::time::Instant;

use serde_json::{Value, json};

/// The method that reports an instance's generation, or why its text would not
/// survive a restart.
const CHECK: &str = "check-handover";

/// How a handover ended.
#[derive(Debug)]
pub enum Handover<I> {
    /// The new instance has the state, and the old one was stopped.
    Replaced(I),
    /// A new instance runs beside the old one, which is untouched.
    Beside { new: I, reason: String },
    /// There is no new instance. The old one is untouched.
    Failed(String),
}

/// Why an instance is still there after being asked to exit.
#[derive(Debug, PartialEq, Eq)]
pub enum NotStopped {
    /// It refused, as its text has changed, so it surely runs on.
    Refused(String),
    /// It did not answer, or did not exit. It may yet.
    Unknown(String),
}

impl NotStopped {
    fn into_reason(self) -> String {
        match self {
            Self::Refused(reason) | Self::Unknown(reason) => reason,
        }
    }
}

/// What a handover needs from an instance.
pub trait Instance {
    /// Its process ID, for the log.
    fn pid(&self) -> u32;
    /// Makes one control socket request.
    fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, String>;
    /// Waits for its window to exist.
    fn wait_until_ready(&mut self) -> Result<(), String>;
    /// Whether it has exited.
    fn gone(&mut self) -> bool;
    /// Asks it to exit if its generation is still `generation`, and waits for
    /// it to. Never forces it.
    fn stop(&mut self, generation: u64) -> Result<(), NotStopped>;
}

/// The generation in a result of `check-handover` or `export-state`.
fn generation_in(result: &Value) -> Result<u64, String> {
    result["generation"]
        .as_u64()
        .ok_or_else(|| "the instance reported no generation".to_owned())
}

/// Closes an instance if it holds nothing unsaved, or returns why it stays. The
/// generation closes the gap between asking and exiting, in which it can be edited.
fn close<I: Instance>(instance: &mut I) -> Result<(), String> {
    if instance.gone() {
        return Ok(());
    }
    let generation = generation_in(&instance.call(CHECK, None)?)?;
    instance.stop(generation).map_err(NotStopped::into_reason)
}

/// Gives up on a new instance, which the user may have typed into by now, so it
/// stays if it has been edited.
fn give_up<I: Instance>(mut new: I, reason: String) -> Handover<I> {
    match close(&mut new) {
        Ok(()) => Handover::Failed(reason),
        Err(why) => Handover::Beside {
            new,
            reason: format!("{reason}, and it stays: {why}"),
        },
    }
}

/// Hands `old`'s state to a new instance. Nothing starts unless `old` can hand
/// over and `beside`, left by an earlier try, can be closed. The old instance is
/// stopped only after the new one has confirmed the import.
///
/// If the old instance has changed since the export, it stays and the new one
/// goes, unless that one has been edited too: then both stay, each with its own
/// text, for the user to sort out.
pub fn hand_over<I: Instance>(
    old: &mut I,
    beside: &mut Option<I>,
    start: impl FnOnce() -> Result<I, String>,
) -> Handover<I> {
    if let Err(reason) = old.call(CHECK, None) {
        return Handover::Failed(reason);
    }
    if let Some(instance) = beside {
        if let Err(why) = close(instance) {
            return Handover::Failed(format!(
                "instance {} beside it comes first: {why}",
                instance.pid()
            ));
        }
        *beside = None;
    }
    let started = Instant::now();
    let mut new = match start() {
        Ok(new) => new,
        Err(reason) => return Handover::Failed(format!("the new build did not start: {reason}")),
    };
    if let Err(reason) = new.wait_until_ready() {
        return give_up(new, format!("the new build did not start: {reason}"));
    }
    let ready = started.elapsed();
    let (state, generation) = match old
        .call("export-state", None)
        .and_then(|state| generation_in(&state).map(|generation| (state, generation)))
    {
        Ok(exported) => exported,
        Err(reason) => return give_up(new, format!("export failed: {reason}")),
    };
    let exported = started.elapsed();
    if let Err(reason) = new.call("import-state", Some(json!({ "state": state }))) {
        return Handover::Beside {
            new,
            reason: format!("it refused the state: {reason}"),
        };
    }
    let imported = started.elapsed();
    match old.stop(generation) {
        Ok(()) => {
            log::info!(
                "oso dev: new window {ready:?}, export {:?}, import {:?}, old instance gone {:?}",
                exported - ready,
                imported - exported,
                started.elapsed() - imported,
            );
            Handover::Replaced(new)
        }
        Err(NotStopped::Refused(reason)) => give_up(new, reason),
        // It may have agreed and be on its way out, and then the new one must be
        // there. Whichever goes first, the other is what remains.
        Err(NotStopped::Unknown(reason)) => Handover::Beside { new, reason },
    }
}

/// Moves on to a new binary, keeping what `current` holds if there is one.
/// Returns whether a new instance runs. Reasons are logged escaped, since they
/// can quote a file name and this is a terminal.
pub fn restart<I, S>(current: &mut Option<I>, beside: &mut Option<I>, start: S) -> bool
where
    I: Instance,
    S: FnOnce() -> Result<I, String>,
{
    let Some(old) = current.as_mut() else {
        return start_first(current, start);
    };
    match hand_over(old, beside, start) {
        Handover::Replaced(new) => {
            *current = Some(new);
            true
        }
        Handover::Beside { new, reason } => {
            log::warn!(
                "oso dev: new instance {} runs beside the old one, which stays: {reason:?}",
                new.pid()
            );
            *beside = Some(new);
            true
        }
        Handover::Failed(reason) => {
            log::warn!("oso dev: the old instance stays: {reason:?}");
            false
        }
    }
}

/// Starts an instance with nothing to take over from. One that does not get a
/// window and cannot be closed is still kept, so that its exit is noticed.
fn start_first<I, S>(current: &mut Option<I>, start: S) -> bool
where
    I: Instance,
    S: FnOnce() -> Result<I, String>,
{
    let mut new = match start() {
        Ok(new) => new,
        Err(reason) => {
            log::warn!("oso dev: the new build did not start: {reason:?}");
            return false;
        }
    };
    let ready = new.wait_until_ready();
    if let Err(reason) = &ready {
        log::warn!("oso dev: the new build did not start: {reason:?}");
        let Err(why) = close(&mut new) else {
            return false;
        };
        log::warn!("oso dev: instance {} stays: {why:?}", new.pid());
    }
    *current = Some(new);
    ready.is_ok()
}

#[cfg(test)]
mod tests {
    use super::{Handover, Instance, NotStopped, hand_over, restart};
    use serde_json::{Value, json};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// What the fake instances were asked to do, in order.
    type Log = Rc<RefCell<Vec<String>>>;

    /// The generation every fake reports.
    const GENERATION: u64 = 7;

    struct Fake {
        name: &'static str,
        log: Log,
        ready: bool,
        exited: bool,
        check: Result<Value, String>,
        import: Result<Value, String>,
        export: Result<Value, String>,
        stop: Result<(), NotStopped>,
    }

    impl Fake {
        fn new(name: &'static str, log: &Log) -> Self {
            Self {
                name,
                log: Rc::clone(log),
                ready: true,
                exited: false,
                check: Ok(json!({ "generation": GENERATION })),
                import: Ok(json!({})),
                export: Ok(json!({ "version": 1, "generation": GENERATION })),
                stop: Ok(()),
            }
        }

        /// An instance with unsaved edits, and no way to hand over.
        fn edited(name: &'static str, log: &Log) -> Self {
            Self {
                check: Err("cannot hand over notes: it has unsaved edits".into()),
                ..Self::new(name, log)
            }
        }

        /// An instance whose text changed after its generation was read.
        fn refusing(name: &'static str, log: &Log) -> Self {
            Self {
                stop: Err(NotStopped::Refused("exit refused".into())),
                ..Self::new(name, log)
            }
        }

        fn note(&self, what: &str) {
            self.log.borrow_mut().push(format!("{} {what}", self.name));
        }
    }

    impl Instance for Fake {
        fn pid(&self) -> u32 {
            1
        }

        fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
            self.note(method);
            match method {
                "check-handover" => self.check.clone(),
                "export-state" => self.export.clone(),
                "import-state" => {
                    assert_eq!(params.unwrap()["state"]["version"], 1);
                    self.import.clone()
                }
                other => panic!("{other}"),
            }
        }

        fn wait_until_ready(&mut self) -> Result<(), String> {
            self.note("ready?");
            if self.ready {
                Ok(())
            } else {
                Err("no window".into())
            }
        }

        fn gone(&mut self) -> bool {
            self.exited
        }

        fn stop(&mut self, generation: u64) -> Result<(), NotStopped> {
            assert_eq!(generation, GENERATION, "the generation it reported");
            self.note("stop");
            match &self.stop {
                Ok(()) => Ok(()),
                Err(NotStopped::Refused(reason)) => Err(NotStopped::Refused(reason.clone())),
                Err(NotStopped::Unknown(reason)) => Err(NotStopped::Unknown(reason.clone())),
            }
        }
    }

    fn log() -> Log {
        Rc::new(RefCell::new(Vec::new()))
    }

    /// Hands over with no instance beside the old one.
    fn handed_over(old: &mut Fake, new: Fake) -> Handover<Fake> {
        hand_over(old, &mut None, || Ok(new))
    }

    #[test]
    fn the_old_instance_stops_only_after_the_import_is_confirmed() {
        let log = log();
        let mut old = Fake::new("old", &log);
        let new = Fake::new("new", &log);
        assert!(matches!(handed_over(&mut old, new), Handover::Replaced(_)));
        assert_eq!(
            *log.borrow(),
            [
                "old check-handover",
                "new ready?",
                "old export-state",
                "new import-state",
                "old stop"
            ]
        );
    }

    #[test]
    fn a_modified_old_instance_starts_no_new_binary() {
        let log = log();
        let mut old = Fake::edited("old", &log);
        let outcome = hand_over::<Fake>(&mut old, &mut None, || panic!("a new window opened"));
        let Handover::Failed(reason) = outcome else {
            panic!("{}", "a new instance runs");
        };
        assert!(reason.contains("notes"), "{reason}");
        assert_eq!(*log.borrow(), ["old check-handover"], "nothing else");
    }

    #[test]
    fn a_change_after_the_export_keeps_the_old_one_and_closes_the_new_one() {
        let log = log();
        let mut old = Fake::refusing("old", &log);
        let new = Fake::new("new", &log);
        let outcome = handed_over(&mut old, new);
        let Handover::Failed(reason) = outcome else {
            panic!("{}", "the old instance was replaced");
        };
        assert!(reason.contains("exit refused"), "{reason}");
        assert_eq!(
            log.borrow()[4..],
            ["old stop", "new check-handover", "new stop"],
            "the new one goes only once the old one has refused, and if it agrees"
        );
    }

    #[test]
    fn a_change_after_the_export_with_input_in_the_new_one_keeps_both() {
        let log = log();
        let mut old = Fake::refusing("old", &log);
        let new = Fake {
            check: Ok(json!({ "generation": GENERATION })),
            ..Fake::new("new", &log)
        };
        // The new one takes input once it has imported, as its window is up.
        let edited = Fake {
            check: Err("cannot hand over notes: it has unsaved edits".into()),
            ..new
        };
        let Handover::Beside { reason, .. } = handed_over(&mut old, edited) else {
            panic!("{}", "an instance holding text was closed");
        };
        assert!(reason.contains("exit refused"), "{reason}");
        assert!(reason.contains("unsaved edits"), "{reason}");
        assert_eq!(
            log.borrow().iter().filter(|l| l.ends_with("stop")).count(),
            1,
            "only the old one was asked, and it refused"
        );
    }

    #[test]
    fn a_new_instance_edited_as_it_is_closed_stays() {
        let log = log();
        let mut old = Fake::refusing("old", &log);
        // It agreed, then was edited before the exit arrived.
        let new = Fake::refusing("new", &log);
        assert!(matches!(
            handed_over(&mut old, new),
            Handover::Beside { .. }
        ));
    }

    #[test]
    fn an_old_instance_that_does_not_answer_keeps_both() {
        let log = log();
        let mut old = Fake {
            stop: Err(NotStopped::Unknown("it did not exit".into())),
            ..Fake::new("old", &log)
        };
        let new = Fake::new("new", &log);
        assert!(matches!(
            handed_over(&mut old, new),
            Handover::Beside { .. }
        ));
        assert_eq!(
            log.borrow().iter().filter(|l| *l == "new stop").count(),
            0,
            "it may be on its way out, so the new one must stay"
        );
    }

    #[test]
    fn a_refused_import_keeps_both() {
        let log = log();
        let mut old = Fake::new("old", &log);
        let mut new = Fake::new("new", &log);
        new.import = Err("the state has version 1".into());
        let outcome = handed_over(&mut old, new);
        let Handover::Beside { reason, .. } = outcome else {
            panic!("{}", "not refused");
        };
        assert!(reason.contains("refused the state"));
        assert!(reason.contains("version"));
        assert!(!log.borrow().iter().any(|line| line.ends_with("stop")));
    }

    #[test]
    fn a_new_instance_that_does_not_start_is_closed_and_the_old_one_kept() {
        let log = log();
        let mut old = Fake::new("old", &log);
        let mut new = Fake::new("new", &log);
        new.ready = false;
        assert!(matches!(handed_over(&mut old, new), Handover::Failed(_)));
        assert_eq!(
            *log.borrow(),
            [
                "old check-handover",
                "new ready?",
                "new check-handover",
                "new stop"
            ],
            "nothing exported"
        );

        let log = self::log();
        let mut old = Fake::new("old", &log);
        assert!(matches!(
            hand_over::<Fake>(&mut old, &mut None, || Err("no binary".into())),
            Handover::Failed(_)
        ));
        assert_eq!(*log.borrow(), ["old check-handover"]);
    }

    #[test]
    fn a_new_instance_that_has_already_exited_is_not_asked_anything() {
        let log = log();
        let mut old = Fake::new("old", &log);
        let new = Fake {
            ready: false,
            exited: true,
            ..Fake::new("new", &log)
        };
        assert!(matches!(handed_over(&mut old, new), Handover::Failed(_)));
        assert_eq!(*log.borrow(), ["old check-handover", "new ready?"]);
    }

    #[test]
    fn a_failed_export_closes_the_new_instance_and_keeps_the_old_one() {
        let log = log();
        let mut old = Fake::new("old", &log);
        old.export = Err("a buffer has text and no file".into());
        let new = Fake::new("new", &log);
        assert!(matches!(handed_over(&mut old, new), Handover::Failed(_)));
        assert_eq!(
            *log.borrow(),
            [
                "old check-handover",
                "new ready?",
                "old export-state",
                "new check-handover",
                "new stop"
            ]
        );
    }

    #[test]
    fn a_failed_export_keeps_a_new_instance_that_has_been_edited() {
        let log = log();
        let mut old = Fake::new("old", &log);
        old.export = Err("a buffer has text and no file".into());
        let new = Fake::edited("new", &log);
        assert!(matches!(
            handed_over(&mut old, new),
            Handover::Beside { .. }
        ));
        assert!(!log.borrow().iter().any(|line| line.ends_with("stop")));
    }

    #[test]
    fn an_export_without_a_generation_stops_nothing() {
        let log = log();
        let mut old = Fake::new("old", &log);
        old.export = Ok(json!({ "version": 1 }));
        let new = Fake::new("new", &log);
        assert!(matches!(handed_over(&mut old, new), Handover::Failed(_)));
        assert!(
            !log.borrow().contains(&"old stop".to_owned()),
            "{:?}",
            log.borrow()
        );
    }

    #[test]
    fn an_edited_instance_beside_the_old_one_stays_and_holds_the_restart() {
        let log = log();
        let mut old = Fake::new("old", &log);
        let mut beside = Some(Fake::edited("beside", &log));
        let outcome = hand_over::<Fake>(&mut old, &mut beside, || panic!("a new window opened"));
        let Handover::Failed(reason) = outcome else {
            panic!("{}", "a new instance runs");
        };
        assert!(reason.contains("unsaved edits"), "{reason}");
        assert!(beside.is_some(), "it is still looked after");
        assert!(!log.borrow().iter().any(|line| line.ends_with("stop")));
    }

    #[test]
    fn an_unedited_instance_beside_the_old_one_is_closed_before_the_new_one_starts() {
        let log = log();
        let mut old = Fake::new("old", &log);
        let new = Fake::new("new", &log);
        let mut beside = Some(Fake::new("beside", &log));
        assert!(matches!(
            hand_over(&mut old, &mut beside, || Ok(new)),
            Handover::Replaced(_)
        ));
        assert!(beside.is_none());
        assert_eq!(
            log.borrow()[..4],
            [
                "old check-handover",
                "beside check-handover",
                "beside stop",
                "new ready?"
            ]
        );
    }

    #[test]
    fn an_instance_beside_a_modified_one_is_left_alone() {
        let log = log();
        let mut old = Fake::edited("old", &log);
        let mut beside = Some(Fake::new("beside", &log));
        let outcome = hand_over::<Fake>(&mut old, &mut beside, || panic!("a new window opened"));
        assert!(matches!(outcome, Handover::Failed(_)));
        assert!(beside.is_some());
        assert_eq!(*log.borrow(), ["old check-handover"]);
    }

    #[test]
    fn restart_keeps_every_instance_that_runs() {
        let log = log();

        // Replaced: the new one takes over.
        let mut current = Some(Fake::new("old", &log));
        let mut beside = None;
        let new = Fake::new("new", &log);
        assert!(restart(&mut current, &mut beside, || Ok(new)));
        assert_eq!(current.as_ref().map(|i| i.name), Some("new"));

        // Refused: both stay, and the old one is still the current one.
        let mut current = Some(Fake::new("old", &log));
        let mut new = Fake::new("new", &log);
        new.import = Err("no".into());
        assert!(restart(&mut current, &mut beside, || Ok(new)));
        assert_eq!(current.as_ref().map(|i| i.name), Some("old"));
        assert_eq!(beside.as_ref().map(|i| i.name), Some("new"));
        beside = None;

        // Held: nothing starts, and the next build tries again.
        let mut current = Some(Fake::edited("old", &log));
        assert!(!restart::<Fake, _>(&mut current, &mut beside, || {
            panic!("a new window opened")
        }));
        assert_eq!(current.as_ref().map(|i| i.name), Some("old"));
        assert!(beside.is_none());
    }

    #[test]
    fn with_no_instance_a_new_one_just_starts() {
        let log = log();
        let (mut current, mut beside) = (None, None);
        let new = Fake::new("new", &log);
        assert!(restart(&mut current, &mut beside, || Ok(new)));
        assert_eq!(current.as_ref().map(|i| i.name), Some("new"));
        assert_eq!(*log.borrow(), ["new ready?"]);
    }

    #[test]
    fn with_no_instance_one_that_gets_no_window_is_closed_or_kept() {
        let log = log();
        let (mut current, mut beside) = (None, None);
        let new = Fake {
            ready: false,
            ..Fake::new("new", &log)
        };
        assert!(!restart(&mut current, &mut beside, || Ok(new)));
        assert!(current.is_none(), "it agreed to go");

        let new = Fake {
            ready: false,
            ..Fake::refusing("new", &log)
        };
        assert!(!restart(&mut current, &mut beside, || Ok(new)));
        assert!(current.is_some(), "it did not, so it is still looked after");
    }
}
