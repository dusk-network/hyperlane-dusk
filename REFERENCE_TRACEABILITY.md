# Reference Traceability

This note maps the Dusk reference sources from the revival goal to concrete
Dusk Hyperlane hardening decisions. It is not a substitute for reviewing those
repositories; it records how they shaped this branch.

## References Inspected

| Reference | Evidence inspected | Relevance |
|---|---|---|
| `dusk-network/contracts#24` | PR title/body and file list via `gh pr view 24 --repo dusk-network/contracts`; local checkout docs and standards primitives under `/home/hein_/projects/contracts` | Dusk-native OpenZeppelin-like standards: principals, ownership, roles, pausing, nonces, replay protection, signed authorizations, multisig/timelock/proxy patterns, DRC20/DRC721 events. |
| `dusk-network/duskevm-audit` | Local checkout `/home/hein_/projects/duskevm-audit`, especially `threat-model.md` and severity indexes | DuskEVM audit framing: bugs concentrate at compatibility seams between Ethereum assumptions and Dusk runtime/account/value/call primitives. |
| `dusk-network/duskevm-contracts` | Local checkout `/home/hein_/projects/duskevm-contracts`, README and contract layout | Dusk OP-contract port patterns, E2E/soak workflow shape, Dusk value custody and compatibility-harness practices. |
| `dusk-network/rusk-private` | Clean detached worktree `c0c64db4659500d077bb253ad13acba0e347d3fc` and local E2E runs | Dusk Rusk/RUES/GraphQL, transfer-contract, Moonlight transaction, and clean-chain behavior used by the Hyperlane E2E evidence. |

Reference checkout state verified on 2026-05-12:

- `dusk-network/contracts#24`: open PR, head
  `ccd00afee0150d3a10dc1d698aa75f3a63fbcd27`, 70-file PR surface. The local
  `/home/hein_/projects/contracts` checkout is dirty and was used only as
  reference material, not as release evidence.
- `/home/hein_/projects/duskevm-audit`: local commit `98e05ff`; dirty/untracked
  local files exist and were used only as reference material.
- `/home/hein_/projects/duskevm-contracts`: local commit `0e9cf3d`; dirty local
  files exist and were used only as reference material.
- `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db`: clean detached
  Rusk reference `c0c64db4659500d077bb253ad13acba0e347d3fc`; this is the Rusk
  checkout used for the recorded clean-Rusk repro/E2E/soak evidence.

## Standards Mapping

| Reference theme | Hyperlane application |
|---|---|
| Principal/caller separation | `SECURITY_REVIEW.md` documents Dusk Moonlight public senders, `ContractId` callers, transfer-contract mediated direct calls, and the Mailbox `resolve_sender` production decision. |
| Ownership/admin checks | Mailbox hook/ISM setters, warp route router/hook/ISM/admin paths, ProtocolFee, IGP, and MessageIdMultisigISM admin paths are covered by owner/admin tests and production review decisions. |
| Replay and domain separation | Mailbox delivered-message tracking, ValidatorAnnounce replay protection, MessageIdMultisig checkpoint/message-id verification, and test coverage for wrong domains/duplicate delivery are recorded in `SECURITY_REVIEW.md` and `TEST_REPORT.md`. |
| Pausing/admin recovery caution | Hyperlane v1 does not add broad emergency drains for pending escrow. `PRODUCTION_REVIEW_DECISIONS.md` makes the no-admin-drain decision explicit for Dusk review. |
| Typed event surface | Production contracts now use explicit event annotations for protocol and operational paths. Remaining `no_event` usage is limited to test-only contracts. |
| Multisig/governance caution | MessageIdMultisigISM rejects invalid thresholds, unsorted validator sets, insufficient signatures, corrupt signatures, and malformed metadata; production signer/governance custody remains a review gate. |
| Secret and artifact hygiene | `SECRET_HANDLING.md`, `PRODUCTION_SIGNER_POLICY.md`, `CI_REPRO_STRATEGY.md`, and `scripts/secret-hygiene-check.sh` turn signer/key handling into explicit local and production decisions. |

## DuskEVM Audit Pattern Mapping

| Audit pattern | Hyperlane mitigation or open decision |
|---|---|
| Auth bugs on public setters/admin paths | Added owner/admin negative tests and explicit review decisions for Mailbox, warp routes, ProtocolFee, IGP, and MessageIdMultisigISM admin behavior. |
| Compatibility seam bugs between Ethereum-style assumptions and Dusk primitives | `SECURITY_REVIEW.md` calls out account model, transfer-contract deposits, Dusk reverts, deterministic contract IDs, event/indexing, and address mapping deviations. |
| Value custody and bridge accounting failures | WarpNative deposit verification, WarpDrc20 checked arithmetic, WarpDrc20Collateral escrow, dirty redeploy refusal, low-signer-balance recovery, duplicate-delivery checks, and bridge accounting E2E are recorded in `TEST_REPORT.md`. |
| Fail-open remote reads or retry/finality ambiguity | E2E/fault-injection runs cover validator delay, corrupt metadata, origin/destination RPC failures, duplicate relayer attempts, and relayer restart/backlog behavior. |
| Pause/recovery design should be explicit, not accidental | `PRODUCTION_REVIEW_DECISIONS.md` separates contract-policy decisions from implementation evidence so Dusk can accept v1 behavior or request governed recovery paths. |
| Long-running local network proof matters | Clean-Rusk TestMock/MessageIdMultisig E2E, 50-transfer restart/backlog stress, the 3102-second high-volume soak, and the later 7282-second/280-transfer clean-Rusk soak mirror the DuskEVM emphasis on local stack lifecycle and soak evidence. |

## Rusk Wiring Mapping

| Rusk behavior | Hyperlane evidence |
|---|---|
| Moonlight transaction sender and transfer-contract deposit behavior | Mailbox sender resolution and WarpNative deposit verification are documented in `SECURITY_REVIEW.md`; live-Rusk E2E is the verification path for transitory deposit state. |
| RUES/GraphQL and account status behavior | `hyperlane-dusk` provider/indexers use RUES/GraphQL paths; monorepo `cargo check` and clean-Rusk E2E validate current integration shape. |
| Transaction submission and mempool/preverification behavior | Low signer balance, duplicate relayer, RPC failure, and restart/backlog tests document relayer behavior against Rusk preverification/mempool responses. |
| Deterministic contract IDs and dirty state | Dirty redeploy guard `1778522551` documents non-reset state refusal before deployments are treated as reproducible. |

## Remaining Reference-Driven Review Items

- Rusk maintainers should accept or change Mailbox transfer-contract sender
  resolution before production.
- Dusk product/security reviewers should accept or change immutable account
  registration and no-admin-drain pending escrow semantics.
- Dusk operations should accept or replace the signer custody and CI/repro
  strategies before production readiness is claimed.
- Upstream Hyperlane PR preparation should keep Dusk contract ports, E2E
  tooling, and Dusk-specific audit notes in this repository unless Hyperlane
  maintainers request another split.
