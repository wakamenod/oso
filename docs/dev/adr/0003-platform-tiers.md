# 0003. Platform support tiers

- Status: Accepted
- Date: 2026-11-03

## Context

Linux, macOS and Windows support is wanted, as long as it does not cost speed
or security. Development and daily use happen on macOS.

## Decision

- Design for all three platforms from the start.
- **Tier 1: macOS.** Daily use and visual QA.
- **Tier 2: Linux and Windows.** CI builds and tests on every PR.
- OS-specific code sits behind traits in `oso-platform`: text rendering, IME,
  clipboard, filesystem and paths, process spawning, PTY and sandboxing.
- One wgpu backend per OS: Metal, DX12 and Vulkan. No GL fallback unless
  needed.

## Consequences

- Rendering speed is unaffected; an editor's draw load is negligible for wgpu.
- Runtime attack surface does not grow, since `cfg(target_os)` keeps other
  platforms' code out of each binary. More crates appear in `Cargo.lock` to
  audit.
- IME quality differs by platform, and Linux under winit is weakest. Only
  macOS is verified by hand.
- Paths need careful handling: Windows drive letters and UNC paths,
  case-insensitive filesystems and symlinks.

## Alternatives considered

- **macOS only:** fewer dependencies and simpler, but closes the door on the
  other platforms.
