# 0159. `space-new` starts the orchestrator, and a space's display name

- Status: Accepted
- Date: 2026-11-19

## Context

[ADR 0136](0136-issue-spaces.md) made a space from a title the person types, named it with a
slug made from that title (lowercase ASCII letters, digits and `-`, at most 48 bytes), and left
its orchestrator to `o` (`space-orchestrator-start`, the author's choice on 2026-11-15). Using
it, the author found three things:

1. **The buffer `C-x a O` ran from seemed taken into the new space.** It was not: the session's
   record kept its space, and `*Spaces*` listed it there. But `space-new` entered the new space
   with a window tree made from the buffer shown when the key was pressed (`Spaces::showed`
   with that buffer, then `Spaces::enter`), so the new space's one window showed that session,
   and the space remembered it as the buffer it showed last, for every later entry. ADR 0136's
   join rule ("a session started outside an orchestrator joins the space in front") places only
   sessions with no space yet, and `space-new`'s repository pick takes nothing in.
2. **Two keys for one act.** Making a space and starting its orchestrator were `C-x a O` and
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

### The flow of `C-x a O`

- **`space-new` reads one text: a description** of what the space is for, free prose. ADR 0136's
  title is renamed the description everywhere a person or an agent reads it: it is what the
  space is for, not a name.
- The space is made with the selected buffer's repository (ADR 0136, step 1b, P1), entered, and
  its orchestrator started once its directory is made: the person's `C-x a O` is the key that
  starts it, and the start goes the way `o` goes, with ADR 0136's checks before a start, trust by
  path and identity for each `--add-dir`, and [ADR 0135](0135-agents-start-sessions-without-asking.md)
  as for `space-orchestrator-start`.
- **The new space shows `*scratch*` alone (Q1)** until its orchestrator's buffer replaces it. The
  buffer shown before is in no window of the new space, and the space does not remember it.
  An orchestrator whose start comes after the person left its space starts behind, its terminal
  not shown, as an agent's start is (ADR 0144); the same holds for a successor (ADR 0136,
  step 5).
- **No trusted member (Q2):** the orchestrator starts with no `--add-dir`, as `o` does; ADR
  0136's offer pastes `/add-dir` once a member is trusted.
- **A refused directory or start:** the space stays, with no orchestrator. The echo area says
  why; `o` starts it once that is fixed.
