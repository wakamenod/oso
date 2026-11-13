# 0116. Migemo: romaji search for Japanese text, and jumps by label

- Status: Accepted
- Date: 2026-11-12

## Context

The roadmap puts "Migemo-style romaji search for Japanese text, and avy-style jumps" in M3
([roadmap](../roadmap.md)). The author writes Japanese text, and uses migemo (through C/Migemo) and
avy, with avy's migemo variant, in Emacs. Earlier ADRs left these to M3:

- **The input method is off while isearch reads keys,** so a Japanese string cannot be typed there.
  "Migemo (M3) searches Japanese from romaji" ([ADR 0082](0082-isearch.md),
  [ADR 0069](0069-macos-input-method.md)). Every prompt keeps the input method off too, except the
  prompt for a file's name and query-replace's two prompts, where `かな` turns it on
  ([ADR 0106](0106-the-input-source-turns-japanese-on-in-a-file-name-prompt.md),
  [ADR 0113](0113-query-replace-keyboard-macros-and-whitespace-cleanup.md)).
- **Completion's orderless affixes** leave out `%` (char folding) "until migemo (M3) decides how
  Japanese is matched" ([ADR 0076](0076-completion-and-m-x.md)). Romaji matching of file names
  "waits for M3's migemo" ([ADR 0087](0087-finding-files-writing-under-a-new-name-and-large-files.md)).
- **Project search** matches orderless components against each line, with `!` and `=`
  ([ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)).
- **isearch is literal,** with a Knuth–Morris–Pratt matcher over the rope's chunks
  (`oso_core::search`). query-replace's regexps are matched one line at a time with the `regex`
  crate, which needs contiguous text, with a budget of work a key and a slice, and 0113 notes that
  regexp isearch (`C-M-s`) "can use the new matcher". It is not built yet.
- **Only the `regex` crate** is used for regular expressions (AGENTS.md). `oso-core` depends on
  it already, with `unicode-perl`, `unicode-gencat` and `perf`.
- **No GPL code or data can be bundled** (MIT OR Apache-2.0, [ADR 0020](0020-license.md)). The
  common migemo dictionaries derive from GPL SKK dictionaries, and `open-questions.md` lists the
  dictionary's source and licence as open. oso has no network code, so it cannot fetch one.
- **The UI thread never waits** on I/O, parsing or a scan of a whole buffer. Background work runs on
  lazily spawned threads over O(1) snapshots, superseded by a generation (ADR 0082, ADR 0111).
- **Faces** are data with a contrast test per face: 7:1 for text that is read, 4.5:1 for labels,
  3:1 for marks, with OKLab distances between fills
  ([ADR 0089](0089-built-in-themes-switched-at-run-time.md),
  [ADR 0111](0111-tree-sitter-highlighting.md) Q10). An escape (`^M`, `\xFF`) and a boxed code
  point (`U+202E`) have faces of their own (`Face::Escape`, `Face::Boxed`) so they are never
  mistaken for text ([ADR 0061](0061-panes-clipping-and-layers.md)).
- **A frame holds a tree of windows,** each with its own selection set; selecting another window
  flashes ([ADR 0112](0112-window-splits.md), [ADR 0097](0097-visual-aids.md)).
- **Commands act on the selection set;** isearch and query-replace move only the primary cursor
  ([ADR 0091](0091-multiple-cursor-keys-and-m-y.md)).
- **Every command has a reach:** `Look`, `Change` or `Person`. The channel refuses `Person`, waits
  while the person is in the middle of something such as an isearch, and runs nothing while a
  buffer that may hold secrets is shown ([ADR 0085](0085-agent-channel-and-oso-mcp.md)).
- **Settings** live in `config.toml` ([ADR 0108](0108-the-user-configuration-file.md)), which the
  channel never saves.

### What others do

