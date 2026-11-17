//! A review's files pane and its filter (ADR 0122, step 6b): the keys and commands of the
//! narrow window at the left of a review that lists its files, and `/`, which shows only the
//! files whose path or comments hold every word typed.
//!
//! Must not depend on: the host or its threads; the pane's text comes from the review's view
//! thread, and the commands change the editor only.

use oso_core::command::Command;

/// Key sequences and the commands they run, in a review's files pane only.
pub const FILES_KEYS: &[(&str, &str)] = &[];

/// The commands of the files pane and the filter.
pub const COMMANDS: &[Command] = &[];
