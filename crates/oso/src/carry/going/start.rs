//! Starting `oso-restart`'s new instance and its thread (ADR 0133): the binary checked, the
//! socket pair made, the child started with its end as descriptor 3, and the two sides the order
//! in `going` asks: this instance's UI thread, through `post`, and the new instance, over the
//! pair.
//!
//! Must not depend on: `app` or `dev`.

// The restart needs the pair and terminals, which only Unix has for now; its parts are
// tested everywhere.
#![cfg_attr(not(unix), allow(dead_code, reason = "oso-restart runs on Unix only"))]

use std::ffi::OsString;
use std::path::PathBuf;
#[cfg(unix)]
#[cfg(unix)]
use std::time::Duration;

use serde_json::Value;
#[cfg(unix)]
use serde_json::json;

#[cfg(unix)]
use super::{ANSWER_TIME, Ended, Hello, START_TIME, TAKE_OVER, There, hand_over};
use super::{Here, UI_TIME};
use crate::carry::fds::{Fd, Passing};
#[cfg(unix)]
use crate::carry::pair::{HELLO, Pair, STAY};
use crate::carry::{Asked, Op, Passed, binary};

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
    fn wait(
        &self,
        asked: Asked,
        answer: &std::sync::mpsc::Receiver<crate::carry::Reply>,
    ) -> crate::carry::Reply {
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
        if !(self.post)(Asked::Exit { generation, reply }) {
            return Err("the window has gone".into());
        }
        // No time limit: once asked, the UI thread exits or stays, and the new instance must
        // not be told to stay while this one may yet exit.
        answer
            .recv()
            .map_err(|_| "the window did not answer".to_owned())?
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
    child: oso_platform::process::Child,
}

#[cfg(unix)]
impl NewInstance {
    /// Checks the binary, makes the pair and starts the new instance with its end as its
    /// descriptor 3. Reads the file system, so never on the UI thread.
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
        match Pair::new(ours) {
            Ok(pair) => Ok(Self { pair, child }),
            Err(e) => {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                Err(format!("cannot read the pair: {e}"))
            }
        }
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
        .filter(|name| oso_platform::process::may_pass(name))
        .collect()
}
