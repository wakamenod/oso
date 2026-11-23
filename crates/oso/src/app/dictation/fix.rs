//! A dictation's fix in the app (ADR 0175, step 2): once its text is typed as a provisional
//! span, a thread of its own reads TypeWhisper's dictionary, asks the person's `claude -p` for
//! fixes, and the app makes them inside the span, in one undo step.
//!
//! Must not depend on: how a frame is drawn.
//!
//! - The span is drawn in its face only while the fix runs: it goes on the fix, a failure, a
//!   skip, a cancel or an exit.
//! - What leaves the machine, through the person's `claude` login: the dictated text, the lines
//!   around it, the mode's name and the capped dictionary. Nothing of a buffer that may hold
//!   secrets: the editor gives no context for one, and refuses the fix if it turned secret.
//! - The answer's words are shown in the echo area only, withheld from the dev socket and the
//!   agent channel, and never in `*Messages*` or the log: they are the person's and Claude's.
//! - An answer that comes while another dictation is under way waits for that one's text to
//!   be placed: a fix changes the buffer, which would send that text to the kill ring.
//! - At most [`MAX_RUNNING`] fixes run at once: a dictation past them is typed plain.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use oso_core::editor::{DictationMark, DictationSpan};

use super::App;
use crate::app::UserEvent;
use crate::dictation::api::{Call, Curl, Transport};
use crate::dictation::discovery;
use crate::dictation::fix::{self, Applied, Ask, Confidence, Dictionary};
use crate::sessions::oneshot::{self, Oneshot, Running, Unread};

/// The most fixes that run at once, each a whole `claude`.
const MAX_RUNNING: usize = 4;

/// A fix under way.
pub(super) struct Fixing {
    id: u64,
    span: DictationSpan,
    /// The text sent, which the span must still hold when the answer comes.
    sent: String,
    running: Running,
    /// Its answer, held while another dictation is under way.
    answer: Option<Result<Applied, String>>,
}

/// What a fix's thread takes with it.
struct Job {
    id: u64,
    context: oso_core::editor::DictationContext,
    discovery: Option<PathBuf>,
    curl: PathBuf,
    model: String,
    effort: String,
    /// The least confidence a fix is made with.
    min: Confidence,
    terminal: crate::terminal::shell::Settings,
    claude: Option<PathBuf>,
    session_env: Vec<String>,
    state: PathBuf,
    running: Running,
}

impl App {
    /// Starts the fix of `span`, which the text just typed holds; or ends the span, saying why,
    /// if it cannot start.
    pub(super) fn start_dictation_fix(&mut self, span: DictationSpan) {
        let settings = self.configuration.dictation().clone();
        let context =
            self.editor
                .dictation_context(span, settings.fix_context_lines(), fix::MAX_SIDE);
        let Some(context) = context else {
            return self.editor.end_dictation_span(span);
        };
        let Some(state) = self.hosting.state_directory() else {
            self.editor.end_dictation_span(span);
            return self
                .editor
                .message("Dictation typed plain: a fix needs oso's state directory");
        };
        if self.dictations.fixes.len() >= MAX_RUNNING {
            self.editor.end_dictation_span(span);
            return self.editor.message(format!(
                "Dictation typed plain: {MAX_RUNNING} fixes run already"
            ));
        }
        let id = self.dictations.next_fix;
        self.dictations.next_fix += 1;
        let running = Running::default();
        let sent = context.text.clone();
        let job = Job {
            id,
            context,
            discovery: self.discovery_path(),
            curl: settings.curl(),
            model: settings.fix_model().to_owned(),
            effort: settings.fix_effort().to_owned(),
            min: settings.fix_min_confidence(),
            terminal: self.terminal_settings(),
            claude: self.configuration.session_claude().map(PathBuf::from),
            session_env: self.configuration.session_env().to_vec(),
            state,
            running: Arc::clone(&running),
        };
        let waker = self.waker.clone();
        let started = thread::Builder::new()
            .name("oso-dictation-fix".into())
            .spawn(move || {
                let id = job.id;
                waker.send(UserEvent::DictationFix(id, run(&job)));
            });
        if let Err(e) = started {
            log::error!("cannot start a dictation fix's thread: {e}");
            self.editor.end_dictation_span(span);
            return self.editor.message("Dictation could not start its fix");
        }
        self.dictations.fixes.push(Fixing {
            id,
            span,
            sent,
            running,
            answer: None,
        });
        self.show_dictation_mark();
    }

    /// The fix `id` answered. While another dictation is under way, the answer waits for its
    /// text to be placed; otherwise it lands at once.
    pub(in crate::app) fn dictation_fixed(&mut self, id: u64, answer: Result<Applied, String>) {
        let Some(fixing) = self.dictations.fixes.iter_mut().find(|fix| fix.id == id) else {
            return;
        };
        fixing.answer = Some(answer);
        self.land_dictation_fixes();
    }

