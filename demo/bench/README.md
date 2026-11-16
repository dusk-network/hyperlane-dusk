# Agents benchmark

Measures what the Hyperlane agents cost a Rusk node: the requests each agent
sends to Rusk and Anvil, how long each stage of a message takes, and the
agents' CPU, memory, disk and log use, at idle and under load. Everything
runs locally, with the public development keys of the demo environment.
[docs/costs.md](../../docs/costs.md#agents-api-usage-latency-and-resources)
has the results.

It runs the local rehearsal of the testnet deployment
([docs/testnet.md](../../docs/testnet.md#rehearse-locally)): a relayer,
three Dusk-origin validators (2 of 3) and one EVM-origin validator (1 of 1),
with the configs `deploy/testnet.sh agent-configs` writes, and the native
DUSK route. Each agent reaches Rusk and Anvil through its own port of a
logging proxy. As on testnet, the agents batch their contract reads through
Multicall for Dusk (`batchContractAddress`), which the setup deploys on the
local chain at the ID it has on testnet, and the relayer delivers to Dusk
one at a time. `PIPELINE_DELIVERIES=1` turns on its experimental pipelined
deliveries instead ([docs/operations.md](../../docs/operations.md#deliveries-to-dusk)).

| File | What it does |
|---|---|
| `scenario.sh` | The whole run: fresh environment, deployment, agents, then the phases below. |
| `agents.sh` | Deploys the rehearsal into `BENCH_DIR`, writes one config per agent, starts and stops the agents, the proxy and the sampler, and records phase markers. |
| `proxy.py` | Reverse proxy, one port per agent and chain. Logs one JSON line per request: agent, call kind (contract query, GraphQL root field, JSON-RPC method, and the calls inside a multicall `aggregate`), status, bytes, time. |
| `dusk-tx-shim.sh` | The agents' `DUSK_TX_BIN`. Logs each `dusk-tx` run and its result (a pipelined delivery returns once submitted, with its nonce), and tags the helper's requests so the proxy tells them from the agent's own. |
| `sample.py` | CPU, RSS and I/O of each agent, the node and Anvil every 5 s; database, event cursor, checkpoint and log sizes every 30 s. |
| `load.sh` | Warp transfers on the native route, as in docs/testnet.md: Dusk to EVM one after another, EVM to Dusk paced or as a burst. |
| `report.py` | Turns a run directory into the tables in docs/costs.md. |

## Running it

Needs what the demo needs (Rusk built with `archive`, the genesis state,
Foundry, `jq`, Python 3), `demo/.env.bridge`, built contracts and `dusk-tx`
(`make all dusk-tx`), the agent binaries, and Multicall for Dusk's v0.1.0
Wasm (a dusk-multicall release asset, or its `make wasm`; SHA-256
`65f77a27…`). Ports 8080, 8545, 9000, 9090-9094, 18099-18109 and
18200-18209 must be free. The run takes about two hours with the defaults.

```bash
export BENCH_DIR=/some/empty/scratch/dir
export DUSK_MULTICALL_WASM=/path/to/dusk_multicall.wasm
# Optional: agents from a fixed copy instead of hyperlane-monorepo/rust/main
export AGENT_BIN_DIR=/path/with/relayer-and-validator AGENT_RUN_DIR=/dir/holding/config
bash demo/bench/scenario.sh
python3 demo/bench/report.py "$BENCH_DIR" > report.md
```

`AGENT_RUN_DIR` must contain Hyperlane's `config/` directory (copy
`hyperlane-monorepo/rust/main/config`). Phase lengths and counts are
environment variables at the top of `scenario.sh`. `PHASES` picks the load
phases to run, `BATCH_READS=0` leaves `batchContractAddress` out of the
configs (every read is one query), and `PIPELINE_DELIVERIES=1` gives the
relayer `pipelineDeliveries` and a `maxBatchSize` of
`DUSK_RELAYER_MAX_BATCH_SIZE` (16, from 2 to 64). The runs in docs/costs.md
pipelined with agents from before that setting existed (`0d8af2dda2`), on
`maxBatchSize` 16 alone; with later agents they need
`PIPELINE_DELIVERIES=1`. The comparison without the multicall ran:

```bash
BATCH_READS=0 PHASES="d2e d2e-backlog" IDLE2_SECS=600 bash demo/bench/scenario.sh
```

Each agent starts with only `PATH`, `HOME`, its config, its keys and the
shim's variables. Hyperlane applies every `HYP_*` variable over the config
files, so one left in your shell, such as `HYP_CHAINS_DUSK_CUSTOMRPCURLS`,
would otherwise send an agent past the proxy to another node.

The phases, in order:

1. `baseline`: node and Anvil, no agents.
2. `startup`: the validators announce one at a time, then the relayer starts.
3. `idle`: all agents, no traffic.
4. `d2e`: Dusk to EVM transfers, one after another.
5. `d2e-backlog`: the relayer is stopped while more are sent, then restarted.
6. `e2d`: EVM to Dusk transfers, paced slower than the relayer delivers them.
7. `e2d-burst`: a burst of EVM to Dusk transfers, with a relayer restart
   `RESTART_AFTER` seconds (2) after it has sent the first of their
   deliveries, while it sends the rest.
8. `outage`: the agents' Rusk ports refuse connections while EVM to Dusk
   transfers are sent, then reopen.
9. `idle2`: no traffic, after the load.

The agents pace their own contract queries (`contractQueriesPerSecond`,
100 a second by default), so the proxy passes them on as they come. Agents
from before that need `PACE_RELAYER=100`, which holds the relayer's
contract queries to 100 a second at the proxy: without it their restart in
`d2e-backlog` runs into Rusk's default HTTP rate limit and keeps retrying
(docs/costs.md, "Recovery").

The load and the relayer share the local development account on Dusk, so
Dusk to EVM transfers only run while the relayer has nothing to deliver to
Dusk. Transfers pay what the routes quote, as the relayer requires
(docs/costs.md, "Fee coverage"). After the phases, `scenario.sh` reads back
each transaction the agents sent to Dusk, and the Mailbox's transactions by
block, since a pipelined delivery's `dusk-tx` returns before its block. A Rusk restart is not part of the run: `demo/start-env.sh` starts the
node from the genesis archive each time, so a restarted node is a new chain.
The outage phase shows the agents' side of a node that goes away and comes
back.

The proxy and the shim add a few milliseconds per request and per helper
run. Everything they log stays in `BENCH_DIR`; the shim never reads the
signing key the agent passes the helper on stdin. `BENCH_DIR` does hold the
development signer key the agents use (`secrets/dusk-signer.key`, mode
0600), as `demo/gen-agent-configs.sh` run directories do: delete it when
the run is over, and don't upload the directory.
