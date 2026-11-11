//! The user's keymap from the file's `[keys]` table: each entry under `[keys.global]` binds a key
//! sequence to a command or leaves it undefined, checked against the default keys it would
//! shadow (ADR 0108, ADR 0110).
//!
//! Must not depend on: `app` or `dispatch`; it builds a [`Keymap`] for the dispatcher to take,
//! from the registry and default keymap it is given.

use std::collections::HashSet;

use oso_core::command::{CommandId, Registry};
use oso_core::key::KeyPress;
use oso_core::keymap::{
    BindError, Binding, KeyResolver, Keymap, Layers, Resolution, display_keys, is_cancel,
};
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::problems::{Found, quoted};
use super::{TableName, table, type_name};

/// The most entries `[keys.global]` may have. A file with more binds none of them, so that a
/// file of any size builds in bounded time (ADR 0110).
pub const MAX_ENTRIES: usize = 10_000;

/// The most keys in one sequence. Emacs's own are at most four long; the limit keeps prefix
/// keymaps from nesting deeper than a few levels.
pub const MAX_KEYS: usize = 16;

/// One entry of `[keys.global]` whose keys parse and may be configured.
struct Entry<'t> {
    /// The keys as the file spells them.
    spelled: &'t str,
    /// Where the keys are in the file.
    at: usize,
    keys: Vec<KeyPress>,
}

/// The user's keymap from the `[keys]` table `value`, whose name is at `at`. Entries with a
/// problem are skipped, with a message in `found`.
pub(super) fn keys(
    value: &Spanned<DeValue<'_>>,
    registry: &Registry,
    defaults: &Keymap,
    found: &mut Found,
) -> Keymap {
    let mut keymap = Keymap::new();
    let Some(keymaps) = table(value, "[keys]", found) else {
        return keymap;
    };
    for (name, value) in keymaps {
        let at = name.span().start;
        match name.get_ref().as_ref() {
            "global" => keymap = global(value, at, registry, defaults, found),
            "question" => found.at(
                at,
                format_args!(
                    "[keys.question] is never configurable: its keys answer the agent channel's \
                 questions"
                ),
            ),
            other => found.at(
                at,
                format_args!(
                    "[keys.{}] is not configurable yet; only [keys.global] is",
                    TableName(other)
                ),
            ),
        }
    }
    keymap
}

/// The user's keymap from `[keys.global]`, whose name is at `at`. Unbinding comes first, so
/// that the order of the entries, which TOML does not keep, changes nothing.
fn global(
    value: &Spanned<DeValue<'_>>,
    at: usize,
    registry: &Registry,
    defaults: &Keymap,
    found: &mut Found,
) -> Keymap {
    let mut keymap = Keymap::new();
    let Some(entries) = table(value, "[keys.global]", found) else {
        return keymap;
    };
    if entries.len() > MAX_ENTRIES {
        found.at(
            at,
            format_args!(
                "[keys.global] has {} entries, more than {MAX_ENTRIES}; none of its keys apply",
                entries.len()
            ),
        );
        return keymap;
    }
    let (mut unbinds, mut binds) = read(entries, registry, found);
    // Shorter sequences first, so that of two entries that conflict the longer one is skipped,
    // whatever the file's order.
    unbinds.sort_unstable_by(|a, b| (a.keys.len(), a.spelled).cmp(&(b.keys.len(), b.spelled)));
    binds.sort_unstable_by(|(a, _), (b, _)| {
        (a.keys.len(), a.spelled).cmp(&(b.keys.len(), b.spelled))
    });
    let unbound: HashSet<&[KeyPress]> = unbinds.iter().map(|entry| &entry.keys[..]).collect();
    for entry in &unbinds {
        // Nothing is bound to a command yet, so only an empty sequence could fail, and
        // `read` keeps none.
        let _ = keymap.unbind(&entry.keys);
    }
    for (entry, command) in &binds {
        bind(
            &mut keymap,
            entry,
            *command,
            registry,
            defaults,
            &unbound,
            found,
        );
    }
    for entry in &unbinds {
        warn_if_shift_finds_more(&keymap, entry, defaults, found);
    }
    keymap
}

