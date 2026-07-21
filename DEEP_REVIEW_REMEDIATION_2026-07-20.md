# Deep review remediation decisions — 2026-07-20

This record documents the decisions made after the deep reviews of Dusk PR #1,
beginning at `193811ee6ae5b62cdd4a29890357e00e3e911a8d` and refreshed after the
2026-07-20 upstream/Rusk reassessment. It records intended behavior, not just
the mechanics of the patch, so future upstream and Rusk syncs can preserve the
security boundaries deliberately.

## Decisions

| Review item | Decision |
| --- | --- |
| C01: implicit `TestMock` default ISM | Deployment now requires an explicit `--default-ism`. `testMock` remains available only as an explicit local/test choice; production callers should select `messageIdMultisig` and supply validators and a threshold. |
| C02: collateral escrow for contract recipients | Keep escrow, and add `claim_pending_contract`. The immediate nested contract caller authenticates the recipient and receives DRC20 tokens as `Account::Contract`. Root Moonlight calls cannot use this path. |
| C03: data-driver ABI drift | Decode all current query shapes, optional owner state, and all operational events emitted by the contracts. DRC20 query types gain serde support behind the existing `serde` feature. |
| C04: removable required review guard | The workflow resolves the trusted base branch and fails if a proposed change deletes a guard that exists on the base. Absence is skippable only for a genuinely pre-guard base. |
| C05: registration failures reported as success | Demo registration is fail-closed for every deployed route. Success is printed only after the CLI confirms the exact transaction result. |
| C06: caller-owned export deletion | A caller-supplied `--export-dir` must not already exist. Cleanup may recursively remove only a fresh directory created by the current invocation. |
| C07: zero DRC20 operations create entries | Preserve sparse state: zero approvals remove allowances, zero transfers do not create balances, and zero balances/allowances are removed. |
| C08/C13: zero inbound/outbound warp transfers | Reject zero amounts on every synthetic, native, and collateral inbound and outbound path before accounting, custody, mint, burn, or dispatch. |
| C09: readiness count mismatch | Status totals, pending counts, and failed counts use the same domain and consistently exclude the currently executing readiness run. |
| C10: stale `--skip-deploy` state | Reuse requires matching stored EVM and Dusk chain IDs, live EVM bytecode at the saved addresses, and successful live Dusk queries against the saved mailbox and warp route. Otherwise redeployment is required. |
| C11: `rg` errors interpreted as no matches | Negative scans distinguish `rg` status 1 (no match) from status 2+ (scan failure) and fail closed on the latter. |
| C12: fixed self-test probe path | The test uses a unique `mktemp` path inside the repository and removes only the path it created. |
| C14: zero owner identities | Every owned contract rejects zero at initialization and ownership transfer. `renounce_ownership` remains the sole explicit transition to `None`. |
| C15: nonce-only transaction success | Every mutating CLI command and deployment waits for the persisted result of its exact transaction hash. Propagation, nonce advancement, missing archive data, malformed GraphQL data, and execution failure are not reported as success. Polling is bounded by an absolute deadline and response bodies are capped at 256 KiB. |
| C16: unbounded password file | Consensus password files are capped at 4 KiB before trimming and use. |
| C17: collateral claims were accounting-only | Track aggregate pending DRC20 liability and query live route custody before accepting either a new pending claim or an immediate registered-recipient delivery. Existing pending claims are reserved first, so direct delivery cannot consume their backing. |
| C18: zero-value dispatch-credit entries | Remove a sender's credit entry when a dispatch spends the exact remaining balance. Permissionless third-party funding remains intentional: credit is keyed to the beneficiary and cannot be redirected by the funder. |
| C19: zero mailbox route configuration | Reject the zero contract ID when initializing every warp route. A route cannot be deployed into an immutable, unusable mailbox configuration. |
| C20: multisig validator cardinality | Cap validator lists at 255 in the contract and deployment CLI because the threshold ABI is `u8`. Threshold validation still requires `1 <= threshold <= validators.len()`. |
| C21: unbounded RUES and key input | Bound successful contract-query bodies at 4 MiB, error bodies at 64 KiB, transaction-status bodies at 256 KiB, and secret-key stdin at 128 bytes. Declared and streamed oversize responses fail before deserialization. |
| C22: explicit collateral token trust | Before deploying a route around a caller-supplied token ID, require the contract to exist and successfully answer the typed `balance_of` query used by the route. This proves presence and the required query ABI, not full semantic conformance; production token selection remains a deployment-governance decision. |
| C23: data-driver same-name ABI | `quote_dispatch` has a five-field Mailbox ABI and a two-field hook ABI. The explorer driver now accepts and decodes both shapes, rather than silently treating Mailbox queries as hook queries. Current accounting and block-provenance queries are covered as well. |
| C24: stale and ambient build artifacts | Demo entry points let Cargo refresh the CLI, all contract WASMs, and the explorer driver. `make data-driver` is build-only and no longer mutates a separate explorer checkout; `start-env.sh` performs the one explicit copy when explorer integration is enabled. |
| C25: implicit demo security policy | `deploy.sh` requires an explicit Dusk ISM on new deployments. `testMock` remains an explicit local-demo selection, while the one-command demo passes that choice visibly. |
| C26: recipient ISM fallback | Preserve Hyperlane recipient semantics: if a recipient ISM query is unavailable or returns zero, Mailbox falls back to its default ISM. The fallback is interoperability behavior, not acceptance of a failed nonzero ISM verification. |
| C27: escrow recovery and expiry | Keep pending recipient claims without an admin drain, expiry, or reassignment path. This avoids privileged seizure, but permanently malformed or lost recipient identities can strand their reserved funds; changing that requires a separately reviewed governance design. |
| C28: native escrow was only bookkeeping | Treat native pending balances as liabilities against live route custody. `pending_total` is reserved before either a new escrow entry or a direct registered-recipient payment, so an inbound message cannot create an unbacked claim or spend DUSK promised to an earlier claimant. |
| C29: unregistered synthetic recipient ambiguity | Do not cast an arbitrary unregistered H256 to `Account::Contract`. Keep the synthetic amount unminted until either the matching Moonlight public key or the immediate contract caller authenticates that recipient and claims it. Total supply increases only at the authenticated claim boundary. |
| C30: fabricated/unfinalized Merkle provenance | Persist hook-owned message IDs, insertion heights, and post-insertion roots. Agents index the hook's exact archive event and expose only consensus-finalized insertions/checkpoints; Mailbox dispatches are not treated as proof that a configured Merkle hook ran. |
| C31: ambiguous agent success and identifiers | Require explicit `err: null` for transaction success, require a height query to return the requested height, reject non-canonical H512 padding for Dusk transaction IDs, and bound/open signer key files once before reading. Malformed observations fail closed. |
| C32: cross-repository ABI drift | Pin the agent gate to an exact companion Dusk commit and record the compatible contract/agent heads. Branch-name checkouts remain available only as an explicit manual override. |
| C33: changed contract storage layout | Require a fresh, mutually compatible deployment of the complete Dusk topology. Every deployed contract exposes an explicit state version: WarpDrc20 is version 2 after adding its aggregate pending-supply reserve; all other current contracts are version 1. Both `--skip-deploy` reuse boundaries require the complete version matrix, so pre-version, pre-reserve, or pre-atomic-policy instances are rejected rather than accepted through an older liveness ABI. No in-place migration is claimed. |
| C34: aggregate synthetic claim capacity | Treat every accepted unminted synthetic transfer as a liability against the `u64` supply domain. `pending_total` reserves that capacity before either a direct mint or another pending entry; claiming releases the reserve in the same transaction that mints. |
| C35: validation evidence must preserve its decision boundary | The primary repro bundle compiles/tests every changed host crate, rejects dirty custom-layout sources instead of silently testing `HEAD`, and requires the exact stale-review diagnostic. Branch-policy checks retain and enforce `strict == true`. |
| C36: restored topology and agent policy binding | Saved-state reuse validates every persisted EVM contract and the exact state version of every persisted Dusk contract, then binds generated agent mode and Moonlight chain ID to the validated bridge-state manifest and live Mailbox default ISM. The post-parse reuse boundary repeats the complete Dusk version matrix before configuration is generated. |
| C37: split and serial agent reads | Add atomic `validators_and_threshold` and bounded `message_ids`/`gas_payments` query ABIs. The agent consumes coherent validator policy and pages lifetime history in 256-record requests instead of issuing one RPC per record. |
| C38: static Dusk dry run | Use Rusk's `/on/transactions/simulate` endpoint through `dusk-tx call --simulate-only`. Relayer preparation now executes the exact signed Mailbox payload in an ephemeral session, requires both simulation response fields, and rejects deterministic contract failures before propagation. |
| C39: helper transport and ambiguous submission | Cap serialized helper arguments below the per-argument operating-system boundary, reject malformed public arguments before signer access, and preserve the exact hash across outcome-unknown propagation and confirmation timeout. Every mutating helper path uses the same submit/reconcile boundary; the agent reconciles that hash before reporting a transaction outcome. |
| C40: transaction provenance and confirmation schema | Read Moonlight sender and nonce from the ledger transaction JSON instead of publishing zero sentinels. Treat a malformed non-null transaction record as schema corruption; retry only observation failures and explicit not-yet-included state. |

