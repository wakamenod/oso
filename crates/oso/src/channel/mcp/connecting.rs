//! How `oso mcp` reaches oso (ADR 0085): which instance it connects to, and the connection to
//! its agent channel or its sessions' socket (ADR 0165, step 2), spoken to only if it is oso.
//!
//! Must not depend on: the bridge's JSON-RPC; this only connects and carries calls.

use std::io::{self, BufReader, Write};
use std::time::Instant;

use oso_platform::control::{ControlDir, ControlStream, Socket};
use serde_json::{Value, json};

use super::super::wire::{parse_response, read_bounded_line};
use super::{Failed, Link, PROBE_EVERY, follower, permission, say};

/// The channel of a running oso, over its socket.
pub(super) struct SocketLink {
    writer: ControlStream,
    reader: BufReader<ControlStream>,
    next_id: u64,
}

impl Link for SocketLink {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Failed> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({ "id": id, "method": method, "params": params });
        let broken = |e: io::Error| Failed::Broken(format!("the connection to oso broke: {e}"));
        writeln!(self.writer, "{request}").map_err(broken)?;
        let mut line = Vec::new();
        // The person may take as long as they like to let this connection in.
        if read_bounded_line(&mut self.reader, &mut line).map_err(broken)? == 0 {
            return Err(Failed::Broken("oso closed the connection".into()));
        }
        if line.last() != Some(&b'\n') {
            return Err(Failed::Broken(
                "oso sent a line longer than a response may be".into(),
            ));
        }
        match parse_response(&String::from_utf8_lossy(&line)) {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => Err(Failed::Refused(message)),
            Err(why) => Err(Failed::Broken(format!(
                "oso sent what is no response: {why}"
            ))),
        }
    }

    fn closer(&self) -> Option<Box<dyn Fn() + Send>> {
        let stream = self.writer.try_clone().ok()?;
        Some(Box::new(move || stream.shutdown()))
    }
}

/// Which instance the bridge connects to, and when it last looked through them all.
pub(super) struct Target {
    /// The instance `--pid` named, or the one connected to last.
    pid: Option<u32>,
    /// Whether `pid` came from `--pid`: the bridge then never looks for another.
    given: bool,
    /// When the bridge last looked through every instance's socket and found none to use, and
    /// what it told the agent.
    probed: Option<(Instant, String)>,
}

impl Target {
    pub(super) fn new(pid: Option<u32>) -> Self {
        Self {
            pid,
            given: pid.is_some(),
            probed: None,
        }
    }

    /// Connects with `connect` to the instance it knows, or else to the only one whose channel
    /// is on. It looks for that one at most once each [`PROBE_EVERY`], and in between tells
    /// the agent what it found last. What goes wrong is said in full on stderr, and to the
    /// agent in fixed words.
    ///
    /// An instance `--pid` named that has gone may have been replaced by a restart (`oso dev`, `oso-restart`):
    /// `follows` names the one that took over from it, if one did (ADR 0120, step 11), and
    /// the bridge goes on with that one.
    pub(super) fn connect<S>(
        &mut self,
        now: Instant,
        mut connect: impl FnMut(Option<u32>) -> io::Result<(u32, S)>,
        follows: impl FnOnce(u32) -> Option<u32>,
    ) -> Result<(u32, S), String> {
        if let Some(pid) = self.pid {
            match connect(Some(pid)) {
                Ok(found) => return Ok(found),
                Err(e) => {
                    say(&format!("connecting to pid {pid}: {e}"));
                    if self.given {
                        if let Some(next) = follows(pid)
                            && let Ok(found) = connect(Some(next))
                        {
                            say(&format!("pid {pid} has gone; pid {next} took over from it"));
                            self.pid = Some(next);
                            return Ok(found);
                        }
                        return Err(told(&e, true));
                    }
                    // It has gone, as when oso restarts.
                    self.pid = None;
                }
            }
        }
        if let Some((at, told)) = &self.probed
            && now.saturating_duration_since(*at) < PROBE_EVERY
        {
            return Err(told.clone());
        }
        match connect(None) {
            Ok((pid, stream)) => {
                self.pid = Some(pid);
                self.probed = None;
                Ok((pid, stream))
            }
            Err(e) => {
                say(&format!("connecting: {e}"));
                let told = told(&e, false);
                self.probed = Some((now, told.clone()));
                Err(told)
            }
        }
    }
}

/// What the agent is told when connecting fails with `e`; `given` if `--pid` named the
/// instance. No process IDs or paths.
fn told(e: &io::Error, given: bool) -> String {
    if e.kind() == io::ErrorKind::InvalidInput && !given {
        "several oso instances have the agent channel on; the person starts oso mcp with --pid to choose one".into()
    } else if given {
        "the oso instance chosen with --pid does not have the agent channel on; the person turns it on with M-x agent-channel-start".into()
    } else {
        "no oso instance has the agent channel on; the person turns it on with M-x agent-channel-start".into()
    }
}

/// Connects to `socket`, the agent channel's or the sessions', of the instance `target` says. Only an oso that runs this
/// program, and is the process that owns the socket, is spoken to: another process of the
/// same user could listen in its place.
pub(super) fn connect(target: &mut Target, socket: Socket) -> Result<Box<dyn Link>, String> {
    let (found, stream) = target
        .connect(
            Instant::now(),
            |pid| ControlDir::open_existing()?.connect(socket, pid),
            follower,
        )
        .map_err(|told| match socket {
            Socket::Session => permission::NOT_HOSTED.to_owned(),
            _ => told,
        })?;
    let peer = stream.peer();
    let is_oso = peer.runs_this_program
        && peer
            .process
            .as_ref()
            .is_some_and(|process| process.pid() == found);
    if !is_oso {
        say(&format!(
            "the {socket:?} socket of pid {found} is served by a process that is not this oso"
        ));
        if !target.given {
            target.pid = None;
        }
        return Err(match socket {
            Socket::Session => "the sessions' socket is served by something other than oso",
            _ => "the agent channel's socket is served by something other than oso; ask the person",
        }
        .into());
    }
    let reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    Ok(Box::new(SocketLink {
        writer: stream,
        reader,
        next_id: 1,
    }))
}
