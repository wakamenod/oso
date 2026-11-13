# Agent rules

oso tells a hosted Claude Code session's state from its screen, through rules
([ADR 0121](adr/0121-agent-sessions-and-spaces.md)). The rules ship with oso
(`crates/oso/src/sessions/claude.toml`). When a new version of the TUI changes what it shows, fix
the rules the same day in your own file, `agent-rules/claude.toml` in oso's configuration
directory, beside `config.toml`. oso reads that file at start and again when it changes. It
never reads rules from a project, and it never fetches them.

## Finding what to change

1. Save the session's screen as text.
2. Run `M-x session-explain-file` on it. `*Session explain*` shows:
   - the state the rules give;
   - the rule that matched, whether it is bundled or local, and its priority;
   - the text of the region it matched;
   - if no rule matched, why not, and the screen's last 40 non-empty lines.
3. Write a rule that matches the new screen. Check it with the same command.

The recorded screens the bundled rules are tested against are in
`crates/oso/src/sessions/screens/`.

## The file

```toml
version = 1

[[rule]]
name = "permission"            # unique in the file
state = "needs_input"          # needs_input, working, idle or hold
reason = "permission"          # needs_input only: permission, question, plan, elicitation, dialog
priority = 940                 # higher first, from -100000 to 100000; 0 if left out
region = "after_last_rule"     # where the rule looks; see below
contains = ["esc to cancel"]   # every string appears; ASCII case is ignored
regex = ['(?s)a.*b']           # every pattern matches somewhere in the region
line_regex = ['^\s*❯ 1\. Yes'] # every pattern matches some line of the region
any = [{ contains = ["x"] }, { regex = ['y'] }]  # at least one table holds
all = [{ contains = ["z"] }]                     # every table holds
not = [{ contains = ["help"] }]                  # no table holds
recap = '(?i)^\s*(do you want to\b.*\?)\s*$'      # optional: the session's recap; see below
recap_region = "after_last_rule"                 # optional: where recap looks; the rule's region if left out
```

- The rules are tried from the highest priority down. Rules of the same priority keep the
  file's order, bundled rules before local ones. The first rule whose matchers all hold gives
  the state.
- Every rule needs a name, a state, a region and at least one matcher.
- `any`, `all` and `not` take a list of tables of the same matchers, nested up to 4 levels.
  They look at the rule's region.
- A string may be given in place of a list of one.
- Patterns use the `regex` crate's syntax. Write `(?i)` for case, `(?m)` for `^` and `$` at
  each line, and `(?s)` for `.` to match newlines.

### States

| `state` | Gives |
|---|---|
| `needs_input` | needs input, with its `reason`. Set it only for a dialog you know: an unknown prompt must stay idle. |
| `working` | working |
| `idle` | idle. With `visible = true`, the rule is a sure sign of it, and a turn ends at once. Without it, a turn ends after 3 looks 100 ms apart, or after 700 ms. |
| `hold` | the state stays as it was: for full-screen views, such as the transcript viewer, help or a picker. Rank hold rules below needs-input rules, so that a view never hides a dialog. |

No rule gives done, ready for review, failed or stopped:
- done and ready for review come from a turn's end;
- failed and stopped come from the process.

When no rule matches, the session counts as idle.

### Regions

| `region` | The text |
|---|---|
| `title` | the terminal's title |
| `progress` | OSC 9;4 progress, as `state` or `state;value` (`0` is cleared); empty if none was set |
| `bottom(N)` | the last N non-empty rows, N from 1 to 40 |
| `prompt_box` | the rows between the last two rules (rows of `─` only): the prompt box, or a dialog's body |
| `after_last_rule` | the rows after the last rule: the footer, or a dialog with its footer; empty with no rule |
| `above_prompt_box` | the last row above the prompt box |

Rows are the last 40 non-empty rows of the live screen. oso never reads your scrolled view or
copy mode. Each row is cut at 1 KiB, and control characters become spaces.

### Recaps

`*Sessions*` and `session-switch` show one line under each session: its recap.
- A session that needs input shows what the matched rule's `recap` finds: the dialog's question.
- A working session shows what the matched rule's `recap` finds, or else the first other working
  rule's whose recap finds something: the status line.
- Any other session, or one whose rule has no recap or finds nothing, shows the terminal's title.
- While a hold rule matches, the recap stays as it was.

`recap` is a pattern, matched against each line of `recap_region` (the rule's own region if
left out), from the bottom up. The lowest line it matches gives the recap: the pattern's first
group if it has one, else the whole match. A recap is one line of at most 80 characters, ending
in `…` if cut; control and bidirectional formatting characters are dropped, and runs of spaces
become one.

A recap never decides whether a rule matches. A wrong `recap` or `recap_region` is skipped and
named in `*Messages*`, and its rule stays. A recap counts among the file's 256 patterns. Finding
it gets 5 ms of its own after the rules' verdict; past that, the recap stays as it was.

### Replacing and disabling

- A rule in your file replaces the bundled rule of the same name.
- To remove a bundled rule, give only its name and `disable = true`:

```toml
[[rule]]
name = "title-spinner"
disable = true
```

### Limits and problems

A file holds at most:
- 256 KiB;
- 128 rules;
- 256 patterns;
- 64 matchers in a rule.

Each pattern or string is at most 4 KiB, and each compiled pattern at most 128 KiB.

All the rules together get 5 ms for one look at a screen. A file can stay within the limits
and still be slow, for example with many patterns that run over every row. Past the deadline,
the state stays as it was, and the same screen is looked at again a little later, the wait
doubling each time, up to 5 s. After two late looks in a row, `*Messages*` names the slowest
rule, once for each time the rules load. `session-explain-file` shows the same.

A rule of your file that is the slowest of 3 late looks in a row is not used until the rules
load again, and `*Messages*` names it. Fix it, and save the file. A bundled rule is never
dropped.

The deadline is best-effort: it is the wall clock's, and the rules check it between matchers,
so a busy machine can pass it once, and one slow pattern can run past it.

oso says what is wrong in `*Messages*`, with the line:
- A syntax error, or a `version` other than 1, refuses the whole file, and the rules in force
  stay.
- Anything else wrong in a rule skips that rule: an unknown name, a bad value, a pattern that
  does not compile. A misspelt matcher must not leave a rule that matches more than you meant.
  A wrong `recap` skips only the recap.

oso does not read the file when anyone else can change it, a group included, or the
directories on the way to it. A problem in a file whose name, or where it leads, says it may
hold secrets gives its line and none of its text, as in `config.toml`.

A state never authorises anything: not a paste, not an answer, not an action of an agent.
