//! What a person or a log is told of a scope a review could not read (ADR 0122): words of oso's,
//! with no file's text in them.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; a plain function.

use super::super::scope::ScopeError;

/// What a person reads for `error`, with no file's text.
pub(in crate::review) fn said(error: &ScopeError) -> String {
    match error {
        ScopeError::Git(e) => e.to_string(),
        ScopeError::NoSuchRevision(revision) => format!("no commit is named {revision:?}"),
        ScopeError::BaselineGone => "the session's baseline is gone".into(),
        ScopeError::SinceGone => {
            "the pull request's head at its last review is gone from this clone: the whole \
             pull request still reviews"
                .into()
        }
        ScopeError::NoParent => "its parent is not in this clone".into(),
        ScopeError::Elsewhere => {
            "git found a repository whose top is not the project's root".into()
        }
        ScopeError::NoScratch => {
            "there is no private place for the review's copies of the index and its empty \
             directory for git's hooks"
                .into()
        }
        ScopeError::Copy(e) => format!("cannot copy the index: {e}"),
        ScopeError::Stopped => "stopped".into(),
        ScopeError::Refused(said) => said.clone(),
    }
}
