---
name: verify-in-app
description: Check a change in the running app over the dev control socket. Start a dev build, send keys with oso ctl, take screenshots and read them, then stop the app. Use after changing input handling, rendering or anything user-visible.
---

# Verify a change in the running app

Agents cannot see the screen or send OS key events. Dev builds expose a
control socket instead (ADR 0016, ADR 0023, ADR 0031), and `oso ctl` drives
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

- If it never answers, read `oso.log`. A refused control directory means
  something else created it with the wrong owner or mode; report it, do not
  change its permissions.

## 2. Drive and inspect

```sh
./target/debug/oso ctl send-keys "<right> <right>"   # Emacs key notation
./target/debug/oso ctl get-state                     # cursor and surface size
./target/debug/oso ctl screenshot                    # prints {"result": {"path": ...}}
```

- Each command prints one JSON response. Exit status 0 is a result, 1 an
  error response, 2 means no request was made (no instance, bad arguments).
- Key notation: `C-x C-f`, `M-<`, `S-<right>`, `RET`, `SPC`, `TAB`, `DEL`,
  `<f1>`. Shift is part of a character: write `A`, not `S-a` (ADR 0032).
- Prefer `get-state` for facts (positions, sizes). Use screenshots for
  what only pixels show.
- Read the PNG at the returned path with the Read tool and check what you
  expected to see. Take one before and one after the input when the change
  is a difference.
- With several instances running, pass `--pid <pid>`.

## 3. Stop and clean up

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
