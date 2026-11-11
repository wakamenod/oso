//! The settings in force: each as the file gives it, or its compiled-in value where the file
//! leaves it out or there is no file (ADR 0108, ADR 0110). A setting applies when its value
//! here changes, so a choice made at run time stands until then; this says which changed, and
//! the app applies them.
//!
//! Must not depend on: `app` or `dispatch`; applying a setting is the caller's.

use oso_core::editor::VisualAids;

use super::Settings;

/// Every setting, each with a value.
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
}

impl InForce {
    /// The compiled-in values, for `themes`, the built-in themes' names, the default first:
    /// the default theme, the first two themes, ADR 0026's aids and no channel.
    pub fn compiled(themes: &[&str]) -> Self {
        let name = |index: usize| themes.get(index).copied().unwrap_or_default().to_owned();
        Self {
            theme: name(0),
            pair: [name(0), name(1)],
            aids: VisualAids::default(),
            agent_channel_start: false,
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
        }
    }

    /// `current`, with each aid whose setting differs here from `before` switched as it is
    /// here; an aid whose setting is the same keeps the switch made at run time.
    pub fn aids_over(&self, before: &Self, current: VisualAids) -> VisualAids {
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

    use super::InForce;
    use crate::config::Settings;
    use crate::config::settings::{Aids, ThemeSetting};

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
            }
        );
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
