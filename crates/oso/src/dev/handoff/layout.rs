//! The windows a state hands over and the tree they are laid out by (ADR 0090, ADR 0112):
//! each tiled window's buffer and positions, in the tree's order, the layout with each
//! child's weight, and which window is selected.
//!
//! Must not depend on: the control socket, the window system or threads; it turns the editor's
//! tree into a state's fields and back, and checks them before anything changes.

use oso_core::frame::{Tiling, Way};
use oso_core::selection::SelectionSet;
use oso_core::window::Scroll;
use serde_json::{Map, Value, json};

use super::format::{array, parse_selections, size};

/// The most windows a state may hand over: what a frame of 400 by 120 cells holds at the
/// smallest window, with room to spare (ADR 0112).
pub const MAX_WINDOWS: usize = 4096;

/// A window the state hands over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowEntry {
    /// The position in the state's buffers of the buffer it shows.
    pub shown: usize,
    pub selections: SelectionSet,
    pub scroll: Scroll,
    /// Whether `scroll`'s start is a logical line, as a state before version 3 kept the top.
    pub top_is_row: bool,
}

/// The windows of a state, in the tree's order, the layout and the selected window's place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Windows {
    pub windows: Vec<WindowEntry>,
    pub layout: Tiling,
    pub selected: usize,
}

/// `tiling` as a state keeps it: `{"window": i}` for the `i`th window in the tree's order,
/// counted by `next`, or `{"across": [...]}` or `{"down": [...]}` of `{"weight": n,
/// "window": i}` and `{"weight": n, "run": {...}}`.
pub fn layout_state(tiling: &Tiling, next: &mut usize) -> Value {
    let Tiling::Run { way, children } = tiling else {
        let window = *next;
        *next += 1;
        return json!({ "window": window });
    };
    let children: Vec<Value> = children
        .iter()
        .map(|(weight, child)| match child {
            Tiling::Window => {
                let window = *next;
                *next += 1;
                json!({ "weight": weight, "window": window })
            }
            Tiling::Run { .. } => json!({ "weight": weight, "run": layout_state(child, next) }),
        })
        .collect();
    let way = match way {
        Way::Across => "across",
        Way::Down => "down",
    };
    json!({ way: children })
}

/// Reads a state's windows, each showing one of `buffers`, the state's buffers as written.
/// Before version 4 a state has one window, selected, and no layout.
pub fn parse_windows(
    state: &Map<String, Value>,
    buffers: &[Value],
    version: u64,
) -> Result<Windows, String> {
    let windows = array(state, "windows")?;
    let selected = size(state, "selected")?;
    let layout = if version < 4 {
        if windows.len() != 1 || selected != 0 {
            return Err("the state must have one window, selected".into());
        }
        Tiling::Window
    } else {
        let layout = state.get("layout").ok_or("the state has no layout")?;
        parse_layout(layout, &mut 0, 0)?
    };
    if windows.is_empty() || windows.len() > MAX_WINDOWS {
        return Err(format!(
            "the state must have from 1 to {MAX_WINDOWS} windows"
        ));
    }
    if layout.windows() != windows.len() {
        return Err("the layout does not hold the state's windows".into());
    }
    if selected >= windows.len() {
        return Err("the selected window is not one of the state's".into());
    }
    let windows = windows
        .iter()
        .map(|window| parse_window(window, buffers, version))
        .collect::<Result<_, _>>()?;
    Ok(Windows {
        windows,
        layout,
        selected,
    })
}

fn parse_window(window: &Value, buffers: &[Value], version: u64) -> Result<WindowEntry, String> {
    let window = window.as_object().ok_or("a window is not an object")?;
    let shown = buffers
        .iter()
        .position(|entry| entry.get("id") == window.get("buffer"))
        .ok_or("a window shows a buffer the state does not have")?;
    let scroll = window
        .get("scroll")
        .and_then(Value::as_object)
        .ok_or("a window has no scroll position")?;
    // Before version 3 the top was a row; one still waiting for its file goes on as a row.
    let top_is_row = version < 3 || !scroll.contains_key("start");
    Ok(WindowEntry {
        shown,
        selections: parse_selections(window)?,
        scroll: Scroll {
            start: size(scroll, if top_is_row { "top" } else { "start" })?,
            left: size(scroll, "left")?,
        },
        top_is_row,
    })
}

/// Reads a layout: `{"window": 0}` for one window, or a run.
fn parse_layout(layout: &Value, next: &mut usize, depth: usize) -> Result<Tiling, String> {
    let layout = layout.as_object().ok_or("a layout is not an object")?;
    if layout.contains_key("window") {
        if layout.len() != 1 {
            return Err("a layout's window says more than its number".into());
        }
        return parse_leaf(layout, next);
    }
    parse_run(layout, next, depth)
}

/// Reads the window of a layout, which must be the next in the tree's order.
fn parse_leaf(leaf: &Map<String, Value>, next: &mut usize) -> Result<Tiling, String> {
    if size(leaf, "window")? != *next {
        return Err("the layout's windows are not in the tree's order".into());
    }
    *next += 1;
    Ok(Tiling::Window)
}

/// Reads a run `depth` runs deep, whose windows must come numbered in the tree's order from
/// `next` on. A run holds two children or more, each `{"weight": n, "window": i}` or
/// `{"weight": n, "run": {...}}` with a weight of at least 1, and runs nest at most
/// [`Tiling::MAX_DEPTH`] deep. Nothing else may stand beside them.
fn parse_run(run: &Map<String, Value>, next: &mut usize, depth: usize) -> Result<Tiling, String> {
    if depth >= Tiling::MAX_DEPTH {
        return Err(format!(
            "the layout nests deeper than {} runs",
            Tiling::MAX_DEPTH
        ));
    }
    if run.len() != 1 {
        return Err("a run of the layout is neither across nor down".into());
    }
    let (way, children) = match (run.get("across"), run.get("down")) {
        (Some(children), None) => (Way::Across, children),
        (None, Some(children)) => (Way::Down, children),
        _ => return Err("a run of the layout is neither across nor down".into()),
    };
    let children = children
        .as_array()
        .ok_or("a run's children are not a list")?;
    if children.len() < 2 {
        return Err("a run of the layout has fewer than two children".into());
    }
    if children.len() > MAX_WINDOWS {
        return Err(format!(
            "a run of the layout has more than {MAX_WINDOWS} children"
        ));
    }
    let children = children
        .iter()
        .map(|child| {
            let child = child
                .as_object()
                .ok_or("a child of a run is not an object")?;
            let weight = size(child, "weight")?;
            if weight == 0 {
                return Err("a child of a run has no weight".to_owned());
            }
            if child.len() != 2 {
                return Err("a child of a run is neither a window nor a run".to_owned());
            }
            let node = match (child.contains_key("window"), child.get("run")) {
                (true, None) => parse_leaf(child, next)?,
                (false, Some(run)) => {
                    let run = run.as_object().ok_or("a run is not an object")?;
                    parse_run(run, next, depth + 1)?
                }
                _ => return Err("a child of a run is neither a window nor a run".to_owned()),
            };
            Ok((weight, node))
        })
        .collect::<Result<_, String>>()?;
    Ok(Tiling::Run { way, children })
}
