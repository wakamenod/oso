//! Asking for the person's attention when an agent session needs them (ADR 0121): on macOS a
//! Dock bounce, repeated until oso is in front for what waits on the person, once for what is
//! only news.
//!
//! Must not depend on: `clipboard`, `control`, `process`, `text` or `window`. The window
//! layer asks for attention in its own way when a window does (ADR 0070); this is for oso as a
//! whole.
//!
//! A notification's text is a session's name and its state only, never what its screen shows
//! (ADR 0121). Banners need an app bundle on macOS and a D-Bus client on Linux, which oso has
//! neither of yet, so the text is not shown anywhere today. On Linux and Windows this does
//! nothing: the tip crate sets the window's urgency through its window layer instead.

/// What a session that asks for attention became.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// It waits on a dialog only the person can answer.
    NeedsInput,
    /// Its process ended with an error.
    Failed,
    /// Its turn ended, and the person has not seen it since.
    Done,
    /// Its turn ended with changes no review has shown.
    ReadyForReview,
}

impl Kind {
    /// Whether it waits on the person, so the request goes on until oso is in front. Only
    /// macOS asks in two ways.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn waits(self) -> bool {
        matches!(self, Self::NeedsInput | Self::Failed)
    }
}

/// Asks for the person's attention for a session that became `kind`, named by `title` and
/// said by `body`. Off the main thread, and while oso is in front, nothing happens.
pub fn attention(kind: Kind, title: &str, body: &str) {
    // Banners will show them (see the module's text).
    let _ = (title, body);
    #[cfg(target_os = "macos")]
    macos::attention(kind);
    #[cfg(not(target_os = "macos"))]
    let _ = kind;
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
    use objc2_foundation::MainThreadMarker;

    use super::Kind;

    pub(super) fn attention(kind: Kind) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let request = if kind.waits() {
            NSRequestUserAttentionType::CriticalRequest
        } else {
            NSRequestUserAttentionType::InformationalRequest
        };
        // AppKit ends the request once oso is in front, so its number is not kept.
        let _ = NSApplication::sharedApplication(mtm).requestUserAttention(request);
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, attention};

    #[test]
    fn what_waits_on_the_person_asks_until_oso_is_in_front() {
        assert!(Kind::NeedsInput.waits() && Kind::Failed.waits());
        assert!(!Kind::Done.waits() && !Kind::ReadyForReview.waits());
    }

    #[test]
    fn off_the_main_thread_it_does_nothing() {
        // Tests run on threads of their own, never AppKit's main thread.
        std::thread::spawn(|| attention(Kind::NeedsInput, "oso", "needs input"))
            .join()
            .unwrap();
    }
}
