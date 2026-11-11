# 0108. The user configuration file

- Status: Accepted, amended by 0110
- Date: 2026-11-11

## Context

[ADR 0019](0019-configuration.md) makes user configuration TOML, validated against a schema and
reloaded on change. [ADR 0051](0051-m2-plan-decisions.md) moved loading it to M3, and made user
key bindings a layer above the global keymap, so that `Keymap::bind` keeps refusing to rebind
([ADR 0034](0034-command-registry-and-keymaps.md)). Three accepted ADRs wait for it:

- [ADR 0085](0085-agent-channel-and-oso-mcp.md): a setting starts the agent channel at launch,
  and the channel never saves oso's configuration.
- [ADR 0089](0089-built-in-themes-switched-at-run-time.md): the configuration names the default
  theme and `<f5>`'s pair, and the last choice wins until the configured theme changes.
- [ADR 0097](0097-visual-aids.md): lasting choices for the visual aids belong to the
  configuration.

What exists today:

- The keymaps are compiled-in tables in `bindings`. `Layers` tries the keymap at point, the minor
  modes, the major mode and the global keymap, and the first that binds the whole sequence
  decides. A question's keymap and isearch's replace the global one.
- 111 commands are registered. None spawns a process, and no setting names a program. A command's
  reach is a field of its definition, not of a key.
- oso reads one small file it writes itself, `oso/theme` in the per-user state directory, on a
  thread started first thing and taken as the window opens without waiting.
- Files changed on disk are noticed by a `stat` once a second and when oso gains the focus
  ([ADR 0083](0083-reloading-files-changed-on-disk.md)), with no watcher crate.
- There is no configuration directory and no TOML parser. `toml_parser` and `winnow` are in the
  lock already, through an Android-only proc macro that oso's builds never compile.

The threat ([`security.md`](../security.md), T1 and T9): a repository must not configure
anything, and a same-user process or a steered agent must not gain, through the file, what it
could not do on its own.

## Decision

The author chose these on 2026-11-11, through the coordinator: the file's place, the parser, how
bindings are written, where the user layer sits, which keymaps are configurable, shadowing,
errors, settings against run-time choices, the channel's setting and the configuration
directory, the command line and the checks on the file.

### Where the file is

- **One file, `config.toml`, in oso's configuration directory:**
  `$XDG_CONFIG_HOME/oso/` when that is set and absolute, else `~/.config/oso/`, on Linux and
  macOS; `%APPDATA%\oso\` on Windows. `oso-platform::privacy` gains `config_home()`, beside
  `state_home()`, asking the environment and not the disk.
- No other place is searched: not the working directory, not the opened file's directories, not
  a variable naming a file. A repository's files are never read as configuration; EditorConfig
  and project trust are later M3 items ([ADR 0009](0009-project-trust.md)).
- **oso never writes `config.toml`.** It is the person's file. What oso remembers by itself, such
  as the last theme, stays in the state directory.
- A missing file is normal: the compiled-in defaults apply, with a log line and no message.
- **Checks before reading:** the path is followed through links, as dotfile managers link
  it. What it reaches must be a regular file of this user or root, not writable by every user,
  in a directory not writable by every user, and at most 1 MiB of UTF-8. A file that fails is
  not read, and `*Messages*` says why. A file writable by its group is read, with a warning.
- **Command line:** `oso --no-config` starts with the defaults in every build, for a
  file that misbehaves and for bug reports. A dev build also takes `--config FILE`, so the
  end-to-end flows and `just perf` never read the person's file; both pass one.

### When it is read

- **At start,** on a thread started first thing, as the theme's file is. The window opens without
  waiting for it. A read done by the first frame is applied before it; a later one is applied
  when it arrives, as a reload is, and keys before that use the defaults.
- **On change:** the thread `stat`s the path once a second and when oso gains the focus, as
  ADR 0083 does for buffers, and reads it again when the modification time, size, device or
  inode differs, or the file appears or goes. A save of the file from oso wakes it at once.
- **Off the UI thread, entirely.** The thread reads, parses, checks and builds the user keymap
  against its own copy of the registry, which `commands::registry()` builds the same way every
  time, so command IDs agree. The UI thread receives a finished value and swaps it in between
  keys. While a prefix key is pending, the swap waits for it to resolve.
- **Nothing is executed.** The file can only name commands that are registered and values of the
  types below. It cannot define a command, a macro, a program, a path to run or a key sequence to
  replay.

### What it holds (version 1)

```toml
[keys.global]
"C-c a" = "beginning-of-buffer"   # bind; "C-c" becomes a prefix in the user layer
"C-z" = false                     # unbind: the key is undefined, the default hidden
"<f5>" = "load-theme"             # shadow a default binding

