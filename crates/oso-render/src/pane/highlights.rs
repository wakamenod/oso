//! The face each kind of highlighted text and each display cell style is drawn in: the map
//! from what `oso-core` marks to the renderer's faces (ADR 0061, ADR 0111).
//!
//! Must not depend on: `atlas`, `cells`, `grid`, `rows`, `scene`, `shape`, `text` or the
//! GPU, as the rest of the pane's types.

use oso_core::display_map::CellStyle;
use oso_core::syntax::Highlight;

use super::Face;

impl Face {
    /// The face text that is `highlight` is drawn in (ADR 0111).
    pub fn of_highlight(highlight: Highlight) -> Self {
        match highlight {
            Highlight::Comment => Self::SyntaxComment,
            Highlight::String => Self::SyntaxString,
            Highlight::Keyword => Self::SyntaxKeyword,
            Highlight::Type => Self::SyntaxType,
            Highlight::Function => Self::SyntaxFunction,
            Highlight::Constant => Self::SyntaxConstant,
            Highlight::Property => Self::SyntaxProperty,
            Highlight::Macro => Self::SyntaxMacro,
            Highlight::Heading => Self::SyntaxHeading,
            Highlight::Link => Self::SyntaxLink,
            Highlight::Markup => Self::SyntaxMarkup,
            Highlight::DiffAdded => Self::DiffAdded,
            Highlight::DiffRemoved => Self::DiffRemoved,
            Highlight::DiffFileHeader => Self::DiffFileHeader,
            Highlight::DiffHunkHeader => Self::DiffHunkHeader,
            // Their background comes from the cells refinement marks; the text is as its line's.
            Highlight::DiffRefineAdded => Self::DiffAdded,
            Highlight::DiffRefineRemoved => Self::DiffRemoved,
            Highlight::CommentPerson => Self::ReviewCommentPerson,
            Highlight::CommentAgent => Self::ReviewCommentAgent,
            Highlight::Outdated => Self::ReviewOutdated,
            Highlight::Sent => Self::ReviewSent,
            Highlight::Decided => Self::ReviewDecided,
            Highlight::SessionAlert => Self::SessionAlert,
            Highlight::SessionAlertLit => Self::SessionAlertLit,
            Highlight::CardTitle => Self::CardTitle,
            // As a menu's key, on its chip (ADR 0117).
            Highlight::CardKey => Self::MenuKey,
            Highlight::SessionWorking => Self::SessionWorking,
            Highlight::SessionDone => Self::SessionDone,
            Highlight::SessionQuiet => Self::SessionQuiet,
            Highlight::SessionFront => Self::SessionFront,
            Highlight::SessionSpace => Self::SessionSpace,
            Highlight::SessionRepository => Self::SessionRepository,
            Highlight::DiredHeader => Self::DiredHeader,
            Highlight::DiredDirectory => Self::DiredDirectory,
            Highlight::DiredSymlink => Self::DiredSymlink,
            Highlight::DiredBrokenSymlink => Self::DiredBrokenSymlink,
            Highlight::DiredExecutable => Self::DiredExecutable,
            Highlight::DiredSpecial => Self::DiredSpecial,
            Highlight::DiredDetails => Self::DiredDetails,
            Highlight::DiredMark => Self::DiredMark,
            Highlight::DiredMarked => Self::DiredMarked,
            Highlight::GitSectionHeading => Self::GitSectionHeading,
            Highlight::GitBranchLocal => Self::GitBranchLocal,
            Highlight::GitBranchRemote => Self::GitBranchRemote,
            Highlight::GitHash => Self::GitHash,
            Highlight::GitDetail => Self::GitDetail,
            Highlight::GitCommitOverlong => Self::GitCommitOverlong,
            Highlight::GitLane0 => Self::GitLane0,
            Highlight::GitLane1 => Self::GitLane1,
            Highlight::GitLane2 => Self::GitLane2,
            Highlight::GitLane3 => Self::GitLane3,
            Highlight::GitLane4 => Self::GitLane4,
            Highlight::GitLane5 => Self::GitLane5,
            Highlight::GitLane6 => Self::GitLane6,
            Highlight::GitLane7 => Self::GitLane7,
            Highlight::GitTag => Self::GitTag,
        }
    }

    /// The face of a run in `style` within text of this face. An escape looks
    /// like an escape wherever it is.
    pub(crate) fn of_run(self, style: CellStyle) -> Self {
        match style {
            CellStyle::Text => self,
            CellStyle::Escape => Self::Escape,
            CellStyle::Boxed => Self::Boxed,
            CellStyle::Whitespace => Self::Whitespace,
        }
    }
}
