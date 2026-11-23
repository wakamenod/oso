//! What is sent to a native session, carried out (ADR 0165, part 2f): the delivery path's
//! native route puts it into the session's composer at point, not sent, the person sending it
//! (the author's Q1 (a), as ADR 0121's Q17 keeps a terminal's paste unsubmitted); and a review's
//! question the person confirmed in oso's prompt goes at once, or waits behind the turn under
//! way, the composer's draft untouched (Q4 (a)): with `submit_paste`, the submits ADR 0163
//! allows. Text at the draft's end, as a suggestion goes (part 3f), is [`App::compose_after`]'s.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - Each is escaped once, the first two as `Outgoing` says, what goes into a draft also so that it makes
//!   no `@` of the person's after it the start of a word.
//! - Nothing goes into a composer that would then hold more than [`MAX_MESSAGE`], measured
//!   before anything is copied, nor into one a yank turned secret, which sends nothing; a
//!   stopped session takes no question, as it is resumed first.
//! - What goes at a draft's end never joins it, and nothing of Claude's goes after a command,
//!   as its argument.

use oso_core::buffer::{Buffer, BufferId, Mode, Replacement};
use oso_core::editor::Editor;
use oso_core::sum_tree::Bias;
use oso_core::window::DockSlot;

use super::HIDDEN;
use crate::app::App;
use crate::app::sessions::Outgoing;
use crate::sessions::native::queue::{MAX_MESSAGE, MAX_QUEUED, Refusal, Run, Sent};
use crate::sessions::send;
use crate::sessions::worker::SessionKey;

/// What a composer that would hold too much says.
fn too_much() -> String {
    format!(
        "The composer would hold over {} KiB: nothing was put in",
        MAX_MESSAGE >> 10
    )
}

/// Why `buffer`, a session's composer, takes no text of oso's, if it takes none.
fn refuses(buffer: &Buffer) -> Result<(), String> {
    if buffer.mode() != Mode::Composer {
        return Err("The composer is no longer one".to_owned());
    }
    if buffer.is_secret() {
        return Err("The composer holds secret text, which is not sent: take it out".to_owned());
    }
    Ok(())
}

/// `text`, escaped, as it goes at the end of `buffer`, a session's composer, or why it does
/// not.
fn at_end(buffer: &Buffer, text: &str) -> Result<String, String> {
    refuses(buffer)?;
    let draft = buffer.text();
    // Measured before it is copied: escaping never shortens it.
    let len = draft.len();
    if len.saturating_add(text.len()) > MAX_MESSAGE {
        return Err(too_much());
    }
    // Its argument would be Claude's text, which no escape holds there.
    if send::starts_command(draft.chunks().flat_map(str::chars)) {
        return Err("The draft is a command: nothing was put after it".to_owned());
    }
    if send::opens_mention(draft.chunks().flat_map(str::chars)) {
        return Err("The draft leaves an @\"…\" open: close it first".to_owned());
    }
    let from = draft.clip_offset(len.saturating_sub(char::MAX.len_utf8()), Bias::Left);
    let last = (draft.chunks_in_range(from..len))
        .flat_map(str::chars)
        .last();
    let text = send::after_draft(text, last).into_owned();
    if len.saturating_add(text.len()) > MAX_MESSAGE {
        return Err(too_much());
    }
    Ok(text)
}

/// Puts `text` at the end of `composer`, docked under `transcript`, with point there after it
/// and in view, so that the person sees it before it is sent; or why nothing was put in.
pub(super) fn put_at_end(
    editor: &mut Editor,
    (transcript, composer): (BufferId, BufferId),
    text: &str,
) -> Result<(), String> {
    if editor.prompt().is_some() {
        return Err("A prompt is open: nothing was put in".to_owned());
    }
    let window = (editor.dock_window(transcript, DockSlot::Composer))
        .filter(|window| editor.regions(*window).is_some())
        .ok_or_else(|| HIDDEN.to_owned())?;
    let at = editor
        .buffer(composer)
        .map_or(0, |buffer| buffer.text().len());
    editor.select_window(window);
    if editor.window().id() != window {
        return Err("Nothing was put in".to_owned());
    }
    let edit = Replacement {
        range: at..at,
        text,
    };
    (editor.apply_edits(composer, &[edit])).map_err(|_| "Nothing was put in".to_owned())?;
    let selections = &mut editor.current_mut().0.selections;
    selections.keep_primary();
    selections.move_primary(at + text.len());
    // Its rows wrap, and the view follows, as after a command: this runs after one.
    editor.refill_wraps();
    editor.follow_cursor();
    Ok(())
}

