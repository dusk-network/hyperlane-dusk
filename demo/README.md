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
make all                # 12 contract WASMs
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
bash demo/deploy.sh --dusk-ism testMock

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
- Each ISM case runs synthetic DRC20, native DUSK, and DRC20-collateral routes
  in both directions. It checks exact native and token custody, DRC20 allowance
  consumption, and one value-backed ProtocolFee collection per outbound Dusk
  dispatch.
- The script generates temporary agent configs in `/tmp` with restrictive file
  permissions, uses them only while agents run, and deletes them on exit.
- Dusk consensus key passwords are passed to `dusk-tx` through environment
  variables instead of CLI arguments, so they do not appear in process argv.
  For production-style local testing, prefer `DUSK_CONSENSUS_PASSWORD_FILE`.

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

For higher-count local runs, lower the per-transfer amount so the funded local
EVM test account can cover every burn:

```bash
TRANSFERS=20 TRANSFER_AMOUNT_WEI=500000000000000000 \
  bash demo/e2e-relayer-restart-stress.sh
```

For soak-style repetition of the same restart/backlog scenario, run:

```bash
SOAK_CYCLES=3 TRANSFERS=20 TRANSFER_AMOUNT_WEI=500000000000000000 \
  bash demo/e2e-soak-restart-stress.sh
```

Set `SOAK_MINUTES` to time-box the soak. The wrapper starts a fresh local
environment for each cycle and writes a summary log plus per-cycle logs under
`/tmp`.

### Validator Delay E2E

To verify MessageIdMultisig delivery waits for validator checkpoint metadata:

```bash
VALIDATOR_DELAY_SECS=35 bash demo/e2e-validator-delay.sh
```

The script starts a `messageIdMultisig` deployment, runs the relayer before the
validator, sends an EVM -> Dusk transfer, verifies the Dusk-side token supply
does not change during the configured validator delay, starts the validator,
and then waits for delivery through the normal relayer retry path.

### Corrupt Checkpoint Metadata E2E

To verify corrupted MessageIdMultisig checkpoint metadata does not deliver:

```bash
CORRUPT_METADATA_SECS=35 bash demo/e2e-corrupt-checkpoint-metadata.sh
```

The script creates a valid EVM -> Dusk validator checkpoint, stops the
validator, corrupts the local checkpoint signature, starts the relayer, verifies
delivery remains blocked while the metadata is corrupt, restores the checkpoint,
and verifies delivery resumes.

### Low Dusk Signer Balance E2E

To verify EVM -> Dusk delivery does not complete when the relayer's Dusk
destination signer cannot pay fees:

```bash
LOW_SIGNER_SECS=35 bash demo/e2e-low-dusk-signer-balance.sh
```

The script starts a TestMock deployment, rewrites a temporary relayer config to
use a deterministic unfunded Dusk test key, verifies delivery remains blocked
while Dusk rejects the relayer transaction for insufficient account balance,
then restarts the relayer with the funded local dev key and verifies recovery.

### Origin RPC Failure E2E

To verify EVM -> Dusk delivery recovers after an origin RPC outage:

```bash
RPC_FAILURE_SECS=35 bash demo/e2e-origin-rpc-failure.sh
```

The script starts a TestMock deployment, rewrites a temporary relayer config to
point Anvil RPC reads at an unreachable local port, submits an EVM -> Dusk
message through the healthy Anvil RPC, verifies Dusk-side delivery remains
blocked during the RPC failure window, then restarts the relayer with the
healthy RPC config and verifies delivery.

### Destination RPC Failure E2E

To verify EVM -> Dusk delivery recovers after a Dusk RUES outage:

```bash
RPC_FAILURE_SECS=35 bash demo/e2e-destination-rpc-failure.sh
```

The script starts a TestMock deployment, rewrites a temporary relayer config to
point Dusk RPC reads/submissions at an unreachable local port, submits an EVM ->
Dusk message, verifies delivery remains blocked during the RPC failure window,
then restarts the relayer with the healthy Dusk RPC config and verifies
delivery.

### Duplicate Relayer Attempt E2E

To verify concurrent relayers do not double-deliver an EVM -> Dusk message:

```bash
STABILITY_SECS=30 bash demo/e2e-duplicate-relayer-attempt.sh
```

The script starts two relayers with separate databases and metrics ports,
submits one EVM -> Dusk transfer, waits for the first delivery, then keeps both
relayers running and verifies Dusk-side token supply remains stable.

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

### Secret Handling

