//! A native session's permission mode (ADR 0165, Q4; part 3b): the mode the CLI says it is in,
//! which the modeline shows, the mode each new process of the session starts in, and the
//! person's switch to the next, from its request to its fallback. These are the rules, at the
//! instants the caller gives; the caller writes the request, and stops and resumes the process.
//!
//! Must not depend on: threads, the stream or the UI; the clock is the caller's.
//!
//! - The mode shown is the one the CLI said last: `system/init`, `system/status`, or the first
//!   `initialize` answer. A switch changes nothing shown until the CLI says so.
//! - The next mode follows the CLI's terminal: `default`, `acceptEdits`, `plan`, `auto`, and
//!   round, from the mode oso last put the process in while it has not said one, else from
//!   the one shown. From a mode the CLI says that oso never sends (`bypassPermissions`,
//!   `dontAsk`) the next is `default`. Before the CLI said one, there is no next.
//! - On tier 2 a switch is the `set_permission_mode` request. With no answer within
//!   [`ANSWER_WAIT`], or an error, it falls back to tier 1, saying why in a log line.
//! - On tier 1 the process, idle, is stopped and resumed (`--resume --permission-mode`), on
//!   Windows too, as no signal is needed. During a turn a switch is refused: the mode can
//!   change once the session is idle. A session never given a message has no conversation to
//!   resume, and one whose resume is held (a stop asked, oso waiting to quit) none now: no
//!   tier 1 for either. A switch whose process ends without resuming is over.
//! - Each new process of the session, a resume, an interrupt's resume or a fork, starts in the
//!   mode held, as `--resume` does not keep one (measured, CLI 2.1.330): the one a tier-1
//!   switch asked for; else the one oso gave the process before, until that process said a
//!   mode; else the one the CLI said last, if it is `default` or `plan`, or if it is one the
//!   person switched to here; else `default`, once the person switched here. The stream is
//!   not authenticated: a line a tool wrote to it can say any mode, so a mode that asks less is
//!   carried only from the person's switch. A session nobody switched is given no mode, and
//!   starts as the person's Claude Code settings say. A fork starts as its source would.
//! - One switch at a time.

use std::time::{Duration, Instant};

use super::control::Control;
use super::wire::Event;
use super::wire::control::Request;
use super::wire::status_line::{Kind, readable};
use crate::channel::protocol::mode::PermissionMode;

/// How long a `set_permission_mode` request has to be answered.
pub const ANSWER_WAIT: Duration = Duration::from_secs(2);

/// A mode the CLI said.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Said {
    /// One oso sends.
    Mode(PermissionMode),
    /// One it never sends, shown as the CLI says it.
    Other(String),
}

/// What the caller does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    /// Log why the request fell back.
    Log(String),
    /// Write `set_permission_mode` with this id and mode.
    Request(u64, PermissionMode),
    /// Stop the idle process, and resume it in this mode once it ended.
    Respawn(PermissionMode),
    /// Tell the person why the mode does not change.
    Refused(Refusal),
}

/// Why a switch was not begun, or came to nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The CLI said no mode yet.
    Unknown,
    /// No process runs.
    NotRunning,
    /// The process is starting: whether it takes the request is not known yet.
    Starting,
    /// One is under way.
    Already,
    /// The request does not go, and a turn runs: the mode can change once the session is idle.
    Busy,
    /// The request does not go, and the session was given no message: nothing resumes.
    Unspoken,
    /// The request does not go, and no resume may start now: a stop is asked, oso waits to
    /// quit or restart, or the conversation moved.
    Held,
}

impl Refusal {
    /// What the person is told.
    pub fn message(self) -> &'static str {
        match self {
            Self::Unknown => "The session's mode is not known yet",
            Self::NotRunning => "The session runs no claude: resume it to change its mode",
            Self::Starting => "The session is starting: its mode can change once it runs",
            Self::Already => "The session's mode is being changed",
            Self::Busy => {
                "This claude changes its mode only between turns: it can change once idle"
            }
            Self::Unspoken => {
                "This claude changes its mode only on a resume: it can change once it was given a message"
            }
            Self::Held => "This claude changes its mode only on a resume, which cannot start now",
        }
    }
}

/// Where the session's process is, as the caller knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Process {
    /// No turn runs.
    pub idle: bool,
    /// It was given a message, so its conversation can be resumed.
    pub spoke: bool,
    /// Nothing holds a resume now: no stop is asked, oso waits neither to quit nor to
    /// restart, and the conversation did not move.
    pub resumable: bool,
}