    /// Lands the answers that came, unless a dictation is under way, whose target a fix's edit
    /// would make stale.
    pub(super) fn land_dictation_fixes(&mut self) {
        let under_way = self.dictations.due.is_some()
            || self
                .dictations
                .current
                .as_ref()
                .is_some_and(|current| !current.cancelled);
        if under_way || self.dictations.fixes.iter().all(|fix| fix.answer.is_none()) {
            return;
        }
        let (answered, running) = std::mem::take(&mut self.dictations.fixes)
            .into_iter()
            .partition(|fix| fix.answer.is_some());
        self.dictations.fixes = running;
        for fixing in answered {
            self.land_dictation_fix(fixing);
        }
        self.show_dictation_mark();
        self.request_redraw();
    }

    /// Makes `fixing`'s fixes at the threshold inside its span, in one undo step, and shows the
    /// rest; or, if it failed, keeps the text as it was typed.
    fn land_dictation_fix(&mut self, fixing: Fixing) {
        let Some(answer) = fixing.answer else {
            return;
        };
        let applied = match answer {
            Ok(applied) => applied,
            Err(why) => {
                self.editor.end_dictation_span(fixing.span);
                return self
                    .editor
                    .message(format!("Dictation kept as typed: the fix failed: {why}"));
            }
        };
        let edits: Vec<(std::ops::Range<usize>, &str)> = applied
            .edits
            .iter()
            .map(|(range, text)| (range.clone(), text.as_str()))
            .collect();
        let fixed = if edits.is_empty() {
            self.editor.end_dictation_span(fixing.span);
            Ok(())
        } else {
            self.editor.fix_dictation(fixing.span, &fixing.sent, &edits)
        };
        match fixed {
            // Its words are the person's and Claude's: in the echo area alone, withheld from
            // the dev socket and agents, and never in `*Messages*`.
            Ok(()) => self.editor.echo_withheld_text(fix::report(&applied)),
            Err(why) => self.editor.message(format!("Dictation fix skipped: {why}")),
        }
        self.editor.follow_cursor();
        self.dispatcher.publish_for_emergency(&self.editor);
    }

    /// Stops every fix under way, as `dictation-cancel` or an exit does: each `claude` is
    /// killed, its thread reaps it, and the text stays as it was typed. Says whether there was
    /// one.
    pub(in crate::app) fn stop_dictation_fixes(&mut self) -> bool {
        let fixes = std::mem::take(&mut self.dictations.fixes);
        for fixing in &fixes {
            oneshot::kill(&fixing.running);
            self.editor.end_dictation_span(fixing.span);
        }
        self.show_dictation_mark();
        !fixes.is_empty()
    }

    /// How many fixes are under way, for the dev socket's state.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn dictation_fixes(&self) -> usize {
        self.dictations.fixes.len()
    }

    /// The modeline's mark for what is under way: the dictation's own, else `fix` while one
    /// runs.
    pub(super) fn show_dictation_mark(&mut self) {
        let own = match &self.dictations.current {
            Some(current) if !current.cancelled => current.mark(),
            _ => DictationMark::Off,
        };
        let mark = match own {
            DictationMark::Off if !self.dictations.fixes.is_empty() => DictationMark::Fixing,
            own => own,
        };
        self.editor.set_dictation_mark(mark);
    }
}

/// A fix's run, on its thread: the dictionary, then `claude`, then the edits its answer makes
/// in the text sent.
fn run(job: &Job) -> Result<Applied, String> {
    let dictionary = job
        .discovery
        .as_deref()
        .map(|path| read_dictionary(path, &job.curl))
        .unwrap_or_default();
    let context = &job.context;
    let input = fix::input(&Ask {
        mode: context.mode,
        before: &context.before,
        target: &context.text,
        after: &context.after,
        dictionary: &dictionary,
    });
    let name = format!("{}-{}", std::process::id(), job.id);
    let answer = oneshot::run(&Oneshot {
        terminal: &job.terminal,
        claude: job.claude.as_deref(),
        session_env: &job.session_env,
        state: &job.state,
        directory: fix::DIRECTORY,
        name: &name,
        arguments: oneshot::arguments(fix::options(&job.model, &job.effort)),
        input: &input,
        limit: fix::LIMIT,
        max_answer: fix::MAX_ANSWER,
        running: &job.running,
    })?;
    let answer = oneshot::structured(&answer).map_err(|unread| {
        match unread {
            Unread::NotJson => "its answer was not JSON",
            Unread::Failed => "it said it failed",
            Unread::Empty => "its answer held no fixes",
        }
        .to_owned()
    })?;
    let fixes = fix::fixes(&answer)?;
    Ok(fix::apply(&context.text, &fixes, job.min))
}

/// TypeWhisper's dictionary, as far as it answers within each read's limit: none if it does
/// not, as a fix goes on without it.
fn read_dictionary(path: &std::path::Path, curl: &std::path::Path) -> Dictionary {
    let Ok(discovery) = discovery::read(path) else {
        return Dictionary::default();
    };
    let Ok(mut curl) = Curl::at(curl) else {
        return Dictionary::default();
    };
    let mut read = |call: &Call| match curl.call(&discovery, call) {
        Ok(answer) if answer.status == 200 => Some(answer.body),
        _ => None,
    };
    Dictionary {
        terms: read(&Call::Terms)
            .map(|body| fix::terms(&body))
            .unwrap_or_default(),
        corrections: read(&Call::Corrections)
            .map(|body| fix::corrections(&body))
            .unwrap_or_default(),
    }
}
