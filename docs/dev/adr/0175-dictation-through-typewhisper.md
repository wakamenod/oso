# 0175. Dictation through TypeWhisper

- Status: Accepted
- Date: 2026-11-23

## Context

[ADR 0129](0129-dictation.md) planned dictation with a recogniser oso would start itself, and
the author deferred it on 2026-11-13. On 2026-11-23 the author chose another way, through the
orchestrator: **TypeWhisper**, the macOS dictation app they already run (their own fork of
`TypeWhisper/typewhisper-mac`), records and recognises on the device, and oso asks it for the
text over its local HTTP API. This ADR supersedes 0129.

What TypeWhisper offers, read from its source and from one read-only probe of a running copy
(`GET /v1/dictation/status`, which answered 200, and 401 without the token):

- An API server on `127.0.0.1`, HTTP/1.1 with `Connection: close`, JSON. Every route needs the
  bearer token. It refuses requests addressed to another host and cross-site browser requests.
- A **discovery file**, `api-discovery.json` in TypeWhisper's Application Support directory
  (`TypeWhisper`, or `TypeWhisper-Dev` for a debug build), mode 0600, with `port`, `token` (43
  characters) and `version` (1).
- `POST /v1/dictation/start` (an optional `workflow_id`), `POST /v1/dictation/stop`, and
  `GET /v1/dictation/transcription?id=…`, whose `status` is `recording`, `processing`,
  `completed` or `failed`, with the `text` once completed. A start while not idle is a 409, and
  so is a stop while not recording.
- A dictation started over the API runs TypeWhisper's usual pipeline, which **types the text
  itself**: through Accessibility, or by putting it on the clipboard and posting Command-V. It
  marks that clipboard item with the type `com.typewhisper.SpeechTranscription`. oso is not an
  app bundle, so it gets Command-V, which oso reads as `M-v`, `scroll-down-command`. The
  clipboard is put back only if TypeWhisper's own setting says so.
- A dictionary of terms and corrections, which steps 2 to 4 below use.

The forces are ADR 0129's: oso has no network code, a person's microphone is on only while
they can see it, the UI thread never waits, dictated text is the person's own typing, and it may
go into a buffer that holds secrets.

## Decision

Dictation goes through TypeWhisper. It is built in four steps, one pull request each, and each
fills in its "as built" below.

1. **Dictate and type, and drop TypeWhisper's Command-V** (this ADR's first pull request).
2. **A fix by Claude**: `s-d` types at once in a provisional face, and `claude -p` fixes the
   text in the background; `C-u s-d` types plain.
3. **A learning record**: what fixes and the person's own rewrites of dictated text teach,
   kept by oso, given to each fix as history.
4. **TypeWhisper's dictionary**: oso adds what it learned there, within a cap, and takes back
   only what it added.

### Calls through curl

- **oso has no network code** ([ADR 0010](0010-process-and-network-policy.md)), so a call is a run of
  curl through `oso-platform::process`: `/usr/bin/curl` by absolute path, or `[dictation] curl`.
- **Its arguments are only `-q --config -`.** `-q` first keeps it from reading a `.curlrc`. The
  configuration goes on its standard input: the address `http://127.0.0.1:PORT/…`, whose host is
  fixed in oso; `noproxy = "*"`, `proto = "=http"`, `max-redirs = 0`, a `max-time` per call
  (10 s for a start, which waits for the microphone, and 3 s otherwise), `max-filesize` of
  1 MiB, `http1.1`, a `write-out` with the status; and the bearer header and the body. The token
  is never in an argument, which other users can read.
- curl gets only `oso-platform::process`'s base variables, no terminal, and a session of its
  own.
- **Calls run on a dictation's thread**, never on the UI thread. While it records, the thread
  looks every second whether TypeWhisper still records, as its own hotkey can stop it. Once
  stopped, it polls every 300 ms for up to 120 s.
- **Nothing is tried twice.** A call that fails ends the dictation, and the echo area says why.
  oso never launches TypeWhisper.

### The discovery file

