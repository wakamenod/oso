#!/usr/bin/env bash
# Runs a command, the e2e flows, under a headless Weston on its own socket, then says how
# Weston ended. Used by e2e.yml's Wayland job and by e2e-weston-repeat.yml, to find out why
# Weston sometimes crashes mid-run. Diagnostics only: the flows run as they would without it.
#
# Usage: e2e-weston.sh DIR COMMAND [ARGS...]
#
# DIR gets Weston's own log (weston.log), its output and how it ended (weston.out), what
# the shell around it saw (weston.probe), its core dump if it crashed (cores/), and then a
# report of the crash (crash.txt) and a `crashed` marker. With WESTON_PROTOCOL_LOG=1, Weston
# also logs every Wayland message (WAYLAND_DEBUG=server); the last PROTOCOL_BYTES of it are
# kept in weston.proto, with any string a message could carry text in replaced, and only
# once Weston has ended. Weston is stopped at the end, so that the next call can start one.
# Exits with the command's status.
set -uo pipefail

dir=$1
shift
mkdir -p "$dir/cores"
probe="$dir/weston.probe"
: > "$probe"
PROTOCOL_BYTES=$((32 * 1024 * 1024))

export XDG_RUNTIME_DIR
XDG_RUNTIME_DIR="$(mktemp -d)"
chmod 700 "$XDG_RUNTIME_DIR"

# Core dumps to a file here, rather than to the runner's handler, which keeps none. The
# setting is the whole machine's, so it is noted with what it was before.
echo "core_pattern was $(cat /proc/sys/kernel/core_pattern)" >> "$probe"
sudo sysctl -q -w "kernel.core_pattern=$dir/cores/core.%e.%p"

# Message strings that could carry text from a buffer: IME text, surrounding text, titles.
# None of oso's do today, but the log must not start to quote one: their arguments go.
redact='s/((surrounding_text|commit_string|preedit_string|set_title|set_app_id)\().*$/\1(redacted))/'

# The shell around Weston notes how it ended: its status, or 128 and the number of the
# signal that ended it. It also notes any signal it catches, and the process group and
# session of Weston, of itself and of the shell that started it, so that a kill of a group
# or a session shows. It runs without `-e`: under it, Weston's failing status would end the
# shell before it says so.
( set +e
  for sig in HUP INT QUIT TERM USR1 USR2 PIPE; do
    # shellcheck disable=SC2064 # the signal's name is wanted now, the time later
    trap "echo \"subshell caught SIG$sig at \$(date -u +%T.%N)\" >> \"$probe\"" "$sig"
  done
  ulimit -c unlimited
  args=(--backend=headless --renderer=pixman --width=1280 --height=800
        --socket=wayland-oso --idle-time=0 --log="$dir/weston.log")
  if [ "${WESTON_PROTOCOL_LOG:-0}" = 1 ]; then
    # tail keeps the end in memory and writes it once Weston's stderr closes.
    WAYLAND_DEBUG=server weston "${args[@]}" > "$dir/weston.out" \
      2> >(sed -E "$redact" | tail -c "$PROTOCOL_BYTES" > "$dir/weston.proto.part" \
             && mv "$dir/weston.proto.part" "$dir/weston.proto") &
  else
    weston "${args[@]}" > "$dir/weston.out" 2>&1 &
  fi
  weston_pid=$!
  echo "$weston_pid" > "$dir/weston.pid"
  ps -o pid,ppid,pgid,sid,comm -p "$weston_pid,$BASHPID,$$" >> "$probe"
  # A caught signal ends a wait early: wait again while Weston runs.
  while :; do
    wait "$weston_pid"
    weston_status=$?
    kill -0 "$weston_pid" 2> /dev/null || break
  done
  echo "weston exited with status $weston_status at $(date -u +%T.%N)" >> "$dir/weston.out" ) &

for _ in $(seq 1 50); do
  [ -S "$XDG_RUNTIME_DIR/wayland-oso" ] && break
  sleep 0.2
done
if [ ! -S "$XDG_RUNTIME_DIR/wayland-oso" ]; then
  cat "$dir/weston.log" "$dir/weston.out" "$probe"
  exit 1
fi
weston_pid=$(cat "$dir/weston.pid")

# When Weston goes, to 200 ms, to line up with the flows' own times.
( while kill -0 "$weston_pid" 2> /dev/null; do sleep 0.2; done
  echo "weston gone by $(date -u +%T.%N)" >> "$probe" ) &

# No X server, so winit can only take Wayland.
status=0
env -u DISPLAY WAYLAND_DISPLAY=wayland-oso "$@" || status=$?

if kill -0 "$weston_pid" 2> /dev/null; then
  echo "weston is still running"
  kill -TERM "$weston_pid"
  for _ in $(seq 1 50); do
    kill -0 "$weston_pid" 2> /dev/null || break
    sleep 0.2
  done
  exit "$status"
fi

# Weston was gone before the flows ended, and every flow after it failed to connect.
touch "$dir/crashed"
echo "::warning::weston was gone before the flows ended; see the report below"
{
  echo "=== How weston ended"
  cat "$dir/weston.out" "$probe"
  echo "=== The kernel's log"
  sudo dmesg -T | tail -40
  echo "=== Memory"
  free -m
  echo "=== Cores"
  ls -l "$dir/cores"
} 2>&1 | tee "$dir/crash.txt"

core=$(find "$dir/cores" -name 'core.weston.*' -print -quit)
if [ -n "$core" ]; then
  # Symbols and sources from Ubuntu's debuginfod, so that the frames in pixman and libweston
  # get names and lines. `info proc mappings` shows which mapping a faulting address is
  # next to, such as a client's shared memory.
  # shellcheck disable=SC2016 # $_siginfo and $pc are gdb's
  { sudo apt-get install -y -qq --no-install-recommends gdb > /dev/null
    echo "=== Backtrace of $core"
    DEBUGINFOD_URLS=https://debuginfod.ubuntu.com timeout 900 \
      gdb -batch -nx -q \
        -iex 'set debuginfod enabled on' \
        -ex 'set pagination off' \
        -ex 'set print frame-arguments all' \
        -ex 'print $_siginfo' \
        -ex 'thread apply all bt full' \
        -ex 'x/8i $pc' \
        -ex 'info registers' \
        -ex 'info sharedlibrary' \
        -ex 'info proc mappings' \
        "$(command -v weston)" "$core" 2>&1 | grep -v '^Downloading ' | head -n 6000
  } 2>&1 | tee -a "$dir/crash.txt"
else
  echo "no core of weston in $dir/cores" | tee -a "$dir/crash.txt"
fi

if [ "${WESTON_PROTOCOL_LOG:-0}" = 1 ]; then
  for _ in $(seq 1 50); do
    [ -e "$dir/weston.proto" ] && break
    sleep 0.2
  done
  { echo "=== The last Wayland messages Weston handled"
    tail -n 3000 "$dir/weston.proto" 2>&1
  } | tee -a "$dir/crash.txt"
fi
exit "$status"
