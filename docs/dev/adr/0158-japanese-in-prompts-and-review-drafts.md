# 0158. Japanese in prompts and review drafts

- Status: Accepted
- Date: 2026-11-19

## Context

The author reported that Japanese cannot be typed in some prompts in the top float, such as the
draft `c` opens in a review.

The cause is a rule, not a fault in the input path. [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md)
keeps the input method off whenever a prompt is selected (`wants_ime`'s `reads_input`), as
[ADR 0026](0026-m2-ui-layout.md) chose, so that the next key in a prompt is a key.
[ADR 0106](0106-the-input-source-turns-japanese-on-in-a-file-name-prompt.md) made one exception:
the prompt for a file's name has the input source type ASCII while it is open, and allows the
input method once the person chooses a Japanese input mode there. [ADR 0113](0113-query-replace-keyboard-macros-and-whitespace-cleanup.md)
added query-replace's two prompts. Every other prompt keeps it off. With the input method off, the
view gives AppKit no input context, so a key reaches oso as typed whatever the menu bar shows, and
`かな` only changes the menu bar.

[ADR 0122](0122-agent-review.md) then made a review comment's draft a prompt in the top float (its
Q3 (a)), where the plan had a buffer in a float under the line. The input method's rule was not
looked at again, so the draft fell under "every other prompt": `c`, a reply and `e` on a card
open a draft that takes no Japanese. The same holds for every prompt that reads free text:
session-send's note, a space's title, a session's new name, a stash's or a tag's message,
`insert`, the review's file filter, the prompts for names and revisions, and the prompts that
complete, from `M-x` to `M-s l`. It holds on
Linux and Windows too, where the winit adapter asks the same function.

The author chose on 2026-11-19, through the coordinator: the draft takes Japanese at once as a
buffer does (Q1 (a)), every prompt that reads text follows the input source as ADR 0106's does
(Q2 (a)), and so do the prompts that complete (Q3 (b), not the recommended option, which kept
them off since migemo reads romaji).

## Decision

- **A review comment's draft (`Kind::Draft`) allows the input method as a buffer does.** It is
  on from the moment the draft opens, and oso leaves the input source as the person had it: no
  switch to ASCII and none back. The rules of a buffer apply as they are: it is off while a prefix
  key or argument waits, while a menu is open, while the draft may hold secrets (as a yank of a
  secret entry makes it), and while any window shows a buffer that may hold secrets, as ADR 0106
  has it for every prompt.
- **Every prompt that reads text or completes one follows the input source, as ADR 0106's
  prompt for a file's name does** (`Kind::Text` and `Kind::Complete`): `M-x`, switching buffers,
  a project's files, `M-s l`, a project search, session-send's note, a space's title, a branch's
  name and the rest. When one opens while oso is in front and the selected source cannot type
  ASCII, oso chooses the input method's ASCII source, and gives the person's back when it
  closes, as ADR 0106 says. Choosing a Japanese input mode while it is selected, by `かな`, the
  menu bar or a shortcut, allows the input method for that prompt until it closes or another
  kind of source is chosen. A prompt opened later starts without.
- **A number, a passphrase and a question keep it off**, and with them isearch, query-replace's
  answers, a jump and a menu, where a letter is a key. A passphrase never switches the source
  (ADR 0073 gives it secure input).
- **The keys of a prompt are unchanged.** None of the text, completion or draft keymaps binds a
  plain printable key, so a letter there is text whether the input method is on or not. On oso's
  own macOS layer, with nothing being composed, a key with Control, Option or Command goes to oso
  before the input method ([ADR 0070](0070-windowing-and-input-without-winit.md)), so `C-n`, `C-p`, `M-p`, `M-n`,
  `C-g`, `C-c C-c` and `M-RET` work as before; `TAB`, `RET` and the arrows go through the input
  method, which gives them back to oso (`doCommandBySelector:`), as in a buffer. While text is
  being composed, every key but `C-g` and Option or Command keys goes to the input method first,
  as in a buffer: `RET` confirms the conversion, and `TAB`, `C-n` and `C-p` reach completion only
  if the input method gives them back. On winit (Linux and Windows), where only the draft has the
  input method on, its keys go as they do in a buffer there
  ([ADR 0069](0069-macos-input-method.md)).
- **Completion and migemo.** Text the input method commits is a literal component, matched as
  typed. A `%` component is still migemo's pattern for romaji
  ([ADR 0116](0116-migemo-and-jumps.md)), and a project search, `M-s l`
  ([ADR 0154](0154-consult-line.md)) and `session-search`
  ([ADR 0145](0145-searching-past-agent-conversations.md)) read romaji as migemo's as before; the
  prompt opens in ASCII for all of them. The candidates change on the commit, as with any typed text,
  not during a composition.
