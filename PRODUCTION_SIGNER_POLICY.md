# Production Signer Policy Proposal

This document scopes the remaining signer-custody decision for Dusk Hyperlane.
It is a proposal for Dusk review, not an accepted production policy.

## Current Implementation

The current Hyperlane monorepo branch adds a Dusk signer type named
`duskKey`. In `hyperlane-base`, `SignerConf::DuskKey` stores a raw 32-byte
BLS key, and the Dusk signer builder accepts that variant only.

Current implication:

- Dusk relayer/validator transaction signing can work from generated local
  agent configs.
- Those configs contain raw signer material and must not be committed or
  uploaded as artifacts.
- Dusk production custody is not equivalent to existing EVM AWS signer support;
  a Dusk external signer, KMS signer, or node signer path would need explicit
  implementation and review.

## Non-Production Defaults

The demo and E2E scripts use deterministic local development material:

- Anvil private keys for the local EVM side.
- Encrypted local `consensus.keys` for Dusk deployer/test transactions.
- Generated Hyperlane relayer/validator JSON files under `/tmp`.
- Local `duskKey` entries generated only for local agent runs.

These defaults are acceptable only for local development and reproducibility.

## Production Requirements

Dusk production deployments should satisfy all of the following before a
production-readiness claim:

- No raw Dusk signer key in git, PR bodies, issue bodies, logs, or uploaded CI
  artifacts.
- No Dusk consensus password in process argv.
- No generated Hyperlane signer config archived from `/tmp`.
- Runtime config files containing raw keys, if used at all, must be generated
  on the target host from a Dusk-approved secret source and removed during
  teardown or rotation.
- Files containing signer material must be readable only by the agent user
  (`0600` file mode or stricter equivalent).
- Relayer and validator keys should be distinct unless Dusk explicitly accepts
  shared-key operation for a test environment.
- Mainnet keys should be distinct from testnet/devnet keys.
- Key rotation and emergency disablement must be documented by operations
  before launch.

## Acceptable V1 Choices

Dusk reviewers should choose one of these paths before production use.

### Option A: Ephemeral Local Key File

Use the existing `duskKey` implementation, but generate the agent config at
runtime from an approved secret manager or sealed host secret.

Required controls:

- The raw key never appears in committed config.
- The generated config is written with restrictive permissions.
- The generated config path is excluded from artifact collection.
- `scripts/secret-hygiene-check.sh` scans any logs or files before upload.
- The runner/host is treated as sensitive infrastructure.

This is the smallest v1 operational path, but it still places raw key material
on the agent host.

### Option B: External Dusk Signer

Add a new Dusk signer mode to the Hyperlane monorepo branch so agents ask an
external service or node to sign Dusk transactions without loading the raw BLS
secret key into the agent config.

Required engineering work:

- New signer config variant and parser behavior.
- Dusk signer implementation in `hyperlane-dusk`.
- Authentication and authorization between agent and signer service.
- Tests covering signing failures, signer unavailability, wrong public key,
  and retry behavior.
- Secret-hygiene updates for the new config shape.

This is a stronger custody path, but it is not implemented in the current
branch.

### Option C: Block Production Until KMS/HSM Support Exists

Keep the current branch limited to internal review, devnet, and testnet until
Dusk has a reviewed KMS/HSM or equivalent remote signer story for BLS
Moonlight transaction signing.

This is the most conservative path if raw key presence on relayer/validator
hosts is unacceptable.

## Recommended Review Decision

For internal review and testnet-style validation, accept Option A only with the
controls listed above.

For mainnet production, Dusk should explicitly decide whether Option A is
acceptable for its validator/relayer threat model. If not, Option B or C should
be required before any production-readiness claim.

## Verification Hooks

Existing guardrails:

```bash
make secret-hygiene
bash scripts/secret-hygiene-check.sh <artifact-path>...
```

These guardrails detect common source and artifact leaks, including generated
`duskKey` and `hexKey` config markers. They do not replace the custody
decision above.

The open decision remains tracked in
https://github.com/dusk-network/hyperlane-dusk/issues/2.