/// The entries that unbind and those that bind, each checked on its own.
fn read<'t>(
    entries: &'t DeTable<'_>,
    registry: &Registry,
    found: &mut Found,
) -> (Vec<Entry<'t>>, Vec<(Entry<'t>, CommandId)>) {
    let (mut unbinds, mut binds) = (Vec::with_capacity(entries.len()), Vec::new());
    for (spelled, value) in entries {
        let Some(entry) = entry(spelled, found) else {
            continue;
        };
        let (spelled, at) = (entry.spelled, value.span().start);
        let unbinds_one_key =
            entry.keys.len() == 1 && matches!(value.get_ref(), DeValue::Boolean(false));
        // A typing key that began a sequence would become a prefix, and swallow the next key
        // typed. Unbinding it alone only stops it typing.
        if entry.keys[0].text().is_some() && !unbinds_one_key {
            found.about(
                entry.at,
                spelled,
                format_args!(
                    "a key that types a character cannot start a sequence; put it after a \
                     prefix, such as {}",
                    quoted(format_args!("C-c {}", display_keys(&entry.keys)))
                ),
            );
            continue;
        }
        match value.get_ref() {
            DeValue::Boolean(false) => unbinds.push(entry),
            DeValue::String(name) => match registry.lookup(name) {
                Some(command) => binds.push((entry, command)),
                None => found.about(
                    at,
                    spelled,
                    format_args!("no command is named {}", quoted(name)),
                ),
            },
            DeValue::Boolean(true) => found.about(
                at,
                spelled,
                format_args!("true binds nothing; give a command's name, or false to unbind"),
            ),
            other => found.about(
                at,
                spelled,
                format_args!(
                    "a key takes a command's name or false, not {}",
                    type_name(other)
                ),
            ),
        }
    }
    (unbinds, binds)
}

/// The entry for the keys `spelled`, if they are a sequence a person can type and may
/// configure.
fn entry<'t>(
    spelled: &'t Spanned<std::borrow::Cow<'_, str>>,
    found: &mut Found,
) -> Option<Entry<'t>> {
    let at = spelled.span().start;
    let spelled: &str = spelled.get_ref();
    let mut keys = Vec::new();
    for token in spelled.split_ascii_whitespace() {
        if keys.len() == MAX_KEYS {
            found.about(
                at,
                spelled,
                format_args!("more than {MAX_KEYS} keys in one sequence"),
            );
            return None;
        }
        let Ok(key) = token.parse::<KeyPress>() else {
            found.about(
                at,
                spelled,
                format_args!(
                    "{} is not a key; write keys as Emacs does, such as C-x, M-<f5> or SPC",
                    quoted(token)
                ),
            );
            return None;
        };
        keys.push(key);
    }
    if keys.is_empty() {
        found.about(at, spelled, format_args!("no keys"));
        return None;
    }
    if keys.iter().copied().any(is_cancel) {
        found.about(
            at,
            spelled,
            format_args!("C-g is not configurable; it always cancels"),
        );
        return None;
    }
    Some(Entry { spelled, at, keys })
}