/// A native session's permission mode, across its processes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModeState {
    said: Option<Said>,
    switch: Option<Switch>,
    /// The mode oso last put the process in, by its command line or a request answered, until
    /// the process says one.
    pending: Option<PermissionMode>,
    /// The mode the person last switched to here, which a mode that asks less must be to be
    /// carried.
    chosen: Option<PermissionMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Switch {
    /// The request `id` for `mode` was written, to be answered by then.
    Asked {
        id: u64,
        mode: PermissionMode,
        by: Instant,
    },
    /// The process was stopped, to resume in `mode`.
    Respawn(PermissionMode),
}

impl ModeState {
    /// The CLI said the session is in mode `text`. Returns whether the mode shown changed.
    /// Empty, it says nothing; not readable as itself, the mode is no more shown, rather than
    /// left saying what the CLI no more says.
    pub fn said(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        // The process said a mode: what oso put it in is settled.
        self.pending = None;
        let same = match &self.said {
            Some(Said::Mode(mode)) => mode.name() == text,
            Some(Said::Other(other)) => other == text,
            None => false,
        };
        if same {
            return false;
        }
        let readable = readable(text, Kind::Word);
        if !readable && self.said.is_none() {
            return false;
        }
        self.said = readable.then(|| match PermissionMode::named(text) {
            Some(mode) => Said::Mode(mode),
            None => Said::Other(text.to_owned()),
        });
        true
    }

    /// Notes the mode `event` says, if it says one: `system/init`'s or `system/status`'s.
    /// Returns whether the mode shown changed. `initialize`'s first answer says one too, which
    /// the caller gives [`ModeState::said`].
    pub fn event(&mut self, event: &Event) -> bool {
        match event {
            Event::Init(init) => self.said(&init.permission_mode),
            Event::Status {
                mode: Some(mode), ..
            } => self.said(mode),
            _ => false,
        }
    }

    /// The mode shown, as the CLI said it.
    pub fn shown(&self) -> Option<&str> {
        match self.said.as_ref()? {
            Said::Mode(mode) => Some(mode.name()),
            Said::Other(text) => Some(text),
        }
    }

    /// The mode the person's switch goes to.
    fn next(&self) -> Option<PermissionMode> {
        let modes = PermissionMode::ALL;
        let from = match (self.pending, self.said.as_ref()?) {
            (Some(mode), _) | (None, &Said::Mode(mode)) => mode,
            (None, Said::Other(_)) => return Some(PermissionMode::Default),
        };
        let at = modes.iter().position(|known| *known == from)?;
        Some(modes[(at + 1) % modes.len()])
    }

    /// The mode a new process of the session, or a fork of it, is given, if any.
    pub fn carried(&self) -> Option<PermissionMode> {
        if let Some(Switch::Respawn(mode)) = self.switch {
            return Some(mode);
        }
        if self.pending.is_some() {
            return self.pending;
        }
        match self.said {
            Some(Said::Mode(mode @ (PermissionMode::Default | PermissionMode::Plan))) => Some(mode),
            Some(Said::Mode(mode)) if self.chosen == Some(mode) => Some(mode),
            // Once the person switched here, the settings' mode, which may ask less, is not
            // gone back to.
            _ if self.chosen.is_some() => Some(PermissionMode::Default),
            _ => None,
        }
    }

    /// The state a fork of this session starts with: the mode it is given pending, as its
    /// source's would be, and the person's choice here carried over.
    pub fn forked(&self) -> Self {
        Self {
            said: None,
            switch: None,
            pending: self.carried(),
            chosen: self.chosen,
        }
    }

    /// A new process starts: the mode it is given, [`ModeState::carried`], which is pending
    /// until it says one. A switch under way ends here: a tier-1 one's mode is the one given,
    /// the person's choice, and a request's was the old process's.
    pub fn for_start(&mut self) -> Option<PermissionMode> {
        let mode = self.carried();
        if let Some(Switch::Respawn(chosen)) = self.switch.take() {
            self.chosen = Some(chosen);
        }
        self.pending = mode;
        mode
    }

    /// The process was stopped to resume in another mode: once it ends, it is resumed.
    pub fn respawns(&self) -> bool {
        matches!(self.switch, Some(Switch::Respawn(_)))
    }

