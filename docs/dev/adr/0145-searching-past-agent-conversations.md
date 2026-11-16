# 0145. Searching past agent conversations

- Status: Accepted
- Date: 2026-11-16

## Context

The author runs many Claude Code sessions a day, in several repositories, worktrees and spaces,
and asked (2026-11-16) to find a session by what was said in it: "search the contents of past
conversations to find a session; by default every conversation on this machine, with an option
to limit it to the current space".

What oso has today:

- [ADR 0121](0121-agent-sessions-and-spaces.md) hosts sessions, keeps a record of them, reads
  Claude Code's registry (`~/.claude/sessions/<pid>.json`) for live processes, and resumes a
  stopped one with `session-resume`. Its "Adopting a session started outside oso" resumes any
  conversation by its id (`session-adopt`), reading only a transcript's id, title, directory and
  change time, never what was said. Its step 6c planned an agent tool, `session_search`, which
  was never built.
- [ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)'s `project-search` is
  a centred float ([ADR 0088](0088-switching-and-killing-buffers.md): 62% wide, 18 rows and the
  answer's) whose rows stream from threads of the host as the input changes, with a preview on
  the right. Its input is [ADR 0076](0076-completion-and-m-x.md)'s orderless components, with
  [ADR 0116](0116-migemo-and-jumps.md)'s migemo.
- [ADR 0136](0136-issue-spaces.md) and [ADR 0140](0140-a-spaces-pane-with-sections-and-guides.md):
  a space is its member repositories, its own directory, its worktrees and its orchestrators;
  `*Spaces*` is a 44-column pane.
- [ADR 0143](0143-agent-keys-on-c-c-a-and-an-agents-menu.md): the agent commands are heads of the
  Agents menu on `C-c a`.

What Claude Code keeps, as surveyed on the author's machine (counts only):

- A transcript per conversation: `$CLAUDE_CONFIG_DIR/projects` (else `~/.claude/projects`),
  `<slug>/<session id>.jsonl`, one JSON object a line, written by appending. A conversation's
  subagents are under `<slug>/<id>/subagents/`. The slug is the first directory with every `/`
  and `.` turned into `-`, so it cannot be read back.
- 685 conversations in 526 directories, 1.95 GB (median 1.2 MB, largest 65 MB, longest line
  1.4 MB). Every file is the user's alone (`0600`). Claude Code deletes transcripts older than its
  `cleanupPeriodDays`.
- About 1% of those bytes is what people read: about 4,000 prompts (4 MiB) and 29,000 reply
  texts (14.5 MiB). The rest is tool results (344 MB), thinking (115 MB), tool inputs (96 MB),
  attachments (`CLAUDE.md`, memory, files named with `@`), hook and command output, and request
  records. A word of the memory index is in 508 of the 685 files through attachments alone, so a
  match on the raw bytes says little.
- A file's name is its session id, as each line's `sessionId` says. The directory is a line's
  top-level `cwd`; 144 conversations moved to another directory partway (a worktree). Only 135
  have a title line (`custom-title`, `agent-name` or `ai-title`); the others are known by their
  first prompt. 143 ran under `/private/tmp` and 356 in worktrees, many of them removed since.

## Decision

The author chose the recommended option of each question (2026-11-16, through the coordinator):
a float, every conversation by default, `RET` goes to the session, one message as the unit,
prompts and replies only, one row per hit, gone directories listed and only readable, and no
agent tool.

### The command and its float (Q1)

- **`session-search` (`Person`, `C-c a /`)**, a head of the Agents menu in its Go to group, and
  `M-x`. It opens a float as `project-search`'s, labelled `Search conversations (all):`, whose
  rows stream in as conversations are searched, with a preview on the right.
- **A row is one message that matches** (Q5, Q7): its conversation's mark, name, place and age,
  then `›` for a prompt or `‹` for a reply and the line of its first match, the match in the match
  face:

  ```text
  Search conversations (all): worktree trust                                    132 …
  ● oso-2 · oso-2 · 3m › …can worktree trust survive a rename?…
  ● oso-2 · oso-2 · 3m ‹ …the trust record keys by device and inode…
  ○ lsp-8b · oso/lsp-8b · 2h ‹ …a worktree's trust root is its main…
  ◌ Fix the login redirect · webapp · 12d › …why does worktree trust…
  ◌ 3f2a91c0 · tmp-run (gone) · 40d › …
  ```

  - Marks: `●` a session this oso hosts that runs, `○` one it hosts that is stopped, `⊘` one
    Claude Code's registry says runs elsewhere, `◌` any other.
  - The name is the hosted session's, else the transcript's title (the person's, then the
    agent's, then Claude Code's, as `session-adopt` ranks them), else the first line of its first
    prompt, else the id's first 8 digits. Text of a transcript is cleaned as `session-adopt`
    cleans it: one line, cut short, controls and the `·` that splits the parts left out.
  - The place is `repository/worktree` for a directory under `<repository>/.claude/worktrees/`,
    else the directory's last name, and `(gone)` when it is no longer a directory.
  - Rows come grouped by conversation, the conversation that changed last first, and a
    conversation's rows in the order they were said. At most 50 rows a conversation and 10,000 in
    all, as `project-search` keeps.
- **The preview** shows the conversation around the selected row: the messages before and after
  it, each starting `› ` or `‹ `, wrapped at the preview's width, with the row's match
  highlighted. It comes from what the search holds in memory; no file is read for it.
- **Nothing in a prompt or a row is interpreted:** text is drawn as text, and controls, bidi
  controls and invisible characters are boxed, as in `project-search`'s rows.

### Scopes (Q3)

- **Every conversation on this machine by default:** every transcript of this user under
  Claude Code's `projects` directory, of every project, open in oso or not.
- **`C-c C-s` in the float cycles the scope** (`session-search-scope`, bound in every prompt,
  where it says "Not searching conversations" elsewhere): all, then the space in front (if one is), then
  the selected buffer's project (if it has one), then all again. The label names it:
  `Search conversations (space oso-2):`, `Search conversations (project oso):`. The input stays
  and the search starts again.
- **`C-u C-c a /`** opens it in the space's scope, else the project's.
- **A conversation is in a scope** when one of the directories it ran in is at or under one of the
  scope's roots, component by component, or its id is one of the scope's sessions:
  - a space: its member repositories, its own directory and its worktrees; its orchestrator and
    earlier orchestrators, and the sessions oso hosts in it;
  - a project: its root, and its main repository's root for a linked worktree.

  A linked worktree outside its repository's directory is in its space's scope once the space
  holds it, and not in a project's.

### `RET` and what is shown (Q4)

- **`RET` goes to the session** of the selected row:
  - a session this oso hosts: shown, in its space, as `session-switch` shows it; a stopped one
    shows its terminal, which says `session-resume`;
  - any other whose directory exists: adopted, as `session-adopt` adopts it, resumed in the
    directory it first ran in, so its transcript is found, and placed by the open project that
    holds that directory;
  - one that runs elsewhere: the resume is refused with `session-adopt`'s question, "stop it
    there and adopt it again, or fork it instead?";
  - one whose directory is gone (Q8): not resumed. The echo area says the directory is gone;
    once step 2 is built, it says that `C-RET` reads it.
- **`C-RET` opens it read-only** (step 2): a buffer `*conversation: NAME*` of its prompts and
  replies, at the row's message, never saved, and withheld from the agent channel as a buffer
  that may hold secrets is. `M-RET` forks it (step 2), as `session-fork` does.
- **The history keeps the input,** as `project-search`'s does, but not beside a buffer that may
  hold secrets, as no prompt does there.

### What is searched (Q5, Q6)

- **Prompts and replies only (Q6):**
  - a prompt is a `user` line's text, a string or its `text` parts, that is not `isMeta`, not a
    compact summary, not of a side chain, and does not start as a command's or hook's wrapper
    (`<command-`, `<local-command`, `<bash-`, `<task-notification`, `<system-reminder`,
    `[Request interrupted`). This is a deny list: a wrapper Claude Code adds later is searched
    and shown, on the screen only, until it is listed;
  - a reply is an `assistant` line's `text` parts, not of a side chain and not an API error.

  Never tool inputs, tool results, thinking, attachments, images, request records, or the files
  under `subagents/`. These are where what an agent read lands, secrets included.
- **One message is the unit (Q5):** a message matches when every component that is not excluded
  matches somewhere in it and no excluded one does, in any order, by `project-search`'s rules:
  a literal, or a regular expression where it holds a character one reads, `!` to exclude, smart
  case over the whole input, a romaji component as its migemo pattern and `=` to keep it literal.
  `^` and `$` are a line's start and end in the message. The row shows the line of the first
  match, at most 512 bytes around it.
- **A search starts** once a component that is not excluded has 2 characters, as
  `project-search`'s does.

### Reading and memory

- **Its own thread, `oso-history`,** never the sessions worker or the UI thread, holds what was
  read; up to `min(cores - 1, 8)` reading threads read the transcripts for it, at most 64
  transcripts ahead of the one the search waits for. Each key starts a search with a new
  generation, which the search reads before each conversation; a superseded one stops there
  and sends nothing more. A search that comes while the transcripts are read takes over at
  once: it searches what was read so far, then each conversation as it comes, so the rows of
  the word being typed do not wait for the whole read.
- **What is read is held in memory only:** each conversation's id, directories, title, change
  time and the text of its prompts and replies. It is never written to disk, logged, carried
  over a restart, or sent over a socket. It goes 5 minutes after the last search.
- **A file is read again only from where it grew:** each is kept by its device, inode, length and
  change time, and the offset of its last whole line. A transcript only grows, so a longer one is
  read from that offset; a shorter or replaced one is read again whole, and a line still being
  written is read once it ends. The transcripts are listed again for a search once the last
  listing is 2 s old, so a running session's new turns are found by the next key 2 s after they
  were written. Rows already shown never move.
- **Files:** only `<slug>/<uuid>.jsonl` at the top level of each directory, at most 1,024
  directories and 4,096 transcripts, newest first. Each must be a regular file of this user with
  one name, opened without following a link, as `session-adopt` reads them. The slug is never
  read as a directory; a directory is taken from a line's `cwd` only when it is absolute with no
  `.` or `..` in it.
- **Lines:** a line is scanned without decoding the values it does not need, so a tool result is
  skipped by its quotes; a line that is not one whole JSON object says nothing. A line longer
  than 4 MiB is skipped: the longest lines are tool results. A message keeps at most 64 KiB of
  text. Each message counts as its text and 256 bytes more, its memory and the time a search
  spends on it, and each directory as its length; a conversation counts at most 1 MiB, so at
  most 4,096 messages, and all of them at most 64 MiB. Past that, older conversations are kept
  as stubs of what their file was, read again only once it changes, and the count says `+`.
- **Where a conversation resumes:** Claude Code keeps a transcript in the directory named after
  the one it started in, each character but an ASCII letter or digit as `-`, and finds it there
  to resume. So a conversation resumes in the first directory a line names whose name, spelled
  so, is that of the directory its transcript is in; one with none is listed but not resumed
  ("oso cannot tell where that conversation ran"). A `cwd` a planted line gives elsewhere is
  shown, never resumed in. The preview's first line names the whole directory, so the person
  sees where `RET` resumes before pressing it.
- **Echo-area messages** about a conversation never quote its text or its directory, and are not
  logged: agents read `*Messages*`.

### The agent channel (Q2)

- **`session-search` is `Reach::Person`,** as are `session-search-scope` and what step 2 adds.
  The channel never sees the input, the rows or the preview: ADR 0085's `state` already withholds
  a prompt's candidates, and nothing else carries them.
- **No agent tool searches conversations.** ADR 0121's planned `session_search` is dropped: a
  search over every conversation would hand one agent every other project's text. Its
  `session_transcript` stays as planned, for a later decision.

### Cost

| What | Budget |
|---|---|
| First rows after the float opens, over 2 GiB of transcripts | ≤ 300 ms |
| Reading 2 GiB of transcripts cold | ≤ 2 s |
| A key once the text is held, over 32 MiB of it (64 MiB at most, ≤ 100 ms) | ≤ 50 ms |
| The UI thread per batch of rows | as `project-search`: at most 1,000 rows every 30 ms |
| Memory | at most 64 MiB of text, gone 5 minutes after the last search |
| While no search runs | nothing: no timer, no thread wakes |

### Code

- **`sessions::history`,** a new submodule beside `sessions::transcripts`, reads, holds and
  searches the conversations, and makes the preview's text. No new crate and no new layer.
- **`oso-core`'s project search** takes a second source: the prompt that searches a project's
  files now also searches conversations, with its rows, highlights and preview, so the float, its
  streaming and its bounds are one piece of code. `editor` gains `ConversationScope` (which
  decides whether a conversation is in it), `ConversationSearchRequest`, `ConversationRow`,
  `ConversationHit` and `ConversationPreviewRequest`; `Excerpt::wrapped` makes a preview of
  prose wrapped at its width; `project::LineQuery::matches_message` matches a whole message.
  A file's search keeps its own request and rows unchanged. `command::spec` gains
  `ArgSpec::ConversationSearch`. No new public module.
- **`SessionRequest::GoToConversation`** carries the row `RET` took, its session and message,
  to the app, which shows, adopts or refuses as above, through `session-adopt`'s start.
- **The space's scope** is given to the editor by the app (`Editor::set_space_scope`) when the
  space in front changes or what the spaces hold does, as the editor knows no space; the
  project's scope the editor makes itself from the selected buffer's project.

### The work, in steps

1. The float, the scopes and `RET` (this ADR's first pull request).
2. The read-only view on `C-RET`, the fork on `M-RET`, and a `just perf` flow over a generated
   2 GiB tree with the budgets above.

## Follow-up: what a session's start logs

The window run of step 2 found that a session adopted or forked from a transcript took its name
from the transcript's title, which then reached the log twice: in "Started session NAME" and in
the spawn's log line, with its `--name`. The author chose (2026-11-17, through the coordinator):

- **"Started session NAME with SHELL in DIR" goes to `*Messages*` only,** with no log line
  (`Editor::note_unlogged`), and `session-adopt`'s "Adopted NAME from DIR" and "Session NAME is
  oso's already" are unlogged messages. The name stays the session's buffer's.
- **`oso_platform::process::Spawn` gains `args_logged`.** With it false, the spawn's log line
  and `Debug` give the executable, how many arguments there are and the directory, never their
  text. A hosted session's start sets it false, through `oso_term::Start::args_logged`: its
  arguments carry its name, its brief and the project's instructions. Every other spawn keeps
  its arguments in the log. The public field is approved by the author, with no ADR of its own.

## Changes to accepted ADRs

- **[ADR 0121](0121-agent-sessions-and-spaces.md), "The bridge and the channel":** the row
  `session_search` of the session tools' table is dropped, and Q21 now covers
  `session_transcript` only. "Step 6, as built"'s 6c is `session_transcript` alone. Its "Cost"
  table's `session_search` row is replaced by this ADR's budgets.
- **[ADR 0143](0143-agent-keys-on-c-c-a-and-an-agents-menu.md), "`C-c a` opens the Agents
  menu":** the Go to group gains `/`, `session-search`.

## Consequences

- A session is found by what was said in it, in any project, whether oso hosted it or not, and
  `RET` brings it back as `session-adopt` would.
- The person sees every project's conversations in one list. The label always says which scope
  is searched. A screen being shared shows other projects' text in the preview.
- Prompts and replies can still quote a secret the person pasted or the agent repeated. That text
  stays on the screen and in memory, never on disk or with an agent.
- History reaches back only as far as Claude Code keeps transcripts.
- The search depends on Claude Code's transcript format, which is not a public interface: if a
  record's shape changes, its text is not found, not misread. The record scanner gets a fuzz
  target.
- Sessions in a linked worktree outside its repository's directory are not in the project scope.

## Alternatives considered

- **A `*Session search*` results buffer, grouped by conversation.** It persists, but is not live
  as one types and goes stale. Left for an export from the float later.
- **A filter in `*Spaces*`.** At 44 columns there is no room for the matched line or a preview,
  and the pane's update budget (0.5 ms) cannot hold a search.
- **Matching the raw bytes, as `rg` would.** Fast, but most hits would be tool results and
  attachments, and those hold the secrets this search leaves out.
- **An index on disk.** It would copy the conversations' text, and any secret in it, into oso's
  state directory. The text is about 1% of the transcripts, so holding it in memory is enough.
- **Matching line by line, as `project-search` does.** It misses words a line apart in one reply.
- **Searching tool inputs too.** Commands and paths would be found, but so would what the agent
  typed into files.
- **An agent tool, `session_search`, as ADR 0121 planned.** Refused by the author: every project's
  conversations would reach one agent.