- Its path is `[dictation] discovery`, from the person's configuration only, absolute or under
  `~/`. Unless set, it is the release build's, `~/Library/Application Support/TypeWhisper/
  api-discovery.json`, on macOS; elsewhere there is none.
- It is read fresh at each start, on the dictation's thread, and **refused** when it is a link
  (also one swapped in after the look), not this user's, writable by its group or others, or
  over 4 KiB.
- `version` must be 1, `port` from 1 to 65535, and `token` 16 to 256 characters from
  `A-Z a-z 0-9 . _ ~ + / = -`, which go into curl's configuration with no quoting. **No fallback
  to port 8978**: with no file, TypeWhisper is taken as not running.
- **The token is a redacted type**: its `Debug` says nothing of it and it has no `Display`. It
  is never in an argument, a log line, a message, restart state or the agent channel.
- With no file, or nothing listening: "TypeWhisper isn't running, or is its API server on?".

### Commands and keys

- **`dictation-toggle` on `s-d`** (Option-D on macOS) starts a dictation where the person is,
  and stops it. `C-u` on either press is accepted and kept, for step 2. A press while the text
  is due says so.
- **`dictation-cancel`**, from `M-x`, drops the dictation and its text; TypeWhisper is stopped if
  it records, and its paste is still dropped.
- Both are `Person` ([ADR 0085](0085-agent-channel-and-oso-mcp.md)): they turn on the microphone and start
  curl. They are refused while a keyboard macro records or replays, and no tool of the agent
  channel or of `oso mcp` reaches them.

### Where the text goes

- **The target is taken at the start**: the selected window, its buffer, the buffer's revision
  and its cursors. A buffer, the minibuffer and a native session's composer are all windows, so
  one path covers them.
- Refused at the start: a passphrase prompt, a question, a terminal, and a read-only buffer.
- **The text goes in at each of those cursors, in one undo step**, as the person's own text.
  Control characters are dropped but tabs and line breaks, and it is cut at 64 KiB.
- **The kill ring instead**, with a message: when the window is gone or shows another buffer,
  the buffer is gone or read-only, or **its text changed meanwhile**, as the cursors then no
  longer say where the person was; and when the text times the cursors is over 16 MiB, a
  yank's bound. The text of a buffer that may hold secrets, at the start or now, makes a secret
  entry.
- **The input method:** while a composition is open in that window, the text waits, at most
  10 s, then goes to the kill ring.
- **Secrets:** dictation into a buffer that may hold secrets is allowed: the text is the
  person's own, and oso keeps it nowhere but in that buffer or a secret kill. Steps 2 to 4 do
  nothing there. TypeWhisper itself still puts it on the clipboard, unmarked for clipboard
  managers, and may keep it in its own history.
- **What TypeWhisper says** in an error is shown with its controls as spaces, cut to 120
  characters, and kept out of the log, as text from outside oso is ([ADR 0138](0138-language-servers.md)).

### The modeline

The selected window's modeline says `● REC` in the alert's red (`Face::Dictation`) while
TypeWhisper records, and a dim `…` while the text is due, after `Agent`. It is a `Status` field
of its own.

### TypeWhisper's Command-V

- **A guard opens as oso asks the dictation to stop**, or learns that it stopped, and closes 3 s
  after the text came, or after the dictation failed once stopped; at the latest, 3 s after the
  text's own 120 s wait.
- While it is open, **an `M-v` alone is held**, and the clipboard's worker is asked only whether
  the clipboard carries `com.typewhisper.SpeechTranscription`: a new **types-only request** in
  `oso-platform::clipboard`, answered within **200 ms**. Nothing of the text is read.
- **With the marker, the key is dropped**, and the log says so. With none, or no answer, `M-v`
  runs as usual. Keys that come meanwhile wait behind it, in order, as behind a yank.
- The check is in the dispatcher's input path, after a key is translated, so keys from the dev
  socket go through it; a keyboard macro's replay does not. For keys from the dev socket over the
  system's clipboard, no client may look at it ([ADR 0074](0074-the-kill-ring-and-the-clipboard.md)): there is no
  answer, and the key runs.
- `MemoryClipboard` holds marker types, so an end-to-end flow covers the drop.

### Exit and restart

An exit starts a stop of a dictation that records and waits at most 300 ms for it to have been
started, not for its answer. Nothing of a dictation is carried across a restart.

## Changes to accepted ADRs

- [ADR 0074](0074-the-kill-ring-and-the-clipboard.md), "Reading only on a yank (chosen)" and its
  consequence that oso reads the clipboard only when asked to yank: while a dictation's text is
  due, an `M-v` also has the clipboard's worker look at its list of types, on its own thread,
  answered within 200 ms. No text or image is read then. Whether that look shows macOS's notice
  about reading another app's clipboard is not known yet; the author checks it by hand.

- [ADR 0165](0165-native-claude-sessions-the-route.md), part 2a ("Sending") and part 2c
  (`C-c C-k`): a composer's `C-c C-c` and `C-u C-c C-c`, an empty one too, wait while a
  dictation's fix runs, as "Step 2, as built" says, and `C-c C-k` drops that wait before it
  gives up an edit or interrupts. A send that waits holds an exit or a restart that waits for
  the sessions (part 4b).

### Layers and tests

- The `oso` crate's `dictation` module, `dictation::{api, discovery, state, fix, learning}`, on the
  lowest line of `guardrails/layers.txt`: it uses nothing of the crate. `app::dictation` carries
  it out; `dispatch::paste_guard` holds the key; `config::dictation` reads `[dictation]`. Where
  the text goes, and its provisional spans, are oso-core's (`editor::dictation`, step 2).
- **`fake-typewhisper`**, a dev-only binary beside `fake-lsp-server`: a loopback HTTP server,
  the one file with an `#[expect]` for `TcpListener`, which writes a discovery file, answers
  from a script and records requests. `just release-safety` checks that release builds leave it
  out.

