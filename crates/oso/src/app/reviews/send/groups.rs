//! A pull request's comments read a commit at a time (ADR 0167): the person's comments not
//! sent of each review of it, a group each, and the message of them, grouped once a commit's
//! review holds some.
//!
//! Must not depend on: key presses, how a frame is drawn, or the host's threads; plain
//! functions over the editor's models.

use oso_core::editor::ReviewId;

use super::unsent;
use crate::commands::agent_sessions::MAX_SEND_BYTES;
use crate::review::message::{Group, Item, Message, compose, compose_groups};

/// The person's comments not sent of each of `members`, a review and its group's heading, in
/// the order the review shows them: the reviews, and a group each.
pub(super) fn gather(
    editor: &oso_core::editor::Editor,
    members: Vec<(ReviewId, Option<String>)>,
) -> (Vec<ReviewId>, Vec<Group<'_>>) {
    let mut reviews = Vec::with_capacity(members.len());
    let mut groups = Vec::with_capacity(members.len());
    for (member, heading) in members {
        let (Some(model), rows) = (editor.review(member), editor.review_rows(member)) else {
            continue;
        };
        let items: Vec<Item<'_>> = (unsent(model, rows).iter())
            .filter_map(|id| model.comment(*id))
            .map(|comment| Item {
                comment,
                parent: comment.reply_to().and_then(|to| model.comment(to)),
                shape: model.file(comment.path()).map(|shape| &**shape),
            })
            .collect();
        reviews.push(member);
        groups.push(Group {
            heading: heading.unwrap_or_default(),
            items,
        });
    }
    (reviews, groups)
}

/// The message of `groups`: a group each, under the note of pull request `number`, once a
/// group after the first, a commit's, holds a comment (ADR 0167); else as any review's.
pub(super) fn composed(
    head: &str,
    scope: &str,
    number: Option<u64>,
    groups: &[Group<'_>],
    withheld: impl Fn(&[u8]) -> bool,
) -> Message {
    let grouped = groups.iter().skip(1).any(|group| !group.items.is_empty());
    match number {
        Some(number) if grouped => {
            compose_groups(head, scope, number, groups, MAX_SEND_BYTES, withheld)
        }
        _ => {
            let items: Vec<Item<'_>> = (groups.iter())
                .flat_map(|group| group.items.iter().map(Item::again))
                .collect();
            compose(head, scope, &items, MAX_SEND_BYTES, withheld)
        }
    }
}
