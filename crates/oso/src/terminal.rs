//! The terminals the app hosts (ADR 0120): which keys go to a program, what a terminal starts,
//! and, from the snapshots its thread sends, the rows the UI keeps to draw, with the cells the
//! mouse selected.
//!
//! Must not depend on: `app`, `config` or `dispatch`; the app hands settings in and acts on
//! what these say. The terminal model itself, its thread and its policy are in their own
//! crate.

pub(crate) mod colors;
pub(crate) mod grid;
pub(crate) mod hosted;
pub(crate) mod paste;
pub(crate) mod route;
pub(crate) mod selection;
pub(crate) mod shell;
