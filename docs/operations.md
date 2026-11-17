# Operations

How to handle keys and secrets, fund dispatch fees, keep the relaying fees
priced and collect them, top up a message that paid for too little gas,
recover from uncertain transactions, and how the relayer delivers to Dusk,
when running Hyperlane on Dusk. The
local demo uses deterministic development keys and is not a template for
production.

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

- Consensus key passwords are read from `--password`, then the file
  `DUSK_CONSENSUS_PASSWORD_FILE` names, then `DUSK_CONSENSUS_PASSWORD`, then
  `DUSK_CONSENSUS_KEYS_PASS`: the first one set is used. With none of them,
  `dusk-tx` tries the local demo's password, `password`. Never pass a
  password as a command-line argument outside local demos.
- A raw BLS secret key goes through `--secret-key-stdin`.
- An authenticated RPC URL goes through `--rues-url-stdin`, which the agents
  use. When both are used, the URL is the first stdin line and the key the
  second. With `call --ack-before-send` the key is one line, and the line
  after it is the go-ahead to send ([below](#acknowledged-sends)).

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
Mailbox. A payer with no credit takes at least 0.01 DUSK (10,000,000 LUX):
each payer with credit is an entry in the Mailbox's memory for as long as
its credit lasts. A payer with credit can be topped up by any amount.
Credit that reaches zero, spent by dispatches or withdrawn, leaves no
entry, so funding that payer again takes the minimum again. Routes fund
their own credit for each transfer through the transfer contract, which
has no minimum.

Near 3 GiB of memory (`memory_bytes`) the Mailbox takes no new payers, as
it takes no dispatches; top-ups and withdrawals go on. A new payer's entry
can allocate 13 pages and a dispatch 4 to 10, so new payers stop first:
from 13 pages below 3 GiB to about 4 below, about 9 pages or 17,000 to
22,000 deliveries' worth, payers with credit still dispatch while new ones
are refused with `Mailbox: storage full, dispatch is closing to new
payers`. A route's credit goes back to zero after every transfer, so each
transfer makes it a new payer, and route transfers fail in that window
with the same message. Standing credit for the route, any amount funded to
its contract ID with `fund-dispatch`, lets its transfers through until
dispatch itself closes.

`dusk-tx` checks what the Mailbox would refuse before it signs, since a
failed transaction pays its whole gas limit: `fund-dispatch` reads the
payer's credit and refuses less than the minimum for a payer with none, or
any amount when the Mailbox has no room for a new payer, and
`transfer-remote` refuses a transfer that pays a dispatch fee when the
route has no credit and the Mailbox no room (a free dispatch funds no
credit, so it adds no payer). `drc20-approve` checks a WarpDrc20's limits
the same way for a new spender: at most 64 allowances per owner, and room
under 3 GiB. A token without `allowance_count` isn't a WarpDrc20 and has
neither limit, but a Rusk node answers a query of a function a contract
doesn't have as it answers every query that fails. So `drc20-approve`
takes the token as without it only when that answer comes back twice, a
second apart, with the token's `allowance` returning an amount after each.
Any other answer, or none, stops it before it signs: another status or
body, a reply that isn't the value asked for, or an `allowance` that fails
or returns no amount, an empty reply among them.

```bash
# Prepay credit (anyone can fund; the payer named owns the credit)
dusk-tx fund-dispatch --mailbox <mailbox> --payer <sender> --amount <lux> \
    --expected-chain-id <id>

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

Before they read a price or send anything, `dusk-igp`, `evm-igp`, `claim`
and `revenue` check that the Dusk node is on `DUSK_EXPECTED_CHAIN_ID` and the
EVM RPC on `EVM_CHAIN_ID`: the contract IDs and addresses in the state can
exist on another network, a Dusk ID being derived from the bytecode, the
deployer key and a nonce. `dusk-igp`, `evm-igp` and `claim` also check that
the recorded contracts are the deployment's: the Mailbox at `DUSK_DOMAIN`,
the fee contracts called from its hooks and taking their calls from what
calls them there (the IGP is its default hook, the ProtocolFee in its
required aggregation hook), the Sepolia contracts created as `deploy`
recorded, and the Sepolia token bound to `EVM_MAILBOX` and the recorded
ISM. `dusk-igp` and `claim` read the Dusk contracts back first, as `deploy`
left them ([testnet.md](testnet.md#reading-back-the-dusk-contracts)).
`revenue`, which only reads, checks the chains alone. Each `dusk-tx`
command they run is bound to the chain too (below).

`evm-igp` first checks that the Sepolia IGP reads Dusk's pricing from the
StorageGasOracle `deploy` recorded, and stops before sending anything when it
reads from another one: only the owner key can have changed that, so find
out why first. `--apply` prices the recorded oracle and reads it back before
it points the IGP at it, so the IGP never reads from an oracle without the
settings' pricing; an oracle with no pricing for Dusk would quote delivery
at zero.
After changing `OPERATOR_MARGIN_BPS`, `EVM_IGP_GAS_OVERHEAD` or
`DUSK_RELAYER_GAS_LIMIT`, run `agent-configs` again and restart the
relayer: its requirements follow them.

A margin absorbs moves until the next update. At the default 50%, EVM gas
or the ETH/DUSK price can rise by half before a warp transfer from Dusk
costs more to deliver than it paid. Watch both, and the relayer's spending
against what the IGPs collect.

### An IGP that had a quote signer

This deployment prices delivery to Dusk with its StorageGasOracle only. A
quote signer the owner adds to the Sepolia IGP can store standing quotes
for any sender, which come before the oracle and outlive the signer's
removal until they expire, so `preflight`, `deploy`, `evm-igp` and
`agent-configs` refuse an IGP that has ever had one. They read its
`QuoteSignerAdded` events from the block that created it,
`EVM_LOG_BLOCK_RANGE` blocks a request (1,999 by default; lower it if the
RPC refuses the range), and record in the state how far they read, up to
`EVM_REORG_PERIOD` blocks below the head, so the next read starts there.

Such an IGP can't be cleaned up. Deploy a new one: remove `.evm.igp`,
`.evm.igp_creation_tx` and `.evm.route_hook` from
`deploy/out/<DEPLOY_NAME>/deployment.json` and run `deploy`, which creates
the IGP, prices it through the recorded oracle and points the route's hook
at it; then run `agent-configs` and restart the relayer. Anyone can still
`claim()` what the old IGP holds for its beneficiary.

### Topping up a message

The relayer only delivers a message once the gas it paid for covers what
its delivery takes ([costs.md](costs.md#what-the-relayer-requires)). A
message that paid for less waits, and the relayer checks it again later.
That happens to a message whose recipient's `handle` takes more gas than
its dispatch paid for (a warp transfer from Dusk pays for 160,000 EVM
gas), or whose delivery got more expensive after it was sent.
Anyone can pay for more gas for it, as with `payForGas` on EVM, and the
relayer adds up every payment for the message's ID and destination.

From **Dusk**, pay the Dusk IGP with `dusk-tx pay-for-gas`:

```bash
# The IGP's own quote, checked as the IGP checks it; signs nothing
dusk-tx pay-for-gas --rues-url <node> --igp <dusk igp> --nonce <nonce> \
    --gas-amount <gas> --quote-only

# Pay it, from any funded account
dusk-tx pay-for-gas --rues-url <node> --keys payer.keys --igp <dusk igp> \
    --nonce <nonce> --gas-amount <gas> --expected-chain-id <id> [--max-payment <lux>]
```

- `--expected-chain-id` is the chain you mean to pay on (2 on testnet). As
  with every `dusk-tx` command that sends a transaction, the command asks
  the node its chain ID first and stops on any other, before it queries a
  contract or reads the key. `--quote-only` doesn't need it.

- `--nonce` is the message's nonce on the Dusk Mailbox (`dusk-tx
  dispatched-message` prints a message by nonce). `pay-for-gas` reads the
  message's ID and destination from its `Dispatch` event, so the node must
  serve the archive. `--message-id <id> --destination <domain>` pays for
  an ID as given instead, as `payForGas` does: nothing checks it, and a
  payment for a wrong ID or destination is lost.
- `--gas-amount` is the gas to add. The relayer from Dusk requires the gas
  paid for, all payments together, to cover its `gasFraction` (`2/3` by
  default) of its estimate of the delivery (`eth_estimateGas` of
  `process`). A message without hook metadata, or with fewer than its 8
  bytes, has paid for 50,000 gas plus the IGP's overhead; pay for the rest,
  with some margin. One dispatched with EVM-style `StandardHookMetadata` has
  paid for 256 gas plus the overhead
  ([security.md](security.md#deviations-from-solidity-hyperlane)).
- The IGP charges `gas amount x gas price x exchange rate / 1e10` LUX, with
  no overhead: a dispatch paid for that. It charges its price when the
  transaction runs, from the transaction's deposit, and sends the rest of
  the deposit back to the signer in the same transaction. The deposit is
  the quote unless `--max-payment` asks for more, which covers a price
  update between the quote and the transaction.
- Before it signs, `pay-for-gas` checks that the contract is an IGP that
  takes top-ups (`state_version` 4), that it prices the destination, that
  the payment isn't zero, and that the signer holds the deposit and the
  whole gas limit: a failed transaction pays its whole limit. A top-up
  takes about 1.85M gas; the default limit is 3M. The output has the
  quote, the deposit, and once included what the IGP charged (`paid`) and
  sent back (`refunded`).
- The relayer sees the payment once its block is final, and delivers the
  message the next time it checks it. It checks a waiting message every
  few minutes for its first hour, then less and less often: every half
  hour to hour after about five hours, every two to eight hours after
  about twelve. Its operator can have it check at once, on its
  `metricsPort`:

  ```bash
  curl -X POST http://<relayer>:9090/message_retry -H 'Content-Type: application/json' \
      -d '[{"messageid": "0x<message id>"}]'
  ```

Only a Moonlight account can top up: the payment is the transaction's own
deposit, which no contract it calls can spend. The Dusk IGP stops taking
top-ups when its memory reaches 3 GiB, leaving the rest for dispatches'
payments ([costs.md](costs.md#attack-costs)).

From **Sepolia**, pay our EVM IGP, the one in `.evm.igp`, for Dusk gas:

```bash
QUOTE=$(cast call <evm igp> "quoteGasPayment(uint32,uint256)(uint256)" <dusk domain> <gas> \
    --rpc-url $EVM_RPC | awk '{print $1}')
cast send <evm igp> "payForGas(bytes32,uint32,uint256,address)" <message id> <dusk domain> <gas> \
    <refund address> --value $QUOTE --rpc-url $EVM_RPC --private-key <payer key>
```

A message to Dusk must have paid for `EVM_IGP_GAS_OVERHEAD` Dusk gas
(2,170,000 by default). Paying for more than the relayer's limit,
`DUSK_RELAYER_GAS_LIMIT` (5M), doesn't help: the relayer sends every
delivery to Dusk with at most that limit, so a message that needs more
fails on chain however much it paid
([costs.md](costs.md#what-stays-uncovered)).
`bash deploy/local-fees-check.sh` tops up a message both ways in the local
rehearsal.

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
dusk-tx claim-fees --rues-url <node> --keys beneficiary.keys --contract <igp or protocol_fee id> \
    --expected-chain-id <id>
```

`claim-fees` checks the signer is the beneficiary and there is something
to claim before it signs, since a failed claim pays its whole gas limit; a
claim takes about 1.87M gas. The deployer is the beneficiary unless
`DUSK_FEE_BENEFICIARY` names another account, which `deploy` sets on both
contracts once it has checked that the recorded Mailbox calls them. The
owner can change it at any time:

```bash
dusk-tx set-beneficiary --rues-url <node> --keys owner.keys --contract <id> \
    --beneficiary <base58 address> --expected-chain-id <id>
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

`dusk-tx call` has two flags for a sender that keeps several transactions
in flight. `--nonce <N>` signs with Moonlight nonce N instead of the
account's next one, without looking the account up. `--no-wait` returns
once the node has taken the transaction, after preverify and propagate,
and prints `success`, `tx_id`, `nonce` and `submitted: true` without block
fields. The node can still drop a transaction it took, so whoever runs it
reconciles the hash. With `--nonce`, preverify refusing an otherwise valid
transaction only because the transactions with the nonces before its own
haven't arrived (`Missing intermediate nonce`) doesn't stop it: propagate
holds it, retrying every 500 ms, and drops it if they haven't arrived after
about 5 seconds. Any other refusal still stops it, as `... submission
failed: Preverify rejected before propagation ...`, and a propagation the
node doesn't confirm is reported as `... Propagation outcome unknown ...`.
The relayer uses both flags only when it pipelines its deliveries
([below](#deliveries-to-dusk)), which the configs `agent-configs` writes
leave off.

### Acknowledged sends

`dusk-tx call --ack-before-send` lets its caller keep a transaction's hash
before the transaction is sent. Once `call` has signed it and printed
`Prepared TX <hash>; nonce <N>; ...` on stderr, it reads one more line from
stdin and sends the transaction only if that line is `send`. Until then only
unsigned reads reach the node: its chain ID and, without `--nonce`, the
signer's account, for its next nonce. Nothing signed does, preverify
included. The end of stdin, a failed read, any other line, or no line within
30 seconds sends nothing more: `call` prints `Not submitted: <reason>` on
stderr and fails with
`Transaction <hash> submission failed: Not submitted: <reason>`. With
`--secret-key-stdin`, the key is then the next line of stdin, not the rest
of it, and the caller keeps stdin open after it. `call --help` lists the
flag, which is how a caller tells a `dusk-tx` that has it.

The nonce on the prepared line is the one the transaction is signed with: a
caller that keeps it can tell when the transaction can no longer execute,
which is once a transaction with that nonce is in a final block. It comes
after the hash's `;`, so the hash is still everything before the first `;`.
A transaction that isn't Moonlight gets the line without a nonce.

## Deliveries to Dusk

A relayer delivers to Dusk one message at a time. For each delivery it runs
`dusk-tx call`, which signs with the account's next nonce, as the node
reports it, and returns once the transaction is in a block. The next
delivery reaches the node after the next block's round has started, so one
relayer key delivers about a message every two blocks, 3 a minute: in the
local measurement, a burst of 39 transfers to Dusk took 778 s
([costs.md](costs.md#throughput-into-dusk)). A delivery sent again while its
first transaction still waits in the mempool takes the same nonce, and the
node refuses it, so at most one of them lands.

The agents can also pipeline deliveries: send a batch as consecutive
transactions of the relayer's account, each signed with a nonce the relayer
chooses and keeps in `eventCursorDir`, so that they share a block. Only a
Dusk chain that sets both `pipelineDeliveries: true` and a `maxBatchSize`
above 1 (at most 64) pipelines, and it is experimental. While it pipelines,
the relayer must be the only process that signs with its key, no validator,
wallet or second relayer, and the last review of it found six issues that
aren't fixed yet, which the agent's README
(`rust/main/chains/hyperlane-dusk/README.md` in the monorepo fork) lists: a
transaction signed before pipelining is switched off can still land after
it, proxy settings reach the helper's loopback signer, a filler can lose the
message it replaced, recovered deliveries escape the relayer's gas
accounting, fillers bypass the bound of 64 outstanding deliveries, and a
batch's simulations can run past its deadline. Don't turn it on for testnet
or mainnet until they are fixed. With `maxBatchSize` 16 and the agents from
before it was opt-in (`0d8af2dda2`), the measurement delivered a burst of 40
transfers in four blocks, every other block, the last 85.7 s after its
dispatch: at most 48 deliveries a minute. All 106 deliveries of that run
executed once, with no second copies and none failed, a relayer restart in
the middle of a batch included.

`agent-configs` writes neither setting, so every agent build delivers one
at a time. `maxBatchSize` alone would deliver one at a time with the current
agents too, but the relayer would take that many ready messages at a time,
try them as a batch, which the chain declines, and fall back to one at a
time with a warning for each batch; and agents from before
`pipelineDeliveries` existed (`testnet/dusk-agents` before `d549a2fc73`)
pipelined on `maxBatchSize` alone. An env file from before that still sets
`DUSK_RELAYER_MAX_BATCH_SIZE` gets a warning that it is no longer read. To
try pipelining in a local rehearsal, add both settings to the relayer's Dusk
chain by hand, or run the bench with `PIPELINE_DELIVERIES=1`
([demo/bench](../demo/bench/README.md)). Turning it off again needs no
manual step, with the first issue above: the relayer waits out the nonces a
pipelined run left in `eventCursorDir`, which must not be deleted by hand.

## CI

`.github/workflows/ci.yml` runs on every pull request: workflow lint,
whitespace, shell syntax, the demo lifecycle tests, the deploy script tests
(`make test-deploy-scripts`: `deploy/testnet.sh` and
`demo/gen-agent-configs.sh` against a stubbed `dusk-tx` and a fake EVM chain) and the
secret-hygiene scan. It can't build the contracts, because they depend on
the private Rusk repository.

`.github/workflows/manual-repro-check.yml` runs the full build and test suite
(`make repro-check-agent`) on demand against exact commits of this repository,
`rusk-private` and `hyperlane-monorepo`. Besides the contracts, the VM tests,
the `dusk-tx` tests and a check that the agents build, it runs the agent's
one test against the real `dusk-tx`,
`a_dusk_tx_build_sends_only_on_the_agents_word`: the agent drives `dusk-tx`
through its output, the `Prepared TX` line, the go-ahead to send and the
error prefixes that decide whether a delivery may have been sent, and the
agents' public CI can only test that against a stand-in, as it can't build
`dusk-tx` without `rusk-private`. It needs:

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

### Release gate

Before a release of this repository or of the agents, and whenever
`dusk-tx` changes, check both against each other where `rusk-private` is
at hand:

- `make repro-check-agent` (or the workflow above) at the commits to
  release: the full build and tests here, and the agent's test against
  this `dusk-tx`.
- In the monorepo fork,
  `bash rust/main/chains/hyperlane-dusk/scripts/dusk-tx-interop-check.sh`:
  the same test against `dusk-tx` built at the commit of this repository the
  agents pin (`DUSK_REF` in `.github/workflows/dusk-agent-gate.yml`). Its
  README says how to point it at another commit.
