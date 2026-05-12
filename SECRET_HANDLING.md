# Secret Handling

This repository is safe to use for deterministic local development, but the
demo keys are not production keys.

## Policy

- Do not pass Dusk consensus key passwords through process argv.
- Prefer `DUSK_CONSENSUS_PASSWORD_FILE` for production-style local testing.
- Use `DUSK_CONSENSUS_PASSWORD` or `DUSK_CONSENSUS_KEYS_PASS` only on trusted
  local runners where environment variables are not collected as artifacts.
- Use `--secret-key-stdin` when a raw BLS secret key is unavoidable.
- Do not commit or upload generated Hyperlane agent configs. Local demo configs
  point at Dusk key files and still contain Anvil signer material.
- Do not upload `/tmp/hyperlane-relayer-*.json`,
  `/tmp/hyperlane-validator-*.json`, `demo/.env*`, `e2e/consensus.keys`,
  `*.key`, `*.keys`, or password files as CI artifacts.
- Dusk `duskKey.keyFile` paths must point at regular files with no group/world
  permissions on Unix.
- E2E wrappers that call `demo/gen-agent-configs.sh` remove generated Dusk
  signer key files on exit after stopping any running agents. Logs and
  non-secret path references may remain for debugging.

`PRODUCTION_SIGNER_POLICY.md` records the current `duskKey` signer-file/env
support and the signer-custody choices that Dusk must accept, change, or
replace before production use.

## Manual Repro Workflow

`.github/workflows/manual-repro-check.yml` is `workflow_dispatch` only and is
intended for a Dusk-controlled self-hosted runner. It needs
`DUSK_ORG_READ_TOKEN` only to checkout private Dusk repositories.

`CI_REPRO_STRATEGY.md` records the proposed runner labels, checkout layout,
token scope, artifact policy, and promotion path for reviewers to accept,
change, or replace.

`DUSK_ORG_READ_TOKEN` must be a read-only repository/org token scoped to source
checkout. It must not be a Dusk signer, validator key, consensus key password,
or deployment secret.

The workflow sets `persist-credentials: false` on checkout steps so the token is
not left in local git config while the repro command runs.

Do not enable artifact upload for the manual repro workflow unless the exact
files are scanned first with `scripts/secret-hygiene-check.sh`.

## Checks

Run the source hygiene check before review:

```bash
make secret-hygiene
```

Before uploading CI or E2E artifacts, scan the exact files or directories:

```bash
bash scripts/secret-hygiene-check.sh /tmp/hyperlane-*.log
```

The artifact scan intentionally fails on inline raw key fields, `hexKey`
signer config markers, and secret-like files such as `*.key`, `*.keys`, and
`*.pem`. Generated agent configs and Dusk signer key files should be kept on
the runner and deleted after the run, not archived.

The check is conservative: it is meant to guard release packaging and CI
artifact upload steps, not to bless production custody. Production signer
storage still needs an explicit Dusk operational decision tracked in
https://github.com/dusk-network/hyperlane-dusk/issues/2.
