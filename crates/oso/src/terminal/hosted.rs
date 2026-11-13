//! The terminals the app hosts, by their buffers' ids (ADR 0120): each one's handle to its
//! thread, the screen the UI keeps, and which window's size it takes. How each reads keys is
//! the dispatcher's, as key-reading state. A
//! terminal starts on a thread of its own, which finds the shell, since that looks at the disk
//! and may ask the user database, never on the UI thread.
//!
//! Must not depend on: `app`, `config` or `dispatch`. The app gives the settings, and wakes
//! itself through the functions it hands in.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use oso_core::buffer::BufferId;
use oso_core::window::WindowId;
use oso_platform::process::pty::Size;
use oso_platform::process::{self, Executable};
use oso_term::{End, Palette, Start, Terminal};

use super::grid::Grid;
use super::route::{Input, ToProgram};
use super::shell::{self, Settings};

/// The most sends kept while a terminal starts.
const WAITING_MAX: usize = 256;

/// One hosted terminal.
pub(crate) struct Hosted {
    /// Its thread, once the start thread handed it over; `None` before, and after the end.
    pub terminal: Option<Terminal>,
    pub grid: Grid,
    /// The window whose size the PTY takes: the last that selected it (ADR 0120).
    pub owner: Option<WindowId>,
    /// The size last given to the PTY.
    pub sized: Option<Size>,
    /// The program's title (OSC 0, 1, 2), already cleaned by the policy.
    pub title: String,
    /// The OSC 9;4 progress the program reported last, for agent sessions (ADR 0121).
    pub progress: Option<oso_term::Progress>,
    /// How the program ended, for the modeline: `exited 0`, `ended`. A hosted session's buffer
    /// says what to do next instead, such as `stopped: session-resume to continue` (ADR 0121).
    pub ended: Option<String>,
    /// A paste of several lines waits for the person's yes to the question asked, without
    /// bracketed paste.
    pub pending_paste: Option<(oso_core::minibuffer::PromptId, String)>,
    /// A snapshot is ready to take.
    pub ready: bool,
    /// When it was last selected, by the app's count, for `terminal`'s "last".
    pub selected: u64,
    /// Whether the program was last told it has the focus (mode 1004).
    pub focused: Option<bool>,
    /// The note the modeline was last given, so that it is set only on a change.
    pub note: String,
    /// The title or the end changed since the note was made.
    pub note_stale: bool,
    /// The input mode the note was made with.
    pub noted_input: Input,
    /// What was typed or pasted while the shell is found, sent once it starts.
    pub waiting: Vec<ToProgram>,
}

impl Hosted {
    fn new() -> Hosted {
        Hosted {
            terminal: None,
            grid: Grid::default(),
            owner: None,
            sized: None,
            title: String::new(),
            progress: None,
            ended: None,
            pending_paste: None,
            ready: false,
            selected: 0,
            focused: None,
            note: String::new(),
            note_stale: false,
            noted_input: Input::SemiChar,
            waiting: Vec::new(),
        }
    }

    /// Keeps `send` until the terminal starts. Returns false, keeping nothing, past
    /// [`WAITING_MAX`] sends.
    pub(crate) fn wait(&mut self, send: ToProgram) -> bool {
        if self.waiting.len() >= WAITING_MAX {
            return false;
        }
        self.waiting.push(send);
        true
    }

    /// Gives the PTY `size` once it runs, if it differs from the size it has. Before it
    /// starts, nothing is sent and nothing noted, so that the size it started with is compared
    /// again once it runs.
    pub(crate) fn resize(&mut self, size: Size) {
        if let Some(terminal) = &self.terminal
            && self.sized != Some(size)
        {
            terminal.resize(size);
            self.sized = Some(size);
        }
    }

    /// Whether a program runs: started, and not ended.
    pub(crate) fn running(&self) -> bool {
        self.ended.is_none()
    }

    /// What the modeline says after `Terminal`: oso's own marks, the input mode and how the
    /// program ended, then the program's title last and in quotes, so that a title can never
    /// pass for one of them.
    pub(crate) fn note_text(&self, input: Input) -> String {
        let mut note = String::new();
        let mut push = |part: &str| {
            if !note.is_empty() {
                note.push_str("  ");
            }
            note.push_str(part);
        };
        if input == Input::Char {
            push("char");
        }
        if let Some(ended) = &self.ended {
            push(ended);
        }
        if !self.title.is_empty() {
            // The title's own quotation marks become plain ones, so that it never closes the
            // quotes oso put around it.
            let title = self.title.replace(['\u{201c}', '\u{201d}'], "\"");
            push(&format!("\u{201c}{title}\u{201d}"));
        }
        note
    }
}

/// Every hosted terminal.
#[derive(Default)]
pub(crate) struct Terminals {
    map: HashMap<BufferId, Hosted>,
    /// Counts selections, for "the last selected".
    clock: u64,
}

