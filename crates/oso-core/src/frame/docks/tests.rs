//! Docked windows' rows inside their host (ADR 0170).
//!
//! Must not depend on: the editor; plain layouts of one window.

use super::{DockAsk, HOST_LEAST};
use crate::frame::{FrameLayout, FrameSize, single};
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

/// A card that wants `card` rows, a strip `strip`, and a composer `composer`, as the editor
/// asks: the composer's least is one, and its rows go first.
fn asks(card: usize, strip: usize, composer: usize) -> [DockAsk; 3] {
    [
        DockAsk {
            window: WindowId::new(1),
            rows: card,
            least: 0,
            rank: 1,
            gutter: 0,
        },
        DockAsk {
            window: WindowId::new(2),
            rows: strip,
            least: 0,
            rank: 2,
            gutter: 0,
        },
        DockAsk {
            window: WindowId::new(3),
            rows: composer,
            least: 1,
            rank: 0,
            gutter: 2,
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
    assert!(frame.dock(HOST, &asks(5, 1, 3)));
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
    assert!(frame.dock(HOST, &asks(6, 1, 3)));
    assert_eq!(frame.regions(HOST).unwrap().text.rows, HOST_LEAST);
    assert_eq!(
        (rows(&frame, 1), rows(&frame, 2), rows(&frame, 3)),
        (5, 0, 3)
    );
    assert!(!frame.regions(WindowId::new(2)).unwrap().has_cells());
}

#[test]
fn a_host_too_short_hides_its_docks_and_says_so() {
    let mut frame = host(HOST_LEAST);
    assert!(!frame.dock(HOST, &asks(0, 0, 1)));
    assert_eq!(frame.docks_hidden(), [HOST]);
    assert_eq!(frame.regions(HOST).unwrap().text.rows, HOST_LEAST);
    assert!(frame.regions(WindowId::new(3)).is_none());
}
