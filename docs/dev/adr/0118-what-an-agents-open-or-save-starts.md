# 0118. What an agent's open or save starts

- Status: Accepted
- Date: 2026-11-12

## Context

The roadmap puts in M3: "Whatever opening or saving starts, such as a format on save, waits in a
buffer the agent channel opened or saved until the person acts there" ([roadmap](../roadmap.md)).
Earlier ADRs set the rules this builds on:

- **A channel action never starts a process by itself (M3 onward).** "What opening or saving a
  file would start, such as a language server or a format on save, waits in a buffer the channel
  opened or saved until the person acts in that buffer from the window"
  ([ADR 0085](0085-agent-channel-and-oso-mcp.md)). Anything that spawns a process is `Person`, so
  the channel never runs such a command; this rule is about what an allowed `open` or save starts
  on its own.
- The channel's `open` visits an existing file in the selected window, and is `Look`. A channel
  save writes without asking only a buffer the person visited; any other asks the person with an
  `Agent:` question, answered from the keyboard only. A buffer the channel opened is marked
  (`Buffer::opened_by_agent`), the mark never clears, and `oso dev` carries it
  ([ADR 0085](0085-agent-channel-and-oso-mcp.md), [ADR 0090](0090-oso-dev-carries-unsaved-text.md),
  [ADR 0094](0094-buffers-already-open-and-switching-for-the-agent.md)). The save the person's `y`
  lets go counts as the agent's.
- **Whitespace cleanup runs for the agent's saves too:** "it changes text oso already has, starts no
  process, and ADR 0085's rule about what a save starts does not apply"
  ([ADR 0113](0113-query-replace-keyboard-macros-and-whitespace-cleanup.md)). The final newline is
  an edit of every save ([ADR 0059](0059-safe-save-and-the-exit-guard.md)). A large cleanup scans on a
  worker, and the agent's reply waits for the save's end (`take_agent_trim_saves`).
- **EditorConfig is read on the loader thread on every visit, in every project:** it is declarative
  and runs nothing ([ADR 0115](0115-indentation.md)). Tree-sitter highlighting runs on workers for
  every buffer that cannot hold secrets ([ADR 0111](0111-tree-sitter-highlighting.md)).
- **Projects open restricted.** Trusted mode adds, from M4, what runs a project's code: language
  servers, formatters and format on save, git write operations and org babel. M4's first spawn asks
  `Editor::project_trusted`, and checks the root's device and inode again on its worker
  ([ADR 0009](0009-project-trust.md), [ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)).
  Git status is read in-process and runs no configured command ([ADR 0011](0011-git-integration.md)).
- **Release builds cannot spawn today.** `oso-platform::process` is behind the `dev` feature, and
  `just release-safety` checks that its marker is in no release binary
  ([ADR 0047](0047-process-spawning-and-restart-handoff.md)). In dev builds only the supervisor of
  `oso dev` spawns.

So in M3, nothing that opening or saving starts runs a process or a project's code, and the rule
has nothing to hold yet. It must be settled before M4's first program does: a formatter, a language
server, or a server told of a save, which for rust-analyzer runs `cargo check` and with it the
project's `build.rs` and proc macros.

The threat is ADR 0085's: an agent steered by text it read. Through the channel it can edit a file
the person has open, such as `build.rs`, and save it without a question. If that save started
`cargo check`, the channel would have run the project's code, now the agent's, with no one at the
keyboard.

### What other editors do

- **Emacs** runs `before-save-hook` and `after-save-hook` for every save, whoever called
  `save-buffer`; a client of `emacsclient --eval` or an MCP server in Emacs starts what a save
  starts. Eglot and apheleia start their servers and formatters the same way.
- **VS Code** does not format on save when the save came from auto save after a delay, or while the
  editor shuts down (`editor.formatOnSave`): a save with no person behind it skips the formatter.
  Workspace Trust turns off tasks, debugging and extensions that ask for trust in restricted mode.
- **Claude Code** edits files through its own tools; an editor sees them as changes on disk. Its own
  permission rules decide whether it runs a formatter or `cargo` itself.

## Decision

