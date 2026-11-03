# 0001. No user scripting

- Status: Accepted
- Date: 2026-11-03

## Context

Emacs gets its extensibility from Elisp and `init.el`. That model exists
because users and developers are different people. oso's primary user is its
developer, and others will use it as-is.

## Decision

- No embedded scripting language: no Lua, Scheme, Rhai or Wasm plugins, and
  no Elisp compatibility.
- Commands are written in Rust and registered in an internal **command
  registry** with a name, docstring, argument spec and function.
- Behavior that users may tune is declarative configuration
  ([ADR 0019](0019-configuration.md)).

## Consequences

- No interpreter overhead, GC pauses or startup cost, which serves the speed
  goal.
- No configuration that executes code, so a whole class of attacks
  disappears.
- Internals can be refactored freely, with no scripting API to keep stable.
  The compiler checks every command, which suits agentic development.
- The registry still provides `M-x`, `describe-function`, `describe-key` and
  a generated command reference.
- If scripting is ever needed, the registry is where it would attach.

## Alternatives considered

- **Embedded Scheme or Lua (Steel, mlua):** closest to Emacs, but adds a
  second language, runtime overhead and attack surface.
- **Wasm plugins (wasmtime):** sandboxed, but a large dependency, with no
  need for third-party extensions.
