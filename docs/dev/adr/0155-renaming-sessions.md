# 0155. Renaming sessions

- Status: Accepted
- Date: 2026-11-18

## Context

A hosted session's name is set once, at its start ([ADR 0121](0121-agent-sessions-and-spaces.md),
Q1): oso cleans it, makes it unique in its directory, passes it with `--name`, keeps it in its
record and names the terminal buffer `*claude: NAME*`. Claude Code's own `/rename` changes the
name its `SendMessage` knows the session by, but not oso's record or the buffer.

- ADR 0121's tool table lists `session_rename` as "Change, metadata only", never built.
- [ADR 0132](0132-a-session-mode-for-hosted-sessions.md), Q4, left the person's `session-rename`
  for its own decision.
- [ADR 0143](0143-agent-keys-on-c-c-a-and-an-agents-menu.md) moved session mode off `C-c` and a
  letter, so the capital heads of the Agents menu (`K`, `P`, `A`) are `C-c a` and the letter
  only.
- [ADR 0136](0136-issue-spaces.md), Q4: a space's orchestrator is named after the space, so that
  its workers reach it with `SendMessage (to: SPACE)`. A replaced one is renamed
  `<space>-was-<n>` in the record.
- No oso tool may type into a terminal (ADR 0121), so oso cannot change a running `claude`'s own
  name. A resume passes `--name` from the record again.

## Decision

The author chose on 2026-11-18, through the space's orchestrator: (a) for each question below.

### What changes (Q3)

- **oso's record and the buffer change at once:** the session's name in `*Spaces*`,
  `session_list` and the record, and its buffer, `*claude: NEW*`, or `*claude: NEW*<2>` when
  another buffer holds that name. The buffer keeps its id, so nothing that holds it moves.
- **A running `claude` keeps the name Claude Code knows it by** until its next resume, which
  passes `--name NEW` from the record. The echo area and the tool's reply say so: "its claude
  answers to "OLD" until it is resumed; /rename NEW in its terminal changes that now". They also
  say how many sessions it started, which were told to report to the old name.
- **oso keeps the name a running `claude` was started with** (`Hosted::launched_as`) once a
  rename changed the record's, until its next start or resume, and carries it over a restart.
- **`messaging_name`** in `session_list` and `session_state` is, while the session runs, the
  name Claude Code's registry gives it, as ADR 0121 step 3's poll reads it, else the name it was
  started with; for a session that does not run, the record's, which its next start or resume
  passes. It was the record's name always. Any process of the person's may write the registry,
  so its name counts only from the entry whose process is the session's own `claude`, oso's
  child, and only when it is a clean name.
- `session-follow-message` finds a renamed running session by that name too, as its messages
  come from it.
- oso does not follow Claude Code's `/rename`, and never types `/rename` itself.

### The name (Q1, Q2)

- **Cleaned as a typed name is** (`identity::clean_name`): ASCII letters, digits, `.`, `_` and
  `-`, at most 48 bytes, not starting or ending with `-` or `.`. The person's name is cleaned,
  and the echo area shows the result; one with nothing left is refused. An agent's is refused
  unless cleaning leaves it as it is, and the refusal gives the cleaned form, so the agent knows
  the name kept.
- **No clash, ASCII case aside (Q2).** Refused: a name another session has (hosted, remembered,
  archived, or held for a start), another program's session's, the name another of oso's
  running sessions was started with or the registry gives it, any space's name, and
  `<space>-was-<n>` of any space. A
  worker renamed to a space's or a sibling's name could otherwise take that session's messages
  after its next resume. Its own name, unchanged, is refused as "named so already".

### Who renames what (Q4, Q6)

- **A space's current orchestrator is never renamed (Q4),** by the person or an agent: its name
  is the space's, which the space's sessions message. "space-rename renames the space." Which
  session that is, is the record's word, failing closed: a closed space's orchestrator too, and
  one whose directory no longer passes the checks that give it its part.
- **The person** renames any other hosted session, earlier orchestrators among them. Another
  program's session, which oso only lists, is refused.
