# 0142. Attaching the clipboard's image to a session

- Status: Accepted
- Date: 2026-11-16

## Context

A hosted session ([ADR 0121](0121-agent-sessions-and-spaces.md)) is a `claude` in one of oso's
terminals. To show Claude Code a screenshot, the person today saves it somewhere and types its
path. The author's own Emacs client for Claude Code had a key for this: it saved the image on the
clipboard to a file and inserted an `@path` reference into the prompt, without sending it. They
want the same in oso.

- **The image goes by path.** Claude Code reads the file an `@path` names when the prompt is
  sent. Base64 typed into the prompt would also sit in the conversation's recording.
- **The types.** Models' APIs take PNG, JPEG, GIF and WebP as they are. A macOS screenshot puts
  `public.png` on the pasteboard beside a `public.tiff` many times larger (9.4 MB of TIFF for
  255 KB of PNG, measured by the author). Some apps copy only TIFF, and that has to be
  converted.
- **oso's clipboard reads text only** ([ADR 0074](0074-the-kill-ring-and-the-clipboard.md)): a
  worker thread owns `NSPasteboard`, and the UI thread never calls into it. `objc2-app-kit`'s
  `NSPasteboard` feature already turns on `NSData`, so `dataForType:` needs no new feature or
  crate. Linux and Windows have no clipboard backend until their window layers.
- **oso's state directory on macOS is `~/Library/Application Support`.** Its space ends an
  `@` reference.
- **Session mode's keys** ([ADR 0132](0132-a-session-mode-for-hosted-sessions.md), Q1): "`C-c`
  and a control key stays the terminal's", except `C-c C-u`. Emacs gives a major mode the `C-c`
  and control key sequences, and `C-y` is yank, so `C-c C-y` reads as "yank into the session".
- **Delivery.** `session-send` pastes into a session through one path, `App::send_to_session`,
  as the person's bracketed paste, not submitted, and refuses a session that needs input, waits
  for a password, shows a full-screen view, is not judged yet or does not run (ADR 0121, Q17).

## Decision

1. **`session-yank-image`, `C-c C-y` in session mode.** In the terminal of a session oso hosts,
   it keeps the clipboard's image in a file and pastes `@PATH` into the session's prompt, with a
   space before and after it, as a bracketed paste, not submitted. The echo area says
   `Image saved as NAME: press Enter there`, and `…, as PNG: …` for a converted TIFF. It names
   the file only: the agent channel reads the echo area, and the directory holds the session's
   id. Elsewhere it says `This buffer holds no session`. It is `Reach::Person`: it reads the
   clipboard and may start a process.
   - The space before it is always there. Claude Code draws its own cursor, so oso cannot tell
     what is left of it. A mention must start a word, and a space too many costs nothing.
   - A plain terminal keeps ADR 0120's rule: `C-c C-y` goes to its program, both keys.
   - The command asks the host with `SessionRequest::YankImage(buffer)` (`oso-core`, new). The
     dispatcher notes, as the command runs, whether its input came through the dev socket,
     held input's origin included, and the host takes that with the request
     (`Dispatcher::take_image_yank_origin`). A request whose origin is lost counts as the
     socket's. The host never reads where the last input came from later, when a held key may
     have run meanwhile.
2. **It goes through `send_to_session`, and is checked first.** The same refusals as a send
   apply, with the same words, before the clipboard is read, so that no file is kept for a
   paste that would be refused, and again when the file is ready, as the session may have
   changed meanwhile. A file whose paste is refused then is removed. One image read at a time:
   a second `C-c C-y` meanwhile says `Still reading the clipboard's image`.