The demo and E2E scripts use deterministic local development keys. They are not
production deployment scripts.

`dusk-tx` resolves encrypted `consensus.keys` passwords in this order:

1. `DUSK_CONSENSUS_PASSWORD_FILE`
2. `DUSK_CONSENSUS_PASSWORD`
3. `DUSK_CONSENSUS_KEYS_PASS`
4. `--password` / the CLI default, intended for local demos only

When a raw BLS secret key is unavoidable, pass it with `--secret-key-stdin` so
it does not appear in shell history or process argv.

`demo/gen-agent-configs.sh` writes temporary agent config files and Dusk signer
key files under `/tmp` with `umask 077`. The configs point at Dusk key files
and still contain local Anvil signer material; neither the configs nor the key
files may be committed, uploaded as CI artifacts, or reused for production.
The E2E wrappers delete generated Dusk signer key files and generated agent
config files on exit after stopping running agents; logs and non-secret path
references are left for debugging.
The EVM private keys used by these scripts are Anvil dev keys only.

Run `make secret-hygiene` before review. Before uploading CI or E2E artifacts,
scan the exact artifact paths with `bash scripts/secret-hygiene-check.sh
<paths...>`. See `SECRET_HANDLING.md` for the release guardrail.

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
  |  Mailbox + ISM/hooks    |           |  Mailbox + selected ISM |
  |                         |           |  +- AggregationHook     |
  |  HypERC20 routes       |<--------->|  |  +- MerkleTreeHook   |
  |  +- synthetic DRC20    |  enrolled  |  |  +- ProtocolFee     |
  |  +- native DUSK token  |  routers   |  +- IGP                |
  |  +- collateral token   |            |                         |
  |                         |           |  Warp routes            |
  |  TestRecipient          |           |  +- WarpDrc20          |
  |                         |           |  +- WarpNative         |
  |                         |           |  +- Drc20Collateral    |
  +-------------------------+           +-------------------------+

  EVM -> Dusk: HypERC20.transferRemote -> relay -> Dusk route handle
  Dusk -> EVM: Dusk route transfer_remote -> value-backed hooks -> relay -> HypERC20.handle
```

## Key dusk-tx Commands

```bash
# Deploy the full local route matrix on Dusk
dusk-tx deploy-hyperlane --domain 4242 --default-ism testMock --deploy-warp-drc20 \
    --deploy-warp-native --warp-collateral-token warp-drc20

# Pre-fund value-backed Mailbox dispatch fees for a route
dusk-tx fund-dispatch --mailbox <hex> --payer <route-hex> --amount <lux>

# Withdraw a Moonlight account's own unused credit to the signing account
dusk-tx withdraw-dispatch --target <mailbox-hex> --amount <lux>

# Select an explicit operational treasury instead of the signing account
dusk-tx withdraw-dispatch --target <mailbox-hex> --amount <lux> \
    --recipient-public-key <96-byte-moonlight-public-key-hex>

# A warp-route owner can withdraw that route's credit to the signer or an
# explicit Moonlight treasury (the route still enforces owner authorization)
dusk-tx withdraw-dispatch --target <route-hex> --amount <lux>

# Query contract state
dusk-tx query --contract <hex> --method nonce --return-type u32

# Enroll a remote router
dusk-tx enroll-router --warp-contract <hex> --domain 31338 --router <hex>

# Register BLS key for receiving bridged tokens
dusk-tx register-account --warp-contract <hex>

# Approve collateral custody and inspect DRC20 balances
dusk-tx drc20-approve --token <hex> --spender <collateral-route-hex> --amount <amount>
dusk-tx drc20-balance --token <hex>
dusk-tx drc20-balance --token <hex> --account-contract <hex>

# Send tokens to a remote chain
dusk-tx transfer-remote --warp-contract <hex> --destination 31338 \
    --recipient <hex> --amount 1000000000000000000

# Native DUSK sends attach an exact Moonlight deposit
dusk-tx transfer-remote --warp-contract <native-route-hex> --destination 31338 \
    --recipient <hex> --amount 100000000 --native

# Encode a Hyperlane message (no TX)
dusk-tx encode-message --nonce 0 --origin 31338 --sender <hex> \
    --destination 4242 --recipient <hex> --body <hex>

# Process an inbound message
dusk-tx process --mailbox <hex> --message <hex>

# Dispatch a message via TestRecipient
dusk-tx dispatch --mailbox <hex> --test-recipient <hex> \
    --destination 31338 --recipient <hex> --body "Hello!"
```
