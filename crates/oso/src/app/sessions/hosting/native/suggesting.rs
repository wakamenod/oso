//! A native session's prompt suggestion in the app (ADR 0165, part 3f; the author's Q-F1 (a)
//! and Q-F2 (a), amended 2026-11-24): the CLI's suggestion, given once a turn ended, kept until
//! a turn starts, the person sends or the process ends, which the empty composer's placeholder
//! shows meanwhile (`composer/placeholder.rs`); `C-c C-s` puts it at the composer's end, and
//! `M-n` past the newest message sent into an empty composer, not sent. Which suggestion is
//! kept is `sessions::native::suggest`'s.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - The suggestion is Claude's text that oso puts into the composer, so it is escaped as
//!   composed text is (part 2f, `send::for_claude`): a suggested `/command` or `@path` runs
//!   nothing unless the person edits it so. Only a composer that takes composed text takes it,
//!   as `compose_into`'s.

use super::Native;
use crate::app::App;
use crate::app::sessions::empty_composer;
use crate::sessions::native::suggest;
use crate::sessions::native::turns;
use crate::sessions::native::wire::Event;
use crate::sessions::send;
use crate::sessions::state::Target;
use crate::sessions::worker::SessionKey;

/// Notes what `event` of a native session says of its suggestion. Whether the suggestion
/// changed, so that the composer's placeholder is drawn again.
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

impl App {
    /// `C-c C-s`: native session `key`'s suggestion, escaped, at the end of its composer, with
    /// point after it, or why not.
    pub(super) fn suggestion_to_composer(&mut self, key: SessionKey) {
        let text = (self.hosting.natives.map.get(&key))
            .and_then(|native| native.composing.suggestion.clone());
        let taken = match text {
            Some(text) => self.compose_after(key, &text),
            None => Err("No suggestion to take".to_owned()),
        };
        if let Err(why) = taken {
            self.editor.message(why);
        }
    }

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
        let takes = self.editor.buffer(id).is_some_and(empty_composer);
        if takes {
            self.set_composer_text(key, &send::for_claude(&text, None));
        }
        takes
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::{Buffer, Mode};

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
    fn a_composers_placeholder_has_its_own_sessions_suggestion() {
        use std::collections::HashMap;
        let mut editor = Editor::with_buffer(Buffer::from_text("*scratch*", ""));
        let mut session = |name: &str, suggestion: Option<&str>| {
            let transcript = editor.add_native_session_buffer(name);
            let composer = editor.add_buffer(Buffer::from_text("*compose*", ""));
            let mut native = super::super::markdown::tests::native(transcript);
            native.composing.buffer = Some(composer);
            native.composing.suggestion = suggestion.map(str::to_owned);
            (composer, native)
        };
        let (with, first) = session("*claude: a*", Some("run it"));
        let (without, second) = session("*claude: b*", None);
        let natives = super::super::NativeSessions {
            map: HashMap::from([(1, first), (2, second)]),
            ..Default::default()
        };
        assert_eq!(natives.suggested(with), Some("run it"));
        assert_eq!(natives.suggested(without), None);
    }

    #[test]
    fn only_an_empty_composer_takes_the_suggestion() {
        let composer = |text: &str, mode: Mode| {
            let mut buffer = Buffer::from_text("*compose: a*", text);
            buffer.set_mode(mode);
            buffer
        };
        assert!(empty_composer(&composer("", Mode::Composer)));
        assert!(
            !empty_composer(&composer("draft", Mode::Composer)),
            "a draft"
        );
        assert!(
            !empty_composer(&composer("", Mode::Fundamental)),
            "not a composer"
        );
        let mut secret = Buffer::visiting(std::path::PathBuf::from("/tmp/.env"));
        secret.set_mode(Mode::Composer);
        assert!(secret.is_secret() && secret.text().is_empty());
        assert!(!empty_composer(&secret), "turned secret");
    }
}