impl App {
    /// Puts `text` at the end of native session `key`'s composer, escaped, not sent, as
    /// [`put_at_end`] does; or why nothing was put in: into a composer not shown, nothing is, as
    /// the person could not read it.
    pub(in crate::app::sessions::hosting::native) fn compose_after(
        &mut self,
        key: SessionKey,
        text: &str,
    ) -> Result<(), String> {
        let transcript = (self.hosting.natives.map.get(&key)).map(|native| native.buffer);
        let transcript = transcript.ok_or_else(|| "That session is gone".to_owned())?;
        let id = (self.composer(key)).ok_or_else(|| "The session has no composer".to_owned())?;
        let buffer = (self.editor.buffer(id)).ok_or_else(|| "The composer is gone".to_owned())?;
        let text = at_end(buffer, text)?;
        put_at_end(&mut self.editor, (transcript, id), &text)
    }

    /// Puts `text` into native session `key`'s composer at its point, escaped as it says;
    /// with `show`, the session is shown first, point in its composer after the text. The
    /// composer's buffer, or why nothing was put in.
    pub(in crate::app) fn compose_into(
        &mut self,
        key: SessionKey,
        text: Outgoing<'_>,
        show: bool,
    ) -> Result<BufferId, String> {
        // `claude -p` runs no `/add-dir`, which the composer's send would carry unescaped.
        if let Outgoing::Command(_) = text {
            return Err("A native session takes no slash command of oso's".to_owned());
        }
        let transcript = (self.hosting.natives.map.get(&key)).map(|native| native.buffer);
        let transcript = transcript.ok_or_else(|| "That session is gone".to_owned())?;
        let id = (self.composer(key)).ok_or_else(|| "The session has no composer".to_owned())?;
        let buffer = (self.editor.buffer(id)).ok_or_else(|| "The composer is gone".to_owned())?;
        refuses(buffer)?;
        // Measured before it is copied: escaping never shortens it.
        let len = buffer.text().len();
        if len.saturating_add(text.len()) > MAX_MESSAGE {
            return Err(too_much());
        }
        let window = self.editor.dock_window(transcript, DockSlot::Composer);
        let cursor = (window.and_then(|id| self.editor.window_by_id(id)))
            .map(|window| window.selections.primary().cursor);
        let at = cursor.map_or(len, |cursor| cursor.min(len));
        let next = (buffer.text().chunks_in_range(at..len)).find_map(|chunk| chunk.chars().next());
        let text = text.escaped(next);
        if len.saturating_add(text.len()) > MAX_MESSAGE {
            return Err(too_much());
        }
        if show {
            self.show_session(key);
            self.composer_go(key, true);
        }
        let edit = Replacement {
            range: at..at,
            text: &text,
        };
        (self.editor.apply_edits(id, &[edit])).map_err(|_| "Nothing was put in".to_owned())?;
        Ok(id)
    }

