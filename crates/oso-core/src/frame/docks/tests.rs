//! Docked windows' rows inside their host (ADR 0170).
//!
//! Must not depend on: the editor; plain layouts of one window.

use super::{DockAsk, HOST_LEAST};
use crate::frame::{FrameLayout, FrameSize, Part, Splits, layout_into, single};
use crate::window::WindowId;

/// One window of `rows` rows of text and a modeline, 80 columns, with a gutter of 4.
fn host(rows: usize) -> FrameLayout {
    single(
        FrameSize {
            rows: rows + 2,
            columns: 80,
        },
        4,
    )
}

const HOST: WindowId = WindowId::new(0);

/// A strip that wants `strip` rows, a card `card`, and a composer `composer`, as the editor
/// asks: the composer's least is one, and its rows go first.
fn asks(strip: usize, card: usize, composer: usize) -> [DockAsk; 3] {
    [
        DockAsk {
            window: WindowId::new(1),
            rows: strip,
            least: 0,
            rank: 2,
            gutter: 0,
            padded: false,
        },
        DockAsk {
            window: WindowId::new(2),
            rows: card,
            least: 0,
            rank: 1,
            gutter: 0,
            padded: false,
        },
        DockAsk {
            window: WindowId::new(3),
            rows: composer,
            least: 1,
            rank: 0,
            gutter: 2,
            padded: false,
        },
    ]
}

fn rows(frame: &FrameLayout, id: u64) -> usize {
    frame
        .regions(WindowId::new(id))
        .map_or(0, |regions| regions.text.rows)
}

#[test]
fn docks_take_rows_from_the_hosts_bottom_above_its_modeline() {
    let mut frame = host(30);
    let modeline = frame.regions(HOST).unwrap().modeline;
    assert!(frame.dock(HOST, &asks(1, 5, 3), false));
    let host = *frame.regions(HOST).unwrap();
    assert_eq!(host.text.rows, 30 - 9);
    assert_eq!(
        host.modeline, modeline,
        "the modeline stays the host's last row"
    );
    let ids: Vec<u64> = frame.windows().iter().map(|(id, _)| id.get()).collect();
    assert_eq!(ids, [0, 1, 2, 3], "each after its host, top to bottom");
    let composer = *frame.regions(WindowId::new(3)).unwrap();
    assert_eq!(composer.text.row + composer.text.rows, modeline.row);
    assert_eq!(composer.gutter.columns, 2);
    assert_eq!(composer.text.column, host.gutter.column + 2);
    assert_eq!(
        composer.text.columns + 2,
        host.gutter.columns + host.text.columns
    );
    assert_eq!(composer.modeline.rows, 0);
    assert!(frame.docks_hidden().is_empty());
}

#[test]
fn a_short_host_keeps_its_least_and_the_composer_its_rows_first() {
    // 2 rows for the host, then 8: the composer's 3, the card's 5, and none for the strip.
    let mut frame = host(10);
    assert!(frame.dock(HOST, &asks(1, 6, 3), false));
    assert_eq!(frame.regions(HOST).unwrap().text.rows, HOST_LEAST);
    assert_eq!(
        (rows(&frame, 1), rows(&frame, 2), rows(&frame, 3)),
        (0, 5, 3)
    );
    assert!(!frame.regions(WindowId::new(1)).unwrap().has_cells());
}

#[test]
fn a_host_too_short_hides_its_docks_and_says_so() {
    let mut frame = host(HOST_LEAST);
    assert!(!frame.dock(HOST, &asks(0, 0, 1), false));
    assert_eq!(frame.docks_hidden(), [HOST]);
    assert_eq!(frame.regions(HOST).unwrap().text.rows, HOST_LEAST);
    assert!(frame.regions(WindowId::new(3)).is_none());
}

/// The asks of [`asks`] with the composer padded.
fn padded(strip: usize, card: usize, composer: usize) -> [DockAsk; 3] {
    let mut asks = asks(strip, card, composer);
    asks[2].padded = true;
    asks
}

