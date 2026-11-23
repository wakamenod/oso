//! What an empty composer's placeholder says (ADR 0170, as amended): that it writes to Claude,
//! and the keys that send and that go back to the transcript, as they are bound now.
//!
//! Must not depend on: how a frame is drawn: the drawing decides where and whether it shows.

use super::super::NativeSessions;
use crate::app::App;
use crate::dispatch::Dispatcher;

impl NativeSessions {
    /// What an empty composer's placeholder says, the whole of it first and then with fewer of
    /// its parts, for a narrow window, or nothing before one was drawn.
    pub(in crate::app) fn placeholder(&self) -> Option<&[String; 3]> {
        self.placeholder.as_ref()
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

/// The placeholder, with `dispatcher`'s keys: the whole, then with its last parts left out.
fn texts(dispatcher: &Dispatcher) -> [String; 3] {
    let send = dispatcher.where_is_in_composer("native-composer-send");
    let back = dispatcher.where_is_in_composer("native-composer-transcript");
    let what = "Write to Claude".to_owned();
    let sends = format!("{what} · {send} sends");
    [format!("{sends} · {back} transcript"), sends, what]
}

#[cfg(test)]
mod tests {
    use oso_core::keymap::Keymap;

    use super::texts;
    use crate::dispatch::Dispatcher;

    #[test]
    fn the_placeholder_names_the_keys_as_bound() {
        assert_eq!(
            texts(&Dispatcher::new()),
            [
                "Write to Claude · C-c C-c sends · C-c C-o transcript",
                "Write to Claude · C-c C-c sends",
                "Write to Claude",
            ]
        );
    }

    #[test]
    fn the_users_shorter_key_is_named_and_the_composers_own_win() {
        let mut dispatcher = Dispatcher::new();
        let keymap = Keymap::from_table(
            &crate::commands::registry(),
            &[("C-RET", "native-composer-send"), ("C-c C-o", "undo")],
        );
        dispatcher.set_user_keymap(keymap.unwrap());
        assert_eq!(
            texts(&dispatcher)[0],
            "Write to Claude · C-RET sends · C-c C-o transcript"
        );
    }
}
