# 0167. Reviewing a pull request a commit at a time

- Status: Accepted
- Date: 2026-11-22

## Context

A review of a pull request ([ADR 0122](0122-agent-review.md), step 12b) shows its whole diff,
from where its head left the base to its head. `c` in the review menu reviews one commit and `r`
a range, but nothing steps through a pull request's commits, lists them, or shows a commit's
message. The author's Emacs package, ECC, has this, and the author asked for it in oso, better
than ECC's flow where there is a better one.

How ECC does it, as read from its code (as a reference for behaviour only):

- After choosing a pull request, `p` asks which of its commits to review. The default is the
  whole pull request. Merges and commits that change no file are left out.
- `]` and `[` open the next and the previous commit in the same window. Each commit is a review
  of its own, of `X^!`, with comments of its own.
- The header line says `#12 commit 2/3` and how many comments not sent the other reviews of the
  pull request hold.
- `C-c C-a` sends the comments of every review of the pull request as one prompt, a group a
  commit. The prompt says that a commit's lines are as that commit left them.
- It shows no commit message and has no list of the commits on screen. It has no "since my last
  review", no viewed state per commit, and nothing kept across a restart.

Others: GitHub's commit filter (`C`), and its "since your last review", which it shows disabled
for now. GitLab's previous and next commit (`X`, `C`). Gerrit's patch sets, its commit message
as a file that takes comments, and its reviewed marks per patch set. magit's log, whose revision
buffer follows point. Graphite's and Reviewable's versions and per-revision reviewed marks.
Reviewable and GitHub anchor a comment to the revision it was written on.

The author chose (a) on each of the plan's ten questions on 2026-11-22, through the coordinator.

## Decision

### The model (Q1)

- **A commit is reviewed as the commit scope is:** a review of one commit, against its first
  parent, with its own comments, decisions, viewed marks and saved file (ADR 0122, steps 1 to 6).
  No reading machinery is new.
- **A review of a pull request keeps its commits**, as its own thread lists them, and each
  commit's review opened by a step is linked to it while both are open. Closing the pull
  request's review unlinks them; they stay open as reviews of a commit.
- **The commits** are listed on the pull request review's own thread, after its sides resolved
  and any fetch ended, so no `gh` runs and nothing reaches the network:
  - The listing is `git log --topo-order --no-show-signature --no-notes` of
    `MERGE-BASE..TIP`, newest first, in at most 1 MiB of output, so that a cut loses the
    oldest, never the head. The newest 1,000 are kept and shown oldest first.
  - A line whose short id is not hex, or whose date is not `YYYY-MM-DD`, is left out. Runs of
    white space in a subject or a name are one space.
  - `TIP` is the head. Where the head branch is checked out here and the work tree shows
    (step 12b), it is `HEAD`, so that commits not pushed yet are listed too, marked `↑` (Q10).
  - A merge has more than one parent. A commit changes nothing when its tree is its first
    parent's.
  - Both are listed, `merge` or `empty` in the marks column, before the short id and anyone's
    words. `M-n` and `M-p` pass over them; `RET` or `SPC` on one shows it, a merge against its
    first parent, as `c` shows one (Q8).
  - The listing is read again on open and on each `g` that asks `gh` again, never on step 4's
    refreshes. Commit reviews still listed keep their link, by id.

### Moving (Q2)

- **`M-n` (`review-next-commit`) and `M-p` (`review-previous-commit`)** work in a pull request's
  review, in a commit's review linked to it, and in the commit list. They follow the
  next/previous heading keys of `*sessions*`. In a review, `[` and `]` are context lines (step
  13), `{` and `}` comments, `n`/`p` hunks and `N`/`P` files.
  - From the whole pull request, `M-n` goes to its first commit and `M-p` to its last.
  - From its first commit, `M-p` goes back to the whole. At its last, `M-n` says so and stays.
- `review-pull-request-whole` goes back to the whole.
- **A step shows the review in the layout's left** (ADR 0160), selected. A step asked while
  the list is selected keeps it selected, and goes from the review the list shows. `RET` in
  the list selects the review's window first, so that the review shown is selected. A step
  that left a review whose files pane showed shows the new review's files pane.
- **The link** of a commit's review to its pull request's is the editor's from the commits
  the host hands in, which name the commit reviews open; the editor links them before a step
  shows one, so the list stays above it.
- **At most eight commit reviews of a pull request stay open.** Past them, the one used longest
  ago is closed. A review a window shows is never closed, nor one holding comments of the
  person's not sent, nor one a draft is open on. So a send finds every comment not sent among
  the reviews open.
