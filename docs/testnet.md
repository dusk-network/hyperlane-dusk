# Testnet deployment

How to deploy the bridge to Dusk testnet, connect it to Sepolia through
Hyperlane's existing Sepolia contracts, and run the agents.
`deploy/testnet.sh` does the deployment; this page covers what it sets up,
what you need first, and how to run and check it.

## What gets deployed

| Where | What |
|---|---|
| Dusk testnet | The full contract set (Mailbox, Merkle hook, ProtocolFee, AggregationHook, IGP, ValidatorAnnounce, MessageIdMultisig ISM, TestRecipient, TestMock) and a native DUSK warp route (`warp-native`). |
| Sepolia | A synthetic DUSK token (`HypERC20`, 9 decimals so one unit is one LUX), and a MessageIdMultisig ISM for messages from Dusk. Everything else is Hyperlane's own Sepolia deployment. |

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

Hyperlane's Sepolia Mailbox has no gas pricing for Dusk. Its default hook falls
back to plain Merkle insertion, so a transfer to Dusk costs only Hyperlane's
1 wei protocol fee. The Dusk IGP's pricing for Sepolia is likewise token-sized
by default. The generated relayer config therefore doesn't enforce gas
payments, and relays only this deployment's routes.

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
| Deployer | Sepolia | about 0.05 ETH | ISM, token and enrollment transactions. |
| Relayer | Dusk testnet | a few DUSK | `process` transactions for Sepolia → Dusk, about 0.0022 DUSK each, and 0.005 DUSK for one that fails. |
| Relayer | Sepolia | 0.1 ETH or more | `process` transactions for Dusk → Sepolia. |
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
  times the cost at `EVM_ORIGIN_THRESHOLD` or, once the contracts are
  deployed, at the threshold of the ISM the route uses on chain, whichever
  is higher. The owner can raise that threshold after deploying, so raise
  the relayer's limit with it. Validators keep `DUSK_AGENT_GAS_LIMIT` (30M)
  for their one-time announcement.
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
# edit: DUSK_KEYS, DUSK_ORIGIN_VALIDATORS, DUSK_ORIGIN_THRESHOLD
export DUSK_CONSENSUS_PASSWORD=...   # password of DUSK_KEYS
export EVM_DEPLOYER_KEY=0x...        # Sepolia deployer

bash deploy/testnet.sh preflight     # read-only checks, safe to run any time
bash deploy/testnet.sh deploy
bash deploy/testnet.sh agent-configs
```

`preflight` checks that:
- the Dusk node is on the expected chain, has both host queries active, allows
  third-party contracts and serves the archive queries the agents need;
- the Sepolia Mailbox, Merkle hook and ValidatorAnnounce belong together, and
  a dispatch to the Dusk domain can be quoted;
- the keys and validator settings are present and consistent;
- a deployment already recorded under `deploy/out/<DEPLOY_NAME>/` is on this
  chain, and its contracts are at this checkout's `state_version`s.

`deploy` records every step in `deploy/out/<DEPLOY_NAME>/deployment.json` and
skips steps already recorded there. If it stops halfway, fix the cause and run
it again. Router enrollment is re-checked on chain each time. If the recorded
contracts aren't on chain, for example after a testnet reset, or a recorded
contract reports another `state_version` than this checkout's (Mailbox 5,
MerkleTreeHook 2, IGP 3, ValidatorAnnounce 2, native route 4), `deploy`
stops; move the directory aside and deploy again. Older routes may pay
sponsored escrow claims to forged keys
([security notes](security.md#issues-found-and-fixed), CRITICAL-1), and
older core contracts keep storage anyone can fill
([storage.md](storage.md)), so `agent-configs` refuses them too.

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

- **`relayer.json`** relays the native route in both directions. Run one relayer:

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
(Mailbox 5, MerkleTreeHook 2, ValidatorAnnounce 2), and an agent build that
reads them: deliveries come from `ProcessId` events, checkpoint roots are
computed from the Merkle hook's message IDs, and validator locations come
from `ValidatorAnnouncement` events ([storage.md](storage.md#what-agents-read)).

Each Dusk transaction an agent sends is logged as `dusk-tx call succeeded`
with its `tx_id`. Its gas use and result are in the node's GraphQL API:
`query { tx(hash: "<tx_id>") { gasSpent err } }` at `/on/graphql/query`.

## Check it works

Send 1 DUSK to Sepolia, then send it back. Use the state file for the
addresses:

```bash
S=deploy/out/<DEPLOY_NAME>/deployment.json
ROUTE=$(jq -r .dusk.warp_native $S); TOKEN=$(jq -r .evm.token $S)
DUSK=https://testnet.nodes.dusk.network/; EVM_RPC=https://ethereum-sepolia.publicnode.com
export DUSK_CONSENSUS_PASSWORD=...

