---
name: verify-in-app
description: Check a change in the running app over the dev control socket. Start a dev build, send keys with oso ctl, take screenshots and read them, then stop the app. Use after changing input handling, rendering or anything user-visible.
---

# Verify a change in the running app

Agents cannot see the screen or send OS key events. Dev builds expose a
control socket instead (ADR 0016, ADR 0023, ADR 0032), and `oso ctl` drives
it. Use this after `just ci` passes, before the reviewer agents.

Unix only for now: on Windows, `oso ctl` reports that the socket is not
supported.

## 1. Build and start

```sh
cargo build --locked -p oso --features dev
RUST_LOG=oso=debug,wgpu=warn ./target/debug/oso > "$SCRATCH/oso.log" 2>&1 &
echo $! > "$SCRATCH/oso.pid"
```

- `$SCRATCH` is your scratchpad directory. Start the binary directly, not
  through `cargo run`, so the PID you record is the app's.
- Run it in the background. The window opens on the user's screen and may
  take focus; tell the user before starting it.
- Wait until the window exists. `get-state` reports a surface size once it
  does (the socket itself answers a few milliseconds after start):

  ```sh
  for i in $(seq 1 50); do
    ./target/debug/oso ctl get-state 2>/dev/null | grep -q '"width"' && break
    sleep 0.2
  done
  ```

- A file you opened counts as secret until its path is resolved, so wait
  until `get-state` reports `"loading":null` for it before reading its text
  or taking a screenshot.
- If it never answers, read `oso.log`. A refused control directory means
  something else created it with the wrong owner or mode; report it, do not
  change its permissions.

## 2. Drive and inspect

```sh
./target/debug/oso ctl send-keys "<right> <right>"   # Emacs key notation
./target/debug/oso ctl run-command forward-char      # a command by name
./target/debug/oso ctl run-command next-line 3       # with a numeric argument
./target/debug/oso ctl get-state                     # state and the visible text
./target/debug/oso ctl get-text 100 20               # 20 whole rows from row 100
./target/debug/oso ctl show-buffer 0                 # a buffer, by its id in get-state
./target/debug/oso ctl screenshot                    # prints {"result": {"path": ...}}
```

- Each command prints one JSON response. Exit status 0 is a result, 1 an
  error response, 2 means no request was made (no instance, bad arguments).
- Key notation: `C-x C-f`, `M-<`, `S-<right>`, `RET`, `SPC`, `TAB`, `DEL`,
  `<f1>`. Shift is part of a character: write `A`, not `S-a` (ADR 0033).
- Prefer `get-state` for facts (positions, sizes, visible text). Use
  screenshots for what only pixels show.
- Secret-pattern files (`.env`, `*.pem`, `id_*`, anything in `.ssh`, and
  symlinks to them; ADR 0044) give no text and no screenshot, and the error
  says so. Test with ordinary files. Never point a test at the user's real
  keys.
- Read the PNG at the returned path with the Read tool and check what you
  expected to see. Take one before and one after the input when the change
  is a difference.
- With several instances running, pass `--pid <pid>`.

## 3. Check a restart

`just dev` (`oso dev [FILE]`) rebuilds on a source change and restarts with the
same buffers, cursors, scroll positions and window (ADR 0045). To check it:

```sh
RUST_LOG=oso=info,wgpu=warn ./target/debug/oso dev "$FILE" > "$SCRATCH/dev.log" 2>&1 &
echo $! > "$SCRATCH/dev.pid"
pgrep -P "$(cat "$SCRATCH/dev.pid")" -x oso          # the instance's PID
```

- Run it from the repository root. It takes a copy of the supervisor's code at
  start, so to test a change to the supervisor, start it again.
- Edit a source file, then watch `dev.log`: each restart logs `restart took`,
  with the build and the handover. The PID changes; `get-state` shows the same
  buffer and cursor.
- A build that fails logs `the build failed` and keeps the PID.
- A new build with another `STATE_VERSION` logs `refused the state`, and both
  instances run until the next good build.
- Revert your edits with `git checkout` when done, which starts one more
  restart. Stop with `oso ctl --pid <pid> exit`, which also ends the supervisor.
  Killing only the supervisor leaves the instance running.

## 4. Stop and clean up

```sh
kill "$(cat "$SCRATCH/oso.pid")"
```

- Always stop the app, also when a step failed.
- Screenshots and the socket of a killed instance stay in the control
  directory until the next dev instance starts and removes them. They are
  private (`0600` in a `0700` directory). Do not copy them into the
  repository, and do not paste paths from them into commits or PRs: they
  contain the user's temp directory.

## Report

In the PR description, say what you drove and what you saw (for example,
"the cursor moved from x=8 to x=28 after two `<right>`"). Things the socket
cannot do yet, such as resizing the window or real OS key events, still
need a manual check by the user; list them.
