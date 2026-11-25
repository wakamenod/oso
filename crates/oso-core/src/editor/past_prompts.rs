//! The list of what the person sent native sessions (ADR 0165, step 2, "History"), as the host
//! gives it: a list that completes, the newest first, with the selected one's whole text in the
//! preview, as `M-y` lists the kill ring (ADR 0091), whose rows and preview it shares.
//!
//! Must not depend on: `command`, `keymap` or the host's window.
//!
//! - It keeps no minibuffer history, and its rows are withheld as `M-y`'s are.
//! - What it picks is the text as it was listed, though the host gave others since, and only
//!   for the buffer it opened over, while that is selected.

use std::sync::Arc;

use super::Editor;
use super::completion::CompletionStyle;
use super::yank_pop::{lines_said, row};
use crate::buffer::BufferId;
use crate::complete::Candidate;
use crate::minibuffer::{Prompt, PromptError, PromptId};

/// What the host lists, and the list open.
#[derive(Default)]
pub(super) struct PastPrompts {
    texts: Arc<[Arc<str>]>,
    list: Option<PastList>,
}

/// A list, what it lists in the order of its rows, over which buffer, and the one picked when
/// it closed.
struct PastList {
    prompt: PromptId,
    buffer: BufferId,
    texts: Arc<[Arc<str>]>,
    picked: Option<Arc<str>>,
}

impl Editor {
    /// Sets what [`Editor::open_past_prompts`] lists from its next opening, the newest first.
    /// With none, a list open picks nothing either, as they are gone.
    pub fn set_past_prompts(&mut self, texts: Arc<[Arc<str>]>) {
        if let Some(list) = &mut self.past_prompts.list
            && texts.is_empty()
        {
            list.texts = Arc::clone(&texts);
            list.picked = None;
        }
        self.past_prompts.texts = texts;
    }

    /// Opens a prompt that says `label` and completes over the past prompts, as the module
    /// says. It keeps no history.
    ///
    /// # Errors
    ///
    /// [`PromptError::Invalid`] with none to list, and [`PromptError::TooDeep`] with as many
    /// prompts open as may be. Nothing changes.
    pub fn open_past_prompts(&mut self, label: &str) -> Result<PromptId, PromptError> {
        let texts = Arc::clone(&self.past_prompts.texts);
        if texts.is_empty() {
            return Err(PromptError::Invalid);
        }
        let items: Arc<[Candidate]> = (texts.iter())
            .map(|text| {
                let said = lines_said(text.lines().count()).unwrap_or_default();
                Candidate::new(row([&**text].into_iter()), "", said)
            })
            .collect();
        let buffer = self.current().0.buffer();
        let style = CompletionStyle {
            given_order: true,
            takes_input: false,
            preview: true,
        };
        let prompt = self.open_completion_as(label, "", items, style)?;
        self.past_prompts.list = Some(PastList {
            prompt,
            buffer,
            texts,
            picked: None,
        });
        Ok(prompt)
    }

    /// The past prompt the list picked, once, if the buffer it opened over is selected.
    pub fn take_past_prompt(&mut self) -> Option<Arc<str>> {
        let list = self.past_prompts.list.take()?;
        (list.buffer == self.current().0.buffer()).then_some(list.picked)?
    }

    /// The list of past prompts, while it is the innermost prompt.
    fn past_list(&self) -> Option<&PastList> {
        let innermost = self.prompt()?.id();
        (self.past_prompts.list.as_ref()).filter(|list| list.prompt == innermost)
    }

    /// Whether the list of past prompts is the innermost prompt.
    pub(super) fn past_prompts_open(&self) -> bool {
        self.past_list().is_some()
    }

    /// Which past prompt the innermost prompt's selected row names, while it lists them.
    pub(super) fn selected_past(&self) -> Option<usize> {
        self.past_list()?;
        (self.prompt().and_then(Prompt::candidates)).and_then(|list| list.selected_item())
    }

    /// The text of the past prompt `at` of the list open.
    pub(super) fn past_text(&self, at: u64) -> Option<&str> {
        let at = usize::try_from(at).ok()?;
        self.past_list()?.texts.get(at).map(|text| &**text)
    }

