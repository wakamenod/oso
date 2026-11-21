//! Every mode's rules and its answer past the budget, for the tests that run them all (ADR
//! 0115, ADR 0164).
//!
//! Must not depend on: `buffer` or `editor`; the tests run on ropes alone.

use super::nesting::{Css, Go, Html, JavaScript, Json, Nesting, TypeScript};
use super::pass::fallback;
use super::tests::Rules;
use super::{
    Answer, Budget, Line, Settings, css, fundamental, go, html, javascript, json, markdown, rust,
    toml, typescript, yaml,
};
use crate::rope::Rope;

/// Every mode's rules, once each, YAML's last.
pub(super) const ALL_RULES: [Rules; 11] = [
    rust,
    toml,
    markdown,
    fundamental,
    css,
    json,
    html,
    javascript,
    typescript,
    go,
    yaml,
];

/// The answer past the budget of the rules of [`ALL_RULES`], in the same order. YAML's
/// is Fundamental's: a new line copies the nearest non-blank line's indentation.
pub(super) type Fallback = fn(&Rope, Settings, Line, &mut Budget) -> Answer;
pub(super) const FALLBACKS: [Fallback; 11] = [
    fallback::<super::rust::Rust>,
    fallback::<super::toml::Toml>,
    fallback::<super::markdown::Markdown>,
    fallback::<super::fundamental::Fundamental>,
    fallback::<Nesting<Css>>,
    fallback::<Nesting<Json>>,
    fallback::<Nesting<Html>>,
    fallback::<Nesting<JavaScript>>,
    fallback::<Nesting<TypeScript>>,
    fallback::<Nesting<Go>>,
    fallback::<super::fundamental::Fundamental>,
];