The author chose on 2026-11-12, through the coordinator, the first option (a) of every question
under [Questions](#questions), which the text below describes. The steps are under [Steps](#steps).

**M3 has nothing to hold.** Nothing that opening or saving starts in M3 runs a process or a
project's code, and the release build cannot spawn. This ADR settles the rule now, and its code
comes with M4's first `Program` kind, before that kind spawns.

### What runs, and what waits (Q1)

- **oso's own work in its process runs for the agent's open or save as for the person's:** the
  final newline and the whitespace cleanup (ADR 0113), EditorConfig and its later
  `trim_trailing_whitespace` and `insert_final_newline` (ADR 0115), highlighting (ADR 0111), finding
  the project and reading the trust record (ADR 0114), watching the file for changes on disk
  ([ADR 0083](0083-reloading-files-changed-on-disk.md)) and, in M4, git status read with `gix`.
  None of it starts a process or runs a project's code, and none of it reaches past what oso already
  reads.
- **What starts a program waits:** a formatter and format on save, starting a language server, and
  later any command that the configuration runs on open or save, when that command is not `Look` or
  `Change`.
- **So does what asks a program already running to work on the buffer:** a running server learns of
  a buffer the agent opened (`didOpen`) only when the person acts there, and of an agent's save
  (`didSave`) only when the person saves. Edits in a buffer the server already knows reach it as
  always (`didChange`), since a server cannot follow a buffer without them.
- **The agent's save itself is never held:** the text is written as soon as the save's own rules
  allow, without what waits. The agent's reply says what waits.

### How each one says which (Q2)

- **One enum in `oso-core` lists what opening or saving starts,** each kind with a required
  `Runs`, with no default, as each command has a reach (ADR 0085):
  - `Runs::InOso`: oso's own work, which runs for the channel;
  - `Runs::Program`: a process, or a request to one, which waits.
- **`Editor::starts_on_open` and `Editor::starts_on_save`,** given the buffer and who acted, return
  what runs now. Every M4 producer goes through them; for the channel they return only `InOso`
  kinds, and mark the buffer for the rest.
- A guard test lists the `Program` kinds, as the `Person` commands are listed, so a new kind shows
  in review.
- Nothing calls `oso-platform::process` for an open or a save outside what these return; the
  architecture check that keeps spawning in the process module stays as it is.

### What a buffer keeps (Q4)

- **Two marks per buffer:** an agent's open waits, and an agent's save waits. Each is set only when
  something `Program` would run now for the person, with the settings and trust as they are.
- **What runs is worked out again when a mark is let go,** from the settings, the trust record and
  the root's identity as they are then, so a mark holds no stale program, path or argument.
- A mark names the kinds it holds, at most one of each, for the modeline and the channel's state.

### What lets a mark go (Q3, Q11, Q12)

- **What an agent's open holds runs on the person's first command from the window in that buffer:**
  a key or a click that runs a command and leaves that buffer in the selected window. Switching to
  it with `C-x b`, `C-x o` or a click counts. A replay of a macro the person started counts, as its
  keys are the person's. In dev builds `send-keys` counts, as for the channel's other waits.
- **What an agent's save holds runs on the person's next save of that buffer,** with everything that
  save starts, as for any of the person's saves. `C-x C-s` in a buffer with a save waiting saves even
  when the buffer is unmodified, rather than saying "(No changes need to be saved)", so a format on
  save runs without an edit first.
- **The person's `y` to an `Agent:` question about a save lets nothing go:** the question asked
  about writing the file, not about running a program, and the save is the agent's, as the app
  already treats it.
- Not a mark's release: a command the channel runs, a wheel scroll, the window gaining focus, a
  restart, an emergency save, or a reload from disk.

### What the person sees (Q5)

- **The buffer's modeline shows `waits` in the dim face, then what waits:** `waits: format`,
  `waits: server`, or both. It follows `restricted` and clears when the mark is let go. It is drawn
  from the marks alone, as `restricted` is from the record.
- **`*Agent*` logs each hold and its release:** time, the buffer's name and the kinds, never text.
- **The echo area says nothing when a mark is set;** what runs on its release reports as it would
  for the person's own open or save.

### What the agent sees (Q6)

- **The reply to `open` or to a save says what waits:** "saved; format on save waits for the person
  in main.rs". A save that waits for nothing replies as today.
- **`state` lists, for each buffer, what waits** (`waits: ["format"]`), and its schema version goes
  up by one.
- **The agent cannot let a mark go,** and opens no question about it. The `oso mcp` tool
  descriptions say what waits and why, so the agent can tell the person in its own conversation, or
  run the formatter through its own tools under its own permission rules.

### Trust, secrets and restarts (Q7, Q8)

- **A restricted project holds nothing,** since nothing `Program` runs there for the person either,
  and its modeline already says `restricted`. Untrusting a project clears its buffers' marks.
  Trusting one does not run what an earlier agent's open or save would have started.
- **A buffer that may hold secrets adds no rule here.** The channel acts in no buffer while one is
  shown, and opens no file the patterns match. What runs for such a buffer when a mark is let go is
  M4's rule for it, the same as for the person's open or save.
- **`oso dev` carries the marks,** as it carries `opened_by_agent` (ADR 0090): an optional field in
  the buffer's entry, and a restart lets nothing go. Emergency copies carry nothing.

### Cost

- Setting a mark reads settings and the trust record oso already holds: O(1), no I/O. Each command
  from the window checks the selected buffer's two bits: O(1) per key, nothing allocated.
- What runs on release starts on its own worker, as it would for the person; the UI thread never
  waits for a program, a server or a file.

### When it is built (Q10)

- **Now: this ADR and the rule in `security.md`, `architecture.md` and the roadmap.** M3 has
  nothing to hold, and its release build cannot spawn.
- **The code is the first step of M4's first `Program` kind,** format on save or language servers,
  before that step spawns (see [Steps](#steps)).
- **Tests:** unit tests for each rule above as plain functions of `Editor`; end-to-end flows over
  the channel's socket for an agent's save that holds a format and the person's `C-x C-s` that lets
  it go, for an open that holds a server and the first key that lets it go, and for a restricted
  project that holds nothing.

## Questions

Put to the author on 2026-11-12, the recommended option first; each answer was the first, (a).

### What waits

1. **Which actions wait.**
   (a) By what they do: oso's own work in its process runs for the agent as for the person (final
   newline, whitespace cleanup, EditorConfig, highlighting, the project and its trust, reload
   watching, git status); what starts a program, or asks a running one to work on the buffer
   (`didOpen`, `didSave`), waits.
   (b) Everything that opening or saving starts waits, in-process work included, so an agent's save
   writes its text as it is. One rule, but ADR 0113's cleanup then differs by who saved, and the
   person's next save makes a diff the agent did not make.
   (c) Only starting a process waits; requests to a server already running go as for the person.
   Simpler, but rust-analyzer's check on save would then run a `build.rs` the agent just saved.
2. **How each one says which.**
   (a) One enum in `oso-core` of what opening or saving starts, each kind with a required `Runs`
   (`InOso` or `Program`), read through `Editor::starts_on_open` and `starts_on_save`, with a guard
   test listing the `Program` kinds.
   (b) Each M4 call site checks who acted itself. No new type, but each new feature must remember.
   (c) As (a), and also a value only the window's input path can make, required to spawn for an open
   or a save. Checked by the compiler, but it changes `oso-platform::process`, whose callers today
   are the dev supervisor and tests; it can come with M4 if (a) proves leaky.
3. **What lets a mark go.**
   (a) Of the same kind as what set it: an open's on the person's first command from the window in
   that buffer; a save's on the person's next save of it, which `C-x C-s` makes even when the buffer
   is unmodified.
   (b) The person's first command in that buffer lets both go; a held format then runs and writes the
   file again, as the person's save. One release, but a key such as `C-n` writes a file.
   (c) Only an explicit `Person` command, `run-waiting`, in that buffer. Nothing runs by surprise, but
   the person has a new command to remember, and a server never starts until they do.
4. **What a mark holds.**
   (a) Two marks per buffer naming the kinds; what runs is worked out again when one is let go, from
   the settings and trust as they are then.
   (b) A queue of the actions as they stood at the agent's open or save, with their programs and
   arguments. Exact, but stale after a settings change or untrusting, and more to carry and check.

### What the person and the agent see

5. **What the person sees.**
   (a) A dim `waits: format` (or `server`) in that buffer's modeline, after `restricted`, and a line
   in `*Agent*` for each hold and release; nothing in the echo area.
   (b) As (a), and an echo area message when a mark is set. Easier to notice, but it covers what the
   person was reading, while the agent saves often.
   (c) As (a), and a `*Waiting*` buffer listing every buffer with a mark. Useful with many buffers,
   but one more internal buffer for what the modelines already say.
6. **What the agent sees, and whether it can ask the person.**
   (a) The reply to `open` or a save says what waits, `state` lists it per buffer, and the agent opens
   no question: it tells the person in its own conversation, or runs the formatter itself under its
   own permission rules.
   (b) As (a), and the channel may open `Agent: run format on save for main.rs? (y or n)`. One key for
   the person, but a question after every agent save is one people learn to answer `y` without
   reading.
   (c) A channel method that lets a mark go. The agent would then start what the rule holds.

### Trust, restarts and edges

7. **Trust.**
   (a) A restricted project holds nothing, since nothing would run for the person; untrusting clears
   its buffers' marks; trusting runs nothing an earlier agent's open or save would have started.
   (b) Hold in restricted projects too, so trusting the project and then acting in the buffer runs
   what waited. Nothing missed, but the modeline would show `restricted` and `waits` for things that
   cannot run.
8. **The restart handoff.**
   (a) `oso dev` carries the marks, as it carries `opened_by_agent`; a restart lets nothing go.
   (b) Not carried: after a restart, the buffers count as the person's, and a held server starts. Less
   to carry, but a restart the agent caused through the dev socket would let go what the rule holds.
9. **What counts as acting from the window.**
   (a) A key or click that runs a command and leaves that buffer in the selected window, a replay of a
   macro the person started included; not a wheel scroll, focus, or a command the channel runs.
   (b) Any input event in a window showing the buffer, wheel scrolls and focus included. Lets go
   sooner, but a scroll over a window the person is not looking at would start a server.
10. **When it is built.**
    (a) This ADR and the docs now; the code as the first step of M4's first `Program` kind, before it
    spawns. M3 has nothing to hold, and its release build cannot spawn.
    (b) Build it now, with a dev-only stand-in kind that the end-to-end flows hold and let go. The
    machinery is tested before M4, but it is code with no real user, and the stand-in's shape may not
    fit a language server's.
    (c) Build the enum and the marks now, and the modeline and the replies with M4. Less dead code than
    (b), but the parts are tested apart.
11. **The person's `y` to an `Agent:` question about a save.**
    (a) Lets nothing go: the question asked about writing the file, and the save stays the agent's,
    as the app treats it today.
    (b) Lets the save's marks go, and the question names what will run: `Agent: save main.rs? Then
    format on save runs (y or n)`. One key fewer, but the question asks two things at once.
12. **Whitespace cleanup on the agent's save.**
    (a) Keep ADR 0113's rule: it runs, as oso's own work.
    (b) Hold it like a format, so the agent's save writes exactly what it sent. Predictable for the
    agent, but the person's next save then cleans lines they did not touch.

## Steps

1. **Now, in M3 (this pull request):** this ADR, ADR 0085's status and the passage below, the rule in
   `security.md` and `architecture.md`, and the roadmap's M3 line pointing at M4. No code: M3 has
   nothing to hold.
2. **In M4, the first step of the first `Program` kind** (format on save or language servers),
   before it spawns anything:
   - `oso-core`: the enum of what opening or saving starts with its `Runs`, `Editor::starts_on_open`
     and `starts_on_save`, the two marks with their kinds, their release by the person's command
     and save, clearing on untrust, and `C-x C-s` saving an unmodified buffer with a save waiting;
   - `oso`: the release check in the dispatcher for commands from the window, the `waits` modeline
     segment, the replies to `open` and to a save, `state`'s `waits` and its schema version, the
     `*Agent*` lines, the `oso dev` handoff entry, and the `oso mcp` tool descriptions;
   - a guard test listing the `Program` kinds, and the tests under [When it is built](#when-it-is-built-q10).
3. Each later `Program` kind declares its `Runs` and joins the guard test's list in its own step.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it. The status of each ADR named
gains `amended by 0118`.

- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "A channel action never starts a process by
  itself (M3 onward). What opening or saving a file would start, such as a language server or a
  format on save, waits in a buffer the channel opened or saved until the person acts in that
  buffer from the window": from M4, when the first program that opening or saving starts lands.
  What an agent's open starts waits for the person's first command from the window in that
  buffer; what an agent's save starts waits for the person's next save of it. oso's own work in
  its process runs at once, and the agent's reply and `state` say what waits.

## Consequences

- An agent's open or save starts no program and asks no running one to check the project, in any
  project, and the person sees in the modeline what waits for them.
- The rule is in place before M4's first program, and each M4 kind declares its side, which a guard
  test shows in review.
- An agent's save in a trusted project is not formatted until the person saves; the agent learns
  this from the reply, and its diffs may then differ from what format on save would write.
- A server the person started still sees changes on disk through its own file watching, and the
  agent's own tools can run any program its permission rules allow. The rule covers what oso starts,
  not those.
- With `open` showing the buffer in the selected window, the person's next key there lets an open's
  mark go even if they did not notice the buffer change; the `waits` segment is the cue.
- `C-x C-s` in an unmodified buffer writes the file when a save waits there.
- Follow-up work, in M4: the code above with its first `Program` kind, and the rule for what runs for
  a buffer that may hold secrets.

## Alternatives considered

- **Refusing an agent's open or save in a trusted project with a format or server configured:** the
  agent could then not save the files the person asked it to change.
- **Running what waits on a timer once the person is back at the keyboard:** "back" is any key
  anywhere, not presence in that buffer, and a program would start at an unpredictable time.
- **Formatting the agent's text before it is written, in-process:** there is no in-process formatter,
  and a program would still run.
- **Holding the agent's save itself until the person acts:** the agent's edits would not reach the
  disk, and its tools that read files would see old text.
- **Treating a save the person visited as the person's, so its format runs:** the visit was earlier;
  the program would still start with no one at the keyboard.