- Steps are `Person`: opening a review runs git (ADR 0085).

### The commit list (Q3)

- **`l` (`review-commits`, `Person`)** shows the list in a window above the review's window, or
  above the whole pair of its panes. The window is as many rows as the list has, at most nine,
  plus its modeline. The list selects itself; `l` or `q` there closes it.
- The list goes when no window shows the pull request's review or any of its commits'.
- **Its rows:**
  - first `All changes · #N · M commits`;
  - then a row a commit, oldest first: its place `2/5`, its marks, its short id, then
    anyone's words: its subject (at most 120 characters), its date and its author;
  - a last row when more than 1,000 were left out.
- **Its marks:**
  - `▶` the review shown last;
  - `merge` or `empty` for one a step passes over;
  - `✓` every hunk viewed, `◐` some;
  - `●N` comments of the person's not sent;
  - `↑` not pushed.

  They are counted from each commit review open, again only when its model changed. The
  entries are one `Arc` the host copies only when a mark changed, and the editor lays out
  again only the rows that changed. Its cursor moves to the review shown only when that one
  changed, so that a mark counted again never moves the person's cursor.
- **Its keys:**
  - `n`/`p` and `C-n`/`C-p` move;
  - `RET` (`review-commits-visit`) shows the row's review and selects it;
  - `SPC` (`review-commits-display`) shows it and stays;
  - `M-n`/`M-p` step;
  - `g` reads the pull request again;
  - `?` (`review-commits-keys`) lists the keys.
- Moving in the list opens nothing: a quick walk down fifty rows would run git fifty times.
- Words are anyone's: subjects and names are escaped on one line as git's text is shown.

### A commit's review

- **Its message shows under its title**, as rows of `RowKind::Message`:
  - a line `commit SHORT · DATE · AUTHOR`, anyone's words last;
  - then each line of the message, indented four spaces as `git log` shows it, each escaped;
  - then an empty row.
- No motion stops on these rows, and no comment goes on them (Q7: comments on the message are
  left for later, as Gerrit's `/COMMIT_MSG`).
- The message is read on the review's own thread, `git log -1` of its id, at most 64 KiB, once
  each time its id is known or moves, for any review of one commit (`c` too). Past 100 lines, a
  row says how many more are not shown.
- **The modeline** of a commit's review says `#12 2/5`, and the pull request's says
  `#12 all 5 commits`.
- The echo area says `Reviewing #12, commit 2/5 abc1234 · subject` after a step.
- **Decisions:** accept and reject already refuse in a commit's review, which compares commits
  (step 5). Viewed marks are a commit review's own, keyed by its hunks' content, so a commit
  force-pushed is a new id whose hunks start unviewed. Undo is per review.

### Sending (Q4)

- **`C-c C-c` in any review of a pull request** gathers the person's comments not sent of the
  whole pull request's review and of each of its commits' reviews open, in one message.
- **Once a commit's review holds some,** the message groups them:
  - first a note: "They are on pull request #N, read a commit at a time: a group a review, the
    whole pull request first, then its commits, oldest first. A commit's lines are as that
    commit left them, and a later commit may have changed them: read every group before
    changing anything.";
  - then `# The whole of #N`;
  - then `# Commit 2/5 abc1234 "subject"`, a group per commit, oldest first, its subject quoted,
    escaped and cut to 120 characters, so that it reads as no words of oso's.

  A group with nothing to send is left out. A group's heading never goes without its first
  comment, and past 256 KiB the rest wait, as before.
- **Where it goes** is the pull request review's target (step 12a, step 12b, and ADR 0163's
  orchestrator in a space). `C-u C-c C-c` still picks the session.
- A file that may hold secrets is judged under the pull request's places. The comments marked
  sent are each review's own.

### Persistence

- **A commit's review stepped to** is saved as the commit scope is, by its full id. Its header
  gains an optional `pull`, the pull request's number, written only when there is one.
- **One brought back** names its pull request. It is linked again once that pull request's
  commits are listed, when the person opens that pull request's review. Nothing is opened for
  it: that would ask `gh`, which only the person's open or `g` does (step 12b).
- **A commit's review a step opened that holds no comment and no mark, and was never saved, is
  not written,** so that walking two hundred commits does not push the person's other reviews
  out of the store's 256.
- The marks of commits never reviewed in this run are not read from saved files: such a commit
  shows none until its review opens.

### Limits

