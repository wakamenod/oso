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
//!
//! The texts a state carries go from the old instance to the new one a piece at a
//! time, between the export and the import, and only one piece is held here at once
//! (ADR 0090). Why a restart waits goes to the old instance's echo area.
//!
//! `carry::going` is the same order for `oso-restart`, run by the instance going itself over a
//! socket pair (ADR 0133): a change to one is a change to the other.
//!
//! Running terminals go too (ADR 0120, step 11). After the import, the old instance hands
//! the supervisor a descriptor for each PTY's master, and the `exit` that follows lets the
//! terminals go there; only once it has gone do the masters reach the new instance. So one
//! instance reads each PTY at a time, and a restart that stops short leaves the terminals
//! with the old one, which reads them again.

use std::time::Instant;

use serde_json::{Value, json};

use super::fds::Fd;
use super::protocol::MAX_DEV_MESSAGE as MAX_MESSAGE;

/// The method that reports an instance's generation, or why its text would not
/// survive a restart.
pub const CHECK: &str = "check-handover";

/// How long an old instance whose exit went unanswered may yet take to go, its terminals'
/// masters kept meanwhile: longer than it waits for an exit after handing them over.
const MAY_YET_GO: std::time::Duration = std::time::Duration::from_secs(10);

/// The method that puts a message in an instance's echo area and `*Messages*`.
pub const MESSAGE: &str = "dev-message";

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
    /// It said it exits, but has not yet: it let its terminals go (ADR 0120, step 11).
    Agreed(String),
}

impl NotStopped {
    fn into_reason(self) -> String {
        match self {
            Self::Refused(reason) | Self::Unknown(reason) | Self::Agreed(reason) => reason,
        }
    }
}

/// What a handover needs from an instance.
pub trait Instance {
    /// Its process ID, for the log.
    fn pid(&self) -> u32;
    /// Makes one control socket request.
    fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, String>;
    /// [`Instance::call`], with `fds` sent along, returning what came with the reply.
    fn call_passing(
        &mut self,
        method: &str,
        params: Option<Value>,
        fds: Vec<Fd>,
    ) -> Result<(Value, Vec<Fd>), String>;
    /// [`CHECK`], giving up soon, as this is for one that may never answer.
    fn check_briefly(&mut self) -> Result<Value, String>;
    /// Waits for its window to exist.
    fn wait_until_ready(&mut self) -> Result<(), String>;
    /// Whether it has exited.
    fn gone(&mut self) -> bool;
    /// Whether it exits within `limit`, looking now and then.
    fn gone_within(&mut self, limit: std::time::Duration) -> bool;
    /// Asks it to exit if its generation is still `generation`, and waits for
    /// it to. Never forces it.
    fn stop(&mut self, generation: u64) -> Result<(), NotStopped>;
    /// The generation its import reported, if it took over a state.
    fn imported(&self) -> Option<u64>;
    /// Records the generation its import reported.
    fn set_imported(&mut self, generation: Option<u64>);
}

/// The generation in a result of `check-handover` or `export-state`.
fn generation_in(result: &Value) -> Result<u64, String> {
    result["generation"]
        .as_u64()
        .ok_or_else(|| "the instance reported no generation".to_owned())
}

/// The generation in a result of `check-handover`, if the instance holds nothing a
/// file does not, so that it may be closed. One that holds unsaved text can hand it
/// over, but stays (ADR 0055, ADR 0090). An instance from before that field holds none.
fn closable(answer: &Value) -> Result<u64, String> {
    if answer["unsaved"] == true {
        return Err("it holds unsaved text".into());
    }
    generation_in(answer)
}

/// Closes an instance if it holds nothing unsaved, or returns why it stays. The
/// generation closes the gap between asking and exiting, in which it can be edited.
/// One that took over a state holds only what the instance it was to replace still
/// does, as long as its generation is still the one its import reported.
fn close<I: Instance>(instance: &mut I) -> Result<(), String> {
    if instance.gone() {
        return Ok(());
    }
    let generation = match instance.imported() {
        Some(generation) => generation,
        None => closable(&instance.call(CHECK, None)?)?,
    };
    instance.stop(generation).map_err(NotStopped::into_reason)
}

