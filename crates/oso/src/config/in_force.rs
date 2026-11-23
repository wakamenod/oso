//! The settings in force: each as the file gives it, or its compiled-in value where the file
//! leaves it out or there is no file (ADR 0108, ADR 0110). A setting applies when its value
//! here changes, so a choice made at run time stands until then: [`InForce::changes`] says
//! what a look comes to, and the app only carries it out.
//!
//! Must not depend on: `app` or `dispatch`; applying a setting is the caller's.

use std::time::Duration;

use oso_core::editor::VisualAids;
use oso_core::indent;
use oso_core::jump::Keys;

use super::Settings;
use crate::sessions::tools::StartLimit;

/// How long a jump reads its string after the last key where `[jump] timeout` says nothing,
/// as avy's `avy-timeout-seconds`.
pub const JUMP_TIMEOUT: Duration = Duration::from_millis(500);

/// The jump's settings in force (ADR 0116).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpInForce {
    /// The keys labels are typed with.
    pub keys: Keys,
    /// How long a jump reads its string after the last key.
    pub timeout: Duration,
}

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
    /// Whether agents start sessions without asking, and how many, from the launch's look
    /// only (ADR 0135).
    pub agent_starts: Option<AgentStarts>,
    /// What to say of `[sessions] agents_start_without_asking`: not applied, or applying from
    /// the next start.
    pub agent_starts_later: Option<String>,
}

/// Whether sessions oso hosts may start sessions without asking, and how many each, as
/// `[sessions]` says at oso's start (ADR 0135).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentStarts {
    /// `agents_start_without_asking`.
    pub on: bool,
    /// `agents_start_limit`.
    pub limit: StartLimit,
}

impl Default for AgentStarts {
    fn default() -> Self {
        Self {
            on: false,
            limit: StartLimit::Most(super::settings::AGENTS_START_LIMIT),
        }
    }
}

impl AgentStarts {
    /// What `sessions` asks for.
    fn of(sessions: &super::settings::Sessions) -> Self {
        Self {
            on: sessions.agents_start_without_asking,
            limit: sessions.agents_start_limit(),
        }
    }
}

/// `limit` as `config.toml` spells it.
fn spelled(limit: StartLimit) -> String {
    match limit {
        StartLimit::Most(most) => most.to_string(),
        StartLimit::Unlimited => "\"none\"".to_owned(),
    }
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
    /// agent channel, as they could otherwise open it at launch (ADR 0110), nor let agents
    /// start sessions without asking (ADR 0135).
    pub group_writes: bool,
}

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
    /// Whether a save cleans the whitespace of the lines edited since (ADR 0113). It has no
    /// run-time switch, so the value in force is the one that applies.
    pub trim_whitespace: bool,
    /// Whether isearch starts with migemo on, and jumps match by it (ADR 0116). `M-m` turns it
    /// for one search, so the value in force is the one that applies.
    pub migemo: bool,
    /// The file migemo takes words from, as written in `[search] migemo-dictionary`, if any
    /// (ADR 0116). The app reads it at the first search that would use it.
    pub migemo_dictionary: Option<String>,
    /// The jump's keys and timeout (ADR 0116). They have no run-time switch either.
    pub jump: JumpInForce,
    /// Whether `RET` and typed closers indent (`electric-indent-mode`, ADR 0115).
    pub electric_indent: bool,
    /// Each mode's indentation, in the order of `config::MODES`, with where each property
    /// came from: the file or the compiled-in default.
    pub mode_indents: [(indent::Settings, indent::Origins); super::MODE_COUNT],
    /// How hosted sessions start (ADR 0121): read when one starts.
    pub sessions: super::settings::Sessions,
    /// `[dictation]`, read when a dictation starts (ADR 0175).
    pub dictation: super::Dictation,
    /// `[terminal]`, as the file gives it: it applies to the next terminal started (ADR 0120).
    pub terminal: super::Terminal,
    /// `[spaces]`: it applies to the next space made (ADR 0136).
    pub spaces: super::Spaces,
    /// `[lsp]` and the servers' table: it applies to the next server started (ADR 0138).
    pub lsp: super::Lsp,
    /// `[git]`: it applies to the next commit (ADR 0127, step 1b).
    pub git: super::GitSettings,
}

