//! A native session's prompt suggestion in the app (ADR 0165, part 3f; the author's Q-F1 (a)
//! and Q-F2 (a)): the CLI's suggestion, given once a turn ended, kept and drawn as a dim row
//! after the queue's at the transcript's end until a turn starts, the person sends or the
//! process ends; and `M-n` past the newest message sent puts it into an empty composer, not
//! sent. Which suggestion is kept, and its row, are `sessions::native::suggest`'s.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - The suggestion is Claude's text that oso puts into the composer, so it is escaped as
//!   composed text is (part 2f, `send::for_claude`): a suggested `/command` or `@path` runs
//!   nothing unless the person edits it so. Only a composer that takes composed text takes it,
//!   as `compose_into`'s.

use oso_core::buffer::{Buffer, Mode};

use super::Native;
use crate::app::App;
use crate::sessions::native::suggest;
use crate::sessions::native::turns;
use crate::sessions::native::wire::Event;
use crate::sessions::send;
use crate::sessions::state::Target;
use crate::sessions::worker::SessionKey;

/// Notes what `event` of a native session says of its suggestion. Whether the suggestion
/// changed, so that the transcript's end is drawn again.
pub(super) fn suggestion_event(native: &mut Native, event: &Event) -> bool {
    let before = native.composing.suggestion.is_some();
    match event {
        // Only an idle session's: one that comes after a turn started is old.
        Event::Suggestion(text) if !native.composing.busy => {
            native.composing.suggestion = suggest::suggestion(text);
            return before || native.composing.suggestion.is_some();
        }
        _ => {}
    }
    if turns::target(event).is_some_and(|target| !matches!(target, Target::Idle { .. })) {
        native.composing.suggestion = None;
    }
    before && native.composing.suggestion.is_none()
}

/// Whether composer `buffer` takes the suggestion: empty, still a composer, and not turned
/// secret by a yank, as `compose_into` asks.
fn takes(buffer: &Buffer) -> bool {
    buffer.mode() == Mode::Composer && !buffer.is_secret() && buffer.text().is_empty()
}

impl App {
    /// `M-n` past the newest message sent: native session `key`'s suggestion, escaped, into
    /// its composer if that is empty, still a composer and not turned secret. Whether it was
    /// put in.
    pub(super) fn take_suggestion(&mut self, key: SessionKey) -> bool {
        let Some(text) = (self.hosting.natives.map.get(&key))
            .and_then(|native| native.composing.suggestion.clone())
        else {
            return false;
        };
        let Some(id) = self.composer(key) else {
            return false;
        };
        let takes = self.editor.buffer(id).is_some_and(takes);
        if takes {
            self.set_composer_text(key, &send::for_claude(&text, None));
        }
        takes
    }
}

#[cfg(test)]
mod tests {
    use oso_core::editor::Editor;

    use super::*;

    fn native() -> Native {
        let mut editor = Editor::with_buffer(Buffer::from_text("*scratch*", ""));
        super::super::markdown::tests::native(editor.add_native_session_buffer("*claude: a*"))
    }

    fn replay() -> Event {
        crate::sessions::native::wire::parse(
            br#"{"type":"user","isReplay":true,"message":{"content":"go"}}"#,
        )
    }

    #[test]
    fn an_idle_sessions_suggestion_is_kept_until_a_turn_starts() {
        let mut native = native();
        assert!(suggestion_event(
            &mut native,
            &Event::Suggestion("run it".into())
        ));
        assert_eq!(native.composing.suggestion.as_deref(), Some("run it"));
        // A turn's end changes nothing; one that starts clears it.
        assert!(!suggestion_event(
            &mut native,
            &Event::Other("system/x".into())
        ));
        assert!(suggestion_event(&mut native, &replay()));
        assert_eq!(native.composing.suggestion, None);
        assert!(
            !suggestion_event(&mut native, &replay()),
            "nothing to clear"
        );
    }

    #[test]
    fn a_suggestion_after_a_turn_started_or_one_hidden_is_not_kept() {
        let mut native = native();
        native.composing.busy = true;
        assert!(!suggestion_event(
            &mut native,
            &Event::Suggestion("old".into())
        ));
        assert_eq!(native.composing.suggestion, None);
        native.composing.busy = false;
        assert!(!suggestion_event(
            &mut native,
            &Event::Suggestion("a\nb".into())
        ));
        assert_eq!(native.composing.suggestion, None);
        // A hidden one replaces one kept.
        suggestion_event(&mut native, &Event::Suggestion("kept".into()));
        assert!(suggestion_event(
            &mut native,
            &Event::Suggestion("a\u{202e}b".into())
        ));
        assert_eq!(native.composing.suggestion, None);
    }

    #[test]
    fn only_an_empty_composer_takes_the_suggestion() {
        let composer = |text: &str, mode: Mode| {
            let mut buffer = Buffer::from_text("*compose: a*", text);
            buffer.set_mode(mode);
            buffer
        };
        assert!(takes(&composer("", Mode::Composer)));
        assert!(!takes(&composer("draft", Mode::Composer)), "a draft");
        assert!(!takes(&composer("", Mode::Fundamental)), "not a composer");
        let mut secret = Buffer::visiting(std::path::PathBuf::from("/tmp/.env"));
        secret.set_mode(Mode::Composer);
        assert!(secret.is_secret() && secret.text().is_empty());
        assert!(!takes(&secret), "turned secret");
    }
}
