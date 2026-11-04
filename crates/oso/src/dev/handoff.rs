//! The state one instance hands to the next on a restart: `export-state` and
//! `import-state` (ADR 0023, ADR 0045).
//!
//! Must not depend on: `app`, `server`, winit or sockets; these are plain
//! functions of the editor state, and the caller opens the files.
//!
//! The state holds paths and positions and never any text, so a secret file's
//! content cannot leave through it (ADR 0044), and a response line is enough
//! to carry it (ADR 0032). The new instance reads each file from disk. Nothing
//! unsaved exists until M2, when text may join the format for buffers that are
//! not secret. A path-less buffer with text cannot occur before then either,
//! so exporting one is an error rather than a silent loss.
//!
//! Parsing changes nothing, so a state with another version or a bad field
//! leaves the new instance clean, and the old one is kept.

use std::path::PathBuf;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::display_map::DisplayPoint;
use oso_core::editor::Editor;
use oso_core::key::{KeyPress, KeySequence};
use oso_core::keymap::display_keys;
use oso_core::selection::{Goal, Selection, SelectionSet};
use oso_core::sum_tree::Bias;
use oso_core::window::Scroll;
use serde_json::{Map, Value, json};

use super::protocol::{MAX_LINE, Reply};
use super::state::goal_state;

/// The version of the state format. Bump it on any change that the previous
/// reader would get wrong.
pub const STATE_VERSION: u64 = 1;

/// The longest state, leaving room for the request line that carries it.
const MAX_STATE: usize = MAX_LINE - 256;

/// The most buffers one state may open, each with a loading thread.
const MAX_BUFFERS: usize = 1024;

/// The OS window's place on the screen and its inner size, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The result of `export-state`.
pub fn export(editor: &Editor, pending: &[KeyPress], frame: Option<Frame>) -> Reply {
    let mut buffers = Vec::new();
    for (id, buffer) in editor.buffers() {
        buffers.push(buffer_state(id, buffer)?);
    }
    let windows: Vec<Value> = editor
        .windows()
        .map(|window| {
            json!({
                "buffer": window.buffer().get(),
                "primary": window.selections.primary_index(),
                "selections": window.selections.iter().map(|selection| json!({
                    "cursor": selection.cursor,
                    "goal": goal_state(selection.goal),
                })).collect::<Vec<_>>(),
                "scroll": { "top": window.scroll.top, "left": window.scroll.left },
            })
        })
        .collect();
    let state = json!({
        "version": STATE_VERSION,
        "buffers": buffers,
        "windows": windows,
        "selected": editor.selected_window(),
        "pending_keys": display_keys(pending).to_string(),
        "frame": frame.map(|frame| json!({
            "x": frame.x,
            "y": frame.y,
            "width": frame.width,
            "height": frame.height,
        })),
    });
    let size = state.to_string().len();
    if size > MAX_STATE {
        return Err(format!(
            "the state takes {size} bytes, more than the {MAX_STATE} it may"
        ));
    }
    Ok(state)
}

fn buffer_state(id: BufferId, buffer: &Buffer) -> Result<Value, String> {
    let path = match buffer.path() {
        Some(path) => Some(path.to_str().ok_or_else(|| {
            format!(
                "cannot hand over a path that is not UTF-8: {}",
                path.display()
            )
        })?),
        None if buffer.text().is_empty() => None,
        None => {
            return Err(format!(
                "cannot hand over {}: it has text and no file",
                buffer.name()
            ));
        }
    };
    Ok(json!({ "id": id.get(), "name": buffer.name(), "path": path }))
}

/// A parsed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub buffers: Vec<BufferEntry>,
    /// The position in `buffers` of the buffer the window shows.
    pub shown: usize,
    pub selections: SelectionSet,
    pub scroll: Scroll,
    pub pending_keys: Vec<KeyPress>,
    pub frame: Option<Frame>,
}

/// A buffer to open again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferEntry {
    pub name: String,
    /// An absolute path, or `None` for a buffer with no file.
    pub path: Option<PathBuf>,
}

