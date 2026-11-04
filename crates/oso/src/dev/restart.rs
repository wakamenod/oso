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
pub const CHECK: &str = "check-handover";

/// The instances a supervisor looks after.
#[derive(Debug)]
pub struct Instances<I> {
    /// The one that hands over at the next restart.
    pub current: Option<I>,
    /// A new one that stays beside it, and holds the next restart if it has to stay.
    pub beside: Option<I>,
    /// New ones that got no window in time and did not answer. Each may yet show
    /// one and be typed into, so they are kept track of, but they hold nothing up.
    pub strays: Vec<I>,
}

impl<I> Instances<I> {
    pub fn new(current: I) -> Self {
        Self {
            current: Some(current),
            beside: None,
            strays: Vec::new(),
        }
    }
}

/// How a handover ended.
#[derive(Debug)]
pub enum Handover<I> {
    /// The new instance has the state, and the old one was stopped.
    Replaced(I),
    /// A new instance runs beside the old one, which has not been replaced.
    Beside { new: I, reason: String },
    /// A new instance got no window, and did not answer when asked to go.
    NoWindow { new: I, reason: String },
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
    /// [`CHECK`], giving up soon, as this is for one that may never answer.
    fn check_briefly(&mut self) -> Result<Value, String>;
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
    let answer = instance.call(CHECK, None)?;
    let generation = generation_in(&answer)?;
    instance.stop(generation).map_err(NotStopped::into_reason)
}

/// [`close`], for an instance that may never answer.
fn close_briefly<I: Instance>(instance: &mut I) -> Result<(), String> {
    if instance.gone() {
        return Ok(());
    }
    let answer = instance.check_briefly()?;
    let generation = generation_in(&answer)?;
    instance.stop(generation).map_err(NotStopped::into_reason)
}

/// Gives up on a new instance that got no window.
fn no_window<I: Instance>(mut new: I, reason: String) -> Handover<I> {
    let reason = format!("the new build did not start: {reason}");
    match close_briefly(&mut new) {
        Ok(()) => Handover::Failed(reason),
        Err(why) => Handover::NoWindow {
            new,
            reason: format!("{reason}: {why}"),
        },
    }
}

