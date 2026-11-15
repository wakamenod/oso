//! The socket pair `oso-restart` shares with the instance it starts (ADR 0133): one JSON
//! object a line, each at most the dev socket's [`MAX_LINE`], descriptors only with the line
//! that hands the masters over. No socket file names it, so only the two processes hold it.
//!
//! Must not depend on: `app`, the editor or the restart's order; this moves lines and
//! descriptors, and [`super::going`] and [`super::coming`] judge them.
//!
//! - The new instance speaks first, [`HELLO`], once its window is up. Then the instance going
//!   asks, and the new one answers each ask with `{"ok": …}` or `{"error": "…"}`, but
//!   [`STAY`], which wants no answer.
//! - A line longer than [`MAX_LINE`], one that is not an object or not UTF-8, ends the restart
//!   as an error.
//! - The names are the pair's own, none a dev method's (`just release-safety`).

// The restart needs the pair and terminals, which only Unix has for now; its parts are
// tested everywhere.
#![cfg_attr(not(unix), allow(dead_code, reason = "oso-restart runs on Unix only"))]

use std::io::{self, BufReader};
use std::time::Duration;

use oso_platform::control::ControlStream;
use serde_json::{Map, Value, json};

use super::fds::Fd;
#[cfg(unix)]
use super::fds::{Receiving, write_line};
use crate::channel::wire::{MAX_LINE, read_line_within};

/// What the new instance says first: its process id, the state versions it reads and the
/// terminal library it reads terminals with.
pub const HELLO: &str = "hello";
/// A piece of a carried text, for the import that follows (ADR 0090).
pub const STAGE_PIECE: &str = "stage-piece";
/// The state to take over.
pub const TAKE_STATE: &str = "take-state";
/// The masters of the carried terminals come with this line, to hold until the instance going
/// has gone.
pub const TAKE_MASTERS: &str = "take-masters";
/// The instance going stays, and keeps its terminals: what the new one took is not its.
pub const STAY: &str = "stay";

/// The longest a reason from the other end that is shown may be, in characters.
const MAX_REASON: usize = 200;

/// One end of the pair.
pub struct Pair {
    #[cfg(unix)]
    reader: BufReader<Receiving>,
    #[cfg(not(unix))]
    reader: BufReader<ControlStream>,
    writer: ControlStream,
}

/// A line that came: what it says, its fields and the descriptors that came with it.
#[derive(Debug)]
pub struct Message {
    pub say: String,
    pub fields: Map<String, Value>,
    pub fds: Vec<Fd>,
}

impl Pair {
    /// `stream`'s end, read and written.
    pub fn new(stream: ControlStream) -> io::Result<Self> {
        let writer = stream.try_clone()?;
        #[cfg(unix)]
        let reader = BufReader::with_capacity(MAX_LINE, Receiving::new(stream));
        #[cfg(not(unix))]
        let reader = BufReader::with_capacity(MAX_LINE, stream);
        Ok(Self { reader, writer })
    }