## Escrow scope

Collateral DRC20 routes can safely support contract recipients because the token
ABI has a first-class `Account::Contract` destination. The native route does not
claim parity here: transferring native DUSK to an arbitrary contract requires a
recipient callback/deposit ABI and cannot be made safe by treating a 32-byte
recipient as an account key. Native contract-recipient support therefore remains
unsupported until that ABI is designed and tested explicitly.

For DRC20 collateral, pending balances are liabilities against the route's live
token custody. `pending_total` is the aggregate reserve. An inbound delivery to
a registered recipient may spend only `balance_of(route) - pending_total`; an
inbound delivery to an unregistered recipient may add a claim only within the
same unreserved amount. Claiming removes the recipient balance and aggregate
liability in the same contract transaction that transfers tokens. This makes
escrow a custody invariant rather than only a bookkeeping promise.

No administrator can expire, reassign, or drain pending native or DRC20 claims.
That is an explicit non-custodial choice for this version, with the accepted
tradeoff that an invalid recipient hash can reserve funds indefinitely.

Native DUSK uses the same live-custody invariant: `pending_total` is subtracted
from the route's transfer-contract balance before accepting a new delivery.
Claims decrement the reserve in the same transaction that pays the recipient.

Synthetic DRC20 has no backing asset to reserve, but it does have a finite
`u64` supply domain. Its pending entries represent authorization-delayed
minting, and `pending_total` reserves aggregate future mint capacity. Direct
mints and new pending entries are accepted only while
`supply + pending_total + amount` is representable. An unregistered 32-byte
recipient is deliberately left untyped; claiming authenticates its account,
decrements the aggregate reserve, and mints in one transaction.

