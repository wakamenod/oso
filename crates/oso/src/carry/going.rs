//! The instance going, in `oso-restart` (ADR 0133): on a thread of its own, it checks the
//! binary, starts it with one end of a socket pair, and plays `oso dev`'s supervisor between
//! its own UI thread and the new instance. Its own state it asks of its UI thread with the
//! same [`Op`]s the dev socket brings; the new instance it asks over the [`Pair`].
//!
//! Must not depend on: `app`; the caller gives what the UI thread answers through [`Here`].
//!
//! The order, in [`hand_over`], can be tested without processes:
//! 1. The new instance says [`HELLO`] within [`START_TIME`], naming the state versions it
//!    reads, which must hold this one's; otherwise it is stopped.
//! 2. The UI thread exports the state, the terminals stopping their reads; its pieces go over
//!    the pair, then the state, which the new instance answers once its files are judged.
//!    Any failure until then stops the new instance, and this one reads its terminals again.
//! 3. The UI thread hands over a second descriptor of each master, and they go over the pair,
//!    for the new instance to hold until this one has gone.
//! 4. The UI thread exits if nothing changed since the export, letting the terminals go,
//!    nothing hung up. Otherwise, and after any failure since the import, the new instance is
//!    told to [`STAY`] out of it: it gives the terminals up, and closes unless it was edited.
//!
//! So only one instance reads each PTY at a time: this one until the export, the new one only
//! once this one has gone.

// The restart needs the pair and terminals, which only Unix has for now; its parts are
// tested everywhere.
#![cfg_attr(not(unix), allow(dead_code, reason = "oso-restart runs on Unix only"))]

use std::ffi::OsString;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Child;
use std::time::Duration;

use serde_json::{Value, json};

use super::fds::{Fd, Passing};
#[cfg(unix)]
use super::pair::{HELLO, Pair, STAY};
use super::pair::{STAGE_PIECE, TAKE_MASTERS, TAKE_STATE};
use super::{Asked, Op, Passed, STATE_VERSION, binary};

/// How long the new instance may take to start and say [`HELLO`].
pub const START_TIME: Duration = Duration::from_secs(10);

/// How long the new instance may take to answer an ask.
pub const ANSWER_TIME: Duration = Duration::from_secs(10);

/// How long the new instance may take to say it holds the masters.
pub const HOLD_TIME: Duration = Duration::from_secs(5);

/// How long this instance's own UI thread may take to answer: an export waits up to 8 s for
/// its terminals.
const UI_TIME: Duration = Duration::from_secs(20);

/// The argument that tells a new instance to take over through the pair it inherits.
pub const TAKE_OVER: &str = "--take-over";

/// What the new instance said first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// The oldest and newest state versions it reads.
    pub versions: (u64, u64),
    /// The terminal library it reads terminals with.
    pub library: Option<String>,
}

/// This instance's own UI thread, as the restart asks it.
pub trait Here {
    /// Asks `op` as the parent does, and returns the answer and the descriptors it put.
    fn ask(&mut self, op: Op) -> Result<(Value, Vec<Fd>), String>;
    /// Asks it to exit if its generation is still `generation`, letting the terminals go.
    /// `Ok` once it has agreed: it then exits.
    fn exit(&mut self, generation: u64) -> Result<(), String>;
    /// The restart is over and this instance stays: its terminals read again, the agent
    /// channel comes back if it was on, and `why` is said.
    fn stays(&mut self, why: String);
}

/// The new instance, over the pair.
pub trait There {
    /// What it said first.
    fn hello(&mut self) -> Result<Hello, String>;
    /// Asks it `say` with `fields` and `fds`, and returns its answer.
    fn ask(
        &mut self,
        say: &str,
        fields: Value,
        fds: Vec<Fd>,
        patience: Duration,
    ) -> Result<Value, String>;
    /// Tells it this instance stays, keeping its terminals.
    fn stay(&mut self);
    /// Stops it, as it holds nothing yet.
    fn stop(&mut self);
}

/// How a restart ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// This instance agreed to exit: the new one has everything.
    Replaced,
    /// The new instance was stopped before it took anything, and this one stays.
    Stopped(String),
    /// The new instance took the state but this one stays, so it was told to [`STAY`].
    Stayed(String),
}

/// Hands this instance's state, texts and terminals to the new instance, in the order the
/// module says, and tells `here` if it stays.
pub fn hand_over(here: &mut impl Here, there: &mut impl There) -> Ended {
    let ended = order(here, there);
    match &ended {
        Ended::Replaced => {}
        Ended::Stopped(why) => {
            there.stop();
            here.stays(why.clone());
        }
        Ended::Stayed(why) => {
            there.stay();
            here.stays(why.clone());
        }
    }
    ended
}

