# The self-hosted macOS runner

`just ci (macos)` and `just e2e (macos)` run on two GitHub Actions runners on one of the
author's Macs
([ADR 0166](adr/0166-macos-ci-on-a-self-hosted-mac-and-fewer-jobs-per-pull-request.md),
as amended):

| Runner      | Labels                               | Runs              | Folder               |
| ----------- | ------------------------------------ | ----------------- | -------------------- |
| `oso-mac`   | `[self-hosted, macOS, ARM64, oso]`    | `just e2e (macos)` | `~/actions-runner`   |
| `oso-mac-2` | `[self-hosted, macOS, ARM64, oso-ci]` | `just ci (macos)`  | `~/actions-runner-2` |

Both jobs run on every push to `main`, and on a branch when its workflow is started by hand;
a pull request runs neither
([ADR 0188](adr/0188-no-ci-on-pull-requests-and-every-commit-on-main-checked.md)). No push
cancels another's run, so with several merges in a row the jobs wait in line, one of each at
a time: about 17.5 minutes per `just e2e (macos)`. So the two jobs run at once, and only
`oso-mac` opens windows. Both runners run as the same
macOS user and use the same hooks. The first runner keeps `oso`, the label every macOS job
asked for before, so a branch that has not merged `main` since still runs both jobs there,
one after the other. Never give `oso-mac-2` the label `oso`: two e2e runs at once would take
the focus from each other.

This page sets them up, keeps them running and removes them. The author does all of it by
hand. No step is run by CI or by an agent.

The repository is public. A fork's pull request must never run on this Mac, and the runners
must not be able to reach anything of yours. Three things keep forks off:

- the job-started hook below, which both runners run, and which lives on the Mac where a
  fork cannot edit it;
- the approval setting in step 1;
- the workflows, which send a fork's macOS jobs to GitHub's own runners.

Placeholders: `OWNER/REPO` is this repository, as `gh repo view --json nameWithOwner` prints
it. `ci` is the runner's macOS user; any other name works if you use it everywhere.
`YOU` is your own macOS user.

## What it needs

- An Apple silicon Mac on macOS 15 or later, with `/usr/bin/jq`. It should be the Mac mini,
  with the runner's user logged in at its console. `just e2e` opens windows, so the runner
  needs a GUI session that is not in the background.
- Disk space for the kept builds: the hooks keep each runner's under a cap, by default
  30 GB for `oso-mac-2` and 15 GB for `oso-mac`, so 45 GB for both, plus the toolchains.
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
no job can change them, and they live outside the runners' folders, as GitHub asks. Both
runners use the same two scripts. Before you paste the commands below, change
`repo=OWNER/REPO` in them to the repository.

`clean.sh` cleans up only after its own runner, which it knows by `RUNNER_NAME`, the name
the runner was registered with:

- It stops the `oso` and `fake-lsp-server` processes whose executable lies in that runner's
  kept builds, `~/ci-target/<runner>/`. `just ci` starts `fake-lsp-server` too, so stopping
  every process of that name would break the other runner's job.
- It keeps that folder under its runner's cap, removing branches' builds (`pr`) first, then
  `main`'s. It never removes the other runner's builds, which may be in use, and it trims
  nothing when its folder is a link.
- The runner named by `e2e_runner`, the first one, has the cap `cap_gb_e2e`; any other
  has `cap_gb_ci`. The first one also owns `~/ci-target/pr` and `~/ci-target/main`,
  where a single runner kept its builds and where branches that have not merged `main` yet
  still build.

On a Mac set up for a single runner, only `clean.sh` changes: replace it with the one
below, before you add the second runner. It works with a single runner too.

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
# Cleans up around a job (ADR 0166, as amended) and never fails one. Two runners share
# this Mac's user, so it touches only what this runner's own jobs could have left: the
# oso and fake-lsp-server processes started from its own kept builds, and those builds,
# which it keeps under the cap.

# The first runner, which runs just e2e. It also owns the kept builds from before there
# were two runners, ~/ci-target/pr and ~/ci-target/main, which branches that have not
# merged main yet use.
e2e_runner=oso-mac
# Each runner's cap on its kept builds, in GB. Change them here, as an admin.
cap_gb_e2e=15   # the e2e runner's
cap_gb_ci=30    # the other runner's, which runs just ci

# Letter ranges below mean ASCII letters only.
export LC_ALL=C
say() { echo "oso-ci: $*"; }
name=${RUNNER_NAME:-}
case "$name" in
    '' | . | .. | pr | main | *[!A-Za-z0-9._-]*)
        say "no usable runner name ('$name'): nothing cleaned"
        exit 0 ;;
