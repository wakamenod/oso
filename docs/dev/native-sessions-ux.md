# Native sessions: UI and UX

Proposals for the native session view of [ADR 0165](adr/0165-native-claude-sessions-the-route.md),
written on 2026-11-21 at the author's request: the view should be more modern than the author's
Emacs client (ECC), and the author asked where the room for improvement is. A companion to ADR
0165: the author's answers are under "Decisions", and each part moves into [`ui.md`](ui.md) as
its step is built. A proposal no question covers is still a proposal, settled in its step.

The base design is ADR 0130's, which ADR 0165 keeps by reference: a read-only session buffer with
a composer window of 1 to 10 rows under it, one row per tool call that `TAB` unfolds, edit diffs in
ADR 0122's faces, thinking folded to one row, a checklist while a turn runs, keyboard-only cards
(`y`, `n`, `N`, no "always allow"), `C-c C-c` to send, and `ctx N%` in the modeline. This document
proposes what to add to it, change in it, or do first.

## Decisions

The author chose (a) on each question of section 6 on 2026-11-21, through the space's
orchestrator:

- **The permission card** is docked between the transcript and the composer, and goes once
  answered; the tool row says what came of it (proposal 8, step 2).
- **The status** is the session window's modeline: model, mode, `ctx N%`, cost and the rate
  limit's window, each once (proposal 13, step 3).
- **The to-do list** is a strip pinned above the composer, one line, opened to at most 6 rows
  (proposal 14, step 3).
- **Edits:** no changed-files list in the session; `v` on an edit row opens ADR 0122's review at
  that hunk, where keeping and undoing happen (proposal 9, step 2).
- **Slash commands:** the minimal set of section 5. Everything is sent as text; completion leaves
  out the terminal's commands; `/model` and `/effort` complete their argument; oso answers only
  `/btw`, in step 5.
- **Tool rows:** consecutive reads and searches are grouped into one row (proposal 1, step 1).
- **Rewind and fork:** fork from a message in step 5; files are not rewound, and `rewind_files`
  stays outside the control requests.
- **Images** are chips; thumbnails wait for an ADR on drawing images in `oso-render`.

## 1. Where ECC is dated or clumsy

Read from ECC 0.5.1's code, its own UI redesign notes (`docs/phase9-ui-redesign.md`) and its
screenshot (`docs/images/session.png`). ECC already copies much of the TUI's look (tool rows with a
status mark, an icon and a summary; thinking folded; Markdown markers hidden), so the gaps are in
layout and flow more than in single rows.

- **One buffer for everything.** Transcript, live turn, `Files` and `Tasks` summaries, recap, a
  rule, the prompt and a mode footer share one buffer. Scrolling up to read takes the prompt off
  the screen; keys differ by region (one-letter keys only in the transcript, `C-c C-a`/`C-c C-d`
  from the prompt), so the same answer has two key sets.
- **Status in three places, some unlabeled.** The header line says `▶ running  greet  88%`, the
  footer `manual mode (S-TAB to cycle)` and `haiku`, the mode line `Claude [▶ running]`: the state
  twice, and `88%` with no word for what it measures.
- **Answered permissions stay as big blocks.** An allowed Edit leaves `✓ Permission: Edit allowed`
  with the whole diff, and the tool row has the same diff: the change shows twice.
- **Folding looks like Emacs.** Invisible overlays end in `...` (`Files (1)...`), and paths are cut
  from the right (`/tmp/greet/hello.py...`), so the file name, the part that matters, is what goes.
  Paths are absolute, not relative to the project.
- **Progress hides at the bottom.** `Files` and `Tasks` sit folded after the transcript, one line
  each; the to-do list is not in view while the work runs.
- **Pop-ups apart from the session.** Usage and `/btw` open in a posframe or another window, so
  they cover code and leave no trace in the session.
- **Movement by timers.** A pulsing background under the running tool, a blinking line where an
  answer is wanted, flashes and a spinner are each a timer over an overlay, capped because they cost
  Emacs's single thread.