- **An agent (Q6)** renames its own session, the sessions it started (the record's parent), and,
  as a space's current orchestrator, its space's sessions, as `session_resume` reaches them
  ([ADR 0153](0153-agents-resume-sessions.md)). It never renames an orchestrator, current or
  earlier. It finds the session by id or name among those it sees, as every session tool does.

### The tool (Q5)

- **`session_rename { session, name }`**, `Change`, no question, as ADR 0121's table says:
  metadata only, and nothing runs. Not for a connection that is no hosted session.
- It counts toward the loop guard, by the session's id, once the session is found and the name
  is clean, before the clash check, so that probing which names are taken counts too. Its
  reply: the id, `name`, `was`, `messaging_name` and `note`.
- The echo area and `*Messages*` say `Agent: session "lead" renamed session "wa" to "api-v2"`,
  with the note; `*Agent*` logs `session_rename: from lead`, then `session_rename: renamed`.

### The person's command and keys (Q7)

- **`session-rename` (`Person`)** works on the session on the row at point in `*Spaces*`, else
  on the session whose terminal is the current buffer, as `session-stop` does. Elsewhere: "Not
  in *Spaces* or a session's buffer".
- It checks the rules first, then opens the prompt of `session-rename-to` (`Person`, history
  `session-name`, as `C-u claude`'s name), holding the session's name now. `M-p` empties it, as
  `review-filter`'s prompt does. The rules and the clash are checked again on the answer.
- **Keys:** `R` on a row of `*Spaces*` (`r` resumes), and `C-c a R`, "rename…", in the Agents
  menu's Session group, live everywhere as `K` is. On a space's, repository's or worktree's row,
  `R` says "R renames a session; the space menu's n renames a space". Session mode gets no key of its own: `C-c`
  and a letter is the user's (ADR 0143), and the capital heads are `C-c a` and the letter.

### Code

- `oso-core`: `RowAction::Rename`, `SessionRequest::RenameTo(String)` and
  `Editor::rename_terminal_buffer(id, base) -> Option<String>`, which shares
  `add_terminal_buffer`'s `<n>` rule, now one pass over the buffers.
- `sessions::identity::{new_name, same_name}`, `sessions::spaces::orchestrator::is_earlier_name`,
  and in `sessions::tools` the table's eleventh tool, `Renamable`, `renamable`, `rename_clash`,
  `renamed_said` and `rename_note` (`sessions/tools/rename.rs`), each pure, with tests.
  `Table::held_names`, `Hosted::launched_as`, and `launched_as` in a carried session's state.
- `channel::protocol::sessions`: `Call::Rename { session, name }`, each at most 255 bytes, held
  as any call. `oso mcp` describes it.
- `app/sessions/tools/rename.rs`: the person's prompt and answer, the tool, and the rename
  itself; the prompt opens through `App::open_host_prompt`, as source control's do.
  `app/sessions/hosting/naming.rs` keeps the rename asked and the registry's names of oso's own
  running sessions, read from each poll while it runs (`Hosting::naming`), with a test.
  `App::messaging_name` is beside `session_json`.
- e2e: `space_tools::an_orchestrator_and_the_person_rename_the_sessions_of_a_space`.

## Questions

1. **The name's rules.**
   (a) Cleaned for the person; for an agent, refused unless already clean.
   (b) Cleaned silently for both.
2. **Clashes.**
   (a) Refused against every other session's name, Claude Code's names, spaces and earlier
   orchestrators' names, case aside.
   (b) `-2` added, as a start does within one directory.
3. **What changes.**
   (a) oso's record and the buffer now; `claude`'s name at its next resume; the line says so and
   gives the `/rename` hint; `messaging_name` from the registry.
   (b) As (a), and oso follows Claude Code's `/rename` through the registry.
   (c) As (a), and the person's command types `/rename NEW` into an idle, empty prompt.
4. **The current orchestrator.**
   (a) Refused for everyone; the person renames earlier orchestrators.
   (b) The person may, with a warning.
5. **Whether the tool asks.**
   (a) No, as ADR 0121's table says; the echo area, `*Messages*` and `*Agent*` say it.
   (b) Unless allowed, as ADR 0135.
   (c) Each time.
6. **Scope.**
   (a) Itself, its children, and a current orchestrator's space's sessions; never an orchestrator.
   (b) Itself, and a current orchestrator's space's sessions.
   (c) Any session of the caller's repository, as ADR 0121 has it.
7. **Keys and the prompt.**
   (a) `R` in `*Spaces*`, `C-c a R`; the prompt holds the name, `M-p` empties it.
   (b) The same keys; the prompt empty, the name on `M-n`, as LSP and branch renames.
8. **Where it is recorded.**
   (a) A new ADR amending 0121, 0132, 0136 and 0143.
   (b) An amendment inside 0121.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0121](0121-agent-sessions-and-spaces.md), the tools table, "`session_rename`,
  `session_pin`, `session_mark_read` | Change | metadata only": `session_rename` is built, under
  this ADR's rules; the other two are not.
- ADR 0121, step 6, "`messaging_name` (the `--name` oso gave its `claude` …)": the registry's
  name while the session runs, else the record's.
- [ADR 0132](0132-a-session-mode-for-hosted-sessions.md), Q4, "Renaming waits for its own
  decision", and the follow-up "a `session-rename` if Q4 leaves it out": `session-rename` is this
  ADR's, on `C-c a R`, not Q4 (b)'s `C-c R`.
- [ADR 0136](0136-issue-spaces.md), Q4, "Its name is the space's name": no rename changes a
  current orchestrator's name, and no session takes a space's name or `<space>-was-<n>`.
- [ADR 0143](0143-agent-keys-on-c-c-a-and-an-agents-menu.md), the Agents menu's Session group:
  a head `R`, `session-rename`.

## Consequences

- The person and agents can name sessions for what they do, and the buffer follows.
- Until a running session is resumed, oso's name and Claude Code's differ. `messaging_name`
  tells an agent the address that works, and the echo line tells the person how to make them
  agree.
- Children told to report to the old name lose that address once their parent is resumed under
  the new one; the line counts them, and the parent can tell them the new name.
- A prompt injection in a session can rename itself, its children or, in an orchestrator, its
  space's sessions, up to the loop guard, without a question. Names stay clean and unique, no
  orchestrator's or space's name can be taken, and the echo area says each rename.
- These name rules are a rename's; how a start or a space names itself is unchanged.
- The record's parent says which sessions a session started. The record is in oso's state
  directory, which agents can write ([ADR 0136](0136-issue-spaces.md) notes the same), so a
  forged parent lets a session rename another session, never an orchestrator and never to a
  name in use.

## Alternatives considered

- **Follow Claude Code's `/rename` through the registry (Q3 b):** not verified that `/rename`
  updates the registry entry, and any process of the person's can write that entry, which would
  rename sessions past the tool's scope.
- **Type `/rename NEW` into the session (Q3 c):** ADR 0121 lets no tool type into a terminal,
  and a prompt with a draft or a dialog open would take it wrongly.
- **Number a clash as starts do (Q2 b):** a start's names are unique within one directory, but
  `SendMessage` addresses are global; a silent `-2` would also hide that the name is taken.
- **Let the person rename the current orchestrator (Q4 b):** its workers would lose their
  address at its next resume; `space-rename` and a replacement change the space's name instead.
- **Ask before each agent rename (Q5 b, c):** ADR 0121 put it as metadata only; nothing runs,
  and the clash rules keep addresses.
