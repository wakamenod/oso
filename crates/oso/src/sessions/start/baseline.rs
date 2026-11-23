//! A new session's baseline, taken by a review's thread of its own before the session starts
//! (ADR 0122), and waited for on its start thread.
//!
//! Must not depend on: `app` or the UI thread; it runs on the session's start thread.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use crate::git::TrustedRoot;
use crate::review::{self, Reviewer};

/// A baseline being taken by a review's thread of its own.
pub(super) struct Baseline {
    _reviewer: Reviewer,
    trees: mpsc::Receiver<Option<String>>,
}

impl Baseline {
    pub(super) fn start(root: TrustedRoot, state: Option<PathBuf>) -> Option<Self> {
        let (sender, trees) = mpsc::channel();
        let reviewer = Reviewer::start(root, state, move |report| {
            let tree = match report {
                review::Report::Baseline { tree, .. } => Some(tree.as_str().to_owned()),
                review::Report::Failed { why, .. } => {
                    log::warn!("a session's baseline was not taken: {why}");
                    None
                }
                _ => return true,
            };
            let _ = sender.send(tree);
            false
        })
        .map_err(|e| log::warn!("a session's baseline cannot be taken: {e}"))
        .ok()?;
        reviewer.ask(review::Request::Baseline).then_some(Self {
            _reviewer: reviewer,
            trees,
        })
    }

    /// The tree, `Some(None)` if it failed, or `None` if it did not come within `wait`.
    pub(super) fn wait(&self, wait: Duration) -> Option<Option<String>> {
        match self.trees.recv_timeout(wait) {
            Ok(tree) => Some(tree),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => Some(None),
        }
    }
}
