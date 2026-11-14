//! How a run's cells are shared among its children, by their weights and what each needs,
//! and how a window's cells become its regions (ADR 0057, ADR 0112).
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type; these are pure
//! functions of cells.

use super::super::{CellRect, FrameSize, WindowRegions, fitting_gutter};

/// A window's regions in `area`: a divider at its right while a window is to its right, its
/// modeline at its bottom, and its gutter at the left of the text while the text keeps a
/// column. A window that has all the tiled area lays out as the one window always has
/// (ADR 0057).
pub(in crate::frame) fn window_regions(
    area: CellRect,
    right: bool,
    gutter: usize,
) -> WindowRegions {
    let divider = (right && area.columns >= 2).then(|| CellRect {
        column: area.column + area.columns - 1,
        columns: 1,
        ..area
    });
    let columns = area.columns - usize::from(divider.is_some());
    let modeline_rows = area.rows.saturating_sub(1).min(1);
    let text_rows = area.rows - modeline_rows;
    let gutter = fitting_gutter(
        FrameSize {
            rows: area.rows,
            columns,
        },
        gutter,
    );
    let text = CellRect {
        rows: text_rows,
        ..area
    };
    WindowRegions {
        gutter: CellRect {
            columns: gutter,
            ..text
        },
        text: CellRect {
            column: area.column + gutter,
            columns: columns - gutter,
            ..text
        },
        modeline: CellRect {
            row: area.row + text_rows,
            rows: modeline_rows,
            columns,
            ..area
        },
        divider,
    }
}

/// Divides `total` cells among children of `weights` by the largest remainder, each getting
/// at least what it `needs`. When their needs do not fit, the last children get none, but
/// `kept`, which takes all there is when even its own needs do not fit.
pub(super) fn distribute(
    total: usize,
    weights: &[usize],
    needs: &[usize],
    kept: usize,
) -> Vec<usize> {
    let mut shown = vec![true; weights.len()];
    let mut needed: usize = needs.iter().sum();
    for i in (0..weights.len()).rev() {
        if needed <= total {
            break;
        }
        if i != kept {
            shown[i] = false;
            needed -= needs[i];
        }
    }
    let mut cells = vec![0; weights.len()];
    // Children held at their needs, which their share would not give them.
    let mut held = vec![false; weights.len()];
    loop {
        let free: Vec<usize> = (0..weights.len())
            .filter(|&i| shown[i] && !held[i])
            .collect();
        let fixed: usize = (0..weights.len())
            .filter(|&i| shown[i] && held[i])
            .map(|i| needs[i])
            .sum();
        let left = total.saturating_sub(fixed);
        let shares = largest_remainder(left, free.iter().map(|&i| weights[i]));
        let mut changed = false;
        for (&i, share) in free.iter().zip(shares) {
            cells[i] = share;
            if share < needs[i] && needed <= total {
                held[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for i in 0..weights.len() {
        if shown[i] && held[i] {
            cells[i] = needs[i];
        }
    }
    cells
}

/// `total` divided in proportion to `weights`, the cells left over by rounding down going
/// one each to the largest remainders, the earlier first on a tie.
pub(super) fn largest_remainder(
    total: usize,
    weights: impl Iterator<Item = usize> + Clone,
) -> Vec<usize> {
    let sum: u128 = weights.clone().map(|w| w as u128).sum();
    if sum == 0 {
        return weights.map(|_| 0).collect();
    }
    let total_wide = total as u128;
    let mut shares: Vec<(usize, u128)> = weights
        .map(|w| {
            let exact = total_wide * w as u128;
            ((exact / sum) as usize, exact % sum)
        })
        .collect();
    let given: usize = shares.iter().map(|(cells, _)| cells).sum();
    let mut order: Vec<usize> = (0..shares.len()).collect();
    order.sort_by(|&a, &b| shares[b].1.cmp(&shares[a].1).then(a.cmp(&b)));
    for &i in order.iter().take(total - given) {
        shares[i].0 += 1;
    }
    shares.into_iter().map(|(cells, _)| cells).collect()
}