- **Listing:**
  - 1,000 commits listed, the newest, and 1 MiB of listing read;
  - a subject, a name or a line of a message kept to 1,024 characters, shown to 120 in the list;
  - a message read to 64 KiB and shown to 100 lines.
- **Open reviews:** eight commit reviews open a pull request, plus those a window shows or that
  hold comments not sent or a draft.
- **Every git run is on a review's own thread** and killed at 10 s; none runs on the UI thread,
  and no `gh` runs per step.
- **The list's text** is laid out by the editor from the commits handed in: whole, at most 1,002
  rows of at most about 240 characters, when the list is made or a listing comes; else only
  the rows that changed. A subject is kept to 200 characters in the host.

### The benchmark

- The performance harness gains a scenario of its own (`bench/review_commits.rs`, the author's
  approval of 2026-11-22): a pull request of 200 commits against a stand-in `gh`.
  `review_commits_list_ms` is the time from `RET` on its number to its modeline naming its
  commits, and `review_commit_step_ms` the median of five steps from `M-n` to the next
  commit's review read whole. Both are recorded with no threshold, on Unix alone.

### Public items

In `oso_core`:

- `ReviewRequest::Step { review, to }` and `Step::{Next, Previous, Whole, Commit(usize)}`;
- `Commits` (with `open`, the commit reviews open, which link them) and `CommitEntry`, plain
  data;
- `Editor::set_review_commits`, `review_commits` and `review_pull`;
- `Editor::show_review_commits`, `hide_review_commits` and `review_commits_pane`, the list's
  window as the files pane's is shown, hidden and named (step 6b), beyond the items the
  author approved, for approval;
- `Mode::ReviewCommits`;
- `RowKind::Message`.

No new crate, crate edge or dependency.

### Steps

1. **This PR:**
   - the listing and the walk;
   - `M-n`/`M-p` and the commit list;
   - the message rows;
   - the grouped send;
   - the saved `pull` and the rule that writes nothing for a commit review holding nothing.
2. **Since the last review:**
   - a checkpoint per pull request: its head id in the pull request review's saved file, set at
     each `C-c C-c` of the pull request and by `M-x review-checkpoint` (Q5,
     `ReviewRequest::Checkpoint`);
   - a `Since last review (N new)` row that reviews `CHECKPOINT..HEAD`, and `new` on the
     commits after it;
   - past a rebase, the tree diff of the old head against the new with a note, and commits
     whose `git patch-id` matches one listed at the checkpoint not marked new (Q6);
   - a range from the list, `m` then `RET` (`Step::Since` and `Step::Range`).
3. **Later:** the message says where each commit comment's line is at the head now ("now L57 at
   the head", "changed later"), carried on the review's thread through ADR 0122's carry (Q9).
   Comments on the commit message come later too (Q7).

## Changes to accepted ADRs

- **ADR 0122, "Keys":**
  - `M-n` and `M-p` step through a pull request's commits, and `l` shows its commit list.
  - The list has keys of its own.
- **ADR 0122, "Persistence" and step 6:** a saved review's header may name `pull`. A commit's
  review a step opened that holds nothing is not written.
- **ADR 0122, "Sending comments to the agent":** a pull request's reviews send together, a
  group a review.
- **ADR 0122, "The views":** a review of one commit shows the commit's message under its title.
- **ADR 0160, "The layout":** the review's area may hold the commit list above the review, as
  it holds the files pane at its left.

## Consequences

- A pull request reads commit by commit with one key each way. Every review piece works in a
  commit's review as in any other, as each is an ordinary review.
- A commit's review pays its own git runs and diff on first open, within ADR 0122's budget for
  a review's first screen. The eight kept open make stepping back free.
- Comments stay on the lines of the commit they were written on, and the message says so. Where
  they are at the head is step 3.
- The marks of commits reviewed in an earlier run come back only as their reviews open again.
- `c` reviews of a commit show its message too.
- A step opens a review at once: holding `M-n` through a long pull request opens one a key,
  each with its threads, those past eight closed again. Opening only once the keys stop is
  left for later.
- The list's marks columns pad by characters; `✓`, `◐`, `●` and `↑` are of ambiguous width,
  as in the files pane.

## Alternatives considered

- **One review buffer whose reading changes per commit, its comments carried between readings**
  (Q1 (b)): fewer buffers, but a comment written on commit 4 would show as outdated on commit 1,
  where its line does not exist yet.
- **`C-c C-n`/`C-c C-p`** (Q2 (b)): longer, for a key pressed many times. **`]`/`[` as ECC's**
  (Q2 (c)): would move step 13's context keys.
