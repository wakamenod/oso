//! A rendered row's faces as the highlights the buffer holds: what the host lays over a row's
//! text, and the face each is drawn in.
//!
//! Must not depend on: the theme; a [`Highlight`] names what text is, not its colour.

use oso_core::syntax::Highlight;

use super::{Face, Icon, Rendered};
use crate::sessions::native::shown::Style;

impl Face {
    /// The highlight the theme draws this face with.
    pub fn highlight(self) -> Highlight {
        match self {
            Self::Prompt => Highlight::CommentPerson,
            Self::PromptMark => Highlight::SessionPromptMark,
            Self::Said => Highlight::SessionPrompt,
            Self::BriefMark => Highlight::SessionBriefMark,
            Self::Brief => Highlight::SessionBrief,
            Self::Dim => Highlight::SessionQuiet,
            // No parser gives these, so no reply's code has a tool row's colours.
            Self::Icon(icon) => icon.highlight(),
            Self::Tool => Highlight::SessionTool,
            Self::Running => Highlight::SessionRunning,
            Self::Target => Highlight::String,
            Self::Added => Highlight::DiffAdded,
            Self::Removed => Highlight::DiffRemoved,
            Self::Error => Highlight::SessionAlert,
            Self::Waiting => Highlight::SessionWorking,
            Self::Reply(Style::Heading) => Highlight::Heading,
            Self::Reply(Style::Strong) => Highlight::Markup,
            Self::Reply(Style::Mark) => Highlight::SessionBullet,
            Self::Reply(Style::Rule) => Highlight::SessionTable,
            Self::Reply(Style::Code) => Highlight::SessionCode,
            Self::Reply(Style::Link) => Highlight::Link,
            Self::CardTitle => Highlight::CardTitle,
            Self::CardKey => Highlight::CardKey,
        }
    }
}

impl Icon {
    /// The highlight of its badge, which its chip is drawn by.
    fn highlight(self) -> Highlight {
        match self {
            Self::Read => Highlight::SessionIconRead,
            Self::Write => Highlight::SessionIconWrite,
            Self::Shell => Highlight::SessionIconShell,
            Self::Search => Highlight::SessionIconSearch,
            Self::Agent => Highlight::SessionIconAgent,
            Self::Web => Highlight::SessionIconWeb,
            Self::Task => Highlight::SessionIconTask,
            Self::Other => Highlight::SessionIconOther,
        }
    }
}

impl Rendered {
    /// Its faces as runs over its text, with nothing between them.
    pub fn runs(&self) -> Vec<(usize, Option<Highlight>)> {
        let mut runs = Vec::new();
        let mut at = 0;
        for (range, face) in &self.faces {
            if range.start > at {
                runs.push((range.start - at, None));
            }
            runs.push((range.end - range.start, Some(face.highlight())));
            at = range.end;
        }
        runs
    }
}
