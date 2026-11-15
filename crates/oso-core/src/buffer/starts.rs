//! What opening or saving a buffer starts, and whether each kind runs a program (ADR 0118,
//! ADR 0138): the kinds, a set of them, and the two marks a buffer keeps of what waits for the
//! person after an agent's open or save.
//!
//! Must not depend on: `editor`, `window` or `command`; the editor decides when a mark is set
//! or let go, and the host what a kind starts.

use super::Buffer;

/// Something that opening or saving a buffer starts, beyond the open or the save itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Start {
    /// A language server starts for the buffer's project (ADR 0138).
    ServerStart,
    /// A running language server learns of the buffer (`didOpen`).
    ServerOpen,
    /// A running language server learns of a save (`didSave`), which for rust-analyzer runs
    /// `cargo check`.
    ServerSave,
}

/// Whether a [`Start`] runs a program, so that it waits when an agent's open or save would start
/// it (ADR 0118).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runs {
    /// oso's own work in its process, which runs for the agent as for the person.
    InOso,
    /// A process, or a request to one: it waits for the person after an agent's open or save.
    Program,
}

impl Start {
    /// Every kind, in the order a set lists them.
    pub const ALL: [Self; 3] = [Self::ServerStart, Self::ServerOpen, Self::ServerSave];

    /// Whether it runs a program. Every kind says so, with no default.
    pub const fn runs(self) -> Runs {
        match self {
            Self::ServerStart | Self::ServerOpen | Self::ServerSave => Runs::Program,
        }
    }

    /// Its name where it is kept or listed by kind: the restart handoff and tests.
    pub const fn id(self) -> &'static str {
        match self {
            Self::ServerStart => "server-start",
            Self::ServerOpen => "server-open",
            Self::ServerSave => "server-save",
        }
    }

    /// The kind [`Self::id`] names, if any.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|start| start.id() == id)
    }

    /// What the person and the agent are told waits: the modeline's `waits: server`, and the
    /// channel's replies and state.
    pub const fn shown(self) -> &'static str {
        match self {
            Self::ServerStart | Self::ServerOpen | Self::ServerSave => "server",
        }
    }

    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

// A set holds each kind in a bit of a `u8`.
const _: () = assert!(Start::ALL.len() <= 8);

/// A set of [`Start`]s, each at most once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Starts(u8);

impl Starts {
    /// The empty set.
    pub const NONE: Self = Self(0);

    /// The set of `start` alone.
    pub const fn of(start: Start) -> Self {
        Self(start.bit())
    }

    /// This set and `start`.
    #[must_use]
    pub const fn with(self, start: Start) -> Self {
        Self(self.0 | start.bit())
    }

    /// The kinds in either set.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether `start` is in the set.
    pub const fn contains(self, start: Start) -> bool {
        self.0 & start.bit() != 0
    }

    /// Whether the set is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The kinds, in [`Start::ALL`]'s order.
    pub fn iter(self) -> impl Iterator<Item = Start> {
        Start::ALL
            .into_iter()
            .filter(move |start| self.contains(*start))
    }

    /// The kinds that run a program.
    #[must_use]
    pub fn programs(self) -> Self {
        self.only(Runs::Program)
    }

    /// The kinds that are oso's own work.
    #[must_use]
    pub fn in_oso(self) -> Self {
        self.only(Runs::InOso)
    }

    fn only(self, runs: Runs) -> Self {
        self.iter()
            .filter(|start| start.runs() == runs)
            .fold(Self::NONE, Self::with)
    }

    /// What is shown of the set, each name once, in [`Start::ALL`]'s order: `server`. It
    /// allocates nothing, as the modeline asks for it each frame.
    pub fn shown(self) -> impl Iterator<Item = &'static str> {
        self.iter().enumerate().filter_map(move |(n, start)| {
            let name = start.shown();
            let earlier = self.iter().take(n).any(|before| before.shown() == name);
            (!earlier).then_some(name)
        })
    }
}

/// Who opened or saved a buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acted {
    /// The person, from the window.
    Person,
    /// A command the agent channel ran (ADR 0085).
    Agent,
}