## Dispatch-credit scope

Dispatch credit is pre-funded native DUSK custody held by Mailbox for a payer
identity. Funding is permissionless so a service or sponsor can pay for another
sender, but only that sender's dispatch can consume the balance. Credit is not
an allowance to the funder, cannot be redirected, and exact consumption removes
the storage entry. Withdrawal belongs in the stacked dispatch-credit PR so its
authorization and receipt semantics can be reviewed without expanding this
base patch.

That split is intentional rather than deferred functionality: PR #1 establishes
the custody and consumption invariant; the stacked PR adds the only permitted
exit, authorized by the beneficiary whose identity owns the credit. Keeping the
withdrawal delta separate makes its authorization and callback boundary visible
while E2E testing still validates the combined stack.

Configured hooks with an empty price still run. The small call overhead is
accepted because it keeps hooks configuration-ready and avoids a second set of
dispatch semantics that depends on the current quote.

## Deployment compatibility

The pending-liability, Merkle-history, fee-accounting, ownership, and atomic
validator-policy changes form one serialized and semantic compatibility set. An
existing deployment cannot be upgraded in place by swapping WASM. WarpDrc20 is
state version 2 because its aggregate synthetic reserve follows an earlier
versioned layout; every other deployed contract in the current topology is
state version 1. These are operational compatibility probes, not migration
mechanisms. Both demo reuse boundaries validate the complete matrix and fail
closed when any exact expected version is absent. Live semantic checks such as
Mailbox default-ISM binding are retained in addition to the version checks.

## Transaction boundary

The CLI's success boundary is the exact transaction hash returned by the signed
transaction. A persisted `tx(hash) { err }` result with `err: null` is success; a
non-null error is failure; a missing transaction remains pending. Transport and
GraphQL observation errors are retried within the deadline and retained in the
final diagnostic. A non-null ledger transaction with malformed execution fields
fails immediately because it is authoritative schema data, not a pending
observation. Outcome-unknown propagation retains the exact hash for
reconciliation before any retry. Confirmation timeout has the same unknown-
outcome classification and retains the same hash; a definitive on-chain
execution error remains a failure rather than an ambiguous outcome.
This rule applies uniformly to generic calls, dispatch, process, enrollment,
registration, approval, warp transfers, dispatch-credit funding, and deployment.

## Validation expectations

The remediation is complete only when shell syntax and fail-closed self-tests,
the Dusk CLI/data-driver tests, every contract WASM build, and the VM integration
suite pass in a clean layout against the current pinned Rusk source. A later
review or sync should treat a stale successful run as evidence to refresh, not as
permission to bypass these boundaries.
