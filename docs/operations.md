# Operations

How to handle keys and secrets, fund dispatch fees, keep the relaying fees
priced and collect them, and recover from uncertain transactions when
running Hyperlane on Dusk. The local demo uses deterministic development keys
and is not a template for production.

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

ProtocolFee and IGP hold the DUSK they collect until their beneficiary
claims it ([Collecting the fees](#collecting-the-fees)).

## Relaying fees

Relaying costs the operator gas on the destination chain: about 141,000
Sepolia gas for a transfer from Dusk, about 2.14M Dusk gas for one to Dusk.
Hyperlane operators get that back the way Hyperlane itself does. Each
message pays an InterchainGasPaymaster (IGP) on its origin chain, in the
origin's token, for the gas its delivery takes, at a price the operator
sets with a margin on top, and the relayer only delivers messages that paid
for enough gas. The operator then claims what the IGPs collected. This
deployment runs an IGP on each side:

| | Dusk IGP | EVM IGP |
|---|---|---|
| Lives on | Dusk, the Mailbox's default hook | Sepolia, in the EVM route's hook |
| Charges | DUSK, for EVM gas | ETH, for Dusk gas |
| Priced by | its own per-domain pricing | its StorageGasOracle, and its own overhead |
| Owner | the Dusk deployer | the EVM deployer (IGP and oracle) |
| Paid out to | its beneficiary account, which claims | its beneficiary address, by anyone's `claim()` |

Separately, every Dusk dispatch pays the Dusk ProtocolFee hook 0.001 DUSK.
That fee isn't for gas. It is income for its beneficiary, as Hyperlane's
own ProtocolFee hook is on chains where it charges one, and it makes each
message cost something on top of its gas. On Sepolia, the Mailbox's
ProtocolFee is Hyperlane's, so its fee (1 wei) goes to Hyperlane.

How the prices and the relayer's requirement are worked out, what the
defaults charge against what delivery costs, and what stays uncovered:
[costs.md](costs.md#fee-coverage).

### Keeping the prices current

The IGPs charge what they were last set to. When the ETH/DUSK price or the
EVM gas price moves, update `ETH_DUSK_PRICE_RATIO` or `EVM_GAS_PRICE_WEI`
in `deploy/testnet.env`, then:

```bash
bash deploy/testnet.sh fees                 # what the new settings charge
bash deploy/testnet.sh dusk-igp             # dry run: on chain now, and the new pricing
bash deploy/testnet.sh dusk-igp --apply     # owner transaction on Dusk (dusk-tx set-igp-config)
bash deploy/testnet.sh evm-igp              # the same for the EVM IGP
bash deploy/testnet.sh evm-igp --apply      # owner transactions on Sepolia
```

The dry runs print the pricing on chain and the new one, each with what a
warp transfer pays under it, and send nothing. `preflight` and
`agent-configs` stop while an IGP's pricing differs from the settings.
After changing `OPERATOR_MARGIN_BPS`, `EVM_IGP_GAS_OVERHEAD` or
`DUSK_RELAYER_GAS_LIMIT`, run `agent-configs` again and restart the
relayer: its requirements follow them.

A margin absorbs moves until the next update. At the default 50%, EVM gas
or the ETH/DUSK price can rise by half before a warp transfer from Dusk
costs more to deliver than it paid. Watch both, and the relayer's spending
against what the IGPs collect.

### Collecting the fees

```bash
bash deploy/testnet.sh revenue   # what each fee contract holds, and for whom
bash deploy/testnet.sh claim     # claim it on both chains
```

On **Dusk**, the IGP and the ProtocolFee each pay their claimable balance to
their beneficiary, and only the beneficiary can claim it, by signing the
claim itself (the contracts check that the signer's account is the
beneficiary and pay that account). `claim` does this when `DUSK_KEYS` is the
beneficiary; any other beneficiary runs:

```bash
dusk-tx claim-fees --rues-url <node> --keys beneficiary.keys --contract <igp or protocol_fee id>
```

`claim-fees` checks the signer is the beneficiary and there is something
to claim before it signs, since a failed claim pays its whole gas limit; a
claim takes about 1.87M gas. The deployer is the beneficiary unless
`DUSK_FEE_BENEFICIARY` names another account, which `deploy` sets on both
contracts. The owner can change it at any time:

```bash
dusk-tx set-beneficiary --rues-url <node> --keys owner.keys --contract <id> \
    --beneficiary <base58 address>
```

On **Sepolia**, the IGP holds its payments as ETH and `claim()` sends all of
it to its beneficiary, `EVM_IGP_BENEFICIARY` (by default the deployer).
Anyone may call it:

```bash
cast send <evm igp> "claim()" --rpc-url $EVM_RPC --private-key <any funded key>
cast send <evm igp> "setBeneficiary(address)" <address> --rpc-url $EVM_RPC --private-key <owner key>
```

The addresses are in `deploy/out/<DEPLOY_NAME>/deployment.json`: `.dusk.igp`,
`.dusk.protocol_fee`, `.evm.igp`.

## Uncertain transactions

Every Dusk write prints its locally computed transaction hash before it is
submitted. If a command is interrupted or reports an unknown outcome, do not
resend it: look up that hash on the node first. The agents do this themselves,
reconciling against the hash they prepared before retrying.

A `dusk-tx` write succeeds once the transaction is included in a block and
executed without error. Inclusion isn't final: until its block is final, a
revert can drop the transaction or run it again in another block, with
another result. The output says which it was:

- `block_height` and `gas_spent`: where the transaction was included, and
  what it cost.
- `finalized`: whether its block was final when `dusk-tx` checked, right
  after inclusion. `block_hash` is set once it is, and `null` until then.

Pass `--wait-finality` to wait until the block is final, for at most
`--finality-timeout` seconds (300 by default) from submission. An execution
error is then only reported once its block is final too, and a timeout is an
unknown outcome with the hash to reconcile. `deploy-hyperlane` waits for its
last deploy, which can only be final once every earlier one is, and then
checks that each earlier one still executed. The agents don't pass the flag:
they only act on finalized events and reconcile their own transactions.

## CI

`.github/workflows/ci.yml` runs on every pull request: workflow lint,
whitespace, shell syntax, the demo lifecycle tests, the deploy script tests
(`make test-deploy-scripts`: `deploy/testnet.sh` and
`demo/gen-agent-configs.sh` against a stubbed `dusk-tx` and a fake EVM chain) and the
secret-hygiene scan. It can't build the contracts, because they depend on
the private Rusk repository.

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
