//! The windows on native sessions' transcripts that a restart shows again (ADR 0165, part 4c-2):
//! each such session by its session id, with the windows that showed it, by their place among the
//! state's windows, and whether its composer was selected under the state's selected one.
//! Nothing else of a transcript is carried: no row, text or position.
//!
//! Must not depend on: `app` or `sessions`; these are values and the editor's windows. The new
//! instance shows only what its own record has as a native session.
//!
//! A window's own entry in the state says what is behind the transcript, as for every internal
//! buffer (ADR 0063), so that is what shows where the transcript does not come back, and what a
//! reader from before this shows. An entry that is not one this writes is left out, never
//! refusing the restart; the field is left out where it would not fit.

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::Editor;
use oso_core::window::{DockSlot, Window};
use serde_json::{Map, Value, json};

use super::layout::MAX_WINDOWS;

/// A native session whose transcript the state's windows may show.
#[derive(Debug, Clone)]
pub struct Shown {
    /// Its session id.
    pub id: String,
    pub transcript: BufferId,
}

/// A native session's transcript to show again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeWindows {
    /// Its session id: 36 ASCII hexadecimal digits and dashes.
    pub id: String,
    /// The windows that showed it, by their place among the state's, each named once in a state.
    pub windows: Vec<usize>,
    /// Its docked composer was selected.
    pub composer: bool,
}

/// The entries of `shown` that the tiled windows show, in the state's order.
fn entries(editor: &Editor, shown: &[Shown]) -> Vec<NativeWindows> {
    let tiled = super::tiled_windows(editor);
    let (selected, selected_buffer) = editor.tiled();
    (shown.iter())
        .filter_map(|session| {
            let shows = |window: &Window| window.buffer() == session.transcript;
            let windows: Vec<usize> = (0..tiled.len()).filter(|at| shows(tiled[*at])).collect();
            (!windows.is_empty()).then(|| NativeWindows {
                id: session.id.clone(),
                windows,
                composer: selected.docked() == Some((session.transcript, DockSlot::Composer))
                    && selected_buffer.mode() == Mode::Composer,
            })
        })
        .collect()
}

/// Adds to the exported `state` the sessions of `shown` that `editor`'s windows show, unless they
/// would not fit: their windows then come back on what is behind them.
pub fn add(state: &mut Value, editor: &Editor, shown: &[Shown]) {
    let entries = entries(editor, shown);
    if entries.is_empty() {
        return;
    }
    state["native_windows"] = (entries.iter())
        .map(
            |entry| json!({ "id": entry.id, "windows": entry.windows, "composer": entry.composer }),
        )
        .collect();
    if super::fits(state).is_err() {
        log::info!("the native sessions the windows show are left out of the restart's state");
        if let Some(state) = state.as_object_mut() {
            state.remove("native_windows");
        }
    }
}

