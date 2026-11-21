# The self-hosted macOS runner

`just ci (macos)` and `just e2e (macos)` run on a GitHub Actions runner on one of the author's
Macs, labelled `[self-hosted, macOS, ARM64, oso]`
([ADR 0166](adr/0166-macos-ci-on-a-self-hosted-mac-and-fewer-jobs-per-pull-request.md)).
This page sets it up, keeps it running and removes it. The author does all of it by hand.
No step is run by CI or by an agent.

The repository is public. A fork's pull request must never run on this Mac, and the runner
must not be able to reach anything of yours. Three things keep forks off:

- the job-started hook below, which lives on the Mac where a fork cannot edit it;
- the approval setting in step 1;
- the workflows, which send a fork's macOS jobs to GitHub's own runners.

Placeholders: `OWNER/REPO` is this repository, as `gh repo view --json nameWithOwner` prints
it. `ci` is the runner's macOS user; any other name works if you use it everywhere.
`YOU` is your own macOS user.

## What it needs

- An Apple silicon Mac on macOS 15 or later, with `/usr/bin/jq`. It should be the Mac mini,
  with the runner's user logged in at its console. `just e2e` opens windows, so the runner
  needs a GUI session that is not in the background.
- Disk space for the kept builds: the hooks keep them under 40 GB, plus the toolchains.
- An admin account for the steps marked "as an admin".

## 1. Prepare the Mac and the repository (as an admin)

1. Install the command-line tools:

   ```sh
   xcode-select --install
   ```

2. Install `gh` and `minisign` with your Homebrew. They land in `/opt/homebrew/bin`, which
   every user can run. Don't log `gh` in for the runner's user: jobs pass their own token.

   ```sh
   brew install gh minisign
   ```

3. Create the runner's user as a standard user, not an admin. Don't sign it into an Apple
   Account, and don't give it SSH keys or keychain items.

   ```sh
   sudo sysadminctl -addUser ci -fullName "CI runner" -password -
   ```

4. Close the other homes. Every local user is in the `staff` group, and a home folder is
   readable by `staff` by default:

   ```sh
   sudo chmod 700 /Users/YOU
   ```

   Do it for every other user on the Mac. Don't keep anything private in `/Users/Shared`.
5. Keep the Mac awake, so that a job's windows keep drawing:

   ```sh
   sudo pmset -a sleep 0 displaysleep 0
   ```

6. On GitHub, open the repository's Settings → Actions → General → "Approval for running
   fork pull request workflows from contributors". Choose "Require approval for all external
   contributors". Before you approve a fork's run, read what it changes under `.github/`.

## 2. Install the hooks (as an admin)

The hooks run as the runner's user before and after every job. They are owned by root, so
no job can change them, and they live outside the runner's folder, as GitHub asks. Before
you paste the commands below, change `repo=OWNER/REPO` in them to the repository.

```sh
sudo mkdir -p /usr/local/libexec/oso-ci
sudo tee /usr/local/libexec/oso-ci/job-started.sh >/dev/null <<'EOF'
#!/bin/sh
# Refuses every job but this repository's own, before any step runs (ADR 0166):
# a push to main, a pull request from one of its own branches, the schedule, or a
# run started by hand. GitHub runs this with bash -e.
repo=OWNER/REPO
refuse() {
    echo "oso-ci: this runner does not run this job: $1" >&2
    exit 1
}
[ "${GITHUB_REPOSITORY:-}" = "$repo" ] || refuse "repository ${GITHUB_REPOSITORY:-unset}"
case "${GITHUB_EVENT_NAME:-}" in
    push)
        [ "${GITHUB_REF:-}" = refs/heads/main ] || refuse "push to ${GITHUB_REF:-unset}" ;;
    pull_request)
        head=$(/usr/bin/jq -r '.pull_request.head.repo.full_name // ""' "${GITHUB_EVENT_PATH:-/nonexistent}") \
            || refuse "the event could not be read"
        [ "$head" = "$repo" ] || refuse "pull request from ${head:-an unknown repository}" ;;
    schedule | workflow_dispatch) ;;
    *) refuse "event ${GITHUB_EVENT_NAME:-unset}" ;;
esac
# GitHub does not say whether the job-completed hook runs after a failed job, so clean here too.
exec /usr/local/libexec/oso-ci/clean.sh
EOF
sudo tee /usr/local/libexec/oso-ci/clean.sh >/dev/null <<'EOF'
#!/bin/sh
# Cleans up around a job (ADR 0166) and never fails one: stops what an e2e run left
# behind, and keeps the kept builds under the cap.
uid=$(id -u)
for name in oso fake-lsp-server; do
    pkill -u "$uid" -x "$name" 2>/dev/null || true
done
root="$HOME/ci-target"
cap_kb=$((40 * 1024 * 1024))
used() { du -sk "$root" 2>/dev/null | cut -f1; }
if [ -d "$root" ] && [ "$(used)" -gt "$cap_kb" ]; then
    rm -rf "$root/pr"
    [ "$(used)" -le "$cap_kb" ] || rm -rf "$root/main"
fi
exit 0
EOF
sudo chown -R root:wheel /usr/local/libexec/oso-ci
sudo chmod 755 /usr/local/libexec/oso-ci /usr/local/libexec/oso-ci/*.sh
```

Then check that line:

```sh
grep '^repo=' /usr/local/libexec/oso-ci/job-started.sh
```

## 3. Install the runner (as the runner's user)

Log in as `ci` at the Mac's console (not over SSH: the service needs this GUI session).
Open Terminal there.

