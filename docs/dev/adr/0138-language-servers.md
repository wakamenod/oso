# 0138. Language servers

- Status: Proposed
- Date: 2026-11-15

## Context

The roadmap's M4 holds "Language servers: diagnostics, go to definition, completion", and says
that ADR 0118's rule comes first, "with the first program that opening or saving starts"
([roadmap](../roadmap.md)). This ADR proposes how oso talks to language servers, with the
questions for the author under [Questions](#questions), each with the recommended option first.
Nothing here is built yet.

What is fixed already:

- **Trust.** Projects open restricted. Trusted mode adds language servers, formatters and git
  write operations ([ADR 0009](0009-project-trust.md)). A project is a directory with `.git`,
  `.jj` or `.hg`; it is trusted when its root's path, device and inode are in the record, and
  M4's first spawn asks `Editor::project_trusted` and checks the root's device and inode again
  on its worker. "Trust never makes oso read a project's settings. A program that runs for a
  trusted project is one the person's configuration names"
  ([ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)).
- **What an agent's open or save starts waits for the person** ([ADR 0118](0118-what-an-agents-open-or-save-starts.md)).
  Starting a server is a `Runs::Program` kind, and so are `didOpen` for a buffer the agent opened
  and `didSave` for the agent's save. `didChange` in a buffer the server already knows goes as
  always. ADR 0118's code is "the first step of M4's first `Program` kind", before it spawns.
- **Processes.** Only `oso_platform::process` spawns, in every build since the terminal
  ([ADR 0120](0120-the-terminal.md)). A program comes from an absolute path, or from `PATH`
  without the current directory, the project or a relative entry. The environment is an
  allowlist plus variables the caller names. A child gets only its standard streams, through
  `posix_spawn` on macOS and Linux, and can have a process group of its own
  ([ADR 0137](0137-starting-processes-with-posix-spawn.md)). Spawning reads the file system, so
  it never runs on the UI thread.
- **No network code. Only the `regex` crate** ([ADR 0010](0010-process-and-network-policy.md)).
- **Reach** ([ADR 0085](0085-agent-channel-and-oso-mcp.md)). Every command is `Look`, `Change`
  or `Person`. Anything that spawns a process, needs a person at the keyboard, or shows what a
  buffer that may hold secrets withholds is `Person`.
- **Configuration** ([ADR 0108](0108-the-user-configuration-file.md),
  [ADR 0110](0110-adr-0108-as-built.md)). One TOML file of the person's, read off the UI thread
  with owner and mode checks. Nothing in a repository is configuration, EditorConfig aside.
- **Buffers and edits.** A buffer has a `Mode` from its file name: Rust, Markdown and TOML are
  the languages oso knows ([ADR 0111](0111-tree-sitter-highlighting.md)). Every edit of a batch
  is a `Splice` in the old text's offsets, and the syntax worker turns a batch into
  tree-sitter's edits from the ropes before and after, highest first, O(log n) a splice. Ropes
  are cheap to clone. Up to 1000 cursors edit in one batch
  ([ADR 0091](0091-multiple-cursor-keys-and-m-y.md)).
- **Completion** runs in a centred float with the orderless matcher, on the UI thread up to
  5000 candidates and on a worker past that ([ADR 0076](0076-completion-and-m-x.md),
  [ADR 0096](0096-completion-on-a-worker-past-5000-candidates.md)). A command with a preview
  splits the float ([ADR 0088](0088-switching-and-killing-buffers.md)). There is no completion
  in the buffer yet.
- **Places.** Each cursor has a mark ring, and a jump pushes the place it left
  ([ADR 0116](0116-migemo-and-jumps.md)). There is no history of places across buffers. `M-,` and
  `M-/` are `previous-buffer` and `next-buffer` (ADR 0088), not xref's keys.
- **The gutter** is a region of the window's layout, and "signs, such as a diagnostic's, can be
  another [column] later" ([ADR 0097](0097-visual-aids.md)). The display model keeps a layer for
  diagnostics: "LSP errors and warnings, underline styles and gutter signs"
  ([display model](../display-model.md)).
- **Restarts.** `oso dev` and `oso-restart` carry buffers and terminals to the new instance
  ([ADR 0090](0090-oso-dev-carries-unsaved-text.md), [ADR 0133](0133-keeping-terminals-over-an-update.md)).
- **Open question:** OS sandboxing of child processes such as language servers, after M4
  ([open questions](../open-questions.md)).

### The threat

A language server for a trusted project runs that project's code. rust-analyzer runs build
scripts and proc macros when it loads a workspace and when it expands macros, and `cargo check`
on save. Trust is the gate, as in ADR 0009, and ADR 0118 keeps an agent from opening that gate
with no one at the keyboard.

The server is also a source of untrusted input. A broken or hostile server, or one that relays
a hostile crate's documentation, can send huge or endless messages, deep JSON, positions past
the end of the text, paths anywhere on disk or on another computer, requests to edit buffers or
open documents, and text with bidirectional controls (T3, T8 and T10 in
[security](../security.md)).

On macOS, `rust-analyzer` on `PATH` is usually rustup's proxy. It picks the toolchain from the
project's `rust-toolchain.toml`, and can download one. In oso's own repository the pinned
toolchain has no `rust-analyzer` component, and the proxy falls back to another toolchain's.

### What Emacs does

Two packages are common, and Emacs users have strong habits around them:

| | Eglot (in Emacs since 29) | lsp-mode |
|---|---|---|
| Starting | `M-x eglot`, or `eglot-ensure` in a mode hook | `lsp` or `lsp-deferred`; asks for the root the first time |
| Root | `project.el`'s root | its own guess, kept in a session file |
| Servers | `eglot-server-programs`: a command for each mode | many built-in clients; downloads servers on request |
| Diagnostics | Flymake: underlines, fringe, `[E W]` in the mode line | lsp-diagnostics with Flycheck or Flymake; lsp-ui's sideline |
| Definition | xref: `M-.`, back on `M-,` | xref, and lsp-ui's peek |
| Completion | `completion-at-point`, shown by Corfu or Company | the same, with Company by default |
| Hover | ElDoc in the echo area | lsp-ui's doc popup, ElDoc |
| One server for | a project and a major mode | a workspace root and a client |
| A crash | starts again if it ran 3 s or more (`eglot-autoreconnect`) | asks to restart |
| Its log | an events buffer with every message in full | `*lsp-log*` and an io log |

**oso follows Eglot (Q1).** Eglot does little of its own and feeds what the editor already has:
Flymake, xref, `completion-at-point` and ElDoc. oso has the same shape: a layer for
diagnostics, the jump commands, the completion matcher and floats. lsp-mode's downloads are
network code oso does not have, and its sideline and peek windows are more UI than the roadmap
asks for. oso differs from Eglot in four places:

- A server starts only in a trusted project, and ADR 0118's holds apply.
- Every message has a bound.
- The log has method names and sizes, never contents.
- Completion in the buffer shows like Corfu, not in the minibuffer.

Other editors, for contrast: some start the server a language's settings name as soon as a file
opens, with no trust step, and some download servers. VS Code asks for Workspace Trust, and
leaves extensions to decide what runs in restricted mode.

## Decision

Proposed. Each part names the question it answers.

### Which servers (Q2)

- **rust-analyzer is the one built-in entry,** for Rust buffers. oso is developed in oso from M4,
  and rust-analyzer is the server that matters there.
- **Any other server that speaks LSP 3.17 over standard input and output can be added in the
  configuration,** for a mode oso knows: Rust, Markdown or TOML (for example `taplo lsp stdio`
  or `marksman server`). A mode oso does not know cannot get a server until it has a `Mode`.
- **The built-in entry is found on `PATH`, then in `$CARGO_HOME/bin` or `~/.cargo/bin`.** An app
  started from the Finder gets a short `PATH`, and rustup puts its proxy in Cargo's directory.
  Neither is under the project. The configuration can name an absolute path instead, or turn
  the entry off.
- With the built-in entry, a program that runs for a trusted project is one that the person's
  configuration or oso's own defaults name, never the project. See
  [Changes to accepted ADRs](#changes-to-accepted-adrs).

### Configuration (Q3)

```toml
[lsp]
start = "trusted"                  # "trusted" (the default) or "manual" (Q4)

[lsp.servers.rust-analyzer]        # the built-in entry; this changes it
modes = ["rust"]
command = ["rust-analyzer"]        # a name on PATH, or an absolute path, then arguments
env = ["CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN"]   # more names from oso's environment

[lsp.servers.rust-analyzer.options] # sent as initializationOptions, and as the answer
cargo.targetDir = true              # to workspace/configuration under "rust-analyzer"

[lsp.servers.taplo]                # a server of the person's own
modes = ["toml"]
command = ["taplo", "lsp", "stdio"]
```

- `enabled = false` turns an entry off. A mode has at most one server. When two entries name a
  mode, the first in the file wins and the second is reported.
- `env` takes names only, refused as `process::may_pass` refuses them. Values come from oso's own
  environment, as `[sessions] env` does ([ADR 0121](0121-agent-sessions-and-spaces.md)).
- `options` is turned from TOML into JSON on the `oso-config` thread. The file's bounds already
  apply: 256 KiB, and TOML's nesting limit.
- **The built-in rust-analyzer entry sets `cargo.targetDir = true`,** so its `cargo check` builds
  in `target/rust-analyzer` and never waits on the build lock of `just ci` or `oso dev`. The
  disk cost is a second target directory.
- A change to an entry applies to servers started after it. `lsp-restart` applies it to a
  running one.

### When a server starts (Q4, Q5)

- **In a trusted project, the person's first visit to a buffer of a configured mode starts that
  mode's server for the project's root,** as `eglot-ensure` does. The start happens once the
  buffer has loaded and its project is known. A buffer in no project gets no server, as it
  cannot be trusted.
- **What an agent's open starts waits for the person,** by ADR 0118: the buffer's mark says
  `waits: server`, and the person's first command in that buffer lets it go. The same holds for
  `didOpen` of a buffer the agent opened when the server already runs.
- **`start = "manual"`** starts nothing on a visit. `lsp-start` starts the selected buffer's
  server, and the project keeps it for the session.
- **Trusting a project starts the servers for its buffers that the person visited,** since the
  person just answered `y` from the keyboard in one of them. Buffers the agent opened keep their
  marks. Untrusting a project stops its servers and clears their diagnostics.
- **The start runs on the server's own thread:** it reads the trust record's root again, checks
  its device and inode, finds the program, and spawns. A root that changed starts nothing, and
  `*Messages*` says why. The UI thread only asks.
- The server runs in the project's root, gets the root as its only workspace folder, and runs in
  a process group of its own, so stopping it stops the `cargo` and build scripts under it.

### Lifetime (Q6, Q7)

- **One server per project root and server entry.** A worktree is its own root, so it has its
  own server. Buffers of that root and mode share it.
- **A server stays up until its project is untrusted, `lsp-shutdown` stops it, or oso quits.**
  Killing its last buffer does not stop it, as `eglot-autoshutdown` defaults to off: the next
  visit would pay for indexing again.
- **A server that exits, or that a bound below stops, starts again by itself** after 1 s, 5 s and
  then 30 s. Three crashes within 3 minutes stop it for the session. The modeline then says
  `lsp!` in the warning face, `*Messages*` says why once, and `lsp-restart` tries again. Each
  start opens the documents again with their current text, so nothing is out of step.
- **`lsp-shutdown` and untrusting** send `shutdown`, wait 2 s for the answer, send `exit`, and after
  2 s more send `SIGTERM` to the group, then `SIGKILL` after 2 s more. All of it runs on the
  server's thread.
- **Quitting oso closes each server's standard input and sends `SIGTERM` to its group,** with no
  wait. Servers end on a closed input, and a quit never waits for a server.
- **A restart of oso (`oso dev`, `oso-restart`) carries no server.** The new instance starts the
  servers again for buffers the person visited, and carries ADR 0118's marks. rust-analyzer
  indexes again, which takes seconds on oso's repository.

### Files a jump reaches outside the project (Q8)

- **A file that a server's answer leads to, outside any project, joins that server,** as
  `eglot-extend-to-xref` does: the standard library's sources and the crates in Cargo's registry.
  So `M-.` keeps working there. The file is not trusted for anything else, and it starts no
  server of its own. It leaves the server when its buffer is killed.
- A file in another project, trusted or not, follows that project's own rules.

### The protocol (Q9, Q10, Q11)

- **A new library crate, `oso-lsp`,** holds the client. It depends on `oso-core` and
  `oso-platform`, and only the tip crate depends on it, as with `oso-term`. It keeps the protocol
  out of the tip crate, which must build in 2 s. Its modules:
  - `framing`: the `Content-Length` header and the message's bytes, both ways;
  - `json`: bounded reads of `serde_json::Value`: a string cut to a length, an array cut to a
    count, a number in range;
  - `uri`: `file:` URIs to paths and back;
  - `position`: LSP positions to byte offsets in a `Rope` and back, in the encoding agreed;
  - `protocol`: the requests and notifications oso sends, and what it reads from the answers,
    as plain Rust types;
  - `server`: the threads, the documents a server knows, and the requests in flight;
  - `limits`: every bound below, in one place.
- **No new dependency.** The framing is about 100 lines of oso's own. JSON is `serde_json`,
  already in the tree for the agent channel, used through `Value`, as there. Its default nesting
  limit of 128 stays on. oso uses a small part of the protocol, and plain types are shorter than
  `lsp-types`' generality, which would bring `serde`'s derive macros and `url` or `fluent-uri`.
- **The capabilities oso claims are the ones it uses:** text document sync (incremental, with
  open, close and save), publish diagnostics (with tags and versions), definition (with links),
  references, completion (no snippets, `resolve` for edits and documentation), hover (Markdown
  and plain text), work done progress, and `workspace/configuration`. oso claims no file
  watching, so rust-analyzer watches files itself.
- **Positions are UTF-8 when the server offers it** (`general.positionEncodings`, LSP 3.17).
  rust-analyzer and clangd do. Otherwise they are UTF-16, through a new count in the
  rope's summary, `TextSummary::utf16`, so a column converts in O(log n) on a long line, as a byte
  column does. Computing it costs one more count of each chunk's 4-byte scalars, when the chunk
  is made.
- **A position the server sends is clamped:** a line past the end to the end, a column past its
  line to the line's end, and a column inside a character to that character's start. Nothing it
  sends can make oso index out of the text.

### Threads

Each server has four threads, named for its entry and a number (`oso-lsp-1` and so on):

- **The session thread** owns the protocol: the documents and their versions, the requests in
  flight, the answers, and the conversions between positions and offsets. It takes the UI's
  messages and the reader's from one channel, and wakes the UI as the loader does.
- **The writer** owns the server's standard input and writes what the session thread queued.
- **The reader** owns the standard output. It reads frames and hands their bytes to the session
  thread, which parses them, so a slow parse holds up neither pipe.
- **The error reader** keeps the last 256 KiB of the standard error in memory, for `*LSP*`.

The UI thread never reads, writes, parses or waits on a server. For an edit it hands over the
batch's splices and the ropes before and after, which are cheap clones. For a report it swaps in
what the session thread built.

### Document sync

- **A buffer is opened with its server** (`didOpen`) once it has loaded, is in the server's mode
  and root, and is not held by ADR 0118. Its text goes once, in full.
- **Not opened with a server:** a buffer that may hold secrets (see [Secrets](#secrets)), one that
  is read-only for invalid UTF-8 or binary bytes, one over 8 MiB, and internal buffers.
- **Each batch of edits is one `didChange`** with one change for each splice, highest first, so
  each change's range is still where the text before the batch had it. This is the order the
  syntax worker uses. 1000 cursors make one notification with 1000 changes. The version goes up
  by one for each batch.
- **The session thread works out the ranges** from the ropes it was given, never the UI. It keeps
  the rope of each version sent, until the server has answered or reported past it, and at most
  16 of them, so a late report can be mapped forward.
- **Undo, a sliced undo's turns ([ADR 0119](0119-undoing-large-steps-in-slices.md)), a reload from
  disk and query-replace are batches like any other.**
- `C-x C-w` closes the old document and opens the new one if its mode and root still fit.
  Killing the buffer closes it. The person's save sends `didSave`. The agent's save holds it
  until the person's next save, by ADR 0118.

### Bounds (Q12)

Every bound lives in `oso-lsp`'s `limits`:

| What | Bound |
|---|---|
| A header | 32 lines, 1 KiB a line, 4 KiB in all; `Content-Length` once, ASCII digits |
| A message | 16 MiB |
| JSON nesting | 128 (`serde_json`'s default) |
| A completion answer | 5000 items, the rest dropped |
| A label, a detail, a diagnostic's message | 1 KiB, 4 KiB, 8 KiB |
| Hover or documentation text | 64 KiB |
| Diagnostics of one document | 2000, the most severe first |
| Documents with diagnostics | 10,000; the oldest unopened goes first |
| Locations in one answer | 1000 |
| Edits in a completion's `additionalTextEdits` | 16, at most 64 KiB of text, none overlapping the main edit |
| Queued for the writer | 64 MiB |
| Requests in flight | one of each kind for each buffer; a newer one cancels the older (`$/cancelRequest`) |
| Waits | `initialize` 60 s; definition and references 10 s; completion and hover 3 s; `shutdown` 2 s |
| Reports to the UI | diagnostics and progress wake it at most once each 50 ms, coalesced to the latest of each document; an answer to a request wakes it at once |
| `window/showMessage` | one a second in the echo area; the rest go to `*LSP*` |
| Messages from the server | 10,000, or 256 MiB, in any 10 s |

- **A message over 16 MiB is read through in 64 KiB pieces and dropped unparsed.** The framing
  stays in step. If it was an answer, its request fails, and `*LSP*` says so.
- **A broken header, a message that is not JSON-RPC, a flood past the last row, or a writer queue
  past 64 MiB stops the server,** and counts as a crash. Sync cannot drop a message, so a full
  queue can only end the server.
- **A request that waits too long is cancelled,** and the echo area says
  `rust-analyzer did not answer`.
- **What the server asks of oso:**
  - `workspace/configuration` is answered from the entry's `options`.
  - `window/workDoneProgress/create` and `client/registerCapability` get an empty success; the
    progress feeds the modeline.
  - `workspace/applyEdit` gets `applied: false`, and `window/showDocument` gets
    `success: false`: a server never edits a buffer or opens a file by itself.
  - `window/showMessageRequest` gets `null`, and its message is shown.
  - Anything else gets `MethodNotFound`.
- **Text from a server is untrusted.** Messages, labels and hover text are drawn as labels are,
  with control and invisible characters boxed. The echo area names the server first
  (`rust-analyzer: …`), so a server cannot pass as oso.
- **A URI must be `file:`, with an empty host or `localhost`.** Any other is dropped, so an answer
  cannot make oso reach another computer. On Windows, that would be an SMB path that sends the
  person's credentials.

### Diagnostics (Q13–Q16)

- **`oso-core` gains `diagnostic`:** each buffer's set, sorted by start. Each entry has a byte
  range, a severity, the tags `Unnecessary` and `Deprecated`, a source and a code, and its message
  in an arena of the set. The ranges follow each edit, as the syntax runs do, in O(entries), at
  most 2000.
- **A report becomes a set on the session thread:** positions are converted against the rope of
  the version it names, or the latest sent when it names none, as rust-analyzer's `cargo check`
  reports are. They are mapped forward through the splices sent since then. The UI then maps
  them through the edits it has not sent yet, and swaps the set in.
- **What the person sees (Q13):**
  - a wavy underline in the severity's color: `Face::DiagnosticError`, `DiagnosticWarning` and
    `DiagnosticInfo`, each checked for contrast in every theme
    ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)). The renderer gains the wavy
    underline;
  - a sign column of one cell in the gutter, while a server follows the buffer. It shows a bar
    in the color of the line's most severe diagnostic. It is there before the first report, so
    the text does not move when one comes;
  - in the modeline, after the project's name, `lsp` with the server's state: dim while it
    starts, `lsp 34%` while it reports progress, plain when ready, and `lsp!` in the warning face
    when stopped. Then the counts that are not zero, as Flymake shows them: `E2 W5`, in the
    severities' faces.
- **The message at point (Q14):** when the cursor stays 300 ms on a diagnostic, the echo area shows
  its first line, and `+2` when more cover that place. This is what Flymake does with ElDoc. A
  message the person put there stays until it times out, as usual.
- **Keys (Q15):** `M-g n` and `M-g p` (`lsp-next-diagnostic`, `lsp-previous-diagnostic`) move to the
  next and previous diagnostic, as Emacs's `next-error` keys. They wrap, and say `Wrapped`, as
  isearch does. `M-g d` (`lsp-diagnostics`) lists the project's diagnostics in the centred float
  with a preview, as `consult-flymake` does. The buffer's own come first, then the project's by
  file.
- **Hints (Q16):** a hint gets no underline and no count. It shows at point. A range tagged
  `Unnecessary`, such as an unused variable, is drawn in the dim face, as VS Code fades it.
  `Deprecated` gets a strike-through once the renderer has one, and until then nothing.

### Go to definition and back (Q17, Q18)

- **`M-.` (`lsp-find-definition`)** asks for the definition at the primary cursor. One answer
  goes there. Several answers open the centred float with a preview, as `consult-xref` does. No
  answer says `No definition found`.
- **A history of places in each window, `oso-core`'s `window::places`:** each jump from `M-.`,
  `M-?` or the float pushes the place it left: the buffer and the primary cursor's offset, which
  follows edits. It holds 64 places. `M-*` (`lsp-go-back`) goes back to the last one, and
  `C-M-*` (`lsp-go-forward`) undoes a `M-*`, as Emacs 29's `xref-go-back` and
  `xref-go-forward`. A killed buffer's places go. The jump also pushes the place onto the
  cursor's mark ring, as avy's jumps do, so `C-.` finds it.
- **The jump ends with one cursor at the target,** as other jumps do. It works with several
  cursors, from the primary.
- **`M-?` (`lsp-find-references`)** lists the references in the same float with a preview.
- A target opens as `C-x C-f` would open it. It never blocks on a special file, and a target
  whose name matches the secret patterns opens as a buffer that may hold secrets, with no server.

### Completion in the buffer (Q19–Q22)

- **A popup at point, as Corfu's.** It is a float under the cursor's row, starting at the column
  where the word being completed starts. It shows up to 10 rows, and goes above the row when
  fewer fit below. It never covers the cursor's row. Each row shows the label, then the kind and
  the detail as an annotation column, in the completion faces. It is at most 60 cells wide, or
  half the window.
- **Matching is oso's own matcher** (ADR 0076), on the item's `filterText` or label, keeping the
  server's order (`sortText`), as vertico keeps a list's. When the answer says it is incomplete,
  typing asks again.
- **When it opens (Q20):** after 200 ms with no key, once two identifier characters are typed, or
  at once after one of the server's trigger characters (`.` and `::` for rust-analyzer), as
  `corfu-auto` does. `C-M-i` (`completion-at-point`) opens it at any time. `M-TAB` is left out:
  Meta is Command, and Command-Tab is macOS's application switcher. It does not open during an input method's
  composition, in a prompt, or in a buffer that may hold secrets.
- **Keys while it is open** (a keymap above the buffer's, as `corfu-map`):
  - `C-n` and `C-p` (and the arrows) move the selection;
  - `TAB` inserts the selected item;
  - `RET` inserts it only after the selection was moved; otherwise it closes the popup and
    runs `RET`'s command, so an automatic popup never takes a newline;
  - `C-g` closes it;
  - typing goes on into the buffer and narrows the list;
  - a key that moves the cursor out of the word, or any other command, closes it.
- **Inserting** replaces the word with the item's `textEdit`, or its label. After `resolve`, its
  `additionalTextEdits` apply in the same undo step, such as rust-analyzer's added `use` line.
  When one of them lies outside the view, the echo area says which line changed.
- **No snippets (Q22):** oso claims no snippet support, so the server sends plain text. If one
  still comes, its `$1` and `${1:name}` markers are dropped and the placeholder's text kept.
- **With several cursors (Q21):** the item's text goes in at every cursor whose word before it
  is the primary's, as the person typed the same word at each. The `additionalTextEdits` apply
  once.
- **Budget:** the answer is parsed and cut on the session thread. On the UI thread, filtering is
  bounded by ADR 0076's limits, and the popup is drawn on the next frame after the answer comes.
  An answer for an older version, or for a cursor that left the word, is dropped.

### Hover (Q23)

- **`C-h .` (`lsp-describe-at-point`)** shows the hover text in a float at point, up to 20 rows and
  80 columns. It closes on the next key, and `C-v` and `M-v` scroll it. Markdown is highlighted
  with oso's Markdown grammar, Rust code in it with Rust's (ADR 0111). This is cheap, as both
  exist.
- Nothing opens by itself on idle. The echo area at point belongs to diagnostics (Q14).

### What agents may do (Q24)

| Command or tool | Reach | Why |
|---|---|---|
| `lsp-start`, `lsp-restart` | `Person` | spawns a process |
| `lsp-shutdown` | `Person` | manages the person's processes |
| `M-.`, `M-?`, `M-*`, `C-M-*` | `Look` | moves and opens; asks a running server about a buffer it knows |
| `M-g n`, `M-g p` | `Look` | moves |
| `M-g d`, `C-M-i`, `C-h .` | `Person` | an interactive float for the keyboard |
| `diagnostics` tool on `oso mcp` | read, like `text` | reads the reports oso holds |

- **A definition the agent asks for opens as the channel's `open` does:** a target whose name
  matches the secret patterns is refused, a file is shown only once judged ordinary, it counts
  toward the connection's 100 buffers, and it is marked as the agent's. So its `didOpen` waits for
  the person, by ADR 0118.
- **The `diagnostics` tool** takes a buffer or `project`, and gives at most 500 entries: the path,
  line and column (1-based, in characters), the severity, the message (2 KiB at most), the source
  and the code. ADR 0121's `@diagnostics` mention reads the same. `state` gains each buffer's
  server state and counts, and its schema version goes up by one.
- **No completion or hover for the channel:** the agent has its own tools, and both are made for
  the keyboard.
- The channel's "the person is in the middle of something" includes an open completion popup or
  hover float.

### Secrets

- **A buffer that may hold secrets is never sent to a server,** and gets no diagnostics, completion,
  hover or definition: those say `No server for a buffer that may hold secrets`. A buffer that
  becomes one, such as through `C-x C-w` to `.env`, is closed with its server first.
- **A report for a path whose name matches the secret patterns is dropped on the session thread,**
  as a compiler's message can quote the file.
- `M-g d`'s preview never shows a buffer that may hold secrets (ADR 0088).
- `*LSP*` and the error ring are in memory only. They never reach a log line or the disk, and have
  method names, ids, sizes and timings, never a document's text. The standard error is shown
  there with controls boxed, since a server may print the code it read.
- oso cannot keep the server from reading files on disk itself. Trust covers that, as it covers
  running the project's code.

### Windows (Q25)

- **Built and unit-tested in CI, as the rest is (Tier 2).** The author does not use Windows yet.
- **Only an `.exe` starts a server.** A `.bat` or `.cmd`, which npm makes for many servers, is
  refused and says why: the standard library runs those through `cmd.exe`, whose quoting has let
  arguments run commands before (CVE-2024-24576).
- **Without process groups,** stopping a server kills the server alone. A `cargo check` under it
  runs to its end. A job object comes when the author uses Windows.
- URIs carry the drive letter (`file:///C:/…`), compared without case, with `%3A` read as `:`.
  A UNC path (`\\server\share`) is never sent or read.

### Performance

- **Typing in a buffer a server follows stays within the budget,** p99 ≤ 8 ms from key to photon.
  The UI's added work per batch is O(splices) to queue it, and nothing is allocated per key
  beyond the batch's splices, which the syntax worker already gets.
- **A report swaps in at O(1) on the UI thread,** plus mapping it through unsent edits:
  O(entries), at most 2000.
- **Drawing finds the diagnostics in view by binary search:** O(rows shown + diagnostics shown).
- **The completion popup** is drawn on the frame after the answer comes, and its filtering stays
  within ADR 0076's limits for 5000 items.
- **Nothing before the first frame:** servers start after the project is found.
- **Memory:** the session thread keeps at most 16 ropes of each document, which share their
  chunks with the buffer's.
- **`just perf` gains report-only metrics, with thresholds later as ADR 0123 sets them:**
  - typing with a server attached, against oso's fake server;
  - a 2000-entry report applied while typing;
  - completion with 5000 items.

### oso's own repository (Q26)

- **`rust-toolchain.toml` gains the `rust-analyzer` component,** so rustup's proxy runs the pinned
  toolchain's server, whose proc-macro server matches the compiler. This file defines the checks,
  so the change needs the author's approval, in its own step.

### Steps

Each step is one pull request. No step spawns anything before step 1 has merged.

1. **ADR 0118's code** (its step 2), with the kinds `server-start`, `server-open` and
   `server-save` as `Runs::Program`: the enum and `Editor::starts_on_open` and `starts_on_save`,
   the marks and their release, the `waits` segment, the replies, `state`'s field, the handoff
   entry, the guard test, and its end-to-end flows. A dev-only stand-in kind holds and lets go
   until step 3 gives the real ones a server.
2. **`oso-lsp` without spawning:** the crate, `framing`, `json`, `uri`, `position` and `limits`,
   with unit, property and fuzz tests for each bound, and `TextSummary::utf16` if Q11 is (a).
   Also a fake server for tests: an example binary of `oso-lsp` that never ships. It answers
   from a script and can misbehave on cue. The crate edge, `layers.txt` and `Cargo.toml` need the
   author's approval.
3. **A server's life:** `[lsp]` in the configuration, finding the program, the trust check on
   its thread, the spawn in its own group, `initialize` to `exit`, restarts after crashes, quit,
   untrust, `*LSP*`, the modeline's `lsp` state, and `lsp-start`, `lsp-restart` and
   `lsp-shutdown`. `oso-platform::process` gains a way to signal a child's group. End-to-end
   flows run against the fake server.
4. **Document sync:** open, change, close and save, with many cursors, sliced undo, reload,
   `C-x C-w`, the size limit and secrets. ADR 0118's holds then apply to the real kinds. Property
   tests check that the fake server's copy of the text equals the buffer's after random batches.
5. **Diagnostics, model and keys:** `oso_core::diagnostic`, reports mapped forward, the counts in
   the modeline, the message at point, `M-g n` and `M-g p`, and `M-g d` with its preview.
6. **Diagnostics, drawing:** the wavy underline and dim `Unnecessary` ranges in `oso-render`, the
   faces in the three themes with their contrast check, and the sign column. Text-grid snapshots
   cover both.
7. **Definition and references:** `window::places`, `M-.`, `M-?`, `M-*`, `C-M-*`, the float
   with a preview, files outside the project joining the server, and the channel's rules.
8. **Completion in the buffer:** the popup, its keys, matching, the automatic trigger,
   `resolve` and `additionalTextEdits`, and many cursors.
9. **Hover:** `C-h .`.
10. **Agents:** the `diagnostics` tool, `state`'s fields, the tool descriptions, and ADR 0121's
    `@diagnostics`.
11. **The `just perf` metrics,** and `rust-toolchain.toml`'s component if Q26 is (a).

Steps 5 and 6 can run beside 7 to 9 once step 4 has merged. Windows builds and tests in CI in
each step.

## Questions

Each lists the recommended option first.

### The model

1. **Which Emacs set-up oso follows.**
   (a) Eglot, with Flymake, xref, Corfu-style completion at point and ElDoc. It is minimal, feeds
   what oso already has, and is built into Emacs.
   (b) lsp-mode with lsp-ui and Company: more features, such as the sideline, peek and
   breadcrumbs, but much more UI.
   (c) Your own set-up. Say what you use, and oso follows its behaviour where it differs.

### Servers, configuration and lifetime

2. **Which servers oso knows itself.**
   (a) rust-analyzer only, found on `PATH`, then in Cargo's `bin`. Others, for Markdown or TOML,
   come from the configuration.
   (b) Also taplo for TOML and marksman for Markdown, built in, used when found.
   (c) None. The configuration names every server, as ADR 0114 says today.
3. **The configuration's shape.**
   (a) `[lsp.servers.<name>]` with `modes`, `command`, `env` and `options` (sent as
   `initializationOptions` and as answers to `workspace/configuration`), and `[lsp] start`. The
   built-in rust-analyzer entry sets `cargo.targetDir = true`.
   (b) As (a), without `cargo.targetDir`: rust-analyzer's checks share `target` with `just ci`
   and `oso dev`, and wait on its lock.
   (c) Only `command` per mode under `[modes.<mode>]`, with no options. Simpler, but
   rust-analyzer's settings could not be changed.
4. **When a server starts.**
   (a) By itself, in a trusted project, at the person's first visit of a buffer of a configured
   mode, as `eglot-ensure` does. An agent's open waits (ADR 0118).
   (b) Only by `lsp-start` (as `M-x eglot`), and then for the rest of the session in that project.
   (c) As (a), and the first start of each server for each project asks once (`Start
   rust-analyzer for oso? (y or n)`). One more question, after trust already asked.
5. **Trusting a project.**
   (a) Starts the servers for its buffers the person visited, at once. Buffers the agent opened
   keep their marks.
   (b) Starts nothing. The person's next command in such a buffer starts its server.
6. **Lifetime and crashes.**
   (a) A server stays up until untrust, `lsp-shutdown` or quit. After a crash it starts again
   after 1 s, 5 s and 30 s, and stays down after three crashes in 3 minutes, saying so. Close to
   Eglot's `eglot-autoreconnect`, with a limit.
   (b) As (a), but a server also stops 5 minutes after its last buffer is killed.
   (c) No automatic restart, as lsp-mode asks first: the modeline says `lsp!` and `lsp-restart`
   starts it.
7. **oso's own restarts.**
   (a) Carry no server. The new instance starts them again, and rust-analyzer indexes again.
   (b) Carry the server's pipes as descriptors, as terminals are carried. Nothing to index again,
   but the protocol's state must be carried too, and a version mismatch would desynchronize.
8. **Files a jump reaches outside any project,** such as the standard library's sources.
   (a) Join the server that gave the jump, as `eglot-extend-to-xref`, so `M-.` keeps working there.
   (b) Get no server, as Eglot's default. `M-.` stops working in those files.

### The protocol

9. **Where the client lives.**
   (a) A new crate, `oso-lsp`, beside `oso-term`, used only by the tip crate. It keeps the tip
   crate's build fast and the protocol testable alone.
   (b) Modules in the tip crate. No new crate, but more code there to rebuild.
10. **Dependencies.**
    (a) None new: oso's own framing, and `serde_json` through `Value`, already in the tree.
    (b) `lsp-types`: every type of the protocol, but `serde`'s derive macros and `url` or
    `fluent-uri` with it, for the small part oso uses.
    (c) `lsp-server`, rust-analyzer's own crate: framing and threads, made for servers, with
    `crossbeam-channel`.
11. **Positions.**
    (a) UTF-8 when the server offers it, otherwise UTF-16 through a new UTF-16 count in the
    rope's summary, O(log n) on any line.
    (b) UTF-8 only. A server without it, as many JavaScript and Python servers are, is refused.
    (c) UTF-16 by scanning the line, with no change to the rope. Lines over 64 KiB would then be
    too slow with many cursors, so such documents would not be sent.
12. **A message that breaks a bound.**
    (a) A message over 16 MiB is read through and dropped, and its request fails. A broken header,
    a flood or a full writer queue stops the server, and counts as a crash.
    (b) Any broken bound stops the server. Stricter, but a big answer from a healthy server, such
    as diagnostics for a generated file, would restart it.

### Diagnostics

13. **How they look.**
    (a) A wavy underline in the severity's color, a one-cell sign column while a server follows
    the buffer, and `lsp` with `E2 W5` in the modeline, as Flymake.
    (b) As (a), with a straight underline. The renderer then needs no wavy one, but it looks like
    the preedit's underline.
    (c) As (a), with the sign column only while the buffer has diagnostics. Saves a column, but
    the text moves when the first report comes and when the last fix clears it.
14. **The message at point.**
    (a) In the echo area after the cursor stays 300 ms on a diagnostic, as Flymake with ElDoc.
    (b) Only on a key (`C-h .` shows the diagnostics with the hover).
    (c) At the end of the line, as Emacs 30's `flymake-show-diagnostics-at-end-of-line` and lsp-ui's
    sideline. Always visible, but it covers text on long lines.
15. **Keys.**
    (a) `M-g n` and `M-g p` for next and previous, as `next-error`, and `M-g d` for the list with a
    preview, as `consult-flymake`.
    (b) `M-n` and `M-p` in buffers a server follows, as many Flymake set-ups bind, with the list on
    `M-g d`.
16. **Hints and unused code.**
    (a) A hint is not underlined or counted, and shows at point. An `Unnecessary` range is dim.
    (b) Hints are drawn and counted as information is.

### Definition

17. **Going back.**
    (a) A history of 64 places in each window. `M-*` goes back and `C-M-*` forward, as Emacs 29's
    `xref-go-back` and `xref-go-forward` but on other keys, since `M-,` is `previous-buffer`
    (ADR 0088). The place also goes on the mark ring.
    (b) As (a), with one history for the frame, as Emacs's default `xref-history-storage`.
    (c) No history of its own: the mark ring only, through `C-.`. Less to build, but it cannot go
    back across buffers.
18. **References.**
    (a) `M-?` in step 7, in the same float as several definitions. It costs one more request.
    (b) Later, after the roadmap's three.

### Completion

19. **Where it shows.**
    (a) A popup at point, as Corfu, up to 10 rows, filtered by oso's matcher.
    (b) In the centred float, as `consult-completion-in-region` with vertico. One float for all
    completion, but far from the cursor, and typing goes into the prompt, not the buffer.
20. **When it opens.**
    (a) By itself after two identifier characters and 200 ms with no key, or at once after a
    trigger character (`.`, `::`), as `corfu-auto`; and on `C-M-i`.
    (b) Only on `C-M-i`, as Corfu's default.
    (c) As (b), and `TAB` completes when the line is already indented, as
    `tab-always-indent 'complete`.
21. **With several cursors.**
    (a) The item goes in at every cursor whose word before it is the primary's, and the extra edits
    apply once.
    (b) Completion works with one cursor only, and says so.
22. **Snippets and extra edits.**
    (a) No snippets: plain text. `additionalTextEdits` (rust-analyzer's added `use` lines) apply in
    the same undo step, and the echo area names a changed line outside the view.
    (b) Snippets with tab stops, as yasnippet. More to build, and the keys must be decided.
    (c) Neither: insert the label only. An imported name then needs its `use` written by hand.

### Hover

23. **Hover.**
    (a) `C-h .` shows it in a float at point, Markdown highlighted. Nothing opens by itself.
    (b) As (a), and the signature of the call at point shows in the echo area on idle, as ElDoc
    does. A request at each pause, and it competes with diagnostics' messages there.
    (c) No hover in M4.

### Agents, Windows and oso's repository

24. **What agents may do.**
    (a) `M-.`, `M-?`, the back keys and `M-g n`/`M-g p` are `Look`. Starting, restarting,
    stopping, the list, completion and hover are `Person`. A `diagnostics` tool joins `oso mcp`,
    and `state` gains each buffer's server state and counts.
    (b) As (a), without the `diagnostics` tool. The agent runs `cargo check` itself.
    (c) As (a), plus a `definition` tool that answers a path and line without opening anything.
25. **Windows.**
    (a) Built and unit-tested in CI. Only `.exe` servers start. Stopping kills the server alone,
    without a job object, until the author uses Windows.
    (b) Job objects now, so a stop also ends what the server started.
26. **oso's own `rust-toolchain.toml`.**
    (a) Add the `rust-analyzer` component, so rustup runs the pinned toolchain's server. It is a
    file that defines the checks, so the author approves it in step 11.
    (b) Leave it. rustup falls back to another toolchain's server, whose proc-macro server may
    not match the compiler.

## Changes to accepted ADRs

Only if the author accepts Q2 (a) or (b). The ADR's status then gains `amended by 0138`.

- [ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md), "A program that runs
  for a trusted project is one the person's configuration names": or one of oso's built-in
  server entries, which the configuration can change or turn off. It is still never one the
  project names.

## Consequences

- oso gets diagnostics, definitions, references, completion and hover for Rust from M4, and for
  any mode it knows from a server the configuration names.
- A language server runs only for a trusted project and is started only by the person. A broken
  or hostile server can slow only its own threads, and never blocks the UI thread, edits a
  buffer by itself, opens a file by itself, or makes oso reach another computer.
- oso gains a crate, and `oso-core` gains `diagnostic`, `window::places`, ADR 0118's enum and
  completion at point. The renderer gains a wavy underline and three faces in each theme.
- The gutter gains a column in buffers a server follows.
- rust-analyzer indexes again after each restart of oso.
- The server runs with the person's full rights until OS sandboxing comes, after M4.
- Follow-up, not in this ADR: format on save (its own `Program` kind under ADR 0118), rename and
  code actions, signature help, semantic highlighting, inlay hints, pull diagnostics, and a
  server for a mode oso does not know yet.

## Alternatives considered

- **Starting servers in every project, as some editors do:** opening a repository would run its build
  scripts. ADR 0009 forbids it.
- **Downloading servers, as lsp-mode can:** oso has no network code.
- **Running the client in the tip crate's existing workers, such as the syntax worker:** a server
  can stall, and the syntax worker's slices must not wait on a pipe.
- **One thread per server, with non-blocking pipes:** the process module hands out the standard
  library's blocking pipes, and four plain threads are simpler than polling on three platforms.
- **Full-text sync (`TextDocumentSyncKind::Full`):** simpler, but a 100 MB file would be sent at
  each key.
- **Parsing JSON on the reader thread:** a slow parse would stop it reading. A server that stops
  writing because oso stopped reading would look hung.
- **Keeping diagnostics in the server's positions and converting while drawing:** each frame would
  pay for the conversion, and edits made since would move nothing.
