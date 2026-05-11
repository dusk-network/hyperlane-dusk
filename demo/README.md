# Hyperlane Dusk <-> EVM Cross-Chain Bridge

Demonstrates cross-chain token bridging between Dusk and Ethereum (Anvil) with
block explorers on both sides.

## Local Bridge Environment

Full local setup with Rusk (Dusk node), Anvil (EVM), and block explorers.

### Prerequisites

- **Rusk** (Dusk node) built from local fork at `~/projects/rusk`
- **Foundry** (anvil, forge, cast): `curl -L https://foundry.paradigm.xyz | bash && foundryup`
- **Docker**: for Otterscan (EVM explorer)
- **Node.js/npm**: for Dusk Explorer
- **jq**: `sudo apt-get install jq`

### One-Time Setup

```bash
# 1. Build Rusk (from local fork with secp256k1_recover)
cd ~/projects/rusk
make prepare-dev        # Generates circuit keys + genesis state (~10 min)
cargo build --release -p dusk-rusk

# 2. Build Hyperlane contracts + CLI
cd ~/projects/hyperlane/dusk
make all                # 11 contract WASMs
make dusk-tx            # CLI tool

# 3. Install Dusk Explorer deps
cd ~/projects/explorer
npm install
```

### Quick Start

```bash
# From the dusk/ directory:

# 1. Start all services (Rusk, Anvil, Otterscan, Dusk Explorer)
bash demo/start-env.sh

# 2. Deploy contracts on both chains
bash demo/deploy.sh

# 3. Bridge tokens!
bash demo/bridge.sh status          # Check balances
bash demo/bridge.sh to-dusk 3       # Bridge 3 wDUSK to Dusk
bash demo/bridge.sh to-evm 1        # Bridge 1 wDUSK back to EVM

# 4. Stop everything
bash demo/stop-env.sh
```

### Agent-Based E2E (Relayer + Validator)

To validate the actual Hyperlane agent flow (relayer, and validator when using
`messageIdMultisig`), run:

```bash
# Runs both:
# 1) Dusk Mailbox default ISM = testMock
# 2) Dusk Mailbox default ISM = messageIdMultisig
bash demo/e2e-agents.sh
```

Notes:

- Dusk contract deployments are deterministic. Switching ISM modes requires a
  fresh rusk state (the script handles this via `stop-env/start-env`).
- The script generates temporary agent configs in `/tmp` with restrictive file
  permissions (they contain dev keys).

### Dirty Redeploy Guard

To verify deterministic Dusk contract IDs are not silently reused on a non-reset
chain, run:

```bash
bash demo/e2e-dirty-redeploy.sh
```

The script starts a fresh local environment, deploys once, attempts a second
deployment without resetting Rusk state, and expects `deploy-hyperlane` to
refuse with recovery guidance.

### Relayer Restart Stress

To exercise repeated message delivery plus relayer restart/backlog recovery:

```bash
TRANSFERS=5 bash demo/e2e-relayer-restart-stress.sh
```

The script sends a burst of EVM -> Dusk transfers through a live relayer, stops
the relayer, queues the same number of Dusk -> EVM transfers, restarts the
relayer with the same config/database, and verifies final balances.

### Services & Ports

| Service        | Port | URL                     |
|----------------|------|-------------------------|
| Rusk (Dusk)    | 8080 | http://localhost:8080/   |
| Anvil (EVM)    | 8545 | http://localhost:8545/   |
| Dusk Explorer  | 5173 | http://localhost:5173/   |
| EVM Explorer   | 5100 | http://localhost:5100/   |

### Bridge Commands

```bash
# Show balances and contract addresses
bash demo/bridge.sh status

# Bridge from EVM to Dusk (whole tokens)
bash demo/bridge.sh to-dusk 5

# Bridge from Dusk to EVM (whole tokens)
bash demo/bridge.sh to-evm 2

# Help
bash demo/bridge.sh help
```

### Configuration

All settings are in `demo/.env.bridge`. Key overrides:

