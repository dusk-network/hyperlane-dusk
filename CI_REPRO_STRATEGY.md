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
- A 2026-05-12 permission probe showed repo-level Actions is enabled on both
  `dusk-network/hyperlane-dusk` and `dusk-network/hyperlane-monorepo`, with
  allowed actions set to `all` and default workflow permissions set to `write`.
  The current CI blocker is runner/secret/workflow provisioning, not disabled
  repo-level Actions. The probe is recorded in
  dusk-network/hyperlane-dusk#8.
- `dusk-network/hyperlane-dusk` uses `main` as its default branch. The manual
  repro workflow is currently introduced by the `feat/dusk-hardening-v2`
  review branch, so it becomes normally discoverable in the GitHub Actions UI
  after the workflow file is merged or otherwise added to the default branch.
- A narrow default-branch dispatcher PR exists as
  dusk-network/hyperlane-dusk#3. It contains only
  `.github/workflows/manual-repro-check.yml` and `.github/actionlint.yaml`, so
  Dusk can make the workflow visible without first merging the full Hyperlane
  implementation PR.

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

For local reviewer repros that already have a clean Rusk checkout somewhere
else, the same script accepts an override and creates a temporary compatible
layout automatically:

```bash
RUSK_DIR=/path/to/clean/rusk-private make repro-check-agent
```

Set `HYPERLANE_DUSK_REPRO_WORKDIR=/tmp/some-dir` to keep that temporary layout
and its logs after the run.

Reviewers can also print the current machine-checkable gate state without
running the heavy build/test repro:

```bash
make gate-status
make gate-status-fresh
```

`make gate-status` calls `scripts/release-gate-status.sh`, which reports local
worktree state, untracked source status, implementation PR and
workflow-dispatcher PR mergeability/review/status state, production sign-off
checklist counts, split decision issue states, default-branch protection and
merge method settings, workflow visibility, CI provisioning visibility,
reviewer-facing evidence-link visibility, Dusk Dependabot open-alert
visibility, local `Cargo.lock` first-patched-floor comparison through
`make dependency-alert-status`, reviewer-facing `make gate-status-fresh` and
`make dependency-alert-status` handoff visibility, Hyperlane upstream drift,
and Dusk runtime placeholder scans.
`make gate-status-fresh` first fetches Hyperlane `upstream/main` before
reporting drift. Neither command closes any production gates.

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

- `dusk_ref`: defaults to `feat/dusk-hardening-v2`.
- `rusk_ref`: defaults to clean Rusk reference
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- `monorepo_ref`: defaults to `feat/dusk-support-v2`.

Set `dusk_ref` to a PR branch or exact commit SHA when running the workflow
from the default branch, so the repro checks the review head rather than the
default branch contents.

Once the workflow exists on the default branch, either by merging
dusk-network/hyperlane-dusk#3 or by another Dusk-approved equivalent, and the
runner/token are available, a reviewer can dispatch the current internal
review shape with:

```bash
gh workflow run manual-repro-check.yml \
  --repo dusk-network/hyperlane-dusk \
  --ref main \
  -f dusk_ref=feat/dusk-hardening-v2 \
  -f rusk_ref=c0c64db4659500d077bb253ad13acba0e347d3fc \
  -f monorepo_ref=feat/dusk-support-v2
```

For release evidence, prefer exact commit SHAs in those three inputs and record
the resolved heads in `TEST_REPORT.md`. The workflow prints each requested ref
and resolved checkout head before running the repro command so reviewers can
copy the exact Dusk, Rusk, and monorepo SHAs from the Actions log.

To dispatch against exact current internal review heads without hard-coding a
Dusk SHA that becomes stale after docs-only commits, resolve the PR heads first:

```bash
dusk_ref="$(gh pr view 1 --repo dusk-network/hyperlane-dusk --json headRefOid --jq .headRefOid)"
monorepo_ref="$(gh pr view 1 --repo dusk-network/hyperlane-monorepo --json headRefOid --jq .headRefOid)"
rusk_ref="c0c64db4659500d077bb253ad13acba0e347d3fc"

gh workflow run manual-repro-check.yml \
  --repo dusk-network/hyperlane-dusk \
  --ref main \
  -f dusk_ref="$dusk_ref" \
  -f rusk_ref="$rusk_ref" \
  -f monorepo_ref="$monorepo_ref"
```

Expected resolved heads:

- Dusk: the live head of dusk-network/hyperlane-dusk#1.
- Rusk: `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Monorepo: the live head of dusk-network/hyperlane-monorepo#1.

Earlier local review-head E2E evidence tested Dusk
`2ac225175b15aac465d100e748ba68f8b14bd545`; commit
`11f6744bb3514f96db846de1108378e43d161a3e` recorded that evidence in
`TEST_REPORT.md` and `GOAL_AUDIT.md`, and later docs commits may move the Dusk
PR head again. The manual workflow should be dispatched against the live PR
heads resolved above so CI evidence matches the PR headers exactly.

It runs:

```bash
set -euo pipefail
make repro-check-agent
```

The workflow explicitly uses `shell: bash` for this final repro step so the
strict shell flags are applied consistently on the self-hosted runner.

That wraps:

```bash
make all
make clippy-contracts
cargo test -p hyperlane-dusk-types
cargo test -p hyperlane-dusk-integration-tests
cargo test -p dusk-tx
make secret-hygiene
cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p scraper -p lander
```

The final `cargo check` runs from
`hyperlane/hyperlane-monorepo/rust/main`.

Until this workflow file exists on the default branch, reviewers should treat it
as a branch-proposed runner definition plus default-branch dispatcher PR and
use the local equivalent:

```bash
make repro-check-agent
```

This default-branch requirement was checked explicitly on 2026-05-12. A direct
dispatch attempt against the feature branch failed before scheduling a run:

```bash
gh workflow run manual-repro-check.yml \
  --repo dusk-network/hyperlane-dusk \
  --ref feat/dusk-hardening-v2 \
  -f dusk_ref=2de3d22b811eda9762599bbbb5c07d2f2fdad52e \
  -f rusk_ref=c0c64db4659500d077bb253ad13acba0e347d3fc \
  -f monorepo_ref=ecb11359747dce240a24c50fa229afd4479919b5
```

Result:

```text
HTTP 404: Not Found (https://api.github.com/repos/dusk-network/hyperlane-dusk/actions/workflows/manual-repro-check.yml)
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
- generated configs containing `hexKey`, inline raw keys, or secret-like Dusk
  key files

Logs may be uploaded only after the exact files pass the hygiene check.

## Promotion Path

Recommended sequence:

1. Keep the workflow manual until the Dusk runner and token scope are accepted.
2. Run it once against the current Dusk PR head and monorepo branch.
3. Record the workflow URL, `dusk_ref`, resolved Dusk head, `monorepo_ref`,
   resolved monorepo head, `rusk_ref`, resolved Rusk head, and pass/fail result
   in `TEST_REPORT.md`.
4. Only after the manual run is stable, decide whether to make the non-E2E
   repro check required on internal PRs.
5. Keep live E2E, fault-injection, and soak runs separate unless Dusk provides
   a runner specifically intended for long-running local network tests.

## Open Decision

Dusk still needs to decide whether this self-hosted workflow is the accepted
CI/repro strategy for internal review, or whether another private CI system
should own these checks. That decision remains tracked in
https://github.com/dusk-network/hyperlane-dusk/issues/8 and rolls up into the
production sign-off tracker at
https://github.com/dusk-network/hyperlane-dusk/issues/2.