    /// Sends `text`, which the person confirmed in oso's prompt, to native session `key`: at
    /// once, or queued behind the turn under way, escaped, the composer's draft and a paused
    /// queue left as they are. What to say, or why it was not sent: a stopped session is
    /// resumed first.
    pub(in crate::app) fn send_confirmed(
        &mut self,
        key: SessionKey,
        text: &str,
    ) -> Result<String, String> {
        let turn = self
            .turn(key)
            .ok_or_else(|| "That session is gone".to_owned())?;
        if turn.run == Run::Ended {
            let resume = self.dispatcher.where_is(&self.editor, "session-resume");
            return Err(format!("it has stopped: {resume} first"));
        }
        // Measured before it is copied.
        if text.len() > MAX_MESSAGE {
            return Err(format!("it is over {} KiB", MAX_MESSAGE >> 10));
        }
        let text = send::for_claude(text, None).into_owned();
        let native = (self.hosting.natives.map.get_mut(&key))
            .ok_or_else(|| "That session is gone".to_owned())?;
        let paused = native.composing.queue.paused().is_some();
        let said = match native.composing.queue.send_confirmed(text, turn) {
            Sent::Now(text) if self.deliver(key, &text) => Ok("sent".to_owned()),
            Sent::Now(_) => {
                self.not_sent(key, None);
                Err("its input took nothing".to_owned())
            }
            Sent::Queued(count) if paused => {
                let send = self
                    .dispatcher
                    .where_is(&self.editor, "native-composer-send");
                Ok(format!(
                    "queued as {count}, behind a paused queue: {send} in its composer goes on"
                ))
            }
            Sent::Queued(count) => Ok(format!("queued as {count}, to go once the turn ends")),
            Sent::Refused(Refusal::TooLong) => Err(format!("it is over {} KiB", MAX_MESSAGE >> 10)),
            Sent::Refused(Refusal::Full) => Err(format!("its queue holds {MAX_QUEUED} already")),
            _ => Err("it took nothing".to_owned()),
        };
        self.draw_queue(key);
        said
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composer(text: &str) -> Buffer {
        let mut buffer = Buffer::from_text("*compose: a*", text);
        buffer.set_mode(Mode::Composer);
        buffer
    }

    #[test]
    fn text_at_a_drafts_end_is_escaped_and_never_joins_it() {
        let after = |draft: &str, text: &str| at_end(&composer(draft), text);
        assert_eq!(
            after("", "/review @a.md").as_deref(),
            Ok(r" /review \@a.md")
        );
        assert_eq!(after("see @", "etc/hosts").as_deref(), Ok(" etc/hosts"));
        assert_eq!(after("one\n", "two").as_deref(), Ok("two"));
        assert_eq!(after("  ", "/clear").as_deref(), Ok(" /clear"));
        assert_eq!(after("テストを", "実行").as_deref(), Ok(" 実行"));
        // A draft read by its last character, however many bytes it takes.
        assert_eq!(after("x\u{3000}", "y").as_deref(), Ok("y"));
        assert_eq!(after("a🙂", "y").as_deref(), Ok(" y"));
    }

    #[test]
    fn nothing_goes_after_a_command() {
        for draft in ["/", "/model ", "!", "! ls", "# note", "\u{200b}/x"] {
            assert!(at_end(&composer(draft), "x").is_err(), "{draft:?}");
        }
        // One whose escape the person took out is a command; an attached file is not, and is
        // left as it is.
        assert!(at_end(&composer("/review @notes.md"), "x").is_err());
        assert_eq!(at_end(&composer("see @notes.md"), "x").as_deref(), Ok(" x"));
        // Nor into an open `@"`, whose file Claude's text would name.
        assert!(at_end(&composer("see @\"a "), "b\" c").is_err());
        // One escaped takes it.
        assert_eq!(at_end(&composer(" /review"), "x").as_deref(), Ok(" x"));
    }

    #[test]
    fn a_secret_composer_one_that_is_none_and_one_too_full_take_nothing() {
        let mut secret = Buffer::visiting(std::path::PathBuf::from("/tmp/.env"));
        secret.set_mode(Mode::Composer);
        assert!(secret.is_secret());
        assert!(at_end(&secret, "x").is_err());
        assert!(at_end(&Buffer::from_text("plain", ""), "x").is_err());
        let full = "a".repeat(MAX_MESSAGE - 1);
        assert!(at_end(&composer(&full), "x").is_err(), "its space");
        let full = format!("{}\n", "a".repeat(MAX_MESSAGE - 2));
        assert_eq!(at_end(&composer(&full), "x").as_deref(), Ok("x"));
    }
}