    /// Writes `say` with `fields`, and `fds` along with it, giving up after `patience`.
    pub fn send(
        &mut self,
        say: &str,
        fields: Value,
        fds: &[Fd],
        patience: Duration,
    ) -> io::Result<()> {
        let mut line = match fields {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        line.insert("say".into(), Value::String(say.into()));
        self.write(&Value::Object(line), fds, patience)
    }

    /// Answers the ask just read with `reply`.
    pub fn answer(&mut self, reply: &super::Reply, patience: Duration) -> io::Result<()> {
        let line = match reply {
            Ok(value) => json!({ "ok": value }),
            Err(why) => json!({ "error": why }),
        };
        self.write(&line, &[], patience)
    }

    /// The next line, `None` once the other end has closed the pair, giving up after
    /// `patience` with `TimedOut`.
    pub fn receive(&mut self, patience: Duration) -> io::Result<Option<Message>> {
        let mut line = Vec::new();
        let read = read_line_within(&mut self.reader, &mut line, patience)?;
        #[cfg(unix)]
        let fds = self.reader.get_mut().take_fds();
        #[cfg(not(unix))]
        let fds = Vec::new();
        if read == 0 {
            return Ok(None);
        }
        if line.last() != Some(&b'\n') || line.len() > MAX_LINE {
            return Err(invalid("a line is incomplete or too long"));
        }
        let value: Value =
            serde_json::from_slice(&line).map_err(|_| invalid("a line is not JSON"))?;
        let Value::Object(mut fields) = value else {
            return Err(invalid("a line is not an object"));
        };
        let say = match fields.remove("say") {
            Some(Value::String(say)) => say,
            _ => String::new(),
        };
        Ok(Some(Message { say, fields, fds }))
    }

    /// The answer to an ask: what it says, or why it failed, the other end's reason cut and
    /// cleaned so that it can be shown.
    pub fn answered(&mut self, patience: Duration) -> Result<Value, String> {
        let message = self
            .receive(patience)
            .map_err(|e| format!("the new instance did not answer: {e}"))?
            .ok_or("the new instance went away")?;
        let mut fields = message.fields;
        if let Some(value) = fields.remove("ok") {
            return Ok(value);
        }
        match fields.remove("error") {
            Some(Value::String(why)) => Err(shown(&why)),
            _ => Err("the new instance answered nothing".into()),
        }
    }

    /// Ends the pair both ways: the other end reads its end.
    pub fn close(&self) {
        self.writer.shutdown();
    }

    fn write(&mut self, line: &Value, fds: &[Fd], patience: Duration) -> io::Result<()> {
        let line = line.to_string();
        if line.len() >= MAX_LINE {
            return Err(invalid("a line would be too long"));
        }
        self.writer.set_write_timeout(Some(patience))?;
        #[cfg(unix)]
        return write_line(&mut self.writer, &line, fds);
        #[cfg(not(unix))]
        {
            let _ = fds;
            use std::io::Write;
            writeln!(self.writer, "{line}")
        }
    }
}

/// `why`, from the other end, to show: at most [`MAX_REASON`] characters, none a control one.
pub fn shown(why: &str) -> String {
    let mut text: String = why
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_REASON)
        .collect();
    if why.chars().count() > MAX_REASON {
        text.push('…');
    }
    crate::channel::wire::escape_unsafe(text)
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Duration;

    use oso_platform::control::ControlStream;
    use serde_json::json;

    use super::{Pair, shown};

    const SOON: Duration = Duration::from_secs(5);

    fn pair() -> (Pair, Pair) {
        let (a, b) = ControlStream::pair().unwrap();
        (Pair::new(a).unwrap(), Pair::new(b).unwrap())
    }

    #[test]
    fn a_line_says_what_it_is_and_its_fields_and_brings_its_descriptors() {
        let (mut a, mut b) = pair();
        let (_reader, writer) = std::io::pipe().unwrap();
        a.send(
            "take-masters",
            json!({ "count": 1 }),
            &[writer.into()],
            SOON,
        )
        .unwrap();
        let message = b.receive(SOON).unwrap().unwrap();
        assert_eq!(message.say, "take-masters");
        assert_eq!(message.fields["count"], 1);
        assert_eq!(message.fds.len(), 1);
    }

    #[test]
    fn an_answer_is_ok_or_a_cleaned_reason() {
        let (mut a, mut b) = pair();
        b.answer(&Ok(json!({ "generation": 3 })), SOON).unwrap();
        assert_eq!(a.answered(SOON).unwrap()["generation"], 3);
        b.answer(&Err("no\u{1b}[31m way".into()), SOON).unwrap();
        assert_eq!(a.answered(SOON).unwrap_err(), "no[31m way");
    }

    #[test]
    fn the_end_of_the_pair_is_none_and_a_closed_peer_is_an_error() {
        let (mut a, b) = pair();
        drop(b);
        assert!(a.receive(SOON).unwrap().is_none());
        assert!(a.answered(SOON).unwrap_err().contains("went away"));
    }

    #[test]
    fn a_line_that_is_not_an_object_is_refused() {
        let (a, mut b) = pair();
        let mut raw = a.writer.try_clone().unwrap();
        std::io::Write::write_all(&mut raw, b"[1, 2]\n").unwrap();
        assert!(b.receive(SOON).is_err());
    }

    #[test]
    fn a_long_reason_is_cut() {
        let long = "x".repeat(500);
        let text = shown(&long);
        assert_eq!(text.chars().count(), 201);
        assert!(text.ends_with('…'));
    }
}
