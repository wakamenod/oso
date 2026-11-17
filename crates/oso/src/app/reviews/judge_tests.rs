//! The width's judge on a clock of its own: the 160 and 150 column rules, a change judged once
//! it settled, judging nothing while the width stays, and a choice judged at once.
//!
//! Must not depend on: the editor's windows, the frame or the real clock.

use std::time::{Duration, Instant};

use oso_core::editor::{ReviewChoice, ReviewLayout};
use oso_core::frame::Way;
use oso_core::window::WindowId;

use super::{Judge, SETTLE};

const AUTO: ReviewChoice = ReviewChoice::Auto;
const UNIFIED: ReviewLayout = ReviewLayout::Unified;
const ACROSS: ReviewLayout = ReviewLayout::Panes(Way::Across);
const DOWN: ReviewLayout = ReviewLayout::Panes(Way::Down);

fn window() -> WindowId {
    oso_core::editor::Editor::default().window().id()
}

/// A judge that saw the area at `columns` at `at`.
fn seen(judge: &mut Judge, columns: usize, at: Instant) {
    judge.see([(window(), columns)], at);
}

#[test]
fn automatic_is_side_by_side_from_160_unified_below_150_and_keeps_between() {
    let start = Instant::now();
    for (columns, shown, want) in [
        (160, UNIFIED, ACROSS),
        (159, UNIFIED, UNIFIED),
        (150, ACROSS, ACROSS),
        (155, UNIFIED, UNIFIED),
        (149, ACROSS, UNIFIED),
    ] {
        let mut judge = Judge::default();
        seen(&mut judge, columns, start);
        let judged = judge.want(window(), AUTO, shown, start);
        assert_eq!(judged, Some(want), "{columns} showing {shown:?}");
    }
}

#[test]
fn two_panes_chosen_go_stacked_below_150_and_keep_their_way_between() {
    let start = Instant::now();
    for (columns, shown, want) in [
        (160, UNIFIED, ACROSS),
        (149, ACROSS, DOWN),
        (155, ACROSS, ACROSS),
        (155, DOWN, DOWN),
        (155, UNIFIED, DOWN),
    ] {
        let mut judge = Judge::default();
        seen(&mut judge, columns, start);
        let judged = judge.want(window(), ReviewChoice::Panes, shown, start);
        assert_eq!(judged, Some(want), "{columns} showing {shown:?}");
    }
}

#[test]
fn a_width_is_judged_only_once_it_stayed_the_same_for_the_settle() {
    let start = Instant::now();
    let mut judge = Judge::default();
    seen(&mut judge, 120, start);
    assert_eq!(judge.want(window(), AUTO, UNIFIED, start), Some(UNIFIED));
    assert_eq!(judge.deadline(), None, "nothing settles");
    // A drag past 160 and back and past again: judged once, at the width it rests at.
    let mut at = start;
    for columns in [140, 170, 158, 180] {
        at += Duration::from_millis(30);
        seen(&mut judge, columns, at);
        assert_eq!(judge.want(window(), AUTO, UNIFIED, at), Some(UNIFIED));
    }
    assert_eq!(judge.deadline(), Some(at + SETTLE));
    let rest = at + SETTLE;
    seen(&mut judge, 180, rest);
    assert_eq!(judge.want(window(), AUTO, UNIFIED, rest), Some(ACROSS));
    assert_eq!(judge.deadline(), None);
    // The same width later, as after scrolling: nothing is judged again.
    assert_eq!(
        judge.want(window(), AUTO, ACROSS, rest + SETTLE),
        Some(ACROSS)
    );
}

#[test]
fn a_choice_is_judged_at_once_and_an_area_gone_is_forgotten() {
    let start = Instant::now();
    let mut judge = Judge::default();
    seen(&mut judge, 120, start);
    assert_eq!(judge.want(window(), AUTO, UNIFIED, start), Some(UNIFIED));
    let soon = start + Duration::from_millis(1);
    assert_eq!(
        judge.want(window(), ReviewChoice::Panes, UNIFIED, soon),
        Some(DOWN)
    );
    assert_eq!(
        judge.want(window(), ReviewChoice::Unified, DOWN, soon),
        Some(UNIFIED)
    );
    judge.see([], soon);
    assert_eq!(judge.wanted(window()), None);
    assert_eq!(judge.want(window(), AUTO, UNIFIED, soon), None);
}
