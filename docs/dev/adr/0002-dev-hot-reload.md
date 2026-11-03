# 0002. Dev-only hot reload: restart-and-restore plus data reload

- Status: Accepted
- Date: 2026-11-03

## Context

Commands, key bindings and visuals change constantly during development.
Changes should reach the screen without a full rebuild and without losing the
current editing state. The target is 2–3 seconds from edit to screen. Hot
reload is a development tool only.

## Decision

1. **Data reload:** configuration (keymaps, menus, theme, fonts) and shaders
   reload on file change. Config reload also ships in release builds.
2. **Restart-and-restore:** on a `.rs` change, `just dev` builds
   incrementally.
   - On success, the running instance hands its state to the new binary over
     the dev control socket, in memory and never through files. The new binary
     restores open buffers, including unsaved contents, cursors, window layout
     and scroll positions.
   - On failure, the old instance keeps running and shows the build errors.
3. **Fast incremental builds** are a budget: the tip crate must rebuild in
   2 s or less. The tools are the crate split, a dev profile that optimizes
   dependencies but not oso, a fast linker and optionally cranelift.
4. Commands are plain functions over host-owned state and hold no globals.
5. Hot reload and state handoff exist only behind a dev cargo feature, and CI
   checks that release builds exclude it.

## Consequences

- Any change — logic, keys or visuals — reaches the screen in about 2–3 s
  without losing state.
- Fast restart doubles as a startup-time discipline, since restart cost is
  startup cost.
- State handoff code can later grow into a user-facing session restore.

## Alternatives considered

- **Dynamic library reloading (hot-lib-reloader, libloading):** rejected.
  Even with one toolchain, struct layout changes, duplicated statics (logger,
  allocator, thread-locals), dangling function pointers and vtables, and
  per-OS unload quirks cause silent crashes that the compiler cannot catch.
  Struct changes need a restart anyway, so the only gain over
  restart-and-restore is about a second.
- **subsecond (Dioxus hot-patching):** sub-second, state-preserving patches.
  However, it is experimental, patches only the tip crate, cannot change
  struct layouts, has caveats with statics and needs an external driver. It is
  kept as an optional later experiment; restart-and-restore stays as the
  fallback.
