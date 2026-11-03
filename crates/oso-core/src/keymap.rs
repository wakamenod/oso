//! Keymaps with prefix keys, and the resolver that turns key presses into
//! commands (ADR 0034).
//!
//! - A [`Keymap`] maps a [`KeyPress`] to a command or to a nested prefix
//!   keymap: `C-x` is a prefix, `C-x C-c` a command under it.
//! - [`Layers`] are the keymaps active at a moment, highest priority first:
//!   keymap at point, minor modes, major mode, global (architecture.md).
//! - A [`KeyResolver`] keeps the keys of a pending prefix between presses.
//!   `C-x` waits, `C-x C-c` resolves to a command, an unbound sequence is
//!   reported and clears the prefix, and `C-g` cancels a pending prefix.
//!
//! Bindings refer to commands by [`CommandId`], resolved from names once,
//! when the keymap is built, so a key press costs a few hash lookups and no
//! allocation.

use std::collections::HashMap;
use std::fmt;

use crate::command::{CommandId, Registry};
use crate::key::{Key, KeyPress, KeySequence, Modifiers};

/// What a key is bound to in a keymap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// Run this command.
    Command(CommandId),
    /// Wait for another key, and look it up in this keymap.
    Prefix(Keymap),
}

/// Key bindings, possibly nested under prefix keys.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Keymap {
    bindings: HashMap<KeyPress, Binding>,
}

impl Keymap {
    /// An empty keymap.
    pub fn new() -> Self {
        Self::default()
    }

    /// A keymap from a table of key sequences in Emacs notation and command
    /// names, such as `("C-x C-c", "save-buffers-kill-terminal")`.
    pub fn from_table(registry: &Registry, table: &[(&str, &str)]) -> Result<Self, TableError> {
        let mut keymap = Self::new();
        for &(keys, command) in table {
            let error = |reason| TableError {
                keys: keys.to_owned(),
                command: command.to_owned(),
                reason,
            };
            let sequence: KeySequence = keys.parse().map_err(|e: crate::key::ParseKeyError| {
                error(TableErrorReason::Notation(e.to_string()))
            })?;
            let id = registry
                .lookup(command)
                .ok_or_else(|| error(TableErrorReason::UnknownCommand))?;
            keymap
                .bind(&sequence.0, id)
                .map_err(|e| error(TableErrorReason::Bind(e)))?;
        }
        Ok(keymap)
    }

    /// Binds `keys` to `command`, creating prefix keymaps as needed.
    ///
    /// Fails, and leaves the keymap unchanged, if `keys` is empty, already
    /// bound, a prefix of other bindings, or starts with a key bound to a
    /// command.
    pub fn bind(&mut self, keys: &[KeyPress], command: CommandId) -> Result<(), BindError> {
        let (last, prefix) = keys.split_last().ok_or(BindError::Empty)?;
        // Prefix keymaps are created only where none existed, so nothing
        // deeper can conflict, and an error never leaves one behind.
        let mut map = self;
        for (i, key) in prefix.iter().enumerate() {
            let entry = map
                .bindings
                .entry(*key)
                .or_insert_with(|| Binding::Prefix(Keymap::new()));
            match entry {
                Binding::Prefix(sub) => map = sub,
                Binding::Command(_) => return Err(BindError::NonPrefix { len: i + 1 }),
            }
        }
        match map.bindings.get(last) {
            None => {
                map.bindings.insert(*last, Binding::Command(command));
                Ok(())
            }
            Some(Binding::Command(_)) => Err(BindError::AlreadyBound),
            Some(Binding::Prefix(_)) => Err(BindError::IsPrefix),
        }
    }

    /// What `key` is bound to in this keymap, without descending.
    pub fn get(&self, key: KeyPress) -> Option<&Binding> {
        self.bindings.get(&key)
    }

    /// What the whole sequence `keys` is bound to, descending through
    /// prefix keymaps. `None` if it is unbound here, including when a
    /// shorter part of it is bound to a command.
    pub fn lookup(&self, keys: &[KeyPress]) -> Option<&Binding> {
        let (first, rest) = keys.split_first()?;
        let mut binding = self.get(*first)?;
        for key in rest {
            match binding {
                Binding::Prefix(sub) => binding = sub.get(*key)?,
                Binding::Command(_) => return None,
            }
        }
        Some(binding)
    }

