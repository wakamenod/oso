//! The space level of `*Spaces*`' tree (ADR 0136, step 2): a space's row with its roll-up,
//! its repositories' rows when it has several, the worktrees of each, and `Archived spaces`.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain data in, text out, as the list.

use std::time::Instant;

use oso_core::syntax::Highlight;

use super::super::{Entry, Fold, Group, Key, LEAD, Line, List, Rendered, Row, face, mark, rank};
use super::{
    INDENT, Project, QUIET_AFTER, SpaceRows, Worktree, cells, cut, cut_middle, heading,
    named_heading, pad, section, worst,
};
use crate::sessions::recap::clean;
use crate::sessions::state::State;

impl List {
    /// A space's row, then what it holds unless it is folded: by default, while another space
    /// is in front and none of its sessions works or needs input (ADR 0136, step 2, Q4). A
    /// space of one repository shows that repository's worktrees right under its row (Q1).
    pub(super) fn space(
        &self,
        out: &mut Rendered,
        (space, orchestrator): (&SpaceRows, &[&Entry]),
        placed: &[Vec<Vec<&Entry>>],
        width: usize,
        now: Instant,
    ) {
        let mut all: Vec<&Entry> = placed.iter().flatten().flatten().copied().collect();
        all.extend(orchestrator.iter().copied());
        let front = self.tree.front_space.as_ref() == Some(&space.id);
        // Only oso's own sessions unfold a space: another program's are listed, quiet.
        let busy = all.iter().any(|e| {
            matches!(e.key, Key::Hosted(_))
                && matches!(e.state, State::Working | State::NeedsInput(_))
        });
        // With no space in front, none is folded.
        let quiet = self.tree.front_space.is_some() && !front && !busy;
        let folded = self.space_folded(&space.id, quiet);
        let head = format!("{LEAD}{} ", if folded { '▸' } else { '▾' });
        let name = clean(&space.name);
        let line = Line::Space(space.id.clone());
        let face = if front {
            Highlight::SessionFront
        } else {
            Highlight::SessionSpace
        };
        section(out, (&head, &name, Some(face)), roll(&all), width, line);
        if folded {
            return;
        }
        self.opened(1);
        // Its orchestrator first, as the session the person talks to (ADR 0136, step 3), then
        // the earlier ones that are still there, renamed (step 5).
        let current = |entry: &&Entry| Some(entry.key) == space.orchestrator;
        let (now_row, earlier): (Vec<&Entry>, Vec<&Entry>) =
            orchestrator.iter().copied().partition(current);
        for (entry, tag) in (now_row.into_iter().map(|e| (e, "orchestrator")))
            .chain(earlier.into_iter().map(|e| (e, "earlier")))
        {
            let row = Row {
                tag: Some(tag),
                ..Row::new(entry, INDENT[1], self.spin, 0)
            };
            row.render(out, width, now);
        }
        if let [only] = space.projects.as_slice() {
            return self.body(out, (space, only), &placed[0], 1, width, now);
        }
        for (project, placed) in space.projects.iter().zip(placed) {
            self.repository(out, space, project, placed, width, now);
        }
    }

    /// A repository's row in a space of several, then its worktrees unless it is folded.
    fn repository(
        &self,
        out: &mut Rendered,
        space: &SpaceRows,
        project: &Project,
        placed: &[Vec<&Entry>],
        width: usize,
        now: Instant,
    ) {
        let all: Vec<&Entry> = placed.iter().flatten().copied().collect();
        let fold = Fold::Repository(space.id.clone(), project.root.clone());
        // One with nothing to show is folded, so that no open fold has nothing under it.
        let empty = project.worktrees.is_empty();
        let folded = empty || self.folded_as(&fold, false);
        let head = format!("{}{} ", INDENT[1], if folded { '▸' } else { '▾' });
        let line = Line::Repository(space.id.clone(), project.root.clone());
        let name = format!("{}/", clean(&project.name));
        let face = Some(Highlight::SessionRepository);
        named_heading(out, (&head, &name, face), roll(&all), width, line);
        if !folded {
            self.body(out, (space, project), placed, 2, width, now);
        }
    }

