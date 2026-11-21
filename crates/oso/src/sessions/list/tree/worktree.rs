//! A worktree's rows in `*Spaces*`' tree (ADR 0131, ADR 0140): its row, which begins with its
//! fold and has its sessions' rolled-up mark at the right before its age, and its sessions,
//! hung from it by guides, each called by its title when its name is the worktree's (Q2).
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain data in, text out, as the list.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use oso_core::syntax::Highlight;

use super::super::{
    Entry, Fold, Key, Line, List, PLACE_FROM, Rendered, Row, SPINNER, age, age_of, cells, cut,
    cut_middle, face, mark, order, pad,
};
use super::{INDENT, SpaceRows, Worktree, dir_name, worst};
use crate::sessions::recap::clean;
use crate::sessions::state::State;

/// The guides that hang a session from its worktree's row (ADR 0140): before a session that is
/// not its worktree's last, before its last, and on the rows under one that is not.
const BRANCH: &str = "├ ";
const LAST: &str = "└ ";
const STEM: &str = "│ ";
const CLEAR: &str = "  ";

impl List {
    /// A worktree's row, then its sessions unless it is folded: by default, while none of
    /// oso's runs in it. The row begins with its fold, and its sessions' rolled-up mark is at
    /// the right, before its age (ADR 0140).
    pub(super) fn worktree(
        &self,
        out: &mut Rendered,
        (space, worktree, depth): (&SpaceRows, &Worktree, usize),
        sessions: &[&Entry],
        width: usize,
        now: Instant,
    ) {
        let indent = INDENT[depth];
        let line = Line::Worktree(space.id.clone(), worktree.path.clone());
        let hosted: Vec<&Entry> = sessions
            .iter()
            .copied()
            .filter(|e| matches!(e.key, Key::Hosted(_)))
            .collect();
        let running = hosted.iter().any(|e| e.state != State::Stopped);
        let fold = Fold::Worktree(space.id.clone(), worktree.path.clone());
        let folded = sessions.is_empty() || self.folded_as(&fold, !running);
        let (spin, blink, roll_mark) = self.roll_mark(sessions, &hosted);
        let (name, tag) = names(worktree);
        let count = if folded && !sessions.is_empty() {
            format!("  ({})", sessions.len())
        } else {
            String::new()
        };
        let head = format!("{indent}{} ", if folded { '▸' } else { '▾' });
        let age = self.worktree_age(worktree, sessions, now);
        let right = match roll_mark {
            Some((mark, _)) if age.is_empty() => mark.to_string(),
            Some((mark, _)) => format!("{mark}  {age}"),
            None => age,
        };
        let right = super::super::shown_age(right, width, cells(&head) + cells(&count), &name);
        let room = width.saturating_sub(cells(&head) + cells(&right) + cells(&count) + 1);
        let name = cut_middle(&name, room);
        let mut text = format!("{head}{name}");
        let mut spans = Vec::new();
        let front = self.tree.front_space.as_ref() == Some(&space.id)
            && self.tree.front.as_deref() == Some(worktree.path.as_path());
        if front {
            spans.push((head.len()..text.len(), Highlight::SessionFront));
        }
        let left = room.saturating_sub(cells(&name) + 2);
        if let Some(tag) = tag.filter(|_| left >= PLACE_FROM) {
            text.push_str("  ");
            let at = text.len();
            text.push_str(&cut_middle(super::super::unmarked(&tag), left));
            spans.push((at..text.len(), Highlight::SessionQuiet));
        }
        let at = text.len();
        text.push_str(&count);
        spans.push((at..text.len(), Highlight::SessionQuiet));
        let text = pad(&text, width.saturating_sub(cells(&right))) + &right;
        let right_at = text.len() - right.len();
        spans.push((right_at..text.len(), Highlight::SessionQuiet));
        // Inverse on the lit beat, where its fill is: only while the row is drawn whole.
        let lit = blink == Some(true) && !right.is_empty() && cells(&text) <= width;
        if let Some((mark, face)) = roll_mark.filter(|_| !right.is_empty()) {
            let face = if lit {
                Highlight::SessionAlertLit
            } else {
                face
            };
            spans.push((right_at..right_at + mark.len_utf8(), face));
        }
        if spin.is_some() {
            out.spinning.push(out.lines.len());
        }
        if blink.is_some() && !right.is_empty() {
            out.blinking.push(out.lines.len());
        }
        if let Some((mark, _)) = roll_mark.filter(|_| lit) {
            let at = cells(&text[..right_at]);
            let end = at + cells(mark.encode_utf8(&mut [0; 4]));
            out.bands.lit.push((out.lines.len(), at..end));
        }
        if front {
            out.bands.tint.push(out.lines.len());
        }
        out.push_drawn(&cut(&text, width), line, &spans);
        if folded {
            return;
        }
        out.open_to = 2;
        self.hung(out, (worktree, depth + 1, front), sessions, width, now);
    }