- **Review apart from the transcript.** An edit's diff in the transcript has no way to the same
  hunk in the review (ediff), and no keep or undo of its own.
- **Discovery through a menu.** The placeholder says `Ask Claude… (C-c ? for the menu)`; the keys
  a row takes are not shown where the row is.

## 2. What current agent UIs do

From a survey of official docs and changelogs on 2026-11-21 (sources at the end; one forum post
and one mirror are marked as not official). The ideas that recur:

| Idea | Where |
|---|---|
| Tool calls folded to one summary line, opened on demand; a "focus" view that hides them | Claude Code (`Ctrl+O`, `/focus`), its VS Code extension (Focus view), Cursor (compact mode) |
| One review surface for all changes, keep or undo per hunk | Zed (multibuffer), Copilot (multi-diff), Codex app (stage or revert per hunk), Claude Code (`/diff` panel), Warp |
| Line comments on a diff sent back as feedback | Claude Desktop, Copilot, Codex app |
| Fork or rewind from an earlier prompt: code, conversation or both | Claude Code (`/rewind`, `/branch`), VS Code extension, Copilot, Cursor, Windsurf, Codex |
| Permission cards that show the tool, its input and its target; scopes once, session, always | Copilot, ACP, Claude Code, Warp, Cursor (a path and a reason) |
| A plan as an editable document with explicit hand-off choices | Claude Code (`Ctrl+G`), Cursor (Build), VS Code extension (inline comments), Copilot |
| A to-do checklist pinned outside the transcript | Claude Code (`Ctrl+T`, 5 items), Copilot (top of the view), Cursor, Windsurf, ACP plans |
| Sessions grouped as Needs input, Working, Completed; peek and reply | Claude Code agent view, VS Code extension, Cursor Agents Window, Claude Desktop |
| Sub-agents as nested threads that can be opened | Claude Code (`/tasks`), Copilot, Codex (`/subagents`), ACP |
| Queued messages, shown grey, taken back or sent now | Claude Code, Cursor, Zed, Windsurf |
| A side question out of the history (`/btw`) | Claude Code, Cursor, Codex (`/side`) |
| A context meter always in view, with a breakdown on demand | Claude Code (status line, `/context`), Copilot, Claude Desktop, Codex, Zed |
| Pager keys over the transcript: between prompts, search, open in an editor | Claude Code (`{` `}`, `/`, `n`, `v`), Zed (between prompts) |
| While scrolled up, the turn's prompt stays at the top; "Jump to bottom · 3 new messages" | Claude Code fullscreen |
| A one-line recap when the person comes back | Claude Code |
| Thinking shown as `Thought for 12s`, opened on demand | Claude Code VS Code extension, claude.ai |
| A permission waiting shows as a dot on the session's tab | VS Code extension |

Two things none of them can do and oso can: the transcript is a buffer, so isearch, `M-s l`,
the mark, keyboard macros and copying work on it with no extra code; and the review, the
sessions pane and the editor are one program, so a row can open the exact hunk or file.

## 3. Proposals

Value: H high, M medium, L low. Effort for oso: S days, M a week or two, L more. Step: of ADR
0165 (1 the stream, 2 composer and cards, 3 modes, model and usage, 4 hand-off and restart, 5
the rest).

