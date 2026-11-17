# Testnet deployment

How to deploy the bridge to Dusk testnet, connect it to Sepolia through
Hyperlane's existing Sepolia contracts, and run the agents.
`deploy/testnet.sh` does the deployment; this page covers what it sets up,
what you need first, and how to run and check it.

## What gets deployed

| Where | What |
|---|---|
| Dusk testnet | The full contract set (Mailbox, Merkle hook, ProtocolFee, AggregationHook, IGP, ValidatorAnnounce, MessageIdMultisig ISM, TestRecipient, TestMock) and a native DUSK warp route (`warp-native`). |
| Sepolia | A synthetic DUSK token (`HypERC20`, 9 decimals so one unit is one LUX), a MessageIdMultisig ISM for messages from Dusk, an InterchainGasPaymaster with its own StorageGasOracle that charges for delivery to Dusk, and the token's hook: a StaticAggregationHook of Hyperlane's Sepolia MerkleTreeHook and that IGP, from Hyperlane's factory. The token and the IGP are each an `ERC1967Proxy` whose constructor initializes them, so the transaction that creates them also sets their owner, and nobody can initialize them first; the proxies have no admin, so they can't be upgraded. Everything else is Hyperlane's own Sepolia deployment. |

How each direction is secured:

- **Sepolia → Dusk.** The Dusk Mailbox verifies messages with
  `EVM_ORIGIN_VALIDATORS`. The default is Hyperlane's own Sepolia validator
  (1 of 1, run by Abacus Works). It already signs every Sepolia checkpoint, so
  this direction needs no validators of ours.
- **Dusk → Sepolia.** The Sepolia token verifies messages with our
  `DUSK_ORIGIN_VALIDATORS` (m of n). Each runs the Hyperlane validator against
  Dusk and publishes signed checkpoints to its own bucket.

