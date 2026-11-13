//! The menus a key may open (ADR 0117): the set in force, which goes in with the user's keymap
//! that names its menus, and the open menus: a stack of them, each over the one that opened it,
//! and the rules for the keys typed in them. A menu's keys come first while it is open, as
//! isearch's do; a key it does not bind follows its `foreign-keys` rule.
//!
//! Must not depend on: `app` or winit.

use std::sync::Arc;

use oso_core::command::{Args, Effects, Invocation, Prefix};
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::{Binding, Layers, is_cancel};
use oso_core::menu::{
    Definition, ForeignKeys, HeadAction, Lookup, MAX_DEPTH, MenuId, Menus, Open, Stay,
    builds_prefix,
};

use super::Dispatcher;

mod open_menu;
mod set;

use super::shown::{masked, reads_passphrase};
use open_menu::described;
pub use open_menu::{Described, HeadNow, OpenMenu};
pub(super) use set::MenuSet;

/// `RET`, which runs a menu's default head where no head binds it.
const RET: KeyPress = KeyPress::plain(oso_core::key::Key::Named(oso_core::key::NamedKey::Return));

/// The open menus: the set the first one came from, so that a submenu's id names the menu it
/// named when that one opened, whatever a reload has put in since (ADR 0117 step 2), and each
/// level over the one that opened it.
#[derive(Debug)]
struct Stack {
    set: Arc<Menus>,
    levels: Vec<Level>,
    /// The keys typed so far that begin a head's.
    pending: Vec<KeyPress>,
    /// Whether a macro's replay that is still under way opened it: that replay presses its
    /// heads, and one that begins once it ended closes it (ADR 0117, Q19).
    in_replay: bool,
}

/// One open menu.
#[derive(Debug)]
struct Level {
    id: MenuId,
    open: Open,
}

/// What a key typed in an open menu comes to.
enum InMenu {
    /// Handled: what the host should do.
    Done(Effects),
    /// Not a key of the menu's; it goes on as usual, and the menu stays.
    Through,
}

