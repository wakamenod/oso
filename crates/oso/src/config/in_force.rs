//! The settings in force: each as the file gives it, or its compiled-in value where the file
//! leaves it out or there is no file (ADR 0108, ADR 0110). A setting applies when its value
//! here changes, so a choice made at run time stands until then: [`InForce::changes`] says
//! what a look comes to, and the app only carries it out.
//!
//! Must not depend on: `app` or `dispatch`; applying a setting is the caller's.

use oso_core::editor::VisualAids;
use oso_core::indent;

use super::Settings;
use super::settings::Jump;

/// What a look's settings come to, against those in force before it.
#[derive(Debug, PartialEq, Eq)]
pub struct Changes {
    /// The visual aids' switches, if a setting of one changed.
    pub aids: Option<VisualAids>,
    /// Whether `RET` indents, if `[indent] electric` changed (ADR 0115).
    pub electric_indent: Option<bool>,
    /// Whether a mode's indentation changed, for the buffers in it.
    pub mode_indents: bool,
    /// The theme to switch to, quietly, if the configured default changed.
    pub theme: Option<String>,
    /// Whether to start the agent channel.
    pub start_channel: bool,
    /// What to say of a change that applies only at the next start.
    pub later: Option<String>,
}

/// When a look came.
#[derive(Debug, Clone, Copy, Default)]
pub struct Look {
    /// It is the launch's: the first, and no later one replaced it before it was taken.
    pub launch: bool,
    /// A dev restart's handoff set the theme and the aids before any look applied settings,
    /// so they win, and the file's become those in force without being applied (ADR 0108).
    pub handed_over: bool,
    /// Other users in a group can change the file, or the way to it: it does not start the
    /// agent channel, as they could otherwise open it at launch (ADR 0110).
    pub group_writes: bool,
}

/// Every setting, each with a value, but the jump's, whose compiled-in values are the app's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InForce {
    /// The theme to start in, and to switch to when it changes.
    pub theme: String,
    /// What `toggle-theme` switches between.
    pub pair: [String; 2],
    /// The visual aids' switches.
    pub aids: VisualAids,
    /// Whether the agent channel starts at launch.
    pub agent_channel_start: bool,
    /// Whether a save cleans the whitespace of the lines edited since (ADR 0113). It has no
    /// run-time switch, so the value in force is the one that applies.
    pub trim_whitespace: bool,
    /// The jump's keys and timeout (ADR 0116), `None` where the file leaves them out, for the
    /// editor's and the app's own. They have no run-time switch either.
    pub jump: Jump,
    /// Whether `RET` and typed closers indent (`electric-indent-mode`, ADR 0115).
    pub electric_indent: bool,
    /// Each mode's indentation, in the order of `config::MODES`.
    pub mode_indents: [indent::Settings; 4],
}

impl InForce {
    /// The compiled-in values, for `themes`, the built-in themes' names, the default first:
    /// the default theme, the first two themes, ADR 0026's aids, no channel and no cleanup.
    pub fn compiled(themes: &[&str]) -> Self {
        let first = themes.first().copied().unwrap_or_default();
        // With one theme, the toggle stays on it, as the editor's own pair does.
        let second = themes.get(1).copied().unwrap_or(first);
        Self {
            theme: first.to_owned(),
            pair: [first.to_owned(), second.to_owned()],
            aids: VisualAids::default(),
            agent_channel_start: false,
            trim_whitespace: false,
            jump: Jump::default(),
            electric_indent: true,
            mode_indents: [indent::Settings::default(); 4],
        }
    }

    /// What `settings` give, and the compiled-in values for `themes` where they give nothing.
    pub fn of(settings: &Settings, themes: &[&str]) -> Self {
        let compiled = Self::compiled(themes);
        let aids = &settings.aids;
        let or = |set: Option<bool>, compiled: bool| set.unwrap_or(compiled);
        Self {
            theme: settings.theme.default.clone().unwrap_or(compiled.theme),
            pair: settings.theme.pair.clone().unwrap_or(compiled.pair),
            aids: VisualAids {
                whitespace: or(aids.whitespace, compiled.aids.whitespace),
                current_line: or(aids.current_line, compiled.aids.current_line),
                flash: or(aids.flash, compiled.aids.flash),
                line_numbers: or(aids.line_numbers, compiled.aids.line_numbers),
            },
            agent_channel_start: settings
                .agent_channel_start
                .unwrap_or(compiled.agent_channel_start),
            trim_whitespace: settings
                .save
                .trim_whitespace
                .unwrap_or(compiled.trim_whitespace),
            jump: settings.jump.clone(),
            electric_indent: or(settings.indent.electric, compiled.electric_indent),
            mode_indents: std::array::from_fn(|index| {
                settings.modes[index].over(compiled.mode_indents[index])
            }),
        }
    }

