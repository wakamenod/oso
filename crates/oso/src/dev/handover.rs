//! The order of a restart: export, start, import, and only then stop the old
//! instance (ADR 0023, ADR 0045).
//!
//! Must not depend on: `supervisor`, `ctl`, sockets or processes; the supervisor
//! supplies them behind [`Instance`], so that the order can be tested without
//! any of them.

use serde_json::{Value, json};

/// How a handover ended.
#[derive(Debug)]
pub enum Handover<I> {
    /// The new instance has the state, and the old one was stopped.
    Replaced(I),
    /// The new instance runs, without the state. The old one is untouched.
    Refused { new: I, reason: String },
    /// There is no new instance. The old one is untouched.
    Failed(String),
}

/// What a handover needs from an instance.
pub trait Instance {
    /// Makes one control socket request.
    fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, String>;
    /// Waits for its window to exist.
    fn wait_until_ready(&mut self) -> Result<(), String>;
    /// Asks it to exit, and makes sure it does.
    fn stop(&mut self);
}

/// Hands `old`'s state to a new instance. The old instance is stopped only
/// after the new one has confirmed the import.
pub fn hand_over<I: Instance>(
    old: &mut I,
    start: impl FnOnce() -> Result<I, String>,
) -> Handover<I> {
    let state = match old.call("export-state", None) {
        Ok(state) => state,
        Err(reason) => return Handover::Failed(format!("export failed: {reason}")),
    };
    let mut new = match start() {
        Ok(new) => new,
        Err(reason) => return Handover::Failed(format!("the new build did not start: {reason}")),
    };
    if let Err(reason) = new.wait_until_ready() {
        new.stop();
        return Handover::Failed(format!("the new build did not start: {reason}"));
    }
    match new.call("import-state", Some(json!({ "state": state }))) {
        Ok(_) => {
            old.stop();
            Handover::Replaced(new)
        }
        Err(reason) => Handover::Refused { new, reason },
    }
}

#[cfg(test)]
mod tests {
    use super::{Handover, Instance, hand_over};
    use serde_json::{Value, json};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// What the fake instances were asked to do, in order.
    type Log = Rc<RefCell<Vec<String>>>;

    struct Fake {
        name: &'static str,
        log: Log,
        ready: bool,
        import: Result<Value, String>,
        export: Result<Value, String>,
    }

    impl Fake {
        fn new(name: &'static str, log: &Log) -> Self {
            Self {
                name,
                log: Rc::clone(log),
                ready: true,
                import: Ok(json!({})),
                export: Ok(json!({ "version": 1 })),
            }
        }

        fn note(&self, what: &str) {
            self.log.borrow_mut().push(format!("{} {what}", self.name));
        }
    }

    impl Instance for Fake {
        fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
            self.note(method);
            match method {
                "export-state" => self.export.clone(),
                "import-state" => {
                    assert_eq!(params.unwrap()["state"], json!({ "version": 1 }));
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

        fn stop(&mut self) {
            self.note("stop");
        }
    }

    fn log() -> Log {
        Rc::new(RefCell::new(Vec::new()))
    }

    #[test]
    fn the_old_instance_stops_only_after_the_import_is_confirmed() {
        let log = log();
        let mut old = Fake::new("old", &log);
        let new = Fake::new("new", &log);
        assert!(matches!(
            hand_over(&mut old, || Ok(new)),
            Handover::Replaced(_)
        ));
        assert_eq!(
            *log.borrow(),
            [
                "old export-state",
                "new ready?",
                "new import-state",
                "old stop"
            ]
        );
    }

    #[test]
    fn a_refused_import_keeps_both() {
        let log = log();
        let mut old = Fake::new("old", &log);
        let mut new = Fake::new("new", &log);
        new.import = Err("the state has version 1".into());
        let outcome = hand_over(&mut old, || Ok(new));
        let Handover::Refused { reason, .. } = outcome else {
            panic!("{}", "not refused");
        };
        assert!(reason.contains("version"));
        assert!(!log.borrow().iter().any(|line| line.ends_with("stop")));
    }

    #[test]
    fn a_new_instance_that_does_not_start_is_stopped_and_the_old_one_kept() {
        let log = log();
        let mut old = Fake::new("old", &log);
        let mut new = Fake::new("new", &log);
        new.ready = false;
        assert!(matches!(
            hand_over(&mut old, || Ok(new)),
            Handover::Failed(_)
        ));
        assert_eq!(
            *log.borrow(),
            ["old export-state", "new ready?", "new stop"]
        );

        let log = self::log();
        let mut old = Fake::new("old", &log);
        assert!(matches!(
            hand_over::<Fake>(&mut old, || Err("no binary".into())),
            Handover::Failed(_)
        ));
        assert_eq!(*log.borrow(), ["old export-state"]);
    }

    #[test]
    fn a_failed_export_starts_nothing() {
        let log = log();
        let mut old = Fake::new("old", &log);
        old.export = Err("a buffer has text and no file".into());
        let outcome = hand_over::<Fake>(&mut old, || panic!("started"));
        assert!(matches!(outcome, Handover::Failed(_)));
        assert_eq!(*log.borrow(), ["old export-state"]);
    }
}