- **A description from a buffer that may hold secrets** (oso-core's `answer_secret`) is refused,
  as its orchestrator is given it and agents read it.

### Three names

- **The slug** (`Stored::name`, the record's `name`) stays what ADR 0136 made it, everywhere it is
  used now: the space's directory, its orchestrator's name and address, `<slug>-was-<n>`, ADR
  0155's clash set, `space_status`'s `space`, and the space an agent names in `move_to_space`.
  **It never changes once the directory is made.** Before that, ADR 0136's P3 still lets a name
  taken on the disk give `-2`.
- **A new space's slug is no session's name (a fix):** besides every space's slug, it is unlike
  every name a session oso knows answers to, hosted, being started, the one a running `claude`
  was started with or the registry gives it, and other programs' (ADR 0155's set, ASCII case
  aside), and no space's `<slug>-was-<n>`. Session names are unique only in their directory
  (ADR 0121), so a space named as a worker was gave two sessions one address. The name P3 takes
  from the disk passes the same check.
- **The display name** (`Stored::display`, the record's `display`) is what the person sees: free
  text, at most 100 bytes, cleaned by `stored::clean_display`: the clusters oso-core's cell grid
  draws as text, so that an emoji's joiners, selectors and tag characters stay (`is_hidden_extend`'s
  rule); every control, bidi control and other invisible character dropped; runs of white space
  one blank; cut between clusters. Cleaning it again changes nothing, and the record's reader
  refuses a line whose display name is not clean, as it does a description (ADR 0136, P5).
  Empty, the slug is shown. **Display names are unique (Q5)** among the spaces oso keeps, ASCII
  case aside: a new space's gets ` 2` and on, and `space-rename` refuses a taken one.
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
    shell's `PATH`), run through `oso_platform::process` on a thread of its own: `-p
    --safe-mode --tools= --strict-mcp-config --disable-slash-commands --no-session-persistence
    --model=haiku --output-format=json --json-schema=SCHEMA --system-prompt=FIXED`, the
    description on stdin, in an empty private directory under oso's state directory; at most
    64 KiB read, killed after 20 s and when oso quits. No permission flag is given (ADR 0121):
    with no tool, nothing is asked. `--bare` is not used: it reads no OAuth login or keychain;
  - the space is made and entered at once, named when the answer comes; nothing that needs the
    slug (the directory, the orchestrator) happens before. A quit before it keeps the fallback;
  - what comes back is untrusted: a display name cleaned and made unique, a slug taken only
    when it is a slug as `slug()` leaves it with a letter, and made unique as above; each field
    that fails takes the fallback's, and the echo area says why;
  - `[spaces] naming = "claude" | "local"` in `config.toml`, `claude` by default;
  - no confirmation: the echo area names the space, and `n` renames it.

### Where each name is shown

- **The display name:** the space's and the archived spaces' rows in `*Spaces*`, the selected
  window's modeline (the space in front, dim, cut to 20 cells, after the project), `space-switch`
  and `space-reopen`'s candidates (the annotation is the slug where it differs, and the
  description), the echo area, the sessions' list annotations, `session-search`'s scope label,
  and `move_to_space`'s question beside the slug the agent gave.
- **An answer to `space-switch` or `space-reopen`** is a display name, else a slug.
- **Agents (Q6):** `space_status` gains `display_name` and `description`, said to be the person's
  words in a record agents in oso can write too; `session_list`'s `space` stays the slug. The
  orchestrator's header does not carry the display name: it is fixed at the first request, and a
  rename would leave it stale.
- **`*Spaces*` counts cells by cluster,** as the cell grid draws them, so an emoji sequence takes
  the two cells it is drawn in, and a cut never splits a cluster.

### The record

- `spaces` writes `description`, and the description cut to 200 bytes as `title`, which a build
  before this one reads and keeps; and `display`, which such a build leaves out. A line with no
  `description` takes `title` as its description. A line with no `display` shows its slug, so
  nothing is migrated: existing spaces show their slug until renamed.
- The file's cap goes from 256 KiB to 512 KiB, room for 256 spaces each with a whole
  description. A build before this one refuses a larger file, and replaces nothing.

### Public items

- oso-core: `Status::space` and `Editor::set_front_space` for the modeline; `SpaceRequest::New`
  and `SpaceRequest::Rename` carry a description and a display name.
- No new crate, crate edge, dependency, public trait or module, and no new agent tool.

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
- [ADR 0140](0140-a-spaces-pane-with-sections-and-guides.md), "A space's row is a section header:
  the fold glyph, the name": the display name.
- [ADR 0151](0151-moving-a-session-into-a-space.md), "The agent gives only the space's name. It
  must be an open space's name exactly": its slug; the question shows the display name beside it.
- [ADR 0155](0155-renaming-sessions.md), Q4, "space-rename renames the space": it renames the
  display name; the slug a current orchestrator answers to never changes.

## Consequences

- One key makes a space and starts the session the person talks to; the buffer they were in
  stays where it was.
- A space can be called anything, and renamed at any time, without moving its directory,
  renaming its orchestrator or changing what workers message.
- A space's slug is fixed by its description's first words or its repository; step 2 makes it
  readable for a description in any language. A poor slug stays: the person never types one.
- Agents see the display name, which agents can also write through the record: it is given as
  untrusted text, and agents name spaces by slug.
- A build before this one, sharing the state directory, shows titles cut to 200 bytes, and drops
  a display name when it changes that space.
- Typing Japanese in the description and rename prompts needs ADR 0158 (Japanese in prompts,
  open in its own pull request).

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
