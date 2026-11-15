# Operations

How to handle keys and secrets, fund dispatch fees, and recover from
uncertain transactions when running Hyperlane on Dusk. The local demo uses
deterministic development keys and is not a template for production.

## Agent signer keys

Relayers and validators sign Dusk transactions with a `duskKey` signer, which
takes exactly one of:

- `keyFile`: path to a file holding the raw BLS key. On Unix it must be a
  regular file with no group or world permissions (`0600` or stricter).
- `keyEnv`: name of an environment variable holding the key.
- `key`: the key inline in the config. Local development only.

Requirements for any real deployment:

- No raw key in git, logs, PR or issue text, or CI artifacts.
- Separate keys for relayer and validator, and for mainnet and test networks.
- A documented rotation and emergency-disable procedure.

The agents currently load the raw key onto the agent host. The options for
production custody are:

1. **Key file written at runtime** from an approved secret manager, with the
   controls above. This works today.
2. **External Dusk signer**: a new signer mode in the agent where an external
   service signs transactions, so the raw key never reaches the agent.
   Not implemented yet.
3. **Wait for KMS/HSM support** for BLS Moonlight signing before running on
   mainnet.

Option 1 is fine for testnet. Mainnet needs an explicit choice.

## dusk-tx secrets

`dusk-tx` is both the deployment CLI and the agents' transaction helper.

- Consensus key passwords are read from `DUSK_CONSENSUS_PASSWORD_FILE`, then
  `DUSK_CONSENSUS_PASSWORD`, then `DUSK_CONSENSUS_KEYS_PASS`. Never pass a
  password as a command-line argument outside local demos.
- A raw BLS secret key goes through `--secret-key-stdin`.
- An authenticated RPC URL goes through `--rues-url-stdin`, which the agents
  use. When both are used, the URL is the first stdin line and the key the
  second.

Never commit or upload generated agent configs, `*.key`, `*.keys`, `.env*`
files or password files. `make secret-hygiene` checks the tracked source.
Before uploading any logs or artifacts, scan them:

```bash
bash scripts/secret-hygiene-check.sh <paths...>
```

## Dispatch fees

Dusk has no `msg.value`. Warp routes charge each transfer its dispatch fee:
`transfer_remote` takes the quoted fee in the sender's Moonlight deposit and
passes it on to the Mailbox, and reverts if the route's own credit changes.
`dusk-tx transfer-remote` quotes the fee and attaches it. Routes therefore
need no prepaid credit.

Senders that call the Mailbox's `dispatch` directly (for example
TestRecipient, or an application contract) pay from credit prepaid to the
Mailbox:

```bash
# Prepay credit (anyone can fund; the payer named owns the credit)
dusk-tx fund-dispatch --mailbox <mailbox> --payer <sender> --amount <lux>

# Withdraw your own unused credit, optionally to a treasury account
dusk-tx withdraw-dispatch --target <mailbox> --amount <lux> \
    --expected-chain-id <id> [--recipient-public-key <96-byte key hex>]

# A route owner withdraws that route's credit
dusk-tx withdraw-dispatch --target <route> --amount <lux> --expected-chain-id <id>
```

The Mailbox decodes the recipient's public key before paying it, so the
DUSK reaches the account that signs with that key even if the key passed
was a forged twin of it. Decoding costs about 57M gas: a withdrawal takes
about 59M, and `withdraw-dispatch` defaults to an 80M gas limit.

Withdraw credit that is no longer needed, such as credit funded for a route
by an older deployment script.

ProtocolFee and IGP hold the DUSK they collect until the owner calls `claim`,
which pays the configured beneficiary.

## Uncertain transactions

Every Dusk write prints its locally computed transaction hash before it is
submitted. If a command is interrupted or reports an unknown outcome, do not
resend it: look up that hash on the node first. The agents do this themselves,
reconciling against the hash they prepared before retrying.

## CI

`.github/workflows/ci.yml` runs on every pull request: workflow lint,
whitespace, shell syntax, the demo lifecycle tests and the secret-hygiene scan.
It can't build the contracts, because they depend on the private Rusk
repository.

`.github/workflows/manual-repro-check.yml` runs the full build and test suite
(`make repro-check-agent`) on demand against exact commits of this repository,
`rusk-private` and `hyperlane-monorepo`. It needs:

- A self-hosted Linux runner labelled `dusk-hyperlane` and
  `dusk-hyperlane-ephemeral`, with `cargo`, `git`, `make`, `rg` and `rustup`.
- A `DUSK_ORG_READ_TOKEN` secret with read-only access to the source
  repositories, and nothing else. It must never carry signer, validator,
  deployment or publishing authority.
- The `dusk-hyperlane-repro` environment.

```bash
gh workflow run manual-repro-check.yml \
  -f dusk_ref=<40-char sha> -f rusk_ref=<40-char sha> -f monorepo_ref=<40-char sha>
```