[theme]
default = "dusk"                  # a built-in theme's name
pair = ["dusk", "light"]          # what <f5> toggles between

[aids]                            # the visual aids at start (ADR 0097)
whitespace = true
current-line = true
flash = true
line-numbers = false

[agent-channel]
start = false                     # start it at launch (ADR 0085)
```

- **Keys:** under `[keys.<keymap>]`, each entry is a key sequence in Emacs notation
  ([ADR 0033](0033-key-presses-and-notation.md)) and either a command name or `false`. Prefix keys
  are made by the sequences under them, as `Keymap::bind` makes them. A sequence a person cannot
  type, an unknown command, or a command only a dev build has, is refused with its line.
- **Keymaps:** version 1 has `global` only. `minibuffer`, `completion`, `isearch` and the
  major modes' names are reserved, and an entry under one says it is not configurable yet. A
  question's keymap is never configurable: its keys answer the channel's questions
  (ADR 0085).
- **`C-g` is not configurable** in any keymap: it is the way out of every pending state, and the
  resolver cancels a prefix with it whatever the keymaps say.
- **Names are kebab case,** as commands are. Unknown tables and keys warn and are skipped, so a
  file written for a newer oso still loads.
- **Room for what comes:** per-mode tables (`[keys.rust]`, `[modes.markdown]`), fonts, a fill
  column, the large-file threshold ([ADR 0051](0051-m2-plan-decisions.md)) and user secret
  patterns join as new tables. Anything that runs a program on open or save, such as a
  formatter, will come with the process and trust ADRs, as its own table that names a program
  the person approved; version 1 has no such table, and a key cannot reach one.

### The user layer

- **`Layers` gains a user keymap, directly above the global one and below every mode, prompt,
  prefix-argument and isearch keymap**, as Emacs's `global-set-key` writes into
  `global-map`. Where isearch replaces the global keymap and a key ends the search, that key is
  looked up through the user layer too.
- **`Keymap` gains an undefined entry** (`Keymap::unbind`): it stops the lookup, so a lower layer's
  binding of that sequence is hidden and the key reports "C-z is undefined". `bind` and
  `from_table` keep refusing to rebind.
- **Shadowing a default prefix with a command, or a default command with a prefix** (binding
  `C-a x` while `C-a` runs a command) needs that key unbound in the same table too; otherwise the
  entry is refused with a message. A plain rebinding of the same sequence needs nothing.
- `where-is`, `M-x`'s binding annotations and the keymaps `M-x` judges commands by read the
  user layer over the global one, so a hidden default is never shown.
- Binding a `Person` command is allowed: a key is pressed by the person. The channel has no
  `send-keys` and runs commands by name, so no binding widens what it reaches, and no setting
  changes a command's reach.

### Settings and run-time choices

- **A setting applies at start, and again only when its value in the file changes**. A
  choice made at run time (`<f5>`, `load-theme`, an aid's `M-x` toggle) stands until then.
- **Theme:** as ADR 0089 says, the remembered last choice wins over `theme.default` until the
  configured name changes. The state file gains a second line, the configured default when the
  choice was made. An unknown name or pair is reported and the built-in one kept.
- **Aids:** each starts from its setting instead of ADR 0026's default. They are still not
  remembered across starts.
- **Keys:** a reload replaces the whole user layer.

### The agent channel

- **`agent-channel.start = true` starts the channel at launch,** as `agent-channel-start` would,
  and `*Messages*` says the configuration started it.
- **Each connection is still asked,** as ADR 0085 decides. No setting skips the question: anyone
  who can write the file could otherwise drive oso unasked.
- **A reload never starts or stops the channel.** A changed value is reported as taking effect
  at the next start.
- **The channel never saves into oso's configuration directory**, whatever the person
  answers: `save-buffer`, `write-file` and any later writer compare the resolved target, and
  its device and inode, with the directory's. The reply says it was refused.

### Errors

- **Never a crash, never a wait.** A file that fails a check, cannot be read or does not parse
  leaves the configuration that was in force: the defaults at start, the last good one after.
- **A syntax error rejects the whole file; an entry that is wrong is skipped and the rest applies**
 . `toml`'s recoverable parse gives every syntax error with its span.
- **`*Messages*` gets one line per problem,** with the file's line and column: at most 20 a
  load, then a count. The echo area says "Configuration: 3 problems, see *Messages*". Quoted
  text is cut at 80 characters, and control and bidi characters in it are escaped.
- A good load after a bad one says so in one line.

### Budgets

- **Startup:** reading and parsing run beside the window and GPU startup, so the first frame
  waits for nothing. On the thread, a 64 KiB file reads, parses and builds in under 2 ms.
  Swapping it in on the UI thread costs under 0.1 ms. `just perf`'s startup figures stay
  within their thresholds.
- **Per key:** with no user bindings, the user layer is `None` and costs nothing. With some, one
  more hash lookup per key and per prefix level, no allocation: under 100 ns, within the noise of
  `just perf`'s key latency.
- **Watching:** one `stat` a second, and nothing woken while nothing changes.

### Dev restarts

- A new instance from `oso dev` reads the file itself. The handoff's theme and aid switches win:
  the file's values become the baseline without being applied, so a restart does not undo an
  `M-x` toggle. The channel setting applies as at a launch, since each connection asks again.
- [ADR 0002](0002-dev-hot-reload.md)'s configuration reload, which ships in release builds, is
  this reload; nothing about it is dev-only.

### Code

- A new tip-crate module, `config/`, reads, watches, parses and builds. It sits beside `dispatch`
  in `guardrails/layers.txt`, using `bindings` and `commands`. `toml` is used there only.
- `oso-core` gains `Keymap::unbind` and the undefined entry, and `Layers::user`.
- `oso-platform::privacy` gains `config_home()`.
- **The parser is `toml` 1.x, with `default-features = false` and `features = ["std", "parse"]`**
 : `toml::de::DeTable::parse_recoverable` gives spanned tables, with no serde and no derive
  macros. It adds `toml`, `serde_spanned`, `toml_datetime`, `toml_parser` and `winnow` to the
  build, the last two already in the lock. All are MIT or Apache-2.0, none has a `build.rs` or a
  proc macro, and all but `winnow` forbid `unsafe`. Its default recursion limit stays on.
  `cargo deny` and `cargo audit` pass with it. The version is the newest at least two weeks old
  when the dependency is added.

## Changes to accepted ADRs

- [ADR 0034](0034-command-registry-and-keymaps.md): a keymap holds commands and prefixes, and now
  undefined entries, which hide a lower layer's binding. `Layers` has a user keymap above the
  global one.
- [ADR 0089](0089-built-in-themes-switched-at-run-time.md): `oso/theme` holds a second line, the
  configured default theme when the choice was made.
- [ADR 0097](0097-visual-aids.md): "every start begins with ADR 0026's defaults" becomes: every
  start begins with the `[aids]` settings, whose compiled-in values are ADR 0026's defaults.

## Consequences

- People can rebind and unbind global keys and set the theme, the aids and the channel at launch.
  Nothing in the file can run anything, and no repository is read.
- A bad file costs a message, never a start or a key.
- The end-to-end flows and benchmarks must pass `--config` or `--no-config`, or a person's file
  would change what they measure.
- Per-mode keys, prompt and isearch keys, fonts and the other reserved tables are follow-ups.
- `toml` is a new dependency, with its approval.

## Alternatives considered

- **`~/Library/Application Support/oso/` on macOS:** the platform's place, but hard to type, and
  most dotfile setups and cross-platform tools use `~/.config`.
- **An `OSO_CONFIG` variable:** handy, but tools such as direnv set variables per directory, so a
  repository could choose the file.
- **Our own TOML subset parser:** no dependency, but people expect real TOML, and a parser of
  untrusted text needs fuzzing. **`toml_parser` alone:** smaller, but we would build tables from
  its events. **`toml_edit`:** keeps formatting for writing, which oso never does.
  **`basic-toml`:** serde-based and passively maintained.
- **The user layer above every keymap:** user keys would also win in prompts and isearch, unlike
  Emacs.
- **Every keymap configurable now, the question's too:** more to build at once, and the
  question's keys guard the channel.
- **A setting that skips the channel's connection question:** convenient, but it lets anyone who
  writes the file drive oso unasked.
- **Applying every setting on every reload:** a save of an unrelated key would undo `<f5>` and the
  aids' toggles.
- **Rejecting the whole file on any error:** one typo would drop every other binding.
- **Writing the file from oso** (a `customize`): two writers of one file, and the channel's rule
  about configuration gets harder to keep.