/// The sessions `state` shows again in its `windows` windows: none if it says none. An entry that
/// does not read is left out, and so is a place that names no window or one named already.
pub(super) fn parse(state: &Map<String, Value>, windows: usize) -> Vec<NativeWindows> {
    let Some(entries) = state.get("native_windows").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut taken = vec![false; windows];
    (entries.iter())
        .take(MAX_WINDOWS)
        .filter_map(|entry| {
            let id = super::natives::session_id(entry.get("id")?)?;
            let windows: Vec<usize> = (entry.get("windows")?.as_array()?.iter())
                .take(MAX_WINDOWS)
                .filter_map(|at| usize::try_from(at.as_u64()?).ok())
                .filter(|at| {
                    (taken.get_mut(*at)).is_some_and(|taken| !std::mem::replace(taken, true))
                })
                .collect();
            (!windows.is_empty()).then(|| NativeWindows {
                id,
                windows,
                composer: entry.get("composer").and_then(Value::as_bool) == Some(true),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Buffer;
    use oso_core::frame::{FrameSize, Way};
    use serde_json::json;

    use super::*;
    use crate::carry::{self, export_value, tests::absolute, tests::visiting};

    const ID: &str = "0123abcd-0000-4000-8000-000000000001";
    const OTHER: &str = "0123abcd-0000-4000-8000-000000000002";

    /// A frame of three windows, the left one on a file, and of the right ones the lower selected
    /// and on a new transcript of some text, with a composer docked under it.
    fn split() -> (Editor, BufferId) {
        let mut editor = visiting(&absolute("/p/a.txt"), "text\n");
        editor.set_frame_size(FrameSize {
            rows: 40,
            columns: 120,
        });
        editor.split_window(Way::Across, true).unwrap();
        editor.split_window(Way::Down, true).unwrap();
        let transcript = transcript(&mut editor, "s");
        editor.show(transcript);
        (editor, transcript)
    }

    fn transcript(editor: &mut Editor, name: &str) -> BufferId {
        let transcript = editor.add_native_session_buffer(format!("*claude: {name}*"));
        editor.replace_native_rows(transcript, 0..0, "a row no state quotes\n", &[]);
        let mut composer = Buffer::from_text(format!("*compose: {name}*"), "");
        composer.set_mode(Mode::Composer);
        let composer = editor.add_buffer(composer);
        editor.set_dock(transcript, DockSlot::Composer, Some(composer));
        transcript
    }

    fn shown(id: &str, transcript: BufferId) -> Shown {
        Shown {
            id: id.to_owned(),
            transcript,
        }
    }

    fn entry(id: &str, windows: &[usize], composer: bool) -> NativeWindows {
        NativeWindows {
            id: id.to_owned(),
            windows: windows.to_vec(),
            composer,
        }
    }

    /// `editor`'s state with `shown`, written and read back.
    fn carried(editor: &Editor, shown: &[Shown]) -> carry::State {
        let mut state = export_value(editor, &[], None, None).unwrap();
        add(&mut state, editor, shown);
        let text = state.to_string();
        assert!(!text.contains("no state quotes") && !text.contains("*claude"));
        carry::parse(&state).unwrap()
    }

    #[test]
    fn the_windows_on_a_transcript_go_by_its_session_and_their_places() {
        let (editor, transcript) = split();
        let state = carried(&editor, &[shown(ID, transcript)]);
        assert_eq!(state.native_windows, [entry(ID, &[2], false)]);
        // What is behind it is what its window's own entry shows.
        let paths = state.paths();
        assert_eq!(paths[state.windows.windows[2].shown], None, "*scratch*");
        // No session whose transcript no window shows, and an older reader knows no field.
        assert_eq!(carried(&editor, &[]).native_windows, []);
    }

    #[test]
    fn two_windows_on_one_transcript_and_two_sessions_go_each_once() {
        let (mut editor, first) = split();
        let second = transcript(&mut editor, "t");
        let windows: Vec<_> = (carry::tiled_windows(&editor).iter())
            .map(|window| window.id())
            .collect();
        editor.show_in(windows[0], second);
        editor.show_in(windows[1], first);
        let state = carried(&editor, &[shown(ID, first), shown(OTHER, second)]);
        assert_eq!(
            state.native_windows,
            [entry(ID, &[1, 2], false), entry(OTHER, &[0], false)]
        );
    }

    #[test]
    fn a_selected_composer_is_named_with_its_transcripts_window_selected() {
        let (mut editor, transcript) = split();
        let composer = editor.dock_window(transcript, DockSlot::Composer).unwrap();
        editor.select_window(composer);
        let state = carried(&editor, &[shown(ID, transcript)]);
        assert_eq!(state.native_windows, [entry(ID, &[2], true)]);
        assert_eq!(state.windows.selected, 2, "the window it is docked under");
        // What the place holds while it is not the composer is not one.
        let other = editor.add_internal_buffer("*request*", Mode::Sessions);
        editor.set_dock(transcript, DockSlot::Composer, Some(other));
        let state = carried(&editor, &[shown(ID, transcript)]);
        assert_eq!(state.native_windows, [entry(ID, &[2], false)]);
    }

    fn parsed(field: Value, windows: usize) -> Vec<NativeWindows> {
        let state = json!({ "native_windows": field });
        parse(state.as_object().unwrap(), windows)
    }

    #[test]
    fn what_does_not_read_is_left_out_and_the_rest_stay() {
        let good = json!({ "id": OTHER, "windows": [1] });
        for bad in [
            json!("an id"),
            json!({ "id": ID }),
            json!({ "id": "short", "windows": [0] }),
            json!({ "id": "0123abcd-0000-4000-8000-00000000000g", "windows": [0] }),
            json!({ "id": 7, "windows": [0] }),
            json!({ "id": ID, "windows": [] }),
            json!({ "id": ID, "windows": 0 }),
            json!({ "id": ID, "windows": [-1, 2, "0", null] }),
        ] {
            assert_eq!(
                parsed(json!([bad, good]), 2),
                [entry(OTHER, &[1], false)],
                "{bad} is left out"
            );
        }
        let odd = json!({ "id": ID, "windows": [5, 0, 0], "composer": "yes" });
        assert_eq!(
            parsed(json!([odd, { "id": OTHER, "windows": [0, 1] }]), 2),
            [entry(ID, &[0], false), entry(OTHER, &[1], false)],
            "a window is named once, by the first"
        );
        for none in [json!(null), json!({}), json!("x"), json!(7)] {
            assert_eq!(parsed(none, 2), []);
        }
        assert_eq!(parse(json!({}).as_object().unwrap(), 2), []);
    }

    #[test]
    fn one_that_does_not_fit_is_left_out_and_the_restart_goes() {
        let (editor, transcript) = split();
        let mut state = export_value(&editor, &[], None, None).unwrap();
        let room = carry::MAX_STATE - state.to_string().len() - 20 - r#","filler":"""#.len();
        state["filler"] = json!("x".repeat(room));
        add(&mut state, &editor, &[shown(ID, transcript)]);
        assert!(state.get("native_windows").is_none());
        assert!(carry::fits(&state).is_ok());
    }
}
