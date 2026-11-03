# 0008. Minibuffer, menus and popups as floating windows

- Status: Accepted
- Date: 2026-11-03

## Context

The author relies on a vertico/orderless/consult-style minibuffer shown in a
floating frame, and on about nine hydra menus. Git operations need
transient-style menus. Emacs's bottom minibuffer is hard to see.

## Decision

- The minibuffer, menus, completion popups and hover documentation are
  **buffers shown in floating windows** placed anywhere on screen. Editing
  commands (`C-a`, `C-k` and so on) work in them unchanged.
- Hydra and transient behavior merge into **one menu component**:
  - groups of single-key heads
  - heads that keep the menu open (repeatable)
  - toggle-state indicators
- Menus are **defined declaratively in the config file** and reload on change.
- Where each floating window appears is decided per feature, before that
  feature is built.

## Consequences

- One rendering and input path for all UI chrome.
- Users change menus without rebuilding.

## Alternatives considered

- **Dedicated widgets:** easier to style, but editing behavior would have to
  be reimplemented.
