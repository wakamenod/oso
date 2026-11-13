//! The built-in menus (ADR 0117): defaults that `config.toml` adds to, replaces whole by name, or
//! removes. They come first in every set, so their ids, which default keys name, never change.
//! Step 3 of ADR 0117 gives the first ones; until then there are none.
//!
//! Must not depend on: `dispatch`, `app` or winit; menus are data.

use oso_core::command::Registry;
use oso_core::menu::Menus;

/// The built-in menus over the commands in `registry`.
pub fn menus(_registry: &Registry) -> Menus {
    Menus::new()
}
