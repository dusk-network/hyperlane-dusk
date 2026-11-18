# Testnet deployment

How to deploy the bridge to Dusk testnet, connect it to Sepolia through
Hyperlane's existing Sepolia contracts, and run the agents.
`deploy/testnet.sh` does the deployment; this page covers what it sets up,
what you need first, and how to run and check it.

## What gets deployed

| Where | What |
|---|---|
| Dusk testnet | The full contract set (Mailbox, Merkle hook, ProtocolFee, AggregationHook, IGP, ValidatorAnnounce, MessageIdMultisig ISM, TestRecipient, TestMock) and a native DUSK warp route (`warp-native`). For each token route `TOKEN_ROUTES` lists, EURd and USDd by default ([Tokens issued on Dusk](#tokens-issued-on-dusk)): the token, the pinned canonical DRC20, and a collateral route that locks it (`warp-drc20-collateral`). |
| Sepolia | A synthetic DUSK token (`HypERC20`, 9 decimals so one unit is one LUX), a synthetic token for each token route (`HypERC20`, with the token's name, symbol and decimals), a MessageIdMultisig ISM for messages from Dusk, an InterchainGasPaymaster with its own StorageGasOracle that charges for delivery to Dusk, and the tokens' hook: a StaticAggregationHook of Hyperlane's Sepolia MerkleTreeHook and that IGP, from Hyperlane's factory. The tokens and the IGP are each an `ERC1967Proxy` whose constructor initializes them, so the transaction that creates them also sets their owner, and nobody can initialize them first; the proxies have no admin, so they can't be upgraded. Everything else is Hyperlane's own Sepolia deployment. |

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
  DUSK for the Sepolia gas a delivery takes: 17.0625 DUSK for a transfer at
  the default prices, for 455,000 gas. Sepolia charges about 111,000 gas
  for each storage slot a delivery creates, so a delivery takes 218,195 to
  424,972 gas there ([costs.md](costs.md#pricing)). The ProtocolFee adds
  0.001 DUSK for its beneficiary.
- **Sepolia → Dusk.** Our IGP on Sepolia charges ETH for the Dusk gas a
  delivery takes: 1.32e11 wei for a transfer at the default prices, for
  2,200,000 gas, on top of Hyperlane's 1 wei protocol fee. It charges every
  route the same, so it is priced for the costliest delivery the deployment
  has, a token route's payout. Hyperlane's own Sepolia IGP has no
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
behaviour the script can't tell, and `deploy` stops. Sepolia's required
hook is Hyperlane's ProtocolFee, deployed before hooks reported
`hookType()`; `EVM_TYPELESS_HOOKS` lists it, so `deploy` takes it as a
ProtocolFee once it answers `protocolFee()`, and stops at any other hook
without a type.

`relayer.json` enforces payment per origin, against this deployment's IGPs
(`onChainFeeQuoting`). The operator collects what the IGPs take, and
`deploy/testnet.sh revenue` and `claim` show and claim it
([operations.md](operations.md#relaying-fees)).

## Before you start

- Checkouts laid out as in the [README](../README.md#building), with
  `rusk-private` at the commit in [`RUSK_VERSION`](../RUSK_VERSION). Then
  `make all dusk-tx`.
- Foundry (`forge`, `cast`), `jq`, `curl`, `python3`, `flock` (util-linux).
  `forge build` must work in `hyperlane-monorepo/solidity`.
- A **Dusk domain ID**. The default `1146442572` is free in the Hyperlane
  registry; `1146442571` ("DUSK") is kept for mainnet.
- **Funded keys:**

| Key | Network | Needs | Used for |
|---|---|---|---|
| Deployer | Dusk testnet | about 400 DUSK, and 95 more a token route | Contract deploys, about 220 DUSK (a local rehearsal spent 207.4 DUSK on 11 transactions; the native route has since grown by 48 KB, about 10 DUSK). A token route's two deploys took 47.5M gas in a local rehearsal, 95 DUSK: 28.1M for its token's 262 KB of Wasm, 19.5M for its route's 176 KB. Each deploy also needs its gas limit times the deploy gas price, 100 DUSK, in the balance when it is submitted. `preflight` checks the balance, for the token routes deploy has yet to deploy too. |
| Deployer | Sepolia | about 0.1 ETH | ISM, IGP, gas oracle, hook, token and enrollment transactions: 10.4M gas in 10 transactions against the local rehearsal's core, on Anvil, which deploys its ISM from source; less with Hyperlane's ISM factory. Sepolia charges far more for new code and storage than Anvil: the live DUSK token's implementation took 25.06M gas there. Each token route's token takes about 27M: its implementation, 25.3M, the proxy that initializes it, 1.3M, and the enrollment of its Dusk route, 0.37M (Sepolia's estimates; 3.9M in all on Anvil). |
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
  takes 3.27M. With token routes the script prices a delivery at 1.98M
  plus 0.22M per signature, which covers that payout at every threshold
  (`test_relayed_deliveries_cost_no_more_than_deploy_testnet_sh_prices`).
  `preflight` and `agent-configs` refuse a limit below 1.5 times the cost
  at `EVM_ORIGIN_THRESHOLD`. Once the contracts are
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
rusk-wallet. It takes the password from the first of these that is set:
`--password`, the file `DUSK_CONSENSUS_PASSWORD_FILE` names,
`DUSK_CONSENSUS_PASSWORD`, and `DUSK_CONSENSUS_KEYS_PASS`. With none of
them it tries the local demo's password, `password`.

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
- the recorded Dusk contracts read back as `deploy` left them, owned by
  `DUSK_KEYS`'s account
  ([Reading back the Dusk contracts](#reading-back-the-dusk-contracts));
- `TOKEN_ROUTES` is well formed, and a token route the record has keeps
  its line there; each recorded token route reads back on Dusk, and its
  Sepolia token as the DUSK token does, with the route's name, symbol and
  decimals and no scaling ([Tokens issued on Dusk](#tokens-issued-on-dusk));
- the ISM each recorded route uses on chain, its own or else the Mailbox's
  default, is a MessageIdMultisig ISM whose validators and threshold are
  `EVM_ORIGIN_VALIDATORS` and `EVM_ORIGIN_THRESHOLD`, each validator listed
  once. `deploy` checks it again when it finishes, and `agent-configs`
  before it writes anything.
  Any other module type stops them, a TestMock ISM (which accepts every
  message) included, and so does a query that fails. If the owner changes
  the validators or the threshold on chain, update both settings.

`deploy` records every step in `deploy/out/<DEPLOY_NAME>/deployment.json`
and skips steps already recorded there. Each write of the record is synced
to disk, with its directory, before the command goes on, and the first
command on a deployment syncs the directories it makes into their parents
before it sends anything. If it stops halfway, fix the cause and run it
again: a step whose transaction the stopped run sent but couldn't confirm
settles it before that step sends anything (below), and the run may stop
again until that transaction is in a block, or until the block of one that
took its nonce is final ([Known limits](#known-limits-of-resuming-a-deploy)
says what that rests on). Every
command but `fees` takes a lock on that directory (`flock` on its
`.lock`) and holds it until it exits, and reads the record
once: one command at a time runs on a deployment, and what a command
checked is what it records and what the configs it writes name. A command
started while another holds the lock stops at once; run it again when the
other has finished. Router enrollment is re-checked on chain each time: on
either side, `deploy` enrolls a router only where there is none, reading it
right before each write, and refuses another rather than replace it, since
that router's holders would be cut off from what backs their tokens. The
lock keeps out only this script's commands: a router the owner enrolls
another way between that last read and the enrollment is replaced by it,
since enrolling a router replaces whatever is there. Don't use the owner
keys elsewhere while `deploy` runs ([Before mainnet](#before-mainnet)). If
the recorded contracts aren't on chain, for example after a testnet reset,
or a recorded contract reports another `state_version` than this
checkout's (Mailbox 6, MerkleTreeHook 2, IGP 4, ValidatorAnnounce 3, native
route 5), `deploy` stops; move the directory aside and deploy again. Older
routes may pay sponsored escrow claims to forged keys
([security notes](security.md#issues-found-and-fixed), CRITICAL-1) and
register any key that asks (MEDIUM-10), older core contracts keep storage
anyone can fill cheaply ([storage.md](storage.md)), and an older IGP can't
be topped up (LOW-15), so `agent-configs` refuses them too. So is a
Sepolia IGP or token recorded by an older version of this script, which
initialized them in a second transaction that anyone could send first
([security notes](security.md#fourth-review), HIGH-8).

Every transaction `deploy` sends on Sepolia carries its nonce: the highest
of three reads of the deployer's pending nonce, as the nodes behind a public
endpoint can each be behind. Before it goes out, its step records it in the
record, on disk, under `.evm_sending`: its nonce, its recipient (none for a
creation) and its exact input, keyed by the step, a creation by where the
record keeps its address and a call by its function and recipient.

A node that answers one with "nonce too low" doesn't show it was refused: a
gateway that sent it on answers a replay of the request so too. So nothing
is sent again: the deployer's transaction with that nonce tells. A contract
creation is taken if it is that very creation, executed. If it is another
transaction, this one never executes once that transaction's block is
final: `deploy` waits for that, for up to `EVM_FINALITY_SECONDS` (1800),
checks that the endpoint still has that block at its height, and then stops
and forgets it, for the next run to send that step again. Until then
another block can replace that one, and `deploy` stops but keeps it. Any
other transaction answered so, or a send that times out, stops `deploy`
too, keeping it.

The step that sent a kept transaction settles it the next time it runs,
before it sends anything; other steps, and other commands such as `claim`,
leave it alone. Settling keeps to one deadline, `EVM_SETTLE_SECONDS` (120)
plus `EVM_FINALITY_SECONDS` from its start, whatever it finds, and every
read in it is given only what is left. It reads the blocks from where the
send was recorded for the deployer's transaction with that nonce, for up to
`EVM_SETTLE_SECONDS`. If that is the one, executed, a creation's step takes
it as its own, and a call's step reads the chain afresh, as of its block,
for what is left to do. If it isn't in the blocks read by then, the run
stops, sending nothing, and says which blocks it read; nothing records how
far it got, so the next run reads them again, and a larger
`EVM_SETTLE_SECONDS` reads further. If it failed, or another transaction
took the nonce, it is forgotten once that block is final, and the step
goes on afresh; a block that reads as replaced is settled again, within the
same deadline. A transaction that never went out, whose nonce the deployer
never reaches, keeps stopping its step: once you are sure, remove its entry
from `.evm_sending`. With `EVM_ISM_FACTORY` set, a creation of the ISM an
earlier run sent without it, still unsettled, stops `deploy` too: run
`deploy` with `EVM_ISM_FACTORY` empty to settle it, or remove its entry
once the deployer's transaction with its nonce is final and isn't that
creation.

Routes need no prepaid dispatch credit: each transfer's deposit pays its own
dispatch fee.

### Known limits of resuming a deploy

What a resume decides rests on two things it can't check for itself:

- **Sepolia's finality, as the endpoint answers it.** Settling a send reads
  the endpoint's final block number, then its block at the height that
  holds the nonce, in separate requests. Behind a load balancer, two nodes
  can answer them, one still on a fork the other has left, and a block can
  then read as final and the chain's when it isn't: a send can be forgotten
  that can still execute, or a failed outcome taken from the wrong fork.
  Deploy over a dedicated endpoint that answers from one node, or from nodes
  kept consistent, not a public load-balanced one.
- **Rusk publishing its tip promptly.** A Dusk node moves its queries to a
  block's state before GraphQL reports that block as its tip. The fence a
  resume reads a signer's nonce in reads the tip, the nonce, then the tip
  again a second later, and counts the nonce as that block's only if the
  tip stayed the same. A node slower than that second to publish the block
  the nonce came from, with that block then replaced, could have a logged
  token route deploy taken as never executing while it still can: the route
  would be deployed again at its IDs, and, if the logged one landed first,
  planned at other nonces. The fix needs Rusk to answer account queries at
  an identified, or a final, block.

### Reconciling `.evm_pending`

Only a version of this script that local rehearsals ran recorded an EVM
send as `.evm_pending`: one send, without the step that sent it. The live
record has none. Nothing can tell which step such a send was, so a record
that holds one stops `deploy`, `evm-igp --apply` and `claim` before they
send anything. For a rehearsal, the simplest is to move
`deploy/out/<DEPLOY_NAME>` aside and deploy afresh. To keep the deployment:

1. Read it: `jq .evm_pending deploy/out/<DEPLOY_NAME>/deployment.json`
   gives its `nonce`, its recipient `to` (empty for a creation), its exact
   `input` and its `label`.
2. Find the deployer's transaction with that nonce, on a block explorer.
   While the deployer's nonce (`cast nonce <deployer>`) isn't past it, no
   transaction with it is in a block: wait, or, if you are sure it never
   went out, go to step 5.
3. If it is that transaction, with that recipient and input, and it
   succeeded: a call needs nothing more. A creation made its contract at
   the address `cast compute-address <deployer> --nonce <nonce>` gives;
   record it where the step that made it records it. The label names the
   contract: StorageMessageIdMultisigIsm at `.evm.ism` (with
   `.dusk_origin_validators` and `.dusk_origin_threshold` as `deploy`
   records them), StorageGasOracle at `.evm.storage_gas_oracle`,
   StaticAggregationHookFactory at `.evm.aggregation_hook_factory`, an
   implementation at the IGP's or the token's `…_implementation`, and a
   proxy ("the … proxy") at the IGP's or the token's key, with
   `…_creation_tx` its transaction and `.evm.deployer` the deployer. The
   label doesn't name the route: the route whose token has no
   implementation recorded is the one.
4. If it failed, or another transaction has the nonce, wait until that
   block is final.
5. Remove it (`jq 'del(.evm_pending)'`, written back to the record) and
   run the command again.

### Reading back the Dusk contracts

Someone can deploy at our contract IDs first, or front-run a contract's
`init`, and choose its owner and bindings ([Before mainnet](#before-mainnet)).
So `dusk-tx deploy-hyperlane` reads every contract back once it has deployed
them all: the owner Rusk records for each, its owner, each binding its
`init` set and its `state_version`. When one isn't what it sent, or can't
be read, it fails and reports none of them. `deploy` then reads them again,
against the settings, before it records them:

- every owner is `DUSK_KEYS`'s account;
- the Mailbox is at `DUSK_DOMAIN`, its default ISM is the MessageIdMultisig
  ISM deployed with it, with `EVM_ORIGIN_VALIDATORS` and
  `EVM_ORIGIN_THRESHOLD`, its required hook is the aggregation hook and its
  default hook the IGP;
- the aggregation hook takes its calls from the Mailbox and calls the
  Merkle hook, then the ProtocolFee, which take theirs from it; the IGP,
  ValidatorAnnounce and the route take theirs from the Mailbox;
- the IGP and the ProtocolFee pay the deployer, the ProtocolFee charges
  0.001 DUSK and at most 0.1, and the IGP charges what the settings say;
- the route has no ISM or hook of its own and no router yet, and
  TestRecipient no ISM of its own;
- every contract is at this checkout's `state_version`.

When one isn't, `deploy` stops, names it, and records nothing. Don't use
those contracts: their IDs are taken, so move `deploy/out/<DEPLOY_NAME>/`
aside and deploy again with another `DUSK_KEYS`. If `deploy` stopped after
`deploy-hyperlane` reported its contracts, for example because a read
failed, running it again reads back the contracts in `dusk-deploy.json`
instead of deploying again.

`deploy` records the owner it read (`.dusk_owner`). Every command that uses
the recorded contracts reads them back first: `preflight` and `deploy`
against `DUSK_KEYS`'s account, the others against that owner. A deployment
recorded without one, by an older version of this script, is refused.
These later reads leave out what the owner may change on purpose: the
Mailbox's default ISM, the ISM's validators and threshold, the
beneficiaries, the ProtocolFee's fee, the IGP's pricing and the route's
ISM. The ISM the route uses and the IGP's pricing are held to the settings
instead, and the route's router for the EVM chain must be the recorded
token, or none yet. Routes have no view that lists their routers, so only
their routers for Dusk and the EVM chain are read.

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
# deposit is the amount plus the quoted dispatch fee, 17.0635 DUSK at the
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
fee and Hyperlane's 1 wei protocol fee, 132,000,000,001 wei at the default
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
transfer to the zero recipient is one example, a transfer of 0 from Sepolia
another (Dusk's routes refuse a transfer of nothing), and so is anything
that makes the destination's `handle` revert. The relayer simulates each
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
IGP=$(jq -r .evm.igp $S); GAS=2200000
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

## Tokens issued on Dusk

`TOKEN_ROUTES` lists tokens issued on Dusk that get a route to Sepolia,
one a line, `SYMBOL|NAME|DECIMALS|SUPPLY`. By default they are two test
stablecoins:

| Symbol | Name | Decimals | Initial supply |
|---|---|---:|---|
| EURd | Dusk Euro (testnet) | 6 | 1,000,000 EURd (1e12 units) |
| USDd | Dusk Dollar (testnet) | 6 | 1,000,000 USDd (1e12 units) |

For each, `deploy` deploys:

- on Dusk, the token: the pinned canonical DRC20 (`drc20-roles-pausable`
  from dusk-network/contracts, `make canonical-drc20-fixture`), with
  `DUSK_KEYS`'s account as its admin, holding the admin, minter and pauser
  roles and the whole initial supply, with no cap; and a WarpDrc20Collateral
  bound to the deployment's Mailbox that locks it. A transfer to Sepolia
  locks the tokens in the route, and one from Sepolia pays them out of it;
- on Sepolia, a synthetic `HypERC20` with the token's name, symbol and
  decimals, created as the DUSK token is, with the same hook and ISM.

Both sides move amounts in the token's smallest unit, as they are: the Dusk
route puts the amount in the message unchanged, and the Sepolia token's
scale is 1/1, so an amount of EURd is the same number of units, a millionth
of a euro each, on both chains, with nothing rounded. Every command that
reads back the deployment checks the Sepolia token's decimals and scale.

`deploy` plans each route's deploy nonces, from 10, two a route, and
records them before it sends anything; `dusk-tx deploy-token-route` then
deploys the token and the route at them. It writes each deploy transaction
to `deploy/out/<DEPLOY_NAME>/dusk-route-<SYMBOL>.txs` before it sends it,
and `deploy` records them. A run that stops resumes at the same contract
IDs, and takes a contract already at a planned ID only if one of those
transactions created it, in a final block. A logged transaction that is in
neither the ledger nor the mempool, a couple of blocks on, with its nonce
not reached, was never sent: it is marked void, and the ID deployed
afresh. One whose nonce the account has passed without it never executes
once the block that nonce was read at is final, and the ID is deployed
afresh then; if another block takes that block's place first, the nonce is
read again ([Known limits](#known-limits-of-resuming-a-deploy) says what
this fence rests on). Settling a logged deploy keeps to one deadline,
`--finality-timeout` (300 seconds), however often that happens. One still
in the mempool is waited for, and stops the run if it doesn't land. A contract there any other
way may come from a deploy or `init` someone front-ran, and reading it back
can't rule that out: the token doesn't list who holds its roles, so an
`init` that gave someone a role besides the deployer would read back the
same. So `deploy` leaves that pair of nonces, before anything else is sent,
and plans and deploys the next two. A plan also skips nonces whose IDs are
taken already.
Both contracts are read back, by `dusk-tx` and again by `deploy`, before
the route is recorded as deployed: the route's owner, Mailbox, token, hook,
ISM and routers, and the token's name, symbol, decimals, supply, cap and
pause, and that the deployer holds every role. Every later command reads
back the route's bindings and the token's name, symbol and decimals and
admin, but not its supply or its minter and pauser roles, which the admin
may change. Its routers are enrolled both ways as the DUSK route's are,
never replacing one that is there.

### Adding them to a deployment

A deployment made before `TOKEN_ROUTES` existed has the core and the DUSK
route, as the first testnet deployment does. `deploy` adds the token routes
to it and leaves the rest as it is: it deploys only their contracts and
enrolls only their routers. To add them:

1. Build the token's and the route's Wasms and dusk-tx:
   `make all canonical-drc20-fixture dusk-tx`.
2. Add `TOKEN_ROUTES` to `deploy/testnet.env`, as in
   `deploy/testnet.env.example`. A route the record has keeps its line,
   which can't change.
3. Raise `EVM_IGP_GAS_OVERHEAD` to 2,200,000. A token route's payout costs
   more Dusk gas than the native route's, and the Sepolia IGP prices every
   message to Dusk the same ([costs.md](costs.md#pricing)); `fees` and
   `preflight` refuse less once the deployment has a token route.
4. Price the Sepolia IGP for it, deploy, and write the agent configs again:
   the relayer's requirement for messages from Sepolia follows the
   overhead.

   ```bash
   bash deploy/testnet.sh evm-igp            # what changes; sends nothing
   bash deploy/testnet.sh evm-igp --apply
   bash deploy/testnet.sh preflight
   bash deploy/testnet.sh deploy
   bash deploy/testnet.sh agent-configs
   ```

The agents need nothing per route: the relayer relays any message between
the two chains once it is paid for, and the validators sign the Mailbox's
tree. Restart the relayer with the new `relayer.json`; until then it
requires the old overhead's gas, which the routes' transfers pay too. A
message from Sepolia that paid for the old overhead and isn't delivered by
then waits until someone pays for the 30,000 gas more
([operations.md](operations.md#topping-up-a-message)).

### Minting more

The deployer's account is each token's admin and minter:

```bash
S=deploy/out/<DEPLOY_NAME>/deployment.json
EURD=$(jq -r '.token_routes.EURd.dusk.token' $S)
target/release/dusk-tx drc20-mint --rues-url $DUSK --keys $DUSK_KEYS --token $EURD \
  --to <base58 address> --amount 250000000000 --expected-chain-id 2   # 250,000 EURd
target/release/dusk-tx drc20-info --rues-url $DUSK --token $EURD --account <base58 address>
```

`drc20-mint` checks that the signer holds the minter role, that the token
isn't paused, and that its cap (none) and a u64 allow the amount, before it
signs; `--to` defaults to the signer. A token minted on Dusk is backed by
nothing on Sepolia until it is bridged: a transfer locks it in the route and
mints as many there.

### Bridging EURd

Dusk → Sepolia: let the route take the tokens, then transfer them. The
deposit is only the dispatch fee, 17.0635 DUSK at the default prices;
`dusk-tx` adds it.

```bash
ROUTE=$(jq -r '.token_routes.EURd.dusk.route' $S); EURD_EVM=$(jq -r '.token_routes.EURd.evm.token' $S)
target/release/dusk-tx drc20-approve --rues-url $DUSK --keys my.keys --token $EURD --spender $ROUTE \
  --amount 25000000 --expected-chain-id 2                                # 25 EURd
target/release/dusk-tx transfer-remote --rues-url $DUSK --keys my.keys --warp-contract $ROUTE \
  --destination 11155111 --recipient <your sepolia address> --amount 25000000 --expected-chain-id 2
cast call $EURD_EVM "balanceOf(address)(uint256)" <your sepolia address> --rpc-url $EVM_RPC
```

Sepolia → Dusk: burn on Sepolia, paid out of the route on Dusk. As with
DUSK, the route holds the first transfer to an account in escrow until it
is claimed:

```bash
H=$(target/release/dusk-tx account --offline --keys my.keys | jq -r .account_h256)
FEE=$(cast call $EURD_EVM "quoteGasPayment(uint32)(uint256)" 1146442572 --rpc-url $EVM_RPC | awk '{print $1}')
cast send $EURD_EVM "transferRemote(uint32,bytes32,uint256)" 1146442572 0x$H 10000000 \
  --value $FEE --rpc-url $EVM_RPC --private-key <your key>                # 10 EURd
target/release/dusk-tx account --rues-url $DUSK --keys my.keys --warp-contract $ROUTE
target/release/dusk-tx claim-pending --rues-url $DUSK --keys my.keys --warp-contract $ROUTE --expected-chain-id 2
target/release/dusk-tx drc20-info --rues-url $DUSK --token $EURD --account <your base58 address>
```

USDd works the same, with `.token_routes.USDd`. A transfer of either pays
what one of DUSK does in each direction: the Sepolia tokens are all
HypERC20s, which cost the same to deliver to, and the Sepolia IGP prices
every message to Dusk for the costliest route.

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
   ValidatorAnnounce, IGP and WarpNative. A token route's token takes the
   deploy nonce `deployment.json` records for it,
   `.token_routes.<SYMBOL>.dusk.deploy_nonce`, from 10 on, and its route the
   next; the token's Wasm is `make canonical-drc20-fixture`'s
   `canonical_drc20_roles_pausable.wasm`.
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

The deploy takes about six minutes with the two token routes. Then run the
commands from
[Check it works](#check-it-works) with the local values: Dusk at
`http://localhost:8080/`, Anvil at `http://localhost:8545`, Anvil's domain
`31338`, the rehearsal's keys (`rusk-private/examples/consensus.keys`,
password `password`) and Anvil account #0 as the EVM user. Transfers arrive
in about 30 seconds.

The rehearsal deploys a MessageIdMultisig ISM, as on testnet, and the
token routes `deploy/testnet.env.example` lists, EURd and USDd, with the
canonical DRC20 `make canonical-drc20-fixture` builds. A route or
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
  The agents sign with Anvil's public development keys, so it also stops,
  before it exports a key or starts an agent, when `EVM_RPC`,
  `DUSK_RUES_URL` or any endpoint in the configs it makes from
  `agent-configs`' files isn't `http://localhost` or `http://127.0.0.1`.
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
  free. Until then, `deploy` reads every contract back and stops on a deploy
  or `init` someone else front-ran
  ([Reading back the Dusk contracts](#reading-back-the-dusk-contracts)), but
  their IDs are taken by then.
- **Router enrollment.** `deploy` reads a route's router right before it
  enrolls one, and enrolls only where there is none, but a route's
  `enroll_remote_router` replaces whatever router is there: a router the
  owner enrolls another way between the read and the enrollment is
  replaced, and its holders cut off from what backs their tokens. The Dusk
  routes should take the router they expect to replace, none for a first
  enrollment, and refuse any other.
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
