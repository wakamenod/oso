//! Tests of oso's queries: each compiles, names only classes, and uses only the predicates
//! tree-sitter's crate evaluates (ADR 0111).
//!
//! Must not depend on: the worker; these call the module directly.

use super::{Kind, Queries, source};
use crate::language::Language;

const ALL: [Kind; 5] = [
    Kind::Highlights(Language::Rust),
    Kind::Highlights(Language::Markdown),
    Kind::Highlights(Language::MarkdownInline),
    Kind::Highlights(Language::Toml),
    Kind::Inlines,
];

/// The predicates tree-sitter's crate evaluates itself (ADR 0111).
const ALLOWED: [&str; 3] = ["eq?", "match?", "any-of?"];

#[test]
fn every_query_compiles_once() {
    let mut queries = Queries::new();
    for kind in ALL {
        assert!(queries.get(kind).is_none(), "{kind:?} compiled before use");
        assert!(queries.prepare(kind), "{kind:?}");
        assert!(queries.get(kind).is_some(), "{kind:?}");
    }
}

#[test]
fn every_highlight_capture_names_a_class() {
    let mut queries = Queries::new();
    for kind in ALL.into_iter().filter(|kind| *kind != Kind::Inlines) {
        assert!(queries.prepare(kind), "{kind:?}");
        let prepared = queries.get(kind).unwrap();
        for (name, class) in prepared.query.capture_names().iter().zip(&prepared.classes) {
            assert!(class.is_some(), "{kind:?} captures @{name}");
        }
    }
}

#[test]
fn queries_use_only_the_predicates_tree_sitter_evaluates() {
    let mut queries = Queries::new();
    for kind in ALL {
        assert!(queries.prepare(kind), "{kind:?}");
        let query = &queries.get(kind).unwrap().query;
        for pattern in 0..query.pattern_count() {
            assert!(query.general_predicates(pattern).is_empty(), "{kind:?}");
            assert!(query.property_predicates(pattern).is_empty(), "{kind:?}");
            assert!(query.property_settings(pattern).is_empty(), "{kind:?}");
        }
        // The crate evaluates `not-` forms and `#set!` too: the text says what is used.
        let (_, text) = source(kind);
        for line in text
            .lines()
            .filter(|line| !line.trim_start().starts_with(';'))
        {
            for predicate in line.split('#').skip(1) {
                let name: String = predicate
                    .chars()
                    .take_while(|c| !c.is_whitespace() && *c != ')')
                    .collect();
                assert!(ALLOWED.contains(&name.as_str()), "{kind:?} uses #{name}");
            }
        }
    }
}

#[test]
fn the_predicate_check_sees_another_predicate() {
    let text = "((identifier) @type (#not-eq? @type \"x\"))";
    let query = tree_sitter::Query::new(&Language::Rust.grammar(), text).unwrap();
    let name = text.split('#').nth(1).unwrap().split(' ').next().unwrap();
    assert!(!ALLOWED.contains(&name));
    // `#not-eq?` is one the crate evaluates, so only the text check catches it.
    assert!(query.general_predicates(0).is_empty());
}
