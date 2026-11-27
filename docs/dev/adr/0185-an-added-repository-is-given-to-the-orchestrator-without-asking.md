# 0185. An added repository is given to the orchestrator without asking

- Status: Accepted
- Date: 2026-11-27

## Context

A space's running orchestrator cannot read a repository added to its space, or trusted, after
its start until `/add-dir` runs in it (ADR 0136, step 3, Q5). Until now, oso asked the person
"Paste /add-dir PATH into orchestrator NAME's prompt, not sent? (y or n)", and at `y` pasted the
command without submitting it (ADR 0161, Q6). A native orchestrator, which has no `/add-dir` in
`claude -p`, was asked "Resume orchestrator NAME with REPO added, once its turn ends? (y or n)"
instead (ADR 0176, part O-2). Each repository that `space-add-repo` or an orchestrator's
`space_add_repo` added, or that the person trusted later, cost the person a `y` and, for a
terminal, an Enter in the orchestrator as well.

On 2026-11-27 the author asked for this to go with no question: "私への確認も不要なので確認なしで
自動送信して" ("no confirmation from me either: send it automatically without asking").

## Decision

The author chose this through the coordinator on 2026-11-27: a terminal orchestrator gets
`/add-dir PATH` typed and submitted by oso; a native one is resumed with `--add-dir` once its
turn ends, as its `y` did. Nothing is asked.

### What still holds

- **Trust first.** Only a repository that is a member of the space and that oso trusts is given.
  A restricted one gets nothing until the person trusts it with `trust-project`. Then it goes
  the same way, with no question.
- **The path is the recorded one.** The command is `sessions::send::Command`, which only
  `Command::add_dir` makes, from the space's recorded path while that is one plain line: an
  absolute path in UTF-8 with no control or invisible character, no blank but a space, none at
  its end, no `@` after one, and the whole within 512 bytes. No text an agent gave goes this
  way. An agent's `space_add_repo` names a path, but what is recorded is the real path of the
  main checkout found by the look (ADR 0161).
- **Once given, not given again.** A repository given at the start, or typed since, is not
  queued again, and the same repository is queued only once per session.

### A terminal orchestrator

- **Queued, then typed once it is idle.** Each `/add-dir` waits, in the order it came, one typed
  at a time, until all of these hold:
  - the session is still the space's running orchestrator, and the repository is still one of
    the space's that oso trusts;
  - its terminal takes a paste (`paste_target`): it was judged since it started, shows no
    dialog or permission prompt, no full-screen view and no password prompt, and bracketed
    paste is on;
  - its turn has ended (idle, done or ready for review);
  - its input box is empty (`input_box`);
  - the person has typed nothing for one second;
  - no review question's Enter is waiting on it (ADR 0163).
  There is no time limit. It says once, in `*Messages*`, and in the echo area only while the
  echo area says nothing else: "NAME gets /add-dir REPO once its turn ends and its input box is
  empty". An agent's add says it in its own line instead: "Agent: session … added repository
  … to space …; oso types /add-dir into it once its turn ends".
- **Typed without showing the session.** The person's window is left where it is.
- **Then one Enter, as a key of its own,** as a review's question gets one (ADR 0163, "Submit
  once pasted, as amended"): at least 150 ms after the paste, once the screen has changed since
  the paste, within 2 s. It goes only if the offer still holds, the person has typed nothing
  since the paste, and the box holds the command and nothing else, with no menu, picker or
  suggestions below it (`sessions::send::holds_command`): each row the command's next
  characters exactly, a break taking a space of the command or falling inside a run with none,
  as the TUI wraps. If any of these fails, the command stays in the box, not sent:
  "/add-dir REPO waits in NAME's input box, not sent: WHY; press Enter there". Sent, it says
  "Typed /add-dir REPO into NAME and sent it", and the space notes the repository as given.
- **Nothing more is typed for 1 s after an Enter,** into any terminal, nor is a review's
  question pasted into that one: the TUI may draw an empty box for a frame before its own
  question, which the next command would land in.
