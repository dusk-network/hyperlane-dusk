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
- Relaying is paid for in both directions: each side's IGP charges the
  sender for the destination gas, with the operator's margin, and the
  generated relayer config relays any message once it is paid for
  ([docs/costs.md](docs/costs.md#fee-coverage)).
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
`reentrant-hook`, `redteam-attacker` and `callback-fee-token` (a DRC20 that
takes a fee and calls its sender back) exist for tests and demos only, and
so does `make warp-drc20-collateral-unguarded`, a build of the collateral
route without its reentrancy guard.

Routing, Aggregation and CCIP-read ISMs and rate-limited hooks or ISMs are not
implemented. The agents return an explicit error if a deployment points at one.

## How Dusk differs from EVM Hyperlane

- **Addresses.** A contract's 32-byte `ContractId` is used directly as its
  Hyperlane `H256` address. A user account is `keccak256` of its BLS public
  key.
- **Receiving tokens.** A warp route has to map a recipient hash back to a
  BLS key, so an account is registered on the route once. Transfers to an
  unregistered hash wait in escrow until they are claimed, which also
  registers the account. The recipient can claim them itself
  (`claim_pending`), or anyone can claim them for it (`claim_pending_for`)
  and pay the gas, so a recipient with no DUSK yet can still receive. The
  route decodes the key it is given and pays the account that signs with
  it.
- **Paying fees.** Dusk has no `msg.value`. A warp transfer's Moonlight
  deposit includes the quoted dispatch fee (for native DUSK, the amount plus
  the fee), and the route passes the fee on to the Mailbox. Contracts and
  accounts that call `dispatch` directly prepay Mailbox credit with
  `fund_dispatch` instead, and can withdraw what they don't use.
- **Reentrancy.** The Dusk VM allows a callee to call back into its caller.
  The Mailbox holds an explicit guard across hook calls during dispatch, and
  marks a message delivered before any external call during `process`. The
  collateral route holds one across `transfer_remote`, `handle` and its
  claims, since its token can call back into it.
- **No upgrades.** Contract IDs are deterministic and contracts are
  immutable. An upgrade means a new deployment plus a config migration.
- **Storage.** A contract's state is its wasm32 memory, at most 4 GiB, and
  it never shrinks. The contracts keep growing state in 64 KiB chunks and
  store only what they check: message IDs, not messages; 16 bytes of each
  delivered message's ID; hashes of validator locations. Agents read the
  rest from finalized events. [docs/storage.md](docs/storage.md) has the
  layouts, what filling each contract costs, and what agents read;
  [docs/costs.md](docs/costs.md) what every call costs.
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
docs/            Security notes, storage design, costs and operations guide
```

## Building

The workspace builds against Dusk's private Rusk repository, so you need
access to `dusk-network/rusk-private`, checked out at the commit in
[`RUSK_VERSION`](RUSK_VERSION). `make` refuses to build against any other
commit (override with `ALLOW_RUSK_MISMATCH=1`). Cargo reaches it through the
`rusk` link in this repository, which points at `../../rusk-private`, so lay
out the checkouts like this:

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

The contract WASMs are reproducible: the same commits of this repository and
`rusk-private` and the pinned toolchain give the same bytes, and so the same
contract IDs, wherever the checkouts, the toolchain and Cargo's home are.
[docs/testnet.md](docs/testnet.md#reproduce-the-contracts) shows how to check
a deployed contract.

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
  "gasLimit": 5000000,
  "gasPrice": 1,
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
values are the local demo relayer's defaults. A failed Dusk transaction pays
its whole `gasLimit`, so the relayer's is kept low; see
[docs/testnet.md](docs/testnet.md) for how it was chosen.
`demo/gen-agent-configs.sh` generates complete working configs for the local
environment.

Key custody, secret handling and dispatch-credit funding are covered in
[docs/operations.md](docs/operations.md).

## License

MIT OR Apache-2.0.