- **The list in the left column above the files pane, or in the slot** (Q3 (b), (c)): the
  column steals the width two panes need (160 columns), and the slot is for the source and the
  messages (ADR 0160).
- **`C-c C-c` for this review only, `C-c C-a` for all** (Q4 (b)), as ECC: GitHub's review is
  one submission, and two keys would leave comments behind.
- **Leaving merges and empty commits out** (Q8 (b)), as ECC: the list would miss what the pull
  request holds.
- **Moving in the list opening the commit, as magit's revision buffer follows point:** git would
  run for each row passed.
- **Reading the commits through `gh pr view --json commits`:** it runs `gh` and stops at 250
  commits, while the clone holds them all once the pull request's sides are fetched.

## Step 2, as built

The second PR. The author answered its plan's four questions on 2026-11-22, through the
coordinator, and approved its public items as listed.

- **The checkpoint (Q1, Q3):**
  - The checkpoint is the person's last review of a pull request: where its head left the base,
    and its head, as its review last listed them.
  - Pasting the pull request's comments records it: the second `C-c C-c`, from any of its
    reviews. It records the head the message was made from, not one listed since. A dropped
    message records nothing.
  - `M-x review-checkpoint` (`ReviewRequest::Checkpoint { review }`, `Person`) records it too. It
    records the head only and marks nothing viewed.
  - It is saved in the pull request's review's header as `checkpoint` (two whole ids), written
    only when there is one, and saved again whenever it moves.
- **What is new:**
  - A listing whose head is past the checkpoint asks the review's own thread what is new. The
    ask is held as a look is: it stops no reading, it waits until nothing else does, and only the
    latest is answered. A newer request stops it, which the thread says, and it is asked again
    once that request has read. A listing at the same base and head as the one before keeps
    what that one found, and asks nothing.
  - When the head then is an ancestor of the head now, the commits after it are new.
  - Else, after a rebase, a commit is new when its `git patch-id --verbatim` is not one the
    commits then had, so that a change of white space alone is new.
  - Patches are read in the listing's order with `git log -p --format='commit %H' --binary
    --submodule=short --no-renames`, so that no message line reaches `patch-id`. At most 64 MiB
    is read a side.
  - A commit that changes something and has no patch read is new. So is every commit, when git
    has no `--verbatim`.
  - A head then that the clone no longer holds is gone. The list says so, and its row refuses.
- **The list:**
  - Its second row, while the head has moved past the checkpoint, is `Since last review · N new
    commits`, `rebased, N new commits`, or `its head then is gone from this clone`.
  - Its new commits are marked `new` in the marks column.
  - The editor's `Commits` gains `since: Option<Since>`, plain data: how many commits are new,
    rebased, gone, whether its review was shown last, and that review. `CommitEntry` gains `new`.
- **The review of what changed since (Q4):**
  - `RET` or `SPC` on the second row (`Step::Since`) opens a review of the head then against the
    head now. It is a scope of its own, `since` in a saved file, keyed by the pull request and
    the two ids, saved as any review is and swept after 30 days. Its echo area and modeline say
    `#N since last review`.
  - It is linked to the pull request's review, so the list stays above it, and its comments go
    in the pull request's send, after the whole's, under "What changed in #N since the last
    review".
  - `M-n` from it steps to the first new commit that changes something, else the first commit.
    `M-p` from it goes to the whole.
  - A new checkpoint unlinks it.
- **A range:** in the list, set the mark (`C-SPC`) on one commit's row and go to another's.
  - `RET` or `SPC` then opens a review of those commits (`Step::Range(a, b)`), as `r` reviews a
    range, and the mark goes.
  - A mark on a row of no commit refuses.
  - The review is of no one commit, so it is not linked, and the list goes while it is shown.
- **Public items:** `ReviewRequest::Checkpoint`, `Step::Since`, `Step::Range`, `Since`,
  `Commits::since` and `CommitEntry::new`.
- **Tests:**
  - Against temporary repositories: what is new after an ancestor, after a rebase (patch-ids
    kept), and with a head that is gone.
  - The list's second row, its marks and its cursor, and the modeline.
  - The steps from and to what changed since, and a range.
  - The saved `seen`.
  - The commands: the second row, a marked range, a mark off the commits, and the checkpoint.
  - An end-to-end flow, `review_since::a_checkpoint_shows_what_changed_since_and_m_n_steps_to_the_new_commit`.
