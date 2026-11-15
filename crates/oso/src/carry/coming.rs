//! The new instance, in `oso-restart` (ADR 0133): on a thread of its own, it reads the pair it
//! inherited from the instance going, which started it, and asks its own UI thread what the
//! dev socket's `import-text`, `import-state` and `import-terminals` ask under `oso dev`.
//!
//! Must not depend on: `app`; the caller gives the UI thread through `post`.
//!
//! - It speaks first, once its window is up: [`HELLO`], naming the state versions it reads and
//!   its terminal library.
//! - Each ask of the instance going is answered. The masters that come with
//!   [`TAKE_MASTERS`] are held, not read: the terminals take them only once the pair has ended
//!   and the instance going is no longer this one's parent, so that only one instance reads
//!   each PTY. If it stays ([`STAY`]), or does not go within [`GONE_TIME`] of the end, they are
//!   given up and the terminals end with their last screen.
//! - What comes over the pair comes only from the parent: the pair is the one this process
//!   inherited, whose peer was its parent when it started.

// The restart needs the pair and terminals, which only Unix has for now; its parts are
// tested everywhere.
#![cfg_attr(not(unix), allow(dead_code, reason = "oso-restart runs on Unix only"))]

use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::fds::{Fd, Passing};
use super::pair::{HELLO, Message, Pair, STAGE_PIECE, STAY, TAKE_MASTERS, TAKE_STATE};
use super::pieces::CarriedPiece;
use super::{Asked, Op, Passed, Reply, STATE_VERSION, parse};

/// The oldest state version this build reads.
const OLDEST_STATE: u64 = 2;

/// How long the instance going may take between two asks.
const ASK_TIME: Duration = Duration::from_secs(60);

/// How long after the pair ends the instance going may take to be gone.
pub const GONE_TIME: Duration = Duration::from_secs(10);

/// How often its going is looked for meanwhile.
const GONE_LOOK: Duration = Duration::from_millis(50);

/// How long the UI thread may take to answer.
const UI_TIME: Duration = Duration::from_secs(30);

/// Reads the pair from the instance going, `parent`, and asks the UI thread through `post`.
/// Returns once the restart is over.
fn serve(mut pair: Pair, parent: u32, post: impl Fn(Asked) -> bool) {
    let hello = json!({
        "pid": std::process::id(),
        "state_versions": [OLDEST_STATE, STATE_VERSION],
        "terminal_library": oso_term::Terminal::library(),
    });
    if let Err(e) = pair.send(HELLO, hello, &[], ASK_TIME) {
        log::warn!("oso-restart: cannot reach the instance before: {e}");
        return;
    }
    let mut coming = Coming {
        post,
        generation: None,
        terminals: 0,
        masters: Vec::new(),
    };
    loop {
        let message = match pair.receive(ASK_TIME) {
            Ok(Some(message)) => message,
            Ok(None) => break,
            Err(e) => {
                log::warn!("oso-restart: the pair from the instance before failed: {e}");
                break;
            }
        };
        if message.say == STAY {
            coming.left();
            return;
        }
        let reply = coming.answer(message);
        if let Err(e) = pair.answer(&reply, ASK_TIME) {
            log::warn!("oso-restart: cannot answer the instance before: {e}");
            break;
        }
    }
    coming.ended(parent, parent_id);
}

/// What the new instance holds of the restart.
struct Coming<P> {
    post: P,
    /// The generation its import reported, once it took the state.
    generation: Option<u64>,
    /// How many terminals the state carried.
    terminals: usize,
    /// Their masters, held until the instance going has gone.
    masters: Vec<Fd>,
}

impl<P: Fn(Asked) -> bool> Coming<P> {
    fn answer(&mut self, message: Message) -> Reply {
        let Message { say, fields, fds } = message;
        match say.as_str() {
            STAGE_PIECE => self.ask(Op::ImportText(piece(&fields)?), Vec::new()),
            TAKE_STATE => {
                if self.generation.is_some() {
                    return Err("a state was taken already".into());
                }
                let state = parse(fields.get("state").ok_or("no state came")?)?;
                let terminals = state.terminals.len();
                let reply = self.ask(Op::ImportState(Box::new(state)), Vec::new())?;
                self.generation = reply["generation"].as_u64();
                self.terminals = terminals;
                Ok(reply)
            }
            TAKE_MASTERS => {
                let count = fields.get("count").and_then(Value::as_u64);
                if self.generation.is_none()
                    || !self.masters.is_empty()
                    || count != u64::try_from(self.terminals).ok()
                    || fds.len() != self.terminals
                {
                    return Err("the masters do not match the terminals taken".into());
                }
                self.masters = fds;
                Ok(json!({ "holding": self.terminals }))
            }
            _ => Err("not an ask of a restart".into()),
        }
    }