esac
# Without links, as lsof gives executables' paths.
home=$(cd -P "${HOME:-/nonexistent}" 2>/dev/null && pwd) || home=
case "$home" in
    /?*) ;;
    *) say "no usable home ('${HOME:-}'): nothing cleaned"; exit 0 ;;
esac
root="$home/ci-target"
own="$root/$name"

# Whether a path lies in this runner's kept builds.
owned() {
    case "$1" in
        "$own"/*) return 0 ;;
    esac
    [ "$name" = "$e2e_runner" ] || return 1
    case "$1" in
        "$root/pr"/* | "$root/main"/*) return 0 ;;
    esac
    return 1
}

# A process counts as this runner's when its executable, with links resolved, lies in
# this runner's builds. lsof gives that path; ps gives the path it was started by.
uid=$(id -u)
for proc in oso fake-lsp-server; do
    for pid in $(pgrep -u "$uid" -x "$proc" 2>/dev/null); do
        exe=$(/usr/sbin/lsof -a -p "$pid" -d txt -Fn 2>/dev/null | sed -n 's/^n//p' | head -n 1)
        if [ -n "$exe" ] && owned "$exe"; then
            kill "$pid" 2>/dev/null && say "stopped $proc ($pid)"
        fi
    done
done

if [ "$name" = "$e2e_runner" ]; then cap_gb=$cap_gb_e2e; else cap_gb=$cap_gb_ci; fi
case "$cap_gb" in
    '' | *[!0-9]*) say "the cap is not a number ('$cap_gb'): builds not trimmed"; exit 0 ;;
esac
# A link would take the trimming into another folder, such as the other runner's.
if [ -L "$root" ] || [ -L "$own" ]; then
    say "$root or $own is a link: builds not trimmed"
    exit 0
fi
cap_kb=$((cap_gb * 1024 * 1024))
used() { du -sk "$@" 2>/dev/null | awk '{ s += $1 } END { print s + 0 }'; }
if [ "$name" = "$e2e_runner" ]; then
    set -- "$own" "$root/pr" "$root/main"
else
    set -- "$own"
fi
# The oldest kind of build goes first: the single runner's, then pull requests', then main's.
for drop in "$root/pr" "$root/main" "$own/pr" "$own/main"; do
    [ "$(used "$@")" -gt "$cap_kb" ] || break
    owned "$drop/x" || continue
    [ -e "$drop" ] || continue
    rm -rf "$drop" && say "removed $drop, over the cap of $cap_gb GB"
done
say "cleaned for runner $name"
exit 0
EOF
sudo chown -R root:wheel /usr/local/libexec/oso-ci
sudo chmod 755 /usr/local/libexec/oso-ci /usr/local/libexec/oso-ci/*.sh
```

Then check these lines:

```sh
grep '^repo=' /usr/local/libexec/oso-ci/job-started.sh
grep -E '^(e2e_runner|cap_gb_e2e|cap_gb_ci)=' /usr/local/libexec/oso-ci/clean.sh
```

`e2e_runner` must be the first runner's name. The kept builds take up to the two caps
together. Each runner keeps a build for branches (`pr`) and one for `main`, and a `just ci`
build is several times larger than a `just e2e` one, so the ci runner's cap is the larger.

To change a cap, edit `cap_gb_ci=` or `cap_gb_e2e=` with `sudo`; the next job uses it.
Before you raise one, check that both fit, with room left for the toolchains:
`df -h /Users/ci` for the free space, and `sudo du -sh /Users/ci/ci-target/*` for what the
builds take now.

## 3. Install the first runner (as the runner's user)

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

## 4. Add the second runner (as the runner's user)

Still as `ci` at the console. The second runner uses the same user, rustup, toolchains and
hooks, in a folder of its own.

1. Download the runner as in step 3.2, into `~/actions-runner-2` this time. Take a new token
   from the "New self-hosted runner" page.
2. Register it with the label `oso-ci`, and never `oso`:

   ```sh
   cd ~/actions-runner-2
   ./config.sh --url https://github.com/OWNER/REPO --token TOKEN \
       --name oso-mac-2 --labels oso-ci --work _work --unattended
   ```

   Check `cat .path` as in step 3.3.
3. Turn the hooks on and run it as a service, as in steps 3.4 and 3.5, in
   `~/actions-runner-2`. Its LaunchAgent has a name of its own, after the runner's, so the
   first runner's service is not touched.

Its name names its kept builds, `~/ci-target/oso-mac-2`. The hooks and the workflow refuse a
name that is empty, `pr`, `main`, or has characters other than letters, digits, `.`, `_`
and `-`.

## 5. Check it

- Settings → Actions → Runners shows `oso-mac` as Idle with the labels `self-hosted`,
  `macOS`, `ARM64` and `oso`, and `oso-mac-2` with `oso-ci` in place of `oso`.
- Re-run the latest push to `main`'s CI run. `e2e / just e2e (macos)`
  runs on `oso-mac` and `just ci (macos)` on `oso-mac-2`, at the same time. In each job's
  log, "Set up runner" shows `oso-ci: cleaned for runner …` with that job's runner. Then
  `ls ~/ci-target/oso-mac/main ~/ci-target/oso-mac-2/main` lists `e2e` and `ci`.
- A refused job fails at "Set up runner" with `oso-ci: this runner does not run this job`.

## Keeping it up

- **The runners** update themselves when GitHub releases a new version. Leave it on.
- **Rust:** each job's `rustup toolchain install` installs a newly pinned toolchain.
  - Update rustup itself now and then, as `ci`: `rustup self update`.
  - Remove old toolchains: `rustup toolchain list`, then `rustup toolchain uninstall <name>`.
  - When the pin changes, both runners' first jobs install the new toolchain at the same
    time. If a job then fails with a broken toolchain, uninstall it and re-run the job.
- **gh and minisign**, as an admin: `brew upgrade gh minisign`.
- **Kept builds:** the hooks keep each runner's folder in `~/ci-target` under its cap
  (step 2). To start clean, as `ci`, stop both services, run `rm -rf ~/ci-target`, and start
  them again. Once every open branch has merged `main`, nothing builds in `~/ci-target/pr`
  or `~/ci-target/main` any more, and `oso-mac`'s hooks remove them when its folder is over
  the cap. You can also remove them yourself while both services are stopped.
- **Pausing them**, for example while `just perf-remote` measures on this Mac: as `ci`, run
  `./svc.sh stop` in both `~/actions-runner` and `~/actions-runner-2`. Then run
  `./svc.sh start` in both afterwards. Jobs queue meanwhile. `just e2e-remote` needs no
  pause: it runs as your own user over SSH and waits while `oso-mac` has a job
  ([testing.md](testing.md)). It finds that job by its worker, `actions-runner/bin/Runner.Worker`,
  so keep the first runner in `~/actions-runner`.
- **Changing the hooks:** edit them as an admin with `sudo`. They apply from the next job.
  After a change to a runner's `.env`, run `./svc.sh stop && ./svc.sh start` in its folder.

## While a runner is offline

A macOS job stays "Queued" until its runner is back, and GitHub fails a job that waits more
than 24 hours. No check is required yet, so nothing blocks a merge mechanically. On `main`,
`ci-ok` waits for them; pull requests run none.

To run both jobs on GitHub's runners instead:

1. Open Settings → Secrets and variables → Actions → Variables.
2. Add the repository variable `OSO_MACOS_RUNNER` with the value `macos-latest`.
3. Cancel the queued runs and re-run them.

Delete the variable once the runner is back.

When only `oso-mac-2` is offline, `oso-mac` can take `just ci (macos)` too, one job at a
time, by getting the label `oso-ci`. In Settings → Actions → Runners → `oso-mac`, add the
label, or with `gh`, as someone with admin access to the repository:

```sh
id=$(gh api repos/OWNER/REPO/actions/runners --jq '.runners[] | select(.name == "oso-mac") | .id')
gh api -X POST "repos/OWNER/REPO/actions/runners/$id/labels" -f 'labels[]=oso-ci'
```

Remove it once `oso-mac-2` is back:
`gh api -X DELETE "repos/OWNER/REPO/actions/runners/$id/labels/oso-ci"`. Never do the
reverse: `oso-mac-2` must not get `oso`.

## Removing the second runner

1. As `ci`, in `~/actions-runner-2`, run `./svc.sh stop` and `./svc.sh uninstall`.
2. Open Settings → Actions → Runners → `oso-mac-2` → Remove, and copy the token it shows.
   Then, still in `~/actions-runner-2`, run `./config.sh remove --token TOKEN`.
3. Give `oso-mac` the label `oso-ci` (see above), or `just ci (macos)` waits for a runner
   that no longer exists.
4. As `ci`, `rm -rf ~/actions-runner-2 ~/ci-target/oso-mac-2`.

The hooks stay as they are: they work for one runner too.

## Removing both

Remove the second runner first, as above, skipping its step 3. Then:

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

   A runner's registration is the only credential on the Mac. Removing a runner on GitHub
   revokes its registration, even if the Mac is gone.
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