/// Tries again to close the instances that got no window, whatever they are up to
/// by now, keeping any that has been edited or still does not answer.
fn sweep<I: Instance>(strays: &mut Vec<I>) {
    strays.retain_mut(|stray| match close_briefly(stray) {
        Ok(()) => false,
        Err(why) => {
            log::warn!(
                "oso dev: instance {} stays, and is yours to close: {why:?}",
                stray.pid()
            );
            true
        }
    });
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
    let started = Instant::now();
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
    let checked = started.elapsed();
    let mut new = match start() {
        Ok(new) => new,
        Err(reason) => return Handover::Failed(format!("the new build did not start: {reason}")),
    };
    if let Err(reason) = new.wait_until_ready() {
        return no_window(new, reason);
    }
    let ready = started.elapsed();
    let (state, generation) = match old
        .call("export-state", None)
        .and_then(|state| generation_in(&state).map(|generation| (state, generation)))
    {
        Ok(exported) => exported,
        // Gone since the check, so the new one is all there is.
        Err(reason) if old.gone() => {
            return Handover::Beside {
                new,
                reason: format!("the old instance has gone: {reason}"),
            };
        }
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
                "oso dev: checked {checked:?}, new window {:?}, export {:?}, import {:?}, \
                 old instance gone {:?}",
                ready - checked,
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

/// Moves on to a new binary, keeping what the current instance holds if there is
/// one. Returns whether the new binary is now the current instance, so that the
/// next build knows to try again if not. Reasons are logged escaped, since they can
/// quote a file name and this is a terminal.
pub fn restart<I, S>(instances: &mut Instances<I>, start: S) -> bool
where
    I: Instance,
    S: FnOnce() -> Result<I, String>,
{
    sweep(&mut instances.strays);
    let Some(old) = instances.current.as_mut() else {
        return start_first(instances, start);
    };
    match hand_over(old, &mut instances.beside, start) {
        Handover::Replaced(new) => {
            instances.current = Some(new);
            true
        }
        Handover::Beside { new, reason } => {
            log::warn!(
                "oso dev: new instance {} runs beside the old one: {reason:?}",
                new.pid()
            );
            instances.beside = Some(new);
            false
        }
        Handover::NoWindow { new, reason } => {
            keep_stray(&mut instances.strays, new, &reason);
            false
        }
        Handover::Failed(reason) if old.gone() => {
            log::warn!("oso dev: the old instance has gone: {reason:?}");
            false
        }
        Handover::Failed(reason) => {
            log::warn!("oso dev: the old instance stays: {reason:?}");
            false
        }
    }
}

fn keep_stray<I: Instance>(strays: &mut Vec<I>, new: I, reason: &str) {
    log::warn!(
        "oso dev: instance {} stays until it answers or is ended by hand: {reason:?}",
        new.pid()
    );
    strays.push(new);
}

/// Starts an instance with nothing to take over from.
fn start_first<I, S>(instances: &mut Instances<I>, start: S) -> bool
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
    if let Err(reason) = new.wait_until_ready() {
        match no_window(new, reason) {
            Handover::NoWindow { new, reason } => {
                keep_stray(&mut instances.strays, new, &reason);
            }
            Handover::Failed(reason) => log::warn!("oso dev: {reason:?}"),
            Handover::Replaced(_) | Handover::Beside { .. } => {}
        }
        return false;
    }
    instances.current = Some(new);
    true
}

#[cfg(test)]
mod tests {
    use super::{Handover, Instance, Instances, NotStopped, hand_over, restart};
    use serde_json::{Value, json};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// What the fake instances were asked to do, in order.
    type Log = Rc<RefCell<Vec<String>>>;

    /// The generation a fake's `check-handover` reports, and its export's, which
    /// tell apart which one reached `stop`.
    const CHECKED: u64 = 7;
    const EXPORTED: u64 = 8;

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
                check: Ok(json!({ "generation": CHECKED })),
                import: Ok(json!({})),
                export: Ok(json!({ "version": 1, "generation": EXPORTED })),
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

        fn check_briefly(&mut self) -> Result<Value, String> {
            self.note("check-briefly");
            self.check.clone()
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
            self.note(&format!("stop {generation}"));
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
                "old stop 8"
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
            ["old stop 8", "new check-handover", "new stop 7"],
            "the new one goes only once the old one has refused, and if it agrees"
        );
    }

    #[test]
    fn a_change_after_the_export_with_input_in_the_new_one_keeps_both() {
        let log = log();
        let mut old = Fake::refusing("old", &log);
        // The new one takes input once it has imported, as its window is up.
        let new = Fake::edited("new", &log);
        let Handover::Beside { reason, .. } = handed_over(&mut old, new) else {
            panic!("{}", "an instance holding text was closed");
        };
        assert!(reason.contains("exit refused"), "{reason}");
        assert!(reason.contains("unsaved edits"), "{reason}");
        assert_eq!(
            log.borrow().iter().filter(|l| l.contains(" stop ")).count(),
            1,
            "only the old one was asked, and it refused"
        );
    }

    #[test]
    fn the_old_instance_is_held_to_the_export_and_others_to_what_they_just_reported() {
        let log = log();
        let mut old = Fake::refusing("old", &log);
        let new = Fake::new("new", &log);
        handed_over(&mut old, new);
        let log = log.borrow();
        assert!(log.contains(&format!("old stop {EXPORTED}")), "{log:?}");
        assert!(log.contains(&format!("new stop {CHECKED}")), "{log:?}");
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
            log.borrow()
                .iter()
                .filter(|l| l.starts_with("new stop"))
                .count(),
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
        assert!(!log.borrow().iter().any(|line| line.contains(" stop ")));
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
                "new check-briefly",
                "new stop 7"
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
                "new stop 7"
            ]
        );
    }

    #[test]
    fn a_failed_export_keeps_the_new_instance_if_the_old_one_has_gone() {
        let log = log();
        let mut old = Fake {
            export: Err("cannot connect".into()),
            exited: true,
            ..Fake::new("old", &log)
        };
        let new = Fake::new("new", &log);
        let Handover::Beside { reason, .. } = handed_over(&mut old, new) else {
            panic!("{}", "the only instance left was closed");
        };
        assert!(reason.contains("has gone"), "{reason}");
        assert!(!log.borrow().iter().any(|line| line.contains(" stop ")));
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
        assert!(!log.borrow().iter().any(|line| line.contains(" stop ")));
    }

    #[test]
    fn an_export_without_a_generation_stops_nothing() {
        let log = log();
        let mut old = Fake::new("old", &log);
        old.export = Ok(json!({ "version": 1 }));
        let new = Fake::new("new", &log);
        assert!(matches!(handed_over(&mut old, new), Handover::Failed(_)));
        assert!(
            !log.borrow().iter().any(|line| line.starts_with("old stop")),
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
        assert!(!log.borrow().iter().any(|line| line.contains(" stop ")));
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
                "beside stop 7",
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

    /// The instances with `current` as the current one.
    fn with_current(current: Fake) -> Instances<Fake> {
        Instances::new(current)
    }

    fn nothing_running() -> Instances<Fake> {
        Instances {
            current: None,
            beside: None,
            strays: Vec::new(),
        }
    }

    /// A new instance that gets no window and does not answer, as when a bad build
    /// loops before its first frame.
    fn wedged(log: &Log) -> Fake {
        Fake {
            ready: false,
            check: Err("no answer".into()),
            ..Fake::new("wedged", log)
        }
    }

    #[test]
    fn restart_keeps_every_instance_that_runs() {
        let log = log();

        // Replaced: the new one takes over.
        let mut instances = with_current(Fake::new("old", &log));
        let new = Fake::new("new", &log);
        assert!(restart(&mut instances, || Ok(new)));
        assert_eq!(instances.current.as_ref().map(|i| i.name), Some("new"));

        // Refused: both stay, the old one is still the current one, and the next
        // build tries again.
        let mut instances = with_current(Fake::new("old", &log));
        let mut new = Fake::new("new", &log);
        new.import = Err("no".into());
        assert!(!restart(&mut instances, || Ok(new)));
        assert_eq!(instances.current.as_ref().map(|i| i.name), Some("old"));
        assert_eq!(instances.beside.as_ref().map(|i| i.name), Some("new"));

        // Held: nothing starts, and the next build tries again.
        let mut instances = with_current(Fake::edited("old", &log));
        assert!(!restart(&mut instances, || -> Result<Fake, String> {
            panic!("a new window opened")
        }));
        assert_eq!(instances.current.as_ref().map(|i| i.name), Some("old"));
        assert!(instances.beside.is_none());
    }

    #[test]
    fn with_no_instance_a_new_one_just_starts() {
        let log = log();
        let mut instances = nothing_running();
        let new = Fake::new("new", &log);
        assert!(restart(&mut instances, || Ok(new)));
        assert_eq!(instances.current.as_ref().map(|i| i.name), Some("new"));
        assert_eq!(*log.borrow(), ["new ready?"]);
    }

    #[test]
    fn a_new_instance_with_no_window_is_kept_track_of_and_holds_nothing() {
        let log = log();

        let mut instances = nothing_running();
        assert!(!restart(&mut instances, || Ok(wedged(&log))));
        assert!(instances.current.is_none(), "the next build starts another");
        assert_eq!(instances.strays.len(), 1);

        let mut instances = with_current(Fake::new("old", &log));
        assert!(!restart(&mut instances, || Ok(wedged(&log))));
        assert!(instances.beside.is_none(), "the next build is not held");
        assert_eq!(instances.strays.len(), 1);

        log.borrow_mut().clear();
        let new = Fake::new("new", &log);
        assert!(restart(&mut instances, || Ok(new)), "and it is not");
        assert_eq!(instances.current.as_ref().map(|i| i.name), Some("new"));
        assert_eq!(instances.strays.len(), 1, "it is still looked after");
        assert_eq!(log.borrow()[0], "wedged check-briefly", "asked first");
        assert!(
            !log.borrow().iter().any(|l| l.starts_with("wedged stop")),
            "and not stopped, let alone killed"
        );
    }

    #[test]
    fn a_stray_goes_once_it_answers_and_stays_if_it_has_been_edited() {
        let log = log();
        let mut instances = with_current(Fake::new("old", &log));

        // It got its window in the end, with nothing typed in.
        instances.strays.push(Fake::new("late", &log));
        // It got its window, and someone typed in it.
        instances.strays.push(Fake::edited("typed-in", &log));
        // It has ended.
        instances.strays.push(Fake {
            exited: true,
            ..Fake::new("ended", &log)
        });

        let new = Fake::new("new", &log);
        assert!(restart(&mut instances, || Ok(new)), "none of them holds it");
        let left: Vec<_> = instances.strays.iter().map(|i| i.name).collect();
        assert_eq!(left, ["typed-in"]);
        let log = log.borrow();
        assert!(log.contains(&"late stop 7".to_owned()), "{log:?}");
        assert!(
            !log.iter().any(|l| l.starts_with("typed-in stop")),
            "{log:?}"
        );
        assert!(!log.iter().any(|l| l.starts_with("ended")), "{log:?}");
    }
}