impl Dispatcher {
    /// The open menu on top, if one is open, to draw.
    pub fn menu(&self) -> Option<OpenMenu<'_>> {
        let stack = self.menus.open.as_ref()?;
        Some(OpenMenu {
            dispatcher: self,
            stack,
        })
    }

    /// Whether a menu is open: the person is in the middle of something, and the input method
    /// is off (ADR 0117).
    pub fn menu_open(&self) -> bool {
        self.menus.is_open()
    }

    /// The name of the menu shown, if one is open, for the state the channel and the dev
    /// socket give.
    pub fn menu_name(&self) -> Option<&str> {
        let stack = self.menus.open.as_ref()?;
        stack
            .levels
            .last()
            .map(|level| level.open.definition().name())
    }

    /// A key bound to `menu` was pressed: it opens, unless a prompt is open, as no menu sits
    /// over one (ADR 0117, Q20). A prefix argument typed before it goes to its first head.
    pub(super) fn menu_key(&mut self, editor: &mut Editor, menu: MenuId) -> Effects {
        let set = Arc::clone(&self.menus.menus);
        self.open_menu(editor, &set, menu)
    }

    /// Opens the menu `name` names in the set in force, as `open-menu`'s answer asks.
    pub(super) fn open_menu_named(&mut self, editor: &mut Editor, name: &str) -> Effects {
        let set = Arc::clone(&self.menus.menus);
        match set.lookup(name) {
            Some(id) => self.open_menu(editor, &set, id),
            None => {
                editor.message(format!("No menu named {name}"));
                self.forget_keys(editor);
                Effects::default()
            }
        }
    }

    fn open_menu(&mut self, editor: &mut Editor, set: &Arc<Menus>, menu: MenuId) -> Effects {
        if editor.prompt().is_some() {
            editor.prompt_says("Not in the minibuffer");
            self.forget_keys(editor);
            return Effects::default();
        }
        let Some(definition) = set.get(menu) else {
            // One the file removed, bound by a default key (ADR 0117): step 3 unbinds such
            // keys in the user's keymap, so that they say what an undefined key says.
            log::debug!("a key names a menu no longer defined");
            editor.echo("That key is undefined");
            self.forget_keys(editor);
            return Effects::default();
        };
        log::debug!("menu {} opens", definition.name());
        // Its key is one a search, a jump or the layer of `C-x TAB` does not bind, which ends
        // them first (ADR 0117), so that the menu opens over none of them.
        editor.isearch_exit();
        editor.replace_exit();
        editor.jump_end();
        editor.set_indent_rigidly_layer(false);
        self.menus.open(Stack {
            set: Arc::clone(set),
            levels: vec![Level {
                id: menu,
                open: Open::new(Arc::clone(definition)),
            }],
            pending: Vec::new(),
            in_replay: editor.macro_replaying(),
        });
        // A command keeps the run it counts only across keys of its own.
        self.last = None;
        redraw()
    }

    /// Closes the open menus if something else now takes the keys: a prompt or a question, a
    /// search, a jump, a query-replace, the layer of `C-x TAB`, a macro's replay, or a command
    /// waiting for an argument or the clipboard. A menu never sits over any of them
    /// (ADR 0117).
    pub(super) fn close_menus_if_taken(&mut self, editor: &Editor) -> bool {
        let Some(stack) = self.menus.open.as_mut() else {
            return false;
        };
        // A replay that opened the menu presses its heads; once it ends, the menu is the
        // person's, and a replay begun since takes the keys.
        if !editor.macro_replaying() {
            stack.in_replay = false;
        }
        let replay_began = editor.macro_replaying() && !stack.in_replay;
        let taken = editor.prompt().is_some()
            || editor.isearch().is_some()
            || editor.jump().is_some()
            || editor.replace().is_some()
            || editor.indent_rigidly_layer()
            || replay_began
            || !self.reading.is_empty()
            || self.waiting.is_some();
        if taken {
            log::debug!("menu closes for what took the keys");
            self.menus.close();
            return true;
        }
        false
    }

    /// Handles `key` in the open menu, if one is open and no prefix key of the usual keymaps
    /// waits; `None` where it goes on as usual. A menu that something took the keys from
    /// outside an input closes first, and the key goes where they went.
    pub(super) fn menu_takes(&mut self, editor: &mut Editor, key: KeyPress) -> Option<Effects> {
        if !self.menus.is_open() || !self.resolver.pending().is_empty() {
            return None;
        }
        if self.close_menus_if_taken(editor) {
            return None;
        }
        match self.in_menu(editor, key) {
            InMenu::Done(effects) => Some(effects),
            InMenu::Through => None,
        }
    }

    fn in_menu(&mut self, editor: &mut Editor, key: KeyPress) -> InMenu {
        if is_cancel(key) {
            return InMenu::Done(self.menu_cancel(editor));
        }
        let Some(definition) = self.top_definition() else {
            return InMenu::Through;
        };
        let notes = self
            .menus
            .open
            .as_ref()
            .and_then(|stack| stack.levels.last())
            .map(|level| described(editor, &level.open))
            .unwrap_or_default();
        let Some(stack) = self.menus.open.as_mut() else {
            return InMenu::Through;
        };
        if builds_prefix(key) && stack.pending.is_empty() {
            return InMenu::Done(self.menu_prefix(editor, key));
        }
        let mut keys = std::mem::take(&mut stack.pending);
        keys.push(key);
        let head = match definition.lookup(&keys) {
            Lookup::Head(head) => Some(head),
            Lookup::Pending => {
                stack.pending = keys;
                return InMenu::Done(Effects::default());
            }
            Lookup::Quit => {
                self.forget_keys(editor);
                return InMenu::Done(self.close_level());
            }
            Lookup::Unbound if keys == [RET] => notes.ret_head(&definition),
            Lookup::Unbound => None,
        };
        match head.map(|head| (head, notes.now(head))) {
            Some((head, HeadNow::Shown)) => InMenu::Done(self.run_head(editor, &definition, head)),
            Some((_, HeadNow::Unavailable(why))) => {
                editor.echo(why.to_owned());
                self.forget_keys(editor);
                InMenu::Done(Effects::default())
            }
            Some((_, HeadNow::Hidden)) | None => self.foreign(editor, &definition, keys),
        }
    }

    /// The menu on top, as the set it came from holds it. The set always holds it, as a level
    /// opens only from it; were it missing, the menus close, so no hidden menu takes keys.
    fn top_definition(&mut self) -> Option<Arc<Definition>> {
        let stack = self.menus.open.as_ref()?;
        let found = stack
            .levels
            .last()
            .and_then(|level| stack.set.get(level.id))
            .map(Arc::clone);
        if found.is_none() {
            log::error!("an open menu is not in the set it came from");
            self.menus.close();
        }
        found
    }

    /// `C-g` in a menu: it ends the keys that begin a head, else the prefix argument, else
    /// closes one level (ADR 0117, Q5, Q16).
    fn menu_cancel(&mut self, editor: &mut Editor) -> Effects {
        let Some(stack) = self.menus.open.as_mut() else {
            return Effects::default();
        };
        if !stack.pending.is_empty() || self.prefix.is_some() {
            stack.pending.clear();
            log::debug!("Quit");
            editor.echo("Quit");
            self.forget_keys(editor);
            return redraw();
        }
        self.forget_keys(editor);
        self.close_level()
    }

    /// Closes the menu on top, and shows the one under it, if any.
    fn close_level(&mut self) -> Effects {
        let set = &mut self.menus;
        if let Some(stack) = set.open.as_mut() {
            stack.pending.clear();
            stack.levels.pop();
            if stack.levels.is_empty() {
                set.open = None;
            }
            set.changes += 1;
        }
        redraw()
    }

    /// `C-u`, `M-<digit>` or `M--` in a menu: they build a prefix argument for the next head,
    /// as the default global keymap binds them, whatever the user's keys bind (ADR 0117, Q15). Plain
    /// digits and `-` are keys for heads.
    fn menu_prefix(&mut self, editor: &mut Editor, key: KeyPress) -> Effects {
        let Some(Binding::Command(id)) = Layers::global(&self.keymaps.global).lookup(&[key]) else {
            return Effects::default();
        };
        let mut utf8 = [0; 4];
        let input = key.text().map_or("", |c| c.encode_utf8(&mut utf8));
        let key_char = match key.key {
            oso_core::key::Key::Char(c) => Some(c),
            oso_core::key::Key::Named(_) => None,
        };
        let invocation = Invocation {
            input,
            key_char,
            ..Invocation::default()
        };
        let args = Args::with_prefix(self.prefix.take());
        let (effects, _) = self.run_or_read(editor, id, args, invocation);
        effects.union(redraw())
    }

    /// Runs the head `head` of `definition`, the menu on top, as its kind and its menu say
    /// (ADR 0117).
    fn run_head(&mut self, editor: &mut Editor, definition: &Definition, head: usize) -> Effects {
        let Some(given) = definition.heads().get(head) else {
            return Effects::default();
        };
        let count = given.count.map(|count| Prefix::Closed(isize::from(count)));
        match &given.action {
            HeadAction::Switch(name) => {
                if let Some(level) = self.menus.open.as_mut().and_then(|s| s.levels.last_mut()) {
                    level.open.flip(name);
                }
                redraw()
            }
            HeadAction::Quit => {
                self.forget_keys(editor);
                self.menus.close();
                redraw()
            }
            HeadAction::Menu(id) => self.push_menu(editor, *id, count),
            HeadAction::Command(id) => {
                let owned: Vec<String> = self
                    .menus
                    .open
                    .as_ref()
                    .and_then(|stack| stack.levels.last())
                    .map(|level| level.open.switches_on().map(str::to_owned).collect())
                    .unwrap_or_default();
                let switches: Vec<&str> = owned.iter().map(String::as_str).collect();
                // A head's count replaces a prefix argument typed before it (ADR 0117, Q14, Q15).
                let prefix = self.prefix.take();
                let args = Args::with_prefix(count.or(prefix));
                if definition.stay(head) == Stay::Exit {
                    self.menus.close();
                }
                let invocation = Invocation {
                    switches: &switches,
                    ..Invocation::default()
                };
                let readings = self.reading.len();
                let (effects, _) = self.run_or_read(editor, *id, args, invocation);
                // A command that reads an argument first reads the switches once it runs.
                for reading in self.reading.iter_mut().skip(readings) {
                    reading.switches.clone_from(&owned);
                }
                effects.union(self.after_prompts(editor)).union(redraw())
            }
        }
    }

    /// Opens the menu `id` names in the set the open ones came from over the one on top,
    /// unless it is open already or the stack is as deep as it goes (ADR 0117, Q16). `count` passes on as
    /// the prefix argument.
    fn push_menu(&mut self, editor: &mut Editor, id: MenuId, count: Option<Prefix>) -> Effects {
        let Some(stack) = self.menus.open.as_mut() else {
            return Effects::default();
        };
        let refused = if let Some(level) = stack.levels.iter().find(|level| level.id == id) {
            Some(format!(
                "{} is already open",
                level.open.definition().name()
            ))
        } else if stack.levels.len() >= MAX_DEPTH {
            Some(format!("Menus open at most {MAX_DEPTH} deep"))
        } else if let Some(sub) = stack.set.get(id) {
            stack.levels.push(Level {
                id,
                open: Open::new(Arc::clone(sub)),
            });
            None
        } else {
            Some("That menu is not defined".to_owned())
        };
        if let Some(refused) = refused {
            editor.echo(refused);
            self.forget_keys(editor);
            return Effects::default();
        }
        self.menus.changes += 1;
        // A head's count passes on, as a prefix argument typed before it would.
        if count.is_some() {
            self.prefix = count;
        }
        redraw()
    }

    /// `keys`, which `definition`, the menu on top, does not bind, as its `foreign-keys` says:
    /// refused, or run as usual with the menu closed first, or run as usual with it kept
    /// (ADR 0117, Q4).
    fn foreign(
        &mut self,
        editor: &mut Editor,
        definition: &Definition,
        keys: Vec<KeyPress>,
    ) -> InMenu {
        match definition.foreign_keys() {
            ForeignKeys::Warn => {
                log::debug!("{} is not in the menu", masked(&keys, true));
                let quit = if definition.quits_on_q() { "q" } else { "C-g" };
                let keys = masked(&keys, reads_passphrase(editor));
                editor.echo(format!("{keys} is not in this menu: {quit} to quit"));
                self.forget_keys(editor);
                InMenu::Done(Effects::default())
            }
            ForeignKeys::Exit => {
                self.menus.close();
                let mut ran = redraw();
                for key in keys {
                    ran = ran.union(self.resolve_key(editor, key));
                }
                InMenu::Done(ran)
            }
            ForeignKeys::Run => {
                let mut ran = Effects::default();
                for key in keys {
                    ran = ran.union(self.resolve_usual(editor, key));
                }
                InMenu::Done(ran)
            }
        }
    }

    /// Text in an open menu, which binds none: refused, or put in as usual with the menu closed
    /// first or kept, as its `foreign-keys` says (ADR 0117). `true` where it was refused.
    pub(super) fn menu_refuses_text(&mut self, editor: &mut Editor) -> bool {
        if !self.menus.is_open() || !self.resolver.pending().is_empty() {
            return false;
        }
        if self.close_menus_if_taken(editor) {
            return false;
        }
        let Some(definition) = self.top_definition() else {
            return false;
        };
        match definition.foreign_keys() {
            ForeignKeys::Warn => {
                let quit = if definition.quits_on_q() { "q" } else { "C-g" };
                // The text stays out of it, as it does from a log.
                editor.echo(format!("That text is not in this menu: {quit} to quit"));
                self.forget_keys(editor);
                true
            }
            ForeignKeys::Exit => {
                self.menus.close();
                false
            }
            ForeignKeys::Run => false,
        }
    }
}

/// What a change to the float asks of the host.
fn redraw() -> Effects {
    Effects {
        redraw: true,
        ..Effects::default()
    }
}
