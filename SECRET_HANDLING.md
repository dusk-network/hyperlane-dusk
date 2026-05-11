# Secret Handling

This repository is safe to use for deterministic local development, but the
demo keys are not production keys.

## Policy

- Do not pass Dusk consensus key passwords through process argv.
- Prefer `DUSK_CONSENSUS_PASSWORD_FILE` for production-style local testing.
- Use `DUSK_CONSENSUS_PASSWORD` or `DUSK_CONSENSUS_KEYS_PASS` only on trusted
  local runners where environment variables are not collected as artifacts.
- Use `--secret-key-stdin` when a raw BLS secret key is unavoidable.
- Do not commit or upload generated Hyperlane agent configs. They contain local
  signer material for Dusk and Anvil.
- Do not upload `/tmp/hyperlane-relayer-*.json`,
  `/tmp/hyperlane-validator-*.json`, `demo/.env*`, `e2e/consensus.keys`,
  `*.keys`, or password files as CI artifacts.

## Checks

Run the source hygiene check before review:

```bash
make secret-hygiene
```

Before uploading CI or E2E artifacts, scan the exact files or directories:

```bash
bash scripts/secret-hygiene-check.sh /tmp/hyperlane-*.log
```

The artifact scan intentionally fails on signer config markers such as
`"type": "duskKey"` and `"type": "hexKey"`. Generated agent configs should be
kept on the runner and deleted after the run, not archived.

The check is conservative: it is meant to guard release packaging and CI
artifact upload steps, not to bless production custody. Production signer
storage still needs an explicit Dusk operational decision tracked in
https://github.com/dusk-network/hyperlane-dusk/issues/2.
