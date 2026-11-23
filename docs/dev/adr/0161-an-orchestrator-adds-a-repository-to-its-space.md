# 0161. An orchestrator adds a repository to its space

- Status: Accepted, amended by 0176 and 0177
- Date: 2026-11-19

## Context

A space's orchestrator was asked to work on a repository its space did not hold, and replied
that it has no way to add one. Only the person's `space-add-repo` (menu `a`,
[ADR 0136](0136-issue-spaces.md)) adds a repository, and ADR 0136 says "No tool … adds or removes
a repository": those widen what agents reach, so they were the person's commands.

What holds now:

- **`space-add-repo` (`Person`)** takes any absolute path. On a thread of its own it checks
  that the path's real path is a main checkout whose `.git` directory the person or root owns,
  not the root or a directory holding the home directory. A worktree is refused with a word on
  its repository. The device and inode are kept.
- **Trust stays per repository (ADR 0136, Q8).** A member oso does not trust is listed and
  marked restricted. It gets no worktrees, no workers, no unasked starts and no `--add-dir`
  until the person trusts it.
- **A running orchestrator** cannot read a repository added after its start until `/add-dir`
  runs in it. oso offers the person to paste `/add-dir PATH` into the orchestrator's input, not
  sent, for each trusted member it was not given (ADR 0136, step 3, Q5).
- **ADR 0135** lets a session the person started start sessions without asking, in trusted
  projects, as `config.toml` says.

## Decision

The author answered each question below on 2026-11-19, through the space's orchestrator: (a)
for each but Q3, where they chose (c). After the security review they narrowed Q2 too: an add
goes without asking only for a repository oso knew before the call (Q9).

### The tool (Q1)

- **`space_add_repo { path }`**, `Change`, "asks unless allowed", like `session_start`. `path`
  is absolute, at most 4096 bytes, and printable text with no control or invisible character.
  Its reply waits until the repository is added, or says why not.
- **Only a space's current orchestrator calls it,** and only for its own space, by the space's
  id, not the space in front. Any other session is refused with "only a space's orchestrator
  adds repositories; the person adds one with space-add-repo". No outside connection has it.

### Which paths (Q3, Q7)

- **Any repository's main checkout (Q3 (c)),** as the person's prompt takes, under every check
  `space-add-repo` makes. The look runs on a thread of its own, as the person's does.
- **The path must name the repository as it is:** no `.` or `..`, and no symbolic link at any
  directory on the way, at most 128 directories deep, and on this machine: no share, device
  path, `/net` or `/Network`. Otherwise it is refused before anyone is asked, and the reply says
  to name the real path. The question shows the real path the look found, which is then the
  agent's own but for case and repeated separators.
- **Its real path must be plain text,** with no control or invisible character, and it must
  not be the home directory or one above it by its device and inode, whatever path names it.
- **A worktree is refused (Q7),** with a word on its repository's main checkout, as
  `space-add-repo` does. A member of the space already, by its path or its device and inode,
  or a space of 16, is refused.
- **At the person's `y`** the repository is looked at again: its real path, device and inode
  must be those the first look found, or nothing is added.

### Asking (Q2, Q4)

- **ADR 0135's rule decides whether it asks:** the setting, `agent-starts-ask`, a caller the
  person started, and the repository trusted now, by its path and identity
  (`TrustedRoot::of_member`). It must also be one oso knew when the call came (Q9): one of the
  repositories `space-add-repo` completes over (the open spaces', the hosted sessions' and the
  open buffers' projects), matched by its device and inode on the look's thread. Where the
  system has no device and inode, it always asks. Otherwise it asks: `Agent: let session "fix-login" add repository
  "/src/web" to space "fix-login"?`. `?` says what adding gives the space's sessions.
- **A repository oso does not trust always asks (Q2),** whatever the configuration says. Trust
  is the person's, and a `y` adds it only restricted. The question says "oso does not trust it:
  it joins restricted, with no worktrees, workers or --add-dir until you trust it."
- **No limit beyond the loop guard and 16 members (Q4).** An add starts no process, so it does
  not count toward `agents_start_limit`.
- **The loop guard and the 60 s refusal** count it as any call that changes. The refusal is
  kept for the repository's device and inode, else its real path, so it holds for another path
  to the same repository.
- **One look a caller:** while a look at a repository the orchestrator named is out, its next
  call is refused with "try again". A look whose connection went before the person was asked
  stays until its thread answers, and then adds nothing.

### After adding (Q5, Q6)

- **The `/add-dir` offer is the one oso already makes (Q6):** once the repository is a trusted
  member and the orchestrator runs without it, oso asks the person, as a separate `y` or `n`,
  whether to paste `/add-dir PATH` into the orchestrator's input, not sent. For an agent's add
  it is asked right away (`App::offer_added`), so that the reply can say whether it was. A
  restricted one is offered once the person trusts it. A native orchestrator, which runs no
  `/add-dir`, is offered a resume with the repository added instead, and the reply says
  `resume offered` (ADR 0176, part O-2).
- **The reply says what the agent can do next (Q5):** the repository's name, path and trust,
  `asked`, a `note` in words, and `add_dir`:
  - `offered`: the person is asked about the paste. The agent cannot read the repository until
    they accept it and send it, so the agent should ask them to. Workers it starts with `repo`
    can use it at once.
  - `waits for trust`: the repository is restricted until the person trusts it with
    `trust-project`. oso then offers them the paste.
  - `given`: the orchestrator was given it with `--add-dir` already.
  - `not offered`: oso could not ask the person now; the agent asks them to type
    `/add-dir PATH`.
- **What the person sees:** an unasked add says `Agent: session "fix-login" added repository
  "/src/web" to space "fix-login", without asking` in the echo area and `*Messages*`. The
  `*Agent*` log has the call. An add after a `y` says the same, without "without asking". `?`
  on the question shows the whole path, which the question cuts past 80 characters.

### Code

- `channel::protocol::sessions`: `Call::SpaceAddRepo { path }`, which waits for the person as a
  start does. `oso mcp` describes it.
- `sessions::tools`: the table's twelfth tool, and a `starts` field for every tool, so that
  only the three that start a session count toward `agents_start_limit`.
  `sessions/tools/add_repo.rs` holds the add's rules: `addable`, `add_repo_question`,
  `add_repo_said` and `add_dir` with `AddDir`, with tests.
- `sessions::spaces::probe`: `main_checkout_as_named`, `main_checkout` with the path's own
  checks above, and `known`, which matches a repository to the known roots by device and inode.
  Both run on the repository check's thread (`SpaceJob::CheckRepository`'s `as_named` and
  `known`); a check whose thread cannot start is answered.