    /// The number of keys bound at this level, prefixes included.
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Whether nothing is bound.
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

/// Why [`Keymap::bind`] refused a binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    /// The key sequence is empty.
    Empty,
    /// The sequence is already bound to a command.
    AlreadyBound,
    /// The sequence is a prefix key with bindings under it.
    IsPrefix,
    /// The first `len` keys are bound to a command, so they cannot be a
    /// prefix.
    NonPrefix { len: usize },
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the key sequence is empty"),
            Self::AlreadyBound => f.write_str("the key sequence is already bound"),
            Self::IsPrefix => f.write_str("the key sequence is a prefix key"),
            Self::NonPrefix { len } => {
                write!(
                    f,
                    "its first {len} key(s) are bound to a command, not a prefix"
                )
            }
        }
    }
}

impl std::error::Error for BindError {}

/// A row of a binding table that cannot be bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableError {
    pub keys: String,
    pub command: String,
    pub reason: TableErrorReason,
}

/// Why a row of a binding table cannot be bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableErrorReason {
    /// The keys are not valid Emacs notation.
    Notation(String),
    /// No command has this name.
    UnknownCommand,
    /// The binding conflicts with another one.
    Bind(BindError),
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            keys,
            command,
            reason,
        } = self;
        write!(f, "cannot bind {keys:?} to {command:?}: ")?;
        match reason {
            TableErrorReason::Notation(e) => f.write_str(e),
            TableErrorReason::UnknownCommand => f.write_str("no such command"),
            TableErrorReason::Bind(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for TableError {}

/// The keymaps active at one moment.
///
/// Lookup tries them from the highest priority down: keymap at point, minor
/// modes, major mode, global (architecture.md). The first keymap that binds
/// the whole sequence, to a command or a prefix, decides; keymaps that do
/// not bind it are skipped. Only the global layer exists so far.
#[derive(Debug, Clone, Copy)]
pub struct Layers<'a> {
    /// The keymap of the text or overlay at point.
    pub at_point: Option<&'a Keymap>,
    /// Keymaps of the active minor modes, highest priority first.
    pub minor: &'a [&'a Keymap],
    /// The keymap of the buffer's major mode.
    pub major: Option<&'a Keymap>,
    /// The global keymap.
    pub global: &'a Keymap,
}

impl<'a> Layers<'a> {
    /// Only the global keymap.
    pub fn global(global: &'a Keymap) -> Self {
        Self {
            at_point: None,
            minor: &[],
            major: None,
            global,
        }
    }

    /// The keymaps, highest priority first.
    pub fn iter(&self) -> impl Iterator<Item = &'a Keymap> {
        self.at_point
            .into_iter()
            .chain(self.minor.iter().copied())
            .chain(self.major)
            .chain(std::iter::once(self.global))
    }

    /// What `keys` is bound to in the first keymap that binds it.
    pub fn lookup(&self, keys: &[KeyPress]) -> Option<&'a Binding> {
        self.iter().find_map(|keymap| keymap.lookup(keys))
    }
}

/// The result of feeding one key to a [`KeyResolver`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution<'a> {
    /// The keys so far form a command: run it. The prefix is cleared.
    Command(CommandId),
    /// The keys so far are a prefix: wait for the next key.
    Pending,
    /// The keys so far are bound to nothing. The prefix is cleared; the
    /// keys are given for the report ("C-x C-z is undefined").
    Unbound(&'a [KeyPress]),
    /// The cancel key (`C-g`) was pressed while a prefix was pending. The
    /// prefix is cleared.
    Cancelled,
}

/// `C-g`, which cancels a pending prefix.
pub const CANCEL: KeyPress = KeyPress {
    modifiers: Modifiers::CONTROL,
    key: Key::Char('g'),
};

/// Turns key presses into commands, keeping the keys of a pending prefix.
///
/// Each key is looked up as part of the whole pending sequence from the top
/// of every layer, so the resolver holds no references into keymaps, and
/// the layers may change between keys.
#[derive(Debug, Default)]
pub struct KeyResolver {
    /// The pending prefix, or the unbound sequence last reported.
    keys: Vec<KeyPress>,
    /// Whether `keys` is the unbound sequence last reported, to be cleared
    /// on the next key. Keeping it until then lets [`Resolution::Unbound`]
    /// borrow it.
    reported: bool,
}

impl KeyResolver {
    /// A resolver with nothing pending.
    pub fn new() -> Self {
        Self::default()
    }

