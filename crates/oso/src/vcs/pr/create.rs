//! Making and merging pull requests (ADR 0127, step 5a-2): what a pull request's message starts
//! from, the title and body read back from it, and the runs of `gh pr create` and `gh pr merge`
//! with what they say.
//!
//! Must not depend on: `app`, `dispatch`, processes or the file system; pure functions of what
//! the thread read and the person wrote.
//!
//! - A message's first line is its title and the rest its body; nothing is stripped, as a
//!   body's Markdown headings start with `#` (Q9). An empty title, or one past GitHub's 256
//!   characters, is refused.
//! - Every value goes to `gh` as `--flag=value`, so that a title, a base or a branch that
//!   starts with `-` is never read as an option; the body goes on standard input.
//! - A merge names the head commit the person was asked about, so that `gh` refuses it if the
//!   pull request moved since (Q6).

use oso_core::editor::MergeMethod;

/// The places a pull request's template is looked for, from the repository's root, in turn (Q8).
pub(crate) const TEMPLATE_DIRS: [&str; 3] = [".github", "", "docs"];
/// The template's name, in any case.
pub(crate) const TEMPLATE_NAME: &str = "pull_request_template.md";
/// The most bytes of a template read.
pub(crate) const MAX_TEMPLATE: usize = 64 * 1024;
/// The most commits a message starts from.
pub(crate) const MAX_COMMITS: usize = 50;
/// The longest title GitHub takes.
const MAX_TITLE: usize = 256;
/// The most bytes of a pull request's message read back from its buffer.
pub(crate) const MAX_PR_MESSAGE: usize = 256 * 1024;

/// A commit the branch has that its base lacks: its subject and its whole message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BranchCommit {
    pub(crate) subject: String,
    pub(crate) message: String,
}

/// The commits in `git log -z --format=%s%x1f%B`'s output, at most [`MAX_COMMITS`]; one of
/// another shape is left out.
pub(crate) fn read_commits(bytes: &[u8]) -> Vec<BranchCommit> {
    (bytes.split(|&b| b == 0))
        .map(|record| record.strip_prefix(b"\n").unwrap_or(record))
        .filter_map(|record| {
            let at = record.iter().position(|&b| b == 0x1f)?;
            let (subject, message) = (&record[..at], &record[at + 1..]);
            Some(BranchCommit {
                subject: String::from_utf8_lossy(subject).trim().to_owned(),
                message: String::from_utf8_lossy(message).into_owned(),
            })
        })
        .take(MAX_COMMITS)
        .collect()
}

/// What a pull request's message starts from: the branch's single commit's message, or the
/// oldest subject as the title with every subject below it, oldest first; then the template.
/// `commits` come newest first, as `git log` lists them.
pub(crate) fn draft_text(commits: &[BranchCommit], template: Option<&str>) -> String {
    let mut text = match commits {
        [] => String::from("\n"),
        [one] => {
            let message = one.message.trim_end();
            if message.is_empty() {
                format!("{}\n", one.subject)
            } else {
                format!("{message}\n")
            }
        }
        many => {
            let mut text = format!("{}\n\n", many[many.len() - 1].subject);
            for commit in many.iter().rev() {
                text.push_str(&format!("- {}\n", commit.subject));
            }
            text
        }
    };
    if let Some(template) = template.map(str::trim_end).filter(|t| !t.is_empty()) {
        text.push('\n');
        text.push_str(template);
        text.push('\n');
    }
    text
}

/// The title and body of `text`, a pull request's message.
///
/// # Errors
///
/// Why it is no pull request's: its title is empty, or longer than GitHub takes.
pub(crate) fn title_and_body(text: &str) -> Result<(String, String), &'static str> {
    let (title, body) = text.split_once('\n').unwrap_or((text, ""));
    let title = title.trim();
    if title.is_empty() {
        return Err("A pull request needs a title: its first line");
    }
    if title.chars().count() > MAX_TITLE {
        return Err("The title is longer than GitHub takes: 256 characters");
    }
    let body = body.trim_start_matches(['\n', '\r']).trim_end();
    Ok((title.to_owned(), body.to_owned()))
}

/// What a pull request's run does, on the repository's run thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PrRun {
    /// Make the pull request of `head` into `base` in `repo`, `HOST/OWNER/REPO`.
    Create {
        repo: String,
        base: String,
        head: String,
        title: String,
        body: String,
        draft: bool,
    },
    /// Merge pull request `number` of `repo` into `base` as `method`, if its head commit is
    /// still `head_commit`, deleting its branch on the remote if `delete_branch`.
    Merge {
        repo: String,
        number: u64,
        method: MergeMethod,
        base: String,
        head_commit: String,
        delete_branch: bool,
    },
}

impl PrRun {
    /// The repository it is of.
    pub(crate) fn repo(&self) -> &str {
        match self {
            Self::Create { repo, .. } | Self::Merge { repo, .. } => repo,
        }
    }

    /// `gh`'s arguments, and what goes on its standard input.
    pub(crate) fn args(&self) -> (Vec<String>, Option<Vec<u8>>) {
        match self {
            Self::Create {
                repo,
                base,
                head,
                title,
                body,
                draft,
            } => {
                let mut args = vec![
                    "pr".to_owned(),
                    "create".to_owned(),
                    format!("--repo={repo}"),
                    format!("--title={title}"),
                    "--body-file=-".to_owned(),
                    format!("--base={base}"),
                    format!("--head={head}"),
                ];
                if *draft {
                    args.push("--draft".to_owned());
                }
                (args, Some(body.clone().into_bytes()))
            }
            Self::Merge {
                repo,
                number,
                method,
                head_commit,
                delete_branch,
                ..
            } => {
                let mut args = vec![
                    "pr".to_owned(),
                    "merge".to_owned(),
                    number.to_string(),
                    format!("--repo={repo}"),
                    format!("--{}", method_word(*method)),
                    format!("--match-head-commit={head_commit}"),
                ];
                if *delete_branch {
                    args.push("--delete-branch".to_owned());
                }
                (args, None)
            }
        }
    }

    /// What the log and the echo area call it.
    pub(crate) fn title(&self) -> String {
        match self {
            Self::Create { head, base, .. } => format!("gh pr create {head} into {base}"),
            Self::Merge { number, method, .. } => {
                format!("gh pr merge #{number} --{}", method_word(*method))
            }
        }
    }

    /// What the echo area says once it is done.
    pub(crate) fn done(&self) -> String {
        match self {
            Self::Create { head, base, .. } => {
                format!("Made the pull request of {head} into {base}: N v shows it")
            }
            Self::Merge { number, .. } => format!("Merged #{number}"),
        }
    }
}

/// What the echo area says of `title`, a run of `gh`, that failed: its last words.
pub(crate) fn failed(title: &str, output: &str) -> String {
    let said = output.lines().rev().find(|line| !line.trim().is_empty());
    match said {
        Some(said) => format!("{title} failed: {}", said.trim()),
        None => format!("{title} failed"),
    }
}

/// The line the log of git's runs shows for `gh` run with `args`: each value quoted, so that a
/// title's words cannot read as flags of their own.
pub(crate) fn log_line(args: &[String]) -> String {
    let quoted: Vec<String> = (args.iter())
        .map(|arg| match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => format!("{flag}='{value}'"),
            _ => arg.clone(),
        })
        .collect();
    format!("$ gh {}\n", quoted.join(" "))
}

/// `gh pr merge`'s word for `method`.
pub(crate) fn method_word(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Squash => "squash",
        MergeMethod::Merge => "merge",
        MergeMethod::Rebase => "rebase",
    }
}

#[cfg(test)]
mod tests;