/// Reads a state, checking everything before anything changes. There is one
/// window until splits arrive (M3), so a state with others is refused.
pub fn parse(state: &Value) -> Result<State, String> {
    let state = state.as_object().ok_or("the state is not an object")?;
    let version = state.get("version").and_then(Value::as_u64);
    if version != Some(STATE_VERSION) {
        return Err(format!(
            "the state has version {}, and this build reads version {STATE_VERSION}",
            state.get("version").unwrap_or(&Value::Null)
        ));
    }
    let entries = array(state, "buffers")?;
    if entries.len() > MAX_BUFFERS {
        return Err(format!("the state has more than {MAX_BUFFERS} buffers"));
    }
    let buffers = entries.iter().map(parse_buffer).collect::<Result<_, _>>()?;
    let windows = array(state, "windows")?;
    let ([window], Some(0)) = (&windows[..], state.get("selected").and_then(Value::as_u64)) else {
        return Err("the state must have one window, selected".into());
    };
    let window = window.as_object().ok_or("a window is not an object")?;
    let shown = entries
        .iter()
        .position(|entry| entry.get("id") == window.get("buffer"))
        .ok_or("the window shows a buffer the state does not have")?;
    let scroll = window
        .get("scroll")
        .and_then(Value::as_object)
        .ok_or("a window has no scroll position")?;
    Ok(State {
        buffers,
        shown,
        selections: parse_selections(window)?,
        scroll: Scroll {
            top: size(scroll, "top")?,
            left: size(scroll, "left")?,
        },
        pending_keys: parse_keys(state)?,
        frame: parse_frame(state)?,
    })
}

fn parse_buffer(entry: &Value) -> Result<BufferEntry, String> {
    let entry = entry.as_object().ok_or("a buffer is not an object")?;
    if entry.get("id").and_then(Value::as_u64).is_none() {
        return Err("a buffer has no id".into());
    }
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .ok_or("a buffer has no name")?
        .to_owned();
    let path = match entry.get("path") {
        None | Some(Value::Null) => None,
        Some(Value::String(path)) => Some(PathBuf::from(path)),
        Some(_) => return Err("a buffer's path is not a string".into()),
    };
    if path.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err("a buffer's path is not absolute".into());
    }
    Ok(BufferEntry { name, path })
}

fn parse_selections(window: &Map<String, Value>) -> Result<SelectionSet, String> {
    let selections = array(window, "selections")?
        .iter()
        .map(|selection| {
            let selection = selection
                .as_object()
                .ok_or("a selection is not an object")?;
            let goal = match selection.get("goal") {
                None | Some(Value::Null) => None,
                Some(Value::String(goal)) if goal == "line-end" => Some(Goal::LineEnd),
                Some(Value::Object(goal)) => Some(Goal::Column(size(goal, "column")?)),
                Some(_) => return Err("a selection's goal is not understood".to_owned()),
            };
            Ok(Selection {
                cursor: size(selection, "cursor")?,
                goal,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    SelectionSet::from_parts(selections, size(window, "primary")?)
        .ok_or_else(|| "a window's primary selection does not exist".to_owned())
}

fn parse_keys(state: &Map<String, Value>) -> Result<Vec<KeyPress>, String> {
    let keys = state
        .get("pending_keys")
        .and_then(Value::as_str)
        .ok_or("the state has no pending keys")?;
    let KeySequence(keys) = keys.parse().map_err(|e| format!("{e}"))?;
    Ok(keys)
}

fn parse_frame(state: &Map<String, Value>) -> Result<Option<Frame>, String> {
    let frame = match state.get("frame") {
        None | Some(Value::Null) => return Ok(None),
        Some(frame) => frame.as_object().ok_or("the frame is not an object")?,
    };
    let int = |name: &str| {
        frame
            .get(name)
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| format!("the frame's {name} is not a position"))
    };
    let side = |name: &str| {
        frame
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|&value| value > 0)
            .ok_or_else(|| format!("the frame's {name} is not a size"))
    };
    Ok(Some(Frame {
        x: int("x")?,
        y: int("y")?,
        width: side("width")?,
        height: side("height")?,
    }))
}

fn array<'a>(object: &'a Map<String, Value>, name: &str) -> Result<&'a Vec<Value>, String> {
    object
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("the state has no {name}"))
}

fn size(object: &Map<String, Value>, name: &str) -> Result<usize, String> {
    object
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("{name} is not a size"))
}

/// Positions waiting for their file to load, since a cursor cannot sit in text
/// that has not arrived. The file may also have changed since the export, so
/// they are moved to the nearest place that exists.
#[derive(Debug)]
pub struct Restore {
    pub buffer: BufferId,
    pub selections: SelectionSet,
    pub scroll: Scroll,
}

