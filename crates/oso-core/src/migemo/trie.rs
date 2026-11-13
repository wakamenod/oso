//! A trie of strings written as a regexp's alternatives (ADR 0116), as C/Migemo builds its
//! pattern: common prefixes taken out, and every character through `regex::escape`, so
//! nothing in a string is regexp syntax.
//!
//! Must not depend on: any module of this crate.

/// A step of the trie: a character, or either of two (`ず` or `づ`), written as a class.
pub(super) type Key = (char, Option<char>);

/// A node: the strings that end here, and the next step of the others.
#[derive(Default)]
pub(super) struct Trie {
    end: bool,
    /// Sorted by key, so the pattern is the same for the same strings.
    next: Vec<(Key, Trie)>,
}

impl Trie {
    /// Adds `string`.
    pub(super) fn insert(&mut self, string: &str) {
        self.node(string.chars().map(|c| (c, None))).end();
    }

    /// Makes a string end here.
    pub(super) fn end(&mut self) {
        self.end = true;
    }

    /// The node `keys` lead to from here, made if it is not there yet.
    pub(super) fn node(&mut self, keys: impl IntoIterator<Item = Key>) -> &mut Self {
        let mut node = self;
        for key in keys {
            let at = match node.next.binary_search_by_key(&key, |(next, _)| *next) {
                Ok(at) => at,
                Err(at) => {
                    node.next.insert(at, (key, Self::default()));
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
        let mut node = self;
        // A run with one way on and no end is written plainly.
        while let [(key, only)] = node.next.as_slice()
            && !node.end
        {
            write_key(*key, out);
            node = only;
        }
        if node.next.is_empty() {
            return;
        }
        out.push_str("(?:");
        for (index, (key, child)) in node.next.iter().enumerate() {
            if index > 0 {
                out.push('|');
            }
            write_key(*key, out);
            child.write(out);
        }
        out.push(')');
        if node.end {
            out.push('?');
        }
    }
}

/// `key` onto `out`: its character, or a class of its two. An escaped character is one
/// inside a class too.
fn write_key((c, other): Key, out: &mut String) {
    let Some(other) = other else {
        escape(c, out);
        return;
    };
    out.push('[');
    escape(c, out);
    escape(other, out);
    out.push(']');
}

/// `c` onto `out` through `regex::escape`, which escapes only ASCII, so any other character
/// goes as it is, with nothing allocated.
fn escape(c: char, out: &mut String) {
    if c.is_ascii() {
        out.push_str(&regex::escape(c.encode_utf8(&mut [0; 4])));
    } else {
        out.push(c);
    }
}