/// [`close`], for an instance that may never answer. Such an instance took over
/// nothing.
fn close_briefly<I: Instance>(instance: &mut I) -> Result<(), String> {
    if instance.gone() {
        return Ok(());
    }
    let generation = closable(&instance.check_briefly()?)?;
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
    if let Err(reason) = old.call(CHECK, Some(json!({ "terminals": true }))) {
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
    // The terminals go too if the new instance says which library it reads them with.
    let library = new
        .call("version", None)
        .ok()
        .and_then(|version| version["terminal_library"].as_str().map(str::to_owned));
    let params = library.map(|library| json!({ "terminals": { "library": library } }));
    let (state, generation) = match old
        .call("export-state", params)
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
    if let Err(reason) = carry(old, &mut new, &state) {
        if old.gone() {
            return Handover::Beside {
                new,
                reason: format!("the old instance has gone: {reason}"),
            };
        }
        return give_up(new, format!("carrying the text failed: {reason}"));
    }
    let exported = started.elapsed();
    match new.call("import-state", Some(json!({ "state": state }))) {
        Ok(reply) => new.set_imported(reply["generation"].as_u64()),
        Err(reason) => {
            return Handover::Beside {
                new,
                reason: format!("it refused the state: {reason}"),
            };
        }
    }
    let imported = started.elapsed();
    let terminals = state["terminals"].as_array().map_or(0, Vec::len);
    // Native sessions it names resume in the new instance once the old has gone (ADR 0165,
    // part 4c).
    let natives = (state["native_resumes"].as_array()).is_some_and(|named| !named.is_empty());
    let replaced = stop_old(old, new, generation, Carried { terminals, natives });
    if matches!(replaced, Handover::Replaced(_)) {
        log::info!(
            "oso dev: checked {checked:?}, new window {:?}, export {:?}, import {:?}, \
             old instance gone {:?}",
            ready - checked,
            exported - ready,
            imported - exported,
            started.elapsed() - imported,
        );
    }
    replaced
}

/// What the state carried that the new instance takes only once the old has gone: how many
/// terminals, and whether it named native sessions to resume.
#[derive(Debug, Clone, Copy)]
struct Carried {
    terminals: usize,
    natives: bool,
}

impl Carried {
    /// Whether the new instance is to be told that the old has gone, or stays.
    fn any(self) -> bool {
        self.terminals > 0 || self.natives
    }
}

/// Stops `old` once `new` has its state, and gives `new` the masters of the terminals it
/// carried, and word to resume the native sessions it named, only once `old` has gone.
fn stop_old<I: Instance>(
    old: &mut I,
    mut new: I,
    generation: u64,
    carried: Carried,
) -> Handover<I> {
    let masters = match take_masters(old, generation, carried.terminals) {
        Ok(masters) => masters,
        Err(reason) => return give_up_terminals(new, reason, carried),
    };
    match old.stop(generation) {
        Ok(()) => {
            give_masters(&mut new, masters, carried);
            Handover::Replaced(new)
        }
        Err(NotStopped::Refused(reason)) => give_up_terminals(new, reason, carried),
        // It may have agreed and be on its way out, and then the new one must be
        // there. Whichever goes first, the other is what remains. Its terminals are the new
        // one's only once it has gone; until then it may read them again.
        // It agreed, so it let the terminals go: they are the new one's. Its native sessions
        // end as its event loop does, so the new one is told to resume them once it has gone;
        // with terminals, the word goes with their masters whether it has or not, and the
        // start thread's look in Claude Code's registry refuses a conversation still live.
        Err(NotStopped::Agreed(reason)) => {
            let gone = !carried.natives || old.gone_within(MAY_YET_GO);
            if gone || carried.terminals > 0 {
                give_masters(&mut new, masters, carried);
            }
            Handover::Beside { new, reason }
        }
        // Its answer did not come: it may have let the terminals go, and then they are the
        // new one's once it has gone, or it may read them again soon.
        Err(NotStopped::Unknown(reason)) => {
            if carried.any() && old.gone_within(MAY_YET_GO) {
                give_masters(&mut new, masters, carried);
            } else if carried.any() {
                abandon_terminals(&mut new);
            }
            Handover::Beside { new, reason }
        }
    }
}

/// The masters of the `count` terminals the export carries, from `old`, if the generation is
/// still `generation`; none for none.
fn take_masters<I: Instance>(
    old: &mut I,
    generation: u64,
    count: usize,
) -> Result<Vec<Fd>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let params = json!({ "generation": generation });
    let (_, masters) = old.call_passing("hand-over-terminals", Some(params), Vec::new())?;
    if masters.len() != count {
        return Err(format!(
            "the old instance handed over {} masters for {count} terminals",
            masters.len()
        ));
    }
    Ok(masters)
}