impl Restore {
    /// Moves the window to the positions if the buffer holds them by now.
    /// Returns whether this is over: restored, or the window has moved on.
    pub fn try_apply(&self, editor: &mut Editor) -> bool {
        let (window, buffer) = editor.current_mut();
        if window.buffer() != self.buffer {
            return true;
        }
        let needed = self.selections.iter().map(|s| s.cursor).max().unwrap_or(0);
        if buffer.loading().is_some() && buffer.text().len() < needed {
            return false;
        }
        let display = buffer.display();
        let selections = self
            .selections
            .iter()
            .map(|selection| {
                let point: DisplayPoint = display.offset_to_display_point(selection.cursor);
                Selection {
                    cursor: display.display_point_to_offset(point, Bias::Left),
                    goal: selection.goal,
                }
            })
            .collect();
        if let Some(selections) =
            SelectionSet::from_parts(selections, self.selections.primary_index())
        {
            window.selections = selections;
        }
        window.scroll = Scroll {
            top: self.scroll.top.min(display.row_count() - 1),
            left: self.scroll.left,
        };
        editor.follow_cursor();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{BufferEntry, Frame, MAX_BUFFERS, Restore, STATE_VERSION, export, parse};
    use oso_core::buffer::Buffer;
    use oso_core::display_map::TabWidth;
    use oso_core::editor::Editor;
    use oso_core::key::KeySequence;
    use oso_core::load::decode_all;
    use oso_core::selection::{Goal, Selection, SelectionSet};
    use oso_core::window::{Scroll, ViewSize};
    use serde_json::{Value, json};
    use std::path::PathBuf;

    /// An editor showing `path` with `content`, loaded.
    fn visiting(path: &str, content: &str) -> Editor {
        let mut editor = Editor::default();
        let mut buffer = Buffer::visiting(PathBuf::from(path));
        buffer.apply_load(decode_all(content.as_bytes(), TabWidth::default()));
        buffer.judge_secret(Some(&[PathBuf::from(path)]));
        let id = editor.add_buffer(buffer);
        editor.show(id);
        editor.set_view_size(ViewSize::new(5, 20));
        editor
    }

    /// A change that spoils a state.
    type Mutation = Box<dyn Fn(&mut Value)>;

    fn with(mutate: impl FnOnce(&mut Value)) -> Value {
        let mut state = export(&visiting("/p/a.txt", "abc\n"), &[], None).unwrap();
        mutate(&mut state);
        state
    }

    #[test]
    fn a_state_round_trips() {
        let mut editor = visiting("/p/a.txt", &"line\n".repeat(50));
        let selections = vec![
            Selection::at(5),
            Selection {
                cursor: 10,
                goal: Some(Goal::Column(3)),
            },
            Selection {
                cursor: 15,
                goal: Some(Goal::LineEnd),
            },
        ];
        let (window, _) = editor.current_mut();
        window.selections = SelectionSet::from_parts(selections.clone(), 1).unwrap();
        window.scroll = Scroll { top: 2, left: 1 };
        let pending: KeySequence = "C-x 4".parse().unwrap();
        let frame = Frame {
            x: -10,
            y: 20,
            width: 800,
            height: 600,
        };
        let exported = export(&editor, &pending.0, Some(frame)).unwrap();
        assert_eq!(exported["version"], STATE_VERSION);

        let state = parse(&exported).unwrap();
        assert_eq!(
            state.buffers,
            [
                BufferEntry {
                    name: "*scratch*".into(),
                    path: None
                },
                BufferEntry {
                    name: "a.txt".into(),
                    path: Some("/p/a.txt".into())
                },
            ]
        );
        assert_eq!(state.shown, 1);
        assert_eq!(
            state.selections.iter().copied().collect::<Vec<_>>(),
            selections
        );
        assert_eq!(state.selections.primary().cursor, 10);
        assert_eq!(state.scroll, Scroll { top: 2, left: 1 });
        assert_eq!(state.pending_keys, pending.0);
        assert_eq!(state.frame, Some(frame));
    }

    #[test]
    fn a_state_never_holds_text_secret_or_not() {
        for path in ["/p/.env", "/p/notes.txt"] {
            let state = export(&visiting(path, "SECRET-VALUE\n"), &[], None).unwrap();
            let line = state.to_string();
            assert!(!line.contains("SECRET-VALUE"), "{line}");
            assert!(line.contains(path), "{line}");
        }
    }

    #[test]
    fn text_with_no_file_is_refused_not_lost() {
        let editor = Editor::with_buffer(Buffer::from_text("notes", "unsaved"));
        let message = export(&editor, &[], None).unwrap_err();
        assert!(message.contains("notes"), "{message}");
        assert!(!message.contains("unsaved"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_refused() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let mut editor = Editor::default();
        let path = PathBuf::from(OsStr::from_bytes(b"/p/\xff"));
        let id = editor.add_buffer(Buffer::visiting(path));
        editor.show(id);
        assert!(
            export(&editor, &[], None)
                .unwrap_err()
                .contains("not UTF-8")
        );
    }

    #[test]
    fn a_state_too_long_for_a_line_is_refused() {
        let long = format!("/{}", "a".repeat(super::MAX_STATE));
        let mut editor = Editor::default();
        editor.add_buffer(Buffer::visiting(PathBuf::from(long)));
        let message = export(&editor, &[], None).unwrap_err();
        assert!(message.contains("bytes"), "{message}");
    }

    #[test]
    fn another_version_is_refused() {
        for version in [json!(STATE_VERSION + 1), json!(0), json!("1"), json!(null)] {
            let state = with(|state| state["version"] = version.clone());
            let message = parse(&state).unwrap_err();
            assert!(message.contains("version"), "{message}");
        }
        assert!(parse(&json!([])).is_err());
    }

    #[test]
    fn bad_fields_are_refused() {
        let bad: [(&str, Mutation); 12] = [
            ("buffers", Box::new(|s| s["buffers"] = json!(1))),
            ("path", Box::new(|s| s["buffers"][1]["path"] = json!(1))),
            (
                "absolute",
                Box::new(|s| s["buffers"][1]["path"] = json!("a.txt")),
            ),
            ("id", Box::new(|s| s["buffers"][1]["id"] = json!("x"))),
            ("one window", Box::new(|s| s["windows"] = json!([]))),
            ("one window", Box::new(|s| s["selected"] = json!(1))),
            (
                "does not have",
                Box::new(|s| s["windows"][0]["buffer"] = json!(99)),
            ),
            (
                "primary",
                Box::new(|s| s["windows"][0]["primary"] = json!(5)),
            ),
            (
                "cursor",
                Box::new(|s| s["windows"][0]["selections"][0]["cursor"] = json!(-1)),
            ),
            (
                "goal",
                Box::new(|s| s["windows"][0]["selections"][0]["goal"] = json!(1)),
            ),
            (
                "invalid key",
                Box::new(|s| s["pending_keys"] = json!("C-nope")),
            ),
            (
                "width",
                Box::new(|s| {
                    s["frame"] = json!({ "x": 0, "y": 0, "width": 0, "height": 1 });
                }),
            ),
        ];
        for (expected, mutate) in bad {
            let state = with(|state| mutate(state));
            let message = parse(&state).unwrap_err();
            assert!(message.contains(expected), "{expected}: {message}");
        }
        let many = vec![json!({ "id": 0, "name": "x" }); MAX_BUFFERS + 1];
        let state = with(|state| state["buffers"] = json!(many));
        assert!(parse(&state).unwrap_err().contains("more than"));
    }

    fn restore(cursor: usize, top: usize, editor: &Editor) -> Restore {
        Restore {
            buffer: editor.window().buffer(),
            selections: SelectionSet::new(cursor),
            scroll: Scroll { top, left: 0 },
        }
    }

    #[test]
    fn positions_wait_for_the_file_to_load() {
        let mut editor = Editor::default();
        let id = editor.add_buffer(Buffer::visiting(PathBuf::from("/p/a.txt")));
        editor.show(id);
        let restore = restore(6, 0, &editor);
        assert!(!restore.try_apply(&mut editor), "nothing has loaded");
        assert_eq!(editor.window().selections.primary().cursor, 0);

        let text = "abc\ndef\nghi\n";
        let buffer = editor.buffer_mut(id).unwrap();
        buffer.apply_load(decode_all(text.as_bytes(), TabWidth::default()));
        assert!(restore.try_apply(&mut editor));
        assert_eq!(editor.window().selections.primary().cursor, 6);
    }

    #[test]
    fn positions_move_to_what_exists_in_a_changed_file() {
        let mut editor = visiting("/p/a.txt", "日本\n");
        assert!(restore(1, 40, &editor).try_apply(&mut editor));
        assert_eq!(
            editor.window().selections.primary().cursor,
            0,
            "the start of the character"
        );
        assert_eq!(editor.window().scroll.top, 0, "scrolled back to the cursor");
        assert!(restore(1000, 0, &editor).try_apply(&mut editor));
        assert_eq!(editor.window().selections.primary().cursor, 7, "the end");
    }

    #[test]
    fn positions_are_dropped_if_the_window_moved_on() {
        let mut editor = visiting("/p/a.txt", "abc\n");
        let restore = restore(2, 0, &editor);
        let (scratch, _) = editor.buffers().next().unwrap();
        editor.show(scratch);
        assert!(restore.try_apply(&mut editor));
        assert_eq!(editor.window().selections.primary().cursor, 0);
    }
}