| # | Proposal | Value | Effort | Step |
|---|---|---|---|---|
| 1 | **Tool rows as one line:** a status mark, the tool, its target relative to the project, and what came of it (`+4 −2`, `3 lines`, `error`, `denied`). Consecutive reads and searches grouped (`Read 3 files`), unfolded with `TAB`. | H | S | 1 |
| 2 | **Thinking as one row,** `∴ Thought for 4 s`. The CLI sends no thinking text by default, so the row has nothing to unfold unless it does. | M | S | 1 |
| 3 | **A compaction marker:** a full-width rule, `── compacted · 20.2k → 3.9k tokens ──`. | M | S | 1 |
| 4 | **A turn footer,** dim, right-aligned: `12.3 s · $0.21`, or `✗ interrupted`. | M | S | 1 |
| 5 | **Sub-agent progress on its row:** the Agent row shows the subagent's description and its live progress line; its own calls are folded one level under it. | M | S | 1 |
| 6 | **Buffer keys:** `n`/`p` between prompts, `]`/`[` between tool rows, `TAB` folds, `RET` opens a result whole, `w` copies the row; isearch and `M-s l` as in any buffer. The window follows the end unless the person scrolled up; then the modeline says `↓ 3 new` and `M->` goes back. | H | S | 1 |
| 7 | **Streaming Markdown with highlighting** (decided: a follow-up inside step 1). Built in step 2 (ADR 0165 part 2g): a reply is highlighted once whole, plain while it streams. | H | M | 2 |
| 8 | **A permission card docked above the composer,** not in the transcript: the tool, its target, a preview (the command, or the edit as a diff in ADR 0122's faces), the reason (Bash's `description`, else the reply's last line), and the keys as chips (ADR 0075's look). Answered, it goes, and the tool row's mark says `allowed` or `denied: NOTE`. Never the same diff twice. | H | M | 2 |
| 9 | **Edits lead to the review:** `v` on an edit row opens ADR 0122's review at that hunk; keeping and undoing happen there, on one surface. No changed-files list in the session (the author never looked at ECC's). | H | S–M | 2 |
| 10 | **Queued messages** grey above the composer, `M-p` takes the last back. Built differently (ADR 0165 part 2a): grey at the transcript's end, `RET` on a queued row takes it back, `M-p` walks the messages sent. | M | S | 2 |
| 11 | **A focus toggle** (`C-c C-f`): only prompts, replies and turn footers; tool rows hidden. | M | S | 2 |
| 12 | **The composer's `@`** completes files with the preview float of `C-x p f`, and puts a chip (`@auth/redirect.rs`); a pasted image is a chip, `[image 1 · 1280×720]`. | M | M | 2 |
| 13 | **Status in the session window's modeline:** model · mode · `ctx 41%` · cost · `5h 63%`, each a word or a unit, once. A narrow window drops from the right. | H | S | 3 |
| 14 | **A pinned to-do strip** between the transcript and the composer: one line (`☐ 2/5 ▶ Run the tests`) while a list exists, `C-c C-t` opens it to at most 6 rows. | H | M | 3 |
| 15 | **Plan review** as ADR 0130 has it, in an editable buffer, with `y` approve, `n` keep planning with a note. | H | M | 3 |
| 16 | **A recap** when the person comes back to a session they had not seen finish: the turn's last reply's first sentence in the echo area. | M | S | 3 |
| 17 | **`/btw` in a top-edge float** (ADR 0075), streaming, out of the history; `C-g` closes it. | M | M | 5 |
| 18 | **The turn's prompt pinned at the top** while the person reads inside a long turn. Needs a sticky row in the renderer. | M | M | 5 |
| 19 | **Fork from a message** (`f` on a prompt): a new session from that point. Rewinding files needs `rewind_files`, a sixth control request (Q7). | M | M | 5 |
| 20 | **Image thumbnails.** oso's GUI cannot draw images today: `oso-render` has quads and glyphs only, and the terminal's Kitty images wait for ADR 0120's step 12. Thumbnails need textures in the renderer and a decoder: a dependency and an ADR. | L | L | later |
| 21 | **Motion kept small:** a spinner in the running tool's mark only; no pulsing backgrounds or blinking lines. Attention stays ADR 0121's (`*Spaces*`' blink, the modeline mark). | M | S | 1 |

Have already, reused: notifications and attention (ADR 0121), the sessions pane with Needs input
and Working (ADR 0136, 0140), searching past conversations (ADR 0145), session switching.

## 4. Mockups

The conversation, at 80 columns, with the sessions pane on the right left out. Illustrative:
what a row says comes only from what the stream carries.

```text
› Fix the login redirect after OAuth: it follows ?next= to other hosts.

● I'll look at the redirect handler first.
  ▸ Read 3 files · auth/redirect.rs, auth/session.rs, auth/mod.rs
  ▸ Grep "next" · 12 matches in 4 files
  ✎ Edit auth/redirect.rs  +4 −2                                    v review
  ✓ Bash cargo test -p auth
  ▸ Agent Explore · "find other redirect sinks" · 6 tools · done
● The redirect now follows only same-origin paths. `safe_next` rejects an
  absolute URL, a scheme-relative `//host` and a backslash path.
                                                            41.2 s · $0.21

──────────────────────── compacted · 182k → 31k tokens ────────────────────────

› also add a test for an absolute URL
  ◌ Edit auth/redirect_tests.rs  …
```

A tool row unfolded with `TAB`:

```text
  ✎ Edit auth/redirect.rs  +4 −2                                    v review
    @@ -41,6 +41,8 @@
     pub fn redirect(req: &Request) -> Response {
    -    let target = req.query("next");
    -    Response::found(target)
    +    let target = req.query("next").and_then(safe_next);
    +    Response::found(target.unwrap_or("/"))
     }
```

A permission card, docked between the transcript and the composer:

```text
  ◌ Bash git push origin fix-login
╭──────────────────────────────────────────────────────────────────────────────╮
│ Bash wants to run                                        needs input · 0:12  │
│   $ git push origin fix-login                                                │
│   in webapp/.claude/worktrees/fix-login                                      │
│   Why: Push the branch so that the pull request can be opened.               │
│                                                                              │
│   [y] allow once   [n] deny   [N] deny with a note   [RET] the whole input   │
╰──────────────────────────────────────────────────────────────────────────────╯
› █
```

The status line, as the session window's modeline:

```text
 *claude: fix-login*  Session  ● working   Opus 5.5 · auto · ctx 41% · $0.82 · 5h 63%
```

The to-do strip, folded and opened:

```text
 ☐ 2/5 ▶ Run the auth tests                                      C-c C-t list
```

```text
 ☑ Read the redirect handler
 ☑ Patch safe_next
 ▶ Run the auth tests
 ☐ Add a test for absolute URLs
 ☐ Update the changelog
```

## 5. Slash commands

ECC answers eight commands itself and asks for the argument of five; the CLI runs the rest when
they are sent as text. What the author likely needs is marked.

| Command | In ECC | Likely needed | Proposed for oso |
|---|---|---|---|
| `/btw` | itself (`side_question`), in a float | yes | itself, tier 2 (step 5); see proposal 17 |
| `/resume` | itself: a picker | no | none: `session-resume` and `session-search` (ADR 0145) do it |
| `/login`, `/logout`, `/auth-status` | itself (`claude auth`, a terminal) | rarely | none; a failed turn that says the CLI is signed out says to run `claude auth login` in a terminal |
| `/hooks` | itself | no | none |
| `/plugins`, `/plugin` | itself (`claude plugin … --json`) | no | none |
| `/skills`, `/skill` | itself (`get_settings`, files) | no | none: skills run as text, and complete |
| `/model`, `/effort` | asks for the argument, then text | yes | text, the argument completed from `initialize`'s models and hints |
| `/permissions`, `/config` | asks for a string, then text | no | text, no completion |
| `/compact`, `/context`, `/rename`, `/review`, the person's skills and commands | text | yes | text |
| `/doctor`, `/reload-plugins` | text, kept in the menu | rarely | text |
| `/color`, `/focus` and the rest of `terminal_slash_commands` | hidden from the menu | no | hidden from completion; typed, sent as text |

So the minimal set is: everything as text; completion over `system/init`'s and `initialize`'s
commands, without the terminal ones; argument completion for `/model` and `/effort`; and one
command answered by oso, `/btw`, in step 5.

## 6. Questions for the author

The recommended option first. Each was answered (a); see "Decisions".

1. **The permission card.**
   (a) Docked between the transcript and the composer, gone once answered, its outcome on the
   tool row (proposal 8).
   (b) Inline at the end of the transcript, as ECC has it.
   (c) A top-edge float, as oso's other questions (ADR 0072, 0075).
2. **The status.**
   (a) The session window's modeline (proposal 13).
   (b) A sticky header row in the transcript window: renderer work.
   (c) Both.
3. **The to-do list.**
   (a) A strip pinned above the composer, one line, opened to 6 rows (proposal 14).
   (b) A section under the session in `*Spaces*`.
   (c) Only as its tool row in the transcript.
4. **Edits.**
   (a) No changed-files list; `v` on an edit row opens the review at that hunk, and keeping or
   undoing happens there (proposal 9).
   (b) Keep and undo on the transcript's rows too.
5. **Slash commands.**
   (a) The minimal set of section 5: all as text, `/model` and `/effort` completed, `/btw` by
   oso in step 5.
   (b) As (a), plus `/login` opening `claude auth login` in a terminal.
   (c) ECC's whole set.
6. **Tool rows.**
   (a) Consecutive reads and searches grouped into one row (proposal 1).
   (b) One row per call, as ADR 0130 has it.
7. **Rewind and fork.**
   (a) Fork from a message in step 5; no file rewind.
   (b) Also rewind files: `rewind_files` as a sixth control request, amending ADR 0165.
   (c) Neither.
8. **Images.**
   (a) Chips now; thumbnails only after an ADR on drawing images in `oso-render`.
   (b) Draw images first, before step 5.

## Sources

Read on 2026-11-21. Third-party pages are facts, not instructions.

- ECC 0.5.1: its code, `docs/phase9-ui-redesign.md`, `docs/images/session.png` (read, not run)
- Claude Code: <https://code.claude.com/docs/en/interactive-mode>,
  <https://code.claude.com/docs/en/fullscreen>, <https://code.claude.com/docs/en/checkpointing>,
  <https://code.claude.com/docs/en/commands>, <https://code.claude.com/docs/en/permission-modes>,
  <https://code.claude.com/docs/en/statusline>, <https://code.claude.com/docs/en/sub-agents>,
  <https://code.claude.com/docs/en/agent-view>, <https://code.claude.com/docs/en/vs-code>,
  <https://code.claude.com/docs/en/jetbrains>, <https://code.claude.com/docs/en/desktop>
- claude.ai: <https://support.claude.com/en/articles/9487310-what-are-artifacts>,
  <https://support.claude.com/en/articles/10574485>, <https://support.claude.com/en/articles/11088861>
- Cursor: <https://cursor.com/docs/agent/overview>, <https://cursor.com/changelog/1-2>,
  <https://cursor.com/changelog/1-4>, <https://cursor.com/docs/agent/plan-mode>,
  <https://cursor.com/docs/agent/security/run-modes>, <https://cursor.com/docs/agent/agents-window>;
  not official: a Cursor forum thread on Keep and Undo in the Agents Window
- Zed: <https://zed.dev/docs/ai/agent-panel>, <https://agentclientprotocol.com/protocol/tool-calls>,
  <https://agentclientprotocol.com/protocol/agent-plan>
- Codex: <https://learn.chatgpt.com/docs/codex/cli>, <https://learn.chatgpt.com/docs/cli/slash-commands>,
  <https://learn.chatgpt.com/docs/agent-approvals-security>,
  <https://developers.openai.com/codex/app/review>, <https://learn.chatgpt.com/docs/ide>; not
  official: a mirror of its getting-started page for Esc-Esc backtrack
- Copilot in VS Code: <https://code.visualstudio.com/docs/agents/run/review-code-edits>,
  <https://code.visualstudio.com/docs/agents/run/approvals>,
  <https://code.visualstudio.com/docs/agents/run/sessions/manage-sessions>,
  <https://code.visualstudio.com/docs/agents/run/planning>,
  <https://code.visualstudio.com/updates/v1_103>, <https://code.visualstudio.com/updates/v1_110>
- Windsurf: <https://docs.devin.ai/desktop/cascade/cascade>
- Warp: <https://docs.warp.dev/agents/capabilities/agent-profiles-permissions/>,
  <https://docs.warp.dev/agents/local-agents/code-diffs/>; Aider:
  <https://aider.chat/docs/usage/commands.html>
