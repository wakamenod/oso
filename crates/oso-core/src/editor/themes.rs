//! Which theme the frame is drawn in, and the pair `toggle-theme` switches between
//! (ADR 0089). The themes are the renderer's: the editor keeps their names, and the host
//! draws with the one chosen.
//!
//! Must not depend on: `command`, `keymap` or the host's window.

use std::sync::Arc;

use super::Editor;
use crate::complete::Candidate;

/// The themes the host draws with, by name, and which one it draws with now.
#[derive(Debug, Default)]
pub(super) struct Themes {
    /// Each theme's name and what it is, such as `dark`, in the order the host gave them.
    names: Vec<(&'static str, &'static str)>,
    current: usize,
    /// What `toggle-theme` switches between: the first two, until configuration names them.
    pair: [usize; 2],
}

/// Why a theme could not be loaded.
#[derive(Debug, PartialEq, Eq)]
pub struct NoSuchTheme(pub String);

impl std::fmt::Display for NoSuchTheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "No theme named {}", self.0)
    }
}

impl Editor {
    /// Takes the themes the host draws with, each a name and what it is, the default first,
    /// and draws with the one at `current`, or the default if there is none there.
    pub fn set_themes(&mut self, names: Vec<(&'static str, &'static str)>, current: usize) {
        let current = if current < names.len() { current } else { 0 };
        let pair = [0, usize::from(names.len() > 1)];
        self.themes = Themes {
            names,
            current,
            pair,
        };
    }

    /// Makes `toggle-theme` switch between the themes at `pair`, indices in the host's list.
    /// Two that are the same, or one past the list, leave the first two (ADR 0108).
    pub fn set_theme_pair(&mut self, pair: [usize; 2]) {
        let [first, second] = pair;
        let names = self.themes.names.len();
        self.themes.pair = if first != second && first < names && second < names {
            pair
        } else {
            [0, usize::from(names > 1)]
        };
    }

    /// The index, in the host's list, of the theme to draw with.
    pub fn theme(&self) -> usize {
        self.themes.current
    }

    /// The name of the theme to draw with, if the host gave any.
    pub fn theme_name(&self) -> Option<&'static str> {
        self.themes
            .names
            .get(self.themes.current)
            .map(|&(name, _)| name)
    }

    /// Draws with the theme called `name` from the next frame on, and says so in the echo
    /// area.
    ///
    /// # Errors
    ///
    /// [`NoSuchTheme`] if the host gave no theme of that name. Nothing changes.
    pub fn load_theme(&mut self, name: &str) -> Result<(), NoSuchTheme> {
        let index = self
            .themes
            .names
            .iter()
            .position(|&(theme, _)| theme == name)
            .ok_or_else(|| NoSuchTheme(name.to_owned()))?;
        self.show_theme(index);
        Ok(())
    }

    /// Switches between the two themes of the pair. From a theme outside it, goes to the
    /// pair's first. Returns false if the host gave no theme.
    pub fn toggle_theme(&mut self) -> bool {
        if self.themes.names.is_empty() {
            return false;
        }
        let [first, second] = self.themes.pair;
        let next = if self.themes.current == first {
            second
        } else {
            first
        };
        self.show_theme(next);
        true
    }

    /// What `load-theme` completes over: every theme, with what it is, and the current one
    /// said to be so.
    pub fn theme_candidates(&self) -> Arc<[Candidate]> {
        self.themes
            .names
            .iter()
            .enumerate()
            .map(|(index, &(name, kind))| {
                let annotation = if index == self.themes.current {
                    format!("{kind}, current")
                } else {
                    kind.to_owned()
                };
                Candidate::new(name, "", annotation)
            })
            .collect()
    }

    fn show_theme(&mut self, index: usize) {
        self.themes.current = index;
        if let Some(name) = self.theme_name() {
            self.message(format!("Theme: {name}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> Editor {
        let mut editor = Editor::default();
        editor.set_themes(
            vec![("dark", "dark"), ("light", "light"), ("dusk", "dark")],
            0,
        );
        editor
    }

    #[test]
    fn a_theme_loads_by_its_name_and_an_unknown_one_changes_nothing() {
        let mut editor = editor();
        assert_eq!(editor.load_theme("dusk"), Ok(()));
        assert_eq!((editor.theme(), editor.theme_name()), (2, Some("dusk")));
        let error = editor.load_theme("sepia").unwrap_err();
        assert_eq!(error.to_string(), "No theme named sepia");
        assert_eq!(editor.theme(), 2);
    }

    #[test]
    fn the_toggle_switches_between_the_pair_and_comes_back_into_it() {
        let mut editor = editor();
        assert!(editor.toggle_theme());
        assert_eq!(editor.theme_name(), Some("light"));
        assert!(editor.toggle_theme());
        assert_eq!(editor.theme_name(), Some("dark"));
        editor.load_theme("dusk").unwrap();
        assert!(editor.toggle_theme());
        assert_eq!(
            editor.theme_name(),
            Some("dark"),
            "from outside the pair, its first"
        );
    }

    #[test]
    fn the_pair_can_be_set_and_a_wrong_one_gives_the_first_two() {
        let mut editor = editor();
        editor.set_theme_pair([2, 1]);
        assert!(editor.toggle_theme());
        assert_eq!(editor.theme_name(), Some("dusk"), "from outside, the first");
        assert!(editor.toggle_theme());
        assert_eq!(editor.theme_name(), Some("light"));
        for wrong in [[1, 1], [0, 3], [7, 1]] {
            editor.set_theme_pair(wrong);
            editor.load_theme("dark").unwrap();
            assert!(editor.toggle_theme());
            assert_eq!(editor.theme_name(), Some("light"), "{wrong:?}");
        }
    }

    #[test]
    fn with_no_themes_or_one_the_toggle_does_what_it_can() {
        let mut editor = Editor::default();
        assert!(!editor.toggle_theme());
        assert_eq!(editor.theme_name(), None);
        editor.set_themes(vec![("dark", "dark")], 0);
        assert!(editor.toggle_theme());
        assert_eq!(editor.theme_name(), Some("dark"));
    }

    #[test]
    fn a_remembered_theme_past_the_list_gives_the_default() {
        let mut editor = Editor::default();
        editor.set_themes(vec![("dark", "dark"), ("light", "light")], 7);
        assert_eq!(editor.theme(), 0);
    }

    #[test]
    fn the_candidates_say_what_each_theme_is_and_which_is_current() {
        let mut editor = editor();
        editor.load_theme("light").unwrap();
        let candidates = editor.theme_candidates();
        let rows: Vec<_> = candidates
            .iter()
            .map(|c| (c.text(), c.annotation()))
            .collect();
        assert_eq!(
            rows,
            [
                ("dark", "dark"),
                ("light", "light, current"),
                ("dusk", "dark")
            ]
        );
    }
}