    /// The keys of the pending prefix, empty if none is pending.
    pub fn pending(&self) -> &[KeyPress] {
        if self.reported { &[] } else { &self.keys }
    }

    /// Drops the pending prefix.
    pub fn reset(&mut self) {
        self.keys.clear();
        self.reported = false;
    }

    /// Feeds one key press, looked up in `layers`.
    ///
    /// `C-g` with a prefix pending always cancels it, whatever the keymaps
    /// bind, as in Emacs. With nothing pending it is an ordinary key.
    pub fn feed(&mut self, layers: &Layers<'_>, key: KeyPress) -> Resolution<'_> {
        if self.reported {
            self.reset();
        }
        if key == CANCEL && !self.keys.is_empty() {
            self.reset();
            return Resolution::Cancelled;
        }
        // Reuses the buffer: no allocation once it has grown to the longest
        // prefix.
        self.keys.push(key);
        match layers.lookup(&self.keys) {
            Some(Binding::Command(id)) => {
                self.keys.clear();
                Resolution::Command(*id)
            }
            Some(Binding::Prefix(_)) => Resolution::Pending,
            None => {
                self.reported = true;
                Resolution::Unbound(&self.keys)
            }
        }
    }
}

/// Displays keys in Emacs notation, separated by spaces, without
/// allocating a [`KeySequence`].
pub fn display_keys(keys: &[KeyPress]) -> impl fmt::Display + '_ {
    struct Keys<'a>(&'a [KeyPress]);
    impl fmt::Display for Keys<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for (i, key) in self.0.iter().enumerate() {
                if i > 0 {
                    f.write_str(" ")?;
                }
                key.fmt(f)?;
            }
            Ok(())
        }
    }
    Keys(keys)
}

#[cfg(test)]
mod tests {
    use super::{
        BindError, Binding, CANCEL, KeyResolver, Keymap, Layers, Resolution, TableErrorReason,
        display_keys,
    };
    use crate::command::{ArgSpec, Args, Command, CommandError, CommandId, Ctx, Registry};
    use crate::key::{KeyPress, KeySequence};
    use proptest::prelude::*;
    use std::collections::HashMap;

    fn keys(s: &str) -> Vec<KeyPress> {
        s.parse::<KeySequence>().unwrap_or_else(|e| panic!("{e}")).0
    }

    fn noop(_: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
        Ok(())
    }

