# Testing

## Principles

- Keep logic out of GPU and window code so that most behavior can be tested
  with plain `cargo test`.
- Prefer property-based tests (`proptest`) for positions, offsets and
  coordinate mappings, where boundary cases are easy to miss.
- `just ci` (check, lint, fmt-check, test) must pass before every commit.

## Test pyramid

Decided in [ADR 0037](adr/0037-test-pyramid.md). Lower layers have many fast
tests; higher layers have few.

```
              manual checks (IME, real keys, look on macOS)
            E2E over the control socket       few
          pixel snapshots (lavapipe)          few
        text-grid snapshots                   every visible feature
      command scenario tests                  every command
    unit, property, reference-model tests     most
  alongside: fuzzing, performance budgets, mutation testing, coverage
```

### Rules

- A bug fix adds a test at the lowest layer that reproduces the bug.
- Every command gets scenario tests (`add-command` skill).
- Every feature that changes what is drawn gets text-grid snapshots.
- Pixel snapshots and E2E tests cover seams between components and critical
  flows only, never logic a lower layer can test.
- Snapshot updates are reviewed like code.

### Text-grid snapshots

The screen is rendered as a grid of characters and compared as text with
`insta` ([ADR 0045](adr/0045-text-grid-snapshots.md)). They need no GPU, so
they run on every OS in `just ci`, and both people and agents can read the
diffs. Decorations, the modeline and the minibuffer will show up once they
exist.

```
keys: C-f
│日本語 a\xFF        │
:<>^^<>  ~~~~        :
│                    │
```

Each row of the window is a line between `│`. Below a row that needs it, a
line between `:` marks its cells:

| Mark | Meaning |
|---|---|
| `^` | the cursor box, over any other mark |
| `<>` | the two cells of a wide cluster |
| `~` | a cell of an escape (`^M`, `\xFF`) |
| `#` | a cell of a boxed invisible character (`U+202E`) |

`▌` and `▐` stand for a wide cluster that an edge of the window cuts in half.
Text that reads like an escape has no mark, which is how the two differ. `#`
does not show where one box ends and the next begins. The
padding before the closing `│` shows the row's width whatever font the diff is
read in.

**To write one,** add a test to `crates/oso/src/dispatch/grid_tests.rs`. It
lives there because `oso` is a binary crate, so `tests/` cannot reach the
`Dispatcher`:

```rust
#[test]
fn page_keys_scroll_by_a_screen() {
    let text: String = (0..12).map(|i| format!("line {i:02}\n")).collect();
    // A name, the file's bytes, rows and columns, and the keys of each frame.
    // "" is a frame with no new keys. Keys add up from frame to frame.
    check("page_scrolling", text, (5, 12), &["", "C-v", "M-v"]);
}
```

Use a small window, write control and invisible characters as escapes in the
test, and keep a scenario to one idea.

**To update them,** run `just snapshots`, read `git diff` as you would code,
and explain each change in the pull request. `just ci` never accepts a
snapshot: a new or changed one fails it, and so does a `*.snap.new` file that
a plain `cargo test` left behind. `just snapshots` clears those; delete one by
hand if its test is gone.

The grid draws through the code the renderer uses
(`DisplayCluster::for_each_run`, `Window::cursor_point`,
`motion::cursor_cells`). Anything drawn that a test cannot see this way belongs
in the same code, not in the renderer. What happens after the cells, such as
glyph placement and colors, is left to pixel snapshots.

### Guard tests

Some tests check the repository rather than behavior:

- `crates/oso/tests/workspace_lints.rs`: every crate uses the workspace
  lints.
- `crates/oso/tests/architecture`: dependency direction, confined crates,
  file sizes, `static` items and module headers
  ([ADR 0038](adr/0038-architecture-guardrails.md),
  [ADR 0039](adr/0039-architecture-guardrail-mechanics.md),
  [ADR 0041](adr/0041-module-responsibility-headers.md),
  [ADR 0042](adr/0042-statics-snapshots-and-render-errors.md)).

Each check comes with a unit test on a fixture that shows it catches a
violation, so a broken parser cannot pass silently.

### Test quality

- Mutation testing (`cargo-mutants`) runs weekly in CI. Surviving mutants in
  `oso-core` get a test or a note explaining why they are equivalent.
- Coverage (`cargo-llvm-cov`) is reported on pull requests. It never fails
  the build.
- The whole `cargo test` run stays within 30 seconds on the author's Mac.

## Testability by layer

| Testability | Layer | Strategy |
|---|---|---|
| High | `oso-core`: buffers, selections, display model, registry, keymaps | inline unit tests and property tests |
| High | pure coordinate functions | unit tests |
| Medium | command scenarios: key sequences to state | tests in `crates/oso/src/dispatch.rs` with no window |
| Medium | what is drawn: key sequences to the cell grid | text-grid snapshots in `crates/oso/src/dispatch/grid_tests.rs` with no window |
| Medium | the running app | end-to-end tests over the dev control socket |
| Low | `oso-render` GPU code | logic split out and unit-tested; pixels checked by snapshot tests |

## Test levels

### Level 1: logic tests, no GPU (always)