/// What waits in a buffer for the person (ADR 0118): what an agent's open would have started,
/// until the person's first command there, and what an agent's save would have started, until
/// the person's next save. Only kinds that run a program are kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Waits {
    pub(crate) open: Starts,
    pub(crate) save: Starts,
    /// The person let an agent's open go here: the open counts as theirs from now on, also
    /// after a restart (ADR 0118, Q8).
    pub(crate) open_let_go: bool,
}

impl Buffer {
    /// What an agent's open would have started here, which waits for the person's first command
    /// in this buffer.
    pub fn open_waits(&self) -> Starts {
        self.waits.open
    }

    /// What an agent's save would have started here, which waits for the person's next save.
    pub fn save_waits(&self) -> Starts {
        self.waits.save
    }

    /// Both marks, for what the modeline and the channel say waits.
    pub fn waits(&self) -> Starts {
        self.waits.open.union(self.waits.save)
    }

    /// Both marks, for the editor to set and let go.
    pub(crate) fn waits_mut(&mut self) -> &mut Waits {
        &mut self.waits
    }

    /// Whether the person let an agent's open go here, so that the open counts as theirs.
    pub fn open_let_go(&self) -> bool {
        self.waits.open_let_go
    }

    /// Puts back the marks a restart carried (ADR 0118, Q8), the kinds that run a program only.
    /// Whether the person let an agent's open go stays as it is.
    pub fn carry_waits(&mut self, open: Starts, save: Starts) {
        self.waits.open = open.programs();
        self.waits.save = save.programs();
    }

    /// Notes that the person let an agent's open go here, as a restart carries it.
    pub fn carry_open_let_go(&mut self) {
        self.waits.open_let_go = true;
    }
}

#[cfg(test)]
mod tests {
    use super::{Runs, Start, Starts};
    use crate::buffer::Buffer;

    /// Every kind is in [`Start::ALL`], once and in its place: a new kind that is left out
    /// would be neither run nor held, and would not show in the guard test's list.
    #[test]
    fn all_lists_every_kind_in_its_place() {
        for (place, start) in Start::ALL.into_iter().enumerate() {
            let expected = match start {
                Start::ServerStart => 0,
                Start::ServerOpen => 1,
                Start::ServerSave => 2,
            };
            assert_eq!(place, expected, "{start:?}");
            assert_eq!(start as usize, place, "a bit of its own");
        }
    }

    #[test]
    fn every_kind_has_an_id_that_names_it_back() {
        for start in Start::ALL {
            assert_eq!(Start::from_id(start.id()), Some(start));
        }
        assert_eq!(Start::from_id("format"), None);
    }

    #[test]
    fn a_set_keeps_each_kind_once_and_shows_each_name_once() {
        let set = Starts::of(Start::ServerSave)
            .with(Start::ServerStart)
            .with(Start::ServerStart);
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            [Start::ServerStart, Start::ServerSave]
        );
        assert_eq!(set.shown().collect::<Vec<_>>(), ["server"]);
        assert!(Starts::NONE.is_empty() && Starts::NONE.shown().next().is_none());
    }

    #[test]
    fn programs_and_oso_s_own_work_part_a_set() {
        let set = Start::ALL.into_iter().fold(Starts::NONE, Starts::with);
        assert!(Start::ALL.iter().all(|start| start.runs() == Runs::Program));
        assert_eq!(set.programs(), set);
        assert!(set.in_oso().is_empty());
    }

    #[test]
    fn carried_marks_keep_only_programs() {
        let mut buffer = Buffer::from_text("main.rs", "");
        assert!(buffer.waits().is_empty());
        buffer.carry_waits(
            Starts::of(Start::ServerStart),
            Starts::of(Start::ServerSave),
        );
        assert_eq!(buffer.open_waits(), Starts::of(Start::ServerStart));
        assert_eq!(buffer.save_waits(), Starts::of(Start::ServerSave));
        assert_eq!(buffer.waits().shown().collect::<Vec<_>>(), ["server"]);
    }
}