    /// A registry with commands named after the given names.
    fn registry(names: &[&'static str]) -> Registry {
        let mut registry = Registry::new();
        for name in names {
            registry
                .register(Command {
                    name,
                    doc: "A test command.",
                    args: ArgSpec::None,
                    run: noop,
                })
                .unwrap();
        }
        registry
    }

    const NAMES: &[&str] = &[
        "forward-char",
        "backward-char",
        "keyboard-quit",
        "quit",
        "find-file",
        "other",
    ];

    fn global(registry: &Registry) -> Keymap {
        Keymap::from_table(
            registry,
            &[
                ("C-f", "forward-char"),
                ("<right>", "forward-char"),
                ("C-b", "backward-char"),
                ("C-g", "keyboard-quit"),
                ("C-x C-c", "quit"),
                ("C-x C-f", "find-file"),
                ("C-x 4 f", "find-file"),
            ],
        )
        .unwrap()
    }

    fn id(registry: &Registry, name: &str) -> CommandId {
        registry.lookup(name).unwrap()
    }

    /// Feeds each key of `s` and returns what the resolver said, with
    /// commands as names and unbound sequences in notation.
    fn feed_all(
        resolver: &mut KeyResolver,
        layers: &Layers<'_>,
        registry: &Registry,
        s: &str,
    ) -> Vec<String> {
        keys(s)
            .into_iter()
            .map(|key| match resolver.feed(layers, key) {
                Resolution::Command(id) => registry.get(id).unwrap().name.to_owned(),
                Resolution::Pending => "pending".to_owned(),
                Resolution::Unbound(keys) => format!("unbound {}", display_keys(keys)),
                Resolution::Cancelled => "cancelled".to_owned(),
            })
            .collect()
    }

    // --- Keymaps ---

    #[test]
    fn looks_up_single_keys_and_sequences() {
        let registry = registry(NAMES);
        let map = global(&registry);
        assert_eq!(
            map.lookup(&keys("C-f")),
            Some(&Binding::Command(id(&registry, "forward-char")))
        );
        assert_eq!(
            map.lookup(&keys("C-x C-c")),
            Some(&Binding::Command(id(&registry, "quit")))
        );
        assert!(matches!(map.lookup(&keys("C-x")), Some(Binding::Prefix(_))));
        assert!(matches!(
            map.lookup(&keys("C-x 4")),
            Some(Binding::Prefix(_))
        ));
        assert_eq!(map.lookup(&keys("C-x C-z")), None);
        assert_eq!(map.lookup(&keys("C-f C-f")), None, "too long");
        assert_eq!(map.lookup(&[]), None);
    }

    #[test]
    fn refuses_conflicting_bindings() {
        let registry = registry(NAMES);
        let mut map = global(&registry);
        let other = id(&registry, "other");
        let before = map.clone();
        assert_eq!(map.bind(&[], other), Err(BindError::Empty));
        assert_eq!(map.bind(&keys("C-f"), other), Err(BindError::AlreadyBound));
        assert_eq!(map.bind(&keys("C-x"), other), Err(BindError::IsPrefix));
        assert_eq!(map.bind(&keys("C-x 4"), other), Err(BindError::IsPrefix));
        assert_eq!(
            map.bind(&keys("C-f a"), other),
            Err(BindError::NonPrefix { len: 1 })
        );
        assert_eq!(
            map.bind(&keys("C-x C-c a b"), other),
            Err(BindError::NonPrefix { len: 2 })
        );
        assert_eq!(map, before, "a refused binding changes nothing");
        assert_eq!(map.bind(&keys("C-x 4 b"), other), Ok(()));
    }

    #[test]
    fn tables_report_bad_rows() {
        let registry = registry(NAMES);
        let err = Keymap::from_table(&registry, &[("C-x C-q", "no-such-command")]).unwrap_err();
        assert_eq!(err.reason, TableErrorReason::UnknownCommand);
        assert_eq!(
            err.to_string(),
            "cannot bind \"C-x C-q\" to \"no-such-command\": no such command"
        );
        let err = Keymap::from_table(&registry, &[("C-foo", "other")]).unwrap_err();
        assert!(matches!(err.reason, TableErrorReason::Notation(_)), "{err}");
        let err = Keymap::from_table(&registry, &[("C-f", "other"), ("C-f", "quit")]).unwrap_err();
        assert_eq!(err.reason, TableErrorReason::Bind(BindError::AlreadyBound));
        let err = Keymap::from_table(&registry, &[("", "other")]).unwrap_err();
        assert_eq!(err.reason, TableErrorReason::Bind(BindError::Empty));
    }

    // --- Resolver ---

    #[test]
    fn single_keys_resolve_at_once() {
        let registry = registry(NAMES);
        let map = global(&registry);
        let layers = Layers::global(&map);
        let mut resolver = KeyResolver::new();
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "C-f <right> C-b"),
            ["forward-char", "forward-char", "backward-char"]
        );
        assert!(resolver.pending().is_empty());
    }

    #[test]
    fn a_prefix_waits_for_the_next_key() {
        let registry = registry(NAMES);
        let map = global(&registry);
        let layers = Layers::global(&map);
        let mut resolver = KeyResolver::new();
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "C-x"),
            ["pending"]
        );
        assert_eq!(resolver.pending(), keys("C-x"));
        assert_eq!(feed_all(&mut resolver, &layers, &registry, "C-c"), ["quit"]);
        assert!(resolver.pending().is_empty());
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "C-x 4 f"),
            ["pending", "pending", "find-file"]
        );
    }

    #[test]
    fn an_unbound_sequence_is_reported_and_clears_the_prefix() {
        let registry = registry(NAMES);
        let map = global(&registry);
        let layers = Layers::global(&map);
        let mut resolver = KeyResolver::new();
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "C-x C-z C-f"),
            ["pending", "unbound C-x C-z", "forward-char"]
        );
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "a"),
            ["unbound a"]
        );
        assert!(
            resolver.pending().is_empty(),
            "nothing pending after a report"
        );
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "C-x 4 z C-x C-c"),
            ["pending", "pending", "unbound C-x 4 z", "pending", "quit"]
        );
    }

    #[test]
    fn cancel_clears_a_pending_prefix() {
        let registry = registry(NAMES);
        let map = global(&registry);
        let layers = Layers::global(&map);
        let mut resolver = KeyResolver::new();
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "C-x C-g C-f"),
            ["pending", "cancelled", "forward-char"]
        );
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "C-x 4 C-g"),
            ["pending", "pending", "cancelled"]
        );
        // With nothing pending, C-g is an ordinary key.
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "C-g"),
            ["keyboard-quit"]
        );
    }

    #[test]
    fn cancel_wins_over_a_binding_under_the_prefix() {
        let registry = registry(NAMES);
        let map = Keymap::from_table(&registry, &[("C-x C-g", "other")]).unwrap();
        let layers = Layers::global(&map);
        let mut resolver = KeyResolver::new();
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "C-x C-g"),
            ["pending", "cancelled"]
        );
        // Unbound with nothing pending: reported like any other key.
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "C-g"),
            ["unbound C-g"]
        );
    }

    #[test]
    fn reset_drops_the_prefix() {
        let registry = registry(NAMES);
        let map = global(&registry);
        let layers = Layers::global(&map);
        let mut resolver = KeyResolver::new();
        feed_all(&mut resolver, &layers, &registry, "C-x");
        resolver.reset();
        assert!(resolver.pending().is_empty());
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "C-c"),
            ["unbound C-c"]
        );
    }

    #[test]
    fn higher_layers_win() {
        let registry = registry(NAMES);
        let global = global(&registry);
        // The major mode rebinds C-f, makes C-x C-c a prefix, and adds a
        // binding under the global C-x prefix.
        let major = Keymap::from_table(
            &registry,
            &[("C-f", "other"), ("C-x C-c a", "other"), ("C-x m", "other")],
        )
        .unwrap();
        // A minor mode turns the global prefix C-x into a command.
        let minor = Keymap::from_table(&registry, &[("C-x", "keyboard-quit")]).unwrap();
        let at_point = Keymap::from_table(&registry, &[("C-b", "find-file")]).unwrap();

        let layers = Layers {
            at_point: None,
            minor: &[],
            major: Some(&major),
            global: &global,
        };
        let mut resolver = KeyResolver::new();
        assert_eq!(
            feed_all(
                &mut resolver,
                &layers,
                &registry,
                "C-f C-b C-x m C-x C-f C-x C-c a"
            ),
            [
                "other",
                "backward-char",
                "pending",
                "other",
                "pending",
                "find-file",
                "pending",
                "pending",
                "other"
            ]
        );

        let minors = [&minor];
        let layers = Layers {
            at_point: Some(&at_point),
            minor: &minors,
            ..layers
        };
        assert_eq!(
            feed_all(&mut resolver, &layers, &registry, "C-b C-x C-f"),
            ["find-file", "keyboard-quit", "other"]
        );
        assert_eq!(layers.iter().count(), 4);
    }

    #[test]
    fn cancel_is_c_g() {
        assert_eq!(CANCEL, "C-g".parse().unwrap());
    }

    #[test]
    fn displays_keys_in_notation() {
        assert_eq!(display_keys(&keys("C-x  C-c")).to_string(), "C-x C-c");
        assert_eq!(display_keys(&[]).to_string(), "");
    }

    // --- Property-based tests (proptest) ---

    /// A small alphabet, so that random sequences collide and nest often.
    /// `C-g` is in it to exercise cancelling.
    const ALPHABET: [&str; 5] = ["a", "C-x", "C-c", "<right>", "C-g"];

    fn key() -> impl Strategy<Value = KeyPress> {
        prop::sample::select(ALPHABET.to_vec()).prop_map(|s| s.parse().unwrap())
    }

    fn table() -> impl Strategy<Value = Vec<Vec<KeyPress>>> {
        prop::collection::vec(prop::collection::vec(key(), 0..4), 0..12)
    }

    /// A reference model: a keymap as a flat map from sequences to ids.
    type Model = HashMap<Vec<KeyPress>, CommandId>;

    /// What the model says about `keys` in one keymap.
    fn model_lookup(model: &Model, keys: &[KeyPress]) -> Option<Result<CommandId, ()>> {
        if let Some(id) = model.get(keys) {
            return Some(Ok(*id));
        }
        model
            .keys()
            .any(|bound| bound.len() > keys.len() && bound.starts_with(keys))
            .then_some(Err(()))
    }

    /// Builds a keymap and its model from `table`, binding row `i` to
    /// command `i`. The model accepts a row exactly when it neither extends
    /// nor is extended by (or equals) a bound sequence.
    fn build(
        registry: &Registry,
        table: &[Vec<KeyPress>],
    ) -> Result<(Keymap, Model), TestCaseError> {
        let ids: Vec<CommandId> = registry.iter().map(|(id, _)| id).collect();
        let mut keymap = Keymap::new();
        let mut model = Model::new();
        for (i, keys) in table.iter().enumerate() {
            let id = ids[i % ids.len()];
            let conflicts = keys.is_empty()
                || model
                    .keys()
                    .any(|bound| bound.starts_with(keys) || keys.starts_with(bound));
            let bound = keymap.bind(keys, id);
            prop_assert_eq!(bound.is_ok(), !conflicts, "binding {:?}: {:?}", keys, bound);
            if !conflicts {
                model.insert(keys.clone(), id);
            }
        }
        Ok((keymap, model))
    }

    /// The model resolver: the same contract as [`KeyResolver::feed`] over
    /// models, owned for comparison.
    #[derive(Debug, PartialEq, Eq)]
    enum Expected {
        Command(CommandId),
        Pending,
        Unbound(Vec<KeyPress>),
        Cancelled,
    }

    fn model_feed(models: &[Model], pending: &mut Vec<KeyPress>, key: KeyPress) -> Expected {
        if key == CANCEL && !pending.is_empty() {
            pending.clear();
            return Expected::Cancelled;
        }
        pending.push(key);
        match models.iter().find_map(|m| model_lookup(m, pending)) {
            Some(Ok(id)) => {
                pending.clear();
                Expected::Command(id)
            }
            Some(Err(())) => Expected::Pending,
            None => Expected::Unbound(std::mem::take(pending)),
        }
    }

    fn observed(resolution: Resolution<'_>) -> Expected {
        match resolution {
            Resolution::Command(id) => Expected::Command(id),
            Resolution::Pending => Expected::Pending,
            Resolution::Unbound(keys) => Expected::Unbound(keys.to_vec()),
            Resolution::Cancelled => Expected::Cancelled,
        }
    }

    proptest! {
        /// Keymaps accept exactly the bindings that do not conflict, and
        /// look sequences up as the flat model does.
        #[test]
        fn prop_keymap_matches_model(table in table(), probes in prop::collection::vec(prop::collection::vec(key(), 1..5), 0..20)) {
            let registry = registry(NAMES);
            let (keymap, model) = build(&registry, &table)?;
            for probe in &probes {
                let actual = keymap.lookup(probe).map(|b| match b {
                    Binding::Command(id) => Ok(*id),
                    Binding::Prefix(_) => Err(()),
                });
                prop_assert_eq!(actual, model_lookup(&model, probe), "probe {:?}", probe);
            }
        }

        /// The resolver over up to three layers agrees with the model on
        /// every key: commands, prefixes, unbound sequences, cancelling and
        /// layer precedence.
        #[test]
        fn prop_resolver_matches_model(
            tables in prop::collection::vec(table(), 1..4),
            input in prop::collection::vec(key(), 0..40),
        ) {
            let registry = registry(NAMES);
            let mut keymaps = Vec::new();
            let mut models = Vec::new();
            for table in &tables {
                let (keymap, model) = build(&registry, table)?;
                keymaps.push(keymap);
                models.push(model);
            }
            let (global, higher) = keymaps.split_last().unwrap();
            let minor: Vec<&Keymap> = higher.iter().collect();
            let layers = Layers { at_point: None, minor: &minor, major: None, global };

            let longest = models.iter().flat_map(|m| m.keys()).map(Vec::len).max().unwrap_or(0);
            let mut resolver = KeyResolver::new();
            let mut pending = Vec::new();
            for key in input {
                let expected = model_feed(&models, &mut pending, key);
                prop_assert_eq!(observed(resolver.feed(&layers, key)), expected);
                prop_assert_eq!(resolver.pending(), &pending[..]);
                // The prefix never outgrows the deepest binding.
                prop_assert!(resolver.pending().len() < longest.max(1));
            }
        }

        /// Whatever prefix is pending, C-g cancels it and leaves nothing
        /// pending.
        #[test]
        fn prop_cancel_always_clears(table in table(), input in prop::collection::vec(key(), 0..10)) {
            let registry = registry(NAMES);
            let (keymap, _) = build(&registry, &table)?;
            let layers = Layers::global(&keymap);
            let mut resolver = KeyResolver::new();
            for key in input {
                let _ = resolver.feed(&layers, key);
            }
            if !resolver.pending().is_empty() {
                prop_assert_eq!(resolver.feed(&layers, CANCEL), Resolution::Cancelled);
                prop_assert!(resolver.pending().is_empty());
            }
        }
    }
}
