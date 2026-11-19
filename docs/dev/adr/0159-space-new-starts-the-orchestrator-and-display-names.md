# 0159. `space-new` starts the orchestrator, and a space's display name

- Status: Accepted
- Date: 2026-11-19

## Context

[ADR 0136](0136-issue-spaces.md) made a space from a title the person types, named it with a
slug made from that title (lowercase ASCII letters, digits and `-`, at most 48 bytes), and left
its orchestrator to `o` (`space-orchestrator-start`, the author's choice on 2026-11-15). Using
it, the author found three things:

1. **The buffer `C-c a O` ran from seemed taken into the new space.** It was not: the session's
   record kept its space, and `*Spaces*` listed it there. But `space-new` entered the new space
   with a window tree made from the buffer shown when the key was pressed (`Spaces::showed`
   with that buffer, then `Spaces::enter`), so the new space's one window showed that session,
   and the space remembered it as the buffer it showed last, for every later entry. ADR 0136's
   join rule ("a session started outside an orchestrator joins the space in front") places only
   sessions with no space yet, and `space-new`'s repository pick takes nothing in.
2. **Two keys for one act.** Making a space and starting its orchestrator were `C-c a O` and
   then `o`, with nothing to do in between.
3. **The name.** The slug is the space's directory (`<root>/<slug>`), its orchestrator's
   `--name` and so its `SendMessage` address (ADR 0136, Q4; [ADR 0155](0155-renaming-sessions.md)),
   `<slug>-was-<n>` for earlier orchestrators, and the name agents give a space
   ([ADR 0151](0151-moving-a-session-into-a-space.md)). `space-rename` changed it, so a running
   orchestrator answered to the old name until its next start, and a title in Japanese gave
   `space`, `space-2` and on. The author wants a name to rename freely, in any script, with
   emoji, and the slug to stay put.

## Decision

The author chose on 2026-11-19, through the space's orchestrator: (a) for each question below,
the slug's fallback as written, and the split into two steps.

### The flow of `C-c a O`

- **`space-new` reads one text: a description** of what the space is for, free prose. ADR 0136's
  title is renamed the description everywhere a person or an agent reads it: it is what the
  space is for, not a name.
- The space is made with the selected buffer's repository (ADR 0136, step 1b, P1), entered, and
  its orchestrator started once its directory is made: the person's `C-c a O` is the key that
  starts it, and the start goes the way `o` goes, with ADR 0136's checks before a start, trust by
  path and identity for each `--add-dir`, and [ADR 0135](0135-agents-start-sessions-without-asking.md)
  as for `space-orchestrator-start`.
- **The new space shows `*scratch*` alone (Q1)** until its orchestrator's buffer replaces it. The
  buffer shown before is in no window of the new space, and the space does not remember it;
  with no space in front, the frame's tree, which is no space's, is replaced by `*scratch*`
  alone first. The orchestrator's terminal takes that window only while it still shows that
  `*scratch*`, and its space is in front; else it starts behind, its terminal not shown, as an
  agent's start is (ADR 0144), so that it never takes keys the person meant for something else.
  A successor whose start comes after the person left its space starts behind too (ADR 0136,
  step 5).
- **No trusted member (Q2):** the orchestrator starts with no `--add-dir`, as `o` does; ADR
  0136's offer pastes `/add-dir` once a member is trusted.
- **A refused directory or start:** the space stays, with no orchestrator. The echo area says
  why; `o` starts it once that is fixed. One directory job a space is out at a time: `o` or
  `space-directory` while it is says so, as a second would make another directory.
- **A description from a buffer that may hold secrets** (oso-core's `answer_secret`) is refused,
  as its orchestrator is given it and agents read it.

### Three names

- **The slug** (`Stored::name`, the record's `name`) stays what ADR 0136 made it, everywhere it is
  used now: the space's directory, its orchestrator's name and address, `<slug>-was-<n>`, ADR
  0155's clash set, `space_status`'s `space`, and the space an agent names in `move_to_space`.
  **It never changes once the directory is made**, and a space keeps the first directory made
  for it. Before that, ADR 0136's P3 still lets a name taken on the disk give `-2`, and
  `space-reopen` and the record's reader still tell two spaces of one slug apart with `-2`; a
  space with a directory keeps its slug there too, the first one read answering to it.
- **A new space's slug is no session's name (a fix):** besides every space's slug, it is unlike
  every name a session oso knows answers to, hosted, being started, the one a running `claude`
  was started with or the registry gives it, and other programs' (ADR 0155's set, ASCII case
  aside), and no space's `<slug>-was-<n>`. Session names are unique only in their directory
  (ADR 0121), so a space named as a worker was gave two sessions one address. The name P3 takes
  from the disk passes the same check, and so does the slug of a space a session's joining made
  (named after its repository, as that session often is) when its directory is first made.
- **The display name** (`Stored::display`, the record's `display`) is what the person sees: free
  text, at most 100 bytes, cleaned by `stored::clean_display`: the clusters oso-core's cell grid
  draws as text; every control, bidi control and other invisible character dropped, but a
  joiner and the text and emoji selectors inside a cluster that starts with an emoji, and the
  tag characters of the three subdivision flags that are emoji (England, Scotland, Wales), as
  any other one could carry text the person does not see; runs of white space one blank; cut
  between clusters. Cleaning it again changes nothing. The record's reader cleans a display
  name again rather than refusing its line, as what is drawn may change with oso's tables and a
  space must not be lost for its name. Empty, the slug is shown. **Display names are unique
  (Q5):** no two spaces oso keeps are called alike, nor is one called as another's slug or an
  earlier orchestrator's `<slug>-was-<n>`, compared with every invisible character dropped and
  ASCII case aside (`stored::skeleton`); a new space's gets ` 2` and on, and `space-rename`
  refuses a taken one.
- **`space-rename`'s prompt holds the display name** (the author's choice, 2026-11-19), as
  ADR 0155's `session-rename` holds a session's: `space-rename` asks the host, which opens the
  prompt of `space-rename-to` with the name in it, `M-p` emptying it, and its answer renames the
  space the prompt was opened for. An answer with no such prompt open renames nothing.
- **The description** (`Stored::description`) is at most 1 KiB, cleaned as ADR 0136's title was,
  up from 200 bytes. The orchestrator's header gives it, under ADR 0121's untrusted-text line, as
  it gave the title.

### Naming a new space

- **Step 1 (the fallback):** the slug from the description's first ASCII words, at most four,
  else from the first repository's name, then made unique (`oso-3`); the display name the
  description cut to 24 cells. The person never types a slug.
- **Step 2 (a summary by Claude):** oso asks the person's own `claude` for a short display name
  and a slug, and the fallback stays for when that fails:
  - the person's `claude`, found as a hosted session's is (`[sessions] claude`, else the login
    shell's `PATH`), run through `oso_platform::process` on a thread of its own: `--print
    --safe-mode --tools= --strict-mcp-config --setting-sources=user --disable-slash-commands
    --no-session-persistence --model=haiku --output-format=json --json-schema=SCHEMA
    --system-prompt=FIXED`, the description on stdin, in a new private directory of its own
    under oso's state directory, removed after it; at most 64 KiB read, and its whole session
    killed after 20 s, when the space is closed, and when oso quits or restarts. One run at a
    time: a space made while one is out takes oso's own names at once. No permission flag is
    given (ADR 0121): with no tool, nothing is asked. `--bare` is not used: it reads no OAuth
    login or keychain;
  - the space is made and entered at once, named when the answer comes; nothing that needs the
    slug (the directory, the orchestrator) happens before. A quit before it keeps the fallback;
  - what comes back is untrusted: a display name cleaned and made unique, a slug taken only
    when it is a slug as `slug()` leaves it with a letter, and made unique as above; each field
    that fails takes the fallback's, and the echo area says why;
  - `[spaces] naming = "claude" | "local"` in `config.toml`, `claude` by default; any other
    value is said and taken as `local`, as one written to keep descriptions from a model should
    fail closed;
  - no confirmation: the echo area names the space, and `n` renames it.

### Where each name is shown

- **The display name:** the space's and the archived spaces' rows in `*Spaces*`, the selected
  window's modeline (the space in front, dim, cut to 20 cells, after the project), `space-switch`
  and `space-reopen`'s candidates (the annotation is the slug where it differs, and the
  description), the echo area, the sessions' list annotations, `session-search`'s scope label,
  and `move_to_space`'s `?` details. The question itself quotes the slug the agent gave, whole,
  as an agent's text is quoted; the offer to paste `/add-dir` names the orchestrator by its slug,
  which it answers to.
- **An answer to `space-switch` or `space-reopen`** is a display name, else a slug.
- **Agents (Q6):** `space_status` gains `display_name`, with no invisible character, and
  `description`, said to be the person's words in a record agents in oso can write too;
  `session_list`'s `space` stays the slug, and `move_to_space`'s description says it takes the
  slug. The
  orchestrator's header does not carry the display name: it is fixed at the first request, and a
  rename would leave it stale.
- **`*Spaces*` counts cells by cluster,** as the cell grid draws them, so an emoji sequence takes
  the two cells it is drawn in, and a cut never splits a cluster. Segmenting costs several times
  a character's look, on every row of every spinner frame, so text with no character that can
  join another (ASCII, kana, ideographs, Hangul syllables: most text) is measured character by
  character, as before; a release test holds 256 rows of Japanese text and emoji to the 500 µs
  of the ASCII one.

### The record

- `spaces` writes `description`, and the description cut to 200 bytes as `title`, which a build
  before this one reads and keeps; and `display`, which such a build leaves out. A line with no
  `description` takes `title` as its description. A line with no `display` shows its slug, so
  nothing is migrated: existing spaces show their slug until renamed.
- The file's cap goes from 256 KiB to 512 KiB, room for 256 spaces each with a whole
  description. A build before this one refuses a larger file, and replaces nothing. Its lines
  are found by id through an index, so a file of many short lines costs one look a line.

### Public items

- oso-core: `Status::space` and `Editor::set_front_space` for the modeline (the author's
  approval, 2026-11-19); `SpaceRequest::AskRename` for `space-rename`'s prompt;
  `SpaceRequest::New` and `SpaceRequest::Rename` carry a description and a display name.
- No new crate, crate edge, dependency, public trait or module, and no new agent tool.

## Step 2, as built

- **The answer's shape,** read from `claude` 2.1.327 with one call on a made-up description (the
  author's leave, 2026-11-19): `--output-format=json` with `--json-schema` gives a JSON object
  whose `structured_output` holds the two names and whose `result` holds them again as JSON text,
  `is_error` false; the run left its directory empty. oso reads `structured_output`, else
  `result`, and refuses an answer with `is_error` true.
- **The run** is `sessions/spaces/summary.rs`: the command line and the answer's reading are
  plain functions with tests. The run finds the person's shell and launches `claude` as a
  session's launch does (`sessions::launch`, which now also holds which `claude` and which
  variables), with the names a session's terminal passes on but `SSH_AUTH_SOCK` and
  `XDG_RUNTIME_DIR`, as it has no tool and no MCP server, and sets none. It runs in a session
  with no controlling terminal, its error output thrown away, in `space-naming/<space id>` under
  the state directory, made new and private for it and removed after it: a directory already
  there is not used. It waits for `claude` itself, polling, and kills its whole session, so
  that nothing it started outlives it. The run is a `SpaceJob::Summarize`, which the worker runs
  on an `oso-space-naming` thread of its own; `Out::SpaceSummarized` brings the names back, also
  when the thread cannot start or the run panics, with a reason.
- **Its `Child` is shared** (`summary::Running`) with the app, which stops the run when the
  space is closed, and at an exit or a restart's handoff, before the records are written.
- **The registry:** a naming run in the state directory is left out of the external sessions,
  should `claude` enter itself in Claude Code's registry.
- **The space is made at once** with oso's own names and entered, and the echo area says
  `claude` names it. The answer gives the space its slug only while it has no directory and
  none is being made: `o` pressed meanwhile starts the orchestrator under oso's own slug. Its
  display name is taken only if the space is still called what it was when `claude` was asked,
  so a name the person gave meanwhile stays. A quit before the answer keeps oso's names, and `o`
  starts the orchestrator later.
- **What the echo area says:** `Space NAME (SLUG) named`, or `claude did not name the space
  (WHY): it is NAME (SLUG); n in the space menu renames it`, where WHY is a short fixed reason
  that holds no path and none of the description, such as no answer in 20 s, claude did not
  start or did not answer, its answer was too long, not JSON or held no names, or its directory
  is there already.
- **In the end-to-end flows,** every stand-in `claude` answers `--print` with the flow's
  `naming-answer`, or fails, and keeps what it was given apart from the session runs.
- **Checked against the real `claude`** (2.1.327, a second call with a made-up description, the
  author's leave, 2026-11-19), with the run's own command line in a directory planted for it: a
  `.claude/settings.json` and a `.claude/settings.local.json` whose `SessionStart`,
  `UserPromptSubmit` and `Stop` hooks would each leave a file, a `CLAUDE.md` there and another in
  the directory above, each telling it to answer with a name of its own. No hook ran, both names
  came from the description alone, nothing was written in the directory, and no transcript was
  kept for it. Hooks of the person's own settings are left out by `--safe-mode` as its help says;
  the call did not change the person's settings to check that.
- **An invalid `[spaces] naming`** counts as `local` (the author's choice, 2026-11-19).

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0136](0136-issue-spaces.md), "a name (a slug, …, unique among open spaces); a title (the
  issue in the person's words, at most 200 bytes, cleaned as names are)": a slug that never
  changes once the directory is made, unique among the spaces oso keeps and unlike every
  session's name; a display name; a description of at most 1 KiB.
- ADR 0136, "`space-orchestrator-start` (`o` in the space menu) starts it, in every space:
  `space-new` only makes the space (the author's choice, 2026-11-15)", the commands table's
  `space-new` row, and step 3's "`space-new` only makes the space": `space-new` reads a
  description, makes the space, enters it and starts its orchestrator; `o` starts it again.
- ADR 0136, the commands table's `space-rename` row, "the space's name and title; the
  orchestrator keeps its name until its next start": the display name alone.
- ADR 0136, step 1b, "`space-new` reads the title only": a description; and "Entered for the first
  time, it shows the buffer shown now": `*scratch*`.
- ADR 0136, Security, "**Names:** a space's name is a slug of the person's typing": the slug is a
  slug of the person's typing (step 2: or of a summary of it); the display name is cleaned text.
- ADR 0136, Q4, "Its name is the space's name": its slug's, which no rename changes.
- ADR 0136, "Stored in `spaces` … at most 256 KiB": at most 512 KiB.
- ADR 0136, step 1a, "`space-switch` lists open spaces, with the title, else the repositories'
  names, as the annotation": by display name, with the slug where it differs and the description,
  else the repositories' names.
- ADR 0136, step 1a, "two spaces of one name, made by two instances at once, are told apart with
  `-2` when read": only while the later has no directory; one that has one keeps its slug.
- ADR 0136, step 1b, "`space-rename` while the directory is made keeps the new name": the display
  name, which a directory made meanwhile does not change.
- ADR 0136, the space's header ("the name, the title, the issue reference"), step 3's "The title
  is given to the orchestrator as the space's title", and step 4a's `space_status` ("the space's
  name and title"): the description, and `space_status` also gives the display name.
- [ADR 0140](0140-a-spaces-pane-with-sections-and-guides.md), "A space's row is a section header:
  the fold glyph, the name": the display name.
- [ADR 0151](0151-moving-a-session-into-a-space.md), "The agent gives only the space's name. It
  must be an open space's name exactly": its slug; the question shows the display name beside it.
- [ADR 0155](0155-renaming-sessions.md), Q4, "space-rename renames the space", and "`space-rename`
  and a replacement change the space's name instead": `space-rename` renames the display name;
  the slug a current orchestrator answers to never changes.
- ADR 0155, "how a start or a space names itself is unchanged": a new space's slug, and a
  joined space's when its directory is first made, take no name ADR 0155's set holds.

## Consequences

- One key makes a space and starts the session the person talks to; the buffer they were in
  stays where it was.
- A space can be called anything, and renamed at any time, without moving its directory,
  renaming its orchestrator or changing what workers message.
- A space's slug is fixed by its description's first words or its repository; step 2 makes it
  readable for a description in any language. A poor slug stays: the person never types one.
- Agents see the display name, which agents can also write through the record: it is given as
  untrusted text, and agents name spaces by slug.
- A build before this one, sharing the state directory, shows descriptions cut to 200 bytes,
  and drops the display name of every space it writes from memory, as it keeps none.
- Typing Japanese in the description and rename prompts takes [ADR
  0158](0158-japanese-in-prompts-and-review-drafts.md)'s input method in text prompts.

## Alternatives considered

- **Merge the display name into the title:** one field fewer, but the title is the orchestrator's
  issue text, prose up to 200 bytes; the author chose a separate short name and a description.
- **Change the slug on rename, or on request:** Claude Code keys the orchestrator's memory by its
  directory's path, and workers message its name; a moved directory or a renamed orchestrator
  loses both.
- **Ask the person for the slug:** the author chose that the person never types one.
- **Romanize the description for the slug:** needs a dictionary dependency for kanji, and is
  wrong for names; step 2's summary does better.
- **A random slug:** not readable at a glance in `~/spaces` or in `SendMessage`.
- **Let the orchestrator name the space through a tool:** the slug is needed before it starts,
  and a tool would let an agent change what the person sees, for instance to pass for another
  space.
- **Enter the new space only once the orchestrator's buffer exists:** the frame would stay on
  the old space for an unknown time, and a refused start would leave the person nowhere.