- `app/sessions/tools/add_repo.rs`: the call, the two looks, `Plan::AddRepo`, and the add.
  `app/sessions/spaces/agent.rs` keeps the looks with the space commands' pending checks
  (`App::check_repository_for_agent`), one a caller. `App::known_roots`
  (`app/sessions/spaces/lists.rs`) is the list `space-add-repo` completes over. The check that
  a caller is still its space's orchestrator, `App::still_orchestrates`, is shared with
  `session_resume`.
- e2e: `space_tools::an_orchestrator_adds_a_repository_to_its_space`.

## Questions

1. **The name.**
   (a) `space_add_repo`.
   (b) `space_add_repository`.
2. **A repository oso does not trust.**
   (a) Always asks, whatever the configuration says.
   (b) Follows the configuration like a trusted one.
3. **Which paths.**
   (a) Only repositories oso knows: the list `space-add-repo` completes over.
   (b) Those, and the projects in the trust list.
   (c) Any repository's main checkout, as the person's prompt takes.
4. **A limit on unasked adds.**
   (a) None beyond the loop guard and 16 members.
   (b) Unasked adds count toward `agents_start_limit`.
   (c) At most 2 unasked adds a space until the person types.
5. **The reply and `/add-dir`.**
   (a) It tells the agent to ask the person to accept the paste.
   (b) It states the fact only.
6. **The offer.**
   (a) The existing offer, a separate `y` or `n` after the add.
   (b) One question that also pastes on `y`.
7. **A worktree's path.**
   (a) Refused, naming its repository's main checkout.
   (b) Taken as its main checkout.
8. **Where it is recorded.**
   (a) A new ADR that amends 0121, 0135 and 0136.
   (b) An amendment inside ADR 0136.
9. **Which trusted repository goes without asking** (asked after the security review).
   (a) Only one oso knew before the call, by device and inode; any other asks.
   (b) Any trusted one, as built first.
   (c) None: the tool always asks.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0121](0121-agent-sessions-and-spaces.md), the tools table: a new row, `space_add_repo` |
  Change, asks unless allowed | a repository's main checkout for the orchestrator's own space,
  as `space-add-repo`.
- [ADR 0135](0135-agents-start-sessions-without-asking.md), "What it covers": and
  `space_add_repo`, under this ADR's rules: only a trusted repository oso already knew goes
  without asking, and an unasked add counts toward no limit.
- [ADR 0136](0136-issue-spaces.md), "Session tools in a space": "No tool … adds … a
  repository": an orchestrator adds one to its own space with `space_add_repo`. No tool makes,
  closes or reopens a space, removes a repository or replaces an orchestrator.
- ADR 0136, "Security", "`--add-dir` paths come only from the space's record, which only the
  person's commands change": and an orchestrator's `space_add_repo`, on the person's `y` or as
  their configuration allows. A repository is passed with `--add-dir` only once oso trusts it,
  and a running orchestrator gets it only through the person's paste.

## Consequences

- An orchestrator can bring the repositories its task needs into its space, and say what the
  person still has to do: trust a restricted one, or accept the `/add-dir` paste.
- With the configuration on, a prompt injection in an orchestrator can add a trusted repository
  oso knows to its space without asking, then start workers there without asking, up to the
  limit. Its reach grows from its space's repositories to every trusted repository oso knows;
  any other needs the person's `y`. The echo line,
  `*Messages*` and `agent-starts-ask` let the person see it and stop it, and its own `claude`
  reads the new repository only after the person's paste.
- An untrusted repository never joins without the person's `y`, and joins restricted: no git
  runs in it and Claude Code loads nothing of it until the person trusts it.
- The two looks leave a window between the second look and the add, as `space-add-repo` has
  between its look and the add. Each `--add-dir` start checks the member again.

## Alternatives considered

- **Ask every time:** the author chose the configuration's rule, as for starts.
- **Only repositories oso knows (Q3 (a)):** the person's prompt takes any path, and an unknown
  repository is untrusted, so it asks anyway.
- **One question that also pastes `/add-dir`:** the paste types into a terminal, so it stays
  the person's own answer to its own question.
- **Map a worktree to its main checkout:** the person would be asked about a place the agent
  did not name.