## Consequences

- **The clipboard is overwritten**: TypeWhisper puts the text there, and if its own setting does
  not put the clipboard back, the next `C-y` yanks the transcript. A kill-ring fallback writes the
  clipboard too, as a kill does, and a Command-V that comes after that is not recognised.
- **An Accessibility insert would not be caught.** TypeWhisper uses one where the focused app
  exposes a text field, which oso's window does not today. The author checks the real paste and
  the input method by hand.
- **Dictations started with TypeWhisper's own hotkey are not oso's**: no guard, no target; its
  Command-V scrolls. One oso started and TypeWhisper's hotkey stopped is seen within a second;
  a paste before that scrolls.
- **The person's own `M-v`** while the text is due is dropped too while the clipboard still
  carries TypeWhisper's marker.
- **An exit while TypeWhisper starts**, which may take 10 s, ends before the stop can go, so
  TypeWhisper may go on recording after oso has gone.
- **Residual:** a stale discovery file, with another local process listening on that port, gives
  that process the token and the text it answers.
- `M-v` waits up to 200 ms while a dictation's text is due.

## Alternatives considered

- **A client over `TcpStream`:** fewer processes, but it breaks a hard rule: oso has no network
  code.
- **Patching the fork** so that it does not paste for API dictations: the author chose to keep
  the fork as it is and drop the key in oso.
- **Reading the clipboard's text** to tell TypeWhisper's paste from the person's: the marker type
  says it without reading any text.
- **ADR 0129's own recogniser:** TypeWhisper already records, recognises on the device and keeps
  a dictionary the person maintains.

## Step 1, as built (2026-11-23)

- Built as decided above, in the `dictation` module with `target` beside `api`, `discovery` and
  `state`, so that the place and the kill-ring fallback are tested without the app. The author
  chose on 2026-11-23 to move `target` into oso-core with step 2's spans, and to keep the guard
  from oso's stop only.
- `[dictation] max_seconds` is 5 to 1800, 120 unless set; at the limit oso stops the dictation and
  says so. `[dictation] workflow` takes a UUID.
- The public items, approved by the author: `oso_platform::clipboard::{Clipboard::has_type,
  Event::HasType, TYPE_TIMEOUT, MemoryClipboard::set_marked}`, `oso_core::editor::{
  DictationRequest, DictationMark, Status::dictation}` with `Editor`'s `request_dictation`,
  `take_dictation_requests`, `set_dictation_mark` and `dictation_mark`, and
  `oso_render::Face::Dictation`.