- **Skipped, and said in the echo area:** a session gone or not running ("NAME is not running:
  /add-dir REPO was not typed; it gets REPO with --add-dir when it starts"), and an offer that
  no longer holds ("/add-dir REPO was not typed: the space or its orchestrator changed
  meanwhile"). A repository added while the orchestrator is stopped says "NAME is not running:
  it gets REPO with --add-dir when it starts".
- **A review's question to a terminal whose `/add-dir` waits for its Enter, or was sent less
  than 1 s ago,** is refused: "NAME is taking an /add-dir; ask again in a moment".

### A native orchestrator

- **Resumed with no question** (`resume_with_dir`), as `y` did before (ADR 0176, part O-2): it is
  stopped once it is free to, and resumed with `--resume` and the new `--add-dir`, its rows
  saying so. The rule that its question never comes over another one the person may be
  answering goes, as there is no question. Its echo says "Resuming NAME with REPO added" or "NAME
  resumes with REPO added once its turn ends".

### `space_add_repo`'s reply

`add_dir` is one of:

- `queued`: oso types and sends `/add-dir PATH` once the turn ends;
- `resume queued`: a native orchestrator is resumed with it once the turn ends;
- `given`;
- `waits for trust`: its note says oso then types it, or resumes the orchestrator with it;
- `not queued`: the path is no plain line, so the agent asks the person to type it;
- `next resume`.

`offered`, `not offered` and `resume offered` are gone. `?` on the add's question says that oso
types `/add-dir` and sends it, asking nothing more.

### A known limit: Claude Code's own question

Claude Code asks its own question when `/add-dir` names a folder that its workspace trust does
not cover. Its permissions documentation says: "Claude Code asks you to trust it first. This
requires Claude Code v2.1.326 or later". Its changelog for 2.1.331 says: "fullscreen `/cd` and
`/add-dir` show their choices in fewer rows". oso does not answer that question. If the screen
after the Enter shows a dialog or no input box within 5 s, oso says once: "NAME asks you to
trust REPO: answer it in NAME". The session's attention mark already shows that it needs
input. The choices it offers were not looked at: a probe with a real `claude` would meet its
workspace trust question first, and answering either one writes the person's Claude Code
configuration.

### Code

- `sessions::send`: `pub fn holds_command(rows: &[String], command: &Command) -> bool`, with
  tests.
- `sessions::tools::AddDir`: `Queued`, `ResumeQueued` and `NotQueued` in place of `Offered`,
  `ResumeOffered` and `NotOffered`, with their words and notes. `add_dir(trusted, given, queued,
  native)` keeps its shape.
- `app::sessions::spaces::add_dir`: the queue, the typing and the Enter. `offers`: `give_added`
  and `give_dir` in place of the offer, and `Gave`. The offer, its `Act` and the space's
  `offer` field are gone; `space-confirm`'s `y` keeps a replacement's and a review's offers.

## Changes to accepted ADRs

- **ADR 0161, "The `/add-dir` offer is the one oso already makes (Q6)":** oso asks nothing. A
  trusted repository not given is typed into a terminal orchestrator and sent once it is idle,
  or a native one is resumed with it, as above. The reply's words are this ADR's. Its "One
  question that also pastes `/add-dir`" alternative is moot: no question is asked.
- **ADR 0176, part O-2 ("`/add-dir`"):** a native orchestrator is resumed with no question; the
  rest of the part stands.
- **ADR 0121, Q17 ("not submitted"), and ADR 0163 ("These two are the only submits to a
  session"):** oso's own `/add-dir PATH`, for a trusted repository of the orchestrator's own
  space, is a further submit, through `submit_paste`, under the checks above.
- **ADR 0136, "Security" ("a running orchestrator gets it only through the person's paste"):**
  and through oso's own typed `/add-dir`, for a member oso trusts.

## Consequences

- The person adds or trusts a repository and does nothing more: the orchestrator reads it from
  its next idle moment.
- An orchestrator's reach grows when a trusted repository is added, with no extra step from
  the person. With `space_add_repo` and the configuration that lets it add without asking, a
  prompt injection in an orchestrator can bring into its own `claude` a repository that oso
  already trusts, with that repository's `CLAUDE.md`, skills, commands and subagents. Before,
  the person's paste and Enter stood between. An untrusted repository still needs the person's
  `y` to join and their `trust-project` to be given. The echo line, `*Messages*` and
  `agent-starts-ask` still let the person see it and stop it.
- Text typed into a session by oso, unasked, now has a second source besides a review's
  confirmed question. It is still only oso's own command, built from a recorded path.
- The person's draft is never mixed with the command: oso waits for an empty box and a pause in
  typing, and sends the Enter only while the box holds the command alone. A key the person
  types in the few milliseconds between the last screen read and the Enter is the one window
  left.
- Whether the real TUI shows a menu, or reads the Enter as a completion, under a pasted
  `/add-dir PATH` was not measured. oso's check refuses the Enter while a menu shows below the
  box.

## Alternatives considered

- **Keep the question and submit at `y`:** the author asked for no confirmation.
- **Resume a terminal orchestrator with `--add-dir`, as the native path does:** it needs no
  typing, but it ends the person's TUI session and its state on screen; typing the command is
  what a person would do.
- **Answer Claude Code's own trust question ("for this session"):** it would type into a dialog
  whose choices were not looked at, and accept a trust the person did not see; left to the
  person, with a word.
- **A time limit on the wait:** a turn can run long, and a draft can sit; the offer is judged
  again before it goes, so a late one is still right.
