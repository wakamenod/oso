//! A docked window (ADR 0170): a window the split tree does not hold, laid out inside the tiled
//! window that shows its host buffer, in one of the host's slots.
//!
//! Must not depend on: `editor` or `frame`; the window only says where it is docked.

use super::{Window, WindowId};
use crate::buffer::BufferId;

/// Where a docked window goes under its host buffer's window (ADR 0170), top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DockSlot {
    /// The to-do strip.
    Strip,
    /// A permission card.
    Card,
    /// The composer.
    Composer,
}

impl DockSlot {
    /// Every slot, top to bottom.
    pub(crate) const ALL: [Self; 3] = [Self::Strip, Self::Card, Self::Composer];
}

impl Window {
    /// The host buffer and slot it is docked under, if it is a docked window (ADR 0170): laid
    /// out inside the window that shows its host, not in the split tree.
    pub fn docked(&self) -> Option<(BufferId, DockSlot)> {
        self.docked
    }

    /// A docked window named `id` showing `buffer` under `host`'s window, in `slot`.
    pub(crate) fn new_docked(
        id: WindowId,
        buffer: BufferId,
        host: BufferId,
        slot: DockSlot,
    ) -> Self {
        Self {
            docked: Some((host, slot)),
            ..Self::new(id, buffer)
        }
    }
}