    /// What these settings come to after `before`, with the aids switched as `current` says,
    /// for a `look` at the file called `name`. Only what differs applies, each aid on its own.
    /// Only the launch's look starts the channel; a later one says that a changed value
    /// applies from the next start. The toggle's pair is applied on every look, as switching
    /// the theme sets it again.
    pub fn changes(&self, before: &Self, current: VisualAids, look: Look, name: &str) -> Changes {
        let (aids, theme, electric_indent) = if look.handed_over {
            (None, None, None)
        } else {
            let aids = self.aids_over(before, current);
            (
                (aids != current).then_some(aids),
                (self.theme != before.theme).then(|| self.theme.clone()),
                (self.electric_indent != before.electric_indent).then_some(self.electric_indent),
            )
        };
        let start = self.agent_channel_start;
        let start_channel = look.launch && start && !look.group_writes;
        let later = if look.launch && start && look.group_writes {
            Some(format!(
                "{name}: [agent-channel] start not applied: other users in a group can change it"
            ))
        } else {
            (!look.launch && start != before.agent_channel_start).then(|| {
                format!("{name}: [agent-channel] start = {start} applies from the next start")
            })
        };
        Changes {
            aids,
            electric_indent,
            mode_indents: self.mode_indents != before.mode_indents,
            theme,
            start_channel,
            later,
        }
    }

    /// `current`, with each aid whose setting differs here from `before` switched as it is
    /// here; an aid whose setting is the same keeps the switch made at run time.
    fn aids_over(&self, before: &Self, current: VisualAids) -> VisualAids {
        let pick = |now: bool, then: bool, current: bool| if now == then { current } else { now };
        let (now, then) = (self.aids, before.aids);
        VisualAids {
            whitespace: pick(now.whitespace, then.whitespace, current.whitespace),
            current_line: pick(now.current_line, then.current_line, current.current_line),
            flash: pick(now.flash, then.flash, current.flash),
            line_numbers: pick(now.line_numbers, then.line_numbers, current.line_numbers),
        }
    }
}

#[cfg(test)]
mod tests {
    use oso_core::editor::VisualAids;
    use oso_core::indent;

    use super::{Changes, InForce, Look};
    use crate::config::Settings;
    use std::time::Duration;

    use oso_core::jump::Keys;

    use crate::config::settings::{Aids, Jump, Save, ThemeSetting};

    const THEMES: &[&str] = &["dusk", "light", "dark"];

    #[test]
    fn what_the_file_leaves_out_is_compiled_in() {
        let compiled = InForce::compiled(THEMES);
        assert_eq!(
            compiled,
            InForce {
                theme: "dusk".to_owned(),
                pair: ["dusk".to_owned(), "light".to_owned()],
                aids: VisualAids::default(),
                agent_channel_start: false,
                trim_whitespace: false,
                jump: Jump::default(),
                electric_indent: true,
                mode_indents: [indent::Settings::default(); 4],
            }
        );
        assert_eq!(InForce::of(&Settings::default(), THEMES), compiled);
    }

    #[test]
    fn what_the_file_gives_is_in_force() {
        let settings = Settings {
            theme: ThemeSetting {
                default: Some("dark".to_owned()),
                pair: Some(["dark".to_owned(), "light".to_owned()]),
            },
            aids: Aids {
                whitespace: Some(false),
                line_numbers: Some(true),
                ..Aids::default()
            },
            agent_channel_start: Some(true),
            save: Save {
                trim_whitespace: Some(true),
            },
            jump: Jump {
                keys: Some(Keys::new("hjkl").unwrap()),
                timeout: Some(Duration::from_secs(1)),
            },
            ..Settings::default()
        };
        assert_eq!(
            InForce::of(&settings, THEMES),
            InForce {
                theme: "dark".to_owned(),
                pair: ["dark".to_owned(), "light".to_owned()],
                aids: VisualAids {
                    whitespace: false,
                    current_line: true,
                    flash: true,
                    line_numbers: true,
                },
                agent_channel_start: true,
                trim_whitespace: true,
                jump: Jump {
                    keys: Some(Keys::new("hjkl").unwrap()),
                    timeout: Some(Duration::from_secs(1)),
                },
                electric_indent: true,
                mode_indents: [indent::Settings::default(); 4],
            }
        );
    }

    const LATER: Look = Look {
        launch: false,
        handed_over: false,
        group_writes: false,
    };
    const LAUNCH: Look = Look {
        launch: true,
        handed_over: false,
        group_writes: false,
    };

    fn changes(now: &InForce, before: &InForce, look: Look) -> Changes {
        now.changes(before, VisualAids::default(), look, "config.toml")
    }

