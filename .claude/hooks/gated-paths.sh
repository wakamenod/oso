# Sourced by the hooks. Files that need human approval (AGENTS.md), as an
# extended regex over paths relative to the checkout.
# shellcheck disable=SC2034 # used by the scripts that source this file
gated_paths='(^|/)(Cargo\.(toml|lock)|deny\.toml|\.?clippy\.toml|rust-toolchain(\.toml)?|[Jj]ustfile|\.gitleaks[^/]*)$|(^|/)\.cargo/|^\.github/|^guardrails/|^crates/oso/tests/(architecture/|workspace_lints\.rs$)|^crates/oso/examples/perf/(compare|bench)\.rs$|^\.claude/(settings[^/]*\.json|hooks/|skills/|agents/|commands/)'
