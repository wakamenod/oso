# 0027. Theme

- Status: Accepted, amended by 0089 and 0111
- Date: 2026-11-03

## Context

The author uses modus-vivendi in Emacs, with faint syntax colors and a key to
toggle to the light variant. The modus themes guarantee readability by
construction:

- every intended foreground and background pair has a contrast ratio of at
  least 7:1 (WCAG AAA)
- there are variants for color vision deficiencies
- a two-layer palette: named colors, then semantic mappings that point at
  those names

The modus themes are GPL. oso is MIT OR Apache-2.0
([ADR 0020](0020-license.md)).

## Decision

- oso ships its **own** dark and light themes, built on the same principles.
  No code or palette data is copied from the modus themes.
- **Two layers:**
  - a palette of named colors (`bg-main`, `fg-main`, `red`, `blue-warmer`
    and so on)
  - semantic mappings (keyword, comment, string, region, modeline active and
    so on) that refer to palette names
- Users customize themes by overriding palette entries or mappings in TOML
  ([ADR 0019](0019-configuration.md)).
- **Contrast is enforced in CI.** A test computes the WCAG contrast ratio of
  every intended foreground and background pair and fails below 7:1.
- Syntax colors are **subdued by default**, matching the author's current
  "faint" setting.
- One key toggles between dark and light. The default is dark.
- Variants for color vision deficiencies can be added later as palette
  overrides.

## Consequences

- Readability does not regress as themes change, because CI checks it.
- Designing a palette that meets 7:1 everywhere takes effort up front.

## Alternatives considered

- **Porting the modus themes directly:** familiar, but GPL.
- **Dark only:** less work, but loses the light variant the author toggles
  to.
- **A 4.5:1 (WCAG AA) minimum:** allows softer palettes, but is less
  readable.
