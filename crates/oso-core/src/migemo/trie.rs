//! A trie of strings written as a regexp's alternatives (ADR 0116), as C/Migemo builds its
//! pattern: common prefixes taken out, and every character through `regex::escape`, so
//! nothing in a string is regexp syntax.
//!
//! Must not depend on: any module of this crate.

/// A node: the strings that end here, and the next character of the others.
#[derive(Default)]
pub(super) struct Trie {
    end: bool,
    /// Sorted by character, so the pattern is the same for the same strings.
    next: Vec<(char, Trie)>,
}

impl Trie {
    /// Adds `string`.
    pub(super) fn insert(&mut self, string: &str) {
        self.node(string).end();
    }

    /// Makes a string end here.
    pub(super) fn end(&mut self) {
        self.end = true;
    }

    /// The node `string` leads to from here, made if it is not there yet.
    pub(super) fn node(&mut self, string: &str) -> &mut Self {
        let mut node = self;
        for c in string.chars() {
            let at = match node.next.binary_search_by_key(&c, |(key, _)| *key) {
                Ok(at) => at,
                Err(at) => {
                    node.next.insert(at, (c, Self::default()));
                    at
                }
            };
            node = &mut node.next[at].1;
        }
        node
    }

    /// The alternatives as a pattern onto `out`: `か(?:き|く)?` for `か`, `かき` and `かく`.
    /// A string that ends where others go on makes the rest optional, which the regexp then
    /// takes as far as it can.
    pub(super) fn write(&self, out: &mut String) {
        let mut one = [0; 4];
        let mut node = self;
        // A run with one way on and no end is written plainly.
        while let [(c, only)] = node.next.as_slice()
            && !node.end
        {
            out.push_str(&regex::escape(c.encode_utf8(&mut one)));
            node = only;
        }
        if node.next.is_empty() {
            return;
        }
        out.push_str("(?:");
        for (index, (c, child)) in node.next.iter().enumerate() {
            if index > 0 {
                out.push('|');
            }
            out.push_str(&regex::escape(c.encode_utf8(&mut one)));
            child.write(out);
        }
        out.push(')');
        if node.end {
            out.push('?');
        }
    }
}
