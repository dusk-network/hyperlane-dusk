# CI/Repro Bootstrap Decisions

This document records the security and operations decisions for publishing the
Dusk Hyperlane manual repro workflow on the default branch. The dispatcher is
repro evidence infrastructure, not production sign-off by itself.

## Trust boundary

GitHub pull-request validation is split into two jobs with different authority:

- `Dusk proposal validation` runs the proposed tree on an unprivileged GitHub
  hosted runner with read-only contents permission and no repository secrets.
- `Dusk review policy gate` runs only the workflow from the trusted default
  branch under `pull_request_target`. It fetches the exact event head as Git
  data, checks its diff and required file modes, and never checks out or runs
  proposed scripts.

The trusted gate waits for the unprivileged check on the exact proposed commit.
It also requires the proposal-validation and trusted-policy workflow files to
be byte-for-byte unchanged from the base. This prevents a PR from weakening a
policy workflow and then using that replacement to certify itself.

The implementation bootstrap introduces `production-readiness-gate.yml` as a
trusted `pull_request_target` workflow because it may receive status-read
secrets. The bootstrap gate requires it to be a regular file; after it exists
on the base, the trusted policy locks it byte-for-byte as well. It checks out
only the base commit and never executes proposed readiness scripts.

The first merge is necessarily a bootstrap because `main` does not yet contain
the trusted workflow. It must be a focused workflow-policy PR, pass actionlint
and its unprivileged checks, and receive owner review. Later changes to either
policy workflow use another focused, owner-supervised bootstrap: review the
exact diff, use the documented admin bypass only for that policy update, and
immediately verify that branch protection again requires `Dusk review policy
gate`. Normal implementation PRs must not modify either locked workflow.

The empty-script bootstrap branch does not infer scope from an empty worklist.
It permits only the actionlint configuration, the four workflow-policy files,
and this decision record. Any contract, runtime, test, or unrelated document
change fails the proposal check even before the guard scripts exist on `main`.

## Exact evidence inputs

All three dispatcher inputs are required 40-character commit SHAs. There are no
branch-name or moving commit defaults. The job rejects non-SHA inputs, verifies
that every checkout resolved to the requested commit, labels all requested and
resolved values in the log, and writes the same mapping to the job summary.

This removes ambiguity about the Dusk, private Rusk, and Hyperlane monorepo
trees used for a release-evidence run. The workflow URL and all three resolved
SHAs still need to be copied into `TEST_REPORT.md` before the run is cited.

## Runner and secret contract

The repro job requires labels `self-hosted`, `linux`, `dusk-hyperlane`, and
`dusk-hyperlane-ephemeral`, plus the protected
`dusk-hyperlane-repro` environment. Dusk administrators must configure that
label only on one-job ephemeral runner registrations and require an environment
reviewer before the checkout secret is released.

The runner image must already provide `git`, `make`, `rg`, `rustup`, the Rust
toolchain pinned by `rust-toolchain.toml`, and its
`wasm32-unknown-unknown` target. The workflow checks these prerequisites before
running the repro. `DUSK_ORG_READ_TOKEN` is limited to read-only source checkout
for the three Dusk repositories and must not contain signing, deployment,
validator, relayer, image-publishing, or status-administration authority.

No artifacts are uploaded. `persist-credentials: false` is set on every
checkout. The ephemeral runner is discarded after its single job so private
Rusk source, build output, and other workspace residue do not cross runs.

## Pinned validation tooling

The official checkout and GitHub-script actions are pinned to immutable commit
SHAs. The actionlint container is pinned to the immutable digest for upstream
`rhysd/actionlint:1.7.12`:

`sha256:b1934ee5f1c509618f2508e6eb47ee0d3520686341fec936f3b79331f9315667`

The dispatcher gate checks each required file before scanning it, distinguishes
"no match" from scanner failure, includes its own workflow in the scan, and
checks the exact-ref/no-default and resolved-SHA reporting invariants.