- **Linux and Windows:** the draft takes Japanese there too, since it needs no input source. The
  other prompts keep the input method off until their window layers report the input source, as
  ADR 0106 says.
- **Code:** `app/ime.rs`. `names_a_file` becomes `follows_input_source` (`Text` and
  `Complete`), and `japanese_names` becomes `takes_japanese`, which takes a draft at once.
  `secret_changed`, run when a terminal starts or stops reading a password between events, turns
  the input method off at once when that leaves a prompt taking Japanese beside a buffer that may
  hold secrets. The macOS window layer's `update_ascii` logs its time in dev builds. No public
  item, crate, dependency or window-layer call changes.

## Changes to accepted ADRs

- **[ADR 0026](0026-m2-ui-layout.md), "IME":** "the IME is switched off automatically in the
  minibuffer, isearch and menus" now holds for isearch, menus, and the prompts that read a
  number, a passphrase or an answer. A prompt that reads text or completes one follows the input
  source, and a review comment's draft has it on, as this ADR says.
- **[ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md), "The input method":**
  "`wants_ime` gains one condition: a prompt is selected" becomes: a prompt that takes no
  Japanese now is selected. Its alternative "The IME on in prompts that read free text" is taken
  in part: on in a draft, and on once chosen in the others.
- **[ADR 0106](0106-the-input-source-turns-japanese-on-in-a-file-name-prompt.md), Decision:**
  "The prompt for a file's name takes Japanese ... Every other prompt keeps it off, as ADR 0072
  has it" and "Only the prompt for a file's name does this" now hold for every prompt that reads
  text or completes one; a review comment's draft takes Japanese at once and switches no source.
  "Never with secrets ... a passphrase or a question never does, as only the prompt for a file's
  name can" becomes: a passphrase, a number or a question never does, as only a prompt that reads
  text or completes one, or a draft, can; still never while a buffer that may hold secrets is
  shown. In its Consequences, "acts only in that one prompt" and "On Linux and Windows the prompt
  keeps the input method off" now hold for every such prompt, the draft excepted on Linux and
  Windows. Its alternative "The input method on in every prompt" is still not taken: a number, a
  passphrase and a question keep it off.
- **[ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md), Consequences:** "Japanese
  cannot be typed in a prompt until M2-18 decides otherwise", which ADR 0106 already made
  stale, now reads: Japanese can be typed in a review comment's draft, and in a prompt that reads
  text or completes one once a Japanese input mode is chosen there.
- **[ADR 0087](0087-finding-files-writing-under-a-new-name-and-large-files.md), "Japanese file
  names":** "off when the prompt opens, as in every prompt (ADR 0072)" now reads: as in every
  prompt but a review comment's draft.

## Consequences

- A comment, a reply or an edit in a review takes Japanese with no extra key, as a buffer does,
  on every platform.
- Every text or completion prompt on macOS takes Japanese after `かな`, and the menu bar and what
  the prompt types always agree.
- oso now chooses ASCII when any such prompt opens with a Japanese source, `M-x` included, and
  gives the person's source back when it closes. That is two selects
  (`TISSelectInputSource`) and about ten other Text Input Sources calls per prompt, each select
  followed by the system's notice and one more pass after it. They run on the UI thread after
  the key that opens or closes the prompt and before its frame. With an ASCII source, opening a
  prompt costs one look at the current source, and closing it costs nothing. Neither ADR 0106 nor
  this ADR measured them; a select through a third-party input method is the likely slow case.
  Dev builds log each switch's time, and the measurement against the 8 ms key-to-photon budget is
  in `open-questions.md`. If it is over about 1 ms at p99, the switch moves after the frame.
- A draft opened while the person's source is Japanese starts composing at the first letter, as
  a buffer does, and `英数` types ASCII.
- Japanese typed through the input method is precomposed, so without `%` it does not match a
  name stored decomposed, as macOS volumes and zip archives keep some, in `C-x p f`, `C-x b` and
  the like. ADR 0087 and ADR 0116 say so of file names, `open-questions.md` keeps it open, and
  this ADR makes it reachable in many more prompts.

## Alternatives considered

- **The draft as the prompt for a file's name (ASCII first, `かな` for Japanese):** one rule for
  every prompt, but a comment is usually prose, and it would stay off on Linux and Windows.
- **Following the source only in prompts that take prose** (a note, a title, a message, the
  filter): no switch in prompts for names and revisions, but a list of prompts to keep in step
  with the commands, where the switch to ASCII already keeps such names typed as before.
- **Completion prompts kept off, since migemo reads romaji:** the recommended option; the author
  chose to have them follow the source too.
- **The input method on in every prompt:** a question's `y` and `n` and a number's digits would
  start a composition.
