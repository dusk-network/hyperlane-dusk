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
logging proxy.

| File | What it does |
|---|---|
| `scenario.sh` | The whole run: fresh environment, deployment, agents, then the phases below. |
| `agents.sh` | Deploys the rehearsal into `BENCH_DIR`, writes one config per agent, starts and stops the agents, the proxy and the sampler, and records phase markers. |
| `proxy.py` | Reverse proxy, one port per agent and chain. Logs one JSON line per request: agent, call kind (contract query, GraphQL root field, JSON-RPC method), status, bytes, time. |
| `dusk-tx-shim.sh` | The agents' `DUSK_TX_BIN`. Logs each `dusk-tx` run and its result, and tags the helper's requests so the proxy tells them from the agent's own. |
| `sample.py` | CPU, RSS and I/O of each agent, the node and Anvil every 5 s; database, event cursor, checkpoint and log sizes every 30 s. |
| `load.sh` | Warp transfers on the native route, as in docs/testnet.md: Dusk to EVM one after another, EVM to Dusk paced or as a burst. |
| `report.py` | Turns a run directory into the tables in docs/costs.md. |

## Running it

Needs what the demo needs (Rusk built with `archive`, the genesis state,
Foundry, `jq`, Python 3), `demo/.env.bridge`, built contracts and `dusk-tx`
(`make all dusk-tx`), and the agent binaries. Ports 8080, 8545, 9000,
9090-9094, 18099-18109 and 18200-18209 must be free. The run takes about two
hours with the defaults.

```bash
export BENCH_DIR=/some/empty/scratch/dir
# Optional: agents from a fixed copy instead of hyperlane-monorepo/rust/main
export AGENT_BIN_DIR=/path/with/relayer-and-validator AGENT_RUN_DIR=/dir/holding/config
bash demo/bench/scenario.sh
python3 demo/bench/report.py "$BENCH_DIR" > report.md
```

`AGENT_RUN_DIR` must contain Hyperlane's `config/` directory (copy
`hyperlane-monorepo/rust/main/config`). Phase lengths and counts are
environment variables at the top of `scenario.sh`.

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
   while it delivers them.
8. `outage`: the agents' Rusk ports refuse connections while EVM to Dusk
   transfers are sent, then reopen.
9. `idle2`: no traffic, after the load.

The proxy holds the relayer's contract queries to `PACE_RELAYER` a second
(default 100; `0` turns it off). Without it the relayer restart in
`d2e-backlog` runs into Rusk's default HTTP rate limit and keeps retrying,
and the phase waits until `DELIVERY_TIMEOUT` (docs/costs.md, "Recovery").

The load and the relayer share the local development account on Dusk, so
Dusk to EVM transfers only run while the relayer has nothing to deliver to
Dusk. A Rusk restart is not part of the run: `demo/start-env.sh` starts the
node from the genesis archive each time, so a restarted node is a new chain.
The outage phase shows the agents' side of a node that goes away and comes
back.

The proxy and the shim add a few milliseconds per request and per helper
run. Everything they log stays in `BENCH_DIR`; the shim never reads the
signing key the agent passes the helper on stdin. `BENCH_DIR` does hold the
development signer key the agents use (`secrets/dusk-signer.key`, mode
0600), as `demo/gen-agent-configs.sh` run directories do: delete it when
the run is over, and don't upload the directory.