    #[test]
    fn nothing_changed_changes_nothing() {
        let compiled = InForce::compiled(THEMES);
        let none = Changes {
            aids: None,
            electric_indent: None,
            mode_indents: false,
            theme: None,
            start_channel: false,
            later: None,
        };
        assert_eq!(changes(&compiled, &compiled, LATER), none);
        assert_eq!(changes(&compiled, &compiled, LAUNCH), none);
    }

    #[test]
    fn a_changed_default_switches_the_theme_and_an_unchanged_one_keeps_the_choice() {
        let before = InForce::compiled(THEMES);
        let mut now = before.clone();
        now.theme = "light".to_owned();
        assert_eq!(
            changes(&now, &before, LAUNCH).theme.as_deref(),
            Some("light")
        );
        assert_eq!(changes(&now, &now, LATER).theme, None);
    }

    #[test]
    fn only_the_launchs_look_starts_the_channel_and_a_later_change_waits_for_a_start() {
        let off = InForce::compiled(THEMES);
        let mut on = off.clone();
        on.agent_channel_start = true;
        let launch = changes(&on, &off, LAUNCH);
        assert!(launch.start_channel);
        assert_eq!(launch.later, None);
        let later = changes(&on, &off, LATER);
        assert!(!later.start_channel);
        assert_eq!(
            later.later.as_deref(),
            Some("config.toml: [agent-channel] start = true applies from the next start")
        );
        let stop = changes(&off, &on, LATER);
        assert!(!stop.start_channel);
        assert_eq!(
            stop.later.as_deref(),
            Some("config.toml: [agent-channel] start = false applies from the next start")
        );
        assert_eq!(changes(&on, &on, LATER).later, None, "unchanged");
        assert!(!changes(&off, &off, LAUNCH).start_channel);
    }

    #[test]
    fn after_a_handoff_the_theme_and_aids_are_taken_as_they_are_and_the_channel_applies() {
        let before = InForce::compiled(THEMES);
        let mut now = before.clone();
        now.theme = "dark".to_owned();
        now.aids.line_numbers = true;
        now.agent_channel_start = true;
        let look = Look {
            handed_over: true,
            ..LAUNCH
        };
        let handed = changes(&now, &before, look);
        assert_eq!((handed.aids, handed.theme), (None, None));
        assert!(handed.start_channel, "as at a launch");
        let unhanded = changes(&now, &before, LAUNCH);
        assert_eq!(unhanded.theme.as_deref(), Some("dark"));
        assert_eq!(unhanded.aids.map(|aids| aids.line_numbers), Some(true));
    }

    #[test]
    fn a_file_a_group_can_change_does_not_start_the_channel_and_says_why() {
        let off = InForce::compiled(THEMES);
        let mut on = off.clone();
        on.agent_channel_start = true;
        on.aids.line_numbers = true;
        let look = Look {
            group_writes: true,
            ..LAUNCH
        };
        let changes = changes(&on, &off, look);
        assert!(!changes.start_channel);
        assert_eq!(
            changes.later.as_deref(),
            Some(
                "config.toml: [agent-channel] start not applied: other users in a group can \
                 change it"
            )
        );
        assert!(changes.aids.is_some(), "the other settings apply");
    }

    #[test]
    fn electric_indent_and_the_modes_indentation_apply_when_they_change() {
        let before = InForce::compiled(THEMES);
        let mut now = before.clone();
        now.electric_indent = false;
        now.mode_indents[0] = indent::Settings::new(2, indent::Style::Tabs, 4).unwrap();
        let changed = changes(&now, &before, LATER);
        assert_eq!(
            (changed.electric_indent, changed.mode_indents),
            (Some(false), true)
        );
        let same = changes(&now, &now, LATER);
        assert_eq!((same.electric_indent, same.mode_indents), (None, false));
        let look = Look {
            handed_over: true,
            ..LAUNCH
        };
        assert_eq!(
            changes(&now, &before, look).electric_indent,
            None,
            "the handoff's wins"
        );
    }

    #[test]
    fn with_one_theme_the_pair_is_it_twice() {
        let one = InForce::compiled(&["dusk"]);
        assert_eq!(one.pair, ["dusk".to_owned(), "dusk".to_owned()]);
    }

    #[test]
    fn only_an_aid_whose_setting_changed_overrides_its_switch() {
        let before = InForce::compiled(THEMES);
        let mut now = before.clone();
        now.aids.line_numbers = true;
        // Whitespace was switched off at run time, and its setting did not change.
        let current = VisualAids {
            whitespace: false,
            ..VisualAids::default()
        };
        assert_eq!(
            now.aids_over(&before, current),
            VisualAids {
                whitespace: false,
                line_numbers: true,
                ..VisualAids::default()
            }
        );
        assert_eq!(before.aids_over(&before, current), current, "no change");
        assert_eq!(
            before.aids_over(&now, current),
            current,
            "line numbers back off, as they already are"
        );
    }
}