    /// Asks the UI thread `op`, as the parent does, with `brought`.
    fn ask(&self, op: Op, brought: Vec<Fd>) -> Reply {
        let (reply, answer) = std::sync::mpsc::channel();
        let passed = Passed {
            brought,
            passing: Passing::default(),
            parent: true,
        };
        if !(self.post)(Asked::Op { op, passed, reply }) {
            return Err("the window has gone".into());
        }
        answer
            .recv_timeout(UI_TIME)
            .map_err(|_| "the window did not answer".to_owned())?
    }

    /// The instance going stays: what was taken of its terminals is given up, and this one
    /// closes unless it was changed since the import.
    fn left(&mut self) {
        drop(std::mem::take(&mut self.masters));
        let _ = (self.post)(Asked::PreviousStays {
            generation: self.generation,
        });
    }

    /// The pair has ended: once the instance going is no longer `parent`'s, as `parent_now`
    /// tells, the terminals take their masters; if it is still there after [`GONE_TIME`], or no
    /// masters came, they are given up.
    fn ended(mut self, parent: u32, parent_now: impl Fn() -> u32) {
        if self.terminals == 0 {
            return;
        }
        let masters = std::mem::take(&mut self.masters);
        let count = masters.len();
        let gone = count > 0 && {
            let since = Instant::now();
            loop {
                if parent_now() != parent {
                    break true;
                }
                if since.elapsed() >= GONE_TIME {
                    break false;
                }
                std::thread::sleep(GONE_LOOK);
            }
        };
        let op = Op::ImportTerminals {
            count: gone.then_some(count),
        };
        let brought = if gone { masters } else { Vec::new() };
        if let Err(why) = self.ask(op, brought) {
            log::warn!("oso-restart: the terminals carried here did not take over: {why}");
        }
    }
}

/// The piece a [`STAGE_PIECE`] line brings.
fn piece(fields: &Map<String, Value>) -> Result<CarriedPiece, String> {
    let number = |name: &str| {
        fields
            .get(name)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("a piece has no {name}"))
    };
    let size = |name: &str| {
        number(name).and_then(|n| usize::try_from(n).map_err(|_| format!("{name} is too large")))
    };
    let Some(Value::String(text)) = fields.get("text") else {
        return Err("a piece has no text".into());
    };
    Ok(CarriedPiece {
        export: number("export")?,
        item: size("item")?,
        offset: size("offset")?,
        text: text.clone(),
    })
}

/// The pair this process inherited from the instance that started it with `oso-restart`, and
/// that instance's id, its parent's.
pub struct Inherited {
    pair: oso_platform::control::ControlStream,
    parent: u32,
}

/// The pair this process inherited as its descriptor 3, whose other end must be its parent:
/// the instance that started it with `oso-restart`. Call it while this process has one thread.
pub fn inherited_pair() -> Result<Inherited, String> {
    let pair = oso_platform::control::ControlStream::inherited().map_err(|e| e.to_string())?;
    let parent = parent_id();
    let peer = pair.peer().process.map(|process| process.pid());
    if peer != Some(parent) {
        return Err("its other end is not this process's parent".into());
    }
    Ok(Inherited { pair, parent })
}

/// Starts the thread that reads `inherited` and asks the UI thread through `post`, which
/// returns false once the UI thread is gone. It asks [`Asked::Over`] last.
pub fn spawn(
    inherited: Inherited,
    post: impl Fn(Asked) -> bool + Send + 'static,
) -> std::io::Result<()> {
    let pair = Pair::new(inherited.pair)?;
    let parent = inherited.parent;
    std::thread::Builder::new()
        .name("oso-carried".into())
        .spawn(move || {
            serve(pair, parent, &post);
            let _ = post(Asked::Over);
        })
        .map(drop)
}

/// This process's parent now.
#[cfg(unix)]
fn parent_id() -> u32 {
    std::os::unix::process::parent_id()
}

/// No restart reaches a system without one.
#[cfg(not(unix))]
fn parent_id() -> u32 {
    0
}

#[cfg(all(test, unix))]
mod tests;