The deployer key owns every contract on both sides. That's acceptable for
testnet. For mainnet, see [Before mainnet](#before-mainnet).

Delivery is paid for in both directions, and the relayer relays any message
between the two chains, from any sender, once it is paid for
([costs.md](costs.md#fee-coverage)):

- **Dusk → Sepolia.** The Dusk IGP, the Dusk Mailbox's default hook, charges
  DUSK for the Sepolia gas a delivery takes: 6 DUSK for a transfer at the
  default prices. The ProtocolFee adds 0.001 DUSK for its beneficiary.
- **Sepolia → Dusk.** Our IGP on Sepolia charges ETH for the Dusk gas a
  delivery takes: 1.302e11 wei for a transfer at the default prices, on top
  of Hyperlane's 1 wei protocol fee. Hyperlane's own Sepolia IGP has no
  pricing for Dusk, and its Mailbox's default hook falls back to plain
  Merkle insertion for Dusk, so a message sent the default way pays us
  nothing.

The Sepolia token's hook replaces the Mailbox's default hook, and the
Mailbox only adds its required hook, a ProtocolFee. The Merkle insertion
the validators sign comes from the default hook, so a hook of our IGP alone
would leave the token's messages out of the tree, and nothing would ever
deliver them. The token's hook therefore aggregates Hyperlane's
MerkleTreeHook, `EVM_MERKLE_TREE_HOOK`, and our IGP. `deploy` first follows
the Mailbox's required hook, through aggregations and domain routing hooks,
and counts how often it inserts a message to Dusk into that tree. Had it
inserted once, the IGP alone would do, and adding the tree again would
insert every message twice. More than once, or through a hook whose
behaviour the script can't tell, and `deploy` stops.

`relayer.json` enforces payment per origin, against this deployment's IGPs
(`onChainFeeQuoting`). The operator collects what the IGPs take, and
`deploy/testnet.sh revenue` and `claim` show and claim it
([operations.md](operations.md#relaying-fees)).

## Before you start

- Checkouts laid out as in the [README](../README.md#building), with
  `rusk-private` at the commit in [`RUSK_VERSION`](../RUSK_VERSION). Then
  `make all dusk-tx`.
- Foundry (`forge`, `cast`), `jq`, `curl`, `python3`. `forge build` must work
  in `hyperlane-monorepo/solidity`.
- A **Dusk domain ID**. The default `1146442572` is free in the Hyperlane
  registry; `1146442571` ("DUSK") is kept for mainnet.
- **Funded keys:**

| Key | Network | Needs | Used for |
|---|---|---|---|
| Deployer | Dusk testnet | about 400 DUSK | Contract deploys, about 220 DUSK (a local rehearsal spent 207.4 DUSK on 11 transactions; the native route has since grown by 48 KB, about 10 DUSK). Each deploy also needs its gas limit times the deploy gas price, 100 DUSK, in the balance when it is submitted. `preflight` checks the balance. |
| Deployer | Sepolia | about 0.1 ETH | ISM, IGP, gas oracle, hook, token and enrollment transactions: 10.4M gas in 10 transactions against the local rehearsal's core, which deploys its ISM from source; less with Hyperlane's ISM factory. The IGP and the token are each an implementation and a proxy that initializes it, 211,323 and 294,652 gas. |
| Relayer | Dusk testnet | a few DUSK | `process` transactions for Sepolia → Dusk, about 0.0022 DUSK each, and 0.005 DUSK for one that fails. Senders pay it back to the Sepolia IGP. |
| Relayer | Sepolia | 0.1 ETH or more | `process` transactions for Dusk → Sepolia. Senders pay it back to the Dusk IGP. |
| Each Dusk-origin validator | EVM key, no funds | | Signing checkpoints. Its address goes in `DUSK_ORIGIN_VALIDATORS`. |
| Each Dusk-origin validator | Dusk testnet | 0.1 DUSK | Its one-time announcement on Dusk, about 2.1M gas. |

Give the relayer and each validator a Dusk account of its own. An account's
transactions are ordered by its nonce, so two processes signing with one key
make each other's transactions fail. See [Dusk keys](#dusk-keys).

Gas on Dusk:

- Deployments are charged 100 gas per WASM byte, at the network's minimum
  deploy gas price of 2000 LUX. The Mailbox is about 187 KB, so 18.7M gas
  (37 DUSK), the native warp route 172 KB (34 DUSK), and each other contract
  27–95 KB (5–19 DUSK). Initialisation adds a little to each.
- Ordinary transactions only need a gas price above zero. `dusk-tx` uses
  1 LUX for everything except `deploy-hyperlane`, and the agent configs use
  `DUSK_AGENT_GAS_PRICE` (1 LUX). A relayed `process` takes about 2.2M gas
  with one validator signature and about 0.22M more per extra signature:
  0.0022 DUSK at 1 LUX.
- The relayer's gas limit is `DUSK_RELAYER_GAS_LIMIT`, 5M. A failed
  transaction pays its whole limit, and the relayer's `process` fails
  whenever someone else delivers the message first, or when a recipient
  passes the simulation and then reverts. The VM test
  `test_relayed_process_fits_the_relayer_gas_limit` measures a relayed
  native transfer at 1.95M gas plus 0.22M per signature the Dusk ISM checks
  (`EVM_ORIGIN_THRESHOLD`): 2.14M at one signature and 3.01M at five. 5M is
  2.3 times the cost at the default threshold of 1 and still 1.5 times it at
  6: `test_relayed_process_at_threshold_6_fits_a_5m_gas_limit` delivers to
  every route at threshold 6 in a transaction whose limit is 5M, and the
  costliest, a payout from a collateral route locking the upstream DRC20,
  takes 3.27M. `preflight` and `agent-configs` refuse a limit below 1.5
  times the cost at `EVM_ORIGIN_THRESHOLD`. Once the contracts are
  deployed, they also stop unless the ISM the route uses on chain has
  exactly that threshold (see below), so the limit is checked against what
  the chain enforces. The owner can raise the threshold after deploying;
  then set `EVM_ORIGIN_THRESHOLD` to match and raise the relayer's limit
  with it. Validators keep `DUSK_AGENT_GAS_LIMIT` (30M) for their one-time
  announcement.
- Withdrawing dispatch credit (`withdraw-dispatch`) takes about 59M gas,
  because the Mailbox decodes the recipient's public key, and `dusk-tx`
  gives it an 80M limit.
- Claiming your own escrow (`claim-pending`) takes about 1.9M gas, and
  `dusk-tx` gives it a 3M limit. Claiming someone else's
  (`claim-pending --for`) takes about 59M, because the route decodes their
  public key on chain, so `dusk-tx` gives it an 80M limit and the signer
  needs 0.08 DUSK at 1 LUX. `--gas-limit` overrides either.
- The gas limit times the gas price must be in the balance when a
  transaction is submitted. A successful transaction is refunded the gas it
  didn't use; **a failed one is charged its whole gas limit.** `dusk-tx`
  checks what it can before signing (recipient, amount, route, pending
  escrow), and the relayer simulates each `process` before submitting it.

### Dusk keys

`dusk-tx` signs with an encrypted consensus key file, as exported by
rusk-wallet. It takes the password from `--password`, or else from
`DUSK_CONSENSUS_PASSWORD`.

To export an account's key file with rusk-wallet (built from `rusk-private`
with `cargo build --release -p rusk-wallet`):

```bash
export RUSK_WALLET_PWD=...            # the wallet's password
rusk-wallet --wallet-dir ./wallet --network testnet restore < recovery-phrase.txt   # or use an existing wallet
rusk-wallet --wallet-dir ./wallet --network testnet profiles   # find the account
RUSK_WALLET_EXPORT_PWD=... rusk-wallet --wallet-dir ./wallet --network testnet \
  export --profile-idx 0 --dir ./keys --name relayer
chmod 600 keys/relayer.keys
```

- `profiles` lists the accounts as Profile 1, 2, …, but `--profile-idx`
  counts from 0.
- The export writes `relayer.keys`, encrypted with `RUSK_WALLET_EXPORT_PWD`,
  and `relayer.cpk`, the public key. The `.keys` file is world-readable as
  written.
- Use a separate `--wallet-dir` per wallet rather than the default under
  `~/.dusk`.

The agents read a plain key file instead. Write one from the export:

```bash
export DUSK_CONSENSUS_PASSWORD=...
dusk-tx export-signer-key --keys relayer.keys --out /run/secrets/dusk-signer.key
```

It is created with mode 0600 and is never overwritten. It holds the account's
secret key unencrypted, so keep it on the agent's host only.

`dusk-tx account` shows an account's address, balance, and the recipient hash
that warp routes deliver to. Senders on the EVM side need that hash:

```bash
dusk-tx account --rues-url https://testnet.nodes.dusk.network/ --keys my.keys
dusk-tx account --rues-url https://testnet.nodes.dusk.network/ --address <base58 address>
```

## Deploy

```bash
cp deploy/testnet.env.example deploy/testnet.env
# edit: DUSK_KEYS, DUSK_ORIGIN_VALIDATORS, DUSK_ORIGIN_THRESHOLD, and the
# relaying fees: ETH_DUSK_PRICE_RATIO, EVM_GAS_PRICE_WEI, OPERATOR_MARGIN_BPS
export DUSK_CONSENSUS_PASSWORD=...   # password of DUSK_KEYS
export EVM_DEPLOYER_KEY=0x...        # Sepolia deployer

bash deploy/testnet.sh fees          # what the fee settings charge; no network
bash deploy/testnet.sh preflight     # read-only checks, safe to run any time
bash deploy/testnet.sh deploy
bash deploy/testnet.sh agent-configs
```

The fee settings are prices: DUSK per ETH, the EVM gas price and the
operator's margin. `deploy` turns them into each IGP's token exchange rate
and gas price ([costs.md](costs.md#pricing)), and the gas overheads,
`DUSK_IGP_GAS_OVERHEAD` and `EVM_IGP_GAS_OVERHEAD`, into the gas each
transfer pays for. `fees` prints what they come to.

`preflight` checks that:
- the Dusk node is on the expected chain, has both host queries active, allows
  third-party contracts and serves the archive queries the agents need;
- Multicall for Dusk answers `version` with 1 at `DUSK_MULTICALL` (by
  default its ID on mainnet and testnet). `agent-configs` names it in both
  configs (`batchContractAddress`), so the agents batch their contract
  reads, and leaves it out, with a warning, on a chain where it isn't;
- the Sepolia Mailbox, Merkle hook and ValidatorAnnounce belong together;
  how often the Mailbox's required hook inserts a message to Dusk into the
  Merkle tree, following aggregation and routing hooks, which decides the
  route's hook; and that `EVM_AGGREGATION_HOOK_FACTORY`, when set, is a
  contract;
- the fee settings are valid and their gas overheads cover delivery:
  `DUSK_IGP_GAS_OVERHEAD` for `DUSK_ORIGIN_THRESHOLD` of the
  `DUSK_ORIGIN_VALIDATORS`, `EVM_IGP_GAS_OVERHEAD` for
  `EVM_ORIGIN_THRESHOLD`;
- once deployed, both IGPs charge what the settings say, and the Sepolia
  token's hook is the recorded one, inserting into `EVM_MERKLE_TREE_HOOK`'s
  tree and paying our IGP. `dusk-igp` and `evm-igp` show and fix a pricing
  difference ([operations.md](operations.md#keeping-the-prices-current));
- the Sepolia IGP and token were initialized in the transaction that
  created them: `deploy` records that transaction, and its receipt must
  show the EVM deployer creating the contract as a proxy of the recorded
  implementation, and the contract's `Initialized` event. Each proxy must
  still run that implementation and have no proxy admin (EIP-1967's admin
  slot is zero). The IGP must never have had a quote signer: none now, and
  no `QuoteSignerAdded` among its events since the block that created it
  (a new IGP has none). A signer's standing quotes outlive its removal and
  come before the oracle for the senders they name, and this deployment
  uses no signed quotes. It must also quote a transfer what its oracle's
  pricing comes to, asked as the token and as any other sender;
- the recorded Sepolia token is this deployment's: its `mailbox()` is
  `EVM_MAILBOX`, and it verifies messages from Dusk with the recorded ISM,
  a MessageIdMultisig ISM of exactly `DUSK_ORIGIN_VALIDATORS` and
  `DUSK_ORIGIN_THRESHOLD`, which lists each validator once (a validator
  listed twice counts its signature twice). Its router for Dusk is the
  recorded route, or none yet and then it holds no tokens: `deploy`
  creates it with none, and enrolls the route on it after. `deploy` checks
  this again right before each write to the token, and refuses another
  route's router rather than replace it. A token another deployment
  created the same way is refused before anything uses or rewires it;
- the keys and validator settings are present and consistent;
- a deployment already recorded under `deploy/out/<DEPLOY_NAME>/` is on this
  chain, and its contracts are at this checkout's `state_version`s;
- the ISM the recorded route uses on chain, its own or else the Mailbox's
  default, is a MessageIdMultisig ISM whose validators and threshold are
  `EVM_ORIGIN_VALIDATORS` and `EVM_ORIGIN_THRESHOLD`, each validator listed
  once. `deploy` checks it again when it finishes, and `agent-configs`
  before it writes anything.
  Any other module type stops them, a TestMock ISM (which accepts every
  message) included, and so does a query that fails. If the owner changes
  the validators or the threshold on chain, update both settings.

`deploy` records every step in `deploy/out/<DEPLOY_NAME>/deployment.json` and
skips steps already recorded there. If it stops halfway, fix the cause and run
it again. Router enrollment is re-checked on chain each time. If the recorded
contracts aren't on chain, for example after a testnet reset, or a recorded
contract reports another `state_version` than this checkout's (Mailbox 6,
MerkleTreeHook 2, IGP 4, ValidatorAnnounce 3, native route 5), `deploy`
stops; move the directory aside and deploy again. Older routes may pay
sponsored escrow claims to forged keys
([security notes](security.md#issues-found-and-fixed), CRITICAL-1) and
register any key that asks (MEDIUM-10), older core contracts keep storage
anyone can fill cheaply ([storage.md](storage.md)), and an older IGP can't
be topped up (LOW-15), so `agent-configs` refuses them too. So is a
Sepolia IGP or token recorded by an older version of this script, which
initialized them in a second transaction that anyone could send first
([security notes](security.md#fourth-review), HIGH-8).

Routes need no prepaid dispatch credit: each transfer's deposit pays its own
dispatch fee.

## Run the agents

`agent-configs` writes two files to `deploy/out/<DEPLOY_NAME>/`. Neither
contains secrets. Their database and event-cursor paths are under
`AGENT_STATE_DIR` (default `/var/lib/hyperlane`), and their Dusk key file is
`DUSK_SIGNER_KEY_FILE` (default `/run/secrets/dusk-signer.key`); set both in
`deploy/testnet.env` before running `agent-configs` to change them.

Build the agents in `hyperlane-monorepo/rust/main` with
`cargo build --release --bin relayer --bin validator`, and run them from that
directory: they load the chain defaults in its `config/` directory and exit
with `Failed to open config directory` anywhere else. `CONFIG_FILES` takes an
absolute path.

- **`relayer.json`** relays any message between Dusk and Sepolia, in both
  directions, once it is paid for: `gasPaymentEnforcement` has an
  `onChainFeeQuoting` policy for each origin, and the relayer reads payments
  from this deployment's two IGPs, the Sepolia one included
  ([costs.md](costs.md#what-the-relayer-requires)). A message nobody paid
  for, or paid too little for, waits in the relayer's queue; a later
  payment releases it. The relayer delivers to Dusk one message at a time,
  about one every two blocks: measured locally, a burst of 39 took 778 s.
  Its pipelined deliveries took a burst of 40 in 85.7 s, but they are
  experimental and stay off on testnet
  ([operations.md](operations.md#deliveries-to-dusk)). Run one relayer:

  ```bash
  cd hyperlane-monorepo/rust/main
  export DUSK_TX_BIN=/path/to/dusk-tx
  export HYP_CHAINS_SEPOLIA_SIGNER_KEY=0x...      # relayer's Sepolia key
  # plus the relayer's Dusk key file (see Dusk keys) at the config's keyFile
  CONFIG_FILES=/path/to/relayer.json ./target/release/relayer
  ```

- **`validator-dusk.json`** is a template for one Dusk-origin validator. Each
  operator copies it, sets `checkpointSyncer` to their own S3 bucket (public
  read, so the relayer can fetch checkpoints), and runs:

  ```bash
  cd hyperlane-monorepo/rust/main
  export DUSK_TX_BIN=/path/to/dusk-tx
  export HYP_VALIDATOR_KEY=0x...       # one of DUSK_ORIGIN_VALIDATORS
  # plus AWS credentials for the bucket, and the validator's Dusk key file
  # at the config's keyFile (it pays for the announcement)
  CONFIG_FILES=/path/to/validator-dusk.json ./target/release/validator
  ```

  On startup the validator announces its bucket on Dusk's ValidatorAnnounce.
  The relayer finds it there.

Agents on the same host each need their own `db`, `eventCursorDir`,
checkpoint location and `metricsPort` (the templates use 9090 for the relayer
and 9091 for the validator). `deploy/local-agents.sh` shows a working set of
five agents on one host.

Before each delivery to Dusk, the relayer runs `process` as a read-only
contract query (`POST /on/contracts:<mailbox>/process`) and only submits if
it succeeds. Its RUES endpoint must allow that query: behind a proxy that
refuses it, nothing is delivered and the relayer logs `not submitting the
message`. The agents need contracts at the current `state_version`s
(Mailbox 6, MerkleTreeHook 2, ValidatorAnnounce 3), and an agent build that
reads them: deliveries come from `ProcessId` events, checkpoint roots are
computed from the Merkle hook's message IDs, and validator locations come
from `ValidatorAnnouncement` events ([storage.md](storage.md#what-agents-read)).
The Mailbox takes `process`, and ValidatorAnnounce `announce`, only as a
transaction's own call, as the agents send them, or as a query, as the
relayer simulates `process`.

Each Dusk transaction an agent sends is logged as `dusk-tx call succeeded`
with its `tx_id`. Its gas use and result are in the node's GraphQL API:
`query { tx(hash: "<tx_id>") { gasSpent err blockHeight } }` at
`/on/graphql/query`, and whether its block is final in
`query { block(height: <blockHeight>) { status } }`.

`dusk-tx` reports a transaction once it is included: its output has
`block_height`, `gas_spent` and `finalized`, whether that block was already
final. To wait until it is, pass `--wait-finality` to any command (see
[operations](operations.md#uncertain-transactions)).

## Check it works

Send 1 DUSK to Sepolia, then send it back. Use the state file for the
addresses:

```bash
S=deploy/out/<DEPLOY_NAME>/deployment.json
ROUTE=$(jq -r .dusk.warp_native $S); TOKEN=$(jq -r .evm.token $S)
DUSK=https://testnet.nodes.dusk.network/; EVM_RPC=https://ethereum-sepolia.publicnode.com
export DUSK_CONSENSUS_PASSWORD=...

# Dusk -> Sepolia: lock 1 DUSK (1e9 LUX), mint 1 DUSK on Sepolia. The
# deposit is the amount plus the quoted dispatch fee, 6.001 DUSK at the
# default prices; dusk-tx adds the fee.
target/release/dusk-tx transfer-remote --native --rues-url $DUSK --keys my.keys \
  --warp-contract $ROUTE --destination 11155111 \
  --recipient 000000000000000000000000<your sepolia address, no 0x> --amount 1000000000 \
  --expected-chain-id 2
cast call $TOKEN "balanceOf(address)(uint256)" <your sepolia address> --rpc-url $EVM_RPC

# Sepolia -> Dusk: burn on Sepolia, release on Dusk. The route holds the first
# transfer to your account in escrow; claiming it pays it out and registers
# the account, and later transfers arrive directly.
H=$(target/release/dusk-tx account --offline --keys my.keys | jq -r .account_h256)
FEE=$(cast call $TOKEN "quoteGasPayment(uint32)(uint256)" 1146442572 --rpc-url $EVM_RPC | awk '{print $1}')
cast send $TOKEN "transferRemote(uint32,bytes32,uint256)" 1146442572 0x$H 1000000000 \
  --value $FEE --rpc-url $EVM_RPC --private-key <your key>
target/release/dusk-tx account --rues-url $DUSK --keys my.keys --warp-contract $ROUTE
target/release/dusk-tx claim-pending --rues-url $DUSK --keys my.keys --warp-contract $ROUTE \
  --expected-chain-id 2
```

`quoteGasPayment` on the token is what a transfer to Dusk pays: our IGP's
fee and Hyperlane's 1 wei protocol fee, 130,200,000,001 wei at the default
prices. Sending less reverts; the token's hook refunds anything above it.

Every `dusk-tx` command that sends a transaction takes the chain it is for
as `--expected-chain-id` (2 on testnet), and stops before it reads the key
when the node reports another chain.

`--recipient` takes a 20-byte EVM address and pads it to 32 bytes, as
Hyperlane does. `dusk-tx` refuses a 32-byte value whose first 12 bytes aren't
zero: no EVM route can deliver to it, and the DUSK would stay locked on Dusk.
`--raw-recipient` overrides that for non-EVM destinations.

**Escrow.** A transfer to a Dusk account the route hasn't paid before waits
in escrow, and a claim of it registers the account, so later transfers
arrive directly. A recipient whose only DUSK is in that escrow can't pay gas
to claim it, so anyone can claim it for them. The route decodes the public
key the claimant names and pays the account that signs with it, whose hash
the escrow is under. The claimant only pays the gas:

```bash
# What's waiting, and whether the account is registered
target/release/dusk-tx account --rues-url $DUSK --address <base58 address> --warp-contract $ROUTE
# Claim your own escrow, or someone else's with --for; either also registers the account
target/release/dusk-tx claim-pending --rues-url $DUSK --keys my.keys --warp-contract $ROUTE \
  --expected-chain-id 2 [--for <base58 address>]
```

Nobody can register an account in advance, their own or anyone else's:
routes have no `register_account`, so each account in a route's registry
is one somebody sent a transfer to ([security notes](security.md#registry-growth),
MEDIUM-10). A wallet can't prepare an account ahead, but the first transfer
takes the same number of transactions as before: a claim where a
registration used to be.

**Undeliverable messages.** Some transfers can never be delivered. A
transfer to the zero recipient is one example, and so is anything that
makes the destination's `handle` revert. The relayer simulates each
delivery first, finds that it would fail, and retries it with backoff
without paying for it. The tokens stay where they are: burned on Sepolia,
or locked in the Dusk route. Add such messages to the relayer's denylist to
stop the retries.

A message to Dusk whose delivery needs more gas than the relayer's limit,
`DUSK_RELAYER_GAS_LIMIT` (5M), can't be delivered either, and it costs the
relayer each time: the simulation runs with the block's gas, so it passes,
and the transaction fails on chain, paying the whole 5M. The relayer sends
every delivery to Dusk with at most that limit, whatever the message paid
for, and tries it again only once more is paid
([costs.md](costs.md#what-stays-uncovered)). Denylist such a message too.

**Fees.** `bash deploy/testnet.sh revenue` shows what the IGPs and the
ProtocolFee hold, and `claim` collects it
([operations.md](operations.md#collecting-the-fees)). A message that didn't
pay for its delivery isn't delivered: the relayer logs `Message does not
meet the gas payment requirement` for it and checks again later. A message
from Sepolia can still be paid for, by anyone, for at least
`EVM_IGP_GAS_OVERHEAD` gas in all:

```bash
IGP=$(jq -r .evm.igp $S); GAS=2170000
FEE=$(cast call $IGP "quoteGasPayment(uint32,uint256)(uint256)" 1146442572 $GAS --rpc-url $EVM_RPC | awk '{print $1}')
cast send $IGP "payForGas(bytes32,uint32,uint256,address)" <message id> 1146442572 $GAS <refund address> \
  --value $FEE --rpc-url $EVM_RPC --private-key <your key>
```

A message from Dusk pays when it is dispatched, through the Mailbox's
default hook, and can be paid for afterwards too, by anyone, with
`dusk-tx pay-for-gas` ([operations.md](operations.md#topping-up-a-message)).
`--quote-only` prints the IGP's quote and signs nothing; then pay from any
funded account:

```bash
IGP=$(jq -r .dusk.igp $S)
dusk-tx pay-for-gas --rues-url $DUSK_RUES_URL --igp $IGP --nonce <nonce> --gas-amount <gas> --quote-only
dusk-tx pay-for-gas --rues-url $DUSK_RUES_URL --keys <your keys> --igp $IGP --nonce <nonce> --gas-amount <gas> \
  --expected-chain-id 2
```

## Reproduce the contracts

A contract's ID is the BLAKE2b-256 hash of its WASM, its deploy nonce (a
little-endian u64) and the deployer's 96-byte public key, so anyone can
check a deployed contract by rebuilding its WASM:

1. Check out this repository at the deployment's commit, and `rusk-private`
   at the commit in [`RUSK_VERSION`](../RUSK_VERSION) where the `rusk` link
   reaches it: at `../../rusk-private`, as in the
   [README](../README.md#building), or anywhere else with the link pointed
   there.
2. Run `make all`. rustup installs the toolchain `rust-toolchain.toml` pins,
   and `Cargo.lock` pins everything else.
3. Hash each WASM with its deploy nonce and the deployer's key, and compare
   the result with the contract ID:

   ```bash
   python3 - target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_mailbox.wasm \
       <deploy nonce> <deployer public key, 96 bytes in hex> <<'PY'
   import hashlib, sys
   h = hashlib.blake2b(digest_size=32)
   h.update(open(sys.argv[1], 'rb').read())
   h.update(int(sys.argv[2]).to_bytes(8, 'little'))
   h.update(bytes.fromhex(sys.argv[3]))
   print(h.hexdigest())
   PY
   ```

   `deploy/testnet.sh` deploys with `dusk-tx deploy-hyperlane`, which uses
   nonces 0 to 9 in this order: TestMock, TestRecipient, MerkleTreeHook,
   the MessageIdMultisig ISM, ProtocolFee, AggregationHook, Mailbox,
   ValidatorAnnounce, IGP and WarpNative.
4. Check that the owner the node records for each contract is the
   deployer's key, the same 96 bytes you hashed:

   ```bash
   curl -s -X POST -H 'Content-Type: application/octet-stream' \
       https://<rusk-node>/on/contract_owner:<contract id>/owner | xxd -p | tr -d '\n'
   ```

   `dusk-tx deploy-wasm` makes the same check, and more for the Multicall
   with `--check-multicall`, before it accepts a contract already at the ID
   it expects.

Where the checkouts are, where rustup and Cargo keep their files, and the
user's name don't change the bytes. Panic locations are remapped to relative
paths, and Cargo reaches `rusk-private` through the `rusk` link, inside this
workspace. That matters: Cargo hashes a path dependency outside the
workspace by its absolute path into every symbol name, which in a stripped,
LTO-built WASM still reorders functions and data. When the workspace
depended on `../../rusk-private` directly, two checkouts built the same
WASMs only if `rusk-private` was at the same absolute path. Now builds with
`rusk-private` behind a symlink elsewhere, as a copy in another directory,
and with the toolchain installed at another path all give the same bytes.

Anything else does change them: another commit of either repository, a
changed `Cargo.lock`, another toolchain, or building the contracts some
other way than `make all`, which sets the profile, the stack size and the
path remapping.

## Rehearse locally

To run the whole procedure without touching real networks, use the local
demo environment (set up as in [demo/README.md](../demo/README.md)) with
Anvil standing in for Sepolia:

```bash
SKIP_OTTERSCAN=true SKIP_DUSK_EXPLORER=true bash demo/start-env.sh
export DUSK_MULTICALL_WASM=/path/to/dusk_multicall.wasm   # optional, see below
bash deploy/local-evm-core.sh            # Hyperlane core on Anvil; writes deploy/local.env
TESTNET_ENV=deploy/local.env bash deploy/testnet.sh deploy
TESTNET_ENV=deploy/local.env bash deploy/testnet.sh agent-configs
bash deploy/local-agents.sh start        # relayer and four validators
bash deploy/local-fees-check.sh          # the relaying fees, end to end
```

`local-fees-check.sh` sends paid transfers both ways and checks they are
delivered, checks that a message from Anvil that paid our IGP nothing, and
then only half of what it needs, isn't, and that topping it up delivers it,
checks that a message from Dusk whose delivery costs far more than it paid
for isn't delivered either, nor once `dusk-tx pay-for-gas` has paid half of
what it lacks, and is once the rest is paid, and claims the fees on both
chains. It takes about fifteen minutes.

The deploy takes about four minutes. Then run the commands from
[Check it works](#check-it-works) with the local values: Dusk at
`http://localhost:8080/`, Anvil at `http://localhost:8545`, Anvil's domain
`31338`, the rehearsal's keys (`rusk-private/examples/consensus.keys`,
password `password`) and Anvil account #0 as the EVM user. Transfers arrive
in about 30 seconds.

The rehearsal deploys a MessageIdMultisig ISM, as on testnet. A route or
Mailbox pointed at TestMock instead is refused unless
`LOCAL_REHEARSAL_ALLOW_TEST_MOCK=1` is set, and `deploy/testnet.sh` only
honours that when `DUSK_RUES_URL` and `EVM_RPC` are both local.

In the rehearsal:

- The Anvil core is hooked up as Sepolia's is: the Mailbox requires a
  ProtocolFee (1 wei), its default hook falls back to the MerkleTreeHook for
  Dusk, and a StaticAggregationHookFactory stands in for Hyperlane's. So the
  token's hook has to aggregate the MerkleTreeHook and the IGP, as on
  Sepolia, and transfers pay the same fees as on testnet at the default
  prices.
- Anvil account #2 stands in for Hyperlane's Sepolia validator, #3–#5 are the
  Dusk-origin validators (2 of 3), and #1 is the relayer's EVM key.
- The rehearsal signs for `DUSK_EXPECTED_CHAIN_ID` from `demo/.env.bridge`,
  the local devnet's 0 unless set, and `local-evm-core.sh` writes it into
  `local.env`. It, and `start-env.sh`, stop before signing anything when
  the node reports another chain.
- Checkpoints go to local directories under
  `deploy/out/local-rehearsal/agents/`, with logs and PID files.
- The local genesis has no Multicall for Dusk. With `DUSK_MULTICALL_WASM`
  set to its v0.1.0 Wasm (a dusk-multicall release asset, or `make wasm`
  there; SHA-256 `65f77a27…`), `local-evm-core.sh` deploys it with
  `dusk-tx deploy-wasm`, with deploy nonce 0 and the published owner, so it
  lands at the ID it has on testnet. `deploy-wasm` checks the Wasm, nonce
  and owner give that ID before it sends anything; the deploy takes about
  6.9M gas. An existing contract at the expected ID is not trusted without
  verification: `deploy-wasm` takes one only once Rusk reports the
  published owner for it and it answers `version` and an `aggregate` as the
  multicall does (`--check-multicall`), and otherwise stops. The agents then batch their contract reads through it, as on
  testnet. Without it, `local.env` sets `DUSK_MULTICALL` empty and they make
  single queries.
- `local-agents.sh` starts each agent with only `PATH`, `HOME`,
  `DUSK_TX_BIN`, its config and its keys. Hyperlane applies every `HYP_*`
  variable over the config files, so one left in your shell would
  otherwise change the rehearsal's agents, or point them at another node.
- Every agent signs its Dusk transactions with the deployer's key, because
  the local chain funds no other account with a key file. Don't send your own
  transactions from that key while the relayer is delivering, or one of them
  fails on the shared nonce.

Stop with `bash deploy/local-agents.sh stop` and `bash demo/stop-env.sh`.
Running `local-evm-core.sh` again starts a new rehearsal and moves the old
state aside.

## Before mainnet

Testnet runs on trust choices and tooling that mainnet can't:

- **Ownership.** One deployer key owns every contract and can change ISMs,
  hooks, fees and routers. Ownership moves in one step to an address nobody
  confirms, so a typo locks the contracts for good. Mainnet needs a two-step
  handover, owner changes behind a timelock with a guardian veto, a multisig
  owner on the EVM side and a custody plan for the Dusk owner key.
- **Deploy front-running.** A contract's ID depends only on its bytecode, the
  owner key and a nonce, and Rusk doesn't check that the owner signed the
  deploy. Anyone watching the mempool can deploy at our IDs first, and can
  repeat that for a fresh key. Each contract's `init` should require that the
  transaction signer is the owner, so such a deploy fails and leaves the ID
  free.
- **Validator set.** Sepolia → Dusk trusts one Abacus Works validator. Mainnet
  needs a deliberately chosen set for both directions.
- **Gas payments.** Both IGPs are priced and the relayer enforces payment
  ([costs.md](costs.md#fee-coverage)), but the prices are set by hand
  (`dusk-igp`, `evm-igp`). Mainnet needs them updated as ETH, DUSK and gas
  prices move, as Hyperlane's own oracle updater does, monitoring of what
  the relayer spends against what the IGPs collect, and the IGPs' and the
  oracle's ownership under the same custody as the rest.
- **Storage.** Anyone can add to most contracts' storage for good, paying
  gas and, for messages, fees, but each delivery and announcement takes a
  transaction, routes register only accounts that claimed a transfer, a
  WarpDrc20 keeps its last GiB for the bridge, and new dispatch payers fund
  0.01 DUSK. [storage.md](storage.md#ceilings) lists what filling each
  contract costs: 2,300 to 2,900 DUSK to stop a WarpDrc20's transfers to
  new holders at its 3 GiB reserve, 74,000 to 82,000 DUSK for
  ValidatorAnnounce, about 116,000 DUSK of dispatches to close the
  Mailbox's dispatch at the 0.001 DUSK protocol fee, and about 177,000 to
  make its `process` fail. Before mainnet: a successor Mailbox that routes
  can move to, a fee on deliveries through ISMs the deployment doesn't
  trust, a dust threshold for new WarpDrc20 holders and the mainnet
  protocol fee. The Mailbox and WarpDrc20 report their memory; WarpNative
  and WarpDrc20Collateral don't yet.
- **Upgrades.** The contracts are immutable. Plan how a fix ships: a pause
  limited to `dispatch` and `transfer_remote` (never `process` or claims), a
  redeploy, and a migration of route custody that doesn't depend on forging
  messages.
- **Testing.** Run the Rust and VM tests in CI. This needs a build against
  public crates, since `rusk-private` is private. Add fuzzing of the shared
  types against `hyperlane-core`, plus invariant tests over random operation
  sequences: route custody covers escrow, fee contracts hold what they owe,
  and the Merkle count matches the Mailbox nonce.
- **Registry.** Add Dusk's chain metadata and the warp route to the Hyperlane
  registry, so standard tooling and explorers know about them.
- **Monitoring.** Alert on:
  - relayer queue length and messages failing simulation;
  - validator checkpoint lag;
  - signer balances;
  - WarpNative custody against the EVM token's supply;
  - the Mailbox's `memory_bytes`, as dispatch closes just below 3 GiB.
- **Disclosure.** Publish a security contact and response process.