#[test]
fn a_padded_dock_has_a_row_above_and_below_its_text_and_the_footer_is_under_all() {
    let mut frame = host(30);
    let modeline = frame.regions(HOST).unwrap().modeline;
    assert!(frame.dock(HOST, &padded(1, 0, 1), true));
    let host = *frame.regions(HOST).unwrap();
    assert_eq!(host.text.rows, 30 - 5, "a strip, and four for the composer");
    let composer = *frame.regions(WindowId::new(3)).unwrap();
    let [above, below] = frame.padding(WindowId::new(3)).unwrap();
    let footer = frame.footer(HOST).unwrap();
    let strip = frame.regions(WindowId::new(1)).unwrap().text;
    let tops = [strip, above, composer.text, below, footer, modeline].map(|rect| rect.row);
    let first = host.text.row + host.text.rows;
    assert_eq!(tops, [0, 1, 2, 3, 4, 5].map(|row| first + row));
    for rect in [above, below, footer] {
        assert_eq!(rect.rows, 1);
        assert_eq!((rect.column, rect.columns), (0, 80), "its gutter among it");
    }
    assert_eq!(
        (composer.gutter.row, composer.gutter.rows),
        (composer.text.row, 1)
    );
    assert_eq!(frame.padding(WindowId::new(1)), None);
    // The rows are whose they are, for the pointer.
    let hit = |row| frame.hit(row, 40).map(|hit| (hit.window.get(), hit.part));
    assert_eq!(hit(above.row), Some((3, Part::Padding)));
    assert_eq!(hit(below.row), Some((3, Part::Padding)));
    assert_eq!(hit(footer.row), Some((0, Part::Footer)));
    // Laid out again, with no docks, none is left over.
    let (size, splits) = (frame.size(), Splits::new(HOST));
    layout_into(&mut frame, size, (&splits, None), HOST, |_| 4);
    assert_eq!(
        (frame.footer(HOST), frame.padding(WindowId::new(3))),
        (None, None)
    );
}

/// From the tallest host down, the footer goes first, then the padding, both its rows, then the
/// strip and the card; each is there again in every taller one.
#[test]
fn a_short_host_gives_up_the_footer_then_the_padding_then_the_rest() {
    let mut seen = Vec::new();
    for rows in (0..20).rev() {
        let mut frame = host(rows);
        let docked = frame.dock(HOST, &padded(1, 3, 2), true);
        let text = |id: u64| frame.regions(WindowId::new(id)).map_or(0, |r| r.text.rows);
        let has = (
            docked,
            frame.footer(HOST).is_some(),
            frame.padding(WindowId::new(3)).is_some(),
            text(1),
            text(2),
            text(3),
        );
        if docked {
            let around = 2 * usize::from(has.2) + usize::from(has.1);
            let total = text(0) + text(1) + text(2) + text(3) + around;
            assert_eq!(total, rows, "every row is one's");
            assert!(text(0) >= HOST_LEAST);
        }
        seen.push(has);
    }
    seen.dedup();
    assert_eq!(
        seen,
        [
            (true, true, true, 1, 3, 2),
            (true, false, true, 1, 3, 2),
            (true, false, false, 1, 3, 2),
            (true, false, false, 0, 3, 2),
            (true, false, false, 0, 2, 2),
            (true, false, false, 0, 1, 2),
            (true, false, false, 0, 0, 2),
            (true, false, false, 0, 0, 1),
            (false, false, false, 0, 0, 0),
        ]
    );
}

#[test]
fn a_footer_is_under_docks_that_have_no_padding_and_under_none() {
    let mut frame = host(10);
    // A request's line, with the composer away.
    assert!(frame.dock(HOST, &asks(0, 2, 1)[1..2], true));
    let line = frame.regions(WindowId::new(2)).unwrap().text;
    assert_eq!(frame.footer(HOST).unwrap().row, line.row + line.rows);
    let mut bare = host(10);
    assert!(!bare.dock(HOST, &[], true));
    assert_eq!(bare.footer(HOST), None);
}