# Dusk -> Sepolia: lock 1 DUSK (1e9 LUX), mint 1 DUSK on Sepolia. The
# deposit is the amount plus the quoted dispatch fee; dusk-tx adds the fee.
target/release/dusk-tx transfer-remote --native --rues-url $DUSK --keys my.keys \
  --warp-contract $ROUTE --destination 11155111 \
  --recipient 000000000000000000000000<your sepolia address, no 0x> --amount 1000000000
cast call $TOKEN "balanceOf(address)(uint256)" <your sepolia address> --rpc-url $EVM_RPC

# Sepolia -> Dusk: burn on Sepolia, release on Dusk. Register your account on
# the route once first; until you do, transfers to you wait in escrow.
target/release/dusk-tx register-account --rues-url $DUSK --keys my.keys --warp-contract $ROUTE
H=$(target/release/dusk-tx account --offline --keys my.keys | jq -r .account_h256)
cast send $TOKEN "transferRemote(uint32,bytes32,uint256)" 1146442572 0x$H 1000000000 \
  --value 1 --rpc-url $EVM_RPC --private-key <your key>
target/release/dusk-tx account --rues-url $DUSK --keys my.keys
```

`--value 1` pays Hyperlane's 1 wei protocol fee on Sepolia.

`--recipient` takes a 20-byte EVM address and pads it to 32 bytes, as
Hyperlane does. `dusk-tx` refuses a 32-byte value whose first 12 bytes aren't
zero: no EVM route can deliver to it, and the DUSK would stay locked on Dusk.
`--raw-recipient` overrides that for non-EVM destinations.

**Escrow.** A transfer to a Dusk account that hasn't registered on the route
waits in escrow. A recipient whose only DUSK is in that escrow can't pay gas
to claim it, so anyone can claim it for them. The route decodes the public
key the claimant names and pays the account that signs with it, whose hash
the escrow is under. The claimant only pays the gas:

```bash
# What's waiting, and whether the account is registered
target/release/dusk-tx account --rues-url $DUSK --address <base58 address> --warp-contract $ROUTE
# Claim your own escrow, or someone else's with --for; either also registers the account
target/release/dusk-tx claim-pending --rues-url $DUSK --keys my.keys --warp-contract $ROUTE \
  [--for <base58 address>]
# Register your own account in advance, so transfers arrive directly
target/release/dusk-tx register-account --rues-url $DUSK --keys my.keys --warp-contract $ROUTE
```

Nobody can register an account for someone else in advance: only a claim of
an existing escrow registers another account.

**Undeliverable messages.** Some transfers can never be delivered. A
transfer to the zero recipient is one example, and so is anything that
makes the destination's `handle` revert. The relayer simulates each
delivery first, finds that it would fail, and retries it with backoff
without paying for it. The tokens stay where they are: burned on Sepolia,
or locked in the Dusk route. Add such messages to the relayer's denylist to
stop the retries.

## Rehearse locally

To run the whole procedure without touching real networks, use the local
demo environment (set up as in [demo/README.md](../demo/README.md)) with
Anvil standing in for Sepolia:

```bash
SKIP_OTTERSCAN=true SKIP_DUSK_EXPLORER=true bash demo/start-env.sh
bash deploy/local-evm-core.sh            # Hyperlane core on Anvil; writes deploy/local.env
TESTNET_ENV=deploy/local.env bash deploy/testnet.sh deploy
TESTNET_ENV=deploy/local.env bash deploy/testnet.sh agent-configs
bash deploy/local-agents.sh start        # relayer and four validators
```

The deploy takes about four minutes. Then run the commands from
[Check it works](#check-it-works) with the local values: Dusk at
`http://localhost:8080/`, Anvil at `http://localhost:8545`, Anvil's domain
`31338`, the rehearsal's keys (`rusk-private/examples/consensus.keys`,
password `password`) and Anvil account #0 as the EVM user. Transfers arrive
in about 30 seconds.

In the rehearsal:

- Anvil account #2 stands in for Hyperlane's Sepolia validator, #3–#5 are the
  Dusk-origin validators (2 of 3), and #1 is the relayer's EVM key.
- Checkpoints go to local directories under
  `deploy/out/local-rehearsal/agents/`, with logs and PID files.
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
- **Gas payments.** Price the Dusk IGP for the EVM destination, get Dusk
  pricing into the EVM side's hook, and turn on gas-payment enforcement in the
  relayer.
- **Storage.** Anyone can add to most contracts' storage for good, paying
  gas and, for messages, fees. [storage.md](storage.md#ceilings) lists what
  filling each contract costs: about 3,700 DUSK for a WarpDrc20's
  allowances, 4,300 DUSK of deliveries to close the Mailbox's dispatch, and
  23,000 to 47,000 DUSK for a route's registry
  ([security notes](security.md#open-registry-growth)). Only the Mailbox
  reports its memory. Decide which limits mainnet needs, and price or bound
  the rest.
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
