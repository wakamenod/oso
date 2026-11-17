# oso

The tip crate and binary: commands, default key bindings, the event loop and
the glue between the library crates (ADR 0004). It changes most
often, so it must rebuild within 2 s.

- May depend on every library crate; nothing depends on it. It is the only
  crate that may use winit, and only `src/app/shell.rs` names it: the rest works
  on the events, keys and window calls that file gives (ADR 0070). On macOS,
  `src/app/native.rs` gives the same names over `oso_platform::window`'s own layer
  instead, and neither `shell.rs` nor winit is built.
- Commands are plain functions registered with a docstring and argument
  spec (`add-command` skill). They keep no state of their own. Each feature has
  a module in `src/commands/` and, if it has default keys, one of the same name
  in `src/bindings/`.
- `src/app.rs` holds the app's state and what its concerns share, and `src/app/`
  the concerns: `shell` runs winit, or `native` oso's own layer (ADR 0070), `events` routes, `input`, `ime`, `draw`, `configuration`,
  `loads`, `reloads`, `saves`, `idle_quit`, `copies`, `clipboard`, `searches`, `migemo`, `slices`, `walks`, `greps`, `wraps`, `flash`, `jumps`, `reviews`, `dired`, `vcs`, `terminals`, `recent_files` (the recent files' record, ADR 0148)
  and `starts` (what opening and saving start, and what waits for the person, ADR 0118) do the
  work, `control`
  answers the dev socket, and `handover` exports and imports the state of a restart, `oso dev`'s
  or `oso-restart`'s; its terminals go in `terminals/carry.rs` (the instance going) and
  `terminals/adopt.rs` (the one coming) (ADR 0120, step 11). `restart` starts `oso-restart`
  and answers its threads, and says when a new binary is in place (ADR 0133).
- `src/carry.rs` and `src/carry/` are the state a restart carries and both its transports'
  shared parts: the state's format, its texts in pieces, the terminals' entries, the
  descriptors (`fds`) and the requests (`op`). `oso dev`'s supervisor asks them over the dev
  socket; `oso-restart`'s thread (`going`) plays the supervisor over a socket pair with the
  instance it starts (`pair`), whose own thread (`coming`) answers, after the binary's check
  (`binary`). `notice`'s thread looks at the binary again on focus. A dev build's
  `OSO_DEV_RESTART_FAIL` stops such a restart short, for end-to-end flows.
- `src/emergency.rs` and `src/emergency/` hold the panic hook and the emergency copies
  (ADR 0062), and recovering them (ADR 0064). Their writer sits above `saver`, and nothing
  there may hold text in a message or a log. A path in a `.meta` is never used to touch a file.
- `src/config/` reads the user configuration file and turns its text into the user keymap and
  settings (ADR 0108, ADR 0110), and reads and writes the record of trusted projects beside it
  (`config/trust.rs`, ADR 0114). `config/checked.rs` reads each such file, and one the
  configuration names (migemo's dictionary, ADR 0116), with ADR 0108's checks. Its parsing does
  no I/O. Only it and `sessions::rules`, which
  parses the agent rules files (ADR 0121), use `toml`, which review keeps: no guard test checks
  it.
- `src/terminal.rs` and `src/terminal/` host terminals (ADR 0120): which keys go to a program
  (`route`), what a terminal starts (`shell`), the screen the UI keeps (`grid`, `colors`), the
  mouse's selection on it (`selection`), the start thread (`hosted`) and the local directory a
  program says it is in (`directory`, ADR 0141). A hosted session's
  terminal finds its shell with `hosted::find_shell` and is put in its buffer by
  `app/terminals/start.rs`, as a terminal's. `oso-term` owns the PTY, the engine and the
  policy; the dispatcher holds each terminal's key-reading state (`dispatch/terminal.rs`), and
  `app/terminals.rs` carries out what commands and keys ask, with `app/terminals/`: `input`
  (keys, text, pastes, and the wheel over the scrollback), `mouse` (clicks, drags and their
  reports), `wheel` (the wheel's reports and mode 1007's arrows) and `snapshots` (what each
  terminal's thread sent). Terminal text never reaches a
  log or the disk, and reaches the agent channel only as the rows of a terminal the person
  let agents read (`terminal-allow-agent-read`), never while its program reads a password.
- `src/app/servers.rs` runs language servers (ADR 0138): one per project root and entry, each
  on `oso-lsp`'s threads, started from an open (`app/starts.rs`), the `lsp-*` commands
  (`commands/lsp.rs`), trust and the configuration (`config/lsp.rs`, the built-in table and
  `[lsp]`). Its `Gate` checks `git::TrustedRoot` and finds the program (`servers/find.rs`) on
  the server's thread. A server's diagnostics go into its buffers, and the message at point
  waits 300 ms there (`servers/diagnostics.rs`); its reports of files no buffer holds are kept
  there for `M-g d` and the agent channel's `diagnostics`, in one order
  (`channel::diagnostics::ordered`); `M-g d`'s rows are built on `oso-diagnostics` (`servers/diagnostics/rows.rs`). An answer's several places, and `M-?`'s references, are listed (`servers/xrefs.rs`), their rows built on `oso-xrefs` (`servers/xrefs/rows.rs`), each file's lines read by `grep/preview/lines.rs`. `C-M-i`'s answer opens the completion popup (`servers/complete.rs`, which also asks as the person types, after a trigger character or three identifier characters at rest, and resolves the selected item; the asks themselves and who made them are `servers/asks.rs`'), whose keys are `commands/popup.rs` and `bindings/popup.rs` in the transient layer `dispatch/transient.rs` closes, drawn by `app/draw/float/popup.rs`. A server's description at point (`C-c C-d`) goes to its float from `servers/hover.rs`, drawn by `draw/float/hover.rs`. `C-c C-r`'s code actions are listed and
  their edits applied from `servers/actions.rs`, files no buffer holds opened after the person's
  yes and a command's `workspace/applyEdit` applied from `servers/applying.rs` (step 13), its keys in the same transient place. The fake server the tests run against is the dev-only binary
  `fake-lsp-server` (`tests/fake_lsp_server/`), which `just release-safety` keeps out of
  release builds.
- `src/sessions.rs` and `src/sessions/` are agent sessions (ADR 0121): the rules that read a
  hosted session's state from its screen, the tracker, the `oso-sessions` worker, and worktree
  sessions (`sessions/worktree/`), whose git and `gh` jobs run on `oso-sessions-git`, beside the
  worker; a removal's `git worktree remove` runs on `oso-sessions-remove`, with no time limit. Their patterns use `regex`, compiled with size limits; the screen is untrusted text.
  Hosted sessions get ready and start their terminal on `oso-session-start` (`start`), with a
  command line built by pure functions (`command`, `launch`); Claude Code's registry
  (`registry`) and oso's record (`record`) are read off the UI thread, and a bridge's claim is
  judged by one pure function (`bridge`). `session-adopt` reads Claude Code's transcripts
  (`transcripts`) only for ids, names, times and directories, on its own
  `oso-session-adopt-list` thread (`adopt`). `session-search` alone reads what was said in
  them, prompts and replies only, held in memory on `oso-history` (`history`, ADR 0145); no
  agent gets it, and none of it is logged. The clipboard's images a session is given
  (`images`, ADR 0142) are written, converted by `sips` and removed on one `oso-images` thread,
  never the worker's. `app/sessions.rs` and `app/sessions/` carry out what the worker, the jobs
  and the start thread find.
- `src/review.rs` runs git and diffs for an agent review on `oso-review` (ADR 0122), only for
  a trusted project's root, and git only through `oso_platform::process`, with `src/git.rs`'s
  runner, `TrustedRoot` and secret-name check. `review/refine.rs` refines a change block's words,
  for a git status now and a review's view later. `review/view.rs`
  lays out a review buffer's whole text on `oso-review-view`, one thread per review, and
  `review/view/panes.rs` its two panes, which `app/reviews/layout.rs` switches areas to, and
  `review/count.rs` counts the review menu's scopes on `oso-review-count`. `review/message.rs`
  composes the message of the person's comments that `app/reviews/send.rs` pastes into the
  review's session through `send_to_session` (step 12). A review buffer's
  keys are listed in `commands::review::KEYS`, which `?` reads, and bound by
  `bindings/review.rs`.
- `src/git.rs` and `src/git/` are what every run of git shares (ADR 0127, as built): the
  runner (`run`), which starts git only through `oso_platform::process` with oso's own
  arguments and environment, every read bounded (`run/collect`) and a run past 10 s killed; object ids (`id`);
  `TrustedRoot` (`trust`), the only place git starts from; git's names and the secret-name
  check (`path`); the empty hooks directory under oso's state directory (`hooks`); and runs
  with no time limit in a process group of their own, which the person stops (`long`); `gh`, about
  the GitHub repository a remote names (`gh`, ADR 0121 and ADR 0127 step 5a); text from a run
  made safe to show (`said`); a push,
  a pull or a fetch gets the proxies and a session of its own with no controlling terminal
  (`Git::for_remote`, ADR 0127 step 2). The review, the worktree jobs and the git status use it;
  it uses none of them.
- `src/vcs.rs` and `src/vcs/` are source control's git status (ADR 0127): one thread per status
  buffer, `oso-vcs` (`work`), which runs git through `git`'s runner for a trusted root only,
  its reads in `read` and its writes to the index in `write`; staging lines cuts the patch
  (`cut`), and discarding is planned in `discard`, run in `keep`, which writes copies back too,
  and recorded in `discarded`, with `app/vcs/discard.rs` and `commands::vcs::discard` (the list's
  keys, `bindings::vcs::DISCARDED`);
  reads porcelain v2 and patches as untrusted bytes (`status`, `patch`), lays out the text with
  its outline and the table of what each line is about (`layout`), and turns the lines commands
  name into staging (`stage`). A commit runs on its repository's thread, `oso-vcs-run`
  (`run`), with the repository's hooks; what needs no git, its arguments, message, highlights,
  ring and log lines, is `commit`'s. Pushes, pulls and fetches run there too (`run/remotes`,
  streamed by `run/stream`), what they need that runs no git is `remote`'s, and the commit
  sections are read by `history` (step 2), with `app/vcs/remote.rs` and
  `commands::vcs::remote`. What is stopped, a merge, rebase, pick, revert or `git am` (a
  sequence, in the code), is judged by `sequence`, continued, skipped and aborted on the run
  thread (`run/sequence`), with `app/vcs/sequence.rs` and `commands::vcs::sequence` (step 3a).
  Stashes and branches are `stash`'s and `branch`'s, run on the run thread (`run/stash`,
  `run/branch`), with `app/vcs/stash.rs`, `app/vcs/branch.rs`, `commands::vcs::stash` and
  `commands::vcs::branch` (step 3b). The graph is `graph`'s: its records, `graph/lanes`, the
  text `graph/write` and its thread `graph/work`, `oso-vcs-graph`, with `app/vcs/graph.rs` and
  `commands::vcs::graph` (step 4); what its keys do with a commit is `app/vcs/graph/acts.rs`,
  run on the run thread (`run/graph`) (step 4b). Pull requests are `pr`'s: `gh`'s answers read,
  their texts `pr/write` and the repository's `gh` thread `pr/work`, `oso-vcs-gh`, through
  `git::Gh`, with `app/vcs/pr.rs` and `commands::vcs::pr` (step 5a); making and merging is
  `pr/create`, read for on the `gh` thread (`pr/work/create`) and run on the run thread
  (`run/pr`), with `app/vcs/pr/create.rs` and `commands::vcs::pr::create` (step 5a-2); issues are
  `pr/issue`, with `pr/write/issue`, `pr/work/issue`, `app/vcs/pr/issue.rs` and
  `commands::vcs::pr::issue` (step 5b-1). Its keys are listed in `commands::vcs::KEYS` and
  `commands::vcs::commit`, the commit menu is built in, and `bindings/vcs.rs` binds them;
  `app/vcs.rs` carries out what its commands ask, with `app/vcs/status.rs` (the status's own
  requests), `app/vcs/commit.rs` (messages) and `app/vcs/runs.rs` (runs and their log). What a commit asks of the worktree's agent session,
  a draft and a fix, is written by `vcs/agent.rs` and pasted, and the session tool `draft_set`
  answered, by `app/vcs/agent.rs` (step 6a).
- `src/dired.rs` and `src/dired/` are the directory editor's listings (ADR 0134): read on the
  lister's threads (`lister.rs`) from the descriptor, following no link but to tell where one
  leads, ordered and built there into a rope, highlights and a table of what each row names. A
  listing's keys are listed in `commands::dired::KEYS` and bound by `bindings/dired.rs`. Its
  commands that open a file, show a buffer or write the clipboard are `Person`, as the host
  carries them out after the command, where the agent channel's guards no longer see it.
- `src/walk.rs` walks a project's files on `oso-walk` (ADR 0114). Only it and
  `sessions/worktree/include.rs`, which matches `.worktreeinclude` (ADR 0121), use `ignore`,
  which review keeps: no guard test checks it. `src/grep.rs` searches them on `oso-grep` over the same
  walk, and `src/grep/preview.rs` reads a row's file for the preview on `oso-preview`, for the
  search, for `M-g d` and for the list of a server's places (ADR 0138).
- Keep logic that can be tested without a window in `oso-core`. This crate
  translates events and wires things together.
- Dev-only code (the control socket, `oso ctl`, `oso dev`, timings) stays
  behind the `dev` feature; `just release-safety` checks it, the dev socket's restart methods
  among it, though `carry` ships for `oso-restart`.
- `src/channel/` is the agent channel and `oso mcp` (ADR 0085), in every
  build. It has no dev method, and runs only commands whose reach is `Look`
  or `Change`. `src/app/agent.rs` answers it on the UI thread, and
  `src/dispatch/agent.rs` runs its commands. `channel/diagnostics.rs` reads `oso-lsp`'s
  reports and URI rules, the one module of the channel that does (ADR 0138, step 11).
- End-to-end flows live in `tests/e2e` and run with `just e2e` (ADR 0049). They
  drive a dev build from outside, and start it through `oso_platform::process`.
- Module layers are in `guardrails/layers.txt` (`[oso]`).
- Every Rust file opens with its responsibility and a `Must not depend on:`
  paragraph (ADR 0041). The test checks names in backticks against the layers.
- The repository guard tests live in `tests/` (`workspace_lints.rs`,
  `architecture/`). Tests that need the `Dispatcher` stay inside the crate,
  including the text-grid snapshots in `src/dispatch/grid_tests.rs`
  (ADR 0045).
