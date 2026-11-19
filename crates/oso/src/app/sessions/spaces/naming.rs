//! How a space is named (ADR 0159): the session names no new space's slug may take, and
//! `space-rename`'s prompt, holding the display name, whose answer renames the space it was
//! opened for.
//!
//! Must not depend on: key presses or how a frame is drawn.

use crate::app::App;
use crate::commands::agent_sessions::SPACE_RENAME_TO;
use crate::sessions::identity::same_name;
use crate::sessions::spaces::stored::now_ms;

impl App {
    /// Whether a name is one a session oso knows answers to, ASCII case aside (ADR 0155's
    /// set), which no new space's slug may be (ADR 0159).
    pub(super) fn session_name_taken(&self) -> impl Fn(&str) -> bool + use<> {
        let names: Vec<String> = (self.session_names(None).into_iter())
            .map(str::to_owned)
            .collect();
        move |name: &str| names.iter().any(|other| same_name(other, name))
    }

    /// `space-rename`: opens `space-rename-to`'s prompt holding the display name of the space
    /// in front, `M-p` giving back an empty one (ADR 0159).
    pub(super) fn ask_space_rename(&mut self) {
        let Some(at) = self.watch.spaces().current() else {
            return self.editor.message("No space in front".to_owned());
        };
        let Some(space) = self.watch.spaces().get(at) else {
            return;
        };
        let (id, label) = (space.id().clone(), space.label().to_owned());
        let Some(prompt) = self.open_host_prompt(SPACE_RENAME_TO) else {
            return self
                .editor
                .message("Cannot ask for the name now".to_owned());
        };
        self.editor.offer_prompt_default(prompt, label);
        let _ = self.editor.walk_history(false);
        self.hosting.spaces.renaming = Some((prompt, id));
    }

    /// `space-rename-to`'s answer: the space its prompt was opened for takes the display name
    /// alone (ADR 0159).
    pub(super) fn rename_space(&mut self, display: &str) {
        const NONE_ASKED: &str = "No rename was asked: space-rename asks";
        let Some((prompt, id)) = self.hosting.spaces.renaming.take() else {
            return self.editor.message(NONE_ASKED.to_owned());
        };
        if self.editor.prompts().iter().any(|p| p.id() == prompt) {
            return self.editor.message(NONE_ASKED.to_owned());
        }
        let Some(at) = self.watch.spaces().find(&id) else {
            return self.editor.message("The space is gone".to_owned());
        };
        match self.watch.spaces_mut().rename(at, display, now_ms()) {
            Ok(shown) => self.editor.message(format!("The space is now {shown}")),
            Err(why) => self.editor.message(why),
        }
    }
}
