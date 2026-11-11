//! The user's keymap from the file's `[keys]` table: each entry under `[keys.global]` binds a key
//! sequence to a command or leaves it undefined, checked against the default keys it would
//! shadow (ADR 0108, ADR 0110).
//!
//! Must not depend on: `app` or `dispatch`; it builds a [`Keymap`] for the dispatcher to take,
//! from the registry and default keymap it is given.

use std::collections::HashSet;

use oso_core::command::{CommandId, Registry};
use oso_core::key::KeyPress;
use oso_core::keymap::{BindError, Binding, CANCEL, Keymap, Layers, display_keys};
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::problems::{Found, quoted};
use super::{table, table_name};

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
                "[keys.question] is never configurable: its keys answer the agent channel's \
                 questions"
                    .to_owned(),
            ),
            other => found.at(
                at,
                format!(
                    "[keys.{}] is not configurable yet; only [keys.global] is",
                    table_name(other)
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
            format!(
                "[keys.global] has {} entries, more than {MAX_ENTRIES}; none of its keys apply",
                entries.len()
            ),
        );
        return keymap;
    }
    let (mut unbinds, mut binds) = read(entries, registry, found);
    // Shorter sequences first, so that of two entries that conflict the longer one is skipped,
    // whatever the file's order.
    unbinds.sort_by(|a, b| (a.keys.len(), a.spelled).cmp(&(b.keys.len(), b.spelled)));
    binds.sort_by(|(a, _), (b, _)| (a.keys.len(), a.spelled).cmp(&(b.keys.len(), b.spelled)));
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
    let (mut unbinds, mut binds) = (Vec::new(), Vec::new());
    for (spelled, value) in entries {
        let Some(entry) = entry(spelled, found) else {
            continue;
        };
        let (spelled, at) = (entry.spelled, value.span().start);
        match value.get_ref() {
            DeValue::Boolean(false) => unbinds.push(entry),
            DeValue::String(name) => {
                if entry.keys[0].text().is_some() {
                    found.about(
                        entry.at,
                        spelled,
                        format_args!(
                            "a key that types a character cannot start a binding; put it after a \
                             prefix, such as {}",
                            quoted(&format!("C-c {}", display_keys(&entry.keys)))
                        ),
                    );
                } else if let Some(command) = registry.lookup(name) {
                    binds.push((entry, command));
                } else {
                    found.about(
                        at,
                        spelled,
                        format_args!("no command is named {}", quoted(name)),
                    );
                }
            }
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
    if keys
        .iter()
        .any(|&key| key == CANCEL || key.unshifted() == Some(CANCEL))
    {
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
    for len in 1..keys.len() {
        if let Some(Binding::Command(default)) = defaults.lookup(&keys[..len])
            && !hidden(len)
        {
            let prefix = quoted(&display_keys(&keys[..len]).to_string());
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
    }
    let refused = match keymap.bind(keys, command) {
        Ok(()) => None,
        Err(BindError::AlreadyBound) => Some("these keys are set more than once".to_owned()),
        Err(BindError::IsPrefix) => Some("other entries set keys after these".to_owned()),
        Err(BindError::NonPrefix { len }) => Some(format!(
            "{} is bound to a command here, so it cannot be a prefix",
            quoted(&display_keys(&keys[..len]).to_string())
        )),
        Err(BindError::Empty) => Some("no keys".to_owned()),
    };
    if let Some(reason) = refused {
        found.about(entry.at, entry.spelled, format_args!("{reason}; skipped"));
        return;
    }
    if let Some(Binding::Prefix(under)) = defaults.lookup(keys)
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

/// Warns that an unbinding does nothing, when its last key has Shift and the same keys without
/// it are bound: an undefined key is taken without Shift, as Emacs takes a key bound to nil
/// (ADR 0110). `"C-F" = false` while `C-f` is bound leaves `C-F` running what `C-f` does.
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
    let mut keys = before.to_vec();
    keys.push(unshifted);
    let layers = Layers {
        user: Some(keymap),
        ..Layers::global(defaults)
    };
    if matches!(
        layers.lookup(&keys),
        Some(Binding::Command(_) | Binding::Prefix(_))
    ) {
        let without = quoted(&display_keys(&keys).to_string());
        found.about(
            entry.at,
            entry.spelled,
            format_args!(
                "changes nothing while {without} is bound, since a shifted key bound to nothing \
                 runs what it does without Shift"
            ),
        );
    }
}

/// What a TOML value is, for a message.
pub(super) fn type_name(value: &DeValue<'_>) -> &'static str {
    match value {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) | DeValue::Float(_) => "a number",
        DeValue::Boolean(_) => "true or false",
        DeValue::Datetime(_) => "a date",
        DeValue::Array(_) => "a list",
        DeValue::Table(_) => "a table",
    }
}
