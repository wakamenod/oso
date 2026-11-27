# 0187. oso starts as a macOS app

- Status: Accepted (the author, 2026-11-28); step 1 (the login environment) built; step 2 (the
  bundle) to come
- Date: 2026-11-28

## Context

On macOS oso is started from a shell as `~/.local/bin/oso`. It has no Dock icon of its own, no
Spotlight entry, and no bundle, so Notification Center's banners, which need
`UNUserNotificationCenter` and so an app bundle, cannot come
([ADR 0121](0121-agent-sessions-and-spaces.md), Q9), and Files & Folders grants key on a
binary's path rather than an identity. The author wants to start oso from the Dock and Spotlight.

An app LaunchServices starts gets launchd's environment, not a shell's: `PATH` is
`/usr/bin:/bin:/usr/sbin:/sbin`, so a terminal, `git`, `gh`, `claude` and the language servers
would not find what the person installed with Homebrew, cargo or a version manager. Its working
directory is `/`, and its standard error goes nowhere.

## Decision

oso ships on macOS as `~/Applications/oso.app`, bundle id `io.github.wakamenod.oso`, and takes
the person's login-shell environment when LaunchServices starts it.

### The bundle (step 2)

- `Contents/{Info.plist, MacOS/oso, Resources/oso.icns, _CodeSignature}`. The real binary is
  `Contents/MacOS/oso`; `~/.local/bin/oso` is a symlink to it, so that `NSBundle.mainBundle` is
  the app's from either start, and one binary keeps `oso mcp`'s peer check (it canonicalizes
  both paths) matching an app instance. Linux's install is unchanged.
- The bundle id is permanent: TCC grants and notifications key on it.
- `Info.plist` sets `LSEnvironment {OSO_LAUNCHED_AS_APP = "1"}`. It has no document types and no
  `application:openURLs:`: Finder does not open files with oso, which opens a file only from
  `oso FILE`. It has no usage-description keys: oso never uses the microphone (dictation goes
  through TypeWhisper, [ADR 0175](0175-dictation-through-typewhisper.md)).
- One instance is LaunchServices' default: starting the app again activates the running one.
- `just install` signs the bundle with a self-signed local certificate, `oso local signing`,
  which the person makes once in Keychain Access, so that the designated requirement stays the
  same and Files & Folders grants survive a rebuild. `OSO_SIGN_IDENTITY=-` asks for an ad-hoc
  signature (CI, other machines); a missing identity fails the install and points to the steps.
  No hardened runtime.

### The login environment (step 1)

- **When:** only with `OSO_LAUNCHED_AS_APP` set, which `LSEnvironment` sets for every
  LaunchServices start (Dock, Spotlight, Finder, `open -a`). oso removes it from its own
  environment, so no child sees it, and `oso-restart` sets it for the instance it starts
  ([ADR 0133](0133-keeping-terminals-over-an-update.md)), which takes the environment again.
  Without it, nothing changes: a start from a shell already has the shell's environment.
- **Which shell:** the account's login shell from the user database
  (`oso_platform::process::account_shell`), never launchd's `SHELL`. One that is missing or not
  an executable gives `/bin/bash`; that missing too, nothing is taken.
- **How:** in `main`, after the helpers (`oso mcp`, `oso hook`) and the dev tools' check,
  before the options are read and before any thread, through `oso_platform::process::spawn`:
  `<shell> -l -c "printf '\000oso-env\000'; exec /usr/bin/env -0"`, standard input closed,
  standard output piped, standard error thrown away, in a process group of its own. A reader
  thread takes at most 1 MiB and one byte more. Past 2 s the group is killed with `SIGKILL`.
  The command text is a constant: nothing from the configuration, a project or an agent is
  part of it.
- **Reading:** what follows the last `\0oso-env\0`; what the profile printed before it is
  ignored. Each `NAME=VALUE` with a portable name (letters, digits and `_`, not first a digit),
  the first of each name. Dropped: `DYLD_*`, `LD_*`, `PWD`, `OLDPWD`, `SHLVL`, `_`, the marker
  and every other name (exported bash functions among them). How the shell ended does not
  matter when its output is good.
- **Merging:** the shell's `PATH` replaces launchd's when it is not empty. Every other name is
  set only where oso has none, so launchd's `HOME`, `USER`, `TMPDIR` and `SSH_AUTH_SOCK` stay.
  `LANG` unset afterwards becomes `en_US.UTF-8`. A working directory of `/` becomes `HOME`.
  The variables are set with `set_var` and `remove_var` while no other thread exists but the
  reader, which only reads its pipe.
- **Failures:** a timeout, no `\0oso-env\0`, over 1 MiB, or a shell that cannot start keeps
  launchd's environment (but for the marker and `LANG`) and says why in `*Messages*`, as an app
  launch's standard error goes nowhere. The time taken is logged: the author's `bash -l -c`
  takes 60–70 ms, and it delays only an app launch's window.
- **Profiles:** `bash -l` reads `~/.bash_profile`, else `~/.bash_login`, else `~/.profile`; a
  `PATH` set only in `~/.bashrc` counts only when `.bash_profile` sources it. zsh's login shell
  reads `~/.zprofile` and `~/.zshenv`, not `~/.zshrc`.
- **Where:** the tip crate's private module `launch` (`crates/oso/src/launch.rs`), in the
  lowest layer of `[oso]` in `guardrails/layers.txt`, as `app`, `main` and `carry` use it and it
  uses no other module of the crate.
- **Children:** unchanged. Every child still gets only the process module's allowlist and a
  `PATH` filtered by its rules ([ADR 0047](0047-process-spawning-and-restart-handoff.md)); what
  changes is the `PATH` those rules start from.

## Changes to accepted ADRs

None. ADR 0121's Q9 said banners "come when oso ships as one"; they are still a later step,
which needs the `objc2-user-notifications` dependency and is asked first. An erratum there says
so.

## Consequences

- A Dock or Spotlight start finds the person's programs, as a start from a shell does.
- The person's login profile runs once at each app launch, before the window: what it prints is
  ignored, and one that hangs costs 2 s and a note. It runs as the person's own shell would at
  login, with nothing of oso's in its command.
- The marker ships in release builds on purpose; the release-safety check is unchanged. The
  performance benchmarks never set it.
- Banners in Notification Center become possible, as a separate step.

## Alternatives considered

- **Read `~/.bash_profile` or `/etc/paths` ourselves:** would miss what a profile computes, as
  version managers' `eval "$(… init)"` do.
- **launchd's `SHELL`:** launchd sets it from the account anyway, but the user database is the
  record the terminal already uses ([ADR 0120](0120-the-terminal.md)).
- **An interactive shell (`-i`):** reads `~/.bashrc` and `~/.zshrc` directly, but prompts,
  `stty` and terminal-only setup misbehave without a terminal, and a login shell is what the
  author's profile is written for.
- **Taking every variable the shell sets over launchd's:** a profile's `TMPDIR` or
  `SSH_AUTH_SOCK` would replace the session's; only `PATH` is replaced.
- **A wrapper script as the bundle's executable:** gives the bundle the wrong main bundle, so
  no banners and a second Dock icon.
- **Ad-hoc signing:** a new signature on each install asks the Files & Folders questions again.
