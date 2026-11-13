# Using oso daily

Install a release build of oso for daily work, and keep developing oso with `just dev` beside
it. The installed oso is a copy: rebuilding the repository does not change it while it runs.

A terminal in oso holds `oso dev`'s restarts until the restart handoff exists (ADR 0120, step
11). So run `claude` and other long-lived programs in the installed oso, not in `oso dev`.

## Prerequisites

macOS or Linux. Windows is not supported yet.

- The Rust toolchain from `rust-toolchain.toml`, `just`, and a C compiler for the syntax
  grammars (the Xcode Command Line Tools on macOS).
- Zig 0.17, the version `crates/oso-term/ghostty-vt.lock` names, on `PATH` (or set `ZIG` to it).
- `curl` and `minisign`.

Then, once, from the repository:

```sh
just ghostty-vt
```

It fetches the libghostty-vt release that the terminal is built on, and checks its signature and
sha256. The tarballs go to `~/.cache/oso/vendor` (or `$OSO_VENDOR_DIR`). Run it again when the
lock file changes.

## Install

```sh
just install
```

This builds the release binary, as `just release` does, with no dev features, and puts it at
`~/.local/bin/oso`. Give another directory as `just install DIR`. The binary is copied under a
temporary name and then renamed, so an oso that is already running keeps working. The recipe
prints where the binary went and its version and commit, and says if the directory is not on
`PATH`. It never edits your shell profile.

## Run

```sh
oso            # or: oso FILE
```

Start it from a shell. `oso --no-config` starts without your configuration file.

## Claude Code in a terminal

In the installed oso, `M-x terminal` opens a terminal with your shell. Run `claude` in it.

The `terminal` command arrives with ADR 0120's step 4; before that, the installed oso has no
terminal.

## Developing beside it

In the repository, run:

```sh
just dev
```

This runs a dev build that rebuilds and restarts when the source changes, and keeps its state.
The installed oso has no dev features: `oso dev` and `oso ctl` work only in a dev build.

Both share your configuration file and your per-user state, so a change to
`config.toml` reaches both.

## Where things live

- Configuration: `~/.config/oso/config.toml`, or `$XDG_CONFIG_HOME/oso/config.toml` when that is
  set. oso never writes this file.
- State that oso keeps by itself (emergency copies, the theme chosen last):
  `~/Library/Application Support/oso` on macOS, `$XDG_STATE_HOME/oso` or `~/.local/state/oso` on
  Linux.

## Update

```sh
git pull
just install
```

The running oso keeps the old binary. Quit it and start `oso` again when convenient; quitting
ends its terminals and what runs in them.
