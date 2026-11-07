//! Which keys a buffer takes and what its modeline calls it (ADR 0064). This is a closed list
//! until major modes arrive in M3, which replace it with something the tip crate can add to.
//!
//! Must not depend on: `window`, `editor` or `command`.

/// Which keys a buffer takes and what its modeline calls it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    /// A buffer of the user's, or `*Messages*`.
    #[default]
    Fundamental,
    /// The list of emergency copies.
    CopyList,
    /// The text of one emergency copy, or how it differs from its file.
    CopyView,
}

impl Mode {
    /// What the modeline calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Fundamental => "Fundamental",
            Self::CopyList => "Emergency Copies",
            Self::CopyView => "Emergency Copy",
        }
    }

    /// Whether this is one of the buffers that show emergency copies.
    pub fn shows_copies(self) -> bool {
        matches!(self, Self::CopyList | Self::CopyView)
    }
}
