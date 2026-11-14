# Hyperlane for Dusk

Hyperlane's core contracts, hooks, security modules and warp routes, ported to
Dusk's WASM contract VM, along with the tooling needed to deploy and operate
them.

This repository holds the Dusk side. Relayer and validator support lives in
the Dusk fork of the Hyperlane monorepo,
[`dusk-network/hyperlane-monorepo`](https://github.com/dusk-network/hyperlane-monorepo),
in `rust/main/chains/hyperlane-dusk`.

## Status

- Works end to end on a local Rusk node bridged to Anvil, with real Hyperlane
  relayer and validator agents. Synthetic DRC20, native DUSK and collateral
  DRC20 routes are exercised in both directions, under both the `TestMock` and
  `MessageIdMultisig` security modules.
- Built and tested against Rusk 1.7.3, the release testnet and mainnet run
  (pinned in [`RUSK_VERSION`](RUSK_VERSION)). Both networks have the
  `keccak256` and `secp256k1_recover` host queries the contracts need.
- Not yet deployed to Dusk testnet or mainnet. [docs/testnet.md](docs/testnet.md)
  covers deploying to testnet and connecting to Sepolia.
- No external audit is recorded in this repository. See
  [docs/security.md](docs/security.md) for the internal review.

## Contracts

| Contract | Hyperlane equivalent | Notes |
|---|---|---|
| `mailbox` | `Mailbox` | Dispatch and process. Fees are paid from prepaid credit, not `msg.value`. |
| `merkle-tree-hook` | `MerkleTreeHook` | Incremental Merkle tree that validators checkpoint. |
| `aggregation-hook` | `StaticAggregationHook` | Fixed child set. Used as the required hook (MerkleTreeHook + ProtocolFee). |
| `protocol-fee` | `ProtocolFee` | Fixed fee per dispatch, claimable by the owner to a beneficiary. |
| `igp` | `InterchainGasPaymaster` | Per-domain gas pricing. The default hook. |
| `ism-multisig` | `MessageIdMultisigIsm` | m-of-n ECDSA validator signatures via `secp256k1_recover`. |
| `validator-announce` | `ValidatorAnnounce` | Registry of validator checkpoint storage locations. |
| `warp-drc20` | `HypERC20` (synthetic) | Mints and burns a bridged token. |
| `warp-drc20-collateral` | `HypERC20Collateral` | Locks and unlocks an existing DRC20 token. |
| `warp-native` | `HypNative` | Bridges native DUSK. |

`test-recipient`, `test-mock` (a pass-through ISM plus a no-op hook),
`reentrant-hook` and `redteam-attacker` exist for tests and demos only.

Routing, Aggregation and CCIP-read ISMs and rate-limited hooks or ISMs are not
implemented. The agents return an explicit error if a deployment points at one.

## How Dusk differs from EVM Hyperlane

- **Addresses.** A contract's 32-byte `ContractId` is used directly as its
  Hyperlane `H256` address. A user account is `keccak256` of its BLS public
  key.
- **Receiving tokens.** A Dusk account must call `register_account` on a warp
  route once, so the route can map the recipient hash back to a BLS key.
  Transfers to an unregistered hash are held in escrow until that key
  registers and calls `claim_pending`.
- **Paying fees.** Dusk has no `msg.value`. Senders prepay Mailbox credit with
  `fund_dispatch` (an exact Moonlight deposit), and each dispatch spends it.
  Unused credit can be withdrawn by the payer. Warp routes expose an
  owner-only withdrawal of their own credit.
- **Reentrancy.** The Dusk VM allows a callee to call back into its caller.
  The Mailbox holds an explicit guard across hook calls during dispatch, and
  marks a message delivered before any external call during `process`.
- **No upgrades.** Contract IDs are deterministic and contracts are
  immutable. An upgrade means a new deployment plus a config migration.
- **Finality.** Agents only index finalized events, and validators only
  checkpoint finalized Merkle state.

[docs/security.md](docs/security.md) has the full list of assumptions,
deviations from the Solidity contracts, and the remaining design decisions.

## Repository layout

```
contracts/       Contract crates (WASM, built with dusk-forge)
types/           Shared types: message encoding, Merkle tree, metadata,
                 token messages, events. Also used by the agent crate.
tests/           VM integration tests (dusk-vm)
dusk-tx/         CLI that builds and signs Moonlight transactions.
                 Deploys contracts and is the agents' transaction helper.
wasm-bindings/   JavaScript bindings for the shared types
ts-sdk/          TypeScript SDK (@hyperlane-dusk/sdk) over Rusk's HTTP API
data-driver/     Data driver that lets the Dusk explorer decode these contracts
demo/            Local bridge environment, deploy scripts and E2E/fault tests
e2e/             Rust E2E test against a running node
scripts/         Build and check helpers
docs/            Security notes and operations guide
```

## Building

The workspace builds against Dusk's private Rusk repository by relative path,
so you need access to `dusk-network/rusk-private`, checked out at the commit
in [`RUSK_VERSION`](RUSK_VERSION). `make` refuses to build against any other
commit (override with `ALLOW_RUSK_MISMATCH=1`). Lay out the checkouts like
this:

```
<workspace>/
├── rusk-private/            dusk-network/rusk-private
└── hyperlane/
    ├── dusk/                this repository
    └── hyperlane-monorepo/  dusk-network/hyperlane-monorepo (for agents)
```

To keep an existing `rusk-private` checkout untouched, put the pinned commit
in a worktree and build from a separate workspace directory:

```bash
git -C ~/rusk-private worktree add --detach <workspace>/rusk-private \
    "$(sed -n 's/^RUSK_COMMIT=//p' RUSK_VERSION)"
```

The Rust toolchain is pinned in `rust-toolchain.toml`. rustup installs it,
along with `rust-src` and the `wasm32-unknown-unknown` target.

```bash
make all               # build every contract WASM into target/contract/
make test-types        # shared types unit tests
make test-integration  # build the WASMs, then run the VM integration tests
make test              # both of the above
make clippy-contracts  # lint the production contracts for wasm
make dusk-tx           # build the dusk-tx CLI
make data-driver       # build the explorer data driver
make test-sdk          # build the JS bindings and run the TypeScript SDK tests
make help              # everything else
```

To build and test against a different Rusk checkout without touching the one
next to this repository, set `RUSK_DIR`. The script builds in a temporary
layout:

```bash
RUSK_DIR=/path/to/rusk-private make repro-check        # contracts + tests
RUSK_DIR=/path/to/rusk-private make repro-check-agent  # also cargo check the agents
```

## Running locally

`demo/` brings up a local Rusk node, Anvil and both block explorers, deploys
the full contract set on each side, and bridges tokens between them. It needs
a Rusk binary built with `--features archive` from the same `rusk-private`
checkout, and a Dusk consensus key for an account funded in the local genesis.

```bash
cp demo/bridge.env.example demo/.env.bridge   # then set the two secrets in it
bash demo/start-env.sh
bash demo/deploy.sh --dusk-ism testMock
bash demo/bridge.sh to-dusk 3
bash demo/bridge.sh to-evm 1
bash demo/stop-env.sh
```

`bash demo/e2e-agents.sh` runs the full agent flow (relayer plus validators)
for both security modules. With `--only messageIdMultisig --validators 3
--threshold 2` it runs a 2-of-3 validator set on each chain and checks that
messages deliver at the threshold, stall below it, and resume when a
validator returns. The other `demo/e2e-*.sh` scripts inject faults:
RPC outages, validator delay, corrupt checkpoint metadata, duplicate relayers,
low signer balance and restart soak. See [demo/README.md](demo/README.md).

## Running the agents

The agent crate submits transactions through the `dusk-tx` helper. Build it
with `make dusk-tx` and put it on `PATH`, or set `DUSK_TX_BIN` to the binary.

A Dusk chain entry in an agent config looks like this:

```
"dusk": {
  "name": "dusk",
  "protocol": "dusk",
  "domainId": 4242,
  "chainId": <chain id reported by the node>,
  "rpcUrls": [{ "http": "https://<rusk-node>" }],
  "eventCursorDir": "/var/lib/hyperlane/dusk-relayer-events",
  "gasLimit": 30000000,
  "gasPrice": 2000,
  "mailbox": "0x<mailbox contract id>",
  "interchainGasPaymaster": "0x<igp contract id>",
  "validatorAnnounce": "0x<validator-announce contract id>",
  "merkleTreeHook": "0x<merkle-tree-hook contract id>",
  "signer": { "type": "duskKey", "keyFile": "/run/secrets/dusk-signer.key" }
}
```

Every agent process needs its own `eventCursorDir`. The `duskKey` signer takes
exactly one of `keyFile` (a file with no group or world permissions),
`keyEnv`, or an inline `key` (local development only). Domain 4242 and the gas
values are the local demo's defaults. `demo/gen-agent-configs.sh` generates
complete working configs for the local environment.

Key custody, secret handling and dispatch-credit funding are covered in
[docs/operations.md](docs/operations.md).

## License

MIT OR Apache-2.0.
