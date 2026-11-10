# 0107. Foreign constants in `extern` blocks

- Status: Accepted
- Date: 2026-11-11

## Context

[ADR 0042](0042-statics-snapshots-and-render-errors.md) has the architecture test
reject every `static` item outside top-level test items, so that no editor state is global,
and says that "a real need ... gets an ADR that changes the check".

[ADR 0106](0106-the-input-source-turns-japanese-on-in-a-file-name-prompt.md) reads the keyboard
input source through HIToolbox's Text Input Sources, whose property keys are constants the
framework exports (`kTISPropertyInputSourceID`, `kTISPropertyInputSourceIsASCIICapable`). Rust
names a foreign constant as a `static` in an `extern` block, which the check rejected. Looking
the keys up at run time with `dlsym` worked around it, at the cost of more `unsafe` and a check
that no longer said what the code does. The author chose to change the check instead, through
the coordinator, and accepted this on 2026-11-11, through the coordinator.

## Decision

- **A `static` declared inside an `extern` block is allowed:** it names a constant that a
  foreign library defines and oso only reads. It holds no state of oso's, and reading one is
  `unsafe`, with a `// SAFETY:` comment, as every foreign call is.
- **Nothing else changes:** a `static mut` is rejected everywhere, an `extern` block's
  included, and a `static` outside an `extern` block or a top-level test item is rejected as
  before. `extern crate` and `extern fn` open no block.
- The architecture test's `statics` check finds `extern` blocks among its tokens, and its
  self-test pins all of the above.

## Changes to accepted ADRs

- **[ADR 0042](0042-statics-snapshots-and-render-errors.md), "Statics":** "The
  architecture test rejects every `static` item, whatever its type" becomes: every `static`
  item but those declared in an `extern` block, which name foreign constants; `static mut`
  stays banned there too.

## Consequences

- A system constant is declared where its functions are, as Carbon's property keys are in
  `oso-platform`'s `window/appkit/input_source.rs`.
- The check still cannot tell a constant from a foreign mutable global declared as a plain
  `static`; review catches that, as `extern` blocks are few and each is in `oso-platform`.

## Alternatives considered

- **`dlsym` for each constant:** keeps the check as it was, with more `unsafe` per constant
  and the symbol's name as a string.
- **An `#[expect]`-style exception per item:** ADR 0042 turned exceptions down.
