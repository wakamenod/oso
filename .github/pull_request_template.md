## Summary

<!-- What changes, and why. -->

## Verification

<!--
How it was checked: tests, `just ci`, the verify-in-app skill. A new check
shows a deliberately broken example failing.
-->

## Structural change

<!--
These need an ADR (docs/dev/adr/0038-architecture-guardrails.md):
- a new crate, or a new edge between crates
- a change to guardrails/layers.txt
- an addition to guardrails/size-allowlist.txt
- a new public trait or public module in a library crate (it shows up as a
  new `pub mod` or `pub trait` line in guardrails/public-api)
- a change to the architecture checks (crates/oso/tests) or the size
  thresholds
CI fails unless this description cites the ADR as "ADR" followed by its
four-digit number. An existing ADR that the change implements is enough.
-->

- [ ] No structural change.
- [ ] Structural change, decided in: <!-- the ADR -->