fn order(here: &mut impl Here, there: &mut impl There) -> Ended {
    let stopped = |why: String| Ended::Stopped(format!("the new oso did not take over: {why}"));
    let hello = match there.hello() {
        Ok(hello) => hello,
        Err(why) => return stopped(why),
    };
    if !(hello.versions.0..=hello.versions.1).contains(&STATE_VERSION) {
        return stopped(format!(
            "it reads states {} to {}, and this one writes {STATE_VERSION}",
            hello.versions.0, hello.versions.1
        ));
    }
    let (state, _) = match here.ask(Op::ExportState {
        terminals: Some(hello.library.unwrap_or_default()),
    }) {
        Ok(exported) => exported,
        Err(why) => return Ended::Stopped(format!("cannot restart: {why}")),
    };
    let Some(generation) = state["generation"].as_u64() else {
        return stopped("the export reported no generation".into());
    };
    if let Err(why) = carry_pieces(here, there, &state) {
        return stopped(why);
    }
    if let Err(why) = there.ask(
        TAKE_STATE,
        json!({ "state": state }),
        Vec::new(),
        ANSWER_TIME,
    ) {
        return stopped(format!("it refused the state: {why}"));
    }
    let terminals = state["terminals"].as_array().map_or(0, Vec::len);
    if terminals > 0 {
        let masters = match here.ask(Op::HandOverTerminals { generation }) {
            Ok((_, masters)) if masters.len() == terminals => masters,
            Ok((_, masters)) => {
                return Ended::Stayed(format!(
                    "{} masters were handed over for {terminals} terminals",
                    masters.len()
                ));
            }
            Err(why) => return Ended::Stayed(why),
        };
        let fields = json!({ "count": terminals });
        if let Err(why) = there.ask(TAKE_MASTERS, fields, masters, HOLD_TIME) {
            return Ended::Stayed(format!("the new oso did not take the terminals: {why}"));
        }
    }
    match here.exit(generation) {
        Ok(()) => Ended::Replaced,
        Err(why) => Ended::Stayed(why),
    }
}

/// Moves each item the state names from this instance to the new one, a piece at a time.
fn carry_pieces(here: &mut impl Here, there: &mut impl There, state: &Value) -> Result<(), String> {
    let Some(items) = state["items"].as_array() else {
        return Ok(());
    };
    let export = state["export"].as_u64().ok_or("the export has no number")?;
    for (item, len) in items.iter().enumerate() {
        let len = len.as_u64().ok_or("an item has no length")?;
        let mut offset = 0;
        while offset < len {
            let at = Op::ExportText {
                export,
                item,
                offset: usize::try_from(offset).map_err(|_| "an item is too long")?,
            };
            let (mut piece, _) = here.ask(at)?;
            let end = piece["end"]
                .as_u64()
                .filter(|&end| end > offset && end <= len)
                .ok_or("a piece does not move on")?;
            let fields = json!({
                "export": export,
                "item": item,
                "offset": offset,
                "text": piece["text"].take(),
            });
            there.ask(STAGE_PIECE, fields, Vec::new(), ANSWER_TIME)?;
            offset = end;
        }
    }
    Ok(())
}

/// Starts a restart on a thread of its own: `start` is what it runs, and `post` hands what it
/// asks to this instance's UI thread, returning false once that is gone.
pub fn spawn(start: Start, post: impl Fn(Asked) -> bool + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("oso-restart".into())
        .spawn(move || run(&start, post))
        .map(drop)
}

