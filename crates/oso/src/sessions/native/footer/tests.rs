//! A footer's words, and what it leaves out in a narrow window.
//!
//! Must not depend on: the app or how a frame is drawn.

use oso_core::minibuffer::cells_of;

use super::{Footer, Said, Tone};
use crate::sessions::native::wire::status_line::{Kind, readable};

const KEY: &str = " (S-TAB to cycle)";

/// What a running process said.
fn said(mode: Option<&str>) -> Said<'_> {
    Said {
        runs: true,
        mode,
        ..Said::default()
    }
}

fn footer(mode: Option<&str>) -> Footer {
    Footer::of(said(mode), "Opus 5.5")
}

/// The footer's text at `columns`, its model at the right edge.
fn line(footer: &Footer, columns: usize) -> String {
    let parts = footer.parts(columns, KEY);
    let left = format!("{}{}{}", parts.mode, parts.strayed, parts.then);
    let gap = columns.saturating_sub(cells_of(&left) + parts.model_cells);
    format!("{left}{}{}", " ".repeat(gap), parts.model)
        .trim_end()
        .to_owned()
}

#[test]
fn each_mode_says_itself_as_the_terminal_does() {
    for (mode, tone, words) in [
        ("default", Tone::Quiet, "⏸ manual mode on"),
        ("acceptEdits", Tone::Edits, "▶▶ accept edits on"),
        ("plan", Tone::Plan, "⏸ plan mode on"),
        ("auto", Tone::Auto, "▶▶ auto mode on"),
        ("bypassPermissions", Tone::Alarm, "▶▶ bypass permissions on"),
        ("dontAsk", Tone::Alarm, "▶▶ don't ask on"),
        ("Auto", Tone::Alarm, "⚠ mode Auto"),
        ("manual", Tone::Alarm, "⚠ mode manual"),
    ] {
        let footer = footer(Some(mode));
        let parts = footer.parts(80, "");
        assert_eq!((parts.tone, parts.mode), (tone, words));
    }
    let long = footer(Some(&"x".repeat(200)));
    assert!(cells_of(long.parts(80, "").mode) <= 7 + 24);
}

#[test]
fn no_mode_said_is_quiet_and_one_that_cannot_be_shown_is_not() {
    let unknown = footer(None);
    let parts = unknown.parts(80, KEY);
    assert_eq!((parts.tone, parts.mode), (Tone::Quiet, "mode not known"));
    let unshown = Said {
        unshown: true,
        strayed: true,
        ..said(None)
    };
    let unshown = Footer::of(unshown, "");
    assert_eq!(unshown.parts(80, KEY).tone, Tone::Alarm);
    assert_eq!(
        line(&unshown, 80),
        "⚠ mode not shown · set by no key here (S-TAB to cycle)"
    );
}

#[test]
fn with_no_process_it_says_the_model_alone() {
    let ended = Said {
        runs: false,
        strayed: true,
        changing: true,
        unshown: true,
        ..said(Some("auto"))
    };
    let ended = Footer::of(ended, "Opus 5.5");
    assert_eq!(ended.parts(30, KEY).tone, Tone::Quiet);
    let only = format!(
        "{}Opus 5.5",
        " ".repeat(" · set by no key here".chars().count())
    );
    assert_ne!(line(&ended, 30), only, "no word of a mode");
    assert_eq!(line(&ended, 30), format!("{}Opus 5.5", " ".repeat(22)));
}

#[test]
fn a_change_under_way_is_said_in_the_keys_place() {
    let changing = Said {
        changing: true,
        ..said(Some("plan"))
    };
    let changing = Footer::of(changing, "");
    assert_eq!(line(&changing, 80), "⏸ plan mode on (changing…)");
}

/// The key goes first, then the model, then where the mode was set; the mode's words stay whole.
#[test]
fn a_narrow_footer_leaves_out_whole_parts_and_never_the_mode() {
    let strayed = Said {
        strayed: true,
        ..said(Some("auto"))
    };
    for model in ["Opus 5.5 high", &"m".repeat(200)] {
        let footer = Footer::of(strayed, model);
        let mut seen = Vec::new();
        for columns in (0..120).rev() {
            let parts = footer.parts(columns, KEY);
            assert_eq!(parts.mode, "▶▶ auto mode on");
            let kept = (
                !parts.then.is_empty(),
                !parts.model.is_empty(),
                !parts.strayed.is_empty(),
            );
            let left = cells_of(parts.mode) + cells_of(parts.strayed) + cells_of(parts.then);
            let none = kept == (false, false, false);
            assert!(left + parts.model_cells <= columns || none, "{columns}");
            // Where it was set is there wherever the mode and it fit, whatever the model is.
            assert_eq!(kept.2, columns >= 15 + 21, "{columns}");
            seen.push(kept);
        }
        seen.dedup();
        assert_eq!(
            seen,
            [
                (true, true, true),
                (false, true, true),
                (false, false, true),
                (false, false, false)
            ]
        );
    }
    // What a window of 44 columns has.
    let usual = footer(Some("auto"));
    assert_eq!(
        line(&usual, 41),
        "▶▶ auto mode on (S-TAB to cycle) Opus 5.5"
    );
}

/// What the stream gives the model cannot read as a mode: it holds none of their marks.
#[test]
fn a_models_name_holds_no_mark_of_a_mode() {
    for mark in ['⏸', '▶', '⚠', '·', '…'] {
        let name = format!("{mark} auto mode on");
        assert!(!readable(&name, Kind::Label) && !readable(&name, Kind::Id));
    }
    let long = Footer::of(said(Some("plan")), &"m".repeat(200));
    let parts = long.parts(200, KEY);
    assert_eq!((cells_of(parts.model), parts.model_cells), (24, 24));
    assert!(parts.model.ends_with('…'));
}