- The dev socket's `clipboard` takes `types`, and its state says `paste_held`.
- The reviews moved the guard's opening to oso's stop, bounded it, capped the text across the
  cursors, kept the thread until a composition's wait ends, and kept TypeWhisper's words out of
  the log. `fake-typewhisper` ends after 60 s with no request.

## Step 2, as built (2026-11-23)

- **`s-d` always fixes.** The text goes in as before, as a **provisional span**: a range at
  each cursor, kept by the editor, drawn on a faint fill of its own
  (`Face::DictationProvisional`), while the modeline says `fix`. `C-u` on either press types
  it plain, with no fix. The face goes when the fix lands, fails, is skipped or cancelled, or
  oso exits.
- **Where the text goes moved into oso-core**, as the author chose: step 1's `target` is
  `oso_core::editor`'s `DictationTarget` and `DictationPlaced`, with `Editor`'s
  `dictation_target`, `dictation_waits`, `place_dictation` and `keep_dictation`. The spans are
  `DictationSpan` and `DictationContext`, with `dictation_context`, `fix_dictation`,
  `end_dictation_span` and `dictation_span_cells`, and `DictationMark::Fixing`; approved by the
  author on 2026-11-23.
- **Spans** follow every edit, the person's and an undo's, as markers whose start goes after
  text typed at it and whose end stays before, so that typing at either end is not the
  dictation's. They go with a reload or the buffer, and are not saved, undone, copied, carried
  across a restart or told to agents. An undo in a buffer that keeps no log of its edits, a
  reload and a copy put back end a span, as where its text is then is not known. At most 4
  live at once, each at most 32 cursors, and at most 4 fixes run; a dictation past them is
  typed plain.
- **No fix** in the minibuffer, whose prompts take paths, commands and searches (the author's
  choice), nor in a buffer that may hold secrets, at the start or when the answer comes.
