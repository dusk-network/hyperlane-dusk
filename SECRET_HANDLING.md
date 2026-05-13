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
  signer key files and generated agent config files on exit after stopping any
  running agents. Logs and non-secret path references may remain for debugging.

`PRODUCTION_SIGNER_POLICY.md` records the current `duskKey` signer-file/env
support and the signer-custody choices that Dusk must accept, change, or
replace before production use.

## Manual Repro Workflow

`.github/workflows/manual-repro-check.yml` is `workflow_dispatch` only and is
intended for a Dusk-controlled self-hosted runner. It needs
`DUSK_ORG_READ_TOKEN` only for read-only source checkout of the internal repos
used by the repro: `dusk-network/hyperlane-dusk`,
`dusk-network/hyperlane-monorepo`, and `dusk-network/rusk-private`.

`CI_REPRO_STRATEGY.md` records the proposed runner labels, checkout layout,
token scope, artifact policy, and promotion path for reviewers to accept,
change, or replace.

`DUSK_ORG_READ_TOKEN` must be a read-only repository/org token scoped to source
checkout. It must not be a Dusk signer, validator key, consensus key password,
deployment secret, relayer key, or image-publishing credential.

Do not add branch-protection, Actions-secret, runner-admin, or Dependabot-alert
read scopes to `DUSK_ORG_READ_TOKEN` unless Dusk explicitly changes the token
policy. Those APIs are production-readiness visibility gates, not source
checkout requirements. Use an approved admin/security-read local `gh`
credential or a separate Dusk-approved CI credential for those status checks if
Dusk wants the GitHub Actions guard to see them.

If Dusk chooses a CI credential for status visibility, store it separately as
`DUSK_STATUS_READ_TOKEN` and use it only in the production-readiness workflow.
It must not be used by manual repro checkout steps and must not carry Dusk
runtime, signer, validator, consensus, deployment, relayer, or image-publishing
authority.

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

Before linking or relying on GitHub PR/issue review text as release evidence,
export and scan the review surfaces:

```bash
make review-hygiene
```

This writes a timestamped `/tmp/hyperlane-review-export-*` directory, checks
known stale evidence refs/wording, including stale current-head refs and wrong
clean-repro tested-SHA refs, and runs the same secret hygiene scanner over the
exported PR/issue bodies and comments.

The artifact scan intentionally fails on inline raw key fields in JSON,
TOML, or YAML-style assignments, including `key`, `privateKey`, and
`private_key` fields with 32-byte hex values. It also fails on `hexKey`
signer config markers and secret-like files such as `*.key`, `*.keys`, and
`*.pem`. Generated agent configs and Dusk signer key files should be kept on
the runner and deleted after the run, not archived.

The check is conservative: it is meant to guard release packaging and CI
artifact upload steps, not to bless production custody. Production signer
storage still needs an explicit Dusk operational decision tracked in
https://github.com/dusk-network/hyperlane-dusk/issues/2.
