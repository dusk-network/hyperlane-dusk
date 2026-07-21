# Closure reassessment decisions — 2026-07-20

This record is the authority for the remediation reopened after the independent
cross-repository red-team and the first Controlecentrum deep review. Earlier
heads and successful logs remain regression history only. They are not release
evidence for the new candidate.

## Contract and ABI decisions

### Canonical DRC20 is the compatibility boundary

The collateral route targets the Dusk contract-standards ABI at
`dusk-network/contracts@bc1b00ee0af059975e158b7b580b4d0c0f1bdf9f`:

- principals are `Moonlight([u8; 193])`, `Phoenix([u8; 32])`, or
  `Contract(ContractId)`;
- call fields use the canonical `amount` names and archived layout;
- transfer and approval event topics are `drc20/transfer` and
  `drc20/approval`, with canonical principal payloads.

Compatibility is proved at two levels. The types tests compare rkyv bytes and
event topics directly with the exact pinned upstream crate. The VM suite builds
the pinned upstream `drc20-roles-pausable` WASM and uses that real contract as
the wrapped asset for approval, outbound custody, dispatch, inbound release,
and final balance assertions. A locally similar mock is not sufficient proof.

The collateral route accepts only an exact custody increase. If
`transfer_from` reports success but the route receives less or more than the
requested amount, the transaction reverts. Fee-on-transfer or otherwise
non-exact tokens therefore require a different, explicitly designed route.

### Escrow stays non-custodial

Unregistered inbound recipients retain the existing escrow model:

- native and collateral routes reserve live custody before recording a claim;
- the synthetic route reserves future `u64` supply before recording an
  unminted claim;
- Moonlight users and contract recipients authenticate through distinct claim
  paths;
- no owner or administrator may expire, redirect, or drain a user claim.

The accepted tradeoff is that a permanently invalid recipient can reserve
funds indefinitely. Recovery would need separate governance, delay, dispute,
and audit decisions.

### Dispatch credit and withdrawal are intentionally stacked

Mailbox dispatch credit is beneficiary-keyed prepaid native DUSK custody.
Funding is permissionless, but a sponsor gains no withdrawal, dispatch, or
reclaim authority. Dispatch consumes only the encoded sender's balance.

The base PR establishes custody and consumption. The stacked PR adds the only
exit: withdrawal authorized by the beneficiary identity, plus owner-authorized
route proxy methods for credit owned by each route contract. It does not add a
Mailbox-owner global drain or a funder reclaim right. The split keeps the exit
authorization and transfer callback boundary independently reviewable while
the combined E2E must validate both commits together.

### Hooks must have distinct identities

Mailbox rejects a default, selected, or updated hook that is the same contract
as the required hook. Otherwise one hook could be quoted and invoked twice
under two policy roles. The deployed topology remains:

- default hook: IGP;
- required hook: AggregationHook;
- aggregation children: MerkleTreeHook followed by ProtocolFee.

### Dispatch remains non-reentrant across hook callbacks

Pinned Piecrust supports same-contract recursion and reuses the active contract
instance. Rusk restricts selected nested stake mutations; it does not impose a
general contract reentrancy ban. Mailbox therefore holds a persisted dispatch
guard from before the first hook quote until every post-dispatch and payment
callback has completed. Nested dispatch is rejected while the outer dispatch
continues if the hostile hook handles that rejection. We deliberately keep the
existing quote, custody, event, and callback order rather than moving partial
dispatch effects ahead of fee validation.

### IGP configurations must be executable over their declared input domain

An explicit zero gas limit is invalid, and Dusk IGP quotes are capped at
1,000,000,000 destination gas. Configuration writes prove that every valid gas
limit from `1` through that declared maximum has nonzero pricing, cannot overflow
the contract's multiplication order, and produces a result representable as
`u64`. Unknown destinations, zero oracle inputs, rounding-to-zero, arithmetic
overflow, and an out-of-range final quote fail before dispatch.

### Validator announcements are bounded and isolated

A location is limited to 1,024 bytes, one validator retains at most 16
locations, and the registry retains at most 1,024 validators. The legacy batch
query accepts at most two validators. Agents must use the bounded
per-validator query so one malformed or unavailable validator does not poison
all other location discovery.

## Compatibility versions

The reopened base changes the canonical principal archive and stored synthetic
account keys. Its fresh-deployment matrix is:

| Contract | Base version | Reason |
| --- | ---: | --- |
| WarpDrc20 | 3 | canonical principal ABI and stored account-key layout |
| WarpDrc20Collateral | 2 | canonical wrapped-token call ABI |
| IGP | 2 | fail-closed pricing domain introduced earlier |
| Mailbox | 2 | dispatch reentrancy guard |
| Other deployed contracts | 1 | unchanged serialized layout |

The withdrawal stack is a distinct API/semantic deployment set and advances
the three route versions once more: WarpDrc20 `4`, WarpDrc20Collateral `3`, and
WarpNative `2`. Its Mailbox version advances from `2` to `3` because the
beneficiary-withdrawal layout is combined with the guard. Mixed base/stack
deployments are rejected. No in-place state migration is claimed; this
reassessment requires a fresh deterministic deployment.

