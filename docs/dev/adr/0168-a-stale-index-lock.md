# 0168. A stale index.lock

- Status: Accepted
- Date: 2026-11-22

## Context

A run oso stopped just after it began, a commit within about 20 ms of its start under load, could
leave git's `<gitdir>/index.lock` behind ([ADR 0127](0127-source-control-and-github.md), step 1b).
20 of 400 such stops did, and 24 with the group's `SIGKILL` at the reap turned off. Plain git
sent `SIGTERM` outside oso did too. So oso's kill is not the cause: git holds the lock for the
moments before a plain commit's hook, and a signal there can leave it. Every later commit and
staging then failed. The echo area said "commit failed (hook pre-commit)", while only the log
showed git's `index.lock: File exists`. Step 1b's follow-up had said only that `git status`
names such a lock.

## Decision

The author's choices, 2026-11-22, through the coordinator: (B) and (C), and (a) on each of the
questions Q1 to Q6.

- **Where the lock is:** git says, once per repository's thread: `rev-parse --git-path index`,
  with `.lock` after it. A worktree's lock is in its own git directory; nothing is guessed.
- **Noted before and after a run (B):** the lock's path is looked at, no link followed, before
  each request the run thread takes. The status's thread does the same before each staging,
  unstaging or discard (`s`, `u`, `k`, Q2). The file there is kept by its device, its inode and
  the time its inode last changed, so that a number freed and given to a new file is not taken
  for the old one.
  - Once a run oso stopped has ended (git reaped, its group killed), a lock that was not there,
    or another file, is reported and noted for the repository.
  - The echo area then says, for example, "git commit stopped; the stop left .git/index.lock:
    git writes no index while it is there; M-x git-remove-stale-lock removes it".
- **A failure on the lock (C):** this covers a run or a staging that failed with a line naming
  `index.lock'`, the way git quotes the path it could not make. A remote's lines (`remote:`) do
  not count, and the same file must be there after the run as when it began.
  - The echo area says "git found .git/index.lock: another git may run, or a stop left it; M-x
    git-remove-stale-lock removes it", in place of the hooks that may have made the failure,
    and the lock is noted.
  - The name is matched, not git's words, which `LANG` may translate.
  - A commit that failed this way ran no hook, so `C-c C-f` asks no session to fix it (Q6).
- **`M-x git-remove-stale-lock`:** `Person`, with no key (Q1). It works in a status, its log or a
  buffer of the project.
  - It asks "Remove .git/index.lock, made 3 min ago? Another git may own it". The age helps the
    person judge whether a git still runs (Q5).
  - On `y` the run thread looks again. It takes the lock away only if it is still the file
    noted; another file there is left, as a git the person started elsewhere may own it.
  - With no lock noted, the command only says so (Q3): oso removes nothing it did not see left.
  - It is refused while git runs in the repository.
  - Nothing takes a lock away unasked.
- **Windows** (Q4): the standard library gives no device or inode, so a file is known by its
  creation time, last write time and length. This stand-in is weaker.
- **Code:** `vcs/lock.rs` finds the lock, looks at it and takes it away. `vcs/read.rs`'s
  `git_path` asks git where the lock is, as it does for the hooks directory. `app/vcs/lock.rs`
  keeps the note and asks the question. The library item is `GitRequest::RemoveLock` in
  `oso-core`.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0127](0127-source-control-and-github.md), "Follow-up of step 1b", (A): "git may then leave
  its `index.lock`, which `git status` names, as a kill always could". What holds now: a lock
  that a stopped run left is noted and said in the echo area, and `M-x git-remove-stale-lock`
  takes it away after the person's `y`, if it is still the same file.
- ADR 0127, step 1b: "A failure names the hooks that may have made it". A failure on the index's
  lock names the lock instead, and no hook.

## Consequences

- A stop in git's short window now costs one question, not a failing repository that names the
  wrong cause.
- Each run and each staging costs one more `lstat`. Each thread also runs one `git rev-parse`,
  the first time it needs the lock's path.
- Other locks git may leave, such as `HEAD.lock` or a ref's, are not looked at. A note lasts
  until oso ends.
- **Tests:**
  - in `vcs/lock/tests.rs`:
    - how a lock is named;
    - what a stop or a failure notes;
    - that only the file noted is removed: another file, or one whose change time moved, is
      refused, and a link goes itself, not its target.
  - on the run thread:
    - a stop that leaves a lock: a hook makes one, as a stop in git's window would leave it;
    - the next commit failing on that lock, with no hook named;
    - `y` taking it away, and a commit going in after;
    - a lock planted before a commit, and another file put in its place, which is left;
    - a worktree's lock in its own git directory.
  - staging that fails on a lock;
  - the command, and the question's text;
  - an end-to-end flow.

## Alternatives considered

- **Take the lock away by itself after a stop:** rejected. A git the person started elsewhere may
  own a newer lock.
- **Match git's "File exists":** rejected. `LANG` reaches git, which may say it in another
  language.
- **Offer to remove any lock found, noted or not (Q3 b):** rejected. oso would then remove what
  it never saw left.
- **A key in the status (Q1 b):** rejected. The command is rare, and it removes a file.
- **The run thread alone (Q2 b):** rejected. A stop of `s`, `u` or `k` can leave the lock too.
- **Refuse on Windows (Q4 b):** rejected in favour of the weaker stand-in.
