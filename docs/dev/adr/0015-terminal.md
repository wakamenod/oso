# 0015. Terminal

- Status: Accepted, amended by 0120
- Date: 2026-11-03

## Context

A terminal on par with modern Emacs terminal packages is needed after M4. The
author uses it as a pop-up terminal and as the backend for compile output.
Inside the terminal, searching the output (line search) is part of the daily
workflow.

## Decision

- The terminal engine is **`alacritty_terminal`**: pure Rust and proven (Zed
  uses it).
- A terminal is a **separate view type** that renders the cell grid directly.
  It exposes the content as searchable text, so isearch, line search, copy and
  selection work as in text buffers.
- Opening a terminal is **allowed in restricted mode**, since it is an
  explicit user action.
- Escape-sequence policy:
  - no OSC 52 clipboard reads
  - no sequences that echo data back
  - OSC 8 links show the real target before opening
- PTYs go through the process module: openpty on Unix, ConPTY on Windows.
- The terminal may also serve as the compile backend.

## Consequences

- Terminal output does not flow through the rope, so heavy output stays fast.
- The searchable-text interface must be kept in sync with the grid.

## Alternatives considered

- **libghostty-vt:** high quality, but a Zig/C library, which adds FFI,
  memory-safety and build-toolchain cost.
- **Mirroring the terminal into a text buffer, as Emacs packages do:** every
  command works unchanged, but it is slow with heavy output.