    /// Notes which past prompt the selected row names, as the innermost prompt is answered
    /// while it lists them.
    pub(super) fn note_past_pick(&mut self) {
        let Some(at) = self.selected_past() else {
            return;
        };
        if let Some(list) = &mut self.past_prompts.list {
            list.picked = list.texts.get(at).cloned();
        }
    }

    /// Forgets the list as `closed` closes, all of it unless it was answered.
    pub(super) fn forget_past_list(&mut self, closed: PromptId) {
        let kept = &mut self.past_prompts.list;
        match kept {
            Some(list) if list.prompt != closed => {}
            Some(list) if list.picked.is_some() => list.texts = Arc::from([]),
            _ => *kept = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, Mode};
    use crate::editor::Preview;

    /// An editor whose selected buffer is a composer, listing `texts`.
    fn editor(texts: &[&str]) -> (Editor, BufferId) {
        let mut buffer = Buffer::from_text("*compose: x*", "");
        buffer.set_mode(Mode::Composer);
        let mut editor = Editor::default();
        let id = editor.add_buffer(buffer);
        editor.show(id);
        editor.set_past_prompts(texts.iter().map(|text| Arc::from(*text)).collect());
        (editor, id)
    }

    fn rows(editor: &Editor) -> Vec<(String, String)> {
        let candidates = editor.prompt().unwrap().candidates().unwrap();
        candidates
            .shown(9)
            .map(|(_, c)| (c.text().to_owned(), c.annotation().to_owned()))
            .collect()
    }

    fn answer(editor: &mut Editor) {
        editor.submit_prompt().unwrap();
    }

    #[test]
    fn the_newest_is_first_on_one_line_each_and_the_whole_text_is_previewed() {
        let (mut editor, _) = editor(&["b\nc\u{202e}", "a"]);
        editor.open_past_prompts("Past prompt:").unwrap();
        let shown = rows(&editor);
        assert_eq!(shown[0].0, "b⤶c\\u{202e}");
        assert_eq!(shown[0].1, "2 lines");
        assert_eq!(shown[1], ("a".to_owned(), String::new()));
        assert!(editor.kill_list_open(), "withheld as M-y's rows");
        let Some(Preview::Text { entry }) = editor.preview() else {
            panic!("no preview");
        };
        assert_eq!(editor.kill_preview(entry, 5, 10), ["b", "c\\u{202e}"]);
    }

    #[test]
    fn the_whole_text_is_picked_once_though_rows_read_the_same_and_the_list_changed() {
        let (one, two) = ("x".repeat(5000) + "1", "x".repeat(5000) + "2");
        let (mut editor, _) = editor(&[&one, &two]);
        editor.open_past_prompts("Past prompt:").unwrap();
        assert_eq!(rows(&editor)[0].0, rows(&editor)[1].0);
        editor.move_completion(1).unwrap();
        editor.set_past_prompts([Arc::from("other")].into());
        answer(&mut editor);
        assert_eq!(editor.take_past_prompt().as_deref(), Some(&*two));
        assert_eq!(editor.take_past_prompt(), None);
        assert!(!editor.kill_list_open());
    }

    #[test]
    fn nothing_is_picked_for_another_buffer_by_a_list_left_or_with_none() {
        let (mut editor, _) = editor(&["a"]);
        editor.open_past_prompts("Past prompt:").unwrap();
        answer(&mut editor);
        let other = editor.add_buffer(Buffer::from_text("other", ""));
        editor.show(other);
        assert_eq!(editor.take_past_prompt(), None);
        editor.open_past_prompts("Past prompt:").unwrap();
        editor.quit();
        assert_eq!(editor.take_past_prompt(), None);
        // Cleared while it is listed.
        editor.open_past_prompts("Past prompt:").unwrap();
        editor.set_past_prompts(Arc::from([]));
        answer(&mut editor);
        assert_eq!(editor.take_past_prompt(), None);
        assert_eq!(
            editor.open_past_prompts("Past prompt:"),
            Err(PromptError::Invalid)
        );
    }
}