## Transaction boundary

The current Rusk simulation endpoint accepts an ordinary signed transaction
whose bytes can be replayed. `dusk-tx call --simulate-only` therefore refuses
before reading signer material, and the reusable RUES simulation sender was
removed. Agent preparation must use a conservative local gas limit rather than
send a replayable signed payload to a remote simulation endpoint.

After propagation begins, every non-success response—including every 4xx—is
an unknown outcome. The exact locally computed hash is retained and reconciled
against authoritative ledger state before retry or success. Only a persisted
result for that exact hash with an explicit successful execution result is
definitive.

Every agent submission supplies its configured native Dusk `chainId` to
`dusk-tx`. The helper queries the endpoint chain ID and rejects a mismatch
before reading signer material, querying the signer account, or constructing a
transaction. The observed endpoint identity is therefore part of the signing
boundary rather than an informational startup check.

## Deployment and harness decisions

Warm reuse validates a complete live topology, not just bytecode existence:

- exact EVM and Dusk chain/domain identities;
- Mailbox owners, default ISMs, default hooks, and required hooks;
- route owners, Mailboxes, hook/ISM overrides, and both directions of router
  enrollment;
- ValidatorAnnounce, MerkleTreeHook, IGP, ProtocolFee, and AggregationHook
  dependencies;
- fee-hook owners/beneficiaries, protocol-fee parameters, IGP pricing, and the
  ordered aggregation child list;
- exact contract version matrix and the collateral wrapped-token identity.

Dispatch funding is idempotent: deployment and demo entry points query the
current credit and fund only the deficit to the configured target.

Each agent-config invocation exclusively owns one run directory containing a
stable deployment snapshot, signer file, configurations, databases, and
checkpoints. A caller-selected collision fails. The source manifest is hashed
before and after copying and compared with the pending snapshot before atomic
rename. Failure removes only the uncommitted directory owned by that
invocation.

An occupied Rusk port is accepted only after the configured RUES endpoint
passes its health probe. Cold-start readiness timeout is fatal. Shutdown never
kills a Rusk service recorded as external.

The live Anvil harness assigns separate pre-funded development accounts to the
operator, relayer, and validator. The same validator address and threshold are
passed to both the EVM and Dusk MessageIdMultisigISM deployments. This prevents
concurrent processes from sharing one EVM nonce stream and prevents a test from
accidentally proving only one checkpoint direction. The operator and Dusk
validator setup calls finish before either validator starts, for the analogous
Dusk signer boundary.

## Review-policy decision

The default-branch `pull_request_target` workflow is the trusted policy
harness. It checks out only the trusted base, fetches the exact proposed commit
into Git's object database, and treats every proposed path as data: it performs
`git diff --check`, verifies that required guard paths are regular blobs, and
waits for the exact unprivileged `Dusk proposal validation` check on that head.
It never checks out or executes a proposed-tree script. Candidate contract and
script execution remains confined to the read-only-token `pull_request`
workflow, so adding a future read token to a trusted workflow cannot expose it
to head-controlled shell code. Required check evaluation paginates all check
runs, matches exact configured names, and treats a lookalike name as missing.

That trusted workflow cannot execute for the implementation PR until dispatcher
PR #3 installs it on the default branch. During this explicit bootstrap,
`Dusk proposal validation` runs the proposed workflow lint, diff checks,
fail-closed self-tests, report hygiene, and secret hygiene on PRs targeting
`main` or the stacked base branch. Pre-merge readiness requires that proposal
check and the readiness guard. Proposal validation is machine evidence, not a
trusted-policy or production substitute: production mode still requires the
default-branch `Dusk review policy gate`, and branch protection must switch to
that trusted context once PR #3 lands. Human review remains authoritative for
changes to the proposal tests themselves; the target-context gate guarantees
the execution boundary, not that arbitrary proposed policy text is correct.

Hosted report hygiene scans tracked report content but does not claim to verify
the existence of the developer-machine repro archive path. The local gate keeps
that physical archive and hash check enabled by default, and its fail-closed
self-tests continue to cover missing files, missing references, and hash
mismatches.

The compile/runtime clean-layout anchor remains monorepo `b4c46ce9`; the later
policy-only anchor `dad14dbbea4bbd59f6c6697f89cc245d5c1cf2a0` adds the two
validator fail-stop files to the fork-boundary allowlist. That policy delta was
validated by the exact boundary reproduction and a successful hosted
`Dusk review policy gate`; it is tracked separately so it is not mislabeled as
a rerun of the Rust/contract gate. Cross-repository readiness may wait up to
five minutes for the agent contexts rather than converting a still-running
hosted build into a failure after sixty seconds.

