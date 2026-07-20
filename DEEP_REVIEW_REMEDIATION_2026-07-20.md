# Deep review remediation decisions — 2026-07-20

This record documents the decisions made after the deep review of Dusk PR #1 at
`193811ee6ae5b62cdd4a29890357e00e3e911a8d`. It records intended behavior, not
just the mechanics of the patch, so future upstream and Rusk syncs can preserve
the security boundaries deliberately.

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

## Dispatch-credit scope

Dispatch credit is pre-funded native DUSK custody held by Mailbox for a payer
identity. Funding is permissionless so a service or sponsor can pay for another
sender, but only that sender's dispatch can consume the balance. Credit is not
an allowance to the funder, cannot be redirected, and exact consumption removes
the storage entry. Withdrawal belongs in the stacked dispatch-credit PR so its
authorization and receipt semantics can be reviewed without expanding this
base patch.

Configured hooks with an empty price still run. The small call overhead is
accepted because it keeps hooks configuration-ready and avoids a second set of
dispatch semantics that depends on the current quote.

## Transaction boundary

The CLI's success boundary is the exact transaction hash returned by the signed
transaction. A persisted `tx(hash) { err }` result with `err: null` is success; a
non-null error is failure; a missing transaction remains pending. Observation
errors are retried within the deadline and retained in the final diagnostic.
This rule applies uniformly to generic calls, dispatch, process, enrollment,
registration, approval, warp transfers, dispatch-credit funding, and deployment.

## Validation expectations

The remediation is complete only when shell syntax and fail-closed self-tests,
the Dusk CLI/data-driver tests, every contract WASM build, and the VM integration
suite pass in a clean layout against the current pinned Rusk source. A later
review or sync should treat a stale successful run as evidence to refresh, not as
permission to bypass these boundaries.