fn run(start: &Start, post: impl Fn(Asked) -> bool) {
    let mut here = UiThread { post };
    #[cfg(unix)]
    match NewInstance::start(start) {
        Err(why) => here.stays(format!("cannot restart: {why}")),
        Ok(mut new) => {
            if let Ended::Stayed(_) = hand_over(&mut here, &mut new) {
                new.reap();
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = start;
        here.stays("oso-restart needs Unix for now".into());
    }
}

/// This instance's UI thread, asked through `post`.
struct UiThread<P> {
    post: P,
}

impl<P: Fn(Asked) -> bool> UiThread<P> {
    fn wait(&self, asked: Asked, answer: &std::sync::mpsc::Receiver<super::Reply>) -> super::Reply {
        if !(self.post)(asked) {
            return Err("the window has gone".into());
        }
        answer
            .recv_timeout(UI_TIME)
            .map_err(|_| "the window did not answer".to_owned())?
    }
}

impl<P: Fn(Asked) -> bool> Here for UiThread<P> {
    fn ask(&mut self, op: Op) -> Result<(Value, Vec<Fd>), String> {
        let (reply, answer) = std::sync::mpsc::channel();
        let passing = Passing::default();
        let passed = Passed {
            brought: Vec::new(),
            passing: passing.clone(),
            parent: true,
        };
        let value = self.wait(Asked::Op { op, passed, reply }, &answer)?;
        Ok((value, passing.take()))
    }

    fn exit(&mut self, generation: u64) -> Result<(), String> {
        let (reply, answer) = std::sync::mpsc::channel();
        self.wait(Asked::Exit { generation, reply }, &answer)
            .map(drop)
    }

    fn stays(&mut self, why: String) {
        let _ = (self.post)(Asked::Stays(why));
    }
}

/// What the restart starts: the binary as read at start, the arguments before [`TAKE_OVER`]
/// (a dev build's own), and whether this instance read the configuration file.
#[derive(Debug, Clone)]
pub struct Start {
    pub binary: Option<PathBuf>,
    pub args: Vec<OsString>,
    pub read_config: bool,
}

impl Start {
    /// This process's own binary, as its path is now, with `args` and `read_config`.
    pub fn this_binary(args: Vec<OsString>, read_config: bool) -> Self {
        Self {
            binary: binary::own_path(),
            args,
            read_config,
        }
    }
}

/// The new instance, a child of this one, over its pair.
#[cfg(unix)]
pub struct NewInstance {
    pair: Pair,
    child: Child,
}

#[cfg(unix)]
impl NewInstance {
    /// Checks the binary, makes the pair and starts the new instance with its end as its
    /// descriptor 3. Forks this process, so never on the UI thread.
    pub fn start(start: &Start) -> Result<Self, String> {
        use std::os::fd::AsFd;

        use oso_platform::control::ControlStream;
        use oso_platform::process::{PassedFd, Sink, Source, Spawn, spawn};

        let executable = binary::check(start.binary.as_deref())?;
        let (ours, theirs) =
            ControlStream::pair().map_err(|e| format!("cannot make the pair: {e}"))?;
        let mut args = start.args.clone();
        args.push(TAKE_OVER.into());
        if !start.read_config {
            args.push("--no-config".into());
        }
        let names = inherited_names();
        let inherit: Vec<&str> = names.iter().map(String::as_str).collect();
        let child = spawn(&Spawn {
            executable: &executable,
            args: &args,
            cwd: None,
            project: None,
            inherit: &inherit,
            stdin: Source::Closed,
            stdout: Sink::Inherit,
            stderr: Sink::Inherit,
            own_process_group: true,
            set: &[],
            terminal: None,
            pass_fd: Some(PassedFd::new(theirs.as_fd())),
        })
        .map_err(|e| format!("cannot start {}: {e}", executable.path().display()))?;
        // Only the child holds its end now, so its going ends the pair here.
        drop(theirs);
        let pair = Pair::new(ours).map_err(|e| format!("cannot read the pair: {e}"))?;
        Ok(Self { pair, child })
    }

    /// Waits for the new instance to end, as one told to stay may close itself.
    pub fn reap(mut self) {
        let _ = self.child.wait();
    }
}

#[cfg(unix)]
impl There for NewInstance {
    fn hello(&mut self) -> Result<Hello, String> {
        let message = self
            .pair
            .receive(START_TIME)
            .map_err(|e| format!("it said nothing: {e}"))?
            .ok_or("it ended")?;
        if message.say != HELLO {
            return Err("it said something else first".into());
        }
        let versions = &message.fields["state_versions"];
        let version = |at: usize| versions.get(at).and_then(Value::as_u64);
        let (Some(oldest), Some(newest)) = (version(0), version(1)) else {
            return Err("it named no state versions".into());
        };
        Ok(Hello {
            versions: (oldest, newest),
            library: message.fields["terminal_library"]
                .as_str()
                .map(str::to_owned),
        })
    }

    fn ask(
        &mut self,
        say: &str,
        fields: Value,
        fds: Vec<Fd>,
        patience: Duration,
    ) -> Result<Value, String> {
        self.pair
            .send(say, fields, &fds, patience)
            .map_err(|e| format!("cannot reach the new oso: {e}"))?;
        drop(fds);
        self.pair.answered(patience)
    }

    fn stay(&mut self) {
        if let Err(e) = self.pair.send(STAY, json!({}), &[], ANSWER_TIME) {
            log::debug!("oso-restart: the new instance was not told to stay: {e}");
        }
        self.pair.close();
    }

    fn stop(&mut self) {
        self.pair.close();
        if let Err(e) = self.child.kill() {
            log::debug!("oso-restart: cannot stop the new instance: {e}");
        }
        let _ = self.child.wait();
    }
}

/// The names of this process's variables that a child may be given by name: every one the
/// process module allows, so that the new instance, and the shells it starts, see what this
/// one does. `PATH` it sets itself; the loader's never pass.
pub fn inherited_names() -> Vec<String> {
    std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .filter(|name| {
            !name.is_empty()
                && !name.contains(['=', '\0'])
                && name != "PATH"
                && !name.starts_with("LD_")
                && !name.starts_with("DYLD_")
        })
        .collect()
}

#[cfg(test)]
mod tests;
