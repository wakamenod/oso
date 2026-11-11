//! Each entry of `[keys.global]` against the default keys it shadows, read as the resolver reads
//! keys: a key bound to nothing is taken without Shift, so `C-A` runs `C-a`'s command and `C-X`
//! begins `C-x`'s keys (D1 in ADR 0110).
//!
//! Must not depend on: `app` or `dispatch`; it reads the default keymap and registry it is given.

use std::collections::HashSet;

use oso_core::command::{CommandId, Registry};
use oso_core::key::KeyPress;
use oso_core::keymap::{Binding, KeyResolver, Keymap, Layers, Resolution, display_keys};

use super::keys::Entry;
use super::problems::{Found, quoted};

/// What pressing an entry's keys does with the default keys alone.
#[derive(Default)]
struct Walk<'d> {
    /// The first `len` keys, fewer than the entry has, run this command.
    command: Option<(usize, CommandId)>,
    /// Shorter parts of the entry that begin a default prefix only without Shift: how many
    /// keys, the keys without Shift, and the prefix's keymap. The entry makes each a prefix of
    /// the user's own, which no longer reaches the defaults under it.
    shifted: Vec<(usize, Vec<KeyPress>, &'d Keymap)>,
    /// The default prefix the whole entry begins, and its keys if they are not the entry's own.
    prefix: Option<(&'d Keymap, Option<Vec<KeyPress>>)>,
}

/// The entries' checks against the defaults, with what they have warned of.
pub(super) struct Shadows<'a> {
    registry: &'a Registry,
    defaults: &'a Keymap,
    /// Every sequence the file unbinds.
    unbound: &'a HashSet<&'a [KeyPress]>,
    /// The prefixes already warned of, so that each is warned of once.
    warned: HashSet<Vec<KeyPress>>,
}

impl<'a> Shadows<'a> {
    pub(super) fn new(
        registry: &'a Registry,
        defaults: &'a Keymap,
        unbound: &'a HashSet<&'a [KeyPress]>,
    ) -> Self {
        Self {
            registry,
            defaults,
            unbound,
            warned: HashSet::new(),
        }
    }

    /// Whether the file unbinds the first `len` keys of `keys`, or fewer of them.
    fn hidden(&self, keys: &[KeyPress], len: usize) -> bool {
        (1..=len).any(|len| self.unbound.contains(&keys[..len]))
    }

    /// Whether `entry` may apply: not if it would make a prefix of keys that run a default
    /// command, unless the file unbinds them. Binding and unbinding alike: `"C-a x" = false`
    /// would make `C-a` a prefix too.
    pub(super) fn allows(&self, entry: &Entry<'_>, found: &mut Found) -> bool {
        let keys = &entry.keys[..];
        let Some((len, default)) = walk(self.defaults, keys).command else {
            return true;
        };
        if self.hidden(keys, len) {
            return true;
        }
        let prefix = quoted(display_keys(&keys[..len]));
        let name = self
            .registry
            .get(default)
            .map_or("a command", |command| command.name);
        found.about(
            entry.at,
            entry.spelled,
            format_args!(
                "{prefix} runs {name} by default; add {prefix} = false to make it a prefix"
            ),
        );
        false
    }

    /// Warns of the default keys `entry` hides, once it applies: those under a default prefix it
    /// `binds` a command to, and those under a prefix that its keys begin only without Shift.
    pub(super) fn warn(&mut self, entry: &Entry<'_>, binds: bool, found: &mut Found) {
        let keys = &entry.keys[..];
        let walk = walk(self.defaults, keys);
        for (len, without, under) in &walk.shifted {
            if !self.hidden(keys, *len) && self.warned.insert(keys[..*len].to_vec()) {
                found.about(
                    entry.at,
                    entry.spelled,
                    format_args!(
                        "{} begins the keys under {} by default, without Shift; this makes it a \
                         prefix of its own and hides those {}",
                        quoted(display_keys(&keys[..*len])),
                        quoted(display_keys(without)),
                        DefaultKeys(under),
                    ),
                );
            }
        }
        let Some((under, without)) = walk.prefix.filter(|_| binds) else {
            return;
        };
        if self.hidden(keys, keys.len()) {
            return;
        }
        match without {
            None => found.about(
                entry.at,
                entry.spelled,
                format_args!(
                    "a prefix by default; this hides the {} under it",
                    DefaultKeys(under)
                ),
            ),
            Some(without) => found.about(
                entry.at,
                entry.spelled,
                format_args!(
                    "begins the keys under {} by default, without Shift; this hides those {}",
                    quoted(display_keys(&without)),
                    DefaultKeys(under),
                ),
            ),
        }
    }
}

/// What `keys` do with the defaults alone, pressed one at a time through a resolver, so that
/// Shift is dropped where the resolver drops it.
fn walk<'d>(defaults: &'d Keymap, keys: &[KeyPress]) -> Walk<'d> {
    let layers = Layers::global(defaults);
    let mut resolver = KeyResolver::new();
    let mut walk = Walk::default();
    for (i, key) in keys.iter().enumerate() {
        let len = i + 1;
        let command = match resolver.feed(&layers, *key) {
            Resolution::Command(id) | Resolution::Shifted(id) => Some(id),
            Resolution::Pending => None,
            Resolution::Unbound(_) | Resolution::Cancelled => break,
        };
        if let Some(command) = command {
            if len < keys.len() {
                walk.command = Some((len, command));
            }
            break;
        }
        // A prefix found without Shift stays among the pending keys unshifted.
        let reached = resolver.pending();
        let Some(Binding::Prefix(under)) = defaults.lookup(reached) else {
            break;
        };
        let without = (reached != &keys[..len]).then(|| reached.to_vec());
        if len == keys.len() {
            walk.prefix = Some((under, without));
        } else if let Some(without) = without {
            walk.shifted.push((len, without, under));
        }
    }
    walk
}

/// "the N default keys", for the commands bound under a default prefix, at every level.
struct DefaultKeys<'d>(&'d Keymap);

impl std::fmt::Display for DefaultKeys<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut count = 0;
        self.0.bindings(|_, _| count += 1);
        let plural = if count == 1 { "" } else { "s" };
        write!(f, "{count} default key{plural}")
    }
}