- **C/Migemo** ([koron/cmigemo](https://github.com/koron/cmigemo)), MIT code, turns a romaji query
  into a regexp: each kana form of it (hiragana, katakana, half-width, full-width), and every
  dictionary word whose reading starts with it, joined as a trie. An unfinished end (`kak`) gives
  every kana it can start. An uppercase letter starts a new segment, each expanded on its own and
  joined (`kensakuEnjin` → `検索エンジン`). Between characters it puts a pattern for spaces and a
  line break, unless `--nonewline`. Its dictionary is built from `SKK-JISYO.L`, keeping entries
  with okurigana without their last letter, and "is subject to the GPL" as its source is (its
  README). `SKK-JISYO.L` holds about 176,000 readings.
- **migemo.el** ([emacs-jp/migemo](https://github.com/emacs-jp/migemo)), GPL, runs `cmigemo` as a
  process. It is on in isearch by default (`migemo-isearch-enable-p`), `M-m` toggles it, the
  prompt then says `[MIGEMO]`, and a query that already holds a non-ASCII character is searched as
  it is. It is not used in regexp isearch or in query-replace. orderless has no migemo style of
  its own; people define one that matches a component as its migemo pattern
  ([orderless](https://github.com/oantolin/orderless)).
- **Permissive dictionaries:** [yet-another-migemo-dict](https://github.com/oguna/yet-another-migemo-dict)
  is BSD-3-Clause, built from Mozc's `single_kanji.tsv` (BSD-3) and UniDic (under its BSD
  option), and ships their licences. Version 0.6 holds 89,177 readings and 201,827 words, in a
  `migemo-dict` file of 2.6 MB; words with kanji between kana are left out for now.
  [jsmigemo](https://github.com/oguna/jsmigemo) (MIT) ships it, and so does tmux-fuzzy-motion,
  whose notice lists the Mozc and UniDic licences. Larger sources are
  [SudachiDict](https://github.com/WorksApplications/SudachiDict) (Apache-2.0, with UniDic's BSD
  notice and NEologd's attributions; 273,000 readings in its small tier, from 226 MB of CSV) and
  Mozc's OSS dictionary (IPAdic's NAIST and ICOT notices; 746,000 readings, 60 MB). Of the SKK
  dictionaries, only the tiny `pubdic+` maps readings to words and is not GPL
  ([skk-dev/dict](https://github.com/skk-dev/dict/blob/master/committers.md)). Unihan's
  `kJapaneseKun` and `kJapaneseOn` give single kanji only.
- **Rust:** [rustmigemo](https://github.com/oguna/rustmigemo) is MIT but not on crates.io, and the
  dictionary its README points to is GPL-3.0, generated from SKK.
- **Vim:** [kensaku.vim](https://github.com/lambdalisue/vim-kensaku) runs jsmigemo and, by
  default, downloads an SKK-derived dictionary at run time, which oso, with no network code,
  cannot do. EasyMotion has `g:EasyMotion_use_migemo`.
- **avy** ([abo-abo/avy](https://github.com/abo-abo/avy)), GPL, labels targets in view:
  `avy-goto-char-timer` reads characters until 0.5 s pass (`avy-timeout-seconds`), highlighting
  their matches, and `RET` ends early; `avy-goto-line` (`M-g f` in its README) labels line starts,
  and a digit there opens `goto-line`; `avy-isearch` labels isearch's matches. Defaults: keys
  `a s d f g h j k l`, style `at-full` (the label over the target), all windows of the frame, no
  dimming, and a single candidate jumped to at once. Labels come from a tree, so earlier
  candidates get shorter labels; only `avy-goto-char` sorts by distance from the cursor. A key in
  `avy-dispatch-alist` (`x` kill, `t` teleport, `m` mark, `n` copy, `y` yank, `z` zap, and more)
  acts on the target instead. [avy-migemo](https://github.com/momomo5717/avy-migemo) (GPL) adds
  migemo, but has not changed since 2019, and parts of it no longer work with avy.
- **Elsewhere:** [flash.nvim](https://github.com/folke/flash.nvim) shows labels during `/` search,
  chosen so that a label is never a letter that continues the pattern, nearest first.
  [leap.nvim](https://codeberg.org/andyg/leap.nvim) takes two characters and then a label.
  [Helix](https://github.com/helix-editor/helix/blob/master/book/src/keymap.md) labels every word
  in view with two characters (`gw`), and Zed has taken that up. [AceJump](https://github.com/acejump/AceJump)
  does it in JetBrains IDEs, and VS Code has extensions such as MetaGo.
  [ace-window](https://github.com/abo-abo/ace-window) selects a window by a label over avy's tree.

## Questions

Put to the author through the coordinator, the recommended option first. The author chose (a) on
each on 2026-11-12.

### The shape of this ADR

1. **One ADR or two:** (a) one, for migemo and jumps together: they meet where a jump reads romaji,
   and share the view's matcher and its limits; (b) two: this one for migemo, and a later ADR for
   jumps, so each is decided and built on its own.

### Migemo

2. **The dictionary, which gives kanji (the key question):**
   (a) **Kana built in, and a dictionary the person names.** oso converts romaji to hiragana,
   katakana and full-width letters with a table of its own, and needs no data for that. For kanji,
   `[search] migemo-dictionary` in `config.toml` names a file the person has: C/Migemo's
   `migemo-dict`, which a package manager installs, or yet-another-migemo-dict's, which the docs
   name as a permissive one to download. oso reads it at run time and never ships it, so a GPL
   dictionary stays the person's own copy. With no file named, romaji finds kana but no kanji.
   (b) **(a), and yet-another-migemo-dict in the binary** (BSD-3-Clause), used when no file is
   named: kanji with nothing to set up, at 2.6 MB more binary, its three licence texts shipped with
   oso, and a vendored data file to review at each update. It has half of `SKK-JISYO.L`'s readings
   and no words with kanji between kana yet. It can be added later without changing (a).
   (c) **(a), and a dictionary oso generates itself** from SudachiDict (Apache-2.0) or Mozc's OSS
   dictionary: three to eight times the readings, but a generator to maintain over hundreds of
   megabytes of source, a larger file, and more notices.
   (d) **Kana only:** no dictionary of any kind, and no kanji.
3. **Dictionary formats (if Q2 is not (d)):** (a) C/Migemo's `migemo-dict` (a reading, a tab and
   its words) and SKK's format (`reading /word/word/`), both in UTF-8, told apart by each line's
   shape. (b) `migemo-dict` only. (c) (a), and SKK's files in EUC-JP, as `SKK-JISYO.L` ships: a
   decoder, which is a dependency (`encoding_rs`) or a table of oso's own.
4. **Where migemo applies:** (a) isearch, and the characters a jump reads; project search's
   components, as isearch's rule decides for each; completion (`M-x`, `C-x b`, `C-x C-f`,
   `C-x p f`) only through the `%` affix; never query-replace, whose prompts take Japanese.
   (b) Everywhere by isearch's rule, completion included: a component such as `sa` then also
   matches every candidate with `さ` or `サ`, which `M-x` does not need. (c) isearch and jumps
   only.
5. **When isearch uses it:** (a) by itself, while the string is romaji: lowercase ASCII letters,
   `'` and `-` for `ー`; any other character, an uppercase letter among them, searches literally as
   today. `M-m` turns migemo off or on for the rest of this search, as migemo.el's key does, and
   the line says `[migemo] I-search:`. `[search] migemo = false` turns it off by default.
   (b) Off until `M-m` turns it on. (c) As (a), but an uppercase letter starts a new word,
   as C/Migemo's segments do (`kikiKaikan` → `危機会館`), rather than making the search
   case-sensitive.
6. **Line breaks:** (a) a match lies within one line, as 0113's regexps: a word that a hard wrap
   breaks across two lines is not found. (b) Allow a line break, and the spaces around it,
   between any two characters, as migemo.el does: this needs a matcher across lines that the
   `regex` crate over one line cannot give, so later, with its own ADR.
7. **How big a pattern may get, and where it is built:** (a) on the UI thread at each key, from a
   cache of the last 32 strings, so `DEL` builds nothing. Words come from the dictionary entries
   whose reading starts with the kana typed, while there are at most 2048 of them; past that only
   the entries whose reading is the kana itself, so `k` alone gives kana and `kanj` gives `漢字`.
   The compiled pattern's size limit is 4 MiB. Building and compiling must stay under 2 ms at
   p99 over the author's dictionary on their Mac, measured in step 3; if it does not, the build
   moves to the search worker. (b) Always on the worker: a frame late at every key, which ADR 0082
   turned down for isearch. (c) No cap on words, as C/Migemo has none: `k` alone gives a pattern
   of tens of thousands of words.
8. **Loading the dictionary:** (a) on a worker, at the first search that would use it, not at
   start-up; searches until it is in use kana only, and the search on screen runs again when it
   comes. It is read whole, at most 64 MiB, into one sorted table, kept for the session, and read
   again when the setting changes. (b) At start-up, on a worker: kanji from the first search, but
   every start reads it, also when no search follows. (c) Mapped into memory: no read at all, but
   it needs `unsafe` or a dependency, and a file changed under oso changes the table.
9. **Where the code lives:** (a) a public module `oso_core::migemo`, pure, with no I/O: the
   romaji table, the dictionary from bytes, and a pattern for a string; the tip crate reads the
   file, and `search` and `complete` use the module. (b) A crate `oso-migemo`: it builds alone and
   others could use it, but it is a new crate and a new edge for one module's worth of code.

### Jumps

10. **Which jumps in M3:** (a) `avy-goto-char-timer`, `avy-goto-line`, `avy-goto-end-of-line` and
    `avy-isearch` (labels on isearch's matches in view). (b) (a), and `avy-goto-word-1`,
    `avy-goto-word-0` and `avy-goto-char-2`. (c) `avy-goto-char-timer` only.
11. **Names and keys:** (a) avy's names, so `M-x` finds them as in Emacs; `M-j` for
    `avy-goto-char-timer`, and in isearch for `avy-isearch`; `M-g f` for `avy-goto-line`, as
    avy's README suggests; `avy-goto-end-of-line` by `M-x` until the menus come. (b) oso's own
    names (`jump-to-char`, `jump-to-line`), with the same keys. (c) avy's names, and no keys bound.
12. **The label keys:** (a) `a s d f g h j l`: eight keys of the home row, leaving `k` for an
    action. (b) avy's default, `a s d f g h j k l`. (c) More keys, such as the home row and the
    row above it: fewer labels of two keys, but keys off the home row. Each can be changed in
    `[jump] keys`.
13. **When labels appear:** (a) as `avy-goto-char-timer`: characters are read until 0.5 s pass
    with no key, or `RET`, and then the labels show; `[jump] timeout` changes the time. (b) As
    flash.nvim: labels show at each key, taken from letters that would not continue any match,
    so there is no timer, but the labels change as one types.
14. **A single candidate:** (a) still labelled, so an action can apply to it. (b) Jumped to at
    once, as avy's default does.
15. **How labels look:** (a) over the target's first cells, as avy's default style (`at-full`),
    in a new face `JumpLabel`: a strong fill no other face uses, its text at 7:1 on it, apart from
    `Escape`'s, `Boxed`'s, the search fills' and the region's by an OKLab distance, so a label is
    never read as an escape or as text; a second key of a label in a second face, as avy does;
    the rest of the text as it is. (b) As (a), and the rest of the text dimmed, as
    `avy-background`. (c) Before the target, pushing the text to the right (`pre`): nothing is
    hidden, but the text moves.
16. **Several windows:** (a) labels in every tiled window of the frame, as `avy-all-windows`; the
    selected window's candidates nearest the cursor get the shortest labels; `C-u` keeps to the
    selected window; a jump into another window selects it, and it flashes as ADR 0112 has it.
    (b) The selected window only, and `C-u` for all.
17. **Cursors and the mark:** (a) the primary cursor jumps, and the others stay, as in isearch.
    The place it left is pushed onto its mark ring without a message, as avy does; a region
    shown stays shown, so a jump extends it. (b) A jump drops the other cursors.
18. **Actions:** a key that is not a label acts on the target, as `avy-dispatch-alist`. (a) In M3:
    `SPC` sets the region from the cursor to the target; `W`, `K`, `Y` and `T` copy, kill, yank
    and move the target's whole line, coming back; `c` adds a cursor at the target. Avy's
    actions on an expression wait for structural motions. (b) No actions in M3: a jump moves.
    (c) (a), and the word versions of copy, kill, yank and move (`w`, `k`, `y`, `t`).
19. **What candidates cost:** (a) only the text in view: each window's rows, within isearch's
    256 KiB of text a frame, and at most 4096 candidates, which eight keys label with four keys or
    fewer; past that, the farthest are not labelled and the echo area says how many. (b) At most
    512, labels of three keys at most.
20. **The agent channel:** (a) every jump command is `Person`, as they read keys; the channel
    waits while a jump is on, as for isearch; `get-state` gains `jump`, with no positions.
    (b) Also a `Look` command for the channel that jumps to a string's nearest match in view:
    the channel can already move with `goto-line` and `open`.

## Decision

The author chose on 2026-11-12, through the coordinator, the first option of each question under
[Questions](#questions), which the text below describes. No dictionary is bundled (Q2): oso
builds kana itself, and reads for kanji only a file the person names.

### 1. Migemo

#### What a romaji string matches

- **A table of oso's own turns romaji into kana:** the Hepburn and kunrei spellings and the ones
  Japanese input methods take (`shi` and `si`, `chi` and `ti`, `tsu` and `tu`, `fu` and `hu`,
  `ja`, `jya` and `zya`), small kana with `x` or `l` (`xa`, `ltu`, `xtsu`), a doubled consonant
  for `っ` (`kka`), `nn`, `n'` and an `n` before a consonant for `ん`, `wo` for `を` and `-` for
  `ー`. The table is data in the module, with a test per row.
- **An unfinished end matches every kana it can start,** as C/Migemo does: `k` matches `か`, `き`,
  `く`, `け`, `こ` and `っ`; `ky` matches `きゃ`, `きゅ` and `きょ`; an `n` at the end matches `ん`
  and the `な` row.
- **The string then matches any of:** the romaji itself, with ASCII case folded; its hiragana; its
  katakana, full-width and half-width; its full-width letters (`ｋａｎ`); and, with a dictionary,
  every word whose reading starts with its hiragana, within Q7's limits. A dictionary's readings
  are hiragana, and a katakana word is found through the katakana form.
- **The pattern is a trie of alternatives,** with common prefixes taken out, as C/Migemo builds
  it, and every kana and word put in through `regex::escape`, so nothing in a dictionary is
  regexp syntax. `regex::RegexBuilder` compiles it with a size limit of 4 MiB and a nest limit
  of 32. A pattern over the limit drops the dictionary's words and keeps the kana.
- **A pattern is cached per string,** the last 32, so `DEL` and a repeat build nothing.

#### isearch

- **Migemo isearch is a regexp isearch whose pattern comes from the string.** So step 1 builds
  regexp isearch (`C-M-s`, `C-M-r`) on 0113's matcher, and migemo uses it: isearch's rules hold
  as they are, with 0113's per-line regexps: 256 KiB of text and 3 ms a key on the UI thread, the
  rest and the count on the worker, highlights only in view.
- **It applies by itself while the string is romaji (Q5):** lowercase ASCII letters, `'` and `-`.
  Any other character searches literally, as today, with smart case. A match lies within one
  line (Q6).
- **`M-m` turns migemo off or on for the rest of this search,** and the line then says
  `[migemo] I-search:` while it applies, as migemo.el's `[MIGEMO]`. `[search] migemo = false`
  in `config.toml` starts every search with it off.
- **The string enters the history as typed,** the romaji, as before.
- **`get-state`'s `isearch` gains `migemo`,** whether the pattern is migemo's, and nothing of the
  pattern.

#### Project search and completion

- **A project search component that is romaji by the same rule matches as its migemo pattern,**
  which holds the literal too; `=` keeps a component literal. The pattern is built once per
  component, as ADR 0076 compiles a regexp component, and the `oso-grep` threads share it.
- **In completion, `%` makes a component migemo's:** `%kan` matches `漢字.md` with a dictionary,
  and `かんじ.md` without one. Without `%` a component matches as ADR 0076 says.
- **query-replace does not use migemo:** its prompts take Japanese (ADR 0113).

#### The dictionary

- **`[search] migemo-dictionary`** names a file: an absolute path, or one starting with `~/`.
  With none, migemo gives kana only, and says nothing. Only the user's `config.toml` names it:
  nothing in a project does, so opening one reads no dictionary.
- **It is read on a thread of its own, `oso-migemo`, at the first search that would use it,**
  and kept for the session. It is opened through `oso_platform::file_meta::open_for_reading`, so
  a special file is never waited on. It must be a regular file of at most 64 MiB, in UTF-8.
  Until it is in, searches use kana only, and the search on screen runs again when it comes.
  A change of the setting reads the new file, and the old table serves until it is in.
- **Two formats, told apart by each line (Q3):** C/Migemo's `migemo-dict` (a reading, a tab,
  words separated by tabs) and SKK's (`reading /word/word/`). SKK's `;` comment lines and a word's
  `;annotation` are skipped, and an entry with okurigana (`かk /書/`) keeps its reading without
  the last letter, as C/Migemo's converter does. A line in neither format, a reading that is not
  hiragana, a word over 64 bytes, and the words of a reading past 256 are skipped too, so a
  crafted file gives a bounded table.
- **The table is one string arena and a sorted list of entries,** each a reading's range and its
  words' range, so a prefix's entries are found by binary search. It takes about the file's size,
  plus 16 bytes an entry.
- **`*Messages*` says what came of it:** `Migemo dictionary: 123,456 readings`, or why it could
  not be read. The path is the person's own setting. No word of the dictionary is ever logged.

#### Secrets and the sockets

- **Migemo works in a buffer that may hold secrets, as isearch does.** The pattern is the
  string's, and kept as the string is: never logged, out of `Debug`, out of the history around a
  secret, and out of `get-state` there.
- **The channel cannot change which dictionary is read,** as it never saves `config.toml`
  (ADR 0085).
- **`isearch-toggle-migemo` (`M-m`) is `Person`,** as every isearch command is.

### 2. Jumps

#### Commands and keys (Q10, Q11)

| Keys | Command | What it does |
|---|---|---|
| `M-j` | `avy-goto-char-timer` | Reads characters until 0.5 s pass, then labels their matches in view |
| `M-g f` | `avy-goto-line` | Labels each line's start in view; a digit opens `goto-line` with it |
| | `avy-goto-end-of-line` | Labels each line's end in view |
| `M-j` in isearch | `avy-isearch` | Labels isearch's matches in view, and ends the search at the one chosen |

#### A jump

- **A jump is a mode of the selected window, as isearch is.** Its keymap takes the others' place
  while it is on, and the input method is off. It refuses inside a prompt: "Not in the minibuffer".
- **`avy-goto-char-timer` reads a string first,** of at most 64 characters, on the echo row:
  `Jump: ka`. Each key adds a character, `DEL` takes one away, and the matches in view show in
  isearch's other-match fill. 0.5 s after the last key (`[jump] timeout`), or at `RET`, the labels
  show. With no match, the echo area says `No candidates` and the jump ends. A romaji string
  matches by migemo, by isearch's rule.
- **Then each candidate shows a label.** Typing a label's first key keeps the labels that start
  with it; its last key jumps. Labels come from `[jump] keys`, `a s d f g h j l` by default, as
  avy's tree gives them: as many candidates as can get one key do, nearest first, and the rest
  share the other keys as prefixes, so no label starts another.
- **A single candidate is labelled too (Q14),** so an action can apply to it.
- **A key that is neither a label nor an action ends the jump** with `No such candidate`, and does
  not run, as in avy, so a mistyped label inserts nothing. `C-g` ends it with nothing moved, as
  `Quit::Jump`, the first step of `Editor::quit`, before the region's, so one `C-g` always ends
  a jump. (This ADR first put it after isearch's steps and query-replace's; the author chose the
  first place while step 5 was built.)
- **The primary cursor jumps; the other cursors stay (Q17).** The place it left is pushed onto
  its mark ring without a message. A region shown stays shown, so a jump extends it. The flash of
  ADR 0097 follows its rule for a large move.
- **`avy-isearch` ends the search at the match chosen,** as `RET` there does: the string enters
  the history and the mark is saved where the search began.

#### Windows (Q16)

- **Labels show in every tiled window of the frame.** The selected window's candidates come
  first, nearest its primary cursor first, then the other windows' in the tree's order, each
  nearest its own primary cursor. `C-u` keeps to the selected window.
- **A jump into another window selects it,** and moves that window's primary cursor; the window
  flashes as ADR 0112 has it.
- **A window over a buffer that may hold secrets is labelled as any other:** the screen shows its
  text already.

#### Actions (Q18)

After the first key of a label, or before it, a key from this table acts on the target the label
then chosen points at, as avy's `avy-dispatch-alist` does, and none of them is a label key:

| Key | Action |
|---|---|
| `SPC` | Sets the region from the cursor to the target |
| `W` | Copies the target's whole line, and stays |
| `K` | Kills the target's whole line, and stays |
| `Y` | Copies the target's whole line and yanks it at the cursor |
| `T` | Kills the target's whole line and yanks it at the cursor |
| `c` | Adds a cursor at the target |

- An action is one undo step, in each buffer it changes. A kill from a buffer that may hold
  secrets marks the kill, and a yank of it marks the buffer it goes into, as ADR 0074 has it.
- `K` and `T` in a read-only buffer say "Buffer is read-only" and do nothing.

#### Labels on screen (Q15)

- **A label covers the target's first cells,** as avy's `at-full`: one cell a key, from the
  target on, within its row; a label that would run past the row's end is cut there. A wide
  character takes two keys' cells. A target with no cell, such as an empty line's end, shows its
  label in the first cell after the text.
- **Two new faces:** `JumpLabel` for a label's first key, and `JumpLabelNext` for the keys after
  it. Each is a strong fill no other face uses, its text at 7:1 on it, as text that is read. The
  contrast test keeps both fills apart, by an OKLab distance, from `Escape`'s, `Boxed`'s, the two
  search fills', the region's and each other's, so a label is never taken for an escape, a match
  or text. The three built-in themes each give them a colour.
- **`TextView` gains `labels`,** a list of cells, their text and face, drawn after the text, as
  `fills` are drawn before it. The rest of the text is drawn as it is.

#### What candidates cost (Q19)

- **Only the text in view is looked at:** each window's visible rows, within 256 KiB of text a
  window and 1 MiB a jump in all, the selected window first and then the others in the tree's
  order. (This ADR first gave 256 KiB a window alone; the author added the jump's limit while
  step 5 was built.) A string's candidates are found with the matcher isearch's
  highlights use, a line's start and end from the display rows.
- **At most 4096 candidates** are labelled, which eight keys label with four keys or fewer. Past
  that, the farthest are not labelled, and the echo area adds `[4096 of 5210 labelled]` after
  the string.
- **While a string is read, its matches are found again at each key,** within the same limits; a
  frame's cost stays bounded by the view, whatever the buffer's size.

#### Secrets and the sockets (Q20)

- **Every jump command and action is `Person`,** as they read keys. The channel waits while a
  jump is on, as for isearch.
- **`get-state` gains `jump`:** `null`, or `stage` (`reading` or `labels`) and `count`; while a
  buffer that may hold secrets is shown, only `{"active": true}`. The string is left out as
  isearch's is there. Nothing gives the labels' places.
- **A dev restart waits for a jump to end,** as for isearch.

### 3. Code and layers

- **`oso_core::migemo`, public (Q9):** `Dictionary` (from bytes, with its limits), `Romaji` (the
  kana a string gives) and `pattern`, which gives a pattern's text for a string and a dictionary.
  It does no I/O and uses only `regex::escape`. It sits on the lowest line of
  `guardrails/layers.txt`, beside `sum_tree`, so `search` and `complete` can use it.
- **`oso_core::jump`,** public: `Jump`, the session's state (read-only), and the label tree. It
  sits beside `minibuffer`, below `editor` and above `frame`.
- **In `oso-core`'s editor:** `Editor::jump*` methods and `Quit::Jump`. In `oso-render`:
  `Face::JumpLabel`, `Face::JumpLabelNext` and `TextView::labels`.
- **In the tip crate:** the dictionary's reader thread (`app/migemo.rs`), the jump commands and
  their keymap, and the settings `[search] migemo`, `[search] migemo-dictionary`, `[jump] keys`
  and `[jump] timeout`, each checked as ADR 0108 checks a setting: `keys` holds 2 to 32 distinct
  printable ASCII characters, none of them an action's key, and `timeout` is 0.1 to 5 s.
- **No new dependency,** with the recommended options.

### 4. Steps

One pull request a step, each with its own "As built" notes here.

1. **Regexp isearch:** `C-M-s` and `C-M-r` on 0113's per-line matcher, with backward search
   within a line, the count and the view's highlights; smart case as 0113's regexps have it.
2. **Migemo with kana:** `oso_core::migemo` (the romaji table, the kana forms, the trie pattern
   and its cache), its line in `guardrails/layers.txt`, migemo in isearch by Q5's rule, `M-m`,
   `[search] migemo` and `get-state`'s `migemo`.
3. **The dictionary:** `[search] migemo-dictionary`, the `oso-migemo` reader thread, the two
   formats and their limits, the table and its prefix search, and the pattern's limits measured
   over C/Migemo's dictionary on the author's Mac (Q7's 2 ms at p99).
4. **Migemo elsewhere:** project search's romaji components and the `%` affix in completion.
5. **Jumps:** `oso_core::jump` and its line in `guardrails/layers.txt`, the session and
   `Quit::Jump`, the four commands and their keys, the label tree, `JumpLabel` and
   `JumpLabelNext` in the three themes with their contrast tests, `TextView::labels`, every tiled
   window, `[jump] keys` and `[jump] timeout`, and `get-state`'s `jump`.
6. **The actions:** `SPC`, `W`, `K`, `Y`, `T` and `c`.

Step 5 needs none of steps 1 to 4, and can be built beside them; a jump's string goes through
migemo once step 2 is in.

#### As built (step 1, 2026-11-12)

The author chose the first option of each of nine questions on 2026-11-12, through the
coordinator; the notes below are those choices as built.

- **Keys.** `C-M-s` (`isearch-forward-regexp`) and `C-M-r` (`isearch-backward-regexp`) start a
  regexp search. In any search `C-M-s` and `C-M-r` repeat it, as `C-s` and `C-r` do, as in Emacs.
  `M-r` (`isearch-toggle-regexp`) turns the string from literal to a regexp or back for the rest
  of the search, as `M-c` turns case: `DEL` takes the turn back. All three commands are
  `Person`. The line says `Regexp I-search:`, with `Failing`, `Wrapped`, `backward` and the case
  words as before (`Failing regexp I-search backward:`).
- **The API:** `Editor::isearch_begin(backward, regexp)`, `Isearch::regexp` and
  `Editor::isearch_toggle_regexp`. `get-state`'s `isearch` gains `regexp`, left out around a
  buffer that may hold secrets as the rest is. `SearchJob` and `SearchReport` keep their public
  surface. Nothing in `guardrails/layers.txt` changes.
- **Matching** is 0113's per-line matcher with its limits (1024 characters, a line over 64 MiB
  stops the search) and its smart case, `M-c` turning it around and an inline `(?i)` or `(?-i)`
  winning. A `C-w` or `C-y` into a regexp is quoted, as Emacs's `isearch-yank-string` quotes
  it, and cut at the string's room between escapes, never inside one. The U+FFFD of an invalid
  byte can match `.`, as in `C-M-%`.
- **Compiling on a key.** The UI thread compiles a key's regexp within 256 KiB, which takes about
  1 ms at worst, also for one that fails for size. Case folding a large Unicode class takes
  up to 2.8 ms however small the result, `\p{Any}` about 1.4 ms and `[\s\S]` 2.6 ms, so a
  pattern that may hold more than one also goes to the worker: outside brackets `\p` and `\P`
  weigh two and a code point escape one, a bracket class that holds a negated Perl class, a
  Unicode class, a code point escape or a character that is not ASCII weighs three, and a key
  compiles at most two. There, up to 0113's 2 MiB, it compiles, and the
  worker searches and counts in the same job; the line says `[searching…]` until it comes, and
  a newer key takes its place between the compiles.
- **The regexps for a cut.** 0113 makes two more regexps from a pattern for a match at a cut
  inside a line, which only a backward search meets here; they take about twice the pattern's
  compile. They are built when first needed: on a key only for a regexp the key compiled within
  0.5 ms, else the worker goes on with the search; on the worker with its compile; and once as
  the keys that add cursors begin from the search.
- **Long lines.** A key never copies a line longer than 16 KiB, or than what is left of its
  256 KiB: such a line goes to the worker at once. One look at a 16 KiB line takes under a
  millisecond also where the crate's fastest engine gives up, as on a `\b` beside text that is
  not ASCII (0.9 ms, and 3.7 ms at 64 KiB).
- **The worker** asks whether a newer job took its place before every look for a match, not
  every 64 KiB of counted work, as one look can read to the end of a long line; query-replace's
  regexps share this.
- **A key's cost**, the highlights included, measured in a release build by the ignored test
  `key_costs` (`cargo test -p oso-core --release --lib key_costs -- --ignored --nocapture`):
  over 20 MB of short lines with no match, 0.58 ms for a literal string, as before this step
  (0.57 ms); 1.2 ms for `z\w+q`, 1.5 ms backward; 1.8 to 2.3 ms for
  `(?i)\w+ \w+ \w+ \w+ \w+ zq`, at most 1.7 ms over 100 keys; 0.85 ms for `[\p{L}\d]{25}q`,
  which is left to the worker. On an 8 MB line, which goes to the worker, 0.07 ms. Each key's
  search also stops after 3 ms, as 0113's do.
- **Backward** is Emacs's `re-search-backward`: the match that starts last at the point or
  before and ends by it. Each line is looked in from its end in windows that double, from 256
  bytes, so a line with no match is read about twice. In the window that holds a match, the last
  start is found by halving, as whether a match starts at a place or after it only turns false
  as the place moves on: `aa` in `aaa` from the end is 1..3, in a few looks. The bound is a cut
  as 0113's ranges are: a match ends by it, while `$` and `\b` there see what follows; `\B`
  there sees the haystack's end, as in 0113, so `foo\B` is not found ending at the bound. A
  repeat's match ends by the current match's start. Typing keeps the current match while the
  regexp still matches at its start and ends by where the search began, or anywhere once it went
  round, as `isearch.el` keeps it.
- **Empty matches** go as 0113's and the count's: an empty match right where a match ended is
  passed, and a repeat after an empty match goes a character on (back) first, so the index steps
  as the count has them.
- **An incomplete or wrong regexp** stays where the last one matched, with no count, no
  highlights and nothing to repeat, and the line says `[incomplete input]` while the crate says
  something is unclosed, a trailing `\` or a flag is cut, as Emacs says it for its `Premature`
  and `Unmatched` errors, or else `[Invalid regexp: …]` in the crate's words, which never quote
  the pattern. It is not `Failing`. `C-g` goes back past it to the last string that matched, as
  past a failing part. `RET` ends the search there and keeps the string, as Emacs does, and
  `M-p` goes on past it to older ones.
- **A line over 64 MiB** fails the search there, with 0113's words after the string:
  `Failing regexp I-search: foo [Regexp search stops at line 3: it is longer than 64 MiB]`. The
  next `C-s` goes round, as after any failure, and `DEL` back to that step says it again. A
  count that meets one shows no count.
- **The history** of regexps is its own, `isearch-regexp`, as Emacs's `regexp-search-ring`, with
  the same limits and the same rule around secrets. `C-s C-s`, `M-p` and `M-n` use the one of
  the search's kind, and the string goes into the one of its kind when it ends.
- **Highlights** of a regexp read each line in view from a little before what shows to a little
  after, and a character before that so `^` and `\b` see what is there, at most 256 KiB and
  2 ms a frame, in place of 0113's whole line up to 64 KiB; query-replace's are the same.
- **Cursors** from a regexp search (`C->`, `C-<`, `C-c C-<`) go on the regexp's matches, never on
  an empty one, as found one line at a time, keeping the line between matches, with 0113's
  limits and 3 ms on the UI thread. Past a line too long for a regexp, a search for cursors finds
  no more that way.
- **Later, as a chore of its own:** query-replace's search for the next match after an answer
  still copies a long line on the UI thread, about 40 ms on a 60 MB line (0113's "Limits"); the
  rule above, that a key leaves such a line to the worker, would fit it too.

#### As built (step 2, 2026-11-13)

The author chose the first option of each of six questions on 2026-11-12, through the
coordinator; the notes below are those choices as built. Besides the strings that search
literally (a letter that starts no spelling, over 256 characters, or a pattern over a key's
limit, where the Decision kept the kana), two depart from the Decision's text:
`oso_core::migemo` stays private until step 4, and the cache holds compiled needles, in the
editor.

- **The module.** `oso_core::migemo` is private to the crate until step 4, when the tip crate's
  project search needs it, so its public shape is settled once, with the dictionary. It holds
  the romaji table (`migemo/table.rs`, one row a spelling, tested row by row), the other forms
  (`migemo/forms.rs`) and the trie (`migemo/trie.rs`), and uses only `regex::escape`. It sits on
  the lowest line of `guardrails/layers.txt`, beside `sum_tree`.
- **One reading.** The longest spelling wins, and no spelling starts another, so a string has
  one reading: `nn` is `ん`, as C/Migemo and input methods have it, so `konnichiha` is
  `こんいちは` and `konnnichiha` finds `こんにちは`. Besides the table, `tch` and a doubled
  consonant give `っ`, and `n'`, an `n` before a consonant other than `y` and an `n` before `-`
  give `ん`, as does Hepburn's `m` before `b` or `p` (`shimbun` finds `しんぶん`). An `m`
  before `m` doubles, as input methods have it (`mma` is `っま`), so Hepburn's `semmon` does
  not find `せんもん`: the author chose this on 2026-11-13 over `ん` alone and over either,
  which made the largest pattern of all. The table also takes the input methods' `kwa`,
  `gwa`, `xyi`, `xye`, `lyi` and `lye`.
- **`ず` and `づ`, `じ` and `ぢ`.** Hepburn and kunrei write both of each pair alike, so `zu`
  matches `ず` or `づ`, and `ji` and `zi` match `じ` or `ぢ`, as C/Migemo has it: `tsuzuku`
  finds `つづく` and `hanaji` finds `はなぢ`. The reading keeps one place for either, and
  the trie writes it as a class in each form (`[ずづ]`, `[ズヅ]`, `[ｽﾂ]ﾞ`), so a string of
  many grows its pattern no faster than one of other kana. `du` and `di` stay `づ` and `ぢ`
  alone, and `ja` stays `じゃ`.
- **A letter that starts no spelling** before the end (`struct`, `rust`, `http`) leaves the string
  literal: no `[migemo]`, the literal matcher, and no limit on a line's length.
- **The forms:** the string, its hiragana, katakana and half-width katakana (`ｶﾞ`, a katakana with
  no half-width form taking its nearest), and its full-width letters, `-` and `'`, compiled with
  the search's case. Decomposed kana (a base and U+3099) are left to step 4, in
  `open-questions.md`.
- **The API:** `Editor::set_migemo`, `Editor::isearch_toggle_migemo` and `Isearch::migemo`, which
  says whether its pattern is migemo's now: the switch is on, the search is not a regexp's, and
  the string is romaji that gives kana. `get-state`'s `isearch` gains `migemo`, left out around a
  buffer that may hold secrets as the rest is. `isearch-toggle-migemo` is `Person`.
- **`M-m`** turns the search's switch and searches again from the match, as `M-r` does; `DEL`
  takes it back. In a regexp's search it turns the switch, which shows once `M-r` makes the
  string literal. `M-c` makes the letters case-sensitive; kana have no case. The line puts
  `[migemo]` before Emacs's words: `[migemo] Failing I-search: kan`.
- **Limits.** A string of at most 256 characters has a pattern, at most about 18 bytes a letter
  with a constant for an unfinished end; a longer one searches literally, as no one types that
  much romaji and its compile would pass a key's millisecond. The pattern compiles on a key,
  within 0113's 256 KiB and a nest limit of 32, in under a millisecond at 256 characters; over
  that it would search literally, which no romaji string reaches. The largest patterns, by
  what they take of the size limit, are those of a `-` and a spelling such as `ja`, `va` or
  `vo` repeated, then of a doubled consonant (`kk`, `ss`); `a` repeated takes about a quarter
  less. `the_largest_migemo_patterns_compile_on_a_key` compiles them. 0116's 4 MiB and the build on
  the worker wait for the dictionary (step 3). Searching keeps step 1's limits.
- **`C-w` after a kana match** adds the kana after it: with `かんじ kanji`, `C-s ka C-w` makes
  the string `kaんじ`, which is not romaji, so the search turns literal and fails, by the rule
  above. After a match of the letters, `C-w` adds letters and migemo goes on. `C-y` and `M-p`
  bring a string as it is, so romaji from either is migemo's.
- **The cache** holds the last 32 compiled needles, by string and case, in the editor
  (`editor/migemo.rs`, beside isearch and jumps), so `DEL`, a repeat and `C-s C-s` compile no
  pattern again; a string that gives no kana is not kept, as it costs little to look at
  again. Migemo's pattern has no assertion, as every character is escaped, so a match that ends
  where a range is cut is one as it is, and its needle never builds step 1's regexps for a cut,
  on a key, for cursors or on the worker. A string typed around a
  buffer that may hold secrets stays out of it, as does a jump's string while the jump is
  around one, and `Debug` prints none of them. At worst, 32 needles of 256 characters take
  about 8.6 MB (the ignored tests `migemo_cache_memory_1` and `_32` under `/usr/bin/time -l`).
- **The pattern is recorded as migemo's** (`Pattern::Migemo` in `isearch/state.rs`), so
  `Isearch::migemo` does not guess from the needle's kind, and the worker takes the compiled
  needle, never the string to compile.
- **A key's time** is counted from the moment the key starts to build what it searches for, so
  migemo's compile and step 1's search share the key's 3 ms.
- **A line too long for a regexp** (over 64 MiB) does not stop a migemo search, as it stops a
  regexp's: the search, or the count, that meets it goes on with the literal string from where
  it searched, and counts anew. Migemo is then off for that search, as after `M-m`: the
  `[migemo]` tag drops, `M-m` turns it on again and `DEL` goes back to the step before. The
  author chose this on 2026-11-13 over failing there and over a literal look in that line
  alone. Cursors from the search (`C->`, `C-<`, `C-c C-<`) take its needle as it is: once it
  went on literally, the literal string, and before, migemo's regexp, which, as step 1's
  regexp cursors, finds none past such a line.
- **A key's cost**, the highlights included, in a release build on an idle machine (the ignored
  tests in `editor/migemo_costs.rs`, where step 1's literal `zzq` took its 0.58 ms again).
  Building a needle takes 0.06 ms p50 for `k` and `kanji` and 0.4 to 0.7 ms for 256
  characters, at most 1 ms. Over 20 MB of short English lines, forward and backward, a key
  takes 0.2 ms p50 for `k`, 1.2 to 1.5 ms for `kanji`, and 1.4 to 1.8 ms for 256 characters
  (`kanjinohenkan`, `n`, `a`, `aiueo`, `ga` and `vu` repeated); over 20 MB of short Japanese
  lines, 0.3 ms, 1.3 to 1.7 ms and 1.4 to 2.0 ms. All but two keys
  took at most 2.2 ms. Typing 256 characters one at a time over the English lines and taking
  them back, forward and backward, a key takes 1.3 to 1.7 ms p50 and `DEL`, whose needle the
  cache keeps, 0.2 to 0.3 ms; that run's slowest keys, as other work loaded the machine again,
  took 3 to 20 ms. A jump's needle of its 64 characters builds in 0.2 ms p50, at most 0.4 ms,
  well within the 8 ms of a key. A jump's key takes 0.25
  to 0.55 ms p50 in one window of 120 by 400 and 0.35 to 0.9 ms in four; around a secret,
  which builds its needle each key, within 0.2 ms of that. At a line of 100 MB, a migemo
  search's key takes 0.7 ms, and the worker goes on literally to the match. The strings of the
  largest patterns (`-ja`, `-vo`, `kk`, `ss` and `zu` repeated) joined those tests after these
  timings, and were not timed on an idle machine.
- **Jumps** look for their string through the same cache, by the setting: `ka` labels `か` too.
  A jump is bounded by bytes, not time: migemo's needle, as a literal string, keeps step 5's
  byte and candidate budgets alone, so a loaded machine labels as many as an idle one, and no
  label goes missing by chance. Its pattern comes from the table, not from a person, and the
  regexp's scan is linear in the bytes the budgets bound; the densest text measured, every
  character a match in four windows, stays within a key. A person's own regexp, from
  `C-M-s`, keeps the frame's time a run that step 5 gave it. The coordinator chose this on
  2026-11-13, over one time for every window's matches, which labelled fewer on a loaded
  machine. Isearch keeps its key's time, as the worker takes on what a key leaves. A
  regexp's highlights and a jump's key read each line into room they keep, so neither
  allocates once it has grown.

#### As built (step 3, 2026-11-13)

The author chose the first option of each of eight questions on 2026-11-13, through the
coordinator; the notes below are those choices as built.

- **The API (Q1).** `oso_core::migemo` is public now, with only `Dictionary` (`new` from a
  file's text, and its counts `readings`, `words` and `skipped`) and its limits `MAX_BYTES`,
  `MAX_READING`, `MAX_WORD`, `MAX_WORDS_A_READING`, `MAX_ENTRIES`, `MAX_WORDS` and
  `MAX_PREFIX_ENTRIES`; the romaji, the forms and the pattern stay private to the crate until
  step 4. The editor gains `Editor::set_migemo_dictionary` and
  `Editor::take_migemo_dictionary_wanted`. Nothing in `guardrails/layers.txt` changes.
- **The setting.** `[search] migemo-dictionary` takes a path, absolute or `~/` and a relative
  one, of at most 4096 bytes, and not one starting with `\\` or `//`, a share on another
  computer or a Windows device; `~/` and a path from a root (`~//srv`, `~/\\srv`, `~/C:`)
  would replace the home directory, so it is refused too. Anything else is a problem the file
  reports, and the setting is left out. Parsing looks at nothing on disk. `~/` is the home
  directory when the file is read, if that is absolute: else `*Messages*` says the file was not
  read, so an empty `HOME` never names a file in the directory oso started in. The path the
  home directory makes is judged again, and one on another computer is not read either.
- **Reading it.** The editor says when a search, isearch's or a jump's, built or took migemo's
  needle with no dictionary, but not around a buffer that may hold secrets, where the table's
  coming would say what was typed; before the event loop waits, the app then sends the file to the
  `oso-migemo` thread (`app/migemo.rs`) if one is named and not read yet. The thread reads it
  with `config.toml`'s checks (ADR 0108): through links, refused if other users can change it
  or a directory or link on the way, a regular file, the one judged, at most 64 MiB, in UTF-8;
  one its group can change is read, and `*Messages*` warns of it (Q5), as a package
  manager's files may be a group's. It builds the table there and hands it over. A table read
  for an older setting is dropped. `*Messages*` says `Migemo dictionary: 137,025 readings`, or
  `Migemo dictionary not read:` and why, never the path or a word.
- **When it comes (Q6),** the cache of needles empties, the search on screen builds its needle
  again and looks from its match, as typing does, with no step of its own, so `DEL` goes back
  as before; a jump still reading its string finds its matches again, and one at its labels
  stays as it is. A setting changed to another file reads it at once if a table is in, which
  serves until the new one is, or, if the new one cannot be read, is dropped; a setting taken
  away drops the table. A table replaced is freed on the freeing thread, not the UI's. A file that could not be
  read is tried again only when `config.toml` is read again or the setting changes (Q7). A dev
  restart does not carry the table: the next search reads it again.
- **The formats (Q3, Q4).** Each line by its shape: a tab makes it `migemo-dict`'s, and
  ` /` with a final `/` SKK's. A byte-order mark, `\r\n` and `;` comments are taken; an SKK
  word's `;annotation` is cut, a Lisp word (`(` and a lowercase letter, to a `)`) and an
  okuri-ari block (`[す/愛/]`) are left out, `(株)` stays, and okurigana (`かk`) drops its
  letter in SKK's lines only; a word of only white space is left out in either format. A reading is 1 to
  64 characters of hiragana, `ー` and `ゔ`, and a word 1 to 64 bytes with no control character;
  a reading keeps at most 256 words over every line that gives it, and a dictionary at most
  2^20 readings and 2^22 words. What is left out is counted (`Dictionary::skipped`).
- **The table** is the file's text itself, the entries' and words' ranges into it, 16 bytes a
  reading and 8 a word, sorted by reading, each reading once: at most 64 MiB of text, 16 MiB
  of entries and 32 MiB of words. While it is built, the words of a reading several lines give
  are copied together, up to 32 MiB more, and then kept once. The author's `migemo-dict`
  (C/Migemo's, 5 MB) gives 137,025 readings and 190,759 words, with 26,246 lines and words
  left out, nearly all readings in ASCII, built in 14 ms.
- **The words a string's kana give (Q2, Q3).** The table is narrowed by binary search a kana
  at a time; either of two kana (`ず` or `づ`) splits a range, and more than 2048 live ranges
  give no words. An unfinished end's spellings are looked for as the last step, one that
  another starts (`きゃ` after `き`) once. While at most 2048 readings start with the kana and
  an end, their words go in; past that, those of the readings that are the kana and an end
  exactly, none for a string that is only an unfinished end (`k`). At most 4096 words and
  32 KiB of them a pattern, a letter with a case counting four times its bytes, as a search
  that ignores case compiles it as a class of every letter it folds to: about twice what the
  largest pattern the author's dictionary gives costs (the author's choice of 2026-11-13, so
  a pattern of any file's words compiles in about the time of that largest one). The exact
  readings' come first, then the shorter readings', then the table's order.
- **Compiling.** The words go into the kana's trie in order, so each is added after its node's
  last child, every character through `regex::escape`, which changes only ASCII, so the rest
  goes as it is with nothing allocated. The pattern compiles with a size limit of 4 MiB and
  the nest limit of 32; past either, the pattern drops its words and compiles the kana alone
  within step 2's 256 KiB (`words_past_their_size_limit_are_dropped_for_the_kana`). One word
  that nests too deep drops all of them, which no common dictionary does. So the kana's
  largest patterns keep their
  bound (`the_largest_migemo_patterns_compile_on_a_key`), and a dictionary at every limit still
  gives a needle on a key (`the_largest_migemo_patterns_with_words_give_a_needle_on_a_key`).
- **Q7's 2 ms,** measured by the ignored tests in `editor/migemo_dictionary_costs.rs` over the
  author's dictionary on their Mac, in a release build on an idle machine: every prefix of 174
  common words and every string of one to three letters that gives kana, 5,619 strings built
  three times each, uncached. Building and compiling a needle takes 0.071 ms p50, 1.35 ms p99
  and at most 2.3 ms; 19 of 16,857 builds pass 2 ms, each of a pattern of about 2,700 words.
  The largest pattern, of 2,972 words, is 17 KB, and none drops its words, nor loses any to
  the 32 KiB, which the heaviest pattern fills to 80%. Looking up the
  words takes at most 0.1 ms and writing the pattern 0.3 ms; the rest is the compile. So the
  build stays on the key.
- **A key's cost.** An isearch key over 20 MB of short Japanese lines, its highlights with it,
  takes 0.99 ms p50 and 3.1 ms p99: a string with no match near searches for step 1's 3 ms,
  counted from the needle's build, before the worker goes on, and the slowest keys took up to
  6.2 ms, as a large pattern's first look in a line can pass the time before it is looked at
  again. A jump's key, over text made of the words its string finds so nearly every place
  matches, takes at most 3.5 ms in four windows of 120 by 400, so a jump keeps step 5's
  budgets of bytes and candidates and no time of its own. `DEL` takes its needle from the
  cache. The 32 needles of the most words take about 14 MB more than one.

#### As built (step 4, 2026-11-13)

The author chose the first option of each of six questions on 2026-11-13, through the
coordinator, and of two more the reviews raised (Q7, Q8); the notes below are those choices as
built.

- **The API.** No public signature changes. `LineQuery::parse` reads a romaji component as
  migemo's, with no dictionary: its kana. The editor parses with `LineQuery::parse_after`
  (private to the crate), which takes the query before, the setting and the dictionary. Both
  sit below `search`, so `oso_core::migemo::compile` builds the pattern with words within
  4 MiB, or the kana alone within 256 KiB. Each caller compiles with its own builder, of bytes
  for a project's lines and of text for completion; isearch's needle uses the same function.
  The romaji, the forms and the pattern stay private to the crate.
- **A project's search (Q2, Q6).** After `!` or `=`: `=` gives a literal. Romaji by isearch's
  rule (lowercase ASCII letters, `'` and `-`, at most 256 characters) gives migemo's pattern,
  which holds the literal too. A component with a character an expression reads is a literal
  and that expression, as before. Romaji has no such character. A component whose letters
  spell no kana (`struct`) is a literal. `!kan` leaves out the lines holding `かん` and
  `カン` too. Rows match line text, never paths. Every `oso-grep` thread shares the compiled
  query.
- **The setting (Q1).** With `[search] migemo = false`, a project's search reads romaji as a
  literal, as isearch does. Completion's `%` is asked for explicitly, so it applies either way.
- **Kept components.** A project's search now keeps what a component compiled while the input
  before had it with the same affix, text and case, as completion does (ADR 0076). A migemo
  component is kept only for the dictionary it was compiled with, which it holds. So a key
  compiles only the component it changed, and a paste compiles each new component once (Q3).
  A case change anywhere in the input compiles every component again. A migemo component is
  kept only while the input's budget (Q7) gives it the same words.
- **One budget of words an input (Q7).** Every migemo component of one input, in a project's
  search and in completion, shares one pattern's caps, 4096 words and 32 KiB by
  `word_cost`, taken in the input's order. A component past them compiles its kana alone. So
  one event compiles at most one pattern's words on the UI thread, whatever it is: a paste, an
  upper-case key that makes case count and compiles every component again, or the dictionary
  coming. A third or later component whose readings give many words may lose its kanji: with
  the author's dictionary, two of the heaviest prefixes spend most of the budget. A component
  keeps what it spent and, if the budget left a word out, what that word would have made it;
  it is kept only while the budget it is given now gives the same words, so a kept component
  matches as one compiled afresh. Otherwise it compiles again.
- **Narrowing (Q8).** A project's search narrows when each of the old query's components
  matches every line the new one's does. Where either is migemo's, the old one must be a
  literal or migemo's pattern, which match a line holding one of their strings, and it must
  match each string the new one's pattern matches: the string, its full-width letters, its
  hiragana, katakana and half-width forms, each with either of two kana (`ず` or `づ`) and each
  unfinished end, and its words. A migemo component keeps them, an `Arc`'d list built when it
  compiles, while they are at most 1024 besides the words; past that it never narrows, and
  the search starts again. A literal one is its text. So typing `n`, `na`, `nam`, `name`
  narrows on each key, and so do `kan` then `kanj` with a dictionary, but not `ji` then
  `kanji`, whose `漢字` holds no `じ`. Case may stop being ignored, never start, and an
  exclusion or an expression narrows only as it is. A proptest checks that a query that
  narrows matches no line the one before did not, with and without a dictionary. Completion
  still narrows only on the same component.
- **Completion's `%` (Q4).** `%` at a component's start or end, as every affix, makes romaji
  migemo's: `%kan` matches `かんじ.md`, `カンジ`, `ｶﾝｼﾞ` and `kanji`, and `漢字.md` with a
  dictionary. After `%`, anything that is not romaji, or whose letters spell no kana, is a
  literal, as after `=`: `%Kan`, `%a.b`. As at an end, `100%` is the literal `100`. Without
  `%`, a component matches as before. A migemo component highlights where it matched. It
  narrows only when it is the same, with the same dictionary and words.
- **Decomposed kana (Q5).** A candidate whose text holds U+3099 or U+309A, as macOS names
  files, keeps a second copy with each kana and its mark composed (`か` and U+3099 are `が`).
  The copy is built when the candidate is made, by a table of oso's own of the 58 pairs
  Unicode composes, and a migemo component matches it. Where it matched maps back to the
  original, so the mark is highlighted with its kana. A mark that composes with nothing stays
  as it is. A candidate with no mark keeps no copy. A project's lines and isearch still match
  precomposed kana only, and the half-width marks are as they were. Both stay in
  `open-questions.md`.
- **The dictionary.** A project's search or a completion that compiled a migemo component
  with no dictionary says so, as isearch does. Around a buffer that may hold secrets, or a
  prompt opened around one, it does not. When the table comes or goes, a project's search
  with a migemo component starts again (a new generation, never narrowing), and a completion
  with one matches its input again: on the UI thread, or as a job for the worker past 5000
  candidates. The worker's jobs (`Build`, `Matching`, `Merge`) carry the table. A job that ran
  with the table before it is superseded by the new match.
- **Secrets.** No string or pattern reaches a log. `Debug` gives counts only, and `get-state`
  is as it was.
- **A key's cost (Q3),** with the author's dictionary in a release build, load about 2–3 (the
  ignored `editor/migemo_prompt_costs.rs`): every prefix of step 3's 174 common words,
  typed three times. A project's search key takes 0.14 ms p50, 2.1 ms p99 and 4.2 ms at most, and a key
  over 5000 candidates with `%` 0.28 ms, 2.3 ms and 2.5 ms. A paste of 3, 8 or 32 of the words
  takes about 2.0, 3.4 and 8.0 ms at p99 in either. The key after a paste takes 0.11 ms p99 in
  a project's search and 0.33 ms over 5000 candidates. In a first probe over 109 of those prefixes, one
  component compiled in 0.15 to 0.18 ms p50, 1.5 to 1.7 ms p99 and at most about 3.4 ms. A migemo component's search through
  20 MB of mixed code and Japanese on an `oso-grep` thread takes 1.3 to 11 ms, against 1 to
  2 ms for a literal.

#### As built (step 5, 2026-11-12)

The author answered seven questions on 2026-11-12, through the coordinator. Two depart from the
text above, which now says what was chosen: one `C-g` ends a jump first (Q2), and a jump reads
at most 1 MiB in all (Q5).

- **The API,** as the snapshots show: `oso_core::jump` with `Kind`, `Stage`, `Jump`, `Keys` and
  `KeysError`, `Labels` and the limits `MIN_KEYS`, `MAX_KEYS`, `MAX_LABELLED` (4096) and
  `MAX_STRING` (64); in `oso_core::editor`, `JumpCells`, `LabelCells`, `Chose`, the
  `Editor::jump*` methods (`Editor::jump_refuse` among them) and `Quit::Jump`; in `oso-render`, `Face::JumpLabel`,
  `Face::JumpLabelNext` and `TextView::labels`. `guardrails/layers.txt`'s `minibuffer` line
  became `minibuffer jump`.
- **`C-g` (Q2)** ends a jump before anything else in `Editor::quit`, so it never takes the
  region away while a jump is on.
- **A key the jump's keymap does not bind (Q3)** ends a jump that is still reading its string
  silently, and does not run. At the labels it says `No such candidate`, as above, and so do
  `DEL` and `RET` there (`Editor::jump_refuse`). The keymap's own commands are
  `avy-printing-char`, `avy-delete-char` and `avy-show-labels` (`RET`) (Q4). `run-command` of
  any other command ends a jump first, and a prompt opening ends it at once. The jump's keymap
  comes before every other but a question's, and a digit at `avy-goto-line`'s labels (below) is
  looked at before any keymap.
- **The budget (Q5):** 256 KiB of text a window and 1 MiB a jump in all, the selected window
  first, then the tree's order. At most 4096 candidates are labelled; past that the farthest are
  not.
- **Nearest (Q6)** is the distance on screen: view rows from the window's primary cursor, then
  columns. On the row the 4096th falls in, the text's order picks, as no column is known yet.
- **A change to the text (Q7)** under the labels ends the jump with `Jump ended: the text
  changed`, and a change to a view, such as a wheel's scroll, with `Jump ended: the view
  changed`. As the person did not end it, the labels go, and the next key, meant for one, types
  nothing and says it again. While the string is still read, its matches are found again. `RET`, or the timer,
  labels what the last key found unless something changed since.
- **The echo row** says `Jump: <string>`, `Jump to a line`, `Jump to a line's end` or `Jump to
  a match` (`avy-isearch`), and past the limit adds `  [4096 of N labelled]`. It is withheld from
  the socket around a buffer that may hold secrets, as isearch's is.
- **The timer** starts at the string's first character, and each key while the string is read
  starts it again. A jump with no string waits, so neither `RET` nor the timer does anything
  until a character is typed. A keyboard macro being recorded keeps the timer's showing the
  labels as the `RET` that does the same, so a replay shows them where they showed at any
  speed; the timer never fires during a replay.
- **Candidates:** a line's start is labelled on a row that starts its logical line with column 0
  in view, so a view scrolled sideways shows none; a line's end on the line's last row, when its
  end column is in view; and a match whose start is within its row's columns in view. A wrapped
  line's rows in view are matched as one, as isearch's highlights are, so no two matches
  overlap.
- **On screen,** a label is cut where its row's text stops, before a wrapped row's mark, and at
  the next label on its row. A target with
  no cell, such as an empty line's end, is labelled in the first cell after the text. Labels are
  drawn over everything in their cells: the renderer skips the glyphs under a label, and draws
  the labels after all of their layer, the cursor box and the text of other rows and panes
  included, in a layer of their own under the floats. A label's keys stay in its cells, and a
  label whose fill finds no room under the frame's caps draws no key. The text-grid snapshots
  show a label's keys on a line between `@`.
- **`avy-isearch`** labels the selected window's matches only. The cursor goes to the match's
  start, then the search ends as `RET` ends it.
- **The region and the mark:** with a region shown, nothing is pushed and the jump extends the
  region; otherwise the place the cursor left is pushed onto the mark ring without a message.
- **Windows:** a jump into another window selects it, and its flash comes from the dispatcher's
  sample, as for any key that moves far.
- **`M-g f`** with a digit at the labels, before any of a label is typed and when no label
  takes it, opens `goto-line` with the digit typed (`Jump::gives_goto_line`).
- **Settings:** `[jump] keys` is checked by `Keys::new`, and `[jump] timeout` is 0.1 to 5 s,
  0.5 by default.
- **Around a secret:** `get-state`'s `jump` is `{"active": true}`; elsewhere it gives `active`,
  `stage` and `count`. The input method is off while a jump is on, and the agent channel, its
  question about a connection and a dev restart wait for it to end. `avy-isearch` is around a
  secret when the search was, and a buffer that may hold secrets shown during a jump marks it.
- **Left for later,** as a chore: `[jump] timeout`'s default moves into the configuration, so
  that its value in force needs no exception, and an `Editor::follow_text` keeps every session
  true to its text in place of jumps going through `isearch_follow_text`.
- **Done in that chore (2026-11-13):** the jump's keys and timeout, 0.5 s where the file says
  nothing, are values in force as every other setting is, and `Editor::follow_text` keeps isearch
  and a jump true to their text before the host waits, in place of `isearch_follow_text`. A
  jump's byte budgets count what a regexp reads around each row too, not the row alone.

## Changes to accepted ADRs

- **[ADR 0082](0082-isearch.md):** "`C-s` and `C-r` search for the literal string" becomes: for the
  literal string, unless it is romaji and migemo is on, when they search for its migemo pattern
  (one line at a time, as 0113's regexps). "The IME is off while isearch is on … Migemo (M3)
  searches Japanese from romaji" holds as it is.
- **[ADR 0076](0076-completion-and-m-x.md):** "`%` (char folding) is left out until migemo (M3)
  decides how Japanese is matched" becomes: `%` makes a component migemo's. Char folding gets no
  affix.
- **[ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md):** a component that is
  romaji matches as its migemo pattern; `=` keeps it literal.
- **[ADR 0085](0085-agent-channel-and-oso-mcp.md):** the channel also waits while a jump is on.
- **[ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md):** the `C-g` chain gains a jump, ended before the
  region (`Quit::Jump`).
- **[`open-questions.md`](../open-questions.md):** the migemo dictionary item, and avy in "UI of
  each later feature", are settled here and leave the list.
- **`guardrails/layers.txt`:** `migemo` on `oso-core`'s lowest line, and `jump` beside
  `minibuffer`.

## Consequences

- Without a dictionary, romaji finds kana and not kanji. A person who has C/Migemo's dictionary
  names it in one line of `config.toml`; nothing GPL is in oso's repository or binary.
- An isearch for a lowercase string now matches more: `ka` also finds `か` and `カ`, in code with
  Japanese comments too. `M-m` takes it back to the literal string for that search, and the
  setting for every search.
- A romaji isearch is a regexp search one line at a time, so 0113's limits hold for it. At a
  line over 64 MiB, where a regexp is not matched, the search goes on with the literal string
  (the author's choice of 2026-11-13, in step 2's notes): kana in that line are not found, the
  romaji is.
- A Japanese word that a hard wrap breaks across two lines is not found (Q6).
- A dictionary costs about its file's size in memory once a search used it, and a read on a
  worker the first time; nothing at start-up.
- While labels show, they hide the first cells of each target. They show only during a jump, in a
  face no text has.
- Jumps and migemo add four settings, two faces, two `oso-core` modules and no dependency.

## Alternatives considered

- **Running C/Migemo as a process, as migemo.el does:** its dictionary and its patterns as they
  are, but a process at each search, a program to install and find, spawning in release builds
  for a search, and Emacs regexp syntax to translate for the `regex` crate.
- **Bundling `migemo-dict` or an SKK dictionary:** GPL data, which ADR 0020 rules out.
- **rustmigemo's code:** MIT, but not published as a crate, so it would be vendored; its
  dictionary format holds what a sorted table of oso's own holds, and the matcher is the part oso
  must fit to its own limits anyway.
- **Downloading a dictionary, as kensaku.vim does:** oso has no network code.
- **Another regexp engine,** for patterns that match across lines or backtrack: only the `regex`
  crate is allowed.
- **Kanji from per-character readings (Unihan's `kJapaneseOn` and `kJapaneseKun`):** permissive
  and small, but a syllable stands for hundreds of kanji, so `kan` matches nearly every kanji text
  and compounds cannot be told apart.
- **Segments by uppercase letters, as C/Migemo:** finds compounds the dictionary lacks
  (`kikiKaikan`), but takes uppercase away from smart case; Q5 (c) keeps it open.
- **Labels while typing, as flash.nvim (Q13 (b)):** no timer, but labels that change at each key.
- **Labelling windows to select one, as ace-window:** a later command over the same labels.
- **A jump that keeps every cursor and moves each one:** no editor does this, and there is one
  target.