    /// The process ended, and is resumed now if `resumes`. A request that waited is over, and
    /// so is a tier-1 switch that does not resume. Returns whether such a switch came to
    /// nothing.
    pub fn ended(&mut self, resumes: bool) -> bool {
        match self.switch {
            Some(Switch::Respawn(_)) if resumes => false,
            Some(Switch::Respawn(_)) => {
                self.switch = None;
                true
            }
            Some(Switch::Asked { .. }) => {
                self.switch = None;
                false
            }
            None => false,
        }
    }

    /// The person switches to the next mode, at `now`: `control` is the running process's
    /// control layer, which says whether the request goes, and `process` where it is.
    ///
    /// # Errors
    ///
    /// [`Refusal`]: why the mode does not change.
    pub fn begin(
        &mut self,
        control: Option<&mut Control>,
        process: Process,
        now: Instant,
    ) -> Result<Then, Refusal> {
        let control = control.ok_or(Refusal::NotRunning)?;
        if self.switch.is_some() {
            return Err(Refusal::Already);
        }
        let mode = self.next().ok_or(Refusal::Unknown)?;
        let request = Request::SetPermissionMode(mode);
        if control.has(request) {
            let id = control.ask(request);
            self.switch = Some(Switch::Asked {
                id,
                mode,
                by: now + ANSWER_WAIT,
            });
            return Ok(Then::Request(id, mode));
        }
        // Whether the request goes is not known until `initialize` is answered.
        if control.deadline().is_some() {
            return Err(Refusal::Starting);
        }
        self.respawn(mode, process)
    }

    /// The request was not written: it falls back.
    pub fn not_written(&mut self, control: &mut Control, process: Process) -> Vec<Then> {
        self.fall_back(
            control,
            process,
            "the set_permission_mode request was not written",
        )
    }

    /// The CLI answered request `id`, refused with `error` if `Some`. A success changes nothing
    /// shown: the CLI says the mode it is in next (measured), or nothing when it was in it.
    pub fn answered(
        &mut self,
        control: &mut Control,
        id: u64,
        error: Option<&str>,
        process: Process,
    ) -> Vec<Then> {
        let mode = match self.switch {
            Some(Switch::Asked {
                id: asked, mode, ..
            }) if asked == id => mode,
            _ => return Vec::new(),
        };
        match error {
            None => {
                self.switch = None;
                self.pending = Some(mode);
                self.chosen = Some(mode);
                Vec::new()
            }
            Some(why) => {
                let why = format!("set_permission_mode was refused: {why}");
                self.fall_back(control, process, &why)
            }
        }
    }

    /// The person chose `mode` another way than a switch: a plan approved with it (part 3d),
    /// which the CLI puts the session in. It is carried as a switch's would be.
    pub fn chose(&mut self, mode: PermissionMode) {
        self.chosen = Some(mode);
        self.pending = Some(mode);
    }

    /// The process was stopped for nothing: a switch it was stopped for is over.
    pub fn gave_up(&mut self) {
        self.switch = None;
    }

    /// When a request's wait ends, while one waits.
    pub fn deadline(&self) -> Option<Instant> {
        match self.switch? {
            Switch::Asked { by, .. } => Some(by),
            Switch::Respawn(_) => None,
        }
    }

    /// What is due at `now`: a request not answered in time falls back.
    pub fn due(&mut self, control: &mut Control, process: Process, now: Instant) -> Vec<Then> {
        match self.switch {
            Some(Switch::Asked { by, .. }) if now >= by => {
                self.fall_back(control, process, "set_permission_mode was not answered")
            }
            _ => Vec::new(),
        }
    }

    /// The request did not carry the switch, for reason `why`: a resume does, while idle.
    fn fall_back(&mut self, control: &mut Control, process: Process, why: &str) -> Vec<Then> {
        let Some(Switch::Asked { id, mode, .. }) = self.switch.take() else {
            return Vec::new();
        };
        control.forget(id);
        let then = self.respawn(mode, process).unwrap_or_else(Then::Refused);
        vec![Then::Log(why.to_owned()), then]
    }

    /// Tier 1: the process resumed in `mode`, while idle, given a message, and free to resume.
    fn respawn(&mut self, mode: PermissionMode, process: Process) -> Result<Then, Refusal> {
        if !process.idle {
            return Err(Refusal::Busy);
        }
        if !process.spoke {
            return Err(Refusal::Unspoken);
        }
        if !process.resumable {
            return Err(Refusal::Held);
        }
        self.switch = Some(Switch::Respawn(mode));
        Ok(Then::Respawn(mode))
    }
}

#[cfg(test)]
mod tests;
