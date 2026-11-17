# 0138. Language servers

- Status: Accepted
- Date: 2026-11-15

## Context

The roadmap's M4 holds "Language servers: diagnostics, go to definition, completion", and says
that ADR 0118's rule comes first, "with the first program that opening or saving starts"
([roadmap](../roadmap.md)). This ADR proposes how oso talks to language servers, with the
questions for the author under [Questions](#questions), each with the recommended option first.
What has been built is under [As built](#as-built).

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
toolchain had no `rust-analyzer` component, and the proxy fell back to another toolchain's;
it has one since step 12's first pull request.

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

### The author's set-up

The author's Emacs is built on Eglot:

- `eglot-ensure` for each mode with a server;
- Corfu with `corfu-auto`, `corfu-auto-prefix` 3 and `corfu-cycle`;
- `consult-xref` when there are several definitions;
- `xref-history-storage` set to a history for each window;
- in buffers with a server, `M-,` is `xref-go-back`, over `previous-buffer` there only, and `C-M-,`
  is `xref-go-forward`;
- Flycheck's next and previous error on `C-M-n` and `C-M-p`;
- `eglot-help-at-point` on `C-c C-d`, and `eglot-code-actions` on `C-c C-r`.

**oso follows the author's set-up (Q1).** Eglot does little of its own and feeds what the editor
already has: Flymake or Flycheck, xref, `completion-at-point` and ElDoc. oso has the same shape: a
layer for diagnostics, the jump commands, the completion matcher and floats. lsp-mode's downloads
are network code oso does not have, and its sideline and peek windows are more UI than the
roadmap asks for. oso differs from Eglot in three places:

- A server starts only in a trusted project, and ADR 0118's holds apply.
- Every message has a bound.
- The log has method names and sizes, never contents.

Other editors, for contrast: some start the server a language's settings name as soon as a file
opens, with no trust step, and some download servers. VS Code asks for Workspace Trust, and
leaves extensions to decide what runs in restricted mode.

## Decision

The author chose on 2026-11-15, through the coordinator:

- Q1: the author's own set-up, which is built on Eglot;
- Q2 and Q3: a built-in table of servers, so that no configuration is needed, with
  `[lsp.servers.<name>]` only to override it;
- Q15, Q17, Q20 and Q23: the keys and behaviour of that set-up;
- (a) on every other question.

Code actions and rename come in later steps. The author approved the crate `oso-lsp` and its
edges, and the change to `rust-toolchain.toml`, made at the start of step 12. The text names the question each part
answers, and the steps are under [Steps](#steps).

### Which servers (Q2)

**A built-in table says which server each mode gets, how it is found and what it is told, as
`eglot-server-programs` does.** No configuration is needed:

| Modes | Server | Found | Settings oso sends |
|---|---|---|---|
| Rust | `rust-analyzer` | `PATH`, then `$CARGO_HOME/bin` or `~/.cargo/bin` | `cargo.targetDir = true` |
| Go | `gopls` | `PATH`, then `$GOBIN`, the first `$GOPATH` entry's `bin`, or `~/go/bin` | `usePlaceholders`, `gofumpt`, `staticcheck` and `completeUnimported` true; `matcher = "CaseSensitive"` |
| TypeScript, TSX, JavaScript, JSX | `typescript-language-server --stdio`, or `tsc --lsp --stdio` | see below | none |
| LaTeX | `texlab` | `PATH` | none |
| Swift | `sourcekit-lsp` | `PATH` | none |
| TOML | `taplo lsp stdio` | `PATH` | none |
| Markdown | `marksman server` | `PATH` | none |

- **The settings go as `initializationOptions`** and answer `workspace/configuration` under the
  server's section (`rust-analyzer`, `gopls`).
- **`cargo.targetDir = true`** makes rust-analyzer's `cargo check` build in `target/rust-analyzer`,
  so it never waits on the build lock of `just ci` or `oso dev`. The cost is a second target
  directory.
- **gopls's `usePlaceholders` only matters once completion has snippets,** which oso does not
  claim (Q22), and `gofumpt` only once oso formats Go. They are set now so that nothing changes
  when those come.
- **The directories after `PATH` are found from oso's own environment and home, never from the
  project.** An app started from the Finder gets a short `PATH`. rustup puts its proxy in Cargo's
  directory, and `go install` puts gopls in Go's.
- **TypeScript and JavaScript take the first of:**
  1. the project's `node_modules/.bin/typescript-language-server --stdio`, when the project's
     `node_modules/typescript/lib/tsserver.js` exists (TypeScript before its native compiler);
  2. the project's `node_modules/.bin/tsc --lsp --stdio` (the native compiler's own server);
  3. `tsc --lsp --stdio` on `PATH`.

  `node_modules` is the one at the project's root.
- **A server that is not installed is said once per project:** `gopls is not installed (looked on
  PATH and in ~/go/bin)` in the echo area and `*Messages*`. It is not looked for again in that
  project until `lsp-restart`, or a change to its override.
- **New modes:** Go, TypeScript, TSX, JavaScript (with JSX), LaTeX and Swift, chosen by the file's
  name as the others are, so that they can have a server. They have no highlighting until their
  grammars come.

### A server inside the project

- **A project's own server, such as one in `node_modules`, runs only in a trusted project,** as
  every server does. Trust is the gate: the person accepted running the project's code, and this
  is the project's code.
- **Looking for it reads only the metadata of fixed names under the root** (`node_modules/.bin/…`,
  `node_modules/typescript/lib/tsserver.js`), on the server's thread, after the trust check. It
  reads no file's contents and no setting of the project.
- **The program must stay inside the root:** `node_modules/.bin`'s entries are links, so the link
  is followed, and where it leads must be a regular file under the root, owned by this user, that
  no one else can change. Otherwise the next choice is tried, and `*LSP*` says why.
- **It is run by its absolute path** (`Executable::absolute`). A script's `#!/usr/bin/env node`
  finds `node` on the child's `PATH`, which leaves the project out as always.

### Overrides (Q3)

```toml
[lsp]
start = "trusted"                  # "trusted" (the default) or "manual"

[lsp.servers.rust-analyzer]        # changes the built-in entry; every key is optional
command = ["/opt/rust-analyzer/bin/rust-analyzer"]   # a name on PATH, or an absolute path
env = ["CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN"]   # more names from oso's environment

[lsp.servers.rust-analyzer.options] # merged key by key over the built-in settings
check.command = "clippy"

[lsp.servers.gopls]
enabled = false                    # turns a built-in entry off

[lsp.servers.my-server]            # a server of the person's own
modes = ["toml"]
command = ["my-server", "--stdio"]
```

- **The table needs no configuration.** An override changes only the keys it names.
- `modes` moves a built-in entry, or names a new entry's. A mode has at most one server: an entry
  of the person's wins over a built-in one, and of two of the person's, the first in the file
  wins and the second is reported.
- `env` takes names only, refused as `process::may_pass` refuses them. Values come from oso's own
  environment, as `[sessions] env` does ([ADR 0121](0121-agent-sessions-and-spaces.md)).
- `options` is turned from TOML into JSON on the `oso-config` thread. The file's bounds already
  apply: 256 KiB, and TOML's nesting limit.
- A change applies to servers started after it. `lsp-restart` applies it to a running one.

### When a server starts (Q4, Q5)

- **In a trusted project, the person's first visit to a buffer of a mode with a server starts
  that mode's server for the project's root,** as `eglot-ensure` does. The start happens once the
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
| A header | 32 lines, 1 KiB a line, 4 KiB in all; `Content-Length` once, ASCII digits, at most 256 MiB |
| A message | 16 MiB |
| JSON nesting | 128 (`serde_json`'s default) |
| JSON values in a message | 1,000,000, counted before it is parsed |
| A URI | 16 KiB, checked before it is decoded |
| A completion answer | 5000 items, the rest dropped |
| A label, a detail, a diagnostic's message | 1 KiB, 4 KiB, 8 KiB |
| Hover or documentation text | 64 KiB; a hover's `MarkedString`s 256, a fenced block's language 32 bytes (step 10) |
| A diagnostic's source, and its code | 256 bytes each (step 6) |
| Diagnostics of one document | 2000, the most severe first |
| Documents with diagnostics | 10,000; the oldest unopened goes first |
| Diagnostics kept in all | 64 MiB a server, the oldest unopened document's going first (step 6) |
| Locations in one answer | 1000 |
| Edits in a completion's `additionalTextEdits` | 16, at most 64 KiB of text, none overlapping the main edit |
| Queued for the writer | 64 MiB |
| Requests in flight | one of each kind for each buffer; a newer one cancels the older (`$/cancelRequest`) |
| Waits | `initialize` 60 s; definition and references 10 s; completion and hover 3 s; `shutdown` 2 s |
| Reports to the UI | diagnostics and progress wake it at most once each 50 ms, coalesced to the latest of each document; an answer to a request wakes it at once |
| `window/showMessage` | one a second in the echo area; the rest go to `*LSP*` |
| Messages from the server | 10,000, or 256 MiB, in any 10 s |

- **A message over 16 MiB is read through in 64 KiB pieces and dropped unparsed.** The framing
  stays in step. If it was an answer, its request fails, and `*LSP*` says so. So is a message
  of more than 1,000,000 JSON values, which parsed would take hundreds of MiB.
- **A `Content-Length` over 256 MiB is a broken header,** as no message that long can keep
  within the flood bound: so one endless message cannot hide from it.
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
  its first line, and `+2` when more cover that place, as Flymake and Flycheck do with ElDoc. A
  message the person put there stays until it times out, as usual.
- **Keys (Q15):** `C-M-n` and `C-M-p` (`lsp-next-diagnostic`, `lsp-previous-diagnostic`) move to
  the next and previous diagnostic, as the author's Flycheck keys do. They wrap, and say
  `Wrapped`, as isearch does. `M-g d` (`lsp-diagnostics`) lists the project's diagnostics in the centred float
  with a preview, as `consult-flymake` does. The buffer's own come first, then the project's by
  file.
- **Hints (Q16):** a hint gets no underline and no count. It shows at point. A range tagged
  `Unnecessary`, such as an unused variable, is drawn in the dim face, as VS Code fades it.
  `Deprecated` gets a strike-through once the renderer has one, and until then nothing.

### Go to definition and back (Q17, Q18)

- **`M-.` (`lsp-find-definition`)** asks for the definition at the primary cursor. One answer
  goes there. Several answers open the centred float with a preview, as `consult-xref` does. No
  answer says `No definition found`.
- **A history of places in each window, `oso-core`'s `window::jumps`** (`window::places` is
  ADR 0088's place in each buffer): each jump from `M-.`,
  `M-?` or the float pushes the place it left: the buffer and the primary cursor's offset, which
  follows edits. It holds 64 places. `M-,` (`lsp-go-back`) goes back to the last one, and
  `C-M-,` (`lsp-go-forward`) undoes a `M-,`, as Emacs 29's `xref-go-back` and `xref-go-forward`
  with `xref-history-storage` set to a window's history. A killed buffer's places go. The jump also pushes the place onto the
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
- **When it opens (Q20):** after 200 ms with no key, once three identifier characters are typed,
  or at once after one of the server's trigger characters (`.` and `::` for rust-analyzer), as
  `corfu-auto` with `corfu-auto-prefix` 3 does. `C-M-i` (`completion-at-point`) opens it at any time. `M-TAB` is left out:
  Meta is Command, and Command-Tab is macOS's application switcher. It does not open during an input method's
  composition, in a prompt, or in a buffer that may hold secrets.
- **Keys while it is open** (a keymap above the buffer's, as `corfu-map`):
  - `C-n` and `C-p` (and the arrows) move the selection, and wrap from the last row to the first
    and back, as `corfu-cycle`;
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

- **`C-c C-d` (`lsp-describe-at-point`), as the author's `eglot-help-at-point`, and `C-h .`,
  which nothing binds,** show the hover text in a float at point, up to 20 rows and
  80 columns. It closes on the next key, and `C-v` and `M-v` scroll it. Markdown is highlighted
  with oso's Markdown grammar, Rust code in it with Rust's (ADR 0111). This is cheap, as both
  exist.
- Nothing opens by itself on idle. The echo area at point belongs to diagnostics (Q14).

### Keys in buffers a server follows (Q15, Q17, Q20, Q23)

- **A minor mode layer, `lsp`, is on in each buffer a server follows,** above the buffer's major
  mode, as Eglot's minor mode map is ([architecture](../architecture.md), Keymaps). It binds:

  | Key | Command |
  |---|---|
  | `M-.` | `lsp-find-definition` |
  | `M-?` | `lsp-find-references` |
  | `M-,` | `lsp-go-back` |
  | `C-M-,` | `lsp-go-forward` |
  | `C-M-n`, `C-M-p` | `lsp-next-diagnostic`, `lsp-previous-diagnostic` |
  | `M-g d` | `lsp-diagnostics` |
  | `C-M-i` | `completion-at-point` |
  | `C-c C-d`, `C-h .` | `lsp-describe-at-point` |

- **`M-,` is `lsp-go-back` only there.** In every other buffer it stays `previous-buffer`
  ([ADR 0088](0088-switching-and-killing-buffers.md)), as the author's set-up binds it in buffers
  with a server only. `M-/` stays `next-buffer` everywhere.
- The layer goes when the server stops following the buffer, so a buffer whose server stopped
  has `M-,` as `previous-buffer` again. A crashed server that starts again keeps it on.
- `C-c C-r` is left free for code actions (a later step).

### What agents may do (Q24)

| Command or tool | Reach | Why |
|---|---|---|
| `lsp-start`, `lsp-restart` | `Person` | spawns a process |
| `lsp-shutdown` | `Person` | manages the person's processes |
| `M-.`, `M-?`, `M-,`, `C-M-,` | `Look` | moves and opens; asks a running server about a buffer it knows |
| `C-M-n`, `C-M-p` | `Look` | moves |
| `M-g d`, `C-M-i`, `C-c C-d`, `C-h .` | `Person` | an interactive float for the keyboard |
| `lsp-describe-scroll-up`, `-down`, `-close` (step 10) | `Person` | the description's own keys |
| `diagnostics` tool on `oso mcp` | read, like `text` | reads the reports oso holds |
| `session-send-diagnostics`, `session-fix-error` (step 11) | `Person` | the person's paste into a session, as `session-send` |

- **A definition the agent asks for opens as the channel's `open` does:** a target whose name
  matches the secret patterns is refused, a file is shown only once judged ordinary, it counts
  toward the connection's 100 buffers, and it is marked as the agent's. So its `didOpen` waits for
  the person, by ADR 0118.
- **An agent never opens the float** (step 8's Q5): its `M-.` goes to the first of several
  definitions and says `1 of 3`, and its `M-?` is refused, as a float is the person's, and an
  agent cannot read one: `References are listed in a float, which only the person reads; search
  the project with your own tools`, as `oso mcp` has no search tool (step 8b's Q7).
- **The `diagnostics` tool** takes a buffer or `project`, and gives at most 500 entries: the path,
  line and column (1-based, in characters), the severity, the message (2 KiB at most), the source
  and the code. ADR 0121's `@diagnostics` mention reads the same. `state` gains each buffer's
  server state and counts, and its schema version goes up by one. As built (step 11), a file no
  buffer holds gives the server's column with `column_unit`, and `@diagnostics` gives the line
  only.
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
  arguments run commands before (CVE-2024-24576). So npm's TypeScript servers do not start on
  Windows yet: running their script with `node.exe` directly can come when the author uses
  Windows.
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
  toolchain's server, whose proc-macro server matches the compiler. This file defines the checks;
  the author approved the change, which went in on its own at the start of step 12, so that what
  fetching the component costs every toolchain install and CI job is seen apart from the
  metrics (step 12's Q6).

### Steps

Each step is one pull request. No step spawns anything before step 1 has merged.

1. **ADR 0118's code** (its step 2), with the kinds `server-start`, `server-open` and
   `server-save` as `Runs::Program`: the enum and `Editor::starts_on_open` and `starts_on_save`,
   the marks and their release, the `waits` segment, the replies, `state`'s field, the handoff
   entry, the guard test, and its end-to-end flows. A dev-only stand-in kind holds and lets go
   until step 3 gives the real ones a server.
2. **`oso-lsp` without spawning:** the crate, `framing`, `json`, `uri`, `position` and `limits`,
   with unit, property and fuzz tests for each bound, and `TextSummary::utf16`. Also a fake
   server for tests: an example binary of `oso-lsp` that never ships. It answers from a script
   and can misbehave on cue. The crate, its edges and the `[workspace]` line of `layers.txt` are
   approved through this ADR; `Cargo.toml` gains the crate.
3. **A server's life, with the table's rust-analyzer entry:** the table and `[lsp]` overrides in
   the configuration, finding the program, the trust check on its thread, the spawn in its own
   group, `initialize` to `exit`, restarts after crashes, quit, untrust, `*LSP*`, the modeline's
   `lsp` state, and `lsp-start`, `lsp-restart` and `lsp-shutdown`. `oso-platform::process` gains
   a way to signal a child's group. End-to-end flows run against the fake server.
4. **Document sync:** open, change, close and save, with many cursors, sliced undo, reload,
   `C-x C-w`, the size limit and secrets. ADR 0118's holds then apply to the real kinds. Property
   tests check that the fake server's copy of the text equals the buffer's after random batches.
5. **The rest of the table:** the modes for Go, TypeScript, TSX, JavaScript, LaTeX and Swift;
   gopls with its settings; the TypeScript choices with the checks on a program inside the
   project; texlab, sourcekit-lsp, taplo and marksman; and the message for a server that is not
   installed.
6. **Diagnostics, model and keys:** `oso_core::diagnostic`, reports mapped forward, the counts in
   the modeline, the message at point, the `lsp` minor mode layer, `C-M-n` and `C-M-p`, and
   `M-g d` with its preview. Two pull requests: 6a without `M-g d`, 6b with it.
7. **Diagnostics, drawing:** the wavy underline and dim `Unnecessary` ranges in `oso-render`, the
   faces in the three themes with their contrast check, and the sign column. Text-grid snapshots
   cover both.
8. **Definition and references:** `window::jumps`, `M-.`, `M-?`, `M-,` and `C-M-,` in the `lsp`
   layer, the float with a preview, files outside the project joining the server, and the
   channel's rules.
9. **Completion in the buffer:** the popup, its keys and cycling, matching, the automatic trigger,
   `resolve` and `additionalTextEdits`, and many cursors.
10. **Hover:** `C-c C-d` and `C-h .`.
11. **Agents:** the `diagnostics` tool, `state`'s fields, the tool descriptions, and ADR 0121's
    `@diagnostics`.
12. **The `just perf` metrics,** after the `rust-analyzer` component in `rust-toolchain.toml`,
    which goes in first in a pull request of its own.

Steps 5, 6 and 7, and 8 to 10, can run beside each other once step 4 has merged. Windows builds
and tests in CI in each step.

Later, after these, each with its own decisions put to the author first:

13. **Code actions on `C-c C-r`,** as the author's `eglot-code-actions`: a server's edits applied
    only after the person picks an action, so `workspace/applyEdit` gets an answer other than
    `applied: false` only then.
14. **Rename,** as `eglot-rename`: the new name read in a prompt, and the edits across files
    shown before they apply.

## Questions

Put to the author on 2026-11-15, the recommended option first. The answer follows each.

### The model

1. **Which Emacs set-up oso follows.**
   (a) Eglot, with Flymake, xref, Corfu-style completion at point and ElDoc. It is minimal, feeds
   what oso already has, and is built into Emacs.
   (b) lsp-mode with lsp-ui and Company: more features, such as the sideline, peek and
   breadcrumbs, but much more UI.
   (c) Your own set-up. Say what you use, and oso follows its behaviour where it differs.
   **Answer:** (c), the author's set-up, built on Eglot ([The author's set-up](#the-authors-set-
   up)).

### Servers, configuration and lifetime

2. **Which servers oso knows itself.**
   (a) rust-analyzer only, found on `PATH`, then in Cargo's `bin`. Others, for Markdown or TOML,
   come from the configuration.
   (b) Also taplo for TOML and marksman for Markdown, built in, used when found.
   (c) None. The configuration names every server, as ADR 0114 says today.
   **Answer:** a built-in table, so that no configuration is needed: rust-analyzer, gopls,
   TypeScript and JavaScript, texlab, sourcekit-lsp, taplo and marksman
   ([Which servers](#which-servers-q2)).
3. **The configuration's shape.**
   (a) `[lsp.servers.<name>]` with `modes`, `command`, `env` and `options` (sent as
   `initializationOptions` and as answers to `workspace/configuration`), and `[lsp] start`. The
   built-in rust-analyzer entry sets `cargo.targetDir = true`.
   (b) As (a), without `cargo.targetDir`: rust-analyzer's checks share `target` with `just ci`
   and `oso dev`, and wait on its lock.
   (c) Only `command` per mode under `[modes.<mode>]`, with no options. Simpler, but
   rust-analyzer's settings could not be changed.
   **Answer:** the table is the source, with rust-analyzer's `cargo.targetDir = true` and gopls's
   settings; `[lsp.servers.<name>]` only overrides it ([Overrides](#overrides-q3)).
4. **When a server starts.**
   (a) By itself, in a trusted project, at the person's first visit of a buffer of a configured
   mode, as `eglot-ensure` does. An agent's open waits (ADR 0118).
   (b) Only by `lsp-start` (as `M-x eglot`), and then for the rest of the session in that project.
   (c) As (a), and the first start of each server for each project asks once (`Start
   rust-analyzer for oso? (y or n)`). One more question, after trust already asked.
   **Answer:** (a).
5. **Trusting a project.**
   (a) Starts the servers for its buffers the person visited, at once. Buffers the agent opened
   keep their marks.
   (b) Starts nothing. The person's next command in such a buffer starts its server.
   **Answer:** (a).
6. **Lifetime and crashes.**
   (a) A server stays up until untrust, `lsp-shutdown` or quit. After a crash it starts again
   after 1 s, 5 s and 30 s, and stays down after three crashes in 3 minutes, saying so. Close to
   Eglot's `eglot-autoreconnect`, with a limit.
   (b) As (a), but a server also stops 5 minutes after its last buffer is killed.
   (c) No automatic restart, as lsp-mode asks first: the modeline says `lsp!` and `lsp-restart`
   starts it.
   **Answer:** (a).
7. **oso's own restarts.**
   (a) Carry no server. The new instance starts them again, and rust-analyzer indexes again.
   (b) Carry the server's pipes as descriptors, as terminals are carried. Nothing to index again,
   but the protocol's state must be carried too, and a version mismatch would desynchronize.
   **Answer:** (a).
8. **Files a jump reaches outside any project,** such as the standard library's sources.
   (a) Join the server that gave the jump, as `eglot-extend-to-xref`, so `M-.` keeps working there.
   (b) Get no server, as Eglot's default. `M-.` stops working in those files.
   **Answer:** (a).

### The protocol

9. **Where the client lives.**
   (a) A new crate, `oso-lsp`, beside `oso-term`, used only by the tip crate. It keeps the tip
   crate's build fast and the protocol testable alone.
   (b) Modules in the tip crate. No new crate, but more code there to rebuild.
   **Answer:** (a).
10. **Dependencies.**
    (a) None new: oso's own framing, and `serde_json` through `Value`, already in the tree.
    (b) `lsp-types`: every type of the protocol, but `serde`'s derive macros and `url` or
    `fluent-uri` with it, for the small part oso uses.
    (c) `lsp-server`, rust-analyzer's own crate: framing and threads, made for servers, with
    `crossbeam-channel`.
    **Answer:** (a).
11. **Positions.**
    (a) UTF-8 when the server offers it, otherwise UTF-16 through a new UTF-16 count in the
    rope's summary, O(log n) on any line.
    (b) UTF-8 only. A server without it, as many JavaScript and Python servers are, is refused.
    (c) UTF-16 by scanning the line, with no change to the rope. Lines over 64 KiB would then be
    too slow with many cursors, so such documents would not be sent.
    **Answer:** (a).
12. **A message that breaks a bound.**
    (a) A message over 16 MiB is read through and dropped, and its request fails. A broken header,
    a flood or a full writer queue stops the server, and counts as a crash.
    (b) Any broken bound stops the server. Stricter, but a big answer from a healthy server, such
    as diagnostics for a generated file, would restart it.
    **Answer:** (a).

### Diagnostics

13. **How they look.**
    (a) A wavy underline in the severity's color, a one-cell sign column while a server follows
    the buffer, and `lsp` with `E2 W5` in the modeline, as Flymake.
    (b) As (a), with a straight underline. The renderer then needs no wavy one, but it looks like
    the preedit's underline.
    (c) As (a), with the sign column only while the buffer has diagnostics. Saves a column, but
    the text moves when the first report comes and when the last fix clears it.
    **Answer:** (a).
14. **The message at point.**
    (a) In the echo area after the cursor stays 300 ms on a diagnostic, as Flymake with ElDoc.
    (b) Only on a key (`C-h .` shows the diagnostics with the hover).
    (c) At the end of the line, as Emacs 30's `flymake-show-diagnostics-at-end-of-line` and lsp-ui's
    sideline. Always visible, but it covers text on long lines.
    **Answer:** (a).
15. **Keys.**
    (a) `M-g n` and `M-g p` for next and previous, as `next-error`, and `M-g d` for the list with a
    preview, as `consult-flymake`.
    (b) `M-n` and `M-p` in buffers a server follows, as many Flymake set-ups bind, with the list on
    `M-g d`.
    **Answer:** `C-M-n` and `C-M-p`, the author's Flycheck keys, with the list on `M-g d`.
16. **Hints and unused code.**
    (a) A hint is not underlined or counted, and shows at point. An `Unnecessary` range is dim.
    (b) Hints are drawn and counted as information is.
    **Answer:** (a).

### Definition

17. **Going back.**
    (a) A history of 64 places in each window. `M-*` goes back and `C-M-*` forward, as Emacs 29's
    `xref-go-back` and `xref-go-forward` but on other keys, since `M-,` is `previous-buffer`
    (ADR 0088). The place also goes on the mark ring.
    (b) As (a), with one history for the frame, as Emacs's default `xref-history-storage`.
    (c) No history of its own: the mark ring only, through `C-.`. Less to build, but it cannot go
    back across buffers.
    **Answer:** a history for each window, with `M-,` back and `C-M-,` forward in buffers a server
    follows; `M-,` stays `previous-buffer` elsewhere.
18. **References.**
    (a) `M-?` in step 7, in the same float as several definitions. It costs one more request.
    (b) Later, after the roadmap's three.
    **Answer:** (a).

### Completion

19. **Where it shows.**
    (a) A popup at point, as Corfu, up to 10 rows, filtered by oso's matcher.
    (b) In the centred float, as `consult-completion-in-region` with vertico. One float for all
    completion, but far from the cursor, and typing goes into the prompt, not the buffer.
    **Answer:** (a).
20. **When it opens.**
    (a) By itself after two identifier characters and 200 ms with no key, or at once after a
    trigger character (`.`, `::`), as `corfu-auto`; and on `C-M-i`.
    (b) Only on `C-M-i`, as Corfu's default.
    (c) As (b), and `TAB` completes when the line is already indented, as
    `tab-always-indent 'complete`.
    **Answer:** (a), after three identifier characters rather than two, with cycling, as the
    author's Corfu.
21. **With several cursors.**
    (a) The item goes in at every cursor whose word before it is the primary's, and the extra edits
    apply once.
    (b) Completion works with one cursor only, and says so.
    **Answer:** (a).
22. **Snippets and extra edits.**
    (a) No snippets: plain text. `additionalTextEdits` (rust-analyzer's added `use` lines) apply in
    the same undo step, and the echo area names a changed line outside the view.
    (b) Snippets with tab stops, as yasnippet. More to build, and the keys must be decided.
    (c) Neither: insert the label only. An imported name then needs its `use` written by hand.
    **Answer:** (a).

### Hover

23. **Hover.**
    (a) `C-h .` shows it in a float at point, Markdown highlighted. Nothing opens by itself.
    (b) As (a), and the signature of the call at point shows in the echo area on idle, as ElDoc
    does. A request at each pause, and it competes with diagnostics' messages there.
    (c) No hover in M4.
    **Answer:** (a), on `C-c C-d`, the author's key, and on `C-h .` too.

### Agents, Windows and oso's repository

24. **What agents may do.**
    (a) `M-.`, `M-?`, the back keys and `M-g n`/`M-g p` are `Look`. Starting, restarting,
    stopping, the list, completion and hover are `Person`. A `diagnostics` tool joins `oso mcp`,
    and `state` gains each buffer's server state and counts.
    (b) As (a), without the `diagnostics` tool. The agent runs `cargo check` itself.
    (c) As (a), plus a `definition` tool that answers a path and line without opening anything.
    **Answer:** (a).
25. **Windows.**
    (a) Built and unit-tested in CI. Only `.exe` servers start. Stopping kills the server alone,
    without a job object, until the author uses Windows.
    (b) Job objects now, so a stop also ends what the server started.
    **Answer:** (a).
26. **oso's own `rust-toolchain.toml`.**
    (a) Add the `rust-analyzer` component, so rustup runs the pinned toolchain's server. It is a
    file that defines the checks, so the author approves it in step 11.
    (b) Leave it. rustup falls back to another toolchain's server, whose proc-macro server may
    not match the compiler.
    **Answer:** (a), approved, in step 12.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it. The status of each ADR named
gains `amended by 0138`.

- [ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md), "Trust never makes oso
  read a project's settings. A program that runs for a trusted project is one the person's
  configuration names": or one that oso's built-in table of servers names, which the
  configuration can change or turn off. In a trusted project, the table may choose a server the
  project holds, such as `node_modules/.bin/typescript-language-server`: oso looks only at the
  metadata of fixed names under the root, never at a file's contents or the project's settings,
  and runs the program only from inside the root. The project still never names a program.
- [ADR 0115](0115-indentation.md), "`[modes.<mode>]` … for `rust`, `markdown`, `toml` and
  `fundamental`": and for `go`, `typescript`, `tsx`, `javascript`, `latex` and `swift`, the
  modes of step 5, at the same defaults.
- [ADR 0088](0088-switching-and-killing-buffers.md), "`previous-buffer` on `M-,`": in a buffer a
  language server follows, `M-,` is `lsp-go-back`, through the `lsp` minor mode layer. Everywhere
  else it is still `previous-buffer`.
- [ADR 0053](0053-edit-core.md), "An edit with no group open is a step of its own": and the
  edits besides a completion's own that its resolve gives after the insert join the insert's
  step afterwards, as `C-x TAB`'s keys' edits join theirs (ADR 0115), if nothing changed since
  (step 9b).
- [ADR 0008](0008-floating-ui.md), "completion popups and hover documentation [are buffers]
  shown in floating windows … Editing commands … work in them unchanged": a server's
  description at point is a buffer shown in a float with no window of its own. Only `C-v` and
  `M-v` scroll it and `C-g` closes it; any other key closes it and does what it always does in
  the window beneath (step 10, Q3). The completion popup is no buffer: it is the editor's state
  of the items a server offered, drawn as a float at point, and typing goes on into the buffer
  beneath, narrowing it; `C-n`, `C-p`, `TAB`, `C-g`, and `RET` once the selection was moved,
  are its keys, and any other command closes it first (step 9).
- [ADR 0085](0085-agent-channel-and-oso-mcp.md), the channel's methods and `oso mcp`'s tools: and
  `diagnostics`, a read tool like `text`; `state` gains each followed buffer's `server` and
  `diagnostics`, and its version goes to 3, as ADR 0118 bumped it for a buffer's new field
  rather than keeping it as ADR 0103 has it (step 11).
- [ADR 0121](0121-agent-sessions-and-spaces.md), "`@diagnostics` … (M4)" and "`session-fix-error`
  (`Person`): the diagnostic at point, its 20 lines around": both are built (its Q5), as
  `session-send-diagnostics` and `session-fix-error`, the latter with the 10 lines either side,
  and the server's message in a fence of its own under a line saying a language server wrote it
  (step 11).
- [ADR 0143](0143-agent-keys-on-c-c-a-and-an-agents-menu.md), the Talk group: and `!`
  (`session-send-diagnostics`) and `x` (`session-fix-error`) (step 11).

## Consequences

- oso gets diagnostics, definitions, references, completion and hover for Rust, Go, TypeScript
  and JavaScript, LaTeX, Swift, TOML and Markdown, with no configuration, for each server that
  is installed.
- A language server runs only for a trusted project and is started only by the person. A broken
  or hostile server can slow only its own threads, and never blocks the UI thread, edits a
  buffer but within an insert the person made (a completion's edits besides its own, step 9b),
  opens a file by itself, or makes oso reach another computer.
- oso gains a crate, and `oso-core` gains `diagnostic`, `window::jumps`, ADR 0118's enum,
  completion at point and six modes with no highlighting yet. The renderer gains a wavy underline and three faces in each theme.
- The gutter gains a column in buffers a server follows, and `M-,` means another thing there.
- In a trusted project with `node_modules`, the TypeScript server oso runs is the project's own.
- rust-analyzer indexes again after each restart of oso.
- The server runs with the person's full rights until OS sandboxing comes, after M4.
- Later steps: code actions and rename (steps 13 and 14). Follow-up, not in this ADR: format on
  save (its own `Program` kind under ADR 0118, which gopls's `gofumpt` then serves), signature
  help, semantic highlighting, inlay hints, pull diagnostics, highlighting for the new modes, and
  TypeScript servers on Windows.

## Alternatives considered

- **Starting servers in every project, as some editors do:** opening a repository would run its build
  scripts. ADR 0009 forbids it.
- **Downloading servers, as lsp-mode can:** oso has no network code.
- **Requiring the configuration to name every server (Q2 (c)):** safest to read, but the author
  wants servers to work with no configuration, as Eglot's defaults do.
- **TypeScript only from `PATH`, never the project's:** the project's server matches its
  TypeScript version, which the author's set-up relies on; trust already covers running it.
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

## As built

### Step 1: what an agent's open or save starts (ADR 0118's code)

- **`oso_core::buffer::Start`** lists the kinds, `ServerStart`, `ServerOpen` and `ServerSave`, each
  with its `Runs` from an exhaustive match, an id for the handoff (`server-start` and so on) and
  what is shown of it (`server`). `Starts` is a set of them. ADR 0118's list of oso's own work is
  not routed through the enum: nothing in it runs a program, so `Runs::InOso` has no kind yet.
- **Each buffer has two marks** (`open_waits`, `save_waits`). `Editor::starts_on_open` and
  `starts_on_save` take the buffer, what would start for the person and who acted (`Acted`),
  and return what runs now. A project that is not trusted runs and keeps nothing. Untrusting,
  or a record that stops trusting a root, clears its buffers' marks.
- **What lets an open's mark go** is `Editor::note_persons_command`, called after every command
  `Dispatcher::execute` runs that is not the agent's, macro replays and the dev socket's
  `send-keys` included, and after a click that selects a window. Not after the person's answer to
  the agent's question (`y`, `n`, `C-g` or `?`, Q11), nor after the dev socket's `run-command` or
  the window's close request, which any process may send (`Dispatcher::not_as_input`). The host
  takes the marks let go (`take_released_starts`) and works out again what would start.
- **Letting an agent's open go makes the open the person's** (`Buffer::open_let_go`), and a
  restart carries that. Every other open of the agent's settles as the agent's, also after a
  restart: one not settled before it, or that held nothing, as in a project restricted then and
  trusted since, is held again rather than run (Q7, Q8). Marks are cleared only for a buffer
  whose project is known and not trusted, so a carried mark stays while the new instance looks
  for the project.
- **`app/starts.rs` settles each open once:** when the file has loaded, its project is found and
  the record of trusted projects is read, after a load ends, a project is found or the record is
  read. A buffer the agent opened is the agent's, unless a restart carried it. A save's starts
  are settled as it ends, unless it failed, before the agent's reply goes, by who started it.
- **The agent's reply** to `open` has `waits` (`"server waits for the person in lib.rs"`) when the
  project was found by the time the file showed. Otherwise only `state` says so, a moment later.
  The reply to a save has `waits` whenever its save holds something. `state`'s buffers list
  `waits`, and its version is 2. The handoff carries `waits_open`, `waits_save` (by id) and
  `open_let_go`, each only when it holds something. An id the build does not know holds every
  kind that runs a program, so the handoff fails closed.
- **`C-x C-s` in an unmodified buffer with a save waiting goes ahead,** for the person only: the
  agent's save of it still says there is nothing to save. The job writes only if the file has
  gone, as for any unmodified buffer, and its end lets the mark go.
- **The modeline** shows `waits: server` dim, after `restricted`. `*Agent*` says what waits, what
  the person let go and, in a dev build run with `OSO_DEV_STARTS=1`, what the stand-in ran: it
  makes a Rust file in a trusted project start a server on open and tell it of a save. Nothing
  else starts yet. `just release-safety` checks that a release binary does not hold the
  variable's name.
- **Public API in `oso-core`:** `buffer::{Start, Runs, Starts, Acted}`, `Buffer::{open_waits,
  save_waits, waits, carry_waits, open_let_go, carry_open_let_go}`, `Editor::{starts_on_open,
  starts_on_save, note_persons_command, take_released_starts}` and `Status::waits`.
- **Tests:** unit tests of each rule in `oso-core` (`editor/starts_tests.rs`, the modeline, the
  save), dispatcher tests that the person's key lets go and that the agent's command, the
  person's answers to its question and the dev socket's `run-command` do not, the handoff's
  format, a guard test listing the `Program` kinds (`app/starts.rs`), and the
  end-to-end flow `starts.rs`: an agent's open waits for the person's key, an agent's save for
  the person's `C-x C-s`, and a restricted project holds nothing.

### Step 2: `oso-lsp` without spawning

- **The crate `oso-lsp`** depends on `oso-core` and `serde_json` only; the edges to
  `oso-platform` and from the tip crate come with step 3, when something spawns. Its modules
  are `framing`, `json`, `uri`, `position` and `limits` (`guardrails/layers.txt`, `[oso-lsp]`),
  and its public API has a snapshot (`guardrails/public-api/oso-lsp.txt`, through the
  `Justfile`'s `api_crates`, as the author approved).
- **`limits`** holds every bound of the table, each with the step that enforces it, those of
  later steps included. Three were added with the author on 2026-11-16, from this step's
  security review: a declared `Content-Length` of at most 256 MiB, 1,000,000 JSON values,
  and 16 KiB for a URI. It has two of its own: the size of a read and of the pieces an
  oversized message is read through in (64 KiB), and at most 19 digits in a
  `Content-Length`. `serde_json`'s nesting limit of 128 lets arrays and objects nest 127 deep.
- **`framing::Reader`** reads a header of CRLF lines only: a bare LF, a CR inside a line, or
  a byte that is not visible ASCII, a space or a tab breaks it. Header names are read without
  case, and headers other than `Content-Length` are counted and ignored. A body grows only as
  its bytes come. It returns `Frame::Message`, `Frame::Dropped` for a message over 16 MiB,
  `None` at a clean end, or `FrameError::{Broken, Truncated, Io}`. A frame's `Debug` shows its
  length, never its bytes. `encode` writes a message.
- **`json::parse`** counts values first, one plus each `[`, `{` and `,` outside strings, an
  upper bound on what parsing makes, and gives `JsonError::TooManyValues` past the bound: the
  message is dropped, and the server is not stopped. `JsonError::NotJson` says where a message
  stopped being JSON, never what it held, and does stop it. A string is cut at a character
  boundary, an array to a count, and `uinteger` and `integer` are read in the protocol's ranges
  (0 to 2³¹ − 1, and `i32`).
- **`uri`** also refuses a query or fragment, a bad `%` escape, a control written as it is,
  and segments that are empty, `.` or `..`, or hold `/` or NUL once decoded. On Windows it
  also refuses `\`, `:` (a stream), the characters Windows forbids, a trailing dot or
  space, and the devices' names (`NUL`, `COM1`, `LPT¹` and the rest, with any extension), and
  reads `\\?\C:\` paths as `C:\`. A trailing `/` names nothing more. Paths are written with
  every byte but RFC 3986's unreserved ones as `%XX`.
- **`position`** converts in O(log n) through the rope's counts. `Encoding::chosen` reads the
  server's `positionEncoding`: UTF-16 when it says nothing, and `None` for one oso did not
  offer. A range that ends before it starts is read the other way round.
- **Lines are oso's.** `\n` and `\r\n` end a line, and a lone `\r` does not, where the protocol
  counts one too. A buffer that holds a lone `\r` is not opened with its server, as with the
  8 MiB limit.
- **`oso-core`** gains `TextSummary::utf16` and `Rope::offset_to_utf16` and
  `Rope::utf16_to_offset`, O(log n), in `rope/utf16.rs`. A chunk's line breaks, characters
  and 4-byte characters are now counted in one pass over its bytes (`rope/count.rs`), with
  8-bit counters that run on vector lanes. On the reference Mac, `text_perf` builds a rope
  of 100 MB of source-like text in 15.5 ms, from 24 ms before, appends it in 64 KB pieces
  in 25 ms, from 33 ms, and types with 1000 cursors as before (p50 2.1 ms).
- **The fake server** is the example `fake-server`, `examples/fake_server/`. It reads with
  `framing`, and builds its replies by hand, framed by `framing::encode`. A JSON script names,
  for each method and optionally its nth message, its replies, errors, notifications,
  requests to the client, raw bytes, oversized messages, floods, hangs, pauses, standard error
  and exit. `test = true` runs its script's unit tests in `just ci`. Nothing spawns it yet:
  step 3's end-to-end flows do. Its own copy of a document's text, for step 4's property
  tests, comes with step 4.
- **Tests:** each bound at it and past it; round trips through reads of every size; positions
  against a naive scan in both encodings, over texts that cross the rope's chunks; URIs
  against a check of the path they read as that does not share the module's rules; and a
  proptest `fuzz` target in each module's tests, with a `PROPTEST_CASES` note for a long run.
  There is no cargo-fuzz harness, which would need `libfuzzer-sys`.
- **Left for later steps, from this step's reviews:**
  - Step 3: a message dropped for its size carries no id, so its request fails when its wait
    ends, unless step 3 reads an id from its first piece. A server that chooses an encoding
    oso did not offer is stopped, as a crash.
  - Step 4: an edit that splits a `\r\n`, or leaves a lone `\r`, in an open document would put
    the server's copy out of step: oso and the server would count that line differently from
    there. Each batch is checked against the rope after it, O(splices), and such a batch is
    sent as the whole text, or the document is closed. The same holds for a document that grows
    past 8 MiB. Step 4's property tests put `\r` among their characters.
  - Steps 4 and 8: `uri`'s rules are rules of text. A path that the system maps to another
    computer (macOS's `/net` automount, a mapped drive on Windows), a device file on Unix, or a
    Windows 8.3 short name such as `ENV~1` passes them. What opens a path from a server goes
    through the loader's checks of a regular file, and whether it may hold secrets is judged
    from the file opened.

### Step 3: a server's life

The author answered step 3's questions on 2026-11-16, all with the first choice: the fake
server is a binary of the tip crate's dev builds (Q1); `lsp-shutdown` keeps a server down
for its root and entry until `lsp-start` or `lsp-restart` (Q2); the built-in rust-analyzer
entry also passes `CARGO_HOME` and `RUSTUP_HOME` (Q3); an oversized answer that is dropped
fails its request when its wait ends (Q4); a server is told only of the capabilities this
step uses (Q5); the public items below (Q6); `*LSP*` keeps its last 1 MiB (Q7); and the
layers of `oso-lsp` below (Q8).

- **`oso-platform::process`** has `Signal`, `Child::terminate` and `Child::signal_group`
  from [ADR 0127](0127-source-control-and-github.md)'s step 1b, which needed them first, and gains
  `Child::exited`: whether the child ended, without reaping it (`waitid` with `WNOWAIT`),
  so that its group can still be signalled for what it left there. A group is never
  signalled once the child is reaped, as its number may be a stranger's by then. On macOS
  a group that holds only the ended child refuses a signal with `EPERM`; the callers take
  that as nothing left.
- **`oso-lsp`** gains `protocol` and `server` (`guardrails/layers.txt`: `server` over
  `protocol` over `framing json position uri` over `limits`), and depends on
  `oso-platform`:
  - `protocol` tells a message apart as an answer, a request or a notification, and anything
    else stops the server. A method's name over 256 bytes is not JSON-RPC as oso reads it; a
    server's string id over 256 bytes gets no answer. It builds `initialize`, which claims
    only positions in UTF-8 or UTF-16, work done progress and `workspace/configuration`
    (Q5), with the root as the one workspace folder and the entry's settings as
    `initializationOptions`. It answers the server's requests as the bounds above say, with
    `workspace/configuration` from the settings under the entry's section, a dotted section
    reading into them, and at most 256 items. `merge` merges settings key by key.
  - `server::Server::start(Start, Gate, Wake)` starts the session thread, which starts the
    server. `Gate` runs on that thread before each start: the tip crate's checks the trust
    and the root, and finds the program, so nothing of either runs on the UI thread. The UI
    asks with `restart`, `shut_down` and `quit`, and takes `Report`s: the `State`, lines for
    `*LSP*`, and `Shown` messages. A new kind of state and a message wake it at once; progress
    and log lines at most each 50 ms.
  - The threads are `oso-lsp-N` (the session), `-w`, `-r` and `-e`. The reader holds at most
    32 MiB of frames the session thread has not taken, and then waits, so a slow session holds
    the server's writes up rather than oso's memory. The error reader keeps the last 256 KiB
    of the standard error as lines cut to 1 KiB, and the session thread takes them.
  - A crash, for any of the reasons of the bounds above, or because the server exited, did
    not answer `initialize` within 60 s, refused it, or chose an encoding oso did not offer,
    sends `SIGTERM` to the group, then `SIGKILL` after 2 s, and reaps it. The server starts
    again after 1 s, then 5 s, then 30 s, and the third crash within 3 minutes stops it for
    the session, which the echo area and `*Messages*` say once. The flood bound counts in
    seconds: any 10 whole seconds, the current one included.
  - `shut_down` sends `shutdown` (2 s), then `exit` and closes the input (2 s), then
    `SIGTERM` to the group (2 s), then `SIGKILL`. `quit` sends `SIGTERM` to the group from
    the UI thread at once, under a lock the session thread holds only for calls that never
    wait, and closes the input. A `Server` dropped is shut down.
  - Diagnostics reports are counted and dropped until step 6, and `*LSP*` says how many.
- **The tip crate:**
  - `config/lsp.rs` reads `[lsp] start` and `[lsp.servers.<name>]`: `enabled`, `modes`
    (`rust`, `markdown`, `toml`), `command` (a name on `PATH` or an absolute path, at most 64
    words), `env` (as `[sessions] env` refuses them) and `options` (TOML as JSON, nesting at
    most 32 deep, a date as its text). At most 64 servers, each name at most 64 bytes of
    letters, digits, `-`, `_` and `.`. The built-in table has rust-analyzer only; step 5 adds
    the rest. A command of the person's is looked for only where it says.
  - `app/servers.rs` keeps one server per root and entry. An open whose mode has a server
    and whose project has none running would start one, as ADR 0118's `server-start`; the
    person's runs at once, and the agent's waits for the person. `server-open` and
    `server-save` start nothing until step 4, which tells a server of an open and a save.
    `app/servers/find.rs` finds the program: an absolute path, or `PATH` without the project,
    then `$CARGO_HOME/bin` or `~/.cargo/bin` for the built-in entry, an `.exe` only on
    Windows. A program not found stops the server for the session, and says where it was
    looked for, once.
  - Trusting a project settles its buffers' opens again, so the ones the person visited
    start their server. Untrusting it stops its servers. A configuration change stops a
    server whose entry went, and starts again one that stopped for the session whose entry
    changed; a running one takes a change at `lsp-restart`. Quitting oso sends each group
    `SIGTERM`, with no wait, and a restart of oso carries no server.
  - The modeline shows `lsp` after the project: dim while it starts or waits to start again,
    `lsp 34%` with progress, plain when ready, `lsp!` in the warning face when stopped, in
    each buffer of the server's root and mode. `*LSP*` names each line's server and project:
    `[rust-analyzer oso] initialized in 812 ms, positions in UTF-8`. A person can visit it;
    it keeps its last 1 MiB, in whole rows. A server's message names it first:
    `rust-analyzer: …`.
  - `lsp-start`, `lsp-restart` and `lsp-shutdown` are `Person`. They ask the host through
    `Editor::request_server`, as terminal commands do.
  - The dev build's stand-in, `OSO_DEV_STARTS`, is gone, with its marker in
    `just release-safety`. `*Agent*` now says `ran server-start in main.rs` of every start
    that ran.
- **Public API:** `oso-platform`: `process::Signal`, `Child::{terminate, signal_group,
  exited}`. `oso-lsp`: `protocol` (`Incoming`, `Outcome`, `RequestId`, `NotRpc`,
  `Initialize`, `Token`, `Progress`, `Severity`, `classify`, `request`, `notification`,
  `answer`, `initialize_params`, `chosen_encoding`, `server_name`, `answer_for`, `progress`,
  `message`, `merge`, `METHOD_NOT_FOUND`, `INVALID_PARAMS`), `server` (`Server`, `Start`,
  `Gate`, `Wake`, `State`, `Shown`, `Report`), and the new bounds in `limits`. `oso-core`:
  `buffer::ServerMark`, `Buffer::{server, set_server}`, `editor::{ServerFace,
  ServerRequest}`, `Status::{lsp, lsp_face}` and `Editor::{request_server,
  take_server_requests, server_log, server_log_shown, message_unlogged}`.
- **The fake server** moved to `crates/oso/tests/fake_lsp_server/`, the dev-only binary
  `fake-lsp-server` (`required-features = ["dev"]`). `just release-safety` checks that a dev
  build makes it and the release build does not. With `"log_answers": true` in its script,
  it writes each answer the client gives its requests to its standard error.
- **Tests:** `protocol`'s rules and a fuzz target over any JSON; the crash policy, the flood
  window and the outbox's wakes and bounds as units; `crates/oso/tests/lsp_server.rs`
  against the fake server: a server's start, answers to its requests, messages, standard
  error and shutdown, crashes and their restarts, the third crash, a broken header, a
  message that is not JSON or JSON-RPC, an encoding not offered, a refused `initialize`, a
  flood, an oversized message dropped, progress, a closed gate, a server that ignores
  `shutdown` and `exit`, and a quit. `oso-platform`: `exited` leaves the child to be waited
  for, and its group reached. The configuration's `[lsp]`, `find`, the editor's requests and
  `*LSP*`'s bound have unit tests. The end-to-end flows: `lsp.rs` (a visit starts the server
  and the modeline says so; `lsp-shutdown`, a visit that starts nothing, `lsp-start`;
  untrusting; three crashes and `lsp-restart`; a quit ends the server) and `starts.rs`, which
  now waits for the person's key before an agent's open starts a project's server.
- **From this step's reviews:**
  - `*LSP*` takes a report's lines in one edit, escaped as `*Messages*`' rows are, and the
    agent channel never reads it: a server's text can quote files that may hold secrets. A
    server's message reaches the echo area and `*Messages*` but not the log, which gets only
    its size (`Editor::message_unlogged`).
  - Work beginning and ending wakes the UI at most each 50 ms, as progress does; only a
    start, a stop and an end wake it at once. A report that changes no state marks no buffer,
    and draws a frame only for a message or a shown `*LSP*`. Dropped diagnostics are counted
    in `*LSP*` once a second.
  - Every kept line of the standard error, and every frame the reader hands over, counts a
    fixed cost beside its bytes, so empty lines and tiny messages stay within the bounds. A
    `workspace/configuration` answer that would pass 16 MiB is refused before it is built.
  - Untrusting a project stops its servers after the command, before the trust record is
    written and read back, so a server waiting to start again finds its gate closed.
  - After a server has ended, its group gets `SIGKILL` 250 ms after `SIGTERM`, before the
    server is reaped, so what it left there ends even if it ignores `SIGTERM`. A look at the
    process that fails counts as no end, and a process that outlives `SIGKILL` is reaped by a
    thread of its own.
  - `lsp-start` or `lsp-restart` while a server stops starts it again once it has ended,
    and lifts an `lsp-shutdown`.
  - No open settles before the configuration file was looked at once, so the built-in table
    never starts a server ahead of the person's `[lsp]`: an `enabled = false`, `start =
    "manual"` or a `command` of their own. The first end-to-end run found it, as the
    machine's rust-analyzer started in place of the fake server.
  - The end-to-end flows and the review bench that trust a project of Rust files set
    `[lsp] start = "manual"`, so no language server of the machine's runs in them.
  - The rustup proxy that runs rust-analyzer reads the project's `rust-toolchain.toml`,
    which can name a toolchain inside the project. So in a trusted project, the project can
    choose the rust-analyzer that runs, as it chooses its build scripts; trust covers both.
- **Left for later steps:**
  - Step 4: what an agent's save would start, and its end-to-end flow, come back with
    `server-save`.
  - An agent's open in a project whose server already runs holds nothing until step 4's
    `server-open`.

### Step 4: document sync

The author answered step 4's questions on 2026-11-16, all with the first choice: a buffer a
server follows keeps the same edit log as one highlighted, up to 8 MiB (Q1); a document kept
from its server for a lone carriage return or its size opens again at the person's next save
if it fits (Q2); the modeline's `lsp` shows only where a document is open (Q3); only the last
text sent is kept, the 16 versions coming with step 6 (Q4); and the public items below (Q5).

- **`oso-core`:** a buffer's batch log (`buffer/highlighting.rs`) is kept while the buffer can
  be highlighted, as before, or while a server follows it (`Buffer::set_followed`), whatever
  its mode or size; highlights still follow only the first. `Buffer::edits_since(revision)`
  gives the batch from a revision to now, with the log's caps of 64 revisions and 4096
  splices; past them, or after a load or reload, it gives none and the whole text goes. A
  buffer that may hold secrets, or an internal one, keeps no log for a server whatever the
  host says.
- **`oso-lsp`:**
  - `Server::{open, change, close, save}` take a `Document` (id, URI, language, text) and a
    `Change` (the text as last given, the text now, the splices between, if known). The
    session thread keeps each document across runs and opens it again after each
    `initialize`; asks that come before the server is ready only change what it is opened
    with.
  - `server/documents.rs` builds each `didChange`: one change per splice, highest first, its
    positions from the text before in the agreed encoding, its text from the text after. A
    batch whose splices are not ascending and apart on character starts, or do not take one
    length to the other, goes whole, as does every batch to a server that asks for full sync.
    The version goes up by one for each open and each batch.
  - A lone carriage return keeps a document from the server: at open by a scan of its text,
    and after each batch by a look at each splice's bytes and the one around it, O(splices and
    inserted bytes). So does a text over 8 MiB. Such a document gets `didClose`; `*LSP*` says
    why, and the report's `documents` names it, `Closed::LoneCarriageReturn` or
    `Closed::TooLarge`. Its edits are still taken in, and the person's next save opens it
    again if it fits.
  - `initialize` claims `textDocument.synchronization` with `didSave`, and
    `protocol::sync` reads the server's `textDocumentSync`: opens and closes, the kind of
    change, saves. A server that asks for none gets no document.
- **The tip crate** (`app/servers/documents.rs`):
  - A buffer fits its server when it is a file's, loaded and judged, not one that may hold
    secrets, not read-only for invalid UTF-8 or binary, not internal or a terminal, in the
    server's project and mode, at most 8 MiB, with a URI.
  - ADR 0118's `server-open` and `server-save` run now. An open whose server runs opens the
    buffer with it, the person's at once and the agent's once the person lets it go; one whose
    server would start does both. A server that starts opens its project's buffers that fit,
    but those the agent opened and the person has not let go. The person's save sends
    `didSave`; the agent's waits for the person's next save.
  - After every event, each document's buffer is looked at: one killed, or that no longer
    fits (it may hold secrets now, or left the project or mode) is closed; one written to
    another file or set to another mode is closed and opened again as it is, if it fits; one
    whose revision moved hands over the text sent, the text now, both O(1) clones, and
    `edits_since`. Undo, sliced undo, reload and query-replace are batches like any other.
  - The modeline's `lsp` shows in buffers whose document is open and not kept from the
    server.
- **The fake server** keeps its own copy of each document with `"log_documents": true`,
  applying each change with positions in the script's `"encoding"` by a scan of its own, and
  writes `doc <uri> v<version> len=<bytes> hash=<FNV-1a 64>` after each open and change.
- **Tests:** in `oso-lsp`, changes replayed by a scan of their own equal the text after for
  random batches in both encodings, the check after a batch finds what a scan of the whole
  text does (and fails when its window is narrowed by a byte), and a document's opens,
  changes, saves, closes and reopening. In `oso-core`, the log of a followed buffer of any mode
  or size, and none for one that may hold secrets. Against the fake server
  (`tests/lsp_server.rs`): 150 random batches of multibyte characters, surrogate pairs and
  `\r\n` keep its copy equal to the text in UTF-8 and in UTF-16, batches before the server is
  ready go with its open, and a lone carriage return closes the document. End to end:
  `starts.rs` has its save part back, and an agent's open where the server runs waits too;
  `lsp.rs` types into a document and finds the fake server's copy, and `C-x C-w` and a kill
  close it.
- **Public API:** `oso-core`: `Buffer::{set_followed, edits_since}`. `oso-lsp`:
  `server::{Document, Change, Closed}`, `Server::{open, change, close, save}`,
  `Report::documents`, `protocol::{Sync, SyncKind, sync}`.
- **From this step's reviews and its first end-to-end run:**
  - A whole text, an open or a change of the whole text, waits for room: one is built and sent
    at a time while the writer's queue is under half its bound, so opening many large
    documents never fills it, and a whole text that waits goes as the latest. So a server that
    wants the whole text at each change gets one at the pace it reads, and a save waits for the
    text it follows.
  - A batch is placed once, against the text the server has: that answer decides both the
    carriage return check, near the splices or of the whole text, and what goes. A batch
    before the server is ready is judged as one after it, so no open carries a text over the
    bound or with a lone carriage return. Every open reports its state, so a report about an
    earlier document of the same buffer never stands for a later one.
  - A reload stays a batch for a server: the syntax worker gets a cut-off at its revision
    instead of the log being dropped. A single batch since a revision is copied, not composed.
  - After `C-x C-w` the document closes and the buffer's open is settled again once its
    project is known, so ADR 0118's rules apply to the new file; the write tells the old
    document of no save. A change of mode closes the document, and only a visit or
    `lsp-start` opens it again, as the agent can set a mode; for the same reason a server that
    starts opens only buffers in the mode their file's name gives.
  - The UI looks at each server's documents in one pass over the buffers after an event, and
    marks the modeline again only when a document closed.
  - `protocol::sync` tells of saves only when the server asks without wanting the text, and
    a bare sync kind asks for none.
- **Left for later steps:**
  - Steps 6, 8 and 9: a server that asked for no change notifications keeps the text it was
    opened with, so its positions are not to be trusted.
  - Step 8: a document's URI is its buffer's path as visited, not where a link leads, which
    the project is found from.
  - A test of the agent's open through `*LSP*` itself is not possible in the flows: visiting
    `*LSP*` is the person's command, which lets the open go.

### Step 5: the rest of the table

The author answered step 5's questions on 2026-11-16, all with the first choice: each new mode
has a `[modes.<name>]` table of its own (Q1); a server that is not installed shows nothing in
the modeline (Q2); the flows and benches that trust a project of Markdown or TOML files set
`[lsp] start = "manual"` (Q3); a `tsc` is run as the table says, with no look at its version
first (Q4); the six modes (Q5); and the gate returns the program with its arguments (Q6).

- **Six modes in `oso-core`,** chosen by the file's name: Go (`.go`), TypeScript (`.ts`,
  `.mts`, `.cts`), TSX (`.tsx`), JavaScript (`.js`, `.mjs`, `.cjs`, `.jsx`), LaTeX (`.tex`,
  `.sty`, `.cls`, `.ltx`) and Swift (`.swift`). They have no grammar, so no highlighting, and
  indent as Fundamental does, a line keeping the one before's. Each has its `<name>-mode`
  command and a `[modes.<name>]` table, at four spaces with tabs eight wide unless the file
  says, under EditorConfig; `Editor::set_mode_indents` takes the ten modes. A document's
  language is `go`, `typescript`, `typescriptreact`, `javascript` (`javascriptreact` for a
  `.jsx`), `latex` or `swift`.
- **The table** (`config/lsp.rs`): gopls with its settings, as `initializationOptions` and
  under the section `gopls`; `typescript` for the TypeScript, TSX and JavaScript modes;
  texlab, sourcekit-lsp, `taplo lsp stdio` and `marksman server`. An entry names programs
  inside the project to try first (`InProject`), and where else to look after `PATH`
  (`AlsoIn`). A `command` of the person's replaces both.
- **Finding the program** (`app/servers/find.rs`), on the server's thread after the trust
  check: the programs inside the project, then the command on `PATH`, then Cargo's `bin` for
  rust-analyzer, or Go's for gopls: `$GOBIN`, the first `$GOPATH` entry's `bin`, `~/go/bin`. A
  program in `node_modules/.bin` is used when the file it needs is there
  (`node_modules/typescript/lib/tsserver.js` for `typescript-language-server`), the link
  leads to a regular file under the root, owned by this user, that neither group nor others
  can change. Otherwise the next choice is tried, and `*LSP*` says why. Only metadata is read.
- **`oso-lsp`:** the gate returns `server::Found`: the program, its arguments, and lines for
  `*LSP*` on what it passed over. `Start::args` is gone, so a restart picks again, and a
  project's `node_modules` that came since is used. `State::NotStarted` is a server the gate
  refused or that could not be spawned: said once, it shows nothing in the modeline, and
  `lsp-restart` or a change to its entry tries again.
- **Tests:** each mode's file names; the table's entries, the person's command replacing the
  project's choices; Go's `bin` in order; each check of a program inside the project on files
  in a scratch directory (a link out of the root, a file others can change, a directory, a
  missing `tsserver.js`, the order of the choices). End to end, `lsp.rs`: a Go file starts the
  fake server as gopls, which is told gopls's settings, and a server not installed is said once
  and shows nothing in the modeline.
- **From this step's reviews:**
  - A program inside the project, and the file it needs, are judged by ADR 0108's check of
    who can change a file (`privacy::who_can_change`): the file and every directory and link
    on the way to it must be this user's or root's alone, besides being this user's, under the
    root, and a regular file. What a swap after the check can still do is open only to this
    user and root, which trust covers. On macOS an access list is not read, as for
    `config.toml`. No program inside the project runs on Windows yet.
  - When no choice is left, why each program inside the project was passed over goes with the
    reason, so that it reaches `*LSP*` and the echo area too.
  - Cargo's and Go's `bin` directories, from oso's environment, are not looked in when they are
    inside the project or the current directory, as `PATH`'s are left out.
  - gopls takes the person's Go settings from oso's environment (`GOPATH`, `GOPROXY`,
    `GOPRIVATE`, `GONOSUMDB` and the rest), so that its module loading keeps to them.
  - A server that never started holds no agent's open for the person: it has nothing to
    open. `lsp-start` or `lsp-restart` starts it again, and opens its project's buffers then.
  - The configuration's mode names are one list, `config/modes.rs`, for `[modes]` and
    `[lsp.servers.<name>] modes`.
  - The message for a server not installed is, as shown,
    `gopls: not started: gopls is not installed (looked on PATH and in /home/me/go/bin)`.
  - A trusted project's files can make a server reach the network itself, as taplo fetching a
    schema a file names, or the TypeScript server installing type packages; trust covers it,
    as it covers the project's build.
- **Public API:** `oso-core`: `Mode::{Go, TypeScript, Tsx, JavaScript, Latex, Swift}`, and
  `Editor::set_mode_indents` over ten modes. `oso-lsp`: `server::Found`, the `Gate` that
  returns it, `State::NotStarted`, and no `Start::args`.

### Step 6a: diagnostics, model and keys

The author answered step 6's questions on 2026-11-16, all with the first choice: two pull
requests, this one without `M-g d`, and 6b with `M-g d` and its preview (Q1); bounds beyond
the table, 64 MiB of diagnostics a server in all, the oldest unopened document's going first,
256 bytes for a source and for a code, and at most 100,000 rows in `M-g d`, matched on a
worker past 5000 (Q2); `C-M-n` and `C-M-p` skip hints, leave one cursor at the start and push
no mark, as Flycheck does (Q3); the counts plain until step 7 gives them their faces (Q4);
`M-g d` previews a file that is not open with the project search's excerpt reader (Q5); a
report for a file outside the server's root dropped, and only counted in `*LSP*` (Q6); the
message at point in the echo area only, not in `*Messages*` (Q7); and the public items below,
the `revision` fields of step 4's `Document` and `Change` among them (Q8).

- **`oso_core::diagnostic`** (`guardrails/layers.txt`: `load selection diagnostic`):
  `Diagnostics`, a buffer's set, sorted by start, then the most severe first, its strings in
  one arena of the set, so a clone copies at most 2000 small entries. `in_range` finds what
  meets a range by binary search over a running maximum of the ends, O(log n) to the first,
  though a long early range makes it walk the entries before the range's end, so drawing
  asks it once for the view; `at` serves the message at point, and `step` the moves, skipping
  hints and wrapping. `splice` follows a batch in place, each start and end as a mark: text
  inserted at either edge stays outside, inside grows it, and a deletion around it leaves an
  empty range where it was. It sorts only when a tie changed order, so it allocates only the
  shift before each splice. An empty range is kept. `Counts` leaves hints out. `Debug` gives
  counts, never a message. A message's first line ends at any line break: `\n`, `\r`, NEL or
  the line and paragraph separators.
- **A buffer's set is its own** (`buffer/diagnosed.rs`), beside its highlights, and follows
  each edit in place as it commits, O(entries × log splices): an edit, an undo, a reload and
  a recovery are one step, which it follows however many splices it has; a sliced undo's
  crossing follows its composed batch, and one past the log's bound drops the set until the
  next report. A load, unfollowing, or a verdict that it may hold secrets drops it.
  `Buffer::take_diagnostics(revision, set)` maps a set through the edits since `revision`, and
  refuses one the log no longer reaches, or that does not fit the text: the set it had followed
  the edits. A set with no entries is a report of none.
- **The modeline** shows the counts that are not zero after `lsp`: `lsp E2 W5 I1`, in the
  `lsp` face, and `Status::counts` gives them for step 7's faces.
- **`oso-lsp`:**
  - `initialize` claims `textDocument.publishDiagnostics` with `versionSupport` and
    `tagSupport` (`Unnecessary`, `Deprecated`).
  - `protocol::published` reads a report: a URI of at most 16 KiB, the version if it names
    one, and its diagnostics, each with a range that reads (others are left out), a severity
    (an error when it names none, as other clients read it), its tags, and its source and code
    cut to 256 bytes and its message to 8 KiB, at character boundaries. The list is counted by
    severity first, with nothing allocated, and only the 2000 kept are read, the most severe,
    each severity's first ones, so the list holds room for no more than them.
  - The session thread reads each report's URI as a path, and drops it, only counting it in
    `*LSP*` once a second, when the path is outside the server's root or its name may hold
    secrets. Paths are compared as `uri` reads them back, the root too, so that on Windows a
    drive letter in either case and a `\\?\` root match.
  - `server/diagnosed.rs` keeps each document's last report by path, so that a document opened
    later gets one that names no version, placed against the text it opened with, shared
    rather than copied. At most 10,000 documents and 64 MiB, counting each list as allocated,
    its strings and its path, which is held twice; past either, the oldest reports of
    documents not open go first, in one pass, then the oldest of any.
    A run's end forgets them, as the next run reports again.
  - Each document keeps the 16 texts it was given last (`documents/history.rs`), with the
    splices from the one before, when they placed, the caller's revision and the version each
    went as. A whole text lets those before it go, and a document kept from the server or not
    sent yet keeps only its latest, as no report could use the others. A report naming a
    version is placed against that text, O(log n) a position, and mapped through the batches
    since, composed into one; one naming no version, against the latest sent. A version no
    longer kept, or one a whole text came after, is past: `*LSP*` says so, and the next report
    puts it right. A versioned report lets the texts before its version go.
  - `Report::diagnostics` holds the latest set of each open document, `Diagnosed::Open`, with
    the caller's revision it was mapped to, and wakes the UI at most each 50 ms, as progress
    does.
- **The tip crate:**
  - `app/servers/diagnostics.rs` takes each set into its buffer, which maps it through the
    edits since; the set it had goes to the freeing thread, as its strings can take 16 MiB. A
    document kept from its server, for a lone carriage return or its size, has its set emptied.
  - Once the primary cursor has stayed 300 ms on a diagnostic, the echo area says the most
    severe there, the server's name first, its runs of blanks as one, and `+2` when two more
    cover it: `rust-analyzer: mismatched types +2`. It replaces only an empty echo area or its
    own last message, and goes to neither `*Messages*` nor the log.
  - The `lsp` minor mode layer (`bindings/lsp.rs`) binds `C-M-n` and `C-M-p` to
    `lsp-next-diagnostic` and `lsp-previous-diagnostic` (`Look`), which call
    `Editor::to_diagnostic`: the primary cursor alone goes to a display cluster's start, past
    a diagnostic that starts inside its own cluster, and a move past the last wraps and says
    `Wrapped`. The layer is on while the buffer's modeline mark says a server follows it and
    has not stopped for the session, so a server waiting to start again after a crash keeps
    it. It shares the minor mode place of the user's `[keys.session]`, as a session's terminal
    is never a buffer a server follows, and comes after the layer of `C-x TAB` while that is
    open ([ADR 0115](0115-indentation.md)).
- **Public API:** `oso-core`: `diagnostic` (`Severity`, `Tags`, `New`, `Counts`,
  `Diagnostics`, `Diagnostic`), `Buffer::{diagnostics, take_diagnostics}`,
  `Editor::{diagnostic_at_point, to_diagnostic}`, `Status::counts`. `oso-lsp`:
  `server::Diagnosed`, `Report::diagnostics`, `Document::revision`, `Change::revision`,
  `protocol::{Published, Reported, published}` (`Reported` is the type of the approved
  `Published`'s list), `limits::{DIAGNOSTICS_BYTES_MAX, DIAGNOSTIC_LABEL_BYTES_MAX}`.
- **From this step's reviews:** the set moved out of the syntax layer's state into its own,
  and follows each edit in place rather than on a copy, also one too large for the log; the
  moves went into `oso-core` with the cluster clip, as one `Diagnostics::step`; the `lsp`
  layer went after the layer of `C-x TAB`; a report's list holds room for only what it keeps,
  and every byte the store holds is counted; texts no report can use are let go; a replaced
  set is freed off the UI thread; the batches since a version are composed before mapping.
  `protocol::Severity`, the severity of `window/showMessage`, keeps its name, though
  `Reported`'s severity is `oso_core::diagnostic::Severity`.
- **Tests:** in `oso-core`, a set's order, strings and bounds, `in_range` and `at` against a
  scan, the moves, and a property test that each start and end follows a batch as a naive
  re-map does; a followed buffer's set through edits, undo, a load, a secret verdict and
  unfollowing; the modeline's counts and the message at point. In `oso-lsp`, each bound of a
  report at it and past it with a fuzz target over any JSON; the store's bounds; placing and
  mapping by version, with no version, past a whole text and past the 16 texts, and a property
  test that a range no batch touched covers the same text after random batches in both
  encodings. Against the fake server (`tests/lsp_server.rs`): a report for the first version,
  sent after 12 random batches from the editor, lands on its word once the buffer takes it, in
  UTF-8 and UTF-16; reports outside the root, in a sibling whose name the root's begins,
  through `..`, or for `.env` are dropped and counted, never named; one for a file not open
  goes to the UI once it opens. Dispatcher tests of the layer,
  and the end-to-end flow `lsp.rs` (`diagnostics_are_counted_moved_to_and_said_at_point`).
- **Left for 6b:** `M-g d` (`lsp-diagnostics`) with its preview, and the diagnostics of files
  not open in the UI (`Diagnosed::File`); built in step 6b below.
- **Left for step 7:** drawing: the wavy underline, dim `Unnecessary` ranges, the sign column,
  and the counts each in its severity's face, `Status::lsp` then holding only the state.
- **Left for step 12:** the `just perf` metrics: typing with 2000 diagnostics at 1 and 1000
  cursors, and a 2000-entry report applied while typing.

### Step 6b: the list of diagnostics

The author answered 6b's questions on 2026-11-16, all with the first choice: the project
search's preview reader shared with step 8b's float, its two types renamed (Q1); a line a
server gives found within 64 MiB of the file's start (Q2); the list over the current buffer's
project, from every server, hints left out, the buffer's own first (Q3); rows
`path:line: severity: message` with the source and the code as the annotation (Q4); the first
diagnostic at or after point selected (Q5); the list a snapshot while it is open (Q6); and the
public items below (Q7), with the prompt opened through an argument spec of its own.

- **The preview of a list's files** (`oso-core` `editor/file_preview.rs`, from ADR 0114's
  search): `FilePreviewRequest` and `FilePreviewRead` in place of `ProjectPreviewRequest` and
  `ProjectPreviewRead`, and `PreviewAt`, a place by the offset of its line, as a search found
  it, or by its line alone, as a server gives it, with its column as a `VisitColumn`.
  `Excerpt::new` takes that column and finds it on the row's line in characters, UTF-8 bytes or
  UTF-16 units, an invalid byte one of each, past the line's end at its end. Each list keeps a
  `FilePreview` of its own: the search, `M-g d`, and step 8b's float. Its rules stay ADR
  0114's: nothing of a secret file is asked for, an open buffer of the file shows instead, and
  one of the same device and inode replaces a read excerpt, or withholds it. A diagnostic's
  row is judged by its name, as the server's thread judged its report, and the reader judges
  every file again as the search does.
- **The reader** (`grep/preview.rs`, `oso-preview`) reaches the file as before. For a place by
  line, it counts `\n` from the file's start, as a buffer counts lines, a 64 KiB part at a time
  with the generation asked before each, at most `SCAN_MAX`, 64 MiB; a line past them, or past
  the file's end, shows nothing.
- **`oso-lsp`:** a report for a file no document open with the server holds goes to the UI as
  `Diagnosed::File`, the report the session keeps, shared, with its positions' encoding; one
  with no diagnostics forgets the file's. `Store::put` gives the paths its bounds let go, and
  each goes as an empty report, so the UI never holds more than the store. A document that
  opens makes the file's go, its set being the document's now, and one that closes makes its
  kept report a file's again, or forgets it. A run's end sends `Diagnosed::Forgotten`. The
  outbox keeps the latest of each document and each file, in place, by an index;
  `Forgotten` replaces every file's that waits. The paths of the open documents are gathered
  once for a report that reaches the store's bounds.
- **The tip crate** keeps each server's file reports by path, the paths shared too, the old
  ones freed together off the UI thread. `lsp-diagnostics` (`M-g d` in the `lsp` layer,
  `Person`) reads through `ArgSpec::Diagnostics`: the prompt opens at once over the current
  buffer's project, waiting, and its request goes to a thread, `oso-diagnostics`, with a
  snapshot of the open buffers' sets and texts and the file reports of the servers whose root
  is the project's, inside it or around it, each a shared pointer, so the UI thread copies no
  path. The thread judges the reports' paths against the root as a URI reads back, as the
  server's thread does. The rows are the current buffer's by place, then each other file's by
  path and line, open in a buffer or not, from every server: another server's report of a file
  a buffer holds shows beside the buffer's set. The sources are sorted before any row is made,
  so no more than `DIAGNOSTIC_ROWS_MAX`, 100,000, are ever made, nor more than 32 MiB of paths
  and messages, as a server names any path and each row holds its own; a path from the root
  longer than 4096 bytes gives no row. The count says `+` past them. A newer list stops the
  build of an older one. A row copies at most 1 KiB of the message, and keeps at most 512
  bytes of its first line, its blanks as one, as the source and the code have theirs.
  `DiagnosticRows::new` makes the candidates on that thread. What was typed while they came is
  matched against them; past 5000 rows the completion worker matches.
  `DIAGNOSTIC_ROWS_MAX` is in `oso-lsp`'s `limits` with the other bounds of the table above,
  though the tip crate keeps it.
- **A row of an open buffer** goes to its place moved through the buffer's edits since the
  list opened, as the cursors move, clipped to its text, worked out once for each of the
  buffer's revisions, and previews the buffer there; once its log no longer reaches the list's
  revision, the row goes by its line, as a file's. A row of a file previews what the reader
  read at its line, or the open buffer of it. `RET` visits the place with `request_visit_at_column`,
  with the place before on the mark ring, through the loader's verdict for a large file.
- **Public API:** `oso-core`: `editor::{FilePreviewRequest, FilePreviewRead, PreviewAt}` (in
  place of the `ProjectPreview` pair), `Excerpt::new` with a `VisitColumn`,
  `command::ArgSpec::Diagnostics`, `Editor::{open_diagnostics_list, take_diagnostics_request,
  diagnostics_listed, take_diagnostic_hit}`, `editor::{DiagnosticsRequest, DiagnosticRow,
  DiagnosticRowAt, DiagnosticRows, DiagnosticHit}`, the place types since renamed
  `editor::{ListedAt, ListedHit}` (step 8b's Q6). `oso-lsp`: `server::Diagnosed::{File,
  Forgotten}`, `limits::DIAGNOSTIC_ROWS_MAX`.
- **From this step's reviews:** the snapshot copies no path and takes only the project's
  servers; the reports' paths are judged against the root in the servers' spelling; a file's
  report goes from the UI once its document opens; the rows have a bound in bytes and on a
  path's length; a newer list stops an older build; an open buffer's place is worked out once
  a revision and goes by line past the log; the outbox replaces in place; the reader counts a
  part's breaks at once; the source and the code have their blanks as one.
- **Tests:** an excerpt's column in each count; the list's request, rows, selection, a stale
  list, typing while the rows come, the annotation's blanks, and the preview and `RET` of both
  kinds of row, with an edit moving an open buffer's and one past its log; the rows' order,
  hints, another server's, the root, both bounds, a long path and a stale build; the reader's
  line scan, a lone `\r`, past the end and past `SCAN_MAX`; the outbox's coalescing and the
  store's victims; against the fake server, a file's report, its set once open with the file's
  forgotten, a file's again once closed, and `Forgotten` at the end; the dispatcher's `M-g d`;
  and the end-to-end flow `lsp.rs` (`diagnostics_are_listed_previewed_and_gone_to`).

### Step 7: diagnostics, drawing

The author answered step 7's questions on 2026-11-16, all with the first choice: the sign is a
bar a third of a cell wide, the row's full height (Q1), on the line a diagnostic starts on, on
its first view row (Q2), in a column at the window's left edge, before the line numbers (Q3);
unnecessary code keeps its own colours, syntax ones too, faded toward the background (Q4); and
deprecated code is struck through now, as the renderer strikes a terminal's cells (Q5).

- **Faces:** `Face::DiagnosticError`, `DiagnosticWarning` and `DiagnosticInfo`, in red (a
  terminal's colour 9), the warning face's amber and a blue of their own (`FgInfo`), in every
  theme. Their floors: 3:1 for the line and the sign, marks to notice, on the background, the
  current line's band, the region and isearch's fills; 4.5:1 for the counts on both modelines.
  A test keeps the three apart from each other, and the error's from `lsp!`'s warning face.
- **Faded text:** each theme resolves a faded colour for every face, its foreground mixed in sRGB
  toward the background, at most 0.35 of the way (`FADED`), about VS Code's opacity of 0.67,
  and less where that would take it under 4.5:1 on the background or the band: a comment, near
  its floor already, fades less. Only glyphs of text fade: an escape, a box, a mark, the glyph
  under the cursor and text on a fill other than the band keep their colours, as syntax faces
  do (ADR 0111).
- **`oso-render`:**
  - `TextView::marks: TextMarks { underlines, faded, struck }`, the frame's cells of each, and
    `TextMarks::NONE`; `PaneContent::Signs`, a column of bars.
  - Wavy lines are a third pipeline beside the rectangles and the rounded boxes (`shape.rs`,
    `wave.wgsl`): one instanced quad a run of cells, an antialiased sine at the cells' bottom,
    a line's thickness thick, swinging over three, its period twice that, its phase from the
    frame's left so runs side by side join. At most 4096 a frame (`MAX_WAVES`), the most severe
    first so it keeps its room; each layer has its range, drawn after its rectangles and before
    its glyphs, so a line is over the fills and under the text. It leaves the cursor's cells
    out, so the box, or a hollow one's sides, stays whole. The pipeline is made the first time
    a frame has a wave, not at start-up. Strikes and bars are rectangles under the glyphs, as a
    terminal's strikes are, and share its strike's line (`scene/boxes.rs`).
  - New files: `scene/marks.rs` draws the marks and the signs; `scene/boxes.rs` holds the
    cursor box, the outlines and the divider, moved out of `scene.rs`; `pane/face.rs` holds
    `Face`, moved out of `pane.rs`. `highlight.rs`'s list of plain cells became `CellRows`,
    which faded cells use too.
- **`oso-core`** (`editor/diagnostics.rs`):
  - `Editor::diagnostic_cells(id, screen, &mut DiagnosticCells)` gives each severity's cells,
    the most severe where they meet, and the faded and struck ones, as `isearch_cells` does. A
    frame works out each view row's offsets shown, two conversions a row, and asks the set once
    for what meets them all: one query, as a query costs as many entries as an early long one
    reaches over. Only a diagnostic that meets a row's offsets is converted, its ends cut to the
    rows it meets, so the part of a long line out of view and a range around the whole view cost
    a search each. A list a diagnostic takes a whole row of is one range on that row, however
    many do. A range stops at its line's text and never runs to the window's edge or under the
    wrap mark; an empty one, or one of a line break alone, takes its cell on the row it starts
    on, with its line but never faded or struck, which would read as a character the buffer
    does not hold. A preedit's cells get nothing. Hints get no line.
  - `Gutter::signs()`: the sign column is the gutter's first cell while `Buffer::server` is
    `Some` and the buffer is no terminal, with line numbers off too, so the text does not move
    when a report comes; `signs` is empty while there is none, which the tip and the text grid
    read. The line numbers' floor stays the numbers' own. The signs come from the same one
    query; each start's line is found by a search of the rows' offsets.
  - `Status::lsp` holds the state alone, and `Status::lsp_counts` the counts and the gap,
    which `Status::count_parts` splits for the severities' faces. `Status::modeline` reads as
    before. `Status::counts`, step 6's numbers, stays for the agents' `state` (step 11).
- **The tip crate:** each window has a fifth pane, its sign column (`tiled::Part::Signs`), so a
  window's pane ids moved from four to five apart and every window's rows are shaped once again
  after the upgrade; its modeline is drawn in 13 spans (`tiled/modeline.rs`, moved out of
  `tiled.rs`). A preview shows no diagnostics.
- **Tests:** contrast, faded and told-apart tests in every theme; scene tests of the waves'
  places, colours, order, cut and cap, strikes, bars and faded glyphs; a GPU pixel test of the
  wave and a wave cut around the cursor; `oso-core` tests of the cells, ranges across lines and
  wraps, a range from above the view, a view scrolled sideways, a preedit, a range on a wrapped
  line's first row alone, empty ranges, and the signs; the tip's panes; and the text-grid snapshot `diagnostics`, whose legend
  is in `docs/dev/testing.md`. The grid's modeline now has the `lsp` segment.
- **Public API:** `oso-render`: `Face::{DiagnosticError, DiagnosticWarning, DiagnosticInfo}`,
  `TextMarks`, `TextView::marks`, `PaneContent::Signs`. `oso-core`: `editor::DiagnosticCells`,
  `Editor::diagnostic_cells`, `Gutter::signs`, `Status::{lsp_counts, count_parts}`.

### Step 8a: definition and back

The author answered step 8's questions on 2026-11-16, all with the first choice: two pull
requests, this one with the ask and answer path, `M-.` with one answer, the history, `M-,`
and `C-M-,`, files outside the project joining, and the channel's rules, and 8b with the float
and its preview, several definitions and `M-?` (Q1); the history in a new `window::jumps`, as
ADR 0088 has `window::places` (Q2); 8b's preview of a file not open through the project
search's excerpt reader, made to read any absolute path by the same rules and shared with
6b's `M-g d` (Q3); 8b's rows as `path:line: text`, read on a worker (Q4); an agent never
opening the float, its `M-.` going to the first and its `M-?` refused (Q5); a joined file's
diagnostics dropped, as any outside the root are (Q6); `<server> is starting` while it starts,
with nothing asked (Q7); and the public items below (Q8).

- **`oso-lsp`:**
  - `initialize` claims `textDocument.definition` with `linkSupport`, and
    `textDocument.references`.
  - `protocol::located` reads an answer: `null`, a `Location`, or a list of `Location`s or
    `LocationLink`s, a link by its `targetSelectionRange`, else its `targetRange`, as other
    clients read it. At most 1000 are read, in the server's order, each with a URI of at most
    16 KiB and a range that reads; the rest are counted. `position_params` and
    `references_params` build the requests, the declaration included, as Eglot asks.
  - `Server::ask(Ask { serial, document, kind, revision, offset })` (`server/asks.rs`): the session
    thread places the byte offset in the text the server has, which must be the caller's
    `revision`: a document not sent, kept from the server, or whose whole text waits for room
    answers `the server does not have this text yet`. One ask of each kind for each document
    is in flight; a newer one sends `$/cancelRequest` for the older, which gets no answer. Each
    waits 10 s, then is cancelled and fails; a run's end fails what waits; with no run, or
    while it starts or stops, an ask fails at once.
  - An answer's places are read on the session thread: a URI that is not a file's is left out
    and counted; a place in a document the server has open becomes byte offsets in the text of
    the caller's revision it has (`Target::Open`), with the line and column too, for a caller
    whose buffer went or can no longer map them; others keep the line and column as the
    server counts them (`Target::Lines`), with the encoding. A place under an automount,
    macOS's `/net` and `/Network` in any case and Linux's `/net`, or with a `..`, is left out
    and counted, as opening it would reach another computer (step 8's R2). `Answer` brings
    back the ask's `serial`, and its outcome is an `Answered`, `Places` for these two kinds,
    which steps 9 and 10 add to. `Report::answers` holds the
    latest of each document and kind, and wakes the UI at once. `*LSP*` has the request's id,
    the count of places and the time, never a path; a server's error message goes only to the
    echo area.
- **`oso-core`:**
  - `window::jumps` keeps 64 places back, each a buffer and an offset, and the way forward,
    which a new jump empties. `Window::show` carries it over, as it does the places. Each
    edit, sliced undo and reload moves its offsets as markers, in one pass over the edits
    whatever the number of places, on the stack: an edit's steps (`Editor::apply_edits`), the
    batch a crossing's step made as the buffer's log has it (one that keeps none leaves them),
    and a reload's change as the cursors take it. Going there clips them to the text and to a
    cluster's start. A killed buffer's places go. `Debug` gives counts.
  - `Editor::xref_to_place` and `xref_visit` keep the place left in the selected window's
    history and, unless a region shows, on its mark ring, as avy's jumps do
    ([ADR 0116](0116-migemo-and-jumps.md)); a jump to where the cursor is keeps nothing. They
    land one cursor on a cluster's start, the place before on the target's mark ring, the rule
    a visit at a place now shares (`Editor::land_primary`). `xref_back` and `xref_forward`
    change the history only once the place is there, and say `No earlier place` and `No later
    place`; for the agent they refuse a buffer it may not be shown, as one that may hold
    secrets, is withheld, or is loading. `keep_xref_start` keeps the place for a jump the host
    makes in its own way, as the agent's. `request_visit_at_column` visits at a column in
    characters, UTF-8 bytes or UTF-16 units (`editor::VisitColumn`, with the visits), so a line and
    column of the server's land once the file has loaded.
  - `Editor::starts_on_open_in(id, would, by, project)` is `starts_on_open` with the trust
    judged on `project`, the project whose server a file outside any joined: such a file has
    no project of its own to trust, so the person's open runs what would start, and the
    agent's waits for the person, as [ADR 0118](0118-what-an-agents-open-or-save-starts.md) holds any other.
  - `ServerRequest::{Definition, References}` carry `AskAt`: the buffer, its revision, the primary
    cursor, and whether the agent's command asked (`Editor::ask_at`).
- **The tip crate:**
  - `lsp-find-definition`, `lsp-go-back` and `lsp-go-forward` (`Look`), on `M-.`, `M-,` and
    `C-M-,` in the `lsp` layer, so `M-,` stays `previous-buffer` in other buffers.
  - `app/servers/places.rs` asks after the event's edits went to the server: in a buffer that
    may hold secrets it says `No server for a buffer that may hold secrets`, with no server
    following it `No language server follows this buffer`, while it starts `rust-analyzer is
    starting`, and once it stopped `rust-analyzer does not run`. An answer goes only while the
    window that asked is selected and shows the buffer asked about. None says `No definition
    found`; one goes there; several go to the first and say `1 of 3` until 8b's float. A place
    in an open document is mapped through the buffer's edits since; any other is visited as
    `C-x C-f` would, at the server's line and column, so a name that may hold secrets opens as
    such a buffer, with no server; so is a place in an open document whose buffer went or no
    longer maps it. Each ask has a serial of the app's, and an answer whose serial is not the
    waiting ask's is dropped. An answer goes only while nothing moved since the ask: the
    same window selected, showing the same buffer at the same revision with the primary cursor
    where it was, and no prompt or isearch open. The places go to the freeing thread.
  - **Joining:** a place's file joins the server that gave it by its buffer, once project
    detection has said that buffer is in no project: at once for an open one, or when the
    answer for a new visit comes (`App::projects_known`), as `Buffer::project` alone cannot
    tell "not found yet" from "none". It is then the server's as a file of its project is: its
    open is settled again, so ADR 0118 holds the agent's, and it opens with the server, gets
    its modeline mark and its `lsp` layer, and `M-.` there asks the same server. It starts no
    server of its own. One buffer joins one server; a server holds at most 64 joined, and 16
    files waiting for their project. It leaves when its document closes, as when its buffer is
    killed. A name that may hold secrets joins nothing. A file of another project follows that
    project's rules, and one outside any project visited by hand joins nothing, as in Eglot.
  - **The agent's ask:** the connection whose command ran goes with the asks it made
    (`App::sync_servers_for`); an agent's ask with none is dropped, never taken as the
    person's. Its answer is dropped once the connection or the channel is gone, and otherwise
    opens through the channel's `open`, at the place's line: a name that may hold secrets is
    refused, and `*Agent*` says so; the file shows once judged ordinary, counts toward the
    connection's 100, and is the agent's, so its open with the server waits for the person
    (ADR 0118). The place left is kept only as the file shows. Its reply went when the command
    ran, so what the open says, a refusal too, goes to `*Agent*`.
- **Public API:** `oso-core`: `window::{Jumps, JUMPS_MAX}`, `Window::jumps`,
  `Editor::{ask_at, xref_to_place, xref_visit, xref_back, xref_forward, keep_xref_start,
  request_visit_at_column}`, `editor::{AskAt, VisitColumn}`, `ServerRequest::{Definition,
  References}`, `Jumps::counts`, `Editor::starts_on_open_in`. `oso-lsp`: `server::{Ask, AskKind, Answer, Answered,
  Location, Target}`, `Server::ask`, `Report::answers`, `protocol::{Located, located,
  position_params, references_params}`.
- **Tests:** in `oso-lsp`, the three shapes of an answer, the bound and what is left out, and
  the requests' parameters; in `oso-core`, the history's walk, bound, edits and kills, the
  jumps with one cursor and the mark ring, and columns in each unit. Against the fake server,
  which gains `log_positions`: a definition asked at its UTF-16 position and placed in the
  open text and in a file not open; a newer ask cancelling the older; an old text, an error, a
  10 s wait and a crash each failing the ask; an ask with no server running. Dispatcher tests
  of the keys, and the end-to-end flow `lsp.rs`
  (`a_definition_is_gone_to_and_back_and_a_file_outside_the_project_joins`).
- **The author's answers to the reviews (2026-11-16), all the first choice:** the serial,
  `Answered` and `Target::Open`'s line and column, and `position_params` for
  `definition_params` (R1); places under an automount left out (R2); and the names
  `Editor::xref_*`, `editor::{AskAt, VisitColumn}` and `Jumps::counts`, as `jump_*` is avy's
  (R3).
- **From this step's end-to-end flow:** a visit an answer asks for starts at once
  (`App::serve_files`), as an answer comes on a server's report and not after a command;
  `1 of 3` is said before the jump, as said once a file not judged yet shows it would go with
  the window; and a joined file's open is settled with its server's project's trust
  (`starts_on_open_in`, the author's choice).
- **From this step's reviews:** the history follows undos and reloads, in one pass; the
  mark ring follows avy's rule; stale answers, and an agent's whose connection went, are
  dropped; joining goes by buffer, once its project is known to be none, through the settling
  of opens, with bounds; the session thread looks the documents up once an answer; a replaced
  answer is freed outside the outbox's lock; `Column` and its offset went to the visits, and a
  visit at a place lands on a cluster's start too.
- **Left for 8b:** the float with a preview for several definitions, `M-?`
  (`lsp-find-references`), its rows read on a worker, and the excerpt reader made to read any
  absolute path.

### Step 8b: the list of places, `M-?`

The author answered 8b's questions on 2026-11-16, all with the first choice: `RET` in the list
runs a command of its own, `lsp-go-to-xref`, whose argument only the list takes, and the host
opens the list once an answer needs it (Q1); the rows in the server's order, grouped by file
in the order each first comes, the first selected (Q2); one reference gone to at once, saying
`1 reference` (Q3); paths from the server's root, others whole with the home directory as `~`,
and the labels `Definitions:` and `References:` with the list's count (Q4, Q8); at most 64 MiB
read of a file and 256 MiB of a list, over at most the answer's 1000 places, past them a row
with no text, a newer ask or the list's end stopping the read (Q5); the place types shared
with `M-g d` as `editor::{ListedAt, ListedHit}`, renamed once step 6b had merged (Q6); the
agent's `M-?` refused with the words above (Q7); and the public items below (Q9).

- **`oso-core`** (`editor/servers/xref_list.rs`, a twin of `M-g d`'s list):
  `Editor::open_xref_list(kind, generation, root)` opens the list empty and waiting, for the
  host's list `generation`; `Editor::xref_listed` takes its rows, dropped unless they are for the list
  open; `RET` keeps the row for the host, `Editor::take_xref_hit`. A row reads
  `path:line: text`. A row of an open buffer goes where its edits since moved it, as `M-g d`'s
  rows go; any other goes to the
  server's line and column. The preview is `FilePreview`'s, as `Lister::Xrefs`: a row of an
  open buffer shows from memory; a file under the server's root is reached from it, any other
  from its own directory, so one right in a directory that is or holds the home directory
  shows nothing, by the reader's rule; a file that may hold secrets is never asked for.
- **The tip crate:**
  - `lsp-find-references` (`Look`) on `M-?` in the `lsp` layer, and `lsp-go-to-xref`
    (`Person`, `ArgSpec::Xref`), which `M-x` and the channel refuse and which only `RET` in
    the list runs; `Dispatcher::list_xrefs` opens the list for it, as `find_file_in` opens a
    prompt for the host. The command asks the host (`ServerRequest::GoToXref`), which goes to
    the row's place as an answer's jump goes: the history, the mark ring, and a file outside
    any project joining the server.
  - `app/servers/xrefs.rs`: the person's answer with several places, or `M-?`'s, starts the
    rows' build on `oso-xrefs` (`xrefs/rows.rs`) from a snapshot: the places, each open
    buffer's text, and the server's root as a URI reads back. The list opens once the rows
    come, or after 1 s waiting with them to come, only while nothing moved since the ask, as
    an answer goes; otherwise it is dropped. A newer ask stops a list that waits, and the
    list's end stops its build.
  - The rows: an open buffer's line from its text, unless it may hold secrets; any other file
    read once for all its rows by `grep/preview/lines.rs`, reached as the preview reaches its
    file, within the bounds of Q5. At most 256 bytes of the line from its first non-blank,
    each invalid byte one U+FFFD, `…` where more follows.
  - The agent's `M-.` still goes to the first of several, saying `1 of 3`; its `M-?` asks
    nothing.
- **What the two lists share** (`editor/servers/listed.rs`, done once step 6b had merged, as the
  architecture review asked): `ListedAt` and `ListedHit`, renamed from `DiagnosticRowAt` and
  `DiagnosticHit`; `Listed`, the state each list keeps beside its rows (its prompt, root,
  generation, whether its rows came or were cut, their places, `RET`'s row and its preview),
  with how it opens, takes its rows, maps an open buffer's row through the edits since and
  closes; and `Editor::show_listed`, which shows the rows, what was typed meanwhile matched
  against them. Each list keeps only its rows' form and its preview's choice of file.
- **From this step's reviews:** a row never reads the file of an open buffer that withholds
  its text, by its path or its device and inode, as the preview never shows one; a place under
  `/dev`, `/proc` or `/sys` is left out with those under an automount, the jump's too; on
  Windows a name with `~` and a digit, as a short name, is taken to may hold secrets, the
  search's preview's too; a place whose path is not UTF-8 or longer than 4 KiB gives no row;
  the home directory's real place is found on `oso-xrefs`, and an answer's places in open
  documents are looked up once per document; at most two builds read files at once; the last
  list's server is kept until the next list, so `RET`'s row goes to it however late the host
  takes the row, as a command a macro replays runs before it looks; and what the list held is
  freed off the UI thread.
- **Public API:** `oso-core`: `editor::{XrefKind, XrefRow, XrefRows, ListedAt, ListedHit}` (the
  last two in place of `DiagnosticRowAt` and `DiagnosticHit`), `Editor::{open_xref_list,
  xref_listed, take_xref_hit}`, `ServerRequest::GoToXref`, `ArgSpec::Xref`. The tip's
  `Dispatcher::list_xrefs`.
- **Tests:** in `oso-core`, the list's rows, a stale list, the open buffer's mapping, the
  preview's root for a file under the root and outside it, and a file that may hold secrets;
  the reader's lines in one pass, across parts, within the budget, a stale read, and a file
  reached as the preview reaches it, never one that may hold secrets, through a link or under
  the home directory; the rows' order, paths and texts; dispatcher tests of `M-?`, `RET` and
  `M-x lsp-go-to-xref`; and the end-to-end flows
  `a_definition_is_gone_to_and_back_and_a_file_outside_the_project_joins`, which now picks the
  place of two definitions in the list, and `references_are_listed_by_file_and_one_is_gone_to_at_once`.

### Step 9a: completion at point, the popup

The author answered step 9's questions on 2026-11-16, all with the first choice: two pull
requests, this one with the ask and answer, `C-M-i`, the popup, its keys, matching and drawing,
and inserting at one cursor and at many, and 9b with the automatic trigger, the ask again of an
incomplete answer, `resolve` and `additionalTextEdits`, and the echo of a line out of view (Q1);
the popup placed by `frame::at_point`, shared with step 10's hover (Q2), and its keys in one
transient slot of the keymaps with `C-x TAB`'s and hover's (Q3); this ADR's limits table kept,
with a `filterText` of at most 1 KiB, a `sortText` of 256 B and a text to insert of 16 KiB added
(Q4); the word the server's first edit starts, else the identifier characters before the
cursor (Q5); `resolve` on selection, a late `additionalTextEdits` joining the insertion's undo
step only if nothing changed since (Q6, 9b); the popup opening by itself only on the person's
own keys (Q7, 9b); `completion-at-point` `Person`, the agent seeing nothing of the popup (Q8);
and the public items below (Q9).

- **`oso-lsp`:** `initialize` claims `textDocument.completion`, with no snippets, insert and
  replace edits, and the context. `protocol::completion` reads a `CompletionList` or a list of
  items within the bounds: the label and `filterText` cut at 1 KiB, the detail at 4 KiB, the
  `sortText` at 256 B, and an item whose text to insert is past 16 KiB dropped, as a cut one
  would insert the wrong text; at most 5000 kept, in `sortText` order, then as they came. A
  `textEdit` is read by its range, an insert-and-replace edit by its insert range; a snippet that
  still comes is read as plain text (`protocol::plain`). `AskKind::Completion` goes on step 8's
  ask path with its own wait, 3 s (`AskKind::wait`); its answer, `Answered::Completion`, holds
  the items with their ranges placed in bytes of the text the server has, and that text's
  revision.
- **`oso-core`:** `editor/servers/popup.rs`. `Editor::open_completion_popup` takes the host's
  answer while
  nothing moved since the ask; the word starts where the server's first edit does, else at the
  identifier characters before the cursor, whole grapheme clusters whose first character is
  alphanumeric or `_` (an approximation of `XID_Continue` that needs no new dependency). The
  items are matched against the word by ADR 0076's matcher on their `filterText` or label, in the
  server's order; each row shows the label, then the kind's word and the detail's first line.
  The word and the items' ranges follow every edit, as the jump history does; a reload closes
  the popup. After each command the word is matched again, and the popup closes once the cursor
  leaves the word, another window or buffer shows, a prompt opens, or nothing matches.
  `Editor::completion_popup_insert` replaces the word, and what the server's range covers past
  it, at the
  primary cursor, and the word at each other cursor whose word before it is the same, in one
  edit. `frame::at_point` places a float at point: under the anchor's row, or over it when fewer
  rows fit below, never over it, its text at the anchor's column moved left to fit, with a
  column of padding each side; the popup asks for as many rows as match, at most 10, and as many
  columns as its rows shown need, at most 60 and half the window.
- **The tip crate:** `completion-at-point` (`Person`) on `C-M-i` in the `lsp` layer asks the host
  (`ServerRequest::Complete`), whose answer opens the popup (`app/servers/complete.rs`); one with
  no item says `No completions`. The popup's commands, `completion-popup-next`,
  `-previous`, `-insert` and `-quit` (`Person`), are on `C-n`, `C-p`, the arrows, `TAB` and `C-g`
  in a layer above the buffer's, and `RET` only once the selection was moved, so until then
  `RET` closes the popup and breaks the line. The layer takes the place of `C-x TAB`'s, and any
  command but its own and typing (`self-insert-command`, `delete-backward-char`) closes it first
  (`dispatch/transient.rs`). It draws in `app/draw/float/popup.rs` with the completion float's
  rows. The dev state's `popup` gives its rows for end-to-end flows; the agent's state has none
  of it.
- **Public API:** `oso-lsp`: `server::{Completion, Completions}`, `AskKind::{Completion, wait}`,
  `Answered::Completion`, `protocol::{CompletionList, Offered, completed, completion_params,
  kind_name, plain}`, `limits::{SORT_TEXT_BYTES_MAX, INSERT_TEXT_BYTES_MAX}`. `oso-core`:
  `editor::{CompletionPopup, CompletionPopupAnswer, CompletionPopupItem, COMPLETION_POPUP_ROWS,
  COMPLETION_POPUP_COLUMNS}`, `Editor::{completion_popup, open_completion_popup,
  completion_popup_move, completion_popup_insert, close_completion_popup,
  follow_completion_popup, completion_popup_regions}`,
  `ServerRequest::Complete`. `frame::at_point` is the crate's own, as `completion_regions` is.
- **The author's answers to the reviews (2026-11-16), all the first choice:** the popup's names
  say it completes (`CompletionPopup`, `Editor::completion_popup` and the rest), as "the popup"
  is ADR 0076's centred float elsewhere, and the parsed answer is `protocol::CompletionList`
  (R1); ADR 0008 amended, below, for this popup and step 10's hover (R2); and with no word typed
  the item goes in at the primary cursor only, as inserting at every cursor for nothing typed
  would surprise (R3). The identifier stays the grapheme clusters' stand-in for `XID_Continue`.
- **From this step's reviews:** a server's range is kept only on the asked line and around the
  asked place, checked on the session thread against the text asked about, and in the editor
  on the cursor's line within 256 bytes of it, so that no answer edits text out of view; an
  item whose text holds a control character other than a tab or a line feed, or a carriage
  return, is dropped; a row's label and annotation are cut to what it can show and their runs
  of blanks made one, in the editor; the stale check gets the place asked about; the word and
  the ranges follow each edit once per offset (`Editor::answers_follow`, beside the jumps, and
  `answers_reloaded`); the popup's memory, and a dropped answer's, go to the completion worker;
  what is matched is the identifier before the cursor; a press, a wheel turn, a key no keymap
  binds, a menu and a prompt close the popup, as they close `C-x TAB`'s layer
  (`dispatch/transient.rs`); on a wrapped line the popup hangs from the cursor's row; its panes'
  ids are a range of their own; the asks and their answers' routing moved to
  `app/servers/asks.rs`, and a completion no longer stops a list of places that waits.
- **Tests:** in `oso-lsp`, the answer's two shapes, the order, the bounds, both edits and
  snippets as plain text; in `oso-core`, a stale answer, the word, matching as it is typed,
  closing, the wrap, inserting with the server's range and at many cursors, and where the popup
  and a float at point go; text-grid snapshots of the popup under and over the word; dispatcher
  tests of `C-M-i`, `RET`'s two meanings, `C-g` and another command, and the agent refused;
  against the fake server, the ask at a UTF-8 position past a two-byte character and the ranges
  placed in bytes; and the end-to-end flow `completion_at_point_pops_up_narrows_and_inserts`.
- **Left for 9b:** the automatic trigger, the ask again of an incomplete answer, `resolve` and
  `additionalTextEdits`, and the echo of a line they change out of view.

### Step 9b: completion as you type, and the edits besides an item's own

As the author answered at step 9's go (Q1, Q6, Q7): the person's own typing asks by itself, an
incomplete list asks again, the selected item is resolved, and its edits besides its own go in
with it.

- **`oso-lsp`:** `initialize` claims `resolveSupport` for `additionalTextEdits`, and reads the
  server's `completionProvider`: its trigger characters, at most 16 of one printable character
  each, and whether it resolves (`protocol::Completing`, handed over once in
  `Report::completing`). An ask carries its `AskContext`: a completion's `triggerKind` (the
  person asked, a trigger character, or typing on in an incomplete list), or, for
  `AskKind::Resolve`, the item by its answer's serial and its place. The session thread keeps the
  last completions' items as the server gave them while the server resolves, and sends the
  item itself in `completionItem/resolve`; an item of another answer is no one's to resolve.
  An item's `additionalTextEdits`, in the answer or its resolve, are read within this ADR's
  limits, 16 edits and 64 KiB of text, or not at all, as a part of them could leave the text
  broken; their ranges are placed in bytes of the text the server has (`Answered::Resolved`).
- **`oso-core`** (`editor/servers/popup/extra.rs`): the selected item is resolved once
  (`Editor::take_completion_resolve`), unless it came with its edits besides its own. What a
  resolve gives (`Editor::completion_resolved`) is kept with the item, followed through edits
  as its range is, and goes in with it in one edit, but for an edit that would overlap another.
  One that comes once the item was inserted joins the insert's undo step if nothing changed
  since, the buffer at the revision the insert left and the cursors where it left them;
  otherwise it is dropped, and the echo area says so. Where such an edit changes a line out of
  view, the echo area says which. `Editor::note_completion_typed` notes where the person typed,
  and `Editor::completion_typed` reads the character typed and how many grapheme clusters the
  identifier before the cursor has (`CompletionTyped`).
- **The tip crate:** the dispatcher notes typing after the person's `self-insert-command` only,
  never a replayed macro's or the agent's, which types through the channel's `change` and runs
  no command (`dispatch/transient.rs`). After the event's edits went to the server
  (`app/servers/complete.rs`), a trigger character asks at once, typing on in an incomplete
  list asks again, and three identifier characters ask once the typing has rested 200 ms with
  nothing moved; such an ask says nothing of no answer or of a failure. The selected item's
  resolve is asked for when the popup opens and when the selection moves.
- **From this step's reviews:** the late edits join the insert's step afterwards with
  `join_undo_since`, as `C-x TAB`'s do, so the next typed character is a step of its own; an
  item's edits besides its own are kept as given and mapped only when it goes in, so that no
  edit follows thousands of offsets; one ask of an incomplete list at a time, a stale answer
  asking again from where the typing is; the raw items are taken out of the answer, not
  copied, and only while the server resolves; a resolve's ranges are placed in the text it was
  asked in, as an answer's edits besides an item's own are; the late edits pass the overlap
  checks the insert's do; an edit besides an item's own replaces at most 8 lines
  (`limits::EXTRA_EDIT_LINES_MAX`), and the lines they change out of view are all said, as the
  text is after them, in `*Messages*` too; every run reports its completion capabilities;
  `AskContext` is one of none, a completion's `Invoked`, or a resolve's item, a kind with the
  wrong one failing; the typing is noted only after a successful `self-insert-command`, by its
  place alone, and read only where a server completes (`Editor::completion_typed`); and who
  asked is the person, the agent or the host (`app/servers/asks.rs`).
- **Public API:** `oso-lsp`: `server::{AskContext, Resolved}`, `Ask::context`,
  `AskKind::Resolve`, `Answered::Resolved`, `Completion::extra`, `Report::completing`,
  `protocol::{Completing, Extra, Invoked, completing, extra_edits, resolved, take_items}`,
  `CompletionList::at`, `Offered::extra`, `completion_params`' `Invoked`, and
  `limits::{TRIGGERS_MAX, EXTRA_EDIT_LINES_MAX}`. `oso-core`: `editor::CompletionTyped`,
  `CompletionPopupItem::extra`, `CompletionPopupAnswer::{serial, incomplete}`,
  `CompletionPopup::incomplete`, `Editor::{take_completion_resolve, completion_resolved,
  note_completion_typed, take_completion_typed, completion_typed}`.
- **Tests:** in `oso-lsp`, the trigger characters and resolve read, the extra edits within their
  bounds, and the three trigger kinds; in `oso-core`, an answer's extra edits in one undo step,
  the resolve asked once a selection and kept, edits resolved after the insert joining its step
  or dropped once the text changed, the line out of view said, and the typing noted; a
  dispatcher test that only the person's typing is noted; against the fake server, an item
  resolved as the server gave it and its edits placed, and one of another answer refused; and
  the end-to-end flow `typing_opens_the_completion_popup_and_a_resolved_use_line_goes_in_with_it`.

### Step 10: hover

The author answered step 10's questions on 2026-11-16, all with the first choice:
- **Q1:** the text in an internal Markdown buffer, drawn as the preview draws a buffer.
- **Q2:** the float below the cursor's row, above it with more room, sized to the text, at most 80×20.
- **Q3:** `C-v` and `M-v` scroll it, `C-g` closes it, and any other key closes it and runs; a click closes it and the wheel scrolls it.
- **Q4:** a late answer is shown only if nothing moved since the ask, otherwise dropped silently, and nothing shows while waiting.
- **Q5:** the ADR's three echo messages, never logged.
- **Q6:** Markdown mode for a markdown answer, none for plaintext, and `MarkedString` pieces joined with fences and `---`.
- **Q7:** `C-h .` only in the `lsp` layer.
- **Q8:** the public items below.

- **`oso-lsp`:**
  - `AskKind::Hover` asks `textDocument/hover` on 8a's path, with one in flight for each document, a newer one cancelling the older. `AskKind::wait` gives it 3 s (`COMPLETION_WAIT`); a definition still waits 10.
  - `initialize` claims `hover` with `contentFormat` Markdown, then plain text.
  - `protocol::hover` reads `contents` as one text of at most `HOVER_BYTES_MAX` (64 KiB), cut at a character with `…`; once it is full, nothing more of the answer is looked at:
    - a `MarkupContent` is its value, Markdown unless its kind is plain text;
    - `MarkedString`s, at most `HOVER_PIECES_MAX` (256), are Markdown, joined by `---` lines;
    - a `{language, value}` becomes a fenced block. Its fence is longer than any run of backticks in the part of the value that is kept, and its language keeps only the characters a name is spelled with, at most `HOVER_LANGUAGE_BYTES_MAX` (32).
    - `\r\n` reads as `\n`. An answer of `null`, or only blanks, describes nothing.
  - `Answered::Hover` carries it. `*LSP*` gets its size and timing, never the text.
- **`oso-core`** (`editor/servers/hover.rs`):
  - `Editor::show_hover` puts the text in a buffer of its own from `Buffer::described`, below a first row naming the server, which the float draws faded and which scrolls away with the rest, so that no server's text passes as oso's (the author's answer to the review's question, Q1 (a)). It is internal, so no list shows it, nothing saves or hands it over, and the agent channel never reads it. Unlike other internal buffers it is highlighted as its mode says: Markdown, with Rust in fenced code through ADR 0111's injection, or none for plain text.
  - The display model boxes its controls and invisibles, as any buffer's.
  - It shows only if the selected window, its buffer, the revision and the primary cursor are as asked, the buffer may hold no secrets, and no prompt, isearch, jump, query-replace or `C-x TAB` layer is open. `Editor::hover` gives it only while that still holds and the cursor is in view. One that something moved away from is closed after the next command, and an undefined key, a menu or a prompt closes it at once, so it never comes back as the place does.
  - `frame::at_point` places it in the selected window's text area, from an anchor and the text's rows and columns:
    - right below the anchor's row, or above it when fewer rows fit below;
    - never over that row;
    - its left edge at the anchor's column, moved left to fit, with a column of padding at each side.
    - Step 9's popup shares it.
  - The float is as wide as the widest line and as tall as its wrapped rows, at most 80 by 20, cut to the window's text area. Widths are measured for at most 256 lines, a seek each; a longer text takes the widest float.
  - The syntax host gives its buffer a job while it shows, the one internal buffer it does, with its whole text as a shown range, and a report for it asks for a frame.
  - Its rows are wrapped at that width once each command and each scroll, from the row shown first.
  - `Editor::scroll_hover` moves it by rows, stopping at its ends, and `hover_page` gives the float's rows less 2. `hover_layout` and `hover_screen` give its regions and rows for drawing.
  - `Editor::close_hover` empties its buffer.
- **The tip crate:**
  - `lsp-describe-at-point` (`Person`) is on `C-c C-d` and `C-h .` in the `lsp` layer. It refuses a buffer that may hold secrets, and asks through `ServerRequest::Describe`.
  - The answer goes through 8a's check that nothing moved (`app/servers/hover.rs`). An empty one says `rust-analyzer: nothing to describe here`, an error `rust-analyzer: description failed: …`, and a wait past 3 s `rust-analyzer did not answer`, each with no log line.
  - A failure is typed (Q2 (a)): `AskFailure::Overdue` for a wait that ended, `AskFailure::Other` with the reason otherwise, in place of 8a's text. A definition or references past its 10 s now says `rust-analyzer did not answer` too, as the bounds above say.
  - While the float shows, its keys take the place of `C-x TAB`'s layer ([ADR 0115](0115-indentation.md)): `C-v` and `M-v` (`lsp-describe-scroll-up`, `-down`) and `C-g` (`lsp-describe-close`). Any other command closes it first and then runs; an undefined key and a menu close it too. An answer that comes while a menu is open shows nothing.
  - A press of a mouse button closes it, and the wheel over it scrolls it; the wheel elsewhere closes it.
  - It is drawn as a framed panel with the buffer's rows in it, highlighted, with no cursor (`app/draw/float/hover.rs`).
  - The agent channel waits while it shows, as the ADR says. `get-state` gives `hover` as the row shown first, never the text.
- **Public API:**
  - `oso-lsp`: `AskKind::{Hover, wait}`, `Answered::Hover`, `protocol::{Hover, hover}`, and `server::AskFailure`, which `Answer::outcome`'s error now is (8a's `String` before). `server::Hovered` of the plan became `protocol::Hover` itself.
  - `oso-core`: `ServerRequest::Describe`, `Editor::{show_hover, close_hover, hover, scroll_hover, hover_page, hover_layout, hover_screen}`, `editor::HoverShown` with `buffer`, `top` and `label`. `show_hover` takes the server's name. `frame::at_point` is the crate's own, for step 9's popup too.
  - `oso-lsp`'s `limits` gains `HOVER_PIECES_MAX` and `HOVER_LANGUAGE_BYTES_MAX`.
  - The commands say "describe", after Eglot's `eglot-help-at-point` and `C-h`'s describe commands; the editor's API says "hover", after the protocol.
- **From this step's reviews:** the syntax host highlights the description, which it had left out with every internal buffer; a float put away never comes back, and every key, menu, wheel or press that should close it does; a buffer that came to may hold secrets shows none; the width is measured for a bounded number of lines; the reader stops once full and fences only what it keeps; its bounds moved to `limits`; ADR 0008's floats for hover are amended.
- **Tests:**
  - each shape of an answer, the fences, the bounds at and past them, a huge piece, and a fuzz test of any contents;
  - `at_point`'s placement, and the float's buffer, size, scroll and close, a buffer that came to may hold secrets, and one put away that stays away;
  - the syntax host's job for the description, and its drop once closed;
  - the keys through the dispatcher;
  - against the fake server, the ask at its position, the text never logged, a null answer and the 3 s wait;
  - the text-grid snapshot `hover`, and the end-to-end flow `lsp.rs` (`a_description_shows_at_point_scrolls_and_goes_with_the_next_key`).

### Step 11: agents

The author answered step 11's questions on 2026-11-16, all with the first choice:
- **Q1:** the tool takes `buffer`, by default the selected one, and `project: true`.
- **Q2:** `M-g d`'s order, with no hints, at most 500 entries and `more` for the rest.
- **Q3:** a file no buffer holds gives the server's column with its `column_unit`; an open buffer's column is in characters.
- **Q4:** a message is at most 2 KiB, cut at a character. Its line breaks are kept, and every other control, bidi or invisible character is written as `\u{…}`. Each entry names its server.
- **Q5:** `state` gains each followed buffer's `server` and `diagnostics`, and `STATE_VERSION` goes 2→3.
- **Q6:** two `Person` commands: `session-send-diagnostics` on `C-c a !` and `session-fix-error` on `C-c a x`.
- **Q7:** the public items below.

- **The channel's `diagnostics`** (`channel/diagnostics.rs`), and the `oso mcp` tool of that name, is a read tool like `text`. It reads a buffer's diagnostics, or with `project`, its project's, from every server:
  - Sources: the open buffers' sets, then the reports the servers keep of files no buffer holds (6b), which the app shares by `Arc`.
  - Where: the UI thread checks the buffer and takes a snapshot, the sets cloned and the texts and reports shared, O(the open buffers' entries); the answer is made on a thread of its own, `oso-agent-diagnostics`, as `text` copies its rows (`app/agent/reads.rs`).
  - Order: the buffer's own first, by place, then each file's by its path from the root and by line. One function, `channel::diagnostics::ordered`, gives this order to `M-g d`'s rows too, with its paths judged as a URI reads back and its bound on a path's length, so the two cannot drift apart. Hints are left out.
  - Bound: at most `MAX_ENTRIES` (500) and `ANSWER_BYTES` (512 KiB) of the entries' text, so that an answer fits a reply line; the rest are counted in `more`. Once full, an open set is counted from its counts and a file's reports without sorting them.
  - Each entry gives the path, the line and column from 1, the severity (`error`, `warning` or `info`, `Severity::word`), the message, the source, the code and the server. An open buffer's column is in characters, counted in O(log n) by a slice of the rope. A file's column is the server's own, with `column_unit`.
  - Text: the server's text goes to the agent as Q4 has it, the message cut at 2 KiB; the source, the code and a file's path, which a server names too, are escaped the same way, with no line break kept (`server_text`, which a send shares).
  - Refusals: a buffer that may hold secrets, or one withheld from agents, gives none, worded as `text` words it. Such buffers, and those an agent's open still waits on, are left out of a project's, with the servers' reports of their files. A buffer in no project gives no project's.
  - A file no buffer holds was judged by the name its server gave (6a). The author chose on 2026-11-16 that the job's thread also judges where the path really leads, as the loader judges a buffer's, through each link (`loader::resolve`, which the app hands over, as the channel sits below the loader), and gives nothing of a file that may hold secrets, or that nobody can resolve. Only the files whose entries go are judged; those only counted once the answer is full give a number.
- **`state`** (`STATE_VERSION` 3):
  - Each buffer a server follows gets `server`: the server's name, which the app gives, its state (`starting`, `ready`, `working` with `progress`, or `stopped`).
  - It also gets `diagnostics`: its counts of errors, warnings and information, hints left out.
  - Neither is given of a buffer the channel may not read, the same check `diagnostics` makes.
  - The version follows ADR 0118, which bumped it for a buffer's new field; `STATE_VERSION`'s doc now says so.
  - The `state` tool's description says so.
- **ADR 0121's deferred forms** (its Q5) are two more kinds of `SendPart`, composed in `sessions/send.rs`:
  - `session-send-diagnostics` (`C-c a !`, the Agents menu's Talk group) sends the buffer's diagnostics, or the region's, one line each, `path:line: severity: message`. The message's first line goes, as `M-g d` reads it (`diagnostic::first_line`), its blanks as one, hints left out. The command keeps only that line of each, at most 2 KiB, matches them against the sorted regions by a binary search, and refuses past 256 KiB as it goes.
  - `session-fix-error` (`C-c a x`) sends the most severe diagnostic at the primary cursor:
    - `path:line:column: severity`;
    - "The language server says:", and the message, its line breaks kept, in a fence of its own longer than any run of backticks in it, so that no message can pass as the lines or the ask after it;
    - the 10 lines either side of it, as `@region` gives lines;
    - "Fix this error."
  - Both are `Person`, pasted and not submitted, at most 256 KiB, with the controls in a message written out as in a path.
- **Public API:** `oso-core`: `editor::SendPart::{Diagnostics, FixError}` and `editor::SendDiagnostic`; and, for the words and the first line the three readers share, `diagnostic::Severity::word` and `diagnostic::first_line`, which was crate-internal. The channel's and the commands' items are the tip crate's own.
- **Tests:**
  - the channel's answer: a buffer's, a project's with files, the order, the columns, the bounds and `more`, the refusals, the buffers and files left out of a project's, and the escaping of messages, sources, codes and paths;
  - the protocol's parameters and the MCP tool's mapping;
  - `state`'s new fields;
  - the two parts as a send composes them, and a message that tries to pass as the lines and the ask;
  - the two commands through the dispatcher;
  - the end-to-end flow `lsp.rs` (`an_agent_reads_the_diagnostics_and_the_counts`).

### Step 12: the `just perf` metrics

The author answered step 12's questions on 2026-11-17, all with the first choice but Q6:
- **Q1:** the edits to the files that define the checks: the harness, `bench.rs` and the
  Justfile's `perf` and `perf-remote`.
- **Q2:** the fake server is the one built beside oso; without one the phase is skipped with a
  note.
- **Q3:** the seven metrics below, all reported only.
- **Q4:** no budget yet: thresholds and budgets come as [ADR 0123](0123-thresholds-for-the-report-only-metrics.md)
  sets them, once there are numbers.
- **Q5:** 2 runs of 200 keys each, 1 of 20 with `--quick`.
- **Q6 (b):** the `rust-analyzer` component went into `rust-toolchain.toml` first, in a pull
  request of its own.
- **Q7:** Unix only.

- **The phase** (`examples/perf/bench/lsp.rs`) writes a project of its own, `proj-lsp`, with a
  `src/main.rs` of 2000 lines in `main`. Beside the harness's own configuration go the record
  of trusted projects, naming the project's directory by its device and inode
  (`App::start_with_files`), and a configuration that makes rust-analyzer the dev-only fake
  server with a script. oso starts the server as it would a person's; the harness spawns
  nothing more.
- **Ready:** the mode line says `lsp` alike while a server starts and once it is ready, but
  counts only what a ready server reported. So a script with nothing else to report gives one
  note on the closing line, out of view, and the phase waits for its count, or for all 2000.
- **The metrics,** each key press to presented frame unless it says otherwise, in the middle of
  the file with `electric-indent-mode` off, so that the text grows by what the keys type:
  - `lsp_type_median_ms` and `lsp_type_p99_ms`: typing with the server attached;
  - `lsp_diag_type_p99_ms`: typing with 2000 diagnostics, one on each line, of each severity
    in turn;
  - `lsp_diag_cursors_type_p99_ms`: the same at 1000 cursors from `edit-lines`, the text
    checked to grow at each;
  - `lsp_report_type_p99_ms`: typing while each change brings a new report of 2000. The
    reports give no version, so each is placed against the text the server was last sent;
  - `lsp_complete_ms`: `C-M-i` on a line of its own to the state showing the popup of 5000
    items, polled every 2 ms, the median of each run;
  - `lsp_narrow_p99_ms`: typing into that popup, each key narrowing it.
- **Builds:** `just perf` builds `fake-lsp-server` beside oso, and `perf-remote` copies the
  new side's beside both sides' oso. A release build still makes none.