    /// A worktree's rolled-up mark of `sessions`, of which `hosted` are oso's, in its face, and
    /// the spinner's frame it spins at and the beat it blinks on, if it does.
    fn roll_mark(
        &self,
        sessions: &[&Entry],
        hosted: &[&Entry],
    ) -> (Option<usize>, Option<bool>, Option<(char, Highlight)>) {
        let roll = worst(sessions);
        // It spins while its roll-up is a working session of oso's (Q13).
        let spin = self.beat.spin.filter(|_| {
            roll.is_some_and(|e| e.state == State::Working)
                && worst(hosted).is_some_and(|e| e.state == State::Working)
        });
        // Its waiting session's blink, which no spin meets: one needs input, and so leads it.
        let blink = self.beat.roll_blinks(hosted);
        let roll_mark = match (spin, roll) {
            (Some(frame), _) => Some((SPINNER[frame], Highlight::SessionWorking)),
            (None, Some(top)) if matches!(top.key, Key::Hosted(_)) => {
                Some((mark(top.state), face(top.state)))
            }
            (None, Some(top)) => Some((mark(top.state), Highlight::SessionQuiet)),
            (None, None) => None,
        };
        (spin, blink, roll_mark)
    }

    /// The sessions of `worktree`, hung from its row by guides at `depth`, each family's
    /// nested sessions under the session that started them unless that one is folded; on the
    /// front worktree's tint when it is `front`.
    fn hung<'e>(
        &self,
        out: &mut Rendered,
        (worktree, depth, front): (&Worktree, usize, bool),
        sessions: &[&'e Entry],
        width: usize,
        now: Instant,
    ) {
        let families = families(sessions);
        let dir = dir_name(&worktree.path);
        let label_of = |entry: &'e Entry| label(entry, &dir);
        let at = INDENT[depth];
        for (n, (top, nested)) in families.iter().enumerate() {
            let last = n + 1 == families.len();
            let waiting = nested
                .iter()
                .filter(|e| matches!(e.state, State::NeedsInput(_)))
                .count();
            let stem = if last { CLEAR } else { STEM };
            let indent = format!("{at}{}", if last { LAST } else { BRANCH });
            let under = format!("{at}{stem}{}", if nested.is_empty() { CLEAR } else { STEM });
            let label = label_of(top);
            let first = out.lines.len();
            let row = Row {
                under: Some(&under),
                label,
                ..Row::new(top, &indent, self.beat, waiting)
            };
            row.render(out, width, now);
            if !self.folded(&Fold::Nested(top.key)) {
                for (m, entry) in nested.iter().enumerate() {
                    let end = m + 1 == nested.len();
                    let indent = format!("{at}{stem}{}", if end { LAST } else { BRANCH });
                    let under = format!("{at}{stem}{}{CLEAR}", if end { CLEAR } else { STEM });
                    let label = label_of(entry);
                    let row = Row {
                        under: Some(&under),
                        label,
                        ..Row::new(entry, &indent, self.beat, 0)
                    };
                    row.render(out, width, now);
                }
            }
            if front {
                out.bands.tint.extend(first..out.lines.len());
            }
        }
    }

