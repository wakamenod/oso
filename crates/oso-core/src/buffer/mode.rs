//! Which keys a buffer takes, what its modeline calls it, and which language it holds (ADR
//! 0064, ADR 0111). This is a closed list until major modes arrive in M3, which replace it with
//! something the tip crate can add to.
//!
//! Must not depend on: `window`, `editor` or `command`.

use std::path::Path;

use crate::indent::{self, Answer, Budget, Line, Settings, Style};
use crate::rope::Rope;

/// Which keys a buffer takes, what its modeline calls it, and which language it holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    /// A buffer of the user's in no language oso knows, or `*Messages*`.
    #[default]
    Fundamental,
    /// The list of emergency copies.
    CopyList,
    /// The text of one emergency copy, or how it differs from its file.
    CopyView,
    /// The list of agent sessions, `*Spaces*` (ADR 0121).
    Sessions,
    /// Rust source.
    Rust,
    /// Markdown.
    Markdown,
    /// TOML.
    Toml,
    /// Go source (ADR 0138), highlighted (ADR 0147).
    Go,
    /// TypeScript (ADR 0138), highlighted (ADR 0147).
    TypeScript,
    /// TypeScript with JSX, `.tsx` (ADR 0138), highlighted with a grammar of its own (ADR 0147).
    Tsx,
    /// JavaScript, JSX among it (ADR 0138), highlighted (ADR 0147).
    JavaScript,
    /// HTML, its `<script>` and `<style>` highlighted as the code they hold (ADR 0157).
    Html,
    /// CSS (ADR 0157).
    Css,
    /// JSON, comments allowed (ADR 0157).
    Json,
    /// YAML (ADR 0157).
    Yaml,
    /// LaTeX.
    Latex,
    /// Swift.
    Swift,
    /// `*Brief*`, a worktree session's brief being written (ADR 0121).
    Brief,
    /// A native session's composer, docked under its transcript (ADR 0165, step 2; ADR 0170).
    Composer,
    /// `*Worktrees*`, a repository's linked worktrees (ADR 0121).
    Worktrees,
    /// A review's buffer (ADR 0122).
    Review,
    /// A review's files pane: one row a file (ADR 0122, step 6b).
    ReviewFiles,
    /// A pull request review.s commit list, a strip above the review: one row a commit (ADR
    /// 0167).
    ReviewCommits,
    /// `*review message*`, the message of a review's comments before it is sent to an agent
    /// session (ADR 0122, step 12): the person's to change.
    ReviewMessage,
    /// A terminal (ADR 0120): its keys go to the program in it, but for oso's own.
    Terminal,
    /// A directory's listing, as Emacs's dired shows it (ADR 0134).
    Directory,
    /// A repository's status, as magit's status buffer shows it (ADR 0127).
    GitStatus,
    /// A repository's commits as a graph of lanes, `*graph: NAME*`, as magit's log buffer
    /// (ADR 0127, step 4).
    GitGraph,
    /// A pull request, `*pr: #N*`: its state, checks, review threads and conversation, as `gh`
    /// gives them (ADR 0127, step 5a).
    GitPr,
    /// A repository's open pull requests, `*pull requests: NAME*` (ADR 0127, step 5a).
    GitPrList,
    /// A pull request's title and body being written, `*pull request: BRANCH*` (ADR 0127, step
    /// 5a-2): its first line the title, the rest the body.
    GitPrMessage,
    /// A repository's open issues, `*issues: NAME*` (ADR 0127, step 5b-1).
    GitIssueList,
    /// An issue with its comments, `*issue: #N*` (ADR 0127, step 5b-1).
    GitIssue,
    /// A commit's message being written, `*commit: NAME*`, as magit's `git-commit-mode`
    /// (ADR 0127, step 1b).
    GitCommit,
    /// What git's long runs in a repository wrote, `*git log: NAME*`, as magit's process
    /// buffer.
    GitLog,
    /// What discards in a repository kept, `*git discarded: NAME*` (ADR 0127, step 1c).
    GitDiscarded,
    /// A past agent conversation read, `*conversation: NAME*` (ADR 0145, step 2).
    Conversation,
    /// A rename's edits shown before they apply, `*lsp-rename*` (ADR 0138, step 14).
    RenamePreview,
    /// What oso waits for while a session started outside it moves into a space,
    /// `*moving: NAME*`; killing it cancels the move (ADR 0151).
    SessionMove,
    /// A native agent session's conversation, `*claude: NAME*`, which oso writes from the
    /// session's stream (ADR 0165).
    NativeSession,
    /// A native session's permission card, `*card: NAME*`, docked under its transcript: what a
    /// call that waits for the person asks, which oso writes, with keys of its own to answer it
    /// (ADR 0165, step 2e-2; ADR 0170).
    Card,
    /// The line of a native session's request, `*request: NAME*`, docked under its transcript:
    /// the keys that answer the call whose block ends the transcript, which oso writes
    /// (ADR 0182).
    Request,
    /// A native session's plan being edited before it is approved, `*plan: NAME*`, shown in
    /// place of its transcript: Claude's plan, then the person's (ADR 0165, part 3d).
    Plan,
}