3. **The clipboard reads an image** (`oso_platform::clipboard`): `Clipboard::read_image`
   answers with `Event::Image(ImageContents)`, on the worker that owns the pasteboard.
   - On macOS, it reads the first of `public.png`, `public.jpeg`, `com.compuserve.gif`,
     `org.webmproject.webp` and `public.tiff` the pasteboard has, as bytes, never decoded. If
     another app writes the pasteboard during the read (its change count moves), the read
     answers `NoAnswer` (`The clipboard is busy or not answering`), as the image may not be the
     one the concealed check saw.
   - A pasteboard marked `org.nspasteboard.ConcealedType` is not read
     (`The app that copied the image marked it secret; nothing is attached`).
   - At most `MAX_IMAGE`, 32 MiB (`… is over 32 MiB; nothing is attached`).
   - An image read has no early answer, as nothing waits on it. It gets its answer when the
     worker has one, or `NoAnswer` at ADR 0074's give-up time, 5 s, when its worker is left
     behind as a stuck text read's is. A read while another is out is answered `NoAnswer` at
     once.
   - Linux and Windows answer `Unsupported`:
     `Images on the clipboard are not supported on this system yet`.
   - The in-memory clipboard holds an image too, and a copy of text or an image replaces the
     other, as the system's does. The dev socket's `clipboard` method sets one with `image` and
     `bytes`. As for a yank, input from the dev socket reads only a clipboard in memory.
   - New public items: `Clipboard::read_image`, `Event::Image`, `ImageContents`, `Image`,
     `ImageKind` (with `ALL`, `name` and `extension`), `MAX_IMAGE` and
     `MemoryClipboard::set_image`, in the existing module.