| Variable | Default | Description |
|----------|---------|-------------|
| `RUSK_DIR` | `~/projects/rusk` | Path to Rusk source |
| `EXPLORER_DIR` | `~/projects/explorer` | Path to Dusk Explorer |
| `RUSK_HTTP_PORT` | `8080` | Rusk HTTP/RUES port |
| `ANVIL_PORT` | `8545` | Anvil RPC port |
| `DUSK_EXPLORER_PORT` | `5173` | Dusk Explorer port |
| `EVM_EXPLORER_PORT` | `5100` | Otterscan port |

### Troubleshooting

**Rusk won't start**: Check `/tmp/rusk-dev.log`. Ensure genesis state exists at
`/tmp/example.state` (run `make prepare-dev` in the Rusk repo).

**Port conflict**: Change ports in `.env.bridge`. Note: if you change
`RUSK_HTTP_PORT` from 8080, also update the Vite proxy target in
`~/projects/explorer/vite.config.js`.

**Docker on WSL2**: Otterscan uses `--add-host=host.docker.internal:host-gateway`
which should work on WSL2. If not, replace with your WSL2 IP.

**Deploy fails on Dusk**: Ensure the deployer account has sufficient balance (LUX).
The consensus keys at `dusk/e2e/consensus.keys` must match a funded account on
the local Rusk node.

**Redeploy fails with "Refusing to deploy"**: You are re-running `dusk-tx
deploy-hyperlane` against a non-reset chain. Restart the demo environment
(`bash demo/stop-env.sh --force && bash demo/start-env.sh`) or use a different
deployer key so the deterministic contract IDs change.

---

## Standalone Demo (demo.sh)

Self-contained demo that deploys and bridges in one script. Expects a running
`rusk-duskevm` Docker container (RUES on port 18090).

```bash
# Build everything
make all           # Contract WASMs
make dusk-tx       # CLI tool

# Ensure Dusk node is running
docker start rusk-duskevm

# Run demo
bash demo/demo.sh

# Re-run without redeploying
bash demo/demo.sh --skip-deploy
```

## Architecture

```
   EVM (Anvil, domain=31338)              Dusk (domain=4242)
  +-------------------------+           +-------------------------+
  |  Mailbox                |           |  Mailbox                |
  |  +- TestIsm             |           |  +- NullISM (TestMock)  |
  |  +- TestPostDispatchHook|           |  +- MerkleTreeHook      |
  |  +- nonce: tracks msgs  |           |  +- nonce: tracks msgs  |
  |                         |           |                         |
  |  HypERC20 (wDUSK)      |<--------->|  WarpDrc20 (wDUSK)     |
  |  +- ERC20 mint/burn    |  enrolled  |  +- DRC20 mint/burn    |
  |  +- TokenRouter        |  routers   |  +- TokenRouter        |
  |                         |           |                         |
  |  TestRecipient          |           |  TestRecipient          |
  |  +- handle() stores msg|           |  +- dispatch_message()  |
  +-------------------------+           +-------------------------+

  Token Bridge (EVM->Dusk):
  HypERC20.transferRemote() -> Mailbox.dispatch() -> [relay] -> Mailbox.process() -> WarpDrc20.handle()

  Token Bridge (Dusk->EVM):
  WarpDrc20.transfer_remote() -> Mailbox.dispatch() -> [relay] -> Mailbox.process() -> HypERC20.handle()
```

## Key dusk-tx Commands

```bash
# Deploy all Hyperlane contracts on Dusk
dusk-tx deploy-hyperlane --domain 4242 --deploy-warp-drc20

# Query contract state
dusk-tx query --contract <hex> --method nonce --return-type u32

# Enroll a remote router
dusk-tx enroll-router --warp-contract <hex> --domain 31338 --router <hex>

# Register BLS key for receiving bridged tokens
dusk-tx register-account --warp-contract <hex>

# Send tokens to a remote chain
dusk-tx transfer-remote --warp-contract <hex> --destination 31338 \
    --recipient <hex> --amount 1000000000000000000

# Encode a Hyperlane message (no TX)
dusk-tx encode-message --nonce 0 --origin 31338 --sender <hex> \
    --destination 4242 --recipient <hex> --body <hex>

# Process an inbound message
dusk-tx process --mailbox <hex> --message <hex>

# Dispatch a message via TestRecipient
dusk-tx dispatch --mailbox <hex> --test-recipient <hex> \
    --destination 31338 --recipient <hex> --body "Hello!"
```