impl InForce {
    /// The compiled-in values, for `themes`, the built-in themes' names, the default first:
    /// the default theme, the first two themes, ADR 0026's aids, no channel, no cleanup and
    /// migemo on.
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
            migemo: true,
            migemo_dictionary: None,
            jump: JumpInForce {
                keys: Keys::default(),
                timeout: JUMP_TIMEOUT,
            },
            electric_indent: true,
            // Each mode's own default (ADR 0164): two spaces in HTML or JavaScript, a tab in Go.
            mode_indents: std::array::from_fn(|index| {
                (super::MODES[index].1.default_indent(), Default::default())
            }),
            sessions: super::settings::Sessions::default(),
            dictation: super::Dictation::default(),
            terminal: super::Terminal::default(),
            spaces: super::Spaces::default(),
            lsp: super::Lsp::default(),
            git: super::GitSettings::default(),
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
            migemo: or(settings.search.migemo, compiled.migemo),
            migemo_dictionary: settings.search.migemo_dictionary.clone(),
            jump: JumpInForce {
                keys: (settings.jump.keys.clone()).unwrap_or(compiled.jump.keys),
                timeout: settings.jump.timeout.unwrap_or(compiled.jump.timeout),
            },
            electric_indent: or(settings.indent.electric, compiled.electric_indent),
            mode_indents: std::array::from_fn(|index| {
                let mode = settings.modes[index];
                (mode.over(compiled.mode_indents[index].0), mode.origins())
            }),
            sessions: settings.sessions.clone(),
            dictation: settings.dictation.clone(),
            terminal: settings.terminal.clone(),
            spaces: settings.spaces.clone(),
            lsp: settings.lsp.clone(),
            git: settings.git.clone(),
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
        let (agent_starts, agent_starts_later) = self.agent_starts(before, look, name);
        Changes {
            aids,
            electric_indent,
            mode_indents: self.mode_indents != before.mode_indents,
            theme,
            start_channel,
            later,
            agent_starts,
            agent_starts_later,
        }
    }

    /// Whether agents start sessions without asking, decided at the launch's look only and
    /// never from a file a group can change; a later change says it applies from the next
    /// start, so a session that writes the file gains nothing in this oso (ADR 0135).
    fn agent_starts(
        &self,
        before: &Self,
        look: Look,
        name: &str,
    ) -> (Option<AgentStarts>, Option<String>) {
        let (now, then) = (
            AgentStarts::of(&self.sessions),
            AgentStarts::of(&before.sessions),
        );
        if look.launch {
            if now.on && look.group_writes {
                let refused = format!(
                    "{name}: [sessions] agents_start_without_asking not applied: other users in \
                     a group can change it"
                );
                return (Some(AgentStarts { on: false, ..now }), Some(refused));
            }
            return (Some(now), None);
        }
        let later = if now.on != then.on {
            Some(format!(
                "{name}: [sessions] agents_start_without_asking = {} applies from the next start",
                now.on
            ))
        } else {
            (now.limit != then.limit).then(|| {
                format!(
                    "{name}: [sessions] agents_start_limit = {} applies from the next start",
                    spelled(now.limit)
                )
            })
        };
        (None, later)
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

    use super::{AgentStarts, Changes, InForce, JUMP_TIMEOUT, JumpInForce, Look};
    use crate::config::Settings;
    use crate::sessions::tools::StartLimit;
    use std::time::Duration;

    use oso_core::jump::Keys;

    use crate::config::settings::{Aids, Jump, Save, Search, ThemeSetting};

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
                migemo: true,
                migemo_dictionary: None,
                jump: JumpInForce {
                    keys: Keys::default(),
                    timeout: JUMP_TIMEOUT,
                },
                electric_indent: true,
                // Each mode's own default: two spaces in HTML, a tab in Go (ADR 0164).
                mode_indents: crate::config::MODES
                    .map(|(_, mode)| (mode.default_indent(), indent::Origins::default())),
                sessions: crate::config::settings::Sessions::default(),
                dictation: crate::config::Dictation::default(),
                terminal: crate::config::Terminal::default(),
                spaces: crate::config::Spaces::default(),
                lsp: crate::config::Lsp::default(),
                git: crate::config::GitSettings::default(),
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
            search: Search {
                migemo: Some(false),
                migemo_dictionary: Some("~/dict/migemo-dict".to_owned()),
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
                migemo: false,
                migemo_dictionary: Some("~/dict/migemo-dict".to_owned()),
                jump: JumpInForce {
                    keys: Keys::new("hjkl").unwrap(),
                    timeout: Duration::from_secs(1),
                },
                electric_indent: true,
                // Each mode's own default: two spaces in HTML, a tab in Go (ADR 0164).
                mode_indents: crate::config::MODES
                    .map(|(_, mode)| (mode.default_indent(), indent::Origins::default())),
                sessions: crate::config::settings::Sessions::default(),
                dictation: crate::config::Dictation::default(),
                terminal: crate::config::Terminal::default(),
                spaces: crate::config::Spaces::default(),
                lsp: crate::config::Lsp::default(),
                git: crate::config::GitSettings::default(),
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
            agent_starts: None,
            agent_starts_later: None,
        };
        assert_eq!(changes(&compiled, &compiled, LATER), none);
        let off = Some(AgentStarts {
            on: false,
            limit: StartLimit::Most(4),
        });
        assert_eq!(
            changes(&compiled, &compiled, LAUNCH),
            Changes {
                agent_starts: off,
                ..none
            }
        );
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
    fn agents_start_without_asking_only_from_the_launch_and_never_from_a_group_s_file() {
        let off = InForce::compiled(THEMES);
        let mut on = off.clone();
        on.sessions.agents_start_without_asking = true;
        on.sessions.agents_start_limit = Some(StartLimit::Most(2));
        let launch = changes(&on, &off, LAUNCH);
        assert_eq!(
            launch.agent_starts,
            Some(AgentStarts {
                on: true,
                limit: StartLimit::Most(2)
            })
        );
        assert_eq!(launch.agent_starts_later, None);
        // A later look changes nothing in this oso, and says when it will.
        let later = changes(&on, &off, LATER);
        assert_eq!(later.agent_starts, None);
        assert_eq!(
            later.agent_starts_later.as_deref(),
            Some(
                "config.toml: [sessions] agents_start_without_asking = true applies from the \
                 next start"
            )
        );
        let mut fewer = on.clone();
        fewer.sessions.agents_start_limit = Some(StartLimit::Most(1));
        assert_eq!(
            changes(&fewer, &on, LATER).agent_starts_later.as_deref(),
            Some("config.toml: [sessions] agents_start_limit = 1 applies from the next start")
        );
        let mut unlimited = on.clone();
        unlimited.sessions.agents_start_limit = Some(StartLimit::Unlimited);
        assert_eq!(
            changes(&unlimited, &on, LATER)
                .agent_starts_later
                .as_deref(),
            Some(
                "config.toml: [sessions] agents_start_limit = \"none\" applies from the next \
                 start"
            )
        );
        assert_eq!(
            changes(&on, &on, LATER).agent_starts_later,
            None,
            "unchanged"
        );
        // A file a group can change does not apply it.
        let look = Look {
            group_writes: true,
            ..LAUNCH
        };
        let group = changes(&on, &off, look);
        assert_eq!(
            group.agent_starts,
            Some(AgentStarts {
                on: false,
                limit: StartLimit::Most(2)
            })
        );
        assert_eq!(
            group.agent_starts_later.as_deref(),
            Some(
                "config.toml: [sessions] agents_start_without_asking not applied: other users \
                 in a group can change it"
            )
        );
    }

    #[test]
    fn electric_indent_and_the_modes_indentation_apply_when_they_change() {
        let before = InForce::compiled(THEMES);
        let mut now = before.clone();
        now.electric_indent = false;
        now.mode_indents[0].0 = indent::Settings::new(2, indent::Style::Tabs, 4).unwrap();
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
