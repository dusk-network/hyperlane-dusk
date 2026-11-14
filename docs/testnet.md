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
| Deployer | Dusk testnet | about 400 DUSK, and at least 100 DUSK spare per transaction | Contract deploys (about 300 DUSK at the measured sizes), router enrollment, 5 DUSK of dispatch credit. Exported from rusk-wallet as an encrypted consensus key file. |
| Deployer | Sepolia | about 0.05 ETH | ISM, token and enrollment transactions. |
| Relayer | Dusk testnet | 10 DUSK or more | `process` transactions for Sepolia → Dusk, about 2M gas each. |
| Relayer | Sepolia | 0.1 ETH or more | `process` transactions for Dusk → Sepolia. |
| Each Dusk-origin validator | EVM key, no funds | | Signing checkpoints. Its address goes in `DUSK_ORIGIN_VALIDATORS`. |
| Each Dusk-origin validator | Dusk testnet | 1 DUSK | Its one-time announcement on Dusk. |

Gas on Dusk:

- Deployments are charged 100 gas per WASM byte, at the network's minimum
  deploy gas price of 2000 LUX. On a local node the Mailbox costs about
  50 DUSK, each warp route about 40, and each other contract 10–35.
- Ordinary transactions only need a gas price above zero. The agent configs
  use 1 LUX (`DUSK_AGENT_GAS_PRICE`). At that price a relayed `process`,
  about 2M gas, costs 0.002 DUSK. At the deploy price it would cost 4 DUSK.

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
- the keys and validator settings are present and consistent.

`deploy` records every step in `deploy/out/<DEPLOY_NAME>/deployment.json` and
skips steps already recorded there. If it stops halfway, fix the cause and run
it again. Router enrollment and dispatch credit are re-checked on chain each
time.

## Run the agents

`agent-configs` writes two files to `deploy/out/<DEPLOY_NAME>/`. Neither
contains secrets.

- **`relayer.json`** relays the native route in both directions. Run one relayer:

  ```bash
  export DUSK_TX_BIN=/path/to/dusk-tx
  export HYP_CHAINS_SEPOLIA_SIGNER_KEY=0x...      # relayer's Sepolia key
  # relayer's Dusk key: a file with the 32-byte BLS secret key as hex,
  # mode 0600, at the keyFile path in the config (/run/secrets/dusk-signer.key)
  CONFIG_FILES=relayer.json relayer
  ```

- **`validator-dusk.json`** is a template for one Dusk-origin validator. Each
  operator copies it, sets `checkpointSyncer` to their own S3 bucket (public
  read, so the relayer can fetch checkpoints), and runs:

  ```bash
  export DUSK_TX_BIN=/path/to/dusk-tx
  export HYP_VALIDATOR_KEY=0x...       # one of DUSK_ORIGIN_VALIDATORS
  # plus AWS credentials for the bucket, and a Dusk key file for the
  # announcement transaction at the config's keyFile path
  CONFIG_FILES=validator-dusk.json validator
  ```

  On startup the validator announces its bucket on Dusk's ValidatorAnnounce.
  The relayer finds it there.

Every agent process needs its own `eventCursorDir` and `db`. The templates use
paths under `/var/lib/hyperlane/`.

## Check it works

Send 1 DUSK to Sepolia, then send it back. Use the state file for the
addresses:

```bash
S=deploy/out/<DEPLOY_NAME>/deployment.json
ROUTE=$(jq -r .dusk.warp_native $S); TOKEN=$(jq -r .evm.token $S)

# Dusk -> Sepolia: lock 1 DUSK (1e9 LUX), mint 1 DUSK on Sepolia
target/release/dusk-tx transfer-remote --native \
  --rues-url https://testnet.nodes.dusk.network/ --keys <your keys> \
  --warp-contract $ROUTE --destination 11155111 \
  --recipient 000000000000000000000000<your sepolia address> --amount 1000000000
cast call $TOKEN "balanceOf(address)(uint256)" <your sepolia address> --rpc-url $EVM_RPC

# Sepolia -> Dusk: burn on Sepolia, release on Dusk. The recipient is
# keccak256 of your Dusk BLS public key. Register it on the route first or
# the DUSK waits in escrow until you do (see dusk-tx register-account).
cast send $TOKEN "transferRemote(uint32,bytes32,uint256)" 1146442572 0x<recipient hash> 1000000000 \
  --value 1 --rpc-url $EVM_RPC --private-key <your key>
```

## Rehearse locally

To run the whole procedure without touching real networks, use the local
demo environment with Anvil standing in for Sepolia:

```bash
bash demo/start-env.sh
bash deploy/local-evm-core.sh                       # Hyperlane core on Anvil; writes deploy/local.env
TESTNET_ENV=deploy/local.env bash deploy/testnet.sh deploy
TESTNET_ENV=deploy/local.env bash deploy/testnet.sh agent-configs
```

In the rehearsal, Anvil account #2 stands in for Hyperlane's Sepolia
validator, and #3–#5 are the Dusk-origin validators (2 of 3). Point
`checkpointSyncer` at local directories (`{"type": "localStorage", "path":
...}`) and set `allowLocalCheckpointSyncers: true` in the relayer config.

## Before mainnet

Testnet runs on trust choices that mainnet can't:

- **Ownership.** One deployer key owns every contract and can change ISMs,
  hooks, fees and routers. Mainnet needs a multisig owner on the EVM side and a
  custody plan for the Dusk owner key.
- **Validator set.** Sepolia → Dusk trusts one Abacus Works validator. Mainnet
  needs a deliberately chosen set for both directions.
- **Gas payments.** Price the Dusk IGP for the EVM destination, get Dusk
  pricing into the EVM side's hook, and turn on gas-payment enforcement in the
  relayer.
- **Registry.** Add Dusk's chain metadata and the warp route to the Hyperlane
  registry, so standard tooling and explorers know about them.
- **Monitoring.** Alert on relayer queue length, validator checkpoint lag and
  signer balances.