impl Mode {
    /// The mode a file's name gives it, as Emacs's `auto-mode-alist` does: only the name decides,
    /// never what the file holds (ADR 0111). Extensions are compared ignoring ASCII case.
    pub fn for_path(path: &Path) -> Self {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Self::Fundamental;
        };
        if name == "Cargo.lock" {
            return Self::Toml;
        }
        let Some(extension) = Path::new(name).extension().and_then(|e| e.to_str()) else {
            return Self::Fundamental;
        };
        let is = |known: &str| extension.eq_ignore_ascii_case(known);
        if is("rs") {
            Self::Rust
        } else if is("md") || is("markdown") {
            Self::Markdown
        } else if is("toml") {
            Self::Toml
        } else if is("go") {
            Self::Go
        } else if is("ts") || is("mts") || is("cts") {
            Self::TypeScript
        } else if is("tsx") {
            Self::Tsx
        } else if is("js") || is("mjs") || is("cjs") || is("jsx") {
            Self::JavaScript
        } else if is("html") || is("htm") {
            Self::Html
        } else if is("css") {
            Self::Css
        } else if is("json") || is("jsonc") {
            Self::Json
        } else if is("yaml") || is("yml") {
            Self::Yaml
        } else if is("tex") || is("sty") || is("cls") || is("ltx") {
            Self::Latex
        } else if is("swift") {
            Self::Swift
        } else {
            Self::Fundamental
        }
    }

    /// What the modeline calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Fundamental => "Fundamental",
            Self::CopyList => "Emergency Copies",
            Self::CopyView => "Emergency Copy",
            Self::Sessions => "Sessions",
            Self::Rust => "Rust",
            Self::Markdown => "Markdown",
            Self::Toml => "TOML",
            Self::Go => "Go",
            Self::TypeScript => "TypeScript",
            Self::Tsx => "TSX",
            Self::JavaScript => "JavaScript",
            Self::Html => "HTML",
            Self::Css => "CSS",
            Self::Json => "JSON",
            Self::Yaml => "YAML",
            Self::Latex => "LaTeX",
            Self::Swift => "Swift",
            Self::Brief => "Brief",
            Self::Composer => "Compose",
            Self::Worktrees => "Worktrees",
            Self::Review => "Review",
            Self::ReviewFiles => "Review Files",
            Self::ReviewCommits => "Review Commits",
            Self::ReviewMessage => "Review Message",
            Self::Terminal => "Terminal",
            Self::Directory => "Dired",
            Self::GitStatus => "Git",
            Self::GitGraph => "Git Graph",
            Self::GitPr => "Git PR",
            Self::GitPrList => "Git PRs",
            Self::GitPrMessage => "Git PR Message",
            Self::GitIssueList => "Git Issues",
            Self::GitIssue => "Git Issue",
            Self::GitCommit => "Git Commit",
            Self::GitLog => "Git Log",
            Self::GitDiscarded => "Git Discarded",
            Self::Conversation => "Conversation",
            Self::RenamePreview => "Rename",
            Self::SessionMove => "Session Move",
            Self::NativeSession => "Session",
            Self::Card => "Card",
            Self::Request => "Request",
            Self::Plan => "Plan",
        }
    }

    /// Whether this is one of the buffers that show emergency copies.
    pub fn shows_copies(self) -> bool {
        matches!(self, Self::CopyList | Self::CopyView)
    }

    /// The indentation a buffer in this mode takes when neither the configuration nor
    /// EditorConfig gives one (ADR 0115, ADR 0164): two spaces in HTML, CSS, JSON, YAML,
    /// JavaScript and TypeScript, as Prettier writes them; a tab a level in Go, as gofmt does;
    /// four spaces elsewhere. Tabs are eight columns wide.
    pub fn default_indent(self) -> Settings {
        let given = match self {
            Self::Html
            | Self::Css
            | Self::Json
            | Self::Yaml
            | Self::JavaScript
            | Self::TypeScript
            | Self::Tsx => Settings::new(2, Style::Spaces, 8),
            Self::Go => Settings::new(8, Style::Tabs, 8),
            Self::Fundamental
            | Self::CopyList
            | Self::CopyView
            | Self::Sessions
            | Self::Rust
            | Self::Markdown
            | Self::Toml
            | Self::Latex
            | Self::Swift
            | Self::Brief
            | Self::Composer
            | Self::Worktrees
            | Self::Review
            | Self::ReviewFiles
            | Self::ReviewCommits
            | Self::ReviewMessage
            | Self::Terminal
            | Self::Directory
            | Self::GitStatus
            | Self::GitGraph
            | Self::GitPr
            | Self::GitPrList
            | Self::GitPrMessage
            | Self::GitIssueList
            | Self::GitIssue
            | Self::GitCommit
            | Self::GitLog
            | Self::GitDiscarded
            | Self::Conversation
            | Self::RenamePreview
            | Self::SessionMove
            | Self::NativeSession
            | Self::Card
            | Self::Request
            | Self::Plan => None,
        };
        given.unwrap_or_default()
    }

    /// Whether its indentation is made of spaces whatever the setting says: YAML's, as a tab
    /// cannot indent YAML (ADR 0164).
    pub(crate) fn spaces_only(self) -> bool {
        self == Self::Yaml
    }

    /// The rules its lines are indented by (ADR 0115), or none for the emergency copies' and
    /// the worktrees' buffers and the sessions' list, whose text is not the person's to indent,
    /// and for a terminal, which has none. A brief and a commit's message are prose.
    #[expect(
        clippy::type_complexity,
        reason = "the rules' own signature, which naming would make a public item of its own"
    )]
    pub fn indent_rules(self) -> Option<fn(&Rope, Settings, &[Line], &mut Budget) -> Vec<Answer>> {
        match self {
            Self::Rust => Some(indent::rust),
            Self::Toml => Some(indent::toml),
            // A plan is Markdown (ADR 0165, part 3d).
            Self::Markdown | Self::Plan => Some(indent::markdown),
            // ADR 0164.
            Self::Css => Some(indent::css),
            Self::Json => Some(indent::json),
            Self::Html => Some(indent::html),
            Self::JavaScript | Self::Tsx => Some(indent::javascript),
            Self::TypeScript => Some(indent::typescript),
            Self::Go => Some(indent::go),
            Self::Yaml => Some(indent::yaml),
            // LaTeX and Swift have no rules of their own yet: lines keep the one before's.
            Self::Fundamental
            | Self::Brief
            | Self::Composer
            | Self::GitCommit
            | Self::GitPrMessage
            | Self::ReviewMessage
            | Self::Latex
            | Self::Swift => Some(indent::fundamental),
            Self::CopyList
            | Self::CopyView
            | Self::Worktrees
            | Self::Review
            | Self::ReviewFiles
            | Self::ReviewCommits
            | Self::Terminal
            | Self::Directory
            | Self::GitStatus
            | Self::GitGraph
            | Self::GitPr
            | Self::GitPrList
            | Self::GitIssueList
            | Self::GitIssue
            | Self::GitLog
            | Self::Conversation
            | Self::RenamePreview
            | Self::SessionMove
            | Self::NativeSession
            | Self::Card
            | Self::Request
            | Self::GitDiscarded
            | Self::Sessions => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::Mode;

    #[test]
    fn the_modes_of_language_servers_come_from_names_too() {
        let mode = |path: &str| Mode::for_path(Path::new(path));
        for (path, expected) in [
            ("/p/main.go", Mode::Go),
            ("/p/a.ts", Mode::TypeScript),
            ("/p/a.MTS", Mode::TypeScript),
            ("/p/a.cts", Mode::TypeScript),
            ("/p/view.tsx", Mode::Tsx),
            ("/p/a.js", Mode::JavaScript),
            ("/p/a.mjs", Mode::JavaScript),
            ("/p/a.cjs", Mode::JavaScript),
            ("/p/view.jsx", Mode::JavaScript),
            ("/p/paper.tex", Mode::Latex),
            ("/p/style.sty", Mode::Latex),
            ("/p/a.cls", Mode::Latex),
            ("/p/a.ltx", Mode::Latex),
            ("/p/App.swift", Mode::Swift),
            ("/p/index.html", Mode::Html),
            ("/p/OLD.HTM", Mode::Html),
            ("/p/site.css", Mode::Css),
            ("/p/package.json", Mode::Json),
            ("/p/tsconfig.jsonc", Mode::Json),
            ("/p/ci.yml", Mode::Yaml),
            ("/p/compose.yaml", Mode::Yaml),
            ("/p/page.xhtml", Mode::Fundamental),
            ("/p/a.scss", Mode::Fundamental),
            ("/p/go", Mode::Fundamental),
        ] {
            assert_eq!(mode(path), expected, "{path}");
        }
        assert_eq!(Mode::Latex.name(), "LaTeX");
        assert_eq!(Mode::Tsx.name(), "TSX");
        assert_eq!(Mode::Html.name(), "HTML");
        assert_eq!(Mode::Yaml.name(), "YAML");
    }

    #[test]
    fn a_files_name_decides_its_mode() {
        let mode = |path: &str| Mode::for_path(Path::new(path));
        assert_eq!(mode("/src/main.rs"), Mode::Rust);
        assert_eq!(mode("README.md"), Mode::Markdown);
        assert_eq!(mode("notes/Today.MARKDOWN"), Mode::Markdown);
        assert_eq!(mode("Cargo.toml"), Mode::Toml);
        assert_eq!(mode("work/Cargo.lock"), Mode::Toml);
        assert_eq!(mode("LIB.RS"), Mode::Rust);
    }

    #[test]
    fn each_mode_but_the_listings_has_indentation_rules() {
        let modes = [
            Mode::Rust,
            Mode::Toml,
            Mode::Markdown,
            Mode::Fundamental,
            Mode::Brief,
            Mode::Plan,
        ];
        assert!(modes.iter().all(|mode| mode.indent_rules().is_some()));
        assert!(Mode::CopyList.indent_rules().is_none());
        assert!(Mode::CopyView.indent_rules().is_none());
        assert!(Mode::Worktrees.indent_rules().is_none());
        assert!(Mode::Terminal.indent_rules().is_none());
        assert!(Mode::Directory.indent_rules().is_none());
        assert!(Mode::Sessions.indent_rules().is_none());
    }

    #[test]
    fn a_name_with_no_known_extension_is_fundamental() {
        let mode = |path: &str| Mode::for_path(Path::new(path));
        assert_eq!(mode("notes.txt"), Mode::Fundamental);
        assert_eq!(mode(".rs"), Mode::Fundamental);
        assert_eq!(mode("rs"), Mode::Fundamental);
        assert_eq!(mode("cargo.lock"), Mode::Fundamental);
        assert_eq!(mode("main.rs.orig"), Mode::Fundamental);
        assert_eq!(mode("/"), Mode::Fundamental);
        assert_eq!(mode(""), Mode::Fundamental);
    }
}