1. Install rustup, checking its checksum, with no toolchain. Each job installs the one
   `rust-toolchain.toml` pins.

   ```sh
   mkdir -p ~/Downloads/rustup && cd ~/Downloads/rustup
   curl --proto '=https' --tlsv1.2 -sSfO https://static.rust-lang.org/rustup/dist/aarch64-apple-darwin/rustup-init
   curl --proto '=https' --tlsv1.2 -sSf https://static.rust-lang.org/rustup/dist/aarch64-apple-darwin/rustup-init.sha256 \
       | sed 's|  .*|  rustup-init|' | shasum -a 256 -c -
   chmod +x rustup-init && ./rustup-init -y --default-toolchain none --profile minimal
   ```

   Open a new Terminal window. Check that `command -v cargo gh minisign` prints three paths.
2. On GitHub, open Settings → Actions → Runners → "New self-hosted runner", and choose
   macOS and ARM64. Run the page's "Download" lines in `~/actions-runner`:
   - `mkdir actions-runner && cd actions-runner`;
   - the `curl` line;
   - the `shasum` check, which must print `OK`;
   - the `tar` line.

   Copy them from the page each time, so the version and checksum are current. Don't run
   its `config.sh` line yet.
3. Register the runner, with the token that page shows (it expires after an hour):

   ```sh
   cd ~/actions-runner
   ./config.sh --url https://github.com/OWNER/REPO --token TOKEN \
       --name oso-mac --labels oso --work _work --unattended
   ```

   It adds `self-hosted`, `macOS` and `ARM64` itself. It records this shell's `PATH` in
   `.path`. Check that `cat .path` includes `~/.cargo/bin` (expanded) and `/opt/homebrew/bin`.
4. Turn the hooks on:

   ```sh
   cat >>.env <<'EOF'
   ACTIONS_RUNNER_HOOK_JOB_STARTED=/usr/local/libexec/oso-ci/job-started.sh
   ACTIONS_RUNNER_HOOK_JOB_COMPLETED=/usr/local/libexec/oso-ci/clean.sh
   EOF
   ```

5. Run it as a service. On macOS this is a LaunchAgent of this user, so it runs while `ci` is
   logged in:

   ```sh
   ./svc.sh install
   ./svc.sh start
   ./svc.sh status
   ```

6. Leave `ci` logged in at the console. Switching to your own account with fast user
   switching puts its session in the background, where windows may stop drawing. Use your
   account over SSH or Screen Sharing instead. After a restart, log `ci` in again at the
   console. Automatic login works only with FileVault off.

## 4. Check it

- Settings → Actions → Runners shows `oso-mac` as Idle, with its four labels.
- Push a branch of this repository, or re-run a pull request's `just ci (macos)`. In the
  job's log, "Set up runner" shows the hook ran. Then `ls -l ~/ci-target/pr` lists `ci` and
  `e2e`.
- A refused job fails at "Set up runner" with `oso-ci: this runner does not run this job`.

## Keeping it up

- **The runner** updates itself when GitHub releases a new version. Leave it on.
- **Rust:** each job's `rustup toolchain install` installs a newly pinned toolchain.
  - Update rustup itself now and then, as `ci`: `rustup self update`.
  - Remove old toolchains: `rustup toolchain list`, then `rustup toolchain uninstall <name>`.
- **gh and minisign**, as an admin: `brew upgrade gh minisign`.
- **Kept builds:** the hooks keep `~/ci-target` under 40 GB. To start clean, as `ci`, stop
  the service, run `rm -rf ~/ci-target`, and start the service again.
- **Pausing it**, for example while `just perf-remote` measures on this Mac: as `ci`, in
  `~/actions-runner`, run `./svc.sh stop`. Then run `./svc.sh start` afterwards. Jobs queue
  meanwhile.
- **Changing the hooks:** edit them as an admin with `sudo`. After a change to `.env`, run
  `./svc.sh stop && ./svc.sh start`.

## While it is offline

macOS jobs stay "Queued" until the runner is back, and GitHub fails a job that waits more
than 24 hours. No check is required yet, so nothing blocks a merge mechanically. Still, wait
for `ci-ok`, which waits for them.

To run them on GitHub's runners instead:

1. Open Settings → Secrets and variables → Actions → Variables.
2. Add the repository variable `OSO_MACOS_RUNNER` with the value `macos-latest`.
3. Cancel the queued runs and re-run them.

Delete the variable once the runner is back.

## Removing it

1. As `ci`, in `~/actions-runner`:

   ```sh
   ./svc.sh stop
   ./svc.sh uninstall
   ```

2. Open Settings → Actions → Runners → `oso-mac` → Remove, and copy the token it shows.
   Then, still in `~/actions-runner`:

   ```sh
   ./config.sh remove --token TOKEN
   ```

   The runner's registration is the only credential on the Mac. Removing the runner on
   GitHub revokes it, even if the Mac is gone.
3. Set `OSO_MACOS_RUNNER` to `macos-latest` (see above), or change the workflows back, or
   macOS jobs will wait for a runner that no longer exists.
4. As an admin, remove the hooks and the user:

   ```sh
   sudo rm -rf /usr/local/libexec/oso-ci
   sudo sysadminctl -deleteUser ci
   ```

## Optional: keep jobs off the local network

A job can reach other machines on your network. That is a residual of ADR 0166. To block it,
add a packet filter rule for the runner's user. Let DNS through first if your resolver is
on the local network. For example, in a file that `/etc/pf.conf` loads:

```
pass out quick proto { tcp udp } from any to ROUTER_IP port 53 user ci
block drop out quick proto { tcp udp } from any to { 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16 } user ci
```

Test it with a job before you rely on it. pf matches `user` only for TCP and UDP sockets.