/// Binds `entry` to `command`, unless it would make a prefix of a default command that the file
/// does not unbind (D1 in ADR 0110), or conflicts with the file's other entries.
fn bind(
    keymap: &mut Keymap,
    entry: &Entry<'_>,
    command: CommandId,
    registry: &Registry,
    defaults: &Keymap,
    unbound: &HashSet<&[KeyPress]>,
    found: &mut Found,
) {
    let keys = &entry.keys[..];
    let hidden = |len: usize| (1..=len).any(|len| unbound.contains(&keys[..len]));
    let default = defaults_along(defaults, keys);
    if let Along::Command(len, default) = default
        && !hidden(len)
    {
        let prefix = quoted(display_keys(&keys[..len]));
        let name = registry
            .get(default)
            .map_or("a command", |command| command.name);
        found.about(
            entry.at,
            entry.spelled,
            format_args!(
                "{prefix} runs {name} by default; add {prefix} = false to make it a prefix"
            ),
        );
        return;
    }
    if let Err(error) = keymap.bind(keys, command) {
        let (at, spelled) = (entry.at, entry.spelled);
        match error {
            BindError::AlreadyBound => found.about(
                at,
                spelled,
                format_args!("these keys are set more than once; skipped"),
            ),
            BindError::IsPrefix => found.about(
                at,
                spelled,
                format_args!("other entries set keys after these; skipped"),
            ),
            BindError::NonPrefix { len } => found.about(
                at,
                spelled,
                format_args!(
                    "{} is bound to a command here, so it cannot be a prefix; skipped",
                    quoted(display_keys(&keys[..len]))
                ),
            ),
            BindError::Empty => found.about(at, spelled, format_args!("no keys; skipped")),
        }
        return;
    }
    if let Along::Prefix(under) = default
        && !hidden(keys.len())
    {
        let mut count = 0;
        under.bindings(|_, _| count += 1);
        found.about(
            entry.at,
            entry.spelled,
            format_args!(
                "a prefix by default; this hides the {count} default key{} under it",
                if count == 1 { "" } else { "s" }
            ),
        );
    }
}

/// What the default keys have along a sequence.
enum Along<'d> {
    /// A command on its first `len` keys, shorter than the sequence.
    Command(usize, CommandId),
    /// A prefix on the whole sequence.
    Prefix(&'d Keymap),
    Nothing,
}

/// What the defaults have along `keys`, in one walk down their prefix keymaps.
fn defaults_along<'d>(defaults: &'d Keymap, keys: &[KeyPress]) -> Along<'d> {
    let mut level = defaults;
    for (i, key) in keys.iter().enumerate() {
        let whole = i + 1 == keys.len();
        match level.get(*key) {
            Some(Binding::Prefix(under)) if whole => return Along::Prefix(under),
            Some(Binding::Prefix(under)) => level = under,
            Some(Binding::Command(command)) if !whole => return Along::Command(i + 1, command),
            Some(Binding::Command(_) | Binding::Undefined) | None => break,
        }
    }
    Along::Nothing
}

/// Warns that an unbinding does nothing, when its last key has Shift and the same keys without
/// it are bound: an undefined key is taken without Shift, as Emacs takes a key bound to nil
/// (ADR 0110). `"C-F" = false` while `C-f` is bound leaves `C-F` running what `C-f` does. The
/// keys go through a resolver over the user's keys and the defaults, so the warning says what
/// pressing them does.
fn warn_if_shift_finds_more(
    keymap: &Keymap,
    entry: &Entry<'_>,
    defaults: &Keymap,
    found: &mut Found,
) {
    let Some((last, before)) = entry.keys.split_last() else {
        return;
    };
    let Some(unshifted) = last.unshifted() else {
        return;
    };
    let layers = Layers {
        user: Some(keymap),
        ..Layers::global(defaults)
    };
    let mut resolver = KeyResolver::new();
    for key in before {
        if resolver.feed(&layers, *key) != Resolution::Pending {
            return;
        }
    }
    let finds = match resolver.feed(&layers, *last) {
        Resolution::Shifted(_) => true,
        // A prefix found without Shift stays among the pending keys unshifted.
        Resolution::Pending => resolver.pending().last() == Some(&unshifted),
        Resolution::Command(_) | Resolution::Unbound(_) | Resolution::Cancelled => false,
    };
    if finds {
        let mut keys = before.to_vec();
        keys.push(unshifted);
        found.about(
            entry.at,
            entry.spelled,
            format_args!(
                "changes nothing while {} is bound, since a shifted key bound to nothing runs \
                 what it does without Shift",
                quoted(display_keys(&keys))
            ),
        );
    }
}