/// Gives the new instance the masters of the terminals it took over, now that the old one
/// has gone, and with them word to resume the native sessions it named, with no master if
/// it took no terminal. If it does not take them they close, and the terminals end.
fn give_masters<I: Instance>(new: &mut I, masters: Vec<Fd>, carried: Carried) {
    if masters.is_empty() && !carried.natives {
        return;
    }
    let params = json!({ "count": masters.len() });
    if let Err(reason) = new.call_passing("import-terminals", Some(params), masters) {
        log::warn!("oso dev: the new instance did not take the terminals: {reason:?}");
    }
}

/// Tells the new instance that the terminals it took over stay with the old one.
fn abandon_terminals<I: Instance>(new: &mut I) {
    if let Err(reason) = new.call("import-terminals", Some(json!({ "abandon": true }))) {
        log::debug!("oso dev: the new instance was not told the terminals stay: {reason:?}");
    }
}

/// [`give_up`], after the new instance was told that the terminals it took over stay with
/// the old one, and the native sessions it named with them.
fn give_up_terminals<I: Instance>(mut new: I, reason: String, carried: Carried) -> Handover<I> {
    if carried.any() {
        abandon_terminals(&mut new);
    }
    give_up(new, reason)
}

/// Moves each item `state` names from `old` to `new`, a piece at a time, as `oso-restart`
/// moves them over its pair (`carry::pieces::relay`).
fn carry<I: Instance>(old: &mut I, new: &mut I, state: &Value) -> Result<(), String> {
    crate::carry::pieces::relay(
        state,
        |export, item, offset| {
            let at = json!({ "export": export, "item": item, "offset": offset });
            old.call("export-text", Some(at))
        },
        |piece| new.call("import-text", Some(piece)).map(drop),
    )
}

