# CI And Repro Strategy

This document turns the current local verification into an explicit Dusk
runner proposal. It is not a production sign-off; it gives reviewers a concrete
setup to accept, change, or replace.

## Current State

- The Dusk contract/tooling workspace depends on private Rusk path
  dependencies from an adjacent `rusk-private` checkout.
- The Hyperlane agent check depends on an adjacent
  `hyperlane-monorepo` checkout.
- Public GitHub-hosted runners cannot reproduce the full workspace without
  private Dusk repository access.
- Both internal PRs currently have empty GitHub status-check rollups; the
  evidence in `TEST_REPORT.md` is local/clean-Rusk evidence.
- `dusk-network/hyperlane-dusk` uses `main` as its default branch. The manual
  repro workflow is currently introduced by the `feat/dusk-hardening-v2`
  review branch, so it becomes normally discoverable in the GitHub Actions UI
  after the workflow file is merged or otherwise added to the default branch.

## Proposed Runner

Use a Dusk-controlled self-hosted Linux runner with the labels:

```text
self-hosted, linux, dusk-hyperlane
```

Runner requirements:

- Rust toolchain compatible with this repository's `rust-toolchain.toml`.
- `wasm32-unknown-unknown` target installed.
- `make`, `bash`, `git`, and `cargo` available on `PATH`.
- Enough CPU, memory, and disk for contract WASM builds, VM integration tests,
  and the Hyperlane Rust agent `cargo check`.
- No automatic artifact upload from the working directory or `/tmp`.
- Workspace cleanup after each run, including generated Hyperlane relayer and
  validator configs.

## Repository Layout

The manual workflow checks out repositories into the same relative layout used
by local development:

```text
<runner-workspace>/
  rusk-private/
  hyperlane/
    dusk/
    hyperlane-monorepo/
```

This layout matches `scripts/local-repro-check.sh`, whose default Rusk path is
`../../rusk-private` relative to `hyperlane/dusk`.

## Access Token

Use one GitHub secret:

```text
DUSK_ORG_READ_TOKEN
```

Required scope:

- Read-only access to `dusk-network/rusk-private`.
- Read-only access to `dusk-network/hyperlane-monorepo` if the workflow runs
  from `dusk-network/hyperlane-dusk`.

Forbidden uses:

- Do not use this token as a Dusk signer, validator key, consensus key
  password, deployment key, or relayer key.
- Do not persist it in local git config. The workflow sets
  `persist-credentials: false` on all checkout steps.

## Manual Workflow

The current workflow is:

```text
.github/workflows/manual-repro-check.yml
```

It is `workflow_dispatch` only and accepts:

- `rusk_ref`: defaults to clean Rusk reference
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- `monorepo_ref`: defaults to `feat/dusk-support-v2`.

It runs:

```bash
make repro-check-agent
```

That wraps:

```bash
make all
cargo test -p hyperlane-dusk-types
cargo test -p hyperlane-dusk-integration-tests
cargo test -p dusk-tx
make secret-hygiene
cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p scraper -p lander
```

The final `cargo check` runs from
`hyperlane/hyperlane-monorepo/rust/main`.

Until this workflow file exists on the default branch, reviewers should treat it
as a branch-proposed runner definition and use the local equivalent:

```bash
make repro-check-agent
```

## Artifact Policy

Default policy: upload no artifacts.

Before any artifact upload is enabled, scan the exact file set:

```bash
bash scripts/secret-hygiene-check.sh <artifact-path>...
```

Never upload:

- `/tmp/hyperlane-relayer-*.json`
- `/tmp/hyperlane-validator-*.json`
- `demo/.env*`
- `e2e/consensus.keys`
- `*.keys`
- password files
- generated configs containing `duskKey`, `hexKey`, or raw private keys

Logs may be uploaded only after the exact files pass the hygiene check.

## Promotion Path

Recommended sequence:

1. Keep the workflow manual until the Dusk runner and token scope are accepted.
2. Run it once against the current Dusk PR head and monorepo branch.
3. Record the workflow URL, Dusk head, monorepo head, Rusk ref, and pass/fail
   result in `TEST_REPORT.md`.
4. Only after the manual run is stable, decide whether to make the non-E2E
   repro check required on internal PRs.
5. Keep live E2E, fault-injection, and soak runs separate unless Dusk provides
   a runner specifically intended for long-running local network tests.

## Open Decision

Dusk still needs to decide whether this self-hosted workflow is the accepted
CI/repro strategy for internal review, or whether another private CI system
should own these checks. That decision remains tracked in
https://github.com/dusk-network/hyperlane-dusk/issues/2.