- Run in `cargo test` and `just ci`.
- Cover:
  - cursor and selection motion, including multiple cursors
  - scrolling rules as pure functions (`oso_core::window`)
  - streaming file decoding against a whole-file reference, with every
    split (`oso_core::decode`)
  - buffer edits on Japanese text and emoji
  - display-model mappings in both directions, with folds, inlays and wraps
  - keymap resolution and command dispatch
- Reach every command through the registry, as a user would. Key
  sequences run through the tip crate's `Dispatcher` with no window, for
  example `C-f C-f C-b` to a cursor position (`crates/oso/src/dispatch.rs`).

### Level 2: end-to-end tests over the control socket (from M1-7a)

- Start a dev build (`just run-dev`, or `cargo build -p oso --features dev`)
  and drive it with `oso ctl` over the dev control socket
  ([ADR 0023](adr/0023-control-socket-protocol-and-restart.md),
  [ADR 0032](adr/0032-control-socket-transport.md)):
  - `send-keys` sends input in Emacs notation, through the same path as
    keys from the window ([ADR 0033](adr/0033-key-presses-and-notation.md))
  - `run-command` runs a registered command by name, with a numeric
    argument of at most 10,000 if it takes one. Command errors and panics come
    back as error responses.
  - `get-state` reads the state as JSON: every buffer (name, path, whether
    it may hold secrets, read-only reason, line ending, BOM, invalid-byte
    count, loading progress, size), every window with its selections, the
    selected window's buffer, cursor and scroll position, its visible text,
    the keys of a pending prefix and the surface size. The visible text
    stops at 128 KiB and says so with `truncated`. The modeline comes with
    M2.
  - `get-text` reads whole rows, up to 512 KiB and 10,000 rows, and fails
    clearly past that.
  - `show-buffer` shows a buffer by id, until `switch-to-buffer` arrives
    in M2.
  - Secret-pattern buffers give no text and no screenshot
    ([ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md)). Any
    file counts as secret until its path is resolved, so wait for `loading`
    to become null before reading it.
  - `export-state` and `import-state` hand the state to a new instance, and
    `exit` stops one. `just dev` (`oso dev`) drives them on each source change
    ([ADR 0047](adr/0047-process-spawning-and-restart-handoff.md)).
  - `oso [FILE]` opens a file, so test files (generated in a scratch
    directory, never committed) can be driven this way.
  - `screenshot` writes a PNG of the current frame and returns its path
- Agents use these checks to verify behavior in the real app, following the
  `verify-in-app` skill. A Claude Code agent reads the screenshots to check
  visuals.
- Not covered yet: resizing the window, the mouse wheel and real OS key
  events. These are still checked by hand.
- The transport and protocol themselves are tested without a window or GPU
  (`oso-platform` control tests, the server tests in the tip crate), and so
  are process spawning, the state format, the order of a restart and the
  source watcher, so CI runs them on Linux and macOS. Windows is not supported yet (ADR 0032).

### Level 3: snapshot tests (after text rendering lands)

- Render headlessly and compare PNGs with a tolerance.
- macOS runs locally on Metal. Linux CI uses Mesa's software Vulkan driver
  (lavapipe), since oso has no GL backend.
- Golden images are used only once the UI is stable enough that false alarms
  stay rare.

## Performance tests

- Benchmarks cover:
  - key-to-photon latency, through the control socket with frame timestamps
  - startup time
  - opening large files
  - editing very long lines
- CI compares results against the budgets in
  [`roadmap.md`](roadmap.md#performance-budgets).
- Key-to-photon latency is measured inside the app, from receiving a key
  event to presenting the frame that reflects it. CI fails on a regression
  against `main`, and on p99 over 8 ms where a hardware GPU is available.
  Real photon latency is checked by hand from time to time
  ([ADR 0025](adr/0025-latency-measurement.md)).
- The dev incremental build time of the tip crate is measured with
  `just build-time`. The author's Mac decides the 2 s budget, and CI fails
  only on a large regression ([ADR 0024](adr/0024-dev-build-profile.md)).

## Fuzzing

`cargo-fuzz` targets cover file loading and decoding, the display model under
random edits, highlighting and terminal escape-sequence handling. Agents can
run them for long periods.

## CI

GitHub Actions (`.github/workflows/ci.yml`) runs on every pull request and
every push to `main`. See [ADR 0021](adr/0021-development-tooling.md).

- `just ci` on Linux, macOS and Windows. `check`, `lint` and `test` cover
  the dev features too (`--all-features`).
- `just release-safety` on Linux: release builds contain no dev-only
  features, crates or code ([ADR 0032](adr/0032-control-socket-transport.md))
- `cargo-deny` and `cargo-audit`
- a gitleaks scan of the full history
- `just public-api-check` on Linux: the public API matches
  `guardrails/public-api` ([ADR 0038](adr/0038-architecture-guardrails.md))
- the structure workflow (`structure.yml`): a structural change needs an
  ADR cited in the pull request description
- weekly, the design health report (`design-health.yml`) as a GitHub issue

Planned:

- performance and build-time budgets
- snapshot tests on Linux with lavapipe, once they exist