4. **Where the files go: the per-user cache** (`oso_platform::privacy::cache_home`, new):
   `~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` elsewhere on Unix, and
   `%LOCALAPPDATA%` on Windows, where the cache's `oso` is the state directory's too. Images go
   in `oso/images/<session id>/`, a `PrivateDir` (`0700`, checked as ADR 0028's are). Each is a
   new file, `0600` from the start, made with `create_new`, so never through a link, named by
   the local time as `YYYYMMDD-HHMMSS-mmm` (ADR 0134's `file_meta::utc_offset_at`, read on the
   keeper's thread; UTC where it is not read), with `-2` to `-9` after it if that name is taken.
   A path with a space, a control or invisible character, or not UTF-8 is refused before
   anything is written, as an `@` reference cannot hold it. The files are not synced: they are a
   cache's, read by Claude Code seconds later.
5. **A TIFF is converted by `/usr/bin/sips`** in a child process, started through
   `oso_platform::process` at that absolute path, with an argument vector, the base environment
   and a 10 s limit after which it is killed. The PNG's name is taken first, as an empty
   `create_new` file, so that `sips` writes over nothing else, and the TIFF is written next to
   it and removed whatever happens. `sips` exits 0 having written nothing when it cannot read
   its input, so the PNG file itself tells. The PNG is set to `0600`. Off macOS, a TIFF is
   refused. The other types are copied as they came.
   - This lives in the tip crate's `sessions::images`, not in `oso-platform`: there `clipboard`
     and `process` share a layer line and may not use each other, and `process` holds no idea
     of what it runs. A Linux or Windows backend that needs a conversion adds its own here.
6. **How long they stay.** A session's directory goes when the session is archived (`C-c A`, `A`
   in `*Sessions*`, `C-x k`) or dropped from the record. Files older than 7 days go once the
   sessions are recalled at the start, and after each image is kept, so a long-running oso keeps
   none much past its week. A sweep removes only files with the extensions this ADR writes, in
   session directories named as ids that pass `PrivateDir::existing`, at most 4096 entries in
   all, and leaves a root that is not private alone. A directory or file is removed only once it
   and the way to it pass `PrivateDir::existing`, so a link in its place, or a way another user
   could change, is refused. An archived session brought back has no images: Claude Code read
   each when its prompt was sent.
7. **Threads.** The clipboard's worker reads the image. One long-lived thread, `oso-images`
   (`sessions::images::Keeper`), does every write and removal in turn, so that a directory is
   never removed while an image is written into it, and it is never the sessions worker, which
   judges the sessions' screens. It runs `sips`, reports `UserEvent::ImageKept`, and turns a
   panic in keeping an image into a refusal, so that the next `C-c C-y` is not left waiting.
   The UI thread only routes, and an image it drops goes to the freer's thread. No image bytes
   reach the agent channel or a log. The pasted path reaches the channel only where the person
   let it read the terminal (ADR 0120, Q16).

## Changes to accepted ADRs

- [ADR 0132](0132-a-session-mode-for-hosted-sessions.md), "The keys (Q1)": "`C-c` and a
  control key stays the terminal's" now has two exceptions in a session, `C-c C-u` and
  `C-c C-y` (`session-yank-image`). The table gains `C-c C-y`, `session-yank-image`, with no
  `C-x a` key elsewhere.
- [ADR 0074](0074-the-kill-ring-and-the-clipboard.md), "The clipboard (chosen)": "plain text
  in, plain text out" now also has an image out, read as decision 3 says. The text API is
  unchanged.

## Consequences

- A screenshot reaches a session with `C-c C-y` and Enter, and the image is in Claude Code's
  conversation as Claude Code attaches files it is pointed at.
- `C-c C-y` no longer reaches Claude Code in a session as `Ctrl-C Ctrl-Y`; `C-c C-c C-q C-y`
  still sends both.
- A TIFF-only clipboard costs a process start. A crafted TIFF that crashes ImageIO crashes
  `sips`, not oso.
- The files sit in the cache directory, which backup and cleanup tools treat as disposable.
  Another oso instance's sessions are never swept by membership, only by age.
- Residuals:
  - `dataForType:` copies whatever the app that copied the image hands over before its length
    is known, as a text read does, so an app can make oso hold a large image briefly; a kept one
    peaks at twice `MAX_IMAGE`.
  - `sips` gives crash isolation, not a sandbox: a crafted TIFF that exploits ImageIO runs as
    the person. A sandbox profile for it is a follow-up.
  - `PrivateDir` follows the person's own links on the way to a directory, as everywhere in
    oso; only another user's change to the way is refused.
- Follow-ups: Linux and Windows backends with their window layers. Dropped image files,
  `session-insert-image` from a path, and `C-x a` forms that pick a session, if wanted.

## Alternatives considered

- **`C-c y`, which keeps ADR 0132's rule, with `C-x a y` elsewhere.** The author chose
  `C-c C-y`, the key their previous client used and Emacs's convention for a mode.
- **The state directory, with a quoted `@"…"`.** Whether Claude Code takes the quoted form is
  not documented, and the space would show in every reference. `$TMPDIR` is private on macOS
  too, but the OS empties it on its own schedule and its path is long.
- **Converting in oso's process with `NSBitmapImageRep`.** No process start, and only feature
  flags of crates already in the tree. But ImageIO would parse another app's data inside the
  process that holds every buffer. The author chose isolation.
- **Refusing TIFF.** Simpler, but some apps copy only TIFF.
- **Inline base64.** It would be written into the conversation's recording, and a 10 MB image
  would be a paste of 13 MB.
- **Keeping images until the person removes them.** They would pile up, one file per screenshot.
- **UTC in the name.** Simpler, but the person reads the name as the time they took the
  screenshot, and oso already reads the local offset for dired.
- **The image's disk work on the sessions worker.** It would hold the judging of screens on a
  slow cache, which ADR 0136's step 3 took off that thread for the spaces' lookups.

## Questions

Put to the author on 2026-11-16, the recommended option first. The author chose (a) on each.

1. **The key.** (a) `C-c C-y` in session mode only, a second exception to ADR 0132's rule.
   (b) `C-c y`, with `C-x a y` elsewhere. (c) Both.
2. **Where the files go.** (a) The per-user cache directory, refusing a path with a space.
   (b) The state directory, with `@"…"` if Claude Code takes it. (c) `$TMPDIR`.
3. **How long they stay.** (a) Until the session is archived or dropped, and at most 7 days.
   (b) Until it is archived or dropped. (c) Until the person removes them.
4. **A TIFF.** (a) `/usr/bin/sips` in a child process. (b) `NSBitmapImageRep` in oso's
   process. (c) Refused.
5. **The record.** (a) A new ADR that amends ADR 0132 and ADR 0074. (b) Edits to those ADRs.
