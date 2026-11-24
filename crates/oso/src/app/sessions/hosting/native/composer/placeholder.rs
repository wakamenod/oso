//! What an empty composer's placeholder says (ADR 0170, as amended): that it writes to Claude,
//! and the keys that send and that go back to the transcript, as they are bound now; or, while
//! its session has one, the CLI's prompt suggestion and the key that takes it (ADR 0165, part
//! 3f).
//!
//! Must not depend on: how a frame is drawn: the drawing decides where and whether it shows.

use oso_core::buffer::BufferId;

use super::super::NativeSessions;
use crate::app::App;
use crate::dispatch::Dispatcher;

/// What oso says in an empty composer's placeholder, with the keys it names.
#[derive(Debug)]
pub(in crate::app) struct Placeholder {
    /// With no suggestion: the whole of it first and then with fewer of its parts, for a
    /// narrow window.
    pub(in crate::app) usual: [String; 3],
    /// What follows a suggestion.
    pub(in crate::app) takes: String,
}

impl NativeSessions {
    /// What oso says in an empty composer's placeholder, or nothing before one was drawn.
    pub(in crate::app) fn placeholder(&self) -> Option<&Placeholder> {
        self.placeholder.as_ref()
    }

    /// The prompt suggestion of the session whose composer `composer` is, if it has one.
    pub(in crate::app) fn suggested(&self, composer: BufferId) -> Option<&str> {
        let native = self.map.get(&self.composer_session(composer)?)?;
        native.composing.suggestion.as_deref()
    }

    /// Forgets the keys the placeholder names, as the user's keys or menus changed.
    pub(in crate::app) fn forget_placeholder(&mut self) {
        self.placeholder = None;
    }
}

impl App {
    /// Names the composer's keys for its placeholder, before a frame that may show one: once,
    /// until they are forgotten.
    pub(in crate::app) fn note_placeholder(&mut self) {
        let natives = &mut self.hosting.natives;
        if natives.placeholder.is_none() && !natives.map.is_empty() {
            natives.placeholder = Some(texts(&self.dispatcher));
        }
    }
}

/// The placeholder, with `dispatcher`'s keys.
fn texts(dispatcher: &Dispatcher) -> Placeholder {
    let take = dispatcher.where_is_in_composer("native-session-take-suggestion");
    let send = dispatcher.where_is_in_composer("native-composer-send");
    let back = dispatcher.where_is_in_composer("native-composer-transcript");
    let what = "Write to Claude".to_owned();
    let sends = format!("{what} · {send} sends");
    Placeholder {
        usual: [format!("{sends} · {back} transcript"), sends, what],
        takes: format!("   ({take} takes it)"),
    }
}

#[cfg(test)]
impl NativeSessions {
    /// Sessions whose placeholder says `usual` or `takes`, one of them with the suggestion of
    /// the composer in `suggested`.
    pub(in crate::app) fn saying(
        usual: [&str; 3],
        takes: &str,
        suggested: Option<(BufferId, &str)>,
    ) -> Self {
        let mut natives = Self {
            placeholder: Some(Placeholder {
                usual: usual.map(str::to_owned),
                takes: takes.to_owned(),
            }),
            ..Self::default()
        };
        if let Some((composer, text)) = suggested {
            let mut native = super::super::markdown::tests::native(composer);
            native.composing.buffer = Some(composer);
            native.composing.suggestion = Some(text.to_owned());
            natives.map.insert(1, native);
        }
        natives
    }
}

#[cfg(test)]
mod tests {
    use oso_core::keymap::Keymap;

    use super::texts;
    use crate::dispatch::Dispatcher;

    #[test]
    fn the_placeholder_names_the_keys_as_bound() {
        assert_eq!(
            texts(&Dispatcher::new()).usual,
            [
                "Write to Claude · C-c C-c sends · C-c C-o transcript",
                "Write to Claude · C-c C-c sends",
                "Write to Claude",
            ]
        );
        assert_eq!(texts(&Dispatcher::new()).takes, "   (C-c C-s takes it)");
    }

    #[test]
    fn the_users_shorter_key_is_named_and_the_composers_own_win() {
        let mut dispatcher = Dispatcher::new();
        let keymap = Keymap::from_table(
            &crate::commands::registry(),
            &[
                ("C-RET", "native-composer-send"),
                ("C-c C-o", "undo"),
                ("M-RET", "native-session-take-suggestion"),
            ],
        );
        dispatcher.set_user_keymap(keymap.unwrap());
        let texts = texts(&dispatcher);
        assert_eq!(
            texts.usual[0],
            "Write to Claude · C-RET sends · C-c C-o transcript"
        );
        assert_eq!(texts.takes, "   (M-RET takes it)");
    }
}
