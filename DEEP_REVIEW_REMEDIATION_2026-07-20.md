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

## Escrow scope

Collateral DRC20 routes can safely support contract recipients because the token
ABI has a first-class `Account::Contract` destination. The native route does not
claim parity here: transferring native DUSK to an arbitrary contract requires a
recipient callback/deposit ABI and cannot be made safe by treating a 32-byte
recipient as an account key. Native contract-recipient support therefore remains
unsupported until that ABI is designed and tested explicitly.

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
