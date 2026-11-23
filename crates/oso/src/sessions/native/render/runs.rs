//! A rendered row's faces as the highlights the buffer holds: what the host lays over a row's
//! text, and the face each is drawn in.
//!
//! Must not depend on: the theme; a [`Highlight`] names what text is, not its colour.

use oso_core::syntax::Highlight;

use super::{Face, Rendered};
use crate::sessions::native::shown::Style;

impl Face {
    /// The highlight the theme draws this face with.
    pub fn highlight(self) -> Highlight {
        match self {
            Self::Prompt => Highlight::CommentPerson,
            Self::PromptMark => Highlight::SessionFront,
            Self::Dim => Highlight::SessionQuiet,
            // No parser gives it, so no reply's code has a tool's name's colour.
            Self::Tool => Highlight::SessionRepository,
            Self::Target => Highlight::String,
            Self::Added => Highlight::DiffAdded,
            Self::Removed => Highlight::DiffRemoved,
            Self::Error => Highlight::SessionAlert,
            Self::Waiting => Highlight::SessionWorking,
            Self::Reply(Style::Heading) => Highlight::Heading,
            Self::Reply(Style::Strong | Style::Mark) => Highlight::Markup,
            Self::Reply(Style::Code) => Highlight::String,
            Self::Reply(Style::Link) => Highlight::Link,
            Self::CardTitle => Highlight::CardTitle,
            Self::CardKey => Highlight::CardKey,
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