    /// A repository's worktrees at `depth`, the main checkout first and then the newest made,
    /// with those quiet for a week in a folded row of their own.
    fn body(
        &self,
        out: &mut Rendered,
        (space, project): (&SpaceRows, &Project),
        placed: &[Vec<&Entry>],
        depth: usize,
        width: usize,
        now: Instant,
    ) {
        let mut shown: Vec<usize> = (0..project.worktrees.len()).collect();
        shown.sort_by_key(|&w| {
            let worktree = &project.worktrees[w];
            (!worktree.primary, std::cmp::Reverse(worktree.made))
        });
        let quiet_since = self
            .tree
            .wall
            .and_then(|wall| wall.checked_sub(QUIET_AFTER));
        let (quiet, shown): (Vec<usize>, Vec<usize>) = shown.into_iter().partition(|&w| {
            let worktree: &Worktree = &project.worktrees[w];
            placed[w].is_empty()
                && !worktree.primary
                && worktree
                    .active
                    .zip(quiet_since)
                    .is_some_and(|(active, since)| active < since)
        });
        for w in shown {
            self.worktree(
                out,
                (space, &project.worktrees[w], depth),
                &placed[w],
                width,
                now,
            );
        }
        if quiet.is_empty() {
            return;
        }
        let group = Group::Quiet(space.id.clone(), project.root.clone());
        let folded = self.folded(&Fold::Group(group.clone()));
        let head = format!("{}{} ", INDENT[depth], if folded { '▸' } else { '▾' });
        let name = format!("{} quiet", quiet.len());
        heading(out, (&head, &name), None, width, Line::Header(group));
        if !folded {
            for w in quiet {
                self.worktree(out, (space, &project.worktrees[w], depth), &[], width, now);
            }
        }
    }

    /// `Archived spaces`, last and folded: each closed space's name, with its title dim.
    /// `RET` on one opens it again.
    pub(super) fn closed(&self, out: &mut Rendered, width: usize) {
        let closed = &self.tree.closed;
        if closed.is_empty() {
            return;
        }
        let folded = self.folded(&Fold::Group(Group::ArchivedSpaces));
        let head = format!("{LEAD}{} ", if folded { '▸' } else { '▾' });
        let count = Some((closed.len().to_string(), Highlight::SessionQuiet));
        section(
            out,
            (&head, "Archived spaces", None),
            count,
            width,
            Line::Header(Group::ArchivedSpaces),
        );
        if folded {
            return;
        }
        for space in closed {
            let head = format!("{}{} ", INDENT[1], mark(State::Stopped));
            let name = clean(&space.name);
            let room = width.saturating_sub(cells(&head) + 1);
            let mut text = format!("{head}{}", cut_middle(&name, room));
            let title = clean(&space.title);
            let left = room.saturating_sub(cells(&name) + 2);
            let at = text.len();
            if !title.is_empty() && left > 0 {
                text.push_str("  ");
                text.push_str(&cut_middle(super::super::unmarked(&title), left));
            }
            let spans = [
                (INDENT[1].len()..head.len() - 1, Highlight::SessionQuiet),
                (at..text.len(), Highlight::SessionQuiet),
            ];
            let line = Line::ClosedSpace(space.id.clone());
            out.push_drawn(&cut(&pad(&text, width), width), line, &spans);
        }
    }
}

/// The most urgent state of `sessions`, with how many are in it, for a heading's right edge.
fn roll(sessions: &[&Entry]) -> Option<(String, Highlight)> {
    worst(sessions).map(|top| {
        let n = sessions
            .iter()
            .filter(|e| rank(e.state) == rank(top.state))
            .count();
        // Another program's session is listed quiet, as its worktree's row has it.
        let shown = if matches!(top.key, Key::Hosted(_)) {
            face(top.state)
        } else {
            Highlight::SessionQuiet
        };
        (format!("{} {n}", mark(top.state)), shown)
    })
}