/// Says `text` in `instance`'s echo area and `*Messages*`, cut to what it takes. An
/// instance from before this method says nothing, which is no reason to stop.
pub fn tell<I: Instance>(instance: &mut I, text: &str) {
    let mut text = text.to_owned();
    if text.len() > MAX_MESSAGE {
        let end = text.floor_char_boundary(MAX_MESSAGE - '…'.len_utf8());
        text.truncate(end);
        text.push('…');
    }
    if let Err(e) = instance.call(MESSAGE, Some(json!({ "text": text }))) {
        log::debug!("oso dev: instance {} was not told: {e}", instance.pid());
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
            if !old.gone() {
                tell(
                    old,
                    &format!("oso dev: the new build runs beside this one: {reason}"),
                );
            }
            instances.beside = Some(new);
            false
        }
        Handover::NoWindow { new, reason } => {
            tell(old, &format!("oso dev: restart held: {reason}"));
            keep_stray(&mut instances.strays, new, &reason);
            false
        }
        Handover::Failed(reason) if old.gone() => {
            log::warn!("oso dev: the old instance has gone: {reason:?}");
            false
        }
        Handover::Failed(reason) => {
            log::warn!("oso dev: the old instance stays: {reason:?}");
            tell(old, &format!("oso dev: restart held: {reason}"));
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
    use super::{
        Fd, Handover, Instance, Instances, MAX_MESSAGE, NotStopped, hand_over, restart, tell,
    };
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
        /// The text of item 0, which `export-text` gives four bytes at a time.
        text: String,
        /// What `import-text` brought.
        imported: String,
        /// What `dev-message` said.
        told: Vec<String>,
        /// The generation its import reported.
        generation_imported: Option<u64>,
        /// The terminal library it says it reads, which has the old one export terminals.
        library: Option<&'static str>,
        /// How many masters `hand-over-terminals` gives.
        masters: usize,
        /// How many masters `import-terminals` brought, and whether it was told they stay.
        given: Option<usize>,
        abandoned: bool,
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
                text: String::new(),
                imported: String::new(),
                told: Vec::new(),
                generation_imported: None,
                library: None,
                masters: 0,
                given: None,
                abandoned: false,
            }
        }

        /// An instance whose export carries `count` terminals, and gives their masters.
        #[cfg(unix)]
        fn with_terminals(name: &'static str, log: &Log, count: usize) -> Self {
            Self {
                export: Ok(json!({
                    "version": 5, "generation": EXPORTED, "export": 3, "items": [],
                    "terminals": vec![json!({}); count],
                })),
                masters: count,
                ..Self::new(name, log)
            }
        }

        /// A new instance that reads terminals.
        #[cfg(unix)]
        fn reading_terminals(name: &'static str, log: &Log) -> Self {
            Self {
                library: Some("lib"),
                ..Self::new(name, log)
            }
        }

        /// An instance whose export carries `text` as its one item.
        fn carrying(name: &'static str, log: &Log, text: &str) -> Self {
            Self {
                export: Ok(json!({
                    "version": 2, "generation": EXPORTED, "export": 3, "items": [text.len()],
                })),
                text: text.to_owned(),
                ..Self::new(name, log)
            }
        }

        /// An instance with unsaved edits, and no way to hand over.
        fn edited(name: &'static str, log: &Log) -> Self {
            Self {
                check: Err("cannot hand over notes: it has unsaved edits".into()),
                ..Self::new(name, log)
            }
        }

        /// An instance with unsaved text it can hand over, but which must not be closed.
        fn unsaved(name: &'static str, log: &Log) -> Self {
            Self {
                check: Ok(json!({ "generation": CHECKED, "unsaved": true })),
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
            if method == "version" {
                return Ok(json!({ "terminal_library": self.library }));
            }
            self.note(method);
            match method {
                "export-state" if self.library.is_none() => {
                    let asked = params.is_some_and(|p| p["terminals"]["library"] == "lib");
                    let mut export = self.export.clone();
                    if !asked && let Ok(state) = &mut export {
                        state.as_object_mut().unwrap().remove("terminals");
                    }
                    export
                }
                "import-terminals" => {
                    assert_eq!(params.unwrap()["abandon"], true);
                    self.abandoned = true;
                    Ok(Value::Null)
                }
                "check-handover" => self.check.clone(),
                "export-state" => self.export.clone(),
                "import-state" => {
                    assert!(params.unwrap()["state"]["version"].is_u64());
                    self.import.clone()
                }
                "export-text" => {
                    let params = params.unwrap();
                    assert_eq!(
                        (params["export"].clone(), params["item"].clone()),
                        (json!(3), json!(0))
                    );
                    let offset = usize::try_from(params["offset"].as_u64().unwrap()).unwrap();
                    let end = (offset + 4).min(self.text.len());
                    Ok(json!({ "text": &self.text[offset..end], "end": end }))
                }
                "import-text" => {
                    let params = params.unwrap();
                    assert_eq!(params["offset"], self.imported.len());
                    self.imported.push_str(params["text"].as_str().unwrap());
                    Ok(json!({ "end": self.imported.len() }))
                }
                "dev-message" => {
                    self.told
                        .push(params.unwrap()["text"].as_str().unwrap().to_owned());
                    Ok(Value::Null)
                }
                other => panic!("{other}"),
            }
        }

        fn call_passing(
            &mut self,
            method: &str,
            params: Option<Value>,
            fds: Vec<Fd>,
        ) -> Result<(Value, Vec<Fd>), String> {
            self.note(method);
            match method {
                "hand-over-terminals" => {
                    let masters = (0..self.masters).map(|_| pipe_end()).collect();
                    Ok((json!({ "terminals": self.masters }), masters))
                }
                "import-terminals" => {
                    assert_eq!(params.unwrap()["count"], fds.len());
                    self.given = Some(fds.len());
                    Ok((Value::Null, Vec::new()))
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

        fn gone_within(&mut self, _limit: std::time::Duration) -> bool {
            self.exited
        }

        fn imported(&self) -> Option<u64> {
            self.generation_imported
        }

        fn set_imported(&mut self, generation: Option<u64>) {
            self.generation_imported = generation;
        }

        fn stop(&mut self, generation: u64) -> Result<(), NotStopped> {
            self.note(&format!("stop {generation}"));
            match &self.stop {
                Ok(()) => Ok(()),
                Err(NotStopped::Refused(reason)) => Err(NotStopped::Refused(reason.clone())),
                Err(NotStopped::Unknown(reason)) => Err(NotStopped::Unknown(reason.clone())),
                Err(NotStopped::Agreed(reason)) => Err(NotStopped::Agreed(reason.clone())),
            }
        }
    }

    fn log() -> Log {
        Rc::new(RefCell::new(Vec::new()))
    }

    /// A descriptor that stands for a master.
    #[cfg(unix)]
    fn pipe_end() -> Fd {
        std::io::pipe().unwrap().0.into()
    }

    #[cfg(not(unix))]
    fn pipe_end() -> Fd {
        unreachable!("no descriptors pass here")
    }

    #[cfg(unix)]
    #[test]
    fn terminals_reach_the_new_instance_only_once_the_old_one_has_stopped() {
        let log = log();
        let mut old = Fake::with_terminals("old", &log, 2);
        let new = Fake::reading_terminals("new", &log);
        let Handover::Replaced(new) = handed_over(&mut old, new) else {
            panic!("not replaced");
        };
        assert_eq!(new.given, Some(2));
        assert_eq!(
            *log.borrow(),
            [
                "old check-handover",
                "new ready?",
                "old export-state",
                "new import-state",
                "old hand-over-terminals",
                "old stop 8",
                "new import-terminals",
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_new_instance_that_reads_no_terminals_gets_none() {
        let log = log();
        let mut old = Fake::with_terminals("old", &log, 2);
        let new = Fake::new("new", &log);
        let Handover::Replaced(new) = handed_over(&mut old, new) else {
            panic!("not replaced");
        };
        assert_eq!(new.given, None);
        assert!(
            !log.borrow().iter().any(|l| l.contains("terminals")),
            "{log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_refused_exit_keeps_the_terminals_with_the_old_instance() {
        let log = log();
        let mut old = Fake {
            stop: Err(NotStopped::Refused("exit refused".into())),
            ..Fake::with_terminals("old", &log, 1)
        };
        let new = Fake::reading_terminals("new", &log);
        let outcome = handed_over(&mut old, new);
        assert!(matches!(outcome, Handover::Failed(_)));
        let log = log.borrow();
        assert_eq!(
            log.iter().filter(|l| *l == "new import-terminals").count(),
            1
        );
        assert!(log.contains(&"old stop 8".to_owned()));
    }

    #[cfg(unix)]
    #[test]
    fn masters_short_of_the_terminals_stop_nothing() {
        let log = log();
        let mut old = Fake {
            masters: 1,
            ..Fake::with_terminals("old", &log, 2)
        };
        let new = Fake::reading_terminals("new", &log);
        let outcome = handed_over(&mut old, new);
        assert!(matches!(outcome, Handover::Failed(_)));
        assert!(
            !log.borrow().iter().any(|l| l.starts_with("old stop")),
            "{log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_old_instance_that_agreed_but_is_slow_to_go_lets_its_terminals_go() {
        let log = log();
        let mut old = Fake {
            stop: Err(NotStopped::Agreed("slow".into())),
            ..Fake::with_terminals("old", &log, 2)
        };
        let new = Fake::reading_terminals("new", &log);
        let Handover::Beside { new, .. } = handed_over(&mut old, new) else {
            panic!("not beside");
        };
        assert_eq!(new.given, Some(2), "the masters go to the new one");
        assert!(!new.abandoned);
    }

    #[cfg(unix)]
    #[test]
    fn an_old_instance_that_may_yet_exit_keeps_its_terminals() {
        let log = log();
        let mut old = Fake {
            stop: Err(NotStopped::Unknown("no answer".into())),
            ..Fake::with_terminals("old", &log, 1)
        };
        let new = Fake::reading_terminals("new", &log);
        let Handover::Beside { new, .. } = handed_over(&mut old, new) else {
            panic!("not beside");
        };
        assert!(new.abandoned, "the new one is told they stay");
        assert_eq!(new.given, None);
    }

    /// Hands over with no instance beside the old one.
    fn handed_over(old: &mut Fake, new: Fake) -> Handover<Fake> {
        hand_over(old, &mut None, || Ok(new))
    }

    /// An instance whose export names a native session to resume (ADR 0165, part 4c).
    fn with_natives(name: &'static str, log: &Log, stop: Result<(), NotStopped>) -> Fake {
        Fake {
            export: Ok(json!({
                "version": 5, "generation": EXPORTED, "export": 3, "items": [],
                "native_resumes": [{ "id": "0123abcd-0000-4000-8000-000000000001" }],
            })),
            stop,
            ..Fake::new(name, log)
        }
    }

    #[test]
    fn named_native_sessions_resume_in_the_new_instance_once_the_old_one_has_stopped() {
        let log = log();
        let mut old = with_natives("old", &log, Ok(()));
        let Handover::Replaced(new) = handed_over(&mut old, Fake::new("new", &log)) else {
            panic!("not replaced");
        };
        assert_eq!(new.given, Some(0), "told with no master");
        let log = log.borrow();
        assert_eq!(log[log.len() - 2..], ["old stop 8", "new import-terminals"]);
    }

    #[test]
    fn named_native_sessions_stay_stopped_while_the_old_one_stays_or_has_not_gone() {
        let log = log();
        let mut old = with_natives("old", &log, Err(NotStopped::Refused("refused".into())));
        let Handover::Failed(_) = handed_over(&mut old, Fake::new("new", &log)) else {
            panic!("not given up");
        };
        assert!(log.borrow().contains(&"new import-terminals".to_owned()));
        // It agreed but has not gone: the new one is told nothing yet.
        let log = self::log();
        let agreed = Err(NotStopped::Agreed("going".into()));
        let mut old = with_natives("old", &log, agreed);
        let Handover::Beside { new, .. } = handed_over(&mut old, Fake::new("new", &log)) else {
            panic!("not beside");
        };
        assert_eq!((new.given, new.abandoned), (None, false));
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
    fn the_text_goes_across_a_piece_at_a_time_before_the_import() {
        let log = log();
        let mut old = Fake::carrying("old", &log, "ten bytes!");
        let new = Fake::new("new", &log);
        let Handover::Replaced(new) = handed_over(&mut old, new) else {
            panic!("not replaced");
        };
        assert_eq!(new.imported, "ten bytes!");
        let log = log.borrow();
        let pieces = log.iter().filter(|line| *line == "old export-text").count();
        assert_eq!(pieces, 3);
        let import = log
            .iter()
            .position(|line| line == "new import-state")
            .unwrap();
        let last_piece = log
            .iter()
            .rposition(|line| line == "new import-text")
            .unwrap();
        assert!(last_piece < import, "{log:?}");
        assert_eq!(log.last().map(String::as_str), Some("old stop 8"));
    }

    #[test]
    fn an_instance_with_unsaved_text_beside_or_stray_is_never_closed() {
        let log = log();
        let mut old = Fake::new("old", &log);
        let mut beside = Some(Fake::unsaved("beside", &log));
        let outcome = hand_over::<Fake>(&mut old, &mut beside, || panic!("a new window opened"));
        assert!(matches!(outcome, Handover::Failed(reason) if reason.contains("unsaved")));
        assert!(beside.is_some());
        let mut strays = vec![Fake::unsaved("stray", &log)];
        super::sweep(&mut strays);
        assert_eq!(strays.len(), 1);
        assert!(
            !log.borrow().iter().any(|line| line.contains("stop")),
            "{log:?}"
        );
    }

    #[test]
    fn a_new_instance_with_imported_text_goes_by_its_import_generation_if_unchanged() {
        let log = log();
        // The old one was typed into after the export, so it refuses to go.
        let mut old = Fake::refusing("old", &log);
        let mut new = Fake::unsaved("new", &log);
        new.import = Ok(json!({ "buffers": 2, "generation": 42 }));
        let outcome = handed_over(&mut old, new);
        assert!(matches!(outcome, Handover::Failed(_)));
        assert_eq!(log.borrow().last().map(String::as_str), Some("new stop 42"));

        // Typed into too, the new one stays, and only by that generation is it closed later.
        let log = self::log();
        let mut old = Fake::refusing("old", &log);
        let mut new = Fake::unsaved("new", &log);
        new.import = Ok(json!({ "buffers": 2, "generation": 42 }));
        new.stop = Err(NotStopped::Refused("exit refused".into()));
        let Handover::Beside { new, .. } = handed_over(&mut old, new) else {
            panic!("the new one went");
        };
        assert_eq!(new.imported(), Some(42));
    }

    #[test]
    fn a_held_restart_says_why_in_the_old_instance() {
        let log = log();
        let mut instances = with_current(Fake::edited("old", &log));
        assert!(!restart(&mut instances, || -> Result<Fake, String> {
            panic!("a new window opened")
        }));
        let told = &instances.current.as_ref().unwrap().told;
        assert_eq!(told.len(), 1);
        assert!(told[0].starts_with("oso dev: restart held: "), "{told:?}");
        assert!(told[0].contains("notes"), "{told:?}");
        let mut long = Fake::new("long", &log);
        tell(&mut long, &"é".repeat(MAX_MESSAGE));
        assert!(long.told[0].len() <= MAX_MESSAGE);
        assert!(long.told[0].ends_with('…'));
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
