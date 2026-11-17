//! Putting a patch in the index, or taking it out (ADR 0127, ADR 0122 step 5): the words both
//! the git status and a review run `git apply` with, and what a refusal says.
//!
//! Must not depend on: `vcs`, `review` or the app; each runs the words through its own thread's
//! runner, with its own stop or limit.

/// `git apply`'s words for the patch on standard input into the index, whitespace compared
/// exactly, out of it with `reverse`.
pub(crate) fn words(reverse: bool) -> Vec<&'static str> {
    let mut words = vec![
        "-c",
        "apply.ignoreWhitespace=no",
        "apply",
        "--cached",
        "--whitespace=nowarn",
    ];
    if reverse {
        words.push("-R");
    }
    words.push("-");
    words
}

/// What a refused patch says, from the first line git wrote, which names a path and a line, no
/// file's text.
pub(crate) fn refused(stderr: &str) -> String {
    let first = stderr.lines().next().unwrap_or_default().trim();
    format!("git refused the hunk, as the file changed since it was read: {first}")
}