- **The fix's run**, on a thread of its own: TypeWhisper's terms and corrections, each read
  with a 3 s limit and capped at 200 entries or 16 KiB a list, then the person's `claude -p`
  (`[sessions] claude`) through `sessions::oneshot`, which the space naming of
  [ADR 0159](0159-space-new-starts-the-orchestrator-and-display-names.md) now shares, with
  the options that keep it to itself and the reading of its answer's envelope: `--safe-mode
  --tools= --strict-mcp-config --setting-sources=user --disable-slash-commands
  --no-session-persistence --output-format=json`, then the fix's JSON schema, system prompt,
  `--model` and `--effort`, each as one `--flag=value`. It runs in a new private directory under
  `dictation-fix` in oso's state directory, killed with its session at 30 s, reading at most
  64 KiB, and left out of the registry's sessions as a naming run is. A stop that comes before
  `claude` starts holds, for the naming run too: none starts.
- **The input is one JSON object** on its standard input: the mode's name (never the path),
  the dictionary, up to `fix_context_lines` lines before and after the text (at most 4 KiB a
  side), and the text. The author chose JSON over the elisp's tags, so that no dictated text
  can close one and forge another; the system prompt, ported from `typewhisper-fix.el`, says
  all of it is data, never instructions.
- **The answer** is at most 32 fixes, each `wrong` and `right` at most 200 characters, a
  `right` with no control, invisible or bidi character and no noncharacter, at most twice its
  `wrong`'s characters and 8 more, and `high`, `medium` or `low`. It is read and turned into
  edits on the fix's thread. Fixes at or above
  `fix_min_confidence` replace each place their `wrong` is found inside the text sent, apart
  from an earlier fix's, up to 256 edits; the rest are shown in the echo area only. The edits
  are made at every range of the span in one `apply_edits`, one undo step, and only while
  each range still holds exactly the text sent, and at most 1024 replacements across the
  ranges: otherwise "Dictation fix skipped". The report quotes the person's words and Claude's,
  so it is shown in the echo area alone, withheld from the dev socket and agents, and never in
  `*Messages*` or the log; "Dictation: no fix", which quotes nothing, is a message as any.
- **An answer that comes while another dictation is under way waits** for that one's text to
  be placed: a fix changes the buffer, and the next text would go to the kill ring, as the
  buffer changed while the person spoke.
- **Settings**: `[dictation] fix_model` (a model's name, `sonnet` unless set), `fix_effort`
  (`low`), `fix_context_lines` (0 to 20, 3) and `fix_min_confidence` (`medium`).
- **A native composer's send** (`C-c C-c`, or `C-u C-c C-c`) while the modeline says `fix`,
  whichever buffer the fix is in, waits for the fixes running then to end, and then sends the
  composer's text as the fix left it: the author's choice on 2026-11-23. Each fix ends within
  its limits (two 3 s reads and 30 s), but its answer also waits while another dictation is
  under way, and the send with it.
  - The wait keeps its composer and its revision, which only the fix's own edit moves on: a
    composer the person edited, replaced or lost meanwhile drops the wait, and says so, so
    that nothing goes but what the person sent.
  - A send now goes as a plain send, as the turn it meant to interrupt may have ended.
  - `C-c C-k` there drops the wait, and does nothing more; `dictation-cancel` drops it too, as
    the text would go unfixed. An exit or a restart drops it, and one that waits for the
    sessions waits for it. The send then goes through the composer's checks and the queue's
    rules, as any.
  - While a dictation records or waits for its text, a send goes at once, as before.
- **The author's choices on 2026-11-23:** the fix runs in projects that are not trusted too, as
  it reads nothing of the project and runs no tool, though the text it sends is the
  project's; and there is no setting that turns the fix off: `C-u s-d` types plain, each time.
- `M-x dictation-cancel` with no dictation under way stops the fixes running, and the text
  stays as typed; so do an exit and a restart's start. The UI thread only kills `claude`: the
  fix's thread reaps it. The dev socket's state says `dictation.fixes`.

## Step 3, as built (2026-11-23)

Learning runs in the background: nothing of it is shown, but it is logged. It writes nothing
outside oso; adding to TypeWhisper's dictionary is step 4.

- **The record**, `dictation/learning.json` in oso's state directory, mode `0600`, replaced
  whole through a `.part` in a private directory, read as `sessions` is: a regular file of
  this user's with one name, at most 256 KiB. A file oso may not read is left as it is.
  - It keeps pairs: a misheard `wrong` and its `right`, each counted as a fix that stood, as
    the person's own rewrite that stood of text a fix saw, as one of text typed plain, and as
    undone, with the days it stood on (the latest
    8) and the day it was last seen. The author's choice on 2026-11-23, in place of a list of
    observations.
  - **At most 40 pairs.** A pair not seen for 30 days goes; when the record is full, the one
    seen longest ago, then the least used, makes room. A day past today, from a clock set back
    or a record not oso's, is read as today. A pair whose sides hold a control, invisible or
    bidi character, or are longer than a fix's 200 characters, is never kept, nor one that
    looks like a credential: a run of ASCII letters, digits and `_-+/=.` that starts as a
    known token does (`ghp_`, `github_pat_`, `glpat-`, `sk-`, `xoxb-` and the like, `-----BEGIN`)
    or is 16 or more long with letters and digits, as a token pasted over a word would be.
- **A watch**: once a dictation's text settles (its fix lands, fails, finds nothing or is
  skipped, or `C-u s-d` typed it plain), its span stays, undrawn, as a **settled** span, and
  the app watches it. None is made in the minibuffer or a buffer that may hold secrets.
  - The editor counts the **person's commands** that change the span's text. Any other change
    ends the span, and the watch with it, learning nothing: an agent's command, oso's own
    edits outside a command (a server's, a composer's, a slice of `query-replace`), and a
    command the host runs that is no input of the person's. A fix's own edit is not counted.
  - A span's start now stays before a replacement of what begins at it, so that an undo of a
    fix of the first word stays inside; text typed at either end is still outside. An undo
    that crosses in its own command, in a buffer that keeps no log of its edits, now follows
    the undo's own edits, up to 4096, rather than drop the spans; one that takes more slices
    goes on outside the command, and ends the spans it changes.
  - A second dictation typed inside a settled span is not a command of the person's, so it
    ends that span, learning nothing from it.
  - It ends **30 s after the last of the person's changes**, never before 60 s, and at **5
    minutes** at the latest; with the span, when its buffer goes, is read again or may hold
    secrets; past 8 watches, the oldest first. The editor keeps at most 16 settled spans, a
    bound on memory the watches stay under. A save does not end a watch, as it changes no
    text (the author's choice on 2026-11-23). An exit, a restart and `dictation-forget-learned`
    drop the watches.
- **What a watch teaches**, worked out on the learning's thread, `oso-dictation-learn`:
  - the text still as oso left it: each fix stood; back as it was typed: each was undone;
  - otherwise a fix stood if its `right` is still there, and was undone if the person put its
    `wrong` back by hand;
  - and the person's own rewrites: a character diff of the text oso left and the text now
    (Myers's, written in oso rather than `similar`'s for a bound fixed by the edit distance
    and not by time: at most 4 KiB a side and 64 characters changed), a change
    touching an ASCII word taking in the whole word. Kept: replacements only, each `wrong` at
    most 20 characters and `right` at most 40, on one line, at most 5 changes, and at most
    half the text changed unless it is one change. Pure insertions and deletions are ignored.
- **The history in each fix**: the fix's thread reads the record and adds two lists to the
  JSON on `claude`'s standard input, of pairs whose `wrong` is in the dictated text only:
  `history` has those that stood more often than they were undone, the most used first, then
  the latest, at most 50 pairs; `rejected` has those undone at least as often as they stood.
  Both together are at most 4 KiB. A pair learned only from text typed plain (`C-u s-d`, or
  past the fixes that run), which the person kept from Claude, stays on the machine and is
  never sent, the author's choice on 2026-11-23; once it is also learned from text a fix saw,
  it is sent with that count alone. So are a fix that failed and its text, as `claude` may
  not have read it. The system prompt says to
  fix a `wrong` from `history` the same way and never to make a fix from `rejected`. This is
  more of what leaves the machine: words from earlier dictations, never from a buffer that may
  hold secrets.
- **The log**, `dictation/learning.log`, JSON lines, mode `0600`, replaced whole with no flush
  to the disk, its last 500 lines of the last 30 days: each pair a watch saw, with its source
  and outcome, and each history sent, as counts. A command to show it is step 4's (the
  author's choice).
- The learning's thread runs each job on its own, so that one that panics loses only itself.
- **`dictation-forget-learned`** (`Person`: what was learned of the person's words is theirs
  to drop) asks once, then stops the fixes under way, as `dictation-cancel` does, drops the
  watches, and removes the record, the log and what a write left of them, each tried. A job
  made before it, a fix's history among them, is dropped. Step 4 adds TypeWhisper's entries
  oso added. Another instance of oso is not told, and may write the record again; a write
  under way at an exit may be lost. Two instances write the record in turn, the last one's
  standing.
- **The public items**, approved by the author on 2026-11-23: `Editor`'s
  `settle_dictation_span`, `dictation_span_edits` and `dictation_span_text`, and
  `DictationRequest::Forget`. `place_dictation` now makes a settled span for plain text too,
  and `fix_dictation` settles the span instead of ending it, over the fixed text, or over
  what it holds when the fix is skipped. At most 4 spans are drawn and 16 settled; a
  dictation past the 4 drawn gets no span, and is not watched. Where a fix lands but its text
  is past the 4 KiB watched, the span ends, so that Claude's fixes are never learned as the
  person's.
- The module `dictation::learning`, with `rewrites`, is strings alone; `app::dictation::learning`
  watches and runs the thread; oso-core's `editor::dictation::settled` keeps the settled
  spans and how every span follows an edit.
- The dev socket's state says `dictation.watches`. A dev build's end-to-end flow can make the
  watches quick (`OSO_DEV_DICTATION_QUICK`: 1 s, 2 s and 10 s).
