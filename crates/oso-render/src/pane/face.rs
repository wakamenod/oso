//! The faces a pane's parts are drawn in (ADR 0089). What highlighted text and each cell style
//! take is beside it, in highlights.rs.
//!
//! Must not depend on: `atlas`, `cells`, `grid`, `rows`, `scene`, `shape`, `text` or the
//! GPU.

/// What a piece of the screen means. The current theme gives it its colours (ADR 0089).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    /// Text, on the background behind everything else.
    Text,
    /// The label of an escape (`^M`, `\xFF`), not to be mistaken for text
    /// that reads the same.
    Escape,
    /// The label of a boxed code point (`U+202E`), and its box. Also a cluster
    /// the renderer shapes only in part, past its budget of characters.
    Boxed,
    /// The cursor box, and the glyphs on it.
    Cursor,
    /// The outline of a cursor other than the primary, dimmer than the cursor box, over
    /// glyphs as they are (ADR 0091). Only its background is drawn.
    SecondaryCursor,
    /// A modeline, and its text: the selected window's.
    Modeline,
    /// The modeline of a window that is not selected, and its text, dimmer (ADR 0112).
    ModelineInactive,
    /// The line between side by side windows, down the middle of its column (ADR 0112).
    Divider,
    /// A warning that must be seen, on whatever is behind it.
    Warning,
    /// Text an input method is composing, and its underline (ADR 0069). It is
    /// shaped apart from the text around it.
    Preedit,
    /// A float's text, on its own background (ADR 0072).
    Float,
    /// The hairline around a float's panel (ADR 0075).
    FloatBorder,
    /// What a prompt says before its answer, dim on the float's background.
    Prompt,
    /// What a question asks, in a colour no buffer text is drawn in (ADR 0089).
    Question,
    /// An input field in a float: the text's background, and its outline in the foreground
    /// (ADR 0075).
    Field,
    /// A key's chip, such as a question's `y`, and the key on it.
    Key,
    /// The region between a mark and its cursor (ADR 0071). Only its background
    /// is drawn, behind the text, which keeps its own face.
    Region,
    /// The selected candidate's row in a float: its box's background, behind the text, which
    /// keeps its own faces (ADR 0076).
    CompletionSelected,
    /// What the input matched in a candidate.
    CompletionMatch,
    /// What is said of a candidate at the end of its row, and a list's count.
    CompletionAnnotation,
    /// A key sequence after a candidate, such as a command's binding.
    CompletionBinding,
    /// A completion's count and label, at the start of its answer's row.
    CompletionPrompt,
    /// An isearch match in view, drawn as the region is (ADR 0082).
    SearchMatch,
    /// The current isearch match, which stands out from the others.
    SearchCurrent,
    /// A whitespace mark: `»`, `□` or `·` (ADR 0097). Used by nothing else, so a mark is
    /// never taken for text that reads the same.
    Whitespace,
    /// Trailing whitespace's fill, behind the text.
    TrailingWhitespace,
    /// The band on the primary cursor's row, at the back of the fills.
    CurrentLine,
    /// The cursor flash after a large jump, over the fills, at a strength that fades.
    Flash,
    /// A line's number in the gutter (ADR 0097).
    LineNumber,
    /// The primary cursor's line's number, brighter than the others.
    LineNumberCurrent,
    /// The mark `↩` in the last column of a view row that continues (ADR 0100). Used by
    /// nothing else, so it is never taken for text that reads the same.
    WrapMark,
    /// Text that highlighting says is a comment (ADR 0111). A syntax face is a foreground on
    /// the text's background, and a glyph takes one only where it would be [`Face::Text`].
    SyntaxComment,
    /// A string or a character, with its escapes.
    SyntaxString,
    /// A keyword, or a label.
    SyntaxKeyword,
    /// A type, or a constructor.
    SyntaxType,
    /// A function or a method.
    SyntaxFunction,
    /// A constant, a number or a boolean.
    SyntaxConstant,
    /// A field, or a key in TOML.
    SyntaxProperty,
    /// A macro or an attribute.
    SyntaxMacro,
    /// A heading in Markdown.
    SyntaxHeading,
    /// A link or a reference in Markdown.
    SyntaxLink,
    /// Emphasis, strong text and the markers of Markdown.
    SyntaxMarkup,
    /// The first key of a jump's label (ADR 0116): a strong fill no other face uses, with its
    /// text at 7:1, so a label is never taken for an escape, a match or text.
    JumpLabel,
    /// The keys of a jump's label after its first, in a fill of their own, as strong.
    JumpLabelNext,
    /// A menu head's key and its chip (ADR 0117): a head that closes the menu.
    MenuKey,
    /// The key and chip of a head that keeps the menu open, in a red hue.
    MenuKeyStay,
    /// The key and chip of a switch, and the chip of a switch's `[x]` while it is on.
    MenuKeySwitch,
    /// A menu's title and its columns' titles, on the panel.
    MenuTitle,
    /// A menu's notes, rules, `… N more`, the prefix argument and an unavailable head, dim on
    /// the panel.
    MenuNote,
    /// A line a review's diff puts in (ADR 0122): its text, and the band behind its row.
    DiffAdded,
    /// A line a review's diff takes out, and its band.
    DiffRemoved,
    /// The words that changed within an added line: a stronger band, behind the text.
    DiffRefineAdded,
    /// The words that changed within a removed line.
    DiffRefineRemoved,
    /// A hunk's header, on the headers' band.
    DiffHunkHeader,
    /// A file's header, and the band behind every header row.
    DiffFileHeader,
    /// The label of the person's comment, and the band behind every comment's rows.
    ReviewCommentPerson,
    /// The label of an agent's comment, in a hue of its own, on the comments' band.
    ReviewCommentAgent,
    /// What says a comment is outdated, dim.
    ReviewOutdated,
    /// A comment of the person's sent to the agent: its bar, label and text, dim on the
    /// comments' band (ADR 0122, step 12).
    ReviewSent,
    /// What says a hunk was accepted, rejected or viewed, on the headers' band (ADR 0122,
    /// step 5).
    ReviewDecided,
    /// The band behind a pane's filler rows, which hold nothing and line it up with the other
    /// pane (ADR 0122, step 11).
    DiffFiller,
    /// A session's mark that needs the person (ADR 0131): needs input or failed, in red.
    SessionAlert,
    /// The modeline's `● REC` while a dictation records (ADR 0175), in the alert's red.
    Dictation,
    /// The fill behind a dictation's text while Claude fixes it (ADR 0175, step 2): a faint
    /// tint of its own, behind the text, which keeps its colour.
    DictationProvisional,
    /// A native session's permission card (ADR 0180): the background of its own it is drawn
    /// on, apart from the text's and a float's.
    Card,
    /// The card's frame and what oso says in it, and the band behind its title row.
    CardTitle,
    /// A waiting session's mark and name on the lit beat of its blink (ADR 0140, as amended):
    /// in the text's background colour, on a fill in the alert's red, as inverse video.
    SessionAlertLit,
    /// A working session's mark, in amber.
    SessionWorking,
    /// The mark of a session done or ready for review, in green.
    SessionDone,
    /// What `*Spaces*` says quietly: idle and stopped marks, places, reasons, and the rows
    /// of sessions run outside oso.
    SessionQuiet,
    /// The name of the worktree whose space is in front, in an accent of its own.
    SessionFront,
    /// A space's name in `*Spaces*` (ADR 0140), as a heading.
    SessionSpace,
    /// A repository's name in `*Spaces*` (ADR 0140).
    SessionRepository,
    /// What the person said in a native session, and the band behind it (ADR 0165).
    SessionPrompt,
    /// The mark before it, on its band.
    SessionPromptMark,
    /// What an agent wrote to start a native session, and the band behind it, in a hue apart
    /// from the person's.
    SessionBrief,
    /// Who it is from and the mark before it, on its band.
    SessionBriefMark,
    /// A tool's name on a native session's row, in no colour a reply's text has.
    SessionTool,
    /// Inline code in a reply, on the text's background.
    SessionCode,
    /// What stands for a bullet, a list's number, a quote or a rule in a reply.
    SessionBullet,
    /// The box rules of a table in a reply.
    SessionTable,
    /// A heading in a reply, in bold (ADR 0165, "The look: styles").
    SessionHeading,
    /// Strong text in a reply and a table's header, in bold.
    SessionStrong,
    /// Inline code in strong text or a heading, in bold.
    SessionStrongCode,
    /// Emphasis in a reply, in italic.
    SessionEmphasis,
    /// A link in a reply, underlined.
    SessionLink,
    /// What Claude thought, in italic.
    SessionThinking,
    /// The mark of a call or of thinking under way.
    SessionRunning,
    /// The badge of a tool that reads, and the chip behind it, as the 7 below.
    SessionIconRead,
    /// One that changes files.
    SessionIconWrite,
    /// One that runs commands.
    SessionIconShell,
    /// One that searches.
    SessionIconSearch,
    /// One that runs an agent.
    SessionIconAgent,
    /// One that reads the web.
    SessionIconWeb,
    /// One that keeps tasks.
    SessionIconTask,
    /// Any other.
    SessionIconOther,
    /// The band behind a space's row and the pane's other sections' (ADR 0140): a fill.
    SessionHeader,
    /// The faint band behind the front worktree's rows (ADR 0140): a fill.
    SessionFrontTint,
    /// The band behind the row of the session the tree's window shows (ADR 0140), clearer
    /// than the front worktree's tint: a fill.
    SessionShown,
    /// The band of the row at point while `*Spaces*` is selected (ADR 0140): a fill.
    SessionPoint,
    /// The bar in the first cell of that row, a fill in an accent.
    SessionPointBar,
    /// A directory listing's header, its path (ADR 0134), as a heading.
    DiredHeader,
    /// A directory's name in a listing.
    DiredDirectory,
    /// A link's name in a listing.
    DiredSymlink,
    /// The name of a link that leads nowhere, in red.
    DiredBrokenSymlink,
    /// The name of a file someone may run, in green.
    DiredExecutable,
    /// The name of a pipe, a socket or a device.
    DiredSpecial,
    /// A row's mode, owner, size and time, dim.
    DiredDetails,
    /// A row's mark, `*`.
    DiredMark,
    /// The name of a marked row.
    DiredMarked,
    /// A git status section's heading (ADR 0127), as a heading.
    GitSectionHeading,
    /// A local branch's name in a git status.
    GitBranchLocal,
    /// A remote branch's name in a git status, such as its upstream.
    GitBranchRemote,
    /// A commit's short id, dim.
    GitHash,
    /// What a git status says quietly, dim: counts, notes and marks.
    GitDetail,
    /// What a commit's message holds past its line's length, in the warning's colour.
    GitCommitOverlong,
    /// A graph's lanes (ADR 0127, step 4), each in a colour of its own, in turn.
    GitLane0,
    GitLane1,
    GitLane2,
    GitLane3,
    GitLane4,
    GitLane5,
    GitLane6,
    GitLane7,
    /// A tag's name on a graph's row.
    GitTag,
    /// An error a language server reports (ADR 0138): the wavy line under its range, its
    /// line's sign in the gutter and the modeline's `E2`, each in this face's foreground.
    DiagnosticError,
    /// A warning, as an error is drawn.
    DiagnosticWarning,
    /// Information, as an error is drawn. A hint is drawn in none of these.
    DiagnosticInfo,
    /// A terminal's colour 0, black (ADR 0120): one colour, its foreground, which a cell takes
    /// for its text or its background. A theme gives the 16 colours that suit it.
    TerminalColor0,
    /// A terminal's colour 1, red.
    TerminalColor1,
    /// A terminal's colour 2, green.
    TerminalColor2,
    /// A terminal's colour 3, yellow.
    TerminalColor3,
    /// A terminal's colour 4, blue.
    TerminalColor4,
    /// A terminal's colour 5, magenta.
    TerminalColor5,
    /// A terminal's colour 6, cyan.
    TerminalColor6,
    /// A terminal's colour 7, white.
    TerminalColor7,
    /// A terminal's colour 8, bright black.
    TerminalColor8,
    /// A terminal's colour 9, bright red.
    TerminalColor9,
    /// A terminal's colour 10, bright green.
    TerminalColor10,
    /// A terminal's colour 11, bright yellow.
    TerminalColor11,
    /// A terminal's colour 12, bright blue.
    TerminalColor12,
    /// A terminal's colour 13, bright magenta.
    TerminalColor13,
    /// A terminal's colour 14, bright cyan.
    TerminalColor14,
    /// A terminal's colour 15, bright white.
    TerminalColor15,
}

#[cfg(test)]
mod tests {
    use oso_core::display_map::CellStyle;

    use super::*;

    #[test]
    fn a_label_keeps_its_face_inside_text_of_another() {
        assert_eq!(Face::Cursor.of_run(CellStyle::Text), Face::Cursor);
        assert_eq!(Face::Cursor.of_run(CellStyle::Escape), Face::Escape);
        assert_eq!(Face::Cursor.of_run(CellStyle::Boxed), Face::Boxed);
    }
}
