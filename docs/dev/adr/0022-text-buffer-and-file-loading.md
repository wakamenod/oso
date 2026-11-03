# 0022. Text buffer and file loading

- Status: Accepted
- Date: 2026-11-03

## Context

The budgets require the first screen of a 100 MB file within 50 ms, and no
freeze on a 1 GB file or a 10 MB single line
([`roadmap.md`](../roadmap.md#performance-budgets)). The display model already
uses summarized trees in the style of Zed's `SumTree`
([ADR 0006](0006-display-model.md)). The buffer needs O(log n) conversions
between offsets and lines, and cheap snapshots for background work such as
loading and highlighting.

## Decision

- **Own rope, built on a `SumTree`.** oso implements a summarized B-tree in
  `oso-core` and builds both the rope and the display-model layers on it.
  - Leaves hold UTF-8 text chunks of bounded size that never split a
    character. Each node stores a summary (bytes, characters, line breaks)
    so that offset and line conversions take O(log n).
  - Byte offsets are the primary coordinate.
  - Nodes are immutable and reference-counted. Cloning a snapshot is O(1),
    and snapshots can be sent to background threads.
  - No `unsafe` (`oso-core` forbids it). Zed is a design reference only; no
    code is copied.
- **Progressive loading.** A background thread reads the file in chunks.
  - The first chunk goes to the UI as soon as it is read, so the first screen
    appears before the whole file is loaded.
  - The rest is appended in the background. The buffer is read-only until
    loading finishes, and the modeline shows progress.
  - Killing the buffer cancels loading.
  - Files are read, not memory-mapped.
- **UTF-8 only, for now.**
  - A UTF-8 BOM is detected and remembered.
  - The rope keeps line endings as they are in the file. LF and CRLF are
    detected and shown in the modeline. CRLF counts as one line break, and a
    lone CR is shown visibly.
  - Other encodings (Shift_JIS, EUC-JP and others) are added when they are
    needed, by converting at the load and save boundary. The rope always
    holds UTF-8.
- **Invalid UTF-8 and binary files open read-only.**
  - A file is binary if its first 8 KB contain a NUL byte.
  - Invalid bytes and control characters are shown as visible escapes, such
    as `\xNN` and `\0`. They are never interpreted, for example as terminal
    escape sequences.
  - Such buffers can never be saved, so their content is never corrupted.

## Consequences

- One tree implementation serves the buffer and every display-model layer,
  so its correctness matters a lot. It gets heavy unit and property tests,
  and fuzzing later.
- M1-4 grows: it includes the `SumTree` and the rope, not only the display
  model skeleton.
- No new dependency is needed for the buffer.
- Opening a large file shows content immediately, but line count and scroll
  extent are estimates until loading finishes.
- Non-UTF-8 files can be viewed but not edited until encoding support is
  added.

## Alternatives considered

- **ropey:** mature and widely used (Helix), but a second tree structure next
  to the display model's, and its API is centered on character indices.
- **crop:** byte-indexed and fast, but still a separate structure from the
  display model, with fewer users.
- **Memory-mapped files:** fast to open, but a file changed by another
  process can crash oso (SIGBUS), and mapping needs `unsafe`.
- **Loading the whole file before display:** simplest, but misses the 50 ms
  budget on large files.
- **Encoding detection from M1 (`encoding_rs`, `chardetng`):** two more
  dependencies and detection time on the first-screen path, before saving
  even exists.
