//! The permission modes oso gives a native session's `claude` (ADR 0165, Q4; part 3b): the
//! only ones it ever sends, on the command line (`--permission-mode`) and in a control request
//! (`set_permission_mode`). They sit here, below `sessions`, so that a session's bridge, in
//! `channel`, can share the one type when it gives the CLI a mode.
//!
//! Must not depend on: sockets, threads, editor state or the sessions module.
//!
//! - There is no `bypassPermissions` and no `dontAsk`, which let calls run unasked: no value of
//!   this type names either, so neither can be sent. A mode the CLI says that is not one of
//!   these is shown as it says it, and never sent.
//! - No agent or channel method takes one: the person alone chooses a mode.

/// A permission mode oso may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    /// Each call that would ask, asks.
    Default,
    /// Edits run unasked.
    AcceptEdits,
    /// Claude plans, and asks before it acts on the plan.
    Plan,
    /// The CLI's classifier lets calls it judges safe run unasked.
    Auto,
}

impl PermissionMode {
    /// Every mode, in the order the CLI's terminal cycles them (`shift+tab`).
    pub const ALL: [Self; 4] = [Self::Default, Self::AcceptEdits, Self::Plan, Self::Auto];

    /// The mode as the CLI names it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::Auto => "auto",
        }
    }

    /// The mode the CLI names `name`, if it is one of these.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::PermissionMode;

    #[test]
    fn each_mode_reads_back_from_its_name() {
        for mode in PermissionMode::ALL {
            assert_eq!(PermissionMode::named(mode.name()), Some(mode));
        }
    }

    #[test]
    fn the_modes_that_skip_asking_are_none() {
        for name in [
            "bypassPermissions",
            "dontAsk",
            "manual",
            "Default",
            "",
            "auto ",
        ] {
            assert_eq!(PermissionMode::named(name), None, "{name:?}");
        }
    }
}