impl Terminals {
    /// Adds a terminal for `buffer`, which starts on its own thread.
    pub(crate) fn add(&mut self, buffer: BufferId) -> &mut Hosted {
        self.clock += 1;
        let hosted = self.map.entry(buffer).or_insert_with(Hosted::new);
        hosted.selected = self.clock;
        hosted
    }

    /// Starts `buffer`'s terminal over, with an empty screen and no program yet, keeping the
    /// window whose size it takes: a stopped session resumed in its buffer (ADR 0121).
    pub(crate) fn restart(&mut self, buffer: BufferId) -> &mut Hosted {
        let owner = self.map.get(&buffer).and_then(|hosted| hosted.owner);
        self.map.insert(buffer, Hosted::new());
        let hosted = self.add(buffer);
        hosted.owner = owner;
        hosted
    }

    pub(crate) fn get(&self, buffer: BufferId) -> Option<&Hosted> {
        self.map.get(&buffer)
    }

    pub(crate) fn get_mut(&mut self, buffer: BufferId) -> Option<&mut Hosted> {
        self.map.get_mut(&buffer)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (BufferId, &Hosted)> {
        self.map.iter().map(|(id, hosted)| (*id, hosted))
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (BufferId, &mut Hosted)> {
        self.map.iter_mut().map(|(id, hosted)| (*id, hosted))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Notes that `window` selected `buffer`'s terminal: its PTY takes that window's size.
    pub(crate) fn selected(&mut self, buffer: BufferId, window: WindowId) {
        self.clock += 1;
        if let Some(hosted) = self.map.get_mut(&buffer) {
            hosted.selected = self.clock;
            hosted.owner = Some(window);
        }
    }

    /// The terminal still running that was selected last, for `terminal` (ADR 0120, Q2).
    pub(crate) fn last_running(&self) -> Option<BufferId> {
        self.map
            .iter()
            .filter(|(_, hosted)| hosted.running())
            .max_by_key(|(_, hosted)| hosted.selected)
            .map(|(id, _)| *id)
    }

    /// Drops the terminals whose buffers are gone, which hangs each up. `alive` says which
    /// buffers still are.
    pub(crate) fn retain(
        &mut self,
        mut alive: impl FnMut(BufferId) -> bool,
    ) -> Vec<(BufferId, Hosted)> {
        let gone: Vec<BufferId> = self.map.keys().copied().filter(|id| !alive(*id)).collect();
        gone.into_iter()
            .filter_map(|id| Some((id, self.map.remove(&id)?)))
            .collect()
    }
}

/// What a terminal starts with, from the app.
pub(crate) struct Request {
    pub buffer: BufferId,
    pub settings: Settings,
    pub cwd: Option<PathBuf>,
    pub project: Option<PathBuf>,
    pub size: Size,
    pub palette: Palette,
}

/// What the start thread found.
#[derive(Debug)]
pub(crate) struct Started {
    pub buffer: BufferId,
    pub outcome: io::Result<Launched>,
}

/// A terminal that started, and what to say of it.
#[derive(Debug)]
pub(crate) struct Launched {
    pub terminal: Terminal,
    /// The directory it started in, if one was given.
    pub cwd: Option<PathBuf>,
    /// The shell's path, for `*Messages*` (ADR 0120, step 4's Q9).
    pub shell: PathBuf,
    /// Shells passed over before it, and why.
    pub passed_over: Vec<String>,
}

/// Starts `request`'s terminal from a thread of its own, which finds the shell, then hands
/// the terminal to `started`. `ready` is called from the terminal's thread whenever a
/// snapshot is ready.
///
/// # Errors
///
/// When no thread can start.
pub(crate) fn start(
    request: Request,
    started: Box<dyn FnOnce(Started) + Send>,
    ready: Box<dyn Fn() + Send>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name("oso-terminal-start".into())
        .spawn(move || {
            let buffer = request.buffer;
            let outcome = launch(request, ready);
            started(Started { buffer, outcome });
        })
        .map(drop)
}

/// The shell a terminal runs: `[terminal] shell`, `$SHELL` if absolute, the account's, then
/// `/bin/sh`, the first that is an executable (ADR 0120). A session's terminal finds it so too.
///
/// # Errors
///
/// When none is an executable, the last one's error.
pub(crate) fn find_shell(settings: &shell::Settings) -> io::Result<shell::Resolved<Executable>> {
    let candidates = shell::candidates(
        settings.shell.as_deref(),
        std::env::var_os("SHELL").as_deref(),
        process::account_shell,
    );
    shell::resolve(candidates, |path| Executable::absolute(path))
}

fn launch(request: Request, ready: Box<dyn Fn() + Send>) -> io::Result<Launched> {
    let Request {
        settings,
        cwd,
        project,
        size,
        palette,
        ..
    } = request;
    let found = find_shell(&settings)?;
    let start = Start {
        executable: found.executable,
        args: shell::args(settings.login),
        cwd: cwd.clone(),
        project,
        inherit: shell::inherit(&settings.env),
        set: shell::set(&found.path),
        size,
        scrollback: settings.scrollback,
        palette,
    };
    let terminal = Terminal::start(start, ready)?;
    Ok(Launched {
        terminal,
        cwd,
        shell: found.path,
        passed_over: found.passed_over,
    })
}

/// How `end` reads in the modeline.
pub(crate) fn ended(end: &End) -> String {
    match end {
        End::Exited(status) => match status.code() {
            Some(code) => format!("exited {code}"),
            None => signal(status),
        },
        End::Closed => "ended".to_owned(),
        End::Failed(_) => "failed".to_owned(),
    }
}

#[cfg(unix)]
fn signal(status: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map_or_else(
        || "ended".to_owned(),
        |signal| format!("killed by signal {signal}"),
    )
}

#[cfg(not(unix))]
fn signal(_: &std::process::ExitStatus) -> String {
    "ended".to_owned()
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::BufferId;
    use oso_core::editor::Editor;

    use oso_platform::process::pty::Size;

    use super::{Input, Terminals, ToProgram};

    fn ids() -> (BufferId, BufferId) {
        let mut editor = Editor::default();
        (
            editor.add_terminal_buffer("*terminal*"),
            editor.add_terminal_buffer("*terminal*"),
        )
    }

    #[test]
    fn a_restart_empties_the_terminal_and_keeps_its_window() {
        let (a, _) = ids();
        let editor = Editor::default();
        let window = editor.window().id();
        let mut terminals = Terminals::default();
        terminals.add(a);
        terminals.selected(a, window);
        let hosted = terminals.get_mut(a).unwrap();
        hosted.ended = Some("exited 0".to_owned());
        hosted.title = "old".to_owned();
        let hosted = terminals.restart(a);
        assert!(hosted.running());
        assert!(hosted.title.is_empty());
        assert!(hosted.terminal.is_none());
        assert_eq!(hosted.owner, Some(window));
        assert_eq!(terminals.last_running(), Some(a));
    }

    #[test]
    fn the_last_running_is_the_one_selected_last() {
        let (a, b) = ids();
        let mut terminals = Terminals::default();
        terminals.add(a);
        terminals.add(b);
        assert_eq!(terminals.last_running(), Some(b));
        let window = Editor::default().window().id();
        terminals.selected(a, window);
        assert_eq!(terminals.last_running(), Some(a));
        terminals.get_mut(a).unwrap().ended = Some("exited 0".into());
        assert_eq!(terminals.last_running(), Some(b));
    }

    #[test]
    fn the_note_says_char_mode_the_end_and_the_title() {
        let (a, _) = ids();
        let mut terminals = Terminals::default();
        let hosted = terminals.add(a);
        assert_eq!(hosted.note_text(Input::SemiChar), "");
        hosted.title = "vim".into();
        hosted.ended = Some("exited 1".into());
        assert_eq!(
            hosted.note_text(Input::Char),
            "char  exited 1  \u{201c}vim\u{201d}"
        );
        // A title that reads as oso's own mark stays a title.
        hosted.ended = None;
        hosted.title = "exited 0".into();
        assert_eq!(
            hosted.note_text(Input::SemiChar),
            "\u{201c}exited 0\u{201d}"
        );
        // Nor can it close oso's quotes to put a mark of its own after them.
        hosted.title = "x\u{201d}  exited 0  \u{201c}y".into();
        assert_eq!(
            hosted.note_text(Input::SemiChar),
            "\u{201c}x\"  exited 0  \"y\u{201d}"
        );
    }

    #[test]
    fn a_size_given_while_it_starts_is_given_again_once_it_runs() {
        let (a, _) = ids();
        let mut terminals = Terminals::default();
        let hosted = terminals.add(a);
        let started = Size {
            rows: 24,
            columns: 80,
            width: 0,
            height: 0,
        };
        hosted.sized = Some(started);
        hosted.resize(Size {
            rows: 10,
            ..started
        });
        assert_eq!(hosted.sized, Some(started), "nothing sent, nothing noted");
    }

    #[test]
    fn what_is_typed_while_it_starts_waits_up_to_a_bound() {
        let (a, _) = ids();
        let mut terminals = Terminals::default();
        let hosted = terminals.add(a);
        for _ in 0..super::WAITING_MAX {
            assert!(hosted.wait(ToProgram::Text("a".into())));
        }
        assert!(!hosted.wait(ToProgram::Text("b".into())));
        assert_eq!(hosted.waiting.len(), super::WAITING_MAX);
    }

    #[test]
    fn gone_buffers_drop_their_terminals() {
        let (a, b) = ids();
        let mut terminals = Terminals::default();
        terminals.add(a);
        terminals.add(b);
        let dropped = terminals.retain(|id| id == b);
        assert_eq!(dropped.len(), 1);
        assert!(terminals.get(a).is_none());
        assert!(terminals.get(b).is_some());
    }
}