After that evidence run, upstream advanced by one SVM/TypeScript-only commit.
The monorepo PR was rebased onto `67933966ed9c6f9e3d5ec095372e11414c82e4e7`;
all 79 Dusk commits are patch-equivalent under `git range-diff`. Rebased
equivalents are runtime `e95d3ea282a55ead114471ffb1dece77706ffc81`, static
checkout `833b77b4436e146a4776a3b35db68525014b3adb`, and policy
`c35f86405cf8cd83927860aca8b5c38b042ee198`. The complete affected Rust cargo
boundary passed again on the rebased PR head. This mapping does not relabel the
older exact E2E logs as runs against the rebased commit.

`READINESS_MODE=premerge` validates the exact required checks on the current
Dusk PR, the monorepo PR, and the workflow-dispatcher PR, but does not require
any of them to be already approved or merged. Those requirements would create
a circular cross-repository status check and duplicate branch protection.
`READINESS_MODE=production` retains merged-and-approved requirements for all
three PRs and all production-only sign-off, protection, dependency, runner, and
secret gates. A green pre-merge check therefore means machine validation is
green, not that production is authorized.

Reviewer-facing hygiene distinguishes moving review heads from immutable test
anchors. Any text that claims a current PR head is checked against GitHub at
runtime. Contract, agent, Rusk, repro-log, and E2E evidence is instead pinned to
the exact code and content hashes that were tested; a later documentation or
policy commit does not relabel unrerun evidence as head-tested. The active base,
dispatcher, withdrawal, and monorepo PR bodies must expose the evidence relevant
to their own scope, and the consolidated issue #2 handoff must expose the full
cross-repository set. Historical decision issues are scanned for stale or secret
material but are not forced to duplicate a moving handoff on every commit.

The review gate therefore rejects stale head claims, known superseded evidence,
missing immutable anchors, missing decision/compatibility links, and any review
surface that omits the pre-merge-versus-production disclaimer. It deliberately
does not require a machine-local `/tmp` archive path, one historical Actions run
URL, or copied boilerplate in every decision issue: those were properties of an
old run, not protocol safety invariants.

## Companion agent decisions

The monorepo PR must follow these boundaries before new evidence is accepted:

- Mailbox and Merkle hook identities remain separate in event provenance and
  checkpoint state;
- event/header provenance is joined and indexed only after finality;
- contract-scoped `finalizedEvents(contractId, limit, cursor)` pages are capped
  at 16 rows and use each row's own event ID, height, block hash, origin,
  source, topic, data, and reverted flag; the cursor and sequence-to-provenance
  mapping are persisted atomically under an agent-exclusive `eventCursorDir`
  so restart does not fall back to whole-block archive retrieval;
- configured Dusk domain, Mailbox domain, ValidatorAnnounce domain, endpoint
  chain ID, and signing chain ID must agree at startup;
- Dusk signing keys must be canonical, exactly 32 bytes, valid nonzero scalars;
- validator locations are queried independently with bounded responses and
  per-validator error isolation;
- requested and observed heights are distinct, and reorg state is recorded
  before diagnostics can fail;
- archive retrieval cannot require buffering an entire block below the helper
  transport limit;
- Dusk-origin validator E2E is required, not only EVM-origin validation.

The `messageIdMultisig` demo deploys a real storage-backed MessageIdMultisig
ISM on EVM as well as on Dusk. It runs one validator for each origin. Thus the
Dusk-to-EVM leg must consume a Dusk-origin checkpoint and exercise the Dusk
ValidatorAnnounce, finalized-event indexer, Merkle hook checkpoint, checkpoint
syncer, relayer metadata, and EVM ISM verification path end to end.

## Replacement evidence rule

No earlier head or log is called “final.” Replacement proof must be generated
from clean, frozen heads after the base, withdrawal stack, and monorepo changes
are committed. It must include the pinned canonical DRC20 VM round trip, the
full version matrix, both E2E security modes, the Dusk-origin validator path,
fault branches, and secret/archive hygiene. Only then are fresh independent
GPT-5.6 xhigh and Controlecentrum deep/xhigh reviews run against those exact
heads.

### Replacement evidence status — 2026-07-21

The implementation evidence requirement is satisfied for base runtime
`9058755927473239d59ce702a8074acbae0e0a24`, withdrawal-stack runtime
`dc8aba07773993878edd81735d59e66beddd66a3`, monorepo
`6ef326b8a926d262714afd315960b26e441c7b40`, and Rusk
`5c6a0bab11c61fb4c81275afdeceb97fb942d85e`. The exact aggregate reproduction,
TestMock, MessageIdMultisig, and eight fail-closed live-run logs and hashes are
recorded in the newest `TEST_REPORT.md` section. The later monorepo rebase adds
only upstream SVM/TypeScript state and patch-equivalent Dusk commits; later
workflow/documentation commits do not change the validated Dusk runtime tree.

This evidence closes the machine-validation prerequisite; it does not approve
production or resolve the unchecked human decisions in
`PRODUCTION_REVIEW_DECISIONS.md`. Fresh independent GPT-5.6 xhigh and
Controlecentrum deep/xhigh reviews remain the next gate and must use the final
published PR heads.
