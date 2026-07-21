# Reviewer Routing

This file is an advisory handoff for the Dusk Hyperlane review. It is not a
GitHub `CODEOWNERS` file, does not configure branch protection, and does not
replace production sign-off in dusk-network/hyperlane-dusk#2.

## Active Review Entry Points

| Surface | Review target | Primary reviewer route |
|---|---|---|
| Dusk contracts, tooling, E2E, audit notes | dusk-network/hyperlane-dusk#1 | `moCello` |
| Hyperlane Rust agent/runtime integration | dusk-network/hyperlane-monorepo#1 | `Neotamandua` |
| Manual repro workflow/default-branch dispatcher | dusk-network/hyperlane-dusk#3 | `moCello`, `Neotamandua` |
| Production sign-off tracker | dusk-network/hyperlane-dusk#2 | Dusk release reviewers |

## Split Decision Routing

| Issue | Decision | Reviewer route |
|---|---|---|
| dusk-network/hyperlane-dusk#4 | Mailbox transfer-contract sender resolution | `moCello`, `Neotamandua` |
| dusk-network/hyperlane-dusk#5 | Immutable `registered_accounts` registration | `moCello`, `Neotamandua` |
| dusk-network/hyperlane-dusk#6 | Pending escrow recovery policy | `moCello`, `Neotamandua` |
| dusk-network/hyperlane-dusk#7 | Production signer custody and CI artifact policy | `moCello`, `Neotamandua` |
| dusk-network/hyperlane-dusk#8 | CI/repro runner policy | `Neotamandua` |
| dusk-network/hyperlane-dusk#9 | Soak acceptance for release | `moCello` |

## Release Gate

Upstream Hyperlane draft PR preparation should wait until:

- dusk-network/hyperlane-dusk#1 and dusk-network/hyperlane-monorepo#1 complete
  internal Dusk review.
- dusk-network/hyperlane-dusk#2 is signed off by Dusk reviewers.
- Split decision issues dusk-network/hyperlane-dusk#4 through
  dusk-network/hyperlane-dusk#9 are resolved or explicitly deferred by Dusk.
- The CI/repro runner policy in dusk-network/hyperlane-dusk#8 is accepted or
  replaced.