    /// A worktree's age: its newest session's, else since git last changed it.
    fn worktree_age(&self, worktree: &Worktree, sessions: &[&Entry], now: Instant) -> String {
        match sessions.iter().map(|e| e.since).max() {
            Some(since) => age(since, now),
            None => self
                .tree
                .wall
                .zip(worktree.active)
                .and_then(|(wall, active)| wall.duration_since(active).ok())
                // `now` under a minute, as a session's age (ADR 0131, Q6).
                .map(|ago| {
                    if ago.as_secs() < 60 {
                        "now".to_owned()
                    } else {
                        age_of(ago)
                    }
                })
                .unwrap_or_default(),
        }
    }
}

/// What session `entry` is called on its row under the worktree whose directory is `dir`: its
/// title when its name is the worktree's (ADR 0140, Q2), else `None`, its name. Only oso's own
/// sessions: another program's has no title it could take a name from.
fn label<'e>(entry: &'e Entry, dir: &str) -> Option<&'e str> {
    let named_after = matches!(entry.key, Key::Hosted(_)) && entry.name == dir;
    (named_after && !entry.title.is_empty()).then_some(entry.title.as_str())
}

/// What a worktree's row names: the main checkout's branch, marked `primary`, or a linked
/// worktree's directory, with its branch where that differs.
pub(super) fn names(worktree: &Worktree) -> (String, Option<String>) {
    let dir = dir_name(&worktree.path);
    let branch = worktree.branch.as_deref().map(clean);
    let pull = worktree.pull.as_deref().map(clean);
    let with_pull = |tag: Option<String>| match (tag, &pull) {
        (Some(tag), Some(pull)) => Some(format!("{tag}, {pull}")),
        (tag, None) => tag,
        (None, Some(pull)) => Some(pull.clone()),
    };
    if worktree.primary {
        return (branch.unwrap_or(dir), with_pull(Some("primary".to_owned())));
    }
    let tag = branch.filter(|branch| *branch != dir);
    (dir, with_pull(tag))
}

/// `sessions` as families: each top-level one in order, oso's own first, with the sessions
/// nested under it among them. One whose chain of parents runs in a circle stays at the top.
fn families<'e>(sessions: &[&'e Entry]) -> Vec<(&'e Entry, Vec<&'e Entry>)> {
    let by_key: HashMap<Key, &Entry> = sessions.iter().map(|e| (e.key, *e)).collect();
    let top = |entry: &'e Entry| -> Key {
        let mut at = entry;
        let mut seen = HashSet::from([entry.key]);
        while let Some(parent) = at.parent.and_then(|key| by_key.get(&key)) {
            if !seen.insert(parent.key) {
                return entry.key;
            }
            at = parent;
        }
        at.key
    };
    let mut sorted = sessions.to_vec();
    sorted.sort_by(|a, b| {
        let external = |e: &Entry| matches!(e.key, Key::External(_));
        (external(a), order(a)).cmp(&(external(b), order(b)))
    });
    let mut families: Vec<(&Entry, Vec<&Entry>)> = Vec::new();
    for &entry in &sorted {
        if top(entry) == entry.key {
            families.push((entry, Vec::new()));
        }
    }
    for &entry in &sorted {
        let top = top(entry);
        if top != entry.key
            && let Some((_, nested)) = families.iter_mut().find(|(e, _)| e.key == top)
        {
            nested.push(entry);
        }
    }
    families
}

#[cfg(test)]
#[test]
fn the_guides_drawn_are_those_a_row_is_read_past() {
    for guide in [BRANCH, LAST, STEM] {
        let first = guide.chars().next().unwrap();
        assert!(super::super::GUIDES.contains(first), "{guide:?}");
    }
}
