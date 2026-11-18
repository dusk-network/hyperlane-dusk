//! dusk-tx — CLI tool for Hyperlane contract deployment and interaction on Dusk.
//!
//! Subcommands:
//!   call               Call a contract method via Moonlight TX
//!   deploy-hyperlane   Deploy the full Hyperlane contract stack
//!   deploy-wasm        Deploy one contract, at the ID its Wasm, nonce and owner give
//!   deploy-token-route Deploy a token issued on Dusk and a collateral route for it
//!   query              Query contract state via RUES
//!   dispatch           Dispatch a message via TestRecipient proxy
//!   process            Process an inbound message on the Mailbox
//!   encode-message     Encode a Hyperlane message (no TX, pure encoding)
//!   enroll-router      Enroll a remote router on a warp route
//!   transfer-remote    Send tokens or native DUSK through a warp route
//!   drc20-mint         Mint a canonical DRC20 (its minter)
//!   drc20-info         Show a canonical DRC20, and an account's balance and roles
//!   set-igp-config     Set an IGP's pricing for a destination (owner)
//!   set-beneficiary    Set who can claim a fee contract's fees (owner)
//!   claim-fees         Claim a ProtocolFee's or IGP's fees (beneficiary)
//!   pay-for-gas        Pay an IGP for more gas for a message (anyone)
//!   account            Show an account's address, recipient hash and balance
//!   export-signer-key  Write an agent signer key file from consensus.keys
//!
//! Every command that sends a transaction takes the chain it signs for
//! (`--expected-chain-id`), and stops before it queries a contract or reads
//! the key when the node reports another one. It waits until the
//! transaction is included in a block, then says whether that block is final
//! (`finalized`). With `--wait-finality` it waits until it is.

use std::future::Future;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use std::{
    env, fs,
    io::{BufRead, Read},
};

use clap::{Parser, Subcommand};
use dusk_bytes::Serializable;
use dusk_core::abi::ContractId;
use dusk_core::signatures::bls::{PublicKey as BlsPublicKey, SecretKey as BlsSecretKey};
use dusk_core::transfer::data::{ContractBytecode, ContractCall, ContractDeploy};
use dusk_core::transfer::moonlight::Transaction as MoonlightTransaction;
use dusk_core::transfer::Transaction;
use hyperlane_dusk_types::drc20::{
    Account as Drc20Account, Allowance as Drc20Allowance, ApproveCall as Drc20ApproveCall,
    BalanceOf as Drc20BalanceOf,
};
use hyperlane_dusk_types::{DomainGasConfig, EthAddress, GasPaymentRecord};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::ser::Serializer;
use rkyv::Serialize;
use serde_json::json;

mod canonical_drc20;
mod deployment;
mod keys;
mod multicall;
mod rues;

use rues::{BlockStatus, Inclusion, RuesClient, TransactionStatus, TransactionStatusQueryError};
const MAX_PASSWORD_FILE_BYTES: usize = 4 * 1024;
const MAX_SECRET_KEY_STDIN_BYTES: usize = 128;
const MAX_RUES_URL_STDIN_BYTES: usize = 8 * 1024;
const MAX_MULTISIG_VALIDATORS: usize = u8::MAX as usize;
const MAX_CALL_ARGS_BYTES: usize = 60 * 1024;
// Keep this synchronized with validator-announce's MAX_VALIDATOR_PAGE.
const MAX_VALIDATOR_ANNOUNCE_PAGE: u32 = 256;
// Keep this synchronized with the IGP's MAX_PAGE_SIZE.
const MAX_IGP_PAGE: u32 = 256;
const IGP_TOKEN_EXCHANGE_RATE_SCALE: u128 = 10_000_000_000;
const IGP_MAX_GAS_LIMIT: u64 = 1_000_000_000;
/// Default gas limit for claiming the signer's own escrow (`claim_pending`).
/// The costliest route's claim measures 1.91M gas in the VM tests, which
/// check this limit has 1.5 times that.
const CLAIM_PENDING_GAS_LIMIT: u64 = 3_000_000;
/// Default gas limit for claiming another account's escrow
/// (`claim_pending_for`): decoding the account's key costs about 57M gas, so
/// a claim takes about 59M.
const CLAIM_PENDING_FOR_GAS_LIMIT: u64 = 80_000_000;
/// Default gas limit for the owner and beneficiary calls on the fee
/// contracts (`set-igp-config`, `set-beneficiary`, `claim-fees`). They take
/// 1.86M to 1.87M gas (docs/costs.md, Owners); a failed call is charged its
/// whole limit, so it stays low.
const OWNER_CALL_GAS_LIMIT: u64 = 3_000_000;
/// Default gas limit for `pay-for-gas`. A top-up takes about 1.83M gas, or
/// 1.85M when the IGP sends part of the deposit back (docs/costs.md); a
/// failed one pays its whole limit, so it stays low. The VM tests check it
/// has 1.5 times a top-up's gas.
const PAY_FOR_GAS_GAS_LIMIT: u64 = 3_000_000;
/// The first IGP `state_version` with `pay_for_gas` and `quote_pay_for_gas`.
const IGP_TOP_UP_STATE_VERSION: u32 = 4;
/// WarpNative's `state_version` in this checkout. Its DRC20 routes report
/// others (WarpDrc20 8, WarpDrc20Collateral 7), so `transfer-remote
/// --native` tells a native route by it.
const WARP_NATIVE_STATE_VERSION: u32 = 5;
/// The least dispatch credit the Mailbox takes for a payer with none, 0.01
/// DUSK (the Mailbox's `MIN_NEW_PAYER_CREDIT`).
const MIN_NEW_PAYER_CREDIT: u64 = 10_000_000;
/// The memory the Mailbox takes no new payer past, and a WarpDrc20 no new
/// holder or spender: 3 GiB. Each refuses a new entry unless the most it can
/// allocate still fits under it.
const NEW_ENTRY_MEMORY_LIMIT: u64 = 3 * 1024 * 1024 * 1024;
/// A wasm32 memory page.
const WASM_PAGE_BYTES: u64 = 64 * 1024;
/// The most a new Mailbox credit entry or WarpDrc20 holder can allocate.
const NEW_ENTRY_MEMORY_BOUND: u64 = 13 * WASM_PAGE_BYTES;
/// The most a WarpDrc20 allowance for a new spender can allocate.
const NEW_ALLOWANCE_MEMORY_BOUND: u64 = 14 * WASM_PAGE_BYTES;
/// The most allowances a WarpDrc20 owner holds at once.
const MAX_ALLOWANCES_PER_OWNER: u32 = 64;
/// `--warp-collateral-token` value that deploys the pinned canonical DRC20.
const CANONICAL_DRC20: &str = "canonical-drc20";
/// The pinned canonical DRC20's Wasm in the WASM directory, built by `make
/// canonical-drc20-fixture`.
const CANONICAL_DRC20_WASM: &str = "canonical_drc20_roles_pausable.wasm";
/// Why a collateral route doesn't wrap a WarpDrc20.
const WARP_DRC20_COLLATERAL_REFUSED: &str = "a collateral route pays out with its token's \
     `transfer`, and a WarpDrc20 refuses transfers to new holders once its memory nears its 3 GiB \
     reserve, so the route would stop paying new accounts (docs/security.md). Wrap a plain DRC20, \
     such as the pinned canonical one (`--warp-collateral-token canonical-drc20`)";
/// The ProtocolFee `deploy-hyperlane` deploys: 0.001 DUSK a dispatch, which
/// its owner can raise to at most 0.1 DUSK.
const PROTOCOL_FEE: u64 = 1_000_000;
const MAX_PROTOCOL_FEE: u64 = 100_000_000;
/// The hook types the fee contracts report (Hyperlane's HookType).
const IGP_HOOK_TYPE: u8 = 4;
const PROTOCOL_FEE_HOOK_TYPE: u8 = 6;
/// Default gas limit for `drc20-mint`. A mint of the canonical DRC20 takes
/// about 1.9M gas (docs/costs.md); a failed one pays its whole limit, so it
/// stays low.
const MINT_GAS_LIMIT: u64 = 3_000_000;
/// The deploy nonce `deploy-token-route` can't use for its token: the route
/// takes the next one.
const LAST_TOKEN_ROUTE_NONCE: u64 = u64::MAX - 1;
/// Default gas limit for `withdraw-dispatch`: the Mailbox decodes the
/// recipient's key before paying it, which costs about 57M gas, so a
/// withdrawal takes about 59M, directly or through a route's owner-only
/// wrapper. The VM tests check this limit covers both.
const WITHDRAW_DISPATCH_GAS_LIMIT: u64 = 80_000_000;
/// How often a transaction's status is polled.
const TRANSACTION_POLL_INTERVAL: Duration = Duration::from_secs(3);
/// How long a submitted transaction may take to be included.
const INCLUSION_TIMEOUT: Duration = Duration::from_secs(60);
/// The one finality check made without `--wait-finality` gives up after
/// this, so it can't hold up a caller such as the relayer.
const FINALITY_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// `--wait-finality`'s timeout, if it was given.
static FINALITY_WAIT: OnceLock<Option<Duration>> = OnceLock::new();

// ── CLI definition ──────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "dusk-tx", about = "Hyperlane Dusk transaction tool")]
struct Cli {
    /// Wait until each transaction's block is final, not only until the
    /// transaction is included. A block that isn't final yet can still be
    /// reverted, which drops its transactions or runs them again.
    #[arg(long, global = true)]
    wait_finality: bool,
    /// How long --wait-finality waits, in seconds, from submission.
    #[arg(long, global = true, default_value = "300", value_name = "SECONDS")]
    finality_timeout: u64,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Call a contract method via Moonlight transaction.
    Call {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        /// Read the RUES URL from the first stdin line. When combined with
        /// --secret-key-stdin, the key follows on the next line.
        #[arg(long, conflicts_with = "rues_url")]
        rues_url_stdin: bool,
        /// Path to encrypted consensus.keys file.
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        /// Raw BLS secret key hex (alternative to --keys).
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// Contract ID (64 hex chars).
        #[arg(long)]
        contract: String,
        /// Function name.
        #[arg(long, name = "fn", alias = "fn")]
        fn_name: String,
        /// rkyv-serialized arguments as hex.
        #[arg(long, default_value = "")]
        args: String,
        /// Gas limit.
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// the signer is read or a transaction is constructed.
        #[arg(long)]
        expected_chain_id: u8,
        /// Reserved until Rusk supports a non-replayable simulation envelope.
        #[arg(long)]
        simulate_only: bool,
        /// Sign with this Moonlight nonce instead of the account's next one
        /// (its nonce plus one), for a sender that has transactions of its
        /// own in flight. The node holds a transaction whose nonce is ahead
        /// of the account's next one for about 5 seconds, until the ones
        /// before it arrive.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        nonce: Option<u64>,
        /// Return once the node has taken the transaction, after preverify
        /// and propagate, without waiting for its block. The output has its
        /// `tx_id` and `submitted: true` but no block fields. The node can
        /// still drop it: reconcile that hash to learn whether it executed.
        #[arg(long)]
        no_wait: bool,
        /// Send only on the caller's go-ahead. Once the transaction is
        /// signed and its `Prepared TX <hash>; nonce <N>;` line is on
        /// stderr, read one more line from stdin: only the line `send`
        /// sends it. Before that, only unsigned reads reach the node, its
        /// chain ID and, without --nonce, the signer's account; nothing
        /// signed does. The end of stdin, a failed read, any other line, or
        /// none within 30 seconds sends nothing more: the command prints
        /// `Not submitted: <reason>` on stderr and fails. With
        /// --secret-key-stdin the key is then the next line of stdin, not
        /// the rest of it.
        #[arg(long)]
        ack_before_send: bool,
    },
    /// Deploy the full Hyperlane contract stack, then read every contract
    /// back: the owner Rusk records for it, its owner, each binding its init
    /// set and its state version must be what was sent, or the command fails
    /// and reports none of the contracts as deployed.
    DeployHyperlane {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        /// Path to encrypted consensus.keys file.
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        /// Raw BLS secret key hex (alternative to --keys).
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// Hyperlane local domain ID.
        #[arg(long, default_value = "4242")]
        domain: u32,
        /// Gas limit per transaction.
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Deploys must pay at least the
        /// network's minimum deployment gas price (2000 on testnet and mainnet).
        #[arg(long, default_value = "2000")]
        gas_price: u64,
        /// Directory with compiled WASM contracts.
        #[arg(long)]
        wasm_dir: Option<PathBuf>,
        /// Also deploy WarpDrc20 synthetic token.
        #[arg(long)]
        deploy_warp_drc20: bool,
        /// Also deploy the native DUSK collateral warp route.
        #[arg(long)]
        deploy_warp_native: bool,
        /// Deploy a collateral route wrapping a DRC20: its contract ID (64
        /// hex chars), or `canonical-drc20` to deploy the pinned canonical
        /// DRC20 (`canonical_drc20_roles_pausable.wasm` in the WASM
        /// directory, `make canonical-drc20-fixture`) with the deployer as
        /// its admin and holder of --collateral-token-supply. A WarpDrc20
        /// can't be wrapped: a collateral route pays out with its token's
        /// `transfer`, which a WarpDrc20 refuses to new holders past its
        /// 3 GiB reserve.
        #[arg(long)]
        warp_collateral_token: Option<String>,
        #[command(flatten)]
        canonical_token: CanonicalTokenArgs,
        /// Token name for WarpDrc20.
        #[arg(long, default_value = "Wrapped Ether")]
        warp_name: String,
        /// Token symbol for WarpDrc20.
        #[arg(long, default_value = "WETH")]
        warp_symbol: String,
        /// Token decimals for WarpDrc20.
        #[arg(long, default_value = "18")]
        warp_decimals: u8,
        /// Mailbox default ISM. Must be selected explicitly; `testMock` is test-only.
        #[arg(long)]
        default_ism: String,
        /// Initial IGP pricing, repeated as DOMAIN:GAS_OVERHEAD:TOKEN_EXCHANGE_RATE:GAS_PRICE.
        /// At least one destination is required because IGP is the active default hook.
        #[arg(
            long = "igp-domain-config",
            value_name = "DOMAIN:GAS_OVERHEAD:TOKEN_EXCHANGE_RATE:GAS_PRICE",
            required = true
        )]
        igp_domain_configs: Vec<String>,
        /// Comma-separated list of Ethereum validator addresses for `messageIdMultisig`.
        /// Example: `0xabc...,0xdef...`
        #[arg(long, default_value = "")]
        multisig_validators: String,
        /// Threshold (m-of-n) for `messageIdMultisig`.
        #[arg(long, default_value = "0")]
        multisig_threshold: u8,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// anything is queried, the signer is read or a transaction is
        /// constructed.
        #[arg(long)]
        expected_chain_id: u8,
    },
    /// Deploy one contract from its Wasm, with a deploy nonce and an owner
    /// that need not be the signer. A contract ID is BLAKE2b-256 of the
    /// Wasm, the nonce (8 bytes, little-endian) and the owner's key, and of
    /// nothing else, so the same three give the same ID on any chain. The ID
    /// is computed, and checked against --expected-id, before anything is
    /// sent. An existing contract at the expected ID is not trusted without
    /// verification: it is taken, and not deployed again, only when Rusk
    /// reports the given owner for it and, with --check-multicall, it answers
    /// as Multicall for Dusk; otherwise nothing is sent and the command
    /// fails. A contract it deploys is checked the same way.
    DeployWasm {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        /// Path to encrypted consensus.keys file.
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        /// Raw BLS secret key hex (alternative to --keys).
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// The contract's Wasm.
        #[arg(long)]
        wasm: PathBuf,
        /// The owner: a base58 Moonlight address or a 96-byte public key in
        /// hex. Defaults to the signer.
        #[arg(long)]
        owner: Option<String>,
        /// The deploy nonce, part of the contract ID.
        #[arg(long, default_value = "0")]
        deploy_nonce: u64,
        /// The contract ID the Wasm, nonce and owner must give (64 hex
        /// chars). Nothing is sent if they give another.
        #[arg(long)]
        expected_id: Option<String>,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// anything is queried, the signer is read or a transaction is
        /// constructed.
        #[arg(long)]
        expected_chain_id: u8,
        /// The Wasm is Multicall for Dusk: the contract at the ID must answer
        /// `version` with 1, and an `aggregate` of the transfer contract's
        /// `chain_id` with the node's chain ID.
        #[arg(long)]
        check_multicall: bool,
        /// Gas limit. Defaults to the bytecode charge, 100 gas per Wasm
        /// byte, plus 10M.
        #[arg(long)]
        gas_limit: Option<u64>,
        /// Gas price in LUX per unit of gas. Deploys must pay at least the
        /// network's minimum deployment gas price (2000 on testnet and mainnet).
        #[arg(long, default_value = "2000")]
        gas_price: u64,
    },
    /// Deploy a token issued on Dusk and a collateral route that locks it,
    /// for an existing Mailbox: the pinned canonical DRC20
    /// (`canonical_drc20_roles_pausable.wasm` in the WASM directory, `make
    /// canonical-drc20-fixture`), with the signer its admin, holding every
    /// role and the whole initial supply and no cap, and a
    /// WarpDrc20Collateral that locks it, bound to the Mailbox with no
    /// router yet. The token gets --deploy-nonce and the route the next
    /// nonce, so both IDs are known before anything is sent. Each deploy
    /// transaction goes to --tx-log before it is sent. A contract already
    /// at either ID isn't deployed again, and is taken only if the
    /// transaction an earlier run recorded for it (--token-tx, --route-tx)
    /// created it, in a final block; any other contract there is someone
    /// else's, whatever it reads back, and the command stops, sending
    /// nothing more, with "taken" naming it. Then both are read
    /// back, as deploy-hyperlane reads back its own: the owner Rusk records,
    /// the token's settings, supply and roles, and the route's owner and
    /// bindings; when one isn't what was sent, or can't be read, the command
    /// fails and reports neither. With --plan it prints the IDs, and whether
    /// a contract is at each, and sends nothing.
    DeployTokenRoute(TokenRouteArgs),
    /// Query contract state via RUES (read-only, no transaction).
    Query {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        /// Contract ID (64 hex chars).
        #[arg(long)]
        contract: String,
        /// Method name.
        #[arg(long)]
        method: String,
        /// Return type: u8, u32, u64, bool, bytes32, option-bytes32,
        /// contract-id-list, eth-address-list, bytes32-list,
        /// validators-and-threshold, bytes, string, or domain-gas-config.
        #[arg(long, name = "return-type", default_value = "u32")]
        return_type: String,
        /// Optional u32 argument.
        #[arg(long, name = "arg-u32")]
        arg_u32: Option<u32>,
        /// Optional u64 argument, only with --arg-u32 and --return-type u64:
        /// the argument is then the pair (u32, u64), as the IGP's
        /// quote_gas_payment(destination, gas_limit) takes it.
        #[arg(long, name = "arg-u64", requires = "arg-u32")]
        arg_u64: Option<u64>,
        /// Optional pair of u32 arguments, encoded as FIRST,SECOND, for a
        /// page query: ValidatorAnnounce get_announced_validators(start,
        /// limit) or MerkleTreeHook message_ids(start, limit).
        #[arg(long, name = "arg-u32-pair", value_name = "FIRST,SECOND")]
        arg_u32_pair: Option<String>,
        /// Optional bytes32 argument (64 hex chars).
        #[arg(long, name = "arg-bytes32")]
        arg_bytes32: Option<String>,
        /// Optional Ethereum address argument (20-byte hex), for
        /// ValidatorAnnounce get_announced_storage_location_hashes.
        #[arg(long, name = "arg-eth-address")]
        arg_eth_address: Option<String>,
    },
    /// Deposit native DUSK into a Mailbox dispatch-fee credit.
    FundDispatch {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin.
        #[arg(long)]
        secret_key_stdin: bool,
        /// Mailbox contract ID (64 hex chars).
        #[arg(long)]
        mailbox: String,
        /// Encoded sender identity whose fee credit is funded.
        #[arg(long)]
        payer: String,
        /// Native DUSK amount in LUX.
        #[arg(long)]
        amount: u64,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// the signer is read or a transaction is constructed.
        #[arg(long)]
        expected_chain_id: u8,
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Withdraw unused dispatch-fee credit to a Moonlight account.
    WithdrawDispatch {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin.
        #[arg(long)]
        secret_key_stdin: bool,
        /// Mailbox for account credit, or an owned warp route for route credit.
        #[arg(long)]
        target: String,
        /// Native DUSK amount in LUX.
        #[arg(long)]
        amount: u64,
        /// Explicit 96-byte Moonlight BLS public key (hex). Defaults to signer.
        #[arg(long)]
        recipient_public_key: Option<String>,
        /// Expected native Dusk chain ID. The endpoint must match before the
        /// signer is read or a transaction is constructed.
        #[arg(long)]
        expected_chain_id: u8,
        /// Gas limit. The Mailbox decodes the recipient's key, so a
        /// withdrawal takes about 59M gas.
        #[arg(long, default_value_t = WITHDRAW_DISPATCH_GAS_LIMIT)]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Dispatch a message via TestRecipient proxy.
    Dispatch {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// Mailbox contract ID (64 hex chars).
        #[arg(long)]
        mailbox: String,
        /// TestRecipient contract ID (64 hex chars).
        #[arg(long, name = "test-recipient")]
        test_recipient: String,
        /// Destination domain.
        #[arg(long)]
        destination: u32,
        /// Recipient H256 (64 hex chars).
        #[arg(long)]
        recipient: String,
        /// Message body (UTF-8 string or hex with 0x prefix).
        #[arg(long, default_value = "Hello from Dusk!")]
        body: String,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// the signer is read or a transaction is constructed.
        #[arg(long)]
        expected_chain_id: u8,
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Process an inbound message on the Dusk Mailbox.
    Process {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// Mailbox contract ID (64 hex chars).
        #[arg(long)]
        mailbox: String,
        /// Raw encoded Hyperlane message (hex).
        #[arg(long)]
        message: String,
        /// ISM metadata (hex). Empty metadata is valid for TestMock only.
        #[arg(long, default_value = "")]
        metadata: String,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// the signer is read or a transaction is constructed.
        #[arg(long)]
        expected_chain_id: u8,
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Encode a Hyperlane message (no transaction, pure encoding).
    EncodeMessage {
        /// Message version.
        #[arg(long, default_value = "3")]
        version: u8,
        /// Message nonce.
        #[arg(long)]
        nonce: u32,
        /// Origin domain.
        #[arg(long)]
        origin: u32,
        /// Sender H256 (64 hex chars).
        #[arg(long)]
        sender: String,
        /// Destination domain.
        #[arg(long)]
        destination: u32,
        /// Recipient H256 (64 hex chars).
        #[arg(long)]
        recipient: String,
        /// Message body (hex).
        #[arg(long, default_value = "")]
        body: String,
    },
    /// Enroll a remote router on a warp route contract.
    EnrollRouter {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// Warp route contract ID (64 hex chars).
        #[arg(long, name = "warp-contract")]
        warp_contract: String,
        /// Remote domain ID.
        #[arg(long)]
        domain: u32,
        /// Remote router H256 (64 hex chars).
        #[arg(long)]
        router: String,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// the signer is read or a transaction is constructed.
        #[arg(long)]
        expected_chain_id: u8,
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Claim tokens held in escrow on a warp route. A transfer to an account
    /// the route hasn't paid before waits there, and the claim also
    /// registers the account, so later transfers arrive directly.
    ClaimPending {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        /// Path to encrypted consensus.keys file.
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        /// Raw BLS secret key hex (alternative to --keys).
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// Warp route contract ID (64 hex chars).
        #[arg(long, name = "warp-contract")]
        warp_contract: String,
        /// Claim for this account instead of the signer: a base58 Moonlight
        /// address or a 96-byte public key in hex. The tokens go to that
        /// account; the signer only pays the gas.
        #[arg(long = "for")]
        for_account: Option<String>,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// anything is queried, the signer is read or a transaction is
        /// constructed.
        #[arg(long)]
        expected_chain_id: u8,
        /// Gas limit. Defaults to 3M for the signer's own claim
        /// (`claim_pending`, about 1.9M gas) and 80M for another account's
        /// (`claim_pending_for`, about 59M, because the route decodes the
        /// account's public key on chain). A failed claim pays its whole
        /// limit.
        #[arg(long)]
        gas_limit: Option<u64>,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Set an IGP's pricing for one destination domain. Owner only. The
    /// signer and the pricing are checked as the IGP checks them before
    /// anything is signed.
    SetIgpConfig {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        /// Path to encrypted consensus.keys file.
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        /// Raw BLS secret key hex (alternative to --keys).
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// IGP contract ID (64 hex chars).
        #[arg(long)]
        igp: String,
        /// The new pricing, as DOMAIN:GAS_OVERHEAD:TOKEN_EXCHANGE_RATE:GAS_PRICE.
        /// The IGP charges (gas limit + GAS_OVERHEAD) x GAS_PRICE x
        /// TOKEN_EXCHANGE_RATE / 1e10 LUX.
        #[arg(
            long = "igp-domain-config",
            value_name = "DOMAIN:GAS_OVERHEAD:TOKEN_EXCHANGE_RATE:GAS_PRICE"
        )]
        igp_domain_config: String,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// anything is queried, the signer is read or a transaction is
        /// constructed.
        #[arg(long)]
        expected_chain_id: u8,
        /// Gas limit. Setting a domain's pricing takes about 1.86M gas.
        #[arg(long, default_value_t = OWNER_CALL_GAS_LIMIT)]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Set the beneficiary of a ProtocolFee or IGP contract: the account that
    /// can claim its fees. Owner only.
    SetBeneficiary {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        /// Path to encrypted consensus.keys file.
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        /// Raw BLS secret key hex (alternative to --keys).
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// ProtocolFee or IGP contract ID (64 hex chars).
        #[arg(long)]
        contract: String,
        /// The new beneficiary: a base58 Moonlight address or a 96-byte
        /// public key in hex. Only that account can claim, by signing the
        /// claim itself.
        #[arg(long)]
        beneficiary: String,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// anything is queried, the signer is read or a transaction is
        /// constructed.
        #[arg(long)]
        expected_chain_id: u8,
        /// Gas limit. Setting the beneficiary takes about 1.86M gas.
        #[arg(long, default_value_t = OWNER_CALL_GAS_LIMIT)]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Claim the fees a ProtocolFee or IGP contract holds. The signer must be
    /// the contract's beneficiary, and the fees go to its account.
    ClaimFees {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        /// Path to encrypted consensus.keys file.
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        /// Raw BLS secret key hex (alternative to --keys).
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// ProtocolFee or IGP contract ID (64 hex chars).
        #[arg(long)]
        contract: String,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// anything is queried, the signer is read or a transaction is
        /// constructed.
        #[arg(long)]
        expected_chain_id: u8,
        /// Gas limit. A claim takes about 1.87M gas.
        #[arg(long, default_value_t = OWNER_CALL_GAS_LIMIT)]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Pay an IGP for more destination gas for a message, as `payForGas`
    /// does on EVM, so the relayer delivers a message whose dispatch paid
    /// for too little. Anyone can pay for any message, and the relayer adds
    /// up every payment for its ID and destination. Prints the IGP's quote
    /// and checks the payment can succeed before anything is signed, since a
    /// failed transaction pays its whole gas limit.
    PayForGas {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        /// Path to encrypted consensus.keys file.
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        /// Raw BLS secret key hex (alternative to --keys).
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// IGP contract ID (64 hex chars).
        #[arg(long)]
        igp: String,
        /// The ID of the message to pay for (64 hex chars). Nothing checks
        /// that it was dispatched: a payment for a wrong ID or destination
        /// is lost. --nonce looks the message up instead.
        #[arg(long, required_unless_present = "nonce", conflicts_with = "nonce")]
        message_id: Option<String>,
        /// The message's destination domain. Checked against the message
        /// with --nonce.
        #[arg(long, required_unless_present = "nonce")]
        destination: Option<u32>,
        /// Pay for the message the IGP's Mailbox dispatched at this nonce:
        /// its ID and destination come from its Dispatch event, so the node
        /// must serve the archive.
        #[arg(long)]
        nonce: Option<u32>,
        /// The destination gas to pay for, on top of what the message has
        /// paid for. The IGP charges GAS_AMOUNT x gas price x exchange rate
        /// / 1e10 LUX, with no overhead.
        #[arg(long)]
        gas_amount: u64,
        /// The most to pay, in LUX: the transaction's deposit. The IGP keeps
        /// its price when the transaction runs and sends the rest back, so a
        /// margin covers a price update in between. Defaults to the quote.
        #[arg(long)]
        max_payment: Option<u64>,
        /// Print the quote and the checks, and sign nothing.
        #[arg(long)]
        quote_only: bool,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// anything is queried, the signer is read or a transaction is
        /// constructed. Optional with --quote-only, which signs nothing.
        #[arg(long, required_unless_present = "quote_only")]
        expected_chain_id: Option<u8>,
        /// Gas limit. A top-up takes about 1.85M gas.
        #[arg(long, default_value_t = PAY_FOR_GAS_GAS_LIMIT)]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Approve a DRC20 contract spender using the current Dusk ABI.
    Drc20Approve {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        secret_key: Option<String>,
        #[arg(long)]
        secret_key_stdin: bool,
        #[arg(long)]
        token: String,
        #[arg(long)]
        spender: String,
        #[arg(long)]
        amount: u64,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// the signer is read or a transaction is constructed.
        #[arg(long)]
        expected_chain_id: u8,
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Query a DRC20 balance for a Moonlight or contract account.
    Drc20Balance {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        secret_key: Option<String>,
        #[arg(long)]
        secret_key_stdin: bool,
        #[arg(long)]
        token: String,
        /// Query this contract account instead of the configured Moonlight account.
        #[arg(long)]
        account_contract: Option<String>,
    },
    /// Mint a canonical DRC20 to an account. The signer must hold the
    /// token's minter role, as the admin `deploy-token-route` sets up does.
    /// The role, whether the token is paused and its cap are checked before
    /// anything is signed, since a failed transaction pays its whole gas
    /// limit.
    Drc20Mint(Drc20MintArgs),
    /// Show a canonical DRC20: its name, symbol, decimals, supply and cap,
    /// and whether it is paused; with --account, the account's balance and
    /// which of the token's roles it holds.
    Drc20Info(Drc20InfoArgs),
    /// Call transfer_remote on a warp route. The quoted dispatch fee is
    /// attached as a native DUSK deposit.
    TransferRemote {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// Warp route contract ID (64 hex chars).
        #[arg(long, name = "warp-contract")]
        warp_contract: String,
        /// Destination domain.
        #[arg(long)]
        destination: u32,
        /// Recipient on the destination chain: a 20-byte EVM address (40 hex
        /// chars), or 32 bytes (64 hex chars) as Hyperlane encodes it.
        #[arg(long)]
        recipient: String,
        /// Accept a 32-byte recipient whose first 12 bytes aren't zero. Only
        /// for non-EVM destinations: an EVM route can never deliver to it, and
        /// the tokens would be locked.
        #[arg(long)]
        raw_recipient: bool,
        /// Amount in the token's smallest unit (LUX for DUSK: 1 DUSK = 1000000000).
        #[arg(long)]
        amount: u64,
        /// Bridge native DUSK through WarpNative: the deposit is `amount` plus the
        /// quoted dispatch fee.
        #[arg(long)]
        native: bool,
        /// Expected native Dusk chain ID. The endpoint must report it before
        /// anything is queried, the signer is read or a transaction is
        /// constructed.
        #[arg(long)]
        expected_chain_id: u8,
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Print the message the Mailbox dispatched at a nonce. The Mailbox stores
    /// only the message ID; the message comes from that block's Dispatch
    /// event, so the node must serve the archive.
    DispatchedMessage {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        /// Mailbox contract ID (64 hex chars).
        #[arg(long)]
        mailbox: String,
        /// Dispatch nonce.
        #[arg(long)]
        nonce: u32,
    },
    /// Show a Moonlight account: its address, the recipient hash warp routes
    /// deliver to, and its balance and nonce.
    Account {
        #[arg(long, default_value = "http://localhost:8080/")]
        rues_url: String,
        /// Path to encrypted consensus.keys file.
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        /// Raw BLS secret key hex (alternative to --keys).
        #[arg(long)]
        secret_key: Option<String>,
        /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
        #[arg(long)]
        secret_key_stdin: bool,
        /// Look up this Moonlight address (base58) instead of a key you hold.
        #[arg(long, conflicts_with_all = ["keys", "secret_key", "secret_key_stdin", "public_key"])]
        address: Option<String>,
        /// Look up this 96-byte BLS public key (hex) instead of a key you hold.
        #[arg(long, conflicts_with_all = ["keys", "secret_key", "secret_key_stdin"])]
        public_key: Option<String>,
        /// Also show whether the account is registered on this warp route
        /// and how much is waiting for it in escrow.
        #[arg(long, name = "warp-contract")]
        warp_contract: Option<String>,
        /// Only print the address and recipient hash; don't query the node.
        #[arg(long, conflicts_with = "warp-contract")]
        offline: bool,
    },
    /// Write the BLS secret key from an encrypted consensus.keys file to a new
    /// file (mode 0600), as the Hyperlane agents' `duskKey` signer reads it.
    ExportSignerKey {
        /// Path to encrypted consensus.keys file.
        #[arg(long)]
        keys: PathBuf,
        /// Password for consensus.keys. Takes precedence over
        /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
        #[arg(long)]
        password: Option<String>,
        /// File to create. It must not exist yet.
        #[arg(long)]
        out: PathBuf,
    },
}

// ── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    FINALITY_WAIT
        .set(
            cli.wait_finality
                .then(|| Duration::from_secs(cli.finality_timeout)),
        )
        .expect("set once");
    if let Err(e) = run(cli.command).await {
        let output = json!({ "success": false, "error": e });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
        std::process::exit(1);
    }
}

/// Run one command. On success it has printed its JSON output.
async fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Call {
            rues_url,
            rues_url_stdin,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            contract,
            fn_name,
            args,
            gas_limit,
            gas_price,
            expected_chain_id,
            simulate_only,
            nonce,
            no_wait,
            ack_before_send,
        } => {
            cmd_call(
                &rues_url,
                rues_url_stdin,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &contract,
                &fn_name,
                &args,
                gas_limit,
                gas_price,
                expected_chain_id,
                simulate_only,
                CallSubmission { nonce, no_wait },
                ack_before_send.then(SendAck::stdin),
            )
            .await
        }
        Command::DeployHyperlane {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            domain,
            gas_limit,
            gas_price,
            wasm_dir,
            deploy_warp_drc20,
            deploy_warp_native,
            warp_collateral_token,
            canonical_token,
            warp_name,
            warp_symbol,
            warp_decimals,
            default_ism,
            igp_domain_configs,
            multisig_validators,
            multisig_threshold,
            expected_chain_id,
        } => {
            cmd_deploy_hyperlane(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                domain,
                gas_limit,
                gas_price,
                wasm_dir,
                deploy_warp_drc20,
                deploy_warp_native,
                warp_collateral_token.as_deref(),
                CollateralTokenSettings {
                    name: canonical_token.collateral_token_name,
                    symbol: canonical_token.collateral_token_symbol,
                    decimals: canonical_token
                        .collateral_token_decimals
                        .unwrap_or(warp_decimals),
                    supply: canonical_token.collateral_token_supply,
                },
                &warp_name,
                &warp_symbol,
                warp_decimals,
                &default_ism,
                &igp_domain_configs,
                &multisig_validators,
                multisig_threshold,
                expected_chain_id,
            )
            .await
        }
        Command::DeployWasm {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            wasm,
            owner,
            deploy_nonce,
            expected_id,
            expected_chain_id,
            check_multicall,
            gas_limit,
            gas_price,
        } => {
            cmd_deploy_wasm(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &wasm,
                owner.as_deref(),
                deploy_nonce,
                expected_id.as_deref(),
                expected_chain_id,
                check_multicall,
                gas_limit,
                gas_price,
            )
            .await
        }
        // Boxed: its future is large, and `run`'s would otherwise be as large.
        Command::DeployTokenRoute(args) => Box::pin(cmd_deploy_token_route(args)).await,
        Command::Query {
            rues_url,
            contract,
            method,
            return_type,
            arg_u32,
            arg_u64,
            arg_u32_pair,
            arg_bytes32,
            arg_eth_address,
        } => {
            cmd_query(
                &rues_url,
                &contract,
                &method,
                &return_type,
                arg_u32,
                arg_u64,
                arg_u32_pair.as_deref(),
                arg_bytes32,
                arg_eth_address,
            )
            .await
        }
        Command::FundDispatch {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            mailbox,
            payer,
            amount,
            expected_chain_id,
            gas_limit,
            gas_price,
        } => {
            cmd_fund_dispatch(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &mailbox,
                &payer,
                amount,
                expected_chain_id,
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::WithdrawDispatch {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            target,
            amount,
            recipient_public_key,
            expected_chain_id,
            gas_limit,
            gas_price,
        } => {
            cmd_withdraw_dispatch(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &target,
                amount,
                recipient_public_key.as_deref(),
                expected_chain_id,
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::Dispatch {
            rues_url,
            keys,
            password,
            secret_key,
            mailbox,
            test_recipient,
            secret_key_stdin,
            destination,
            recipient,
            body,
            expected_chain_id,
            gas_limit,
            gas_price,
        } => {
            cmd_dispatch(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &mailbox,
                &test_recipient,
                destination,
                &recipient,
                &body,
                expected_chain_id,
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::Process {
            rues_url,
            keys,
            password,
            secret_key,
            mailbox,
            message,
            metadata,
            secret_key_stdin,
            expected_chain_id,
            gas_limit,
            gas_price,
        } => {
            cmd_process(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &mailbox,
                &message,
                &metadata,
                expected_chain_id,
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::EncodeMessage {
            version,
            nonce,
            origin,
            sender,
            destination,
            recipient,
            body,
        } => cmd_encode_message(
            version,
            nonce,
            origin,
            &sender,
            destination,
            &recipient,
            &body,
        ),
        Command::EnrollRouter {
            rues_url,
            keys,
            password,
            secret_key,
            warp_contract,
            secret_key_stdin,
            domain,
            router,
            expected_chain_id,
            gas_limit,
            gas_price,
        } => {
            cmd_enroll_router(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &warp_contract,
                domain,
                &router,
                expected_chain_id,
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::ClaimPending {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            warp_contract,
            for_account,
            expected_chain_id,
            gas_limit,
            gas_price,
        } => {
            cmd_claim_pending(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &warp_contract,
                for_account.as_deref(),
                expected_chain_id,
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::SetIgpConfig {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            igp,
            igp_domain_config,
            expected_chain_id,
            gas_limit,
            gas_price,
        } => {
            cmd_set_igp_config(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &igp,
                &igp_domain_config,
                expected_chain_id,
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::SetBeneficiary {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            contract,
            beneficiary,
            expected_chain_id,
            gas_limit,
            gas_price,
        } => {
            cmd_set_beneficiary(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &contract,
                &beneficiary,
                expected_chain_id,
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::ClaimFees {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            contract,
            expected_chain_id,
            gas_limit,
            gas_price,
        } => {
            cmd_claim_fees(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &contract,
                expected_chain_id,
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::PayForGas {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            igp,
            message_id,
            destination,
            nonce,
            gas_amount,
            max_payment,
            quote_only,
            expected_chain_id,
            gas_limit,
            gas_price,
        } => {
            let message = match (message_id, nonce) {
                (Some(id), _) => TopUpMessage::Id {
                    id,
                    destination: destination
                        .expect("clap requires --destination with --message-id"),
                },
                (None, Some(nonce)) => TopUpMessage::Nonce { nonce, destination },
                (None, None) => unreachable!("clap requires --message-id or --nonce"),
            };
            cmd_pay_for_gas(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &igp,
                message,
                gas_amount,
                max_payment,
                quote_only,
                expected_chain_id,
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::Drc20Approve {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            token,
            spender,
            amount,
            expected_chain_id,
            gas_limit,
            gas_price,
        } => {
            cmd_drc20_approve(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &token,
                &spender,
                amount,
                expected_chain_id,
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::Drc20Balance {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            token,
            account_contract,
        } => {
            cmd_drc20_balance(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &token,
                account_contract.as_deref(),
            )
            .await
        }
        Command::Drc20Mint(args) => {
            let output = drc20_mint(
                &args.rues_url,
                args.keys,
                args.password.as_deref().unwrap_or_default(),
                args.secret_key,
                args.secret_key_stdin,
                &args.token,
                args.to.as_deref(),
                args.amount,
                args.expected_chain_id,
                args.gas_limit,
                args.gas_price,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
            Ok(())
        }
        Command::Drc20Info(args) => {
            let output =
                drc20_info_output(&args.rues_url, &args.token, args.account.as_deref()).await?;
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
            Ok(())
        }
        Command::TransferRemote {
            rues_url,
            keys,
            password,
            secret_key,
            warp_contract,
            secret_key_stdin,
            destination,
            recipient,
            raw_recipient,
            amount,
            native,
            expected_chain_id,
            gas_limit,
            gas_price,
        } => {
            cmd_transfer_remote(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &warp_contract,
                destination,
                &recipient,
                raw_recipient,
                amount,
                native,
                expected_chain_id,
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::Account {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            address,
            public_key,
            warp_contract,
            offline,
        } => {
            let pk = if let Some(address) = address {
                parse_moonlight_address(&address)?
            } else if let Some(public_key) = public_key {
                parse_account_public_key(&public_key)?
            } else {
                load_keys(
                    keys,
                    password.as_deref().unwrap_or_default(),
                    secret_key,
                    secret_key_stdin,
                )?
                .1
            };
            cmd_account(&rues_url, &pk, warp_contract.as_deref(), offline).await
        }
        Command::DispatchedMessage {
            rues_url,
            mailbox,
            nonce,
        } => cmd_dispatched_message(&rues_url, &mailbox, nonce).await,
        Command::ExportSignerKey {
            keys,
            password,
            out,
        } => cmd_export_signer_key(keys, password.as_deref().unwrap_or_default(), &out),
    }
}

// ── Key loading helper ──────────────────────────────────────────────────────

fn load_keys(
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
) -> Result<(BlsSecretKey, BlsPublicKey), String> {
    load_keys_with(
        keys_path,
        password,
        secret_key_hex,
        secret_key_stdin,
        StdinKey::Rest,
    )
}

/// How `--secret-key-stdin` reads the key.
#[derive(Debug, Clone, Copy)]
enum StdinKey {
    /// All that is left on stdin.
    Rest,
    /// The next line only: `call --ack-before-send` reads its go-ahead
    /// from the line after it.
    Line,
}

/// [`load_keys`], reading a `--secret-key-stdin` key as `stdin_key` says.
fn load_keys_with(
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    stdin_key: StdinKey,
) -> Result<(BlsSecretKey, BlsPublicKey), String> {
    if secret_key_stdin {
        if secret_key_hex.is_some() || keys_path.is_some() {
            return Err("Provide only one of --keys, --secret-key, or --secret-key-stdin".into());
        }
        let stdin = std::io::stdin().lock();
        let mut s = match stdin_key {
            StdinKey::Rest => read_secret_key_hex(stdin)?,
            StdinKey::Line => read_secret_key_line(stdin)?,
        };
        if let Some(stripped) = s.strip_prefix("0x") {
            s = stripped.to_string();
        }
        let s = s.trim();
        if s.is_empty() {
            return Err("Secret key read from stdin is empty".into());
        }
        keys::load_from_hex(s)
    } else if let Some(hex) = secret_key_hex {
        keys::load_from_hex(&hex)
    } else if let Some(path) = keys_path {
        let path_str = path
            .to_str()
            .ok_or_else(|| "Invalid keys path".to_string())?;
        let password = resolve_keys_password(password)?;
        keys::load_from_file(path_str, &password)
    } else {
        Err("Provide --keys <path>, --secret-key <hex>, or --secret-key-stdin".into())
    }
}

fn read_rues_url(reader: impl BufRead) -> Result<String, String> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_RUES_URL_STDIN_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| "Failed to read RUES URL from stdin".to_string())?;
    if bytes.len() > MAX_RUES_URL_STDIN_BYTES {
        return Err("RUES URL stdin line exceeds 8192 bytes".into());
    }
    if bytes.pop() != Some(b'\n') {
        return Err("RUES URL stdin must end with a newline".into());
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err("RUES URL stdin is empty".into());
    }
    String::from_utf8(bytes).map_err(|_| "RUES URL stdin must be UTF-8".into())
}

fn read_secret_key_hex(reader: impl Read) -> Result<String, String> {
    let mut buf = String::new();
    reader
        .take((MAX_SECRET_KEY_STDIN_BYTES + 1) as u64)
        .read_to_string(&mut buf)
        .map_err(|e| format!("Failed to read secret key from stdin: {e}"))?;
    if buf.len() > MAX_SECRET_KEY_STDIN_BYTES {
        return Err(format!(
            "Secret key stdin exceeds {MAX_SECRET_KEY_STDIN_BYTES} bytes"
        ));
    }
    Ok(buf.trim().to_string())
}

/// The key on the next line of stdin. Nothing after its newline is read,
/// so the caller can keep stdin open and write more.
fn read_secret_key_line(reader: impl BufRead) -> Result<String, String> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_SECRET_KEY_STDIN_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| format!("Failed to read secret key from stdin: {e}"))?;
    if bytes.len() > MAX_SECRET_KEY_STDIN_BYTES {
        return Err(format!(
            "Secret key stdin line exceeds {MAX_SECRET_KEY_STDIN_BYTES} bytes"
        ));
    }
    let line = String::from_utf8(bytes).map_err(|_| "Secret key stdin must be UTF-8")?;
    Ok(line.trim().to_string())
}

/// Resolve the consensus.keys password: an explicit `--password` first, then
/// `DUSK_CONSENSUS_PASSWORD_FILE`, then `DUSK_CONSENSUS_PASSWORD` (or the
/// legacy `DUSK_CONSENSUS_KEYS_PASS`), then the development default
/// `password`. An empty `cli_password` means `--password` wasn't given.
fn resolve_keys_password(cli_password: &str) -> Result<String, String> {
    if !cli_password.is_empty() {
        return Ok(cli_password.to_string());
    }
    if let Ok(path) = env::var("DUSK_CONSENSUS_PASSWORD_FILE") {
        let file = fs::File::open(&path)
            .map_err(|e| format!("Failed to open DUSK_CONSENSUS_PASSWORD_FILE {path}: {e}"))?;
        let mut password = String::new();
        file.take((MAX_PASSWORD_FILE_BYTES + 1) as u64)
            .read_to_string(&mut password)
            .map_err(|e| format!("Failed to read DUSK_CONSENSUS_PASSWORD_FILE {path}: {e}"))?;
        if password.len() > MAX_PASSWORD_FILE_BYTES {
            return Err(format!(
                "DUSK_CONSENSUS_PASSWORD_FILE exceeds {MAX_PASSWORD_FILE_BYTES} bytes"
            ));
        }
        let password = password.trim_end_matches(&['\r', '\n'][..]).to_string();
        if password.is_empty() {
            return Err("DUSK_CONSENSUS_PASSWORD_FILE is empty".into());
        }
        return Ok(password);
    }

    if let Ok(password) =
        env::var("DUSK_CONSENSUS_PASSWORD").or_else(|_| env::var("DUSK_CONSENSUS_KEYS_PASS"))
    {
        if password.is_empty() {
            return Err("Dusk consensus password environment variable is empty".into());
        }
        return Ok(password);
    }

    Ok(String::from("password"))
}

#[cfg(test)]
mod tests {
    use super::{
        confirmation_error_with_hash, next_moonlight_nonce, parse_account_public_key,
        parse_eth_addresses_in_order, parse_igp_domain_configs, parse_u32_pair,
        prepare_bytes32_query_argument, prepare_dispatch_call, prepare_process_call,
        read_secret_key_hex, resolve_keys_password, resolve_signing_chain_id,
        submission_error_with_hash, wait_for_transaction_with, Bytes32QueryArgument, Cli,
        Confirmation, MAX_CALL_ARGS_BYTES, MAX_PASSWORD_FILE_BYTES, MAX_SECRET_KEY_STDIN_BYTES,
    };
    use super::{finality_check_with, wait_for_finality_with};
    use crate::rues::{BlockStatus, Inclusion, TransactionStatus, TransactionStatusQueryError};
    use clap::Parser;
    use dusk_bytes::Serializable;
    use std::collections::VecDeque;
    use std::future::ready;
    use std::sync::Mutex;
    use std::time::Duration;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// `--mailbox` may carry a `0x` prefix or upper-case hex, and so may the
    /// event source; the match is on the bytes.
    #[test]
    fn dispatched_message_matches_the_mailbox_in_any_hex_form() {
        use hyperlane_dusk_types::events::Dispatch;

        let mailbox = [0xab_u8; 32];
        let message = b"dispatched message".to_vec();
        let id = hyperlane_dusk_types::message::id(&message);
        let data = hex::encode(super::rkyv_serialize(&Dispatch {
            sender: [1; 32],
            destination: 2,
            recipient: [3; 32],
            message: message.clone(),
        }));
        let event = |source: String, reverted: bool| {
            serde_json::json!({
                "source": source,
                "topic": Dispatch::TOPIC,
                "data": data,
                "reverted": reverted,
            })
        };
        let other_contract = event(hex::encode([0xcd_u8; 32]), false);
        let reverted = event(hex::encode(mailbox), true);
        let forms = [
            hex::encode(mailbox),
            format!("0x{}", hex::encode(mailbox)),
            hex::encode_upper(mailbox),
        ];
        for source in &forms {
            let events = [
                other_contract.clone(),
                reverted.clone(),
                event(source.clone(), false),
            ];
            for argument in &forms {
                let parsed = super::parse_bytes32(argument).unwrap();
                assert_eq!(
                    super::find_dispatched_message(&events, &parsed, &id),
                    Some(message.clone()),
                    "event source {source}, --mailbox {argument}"
                );
            }
        }
        assert_eq!(
            super::find_dispatched_message(&[other_contract, reverted], &mailbox, &id),
            None
        );
    }

    /// Routes register an account only by claiming its escrow, so there's
    /// no `register-account`, and `claim-pending` takes another account.
    #[test]
    fn only_claim_pending_takes_another_account() {
        let route = "11".repeat(32);
        let account = "22".repeat(96);
        let register = [
            "dusk-tx",
            "register-account",
            "--warp-contract",
            &route,
            "--expected-chain-id",
            "2",
        ];
        assert!(Cli::try_parse_from(register).is_err());
        assert!(Cli::try_parse_from([
            "dusk-tx",
            "claim-pending",
            "--warp-contract",
            &route,
            "--for",
            &account,
            "--expected-chain-id",
            "2",
        ])
        .is_ok());
    }

    /// The signer's own escrow is claimed with `claim_pending` and its small
    /// default gas limit, also when `--for` names the signer, by address or
    /// by key. Only another account's escrow needs `claim_pending_for` and
    /// its 80M default. An explicit `--gas-limit` wins either way.
    #[test]
    fn claim_pending_call_and_gas_limit_follow_whose_escrow_is_claimed() {
        let (_, signer) = crate::keys::load_from_hex(&"11".repeat(32)).unwrap();
        let (_, other) = crate::keys::load_from_hex(&"22".repeat(32)).unwrap();
        let identity = super::account_identity(&signer);
        let by_address = super::parse_account_arg(identity["address"].as_str().unwrap()).unwrap();
        let by_key = super::parse_account_arg(identity["public_key"].as_str().unwrap()).unwrap();

        for named in [None, Some(signer), Some(by_address), Some(by_key)] {
            let call = super::claim_call(&signer, named, None);
            assert_eq!(call.fn_name, "claim_pending");
            assert!(call.args.is_empty());
            assert_eq!(call.account, signer);
            assert_eq!(call.gas_limit, super::CLAIM_PENDING_GAS_LIMIT);
        }

        let call = super::claim_call(&signer, Some(other), None);
        assert_eq!(call.fn_name, "claim_pending_for");
        assert_eq!(call.args, super::rkyv_serialize(&(other,)));
        assert_eq!(call.account, other);
        assert_eq!(call.gas_limit, super::CLAIM_PENDING_FOR_GAS_LIMIT);

        for named in [None, Some(signer), Some(other)] {
            assert_eq!(
                super::claim_call(&signer, named, Some(4_000_000)).gas_limit,
                4_000_000
            );
        }
    }

    #[test]
    fn claim_pending_gas_limit_is_optional() {
        let route = "11".repeat(32);
        let gas_limit = |extra: &[&str]| {
            let args = [
                "dusk-tx",
                "claim-pending",
                "--warp-contract",
                &route,
                "--expected-chain-id",
                "2",
            ];
            match Cli::try_parse_from(args.iter().chain(extra))
                .unwrap()
                .command
            {
                super::Command::ClaimPending { gas_limit, .. } => gas_limit,
                _ => unreachable!("parsed claim-pending"),
            }
        };
        assert_eq!(gas_limit(&[]), None);
        assert_eq!(gas_limit(&["--gas-limit", "5000000"]), Some(5_000_000));
    }

    #[test]
    fn signing_chain_id_must_match_the_configured_endpoint_identity() {
        assert_eq!(resolve_signing_chain_id(1, 1).unwrap(), 1);
        assert!(resolve_signing_chain_id(1, 2)
            .unwrap_err()
            .contains("does not match endpoint chain ID"));
    }

    #[test]
    fn withdrawal_gas_limit_covers_decoding_the_recipient() {
        let target = "11".repeat(32);
        let gas_limit = |extra: &[&str]| {
            let args = [
                "dusk-tx",
                "withdraw-dispatch",
                "--target",
                &target,
                "--amount",
                "1",
                "--expected-chain-id",
                "1",
            ];
            match Cli::try_parse_from(args.iter().chain(extra))
                .unwrap()
                .command
            {
                super::Command::WithdrawDispatch { gas_limit, .. } => gas_limit,
                _ => unreachable!("parsed withdraw-dispatch"),
            }
        };
        assert_eq!(gas_limit(&[]), super::WITHDRAW_DISPATCH_GAS_LIMIT);
        assert_eq!(gas_limit(&["--gas-limit", "90000000"]), 90_000_000);
    }

    #[test]
    fn withdrawal_requires_an_explicit_expected_chain_id() {
        let target = "11".repeat(32);
        assert!(Cli::try_parse_from([
            "dusk-tx",
            "withdraw-dispatch",
            "--target",
            &target,
            "--amount",
            "1",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "dusk-tx",
            "withdraw-dispatch",
            "--target",
            &target,
            "--amount",
            "1",
            "--expected-chain-id",
            "1",
        ])
        .is_ok());
    }

    #[test]
    fn igp_domain_configs_are_explicit_unique_and_nonzero_priced() {
        let values = vec![
            "31338:50000:10000000000:1".to_string(),
            "1:100000:20000000000:2".to_string(),
        ];
        let configs = parse_igp_domain_configs(&values).unwrap();
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].0, 1);
        assert_eq!(configs[1].0, 31338);

        for invalid in [
            Vec::<String>::new(),
            vec!["31338:0:0:1".to_string()],
            vec!["31338:0:10000000000:0".to_string()],
            vec!["31338:0:1:1".to_string()],
            vec![format!("31338:{}:{}:{}", u64::MAX, u64::MAX, u64::MAX)],
            vec![
                "31338:0:10000000000:1".to_string(),
                "31338:1:10000000000:2".to_string(),
            ],
        ] {
            assert!(parse_igp_domain_configs(&invalid).is_err());
        }
    }

    #[test]
    fn private_url_stdin_preserves_the_following_signer_line() {
        let key = "11".repeat(32);
        let url = "http://synthetic:password@localhost/private?token=fixture";
        let bytes = format!("{url}\n{key}\n");
        let mut input = std::io::BufReader::new(bytes.as_bytes());
        assert_eq!(super::read_rues_url(&mut input).unwrap(), url);
        assert_eq!(read_secret_key_hex(input).unwrap(), key);
        for bytes in [
            b"\n".to_vec(),
            b"missing-newline".to_vec(),
            vec![b'a'; super::MAX_RUES_URL_STDIN_BYTES + 1],
            vec![0xff, b'\n'],
        ] {
            assert!(super::read_rues_url(bytes.as_slice()).is_err());
        }
    }

    #[test]
    fn private_url_flag_accepts_stdin_key_and_rejects_an_argv_url() {
        let contract = "11".repeat(32);
        let mut args = vec![
            "dusk-tx",
            "call",
            "--rues-url-stdin",
            "--expected-chain-id",
            "1",
            "--secret-key-stdin",
            "--contract",
            &contract,
            "--fn-name",
            "process",
        ];
        assert!(Cli::try_parse_from(&args).is_ok());
        args.extend(["--rues-url", "http://localhost"]);
        assert!(Cli::try_parse_from(&args).is_err());
    }

    #[test]
    fn secret_key_stdin_is_bounded() {
        let oversized = vec![b'a'; MAX_SECRET_KEY_STDIN_BYTES + 1];
        assert!(read_secret_key_hex(oversized.as_slice())
            .unwrap_err()
            .contains("exceeds"));

        let key = format!("0x{}\n", "11".repeat(32));
        assert_eq!(read_secret_key_hex(key.as_bytes()).unwrap(), key.trim());
    }

    #[test]
    fn process_call_arguments_share_the_helper_transport_bound() {
        let oversized_metadata = hex::encode(vec![0u8; MAX_CALL_ARGS_BYTES]);
        assert!(prepare_process_call("00", &oversized_metadata)
            .unwrap_err()
            .contains("helper transport limit"));

        let (args, message_id) = prepare_process_call("00", "").unwrap();
        assert!(args.len() <= MAX_CALL_ARGS_BYTES);
        assert_eq!(message_id, hyperlane_dusk_types::message::id(&[0u8]));
    }

    #[test]
    fn withdrawal_recipient_public_key_is_checked() {
        let (_, public_key) = crate::keys::load_from_hex(&"11".repeat(32)).unwrap();
        let encoded = hex::encode(public_key.to_bytes());
        assert_eq!(parse_account_public_key(&encoded).unwrap(), public_key);
        assert_eq!(
            parse_account_public_key(&format!("0x{encoded}")).unwrap(),
            public_key
        );

        let mut identity = [0u8; 96];
        identity[0] = 0xc0;
        assert!(parse_account_public_key(&hex::encode(identity))
            .unwrap_err()
            .contains("identity"));
        assert!(parse_account_public_key("abcd")
            .unwrap_err()
            .contains("96 bytes"));
    }

    #[test]
    fn account_identity_round_trips_the_moonlight_address() {
        let (_, public_key) = crate::keys::load_from_hex(&"11".repeat(32)).unwrap();
        let identity = super::account_identity(&public_key);
        let address = identity["address"].as_str().unwrap();
        assert_eq!(super::parse_moonlight_address(address).unwrap(), public_key);
        assert_eq!(
            identity["account_h256"].as_str().unwrap(),
            hex::encode(hyperlane_dusk_types::message::keccak256(
                &public_key.to_bytes()
            ))
        );
        assert!(super::parse_moonlight_address("0OIl")
            .unwrap_err()
            .contains("Invalid Moonlight address"));
        assert!(
            super::parse_moonlight_address(&bs58::encode([1u8; 32]).into_string())
                .unwrap_err()
                .contains("Moonlight address must decode to 96 bytes, got 32")
        );
    }

    #[test]
    fn recipients_are_padded_evm_addresses_unless_raw() {
        let address = "23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f";
        let padded = super::parse_recipient(&format!("0x{address}"), false).unwrap();
        assert_eq!(&padded[..12], &[0u8; 12]);
        assert_eq!(hex::encode(&padded[12..]), address.to_lowercase());
        assert_eq!(
            super::parse_recipient(&format!("000000000000000000000000{address}"), false).unwrap(),
            padded
        );

        // A right-padded address is the bridger's real mistake: rejected
        // unless explicitly raw.
        let right_padded = format!("{address}000000000000000000000000");
        assert!(super::parse_recipient(&right_padded, false)
            .unwrap_err()
            .contains("isn't an EVM address"));
        assert!(super::parse_recipient(&right_padded, true).is_ok());

        assert!(super::parse_recipient(&"00".repeat(32), true)
            .unwrap_err()
            .contains("cannot be zero"));
        assert!(super::parse_recipient("abcd", false)
            .unwrap_err()
            .contains("20-byte address or 32 bytes"));
    }

    #[test]
    fn accounts_parse_from_address_or_public_key() {
        let (_, public_key) = crate::keys::load_from_hex(&"11".repeat(32)).unwrap();
        let identity = super::account_identity(&public_key);
        let address = identity["address"].as_str().unwrap();
        let hex_key = identity["public_key"].as_str().unwrap();
        assert_eq!(super::parse_account_arg(address).unwrap(), public_key);
        assert_eq!(super::parse_account_arg(hex_key).unwrap(), public_key);
        assert_eq!(
            super::parse_account_arg(&format!("0x{hex_key}")).unwrap(),
            public_key
        );
    }

    #[test]
    fn dusk_amounts_keep_all_decimals() {
        assert_eq!(super::format_dusk(0), "0.000000000");
        assert_eq!(super::format_dusk(1), "0.000000001");
        assert_eq!(super::format_dusk(99_701_351_464_000), "99701.351464000");
    }

    /// The prepared line names a Moonlight transaction's hash and the nonce
    /// it is signed with. The nonce comes after the hash's `;`, so the hash
    /// is still everything before the first `;`. A transaction whose bytes
    /// don't give a Moonlight nonce gets the line without one.
    #[test]
    fn the_prepared_line_names_the_hash_and_the_moonlight_nonce() {
        let (secret_key, _) = crate::keys::load_from_hex(&"11".repeat(32)).unwrap();
        let tx = super::moonlight_call(
            &secret_key,
            dusk_core::abi::ContractId::from_bytes([2; 32]),
            "process",
            vec![1, 2],
            5_000_000,
            1,
            42,
            7,
        )
        .unwrap();
        let tx_id = hex::encode(tx.hash().to_bytes());
        let line = super::prepared_line(&tx_id, super::moonlight_nonce(&tx.to_var_bytes()));
        assert_eq!(
            line,
            format!("  Prepared TX {tx_id}; nonce 42; reconcile this exact hash before retrying if submission is interrupted")
        );
        let rest = line.trim_start().strip_prefix("Prepared TX ").unwrap();
        assert_eq!(rest.split(';').next(), Some(tx_id.as_str()));
        assert_eq!(super::moonlight_nonce(&[1, 2, 3]), None);
        assert_eq!(
            super::prepared_line(&tx_id, None),
            format!("  Prepared TX {tx_id}; reconcile this exact hash before retrying if submission is interrupted")
        );
    }

    #[test]
    fn signer_key_file_is_private_and_never_overwritten() {
        let (secret_key, _) = crate::keys::load_from_hex(&"22".repeat(32)).unwrap();
        let path = std::env::temp_dir().join(format!(
            "dusk-tx-signer-key-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        super::write_signer_key_file(&path, &secret_key).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, format!("0x{}\n", "22".repeat(32)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(super::write_signer_key_file(&path, &secret_key)
            .unwrap_err()
            .contains("Failed to create"));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn dispatch_call_arguments_are_bounded_before_signer_access() {
        let id = "11".repeat(32);
        let recipient = "22".repeat(32);
        let (_, args) = prepare_dispatch_call(&id, &id, 31338, &recipient, "hello").unwrap();
        assert!(args.len() <= MAX_CALL_ARGS_BYTES);

        let oversized_body = "a".repeat(MAX_CALL_ARGS_BYTES);
        assert!(
            prepare_dispatch_call(&id, &id, 31338, &recipient, &oversized_body)
                .unwrap_err()
                .contains("helper transport limit")
        );
    }

    #[test]
    fn validator_lists_keep_their_order_until_deployed() {
        let descending = format!("0x{},0x{}", "22".repeat(20), "11".repeat(20));
        let queried = parse_eth_addresses_in_order(&descending).unwrap();
        assert_eq!(
            queried,
            vec![
                hyperlane_dusk_types::EthAddress([0x22; 20]),
                hyperlane_dusk_types::EthAddress([0x11; 20])
            ]
        );
        let deployed = super::parse_eth_addresses(&descending).unwrap();
        assert_eq!(
            deployed,
            vec![queried[1], queried[0]],
            "multisig deployment must still canonicalize validators"
        );
        let duplicate = format!("0x{},0x{}", "11".repeat(20), "11".repeat(20));
        assert!(parse_eth_addresses_in_order(&duplicate).is_err());
    }

    /// `igp_quote` is the IGP's own arithmetic: (gas + overhead) x price x
    /// rate / 1e10, rounded down.
    #[test]
    fn igp_quote_matches_the_contract_formula() {
        let (domain, config) =
            super::parse_igp_domain_config("11155111:200000:375000:1000000000").unwrap();
        assert_eq!(domain, 11_155_111);
        // 250,000 gas at 1 gwei is 2.5e14 wei; at 375,000 / 1e10 LUX per wei
        // that is 9.375e9 LUX, 9.375 DUSK.
        assert_eq!(super::igp_quote(config, 50_000), Some(9_375_000_000));
        let config = hyperlane_dusk_types::DomainGasConfig {
            gas_overhead: 0,
            token_exchange_rate: 3,
            gas_price: 1,
        };
        assert_eq!(super::igp_quote(config, 3_333_333_333), Some(0));
        assert_eq!(super::igp_quote(config, 3_333_333_334), Some(1));
        let huge = hyperlane_dusk_types::DomainGasConfig {
            gas_overhead: u64::MAX,
            token_exchange_rate: u64::MAX,
            gas_price: u64::MAX,
        };
        assert_eq!(super::igp_quote(huge, 1), None);
        assert!(super::parse_igp_domain_config("11155111:200000:375000").is_err());
        assert!(super::parse_igp_domain_config("11155111:0:1:1").is_err());
    }

    #[test]
    fn fee_contract_commands_default_to_a_low_gas_limit() {
        let contract = "11".repeat(32);
        let parse =
            |args: &[&str]| Cli::try_parse_from(args.iter().copied()).map(|cli| cli.command);
        match parse(&[
            "dusk-tx",
            "set-igp-config",
            "--igp",
            &contract,
            "--igp-domain-config",
            "11155111:200000:375000:1000000000",
            "--expected-chain-id",
            "2",
        ])
        .unwrap()
        {
            super::Command::SetIgpConfig { gas_limit, .. } => {
                assert_eq!(gas_limit, super::OWNER_CALL_GAS_LIMIT);
            }
            _ => unreachable!("parsed set-igp-config"),
        }
        match parse(&[
            "dusk-tx",
            "claim-fees",
            "--contract",
            &contract,
            "--expected-chain-id",
            "2",
        ])
        .unwrap()
        {
            super::Command::ClaimFees { gas_limit, .. } => {
                assert_eq!(gas_limit, super::OWNER_CALL_GAS_LIMIT);
            }
            _ => unreachable!("parsed claim-fees"),
        }
        assert!(parse(&["dusk-tx", "set-beneficiary", "--contract", &contract]).is_err());
        assert!(parse(&["dusk-tx", "set-igp-config", "--igp", &contract]).is_err());
    }

    /// The agents pipeline Dusk deliveries with `call --nonce <N> --no-wait`,
    /// once `call --help` lists both flags, and pass the function as
    /// `--fn-name` (`--fn` works too).
    #[test]
    fn call_takes_a_nonce_and_can_return_without_waiting() {
        use clap::CommandFactory;
        let contract = "11".repeat(32);
        for fn_flag in ["--fn-name", "--fn"] {
            let agent = Cli::try_parse_from([
                "dusk-tx",
                "call",
                "--rues-url-stdin",
                "--expected-chain-id",
                "1",
                "--secret-key-stdin",
                "--contract",
                &contract,
                fn_flag,
                "process",
                "--args",
                "00",
                "--gas-limit",
                "5000000",
                "--gas-price",
                "1",
                "--nonce",
                "9",
                "--no-wait",
            ])
            .unwrap();
            match agent.command {
                super::Command::Call {
                    fn_name,
                    nonce,
                    no_wait,
                    gas_limit,
                    rues_url_stdin,
                    secret_key_stdin,
                    ..
                } => {
                    assert_eq!(fn_name, "process");
                    assert_eq!((nonce, no_wait, gas_limit), (Some(9), true, 5_000_000));
                    assert!(rues_url_stdin && secret_key_stdin);
                }
                _ => unreachable!("parsed call"),
            }
        }
        let base = [
            "dusk-tx",
            "call",
            "--contract",
            &contract,
            "--fn-name",
            "process",
            "--expected-chain-id",
            "1",
        ];
        match Cli::try_parse_from(base).unwrap().command {
            super::Command::Call { nonce, no_wait, .. } => {
                assert_eq!((nonce, no_wait), (None, false))
            }
            _ => unreachable!("parsed call"),
        }
        // A Moonlight transaction's nonce is at least 1: an account's first
        // transaction has nonce 1.
        assert!(Cli::try_parse_from(base.iter().copied().chain(["--nonce", "0"])).is_err());
        assert!(Cli::try_parse_from(base.iter().copied().chain(["--nonce"])).is_err());
        let help = Cli::command()
            .find_subcommand_mut("call")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(
            help.contains("--nonce") && help.contains("--no-wait"),
            "{help}"
        );
        // `--wait-finality` waits for more than inclusion; `--no-wait` waits
        // for none of it, so they can't go together. `--wait-finality` is
        // global and may come before `call`, so `call` checks this itself.
        let no_wait = super::CallSubmission {
            nonce: Some(3),
            no_wait: true,
        };
        let waiting = super::CallSubmission {
            nonce: Some(3),
            no_wait: false,
        };
        let finality = Some(Duration::from_secs(300));
        assert!(super::check_call_submission(no_wait, finality)
            .unwrap_err()
            .contains("--wait-finality"));
        assert!(super::check_call_submission(no_wait, None).is_ok());
        assert!(super::check_call_submission(waiting, finality).is_ok());
        assert!(super::parse_call_inputs(&contract, "", false, no_wait).is_ok());
    }

    /// The paths and bodies of the requests a mock RUES endpoint got.
    type MockRequests = Vec<(String, Vec<u8>)>;
    /// Replies for a mock RUES endpoint, in order: a path suffix, a status
    /// and a body (`mock_rues`).
    type MockReplies = Vec<(&'static str, u16, Vec<u8>)>;

    /// A RUES endpoint on localhost that answers requests in order, one a
    /// connection: each request's path must end with the next reply's
    /// suffix, and gets its status and body. Status 0 is no answer: the
    /// connection stays open for two seconds, then closes. Returns the base
    /// URL and the requests' paths and bodies, once every reply is sent or
    /// 20 seconds pass with no request.
    fn mock_rues(
        replies: Vec<(&'static str, u16, Vec<u8>)>,
    ) -> (String, std::thread::JoinHandle<MockRequests>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (suffix, status, body) in replies {
                let deadline = std::time::Instant::now() + Duration::from_secs(20);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && std::time::Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => return requests,
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let path = first.split_whitespace().nth(1).unwrap().to_owned();
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    assert_ne!(reader.read_line(&mut header).unwrap(), 0);
                    if header == "\r\n" {
                        break;
                    }
                    if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:")
                    {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                let mut input = vec![0; length];
                reader.read_exact(&mut input).unwrap();
                assert!(path.ends_with(suffix), "expected {suffix}, got {path}");
                if status == 0 {
                    std::thread::sleep(Duration::from_secs(2));
                    requests.push((path, input));
                    continue;
                }
                // A 204 has no body, and so no length.
                let length = if status == 204 {
                    String::new()
                } else {
                    format!("Content-Length: {}\r\n", body.len())
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} Mock\r\n{length}Connection: close\r\n\r\n"
                )
                .unwrap();
                stream.write_all(&body).unwrap();
                requests.push((path, input));
            }
            requests
        });
        (url, server)
    }

    /// With `--nonce`, `call` signs with that nonce and doesn't look the
    /// account up; with `--no-wait` it returns once preverify and propagate
    /// accept the transaction, without asking for its block, and its output
    /// has the transaction's ID and nonce but no block fields.
    #[tokio::test]
    async fn call_signs_with_a_given_nonce_and_returns_once_submitted() {
        use dusk_core::transfer::Transaction;
        let (url, server) = mock_rues(vec![
            ("/chain_id", 200, vec![7]),
            ("/on/transactions/preverify", 200, Vec::new()),
            ("/on/transactions/propagate", 200, Vec::new()),
        ]);
        let contract = "22".repeat(32);
        let output = super::call(
            &url,
            None,
            "",
            Some("11".repeat(32)),
            false,
            &contract,
            "process",
            "0102",
            5_000_000,
            1,
            7,
            false,
            super::CallSubmission {
                nonce: Some(42),
                no_wait: true,
            },
            None,
        )
        .await
        .unwrap();
        let requests = server.join().unwrap();
        assert_eq!(
            requests.len(),
            3,
            "the chain ID, preverify and propagate only"
        );
        assert_eq!(
            requests[1].1, requests[2].1,
            "the same transaction both times"
        );
        let tx = Transaction::from_slice(&requests[2].1).unwrap();
        let Transaction::Moonlight(moonlight) = &tx else {
            panic!("a Moonlight transaction");
        };
        assert_eq!(moonlight.nonce(), 42);
        assert_eq!(moonlight.gas_limit(), 5_000_000);
        let tx_id = hex::encode(tx.hash().to_bytes());
        assert_eq!(
            output,
            serde_json::json!({
                "success": true,
                "contract": contract,
                "fn_name": "process",
                "tx_id": tx_id,
                "nonce": 42,
                "submitted": true,
            })
        );
    }

    /// With a chosen nonce, preverify refusing the transaction only because
    /// the nonce is ahead of the account's next one doesn't stop it: the
    /// node's propagate queues it. Any other refusal still does, and keeps
    /// the "before propagation" prefix the agents read as a definite
    /// refusal; a propagation the node doesn't confirm has an unknown
    /// outcome.
    #[tokio::test]
    async fn a_chosen_nonce_passes_a_nonce_gap_and_errors_keep_their_prefixes() {
        let tx_id = "ab".repeat(32);
        let gap = b"Tx ab not accepted: Missing intermediate nonce 8".to_vec();
        let submit = |url: String, queue: bool| async move {
            let client = super::RuesClient::new(&url).unwrap();
            super::propagate_with(&client, &"ab".repeat(32), &[1, 2], queue, None).await
        };

        let (url, server) = mock_rues(vec![("/on/transactions/preverify", 400, gap.clone())]);
        let error = submit(url, false).await.unwrap_err();
        assert!(
            error.starts_with(&format!(
                "Transaction {tx_id} submission failed: Preverify rejected before propagation (400 Bad Request): "
            )) && error.contains("Missing intermediate nonce 8"),
            "{error}"
        );
        assert_eq!(server.join().unwrap().len(), 1, "nothing propagated");

        let (url, server) = mock_rues(vec![
            ("/on/transactions/preverify", 400, gap),
            ("/on/transactions/propagate", 200, Vec::new()),
        ]);
        submit(url, true).await.unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].1, vec![1, 2]);

        let invalid = b"Tx ab not accepted: this transaction is invalid".to_vec();
        let (url, server) = mock_rues(vec![("/on/transactions/preverify", 400, invalid)]);
        let error = submit(url, true).await.unwrap_err();
        assert!(
            error.starts_with(&format!(
                "Transaction {tx_id} submission failed: Preverify rejected before propagation"
            )),
            "{error}"
        );
        assert_eq!(server.join().unwrap().len(), 1, "nothing propagated");

        let (url, server) = mock_rues(vec![
            ("/on/transactions/preverify", 200, Vec::new()),
            ("/on/transactions/propagate", 500, b"lost".to_vec()),
        ]);
        let error = submit(url, true).await.unwrap_err();
        assert!(
            error.starts_with(&format!(
                "Transaction {tx_id} submission failed: Propagation outcome unknown"
            )),
            "{error}"
        );
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// The agents pass `--ack-before-send` once `call --help` lists it. It
    /// is off unless given, and waits 30 seconds.
    #[test]
    fn call_takes_ack_before_send() {
        use clap::CommandFactory;
        let contract = "11".repeat(32);
        let base = [
            "dusk-tx",
            "call",
            "--contract",
            &contract,
            "--fn-name",
            "process",
            "--expected-chain-id",
            "1",
        ];
        let acks = |extra: &[&str]| match Cli::try_parse_from(base.iter().chain(extra).copied())
            .unwrap()
            .command
        {
            super::Command::Call {
                ack_before_send, ..
            } => ack_before_send,
            _ => unreachable!("parsed call"),
        };
        assert!(!acks(&[]));
        assert!(acks(&["--ack-before-send"]));
        let help = Cli::command()
            .find_subcommand_mut("call")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(
            help.contains("--ack-before-send") && help.contains("Not submitted:"),
            "{help}"
        );
        assert_eq!(super::SEND_ACK_TIMEOUT, Duration::from_secs(30));
    }

    /// A go-ahead read from `input`, waiting at most `timeout`.
    fn ack_from(input: &'static [u8], timeout: Duration) -> super::SendAck {
        super::SendAck {
            input: Box::new(move || Box::new(input)),
            timeout,
        }
    }

    /// Input that blocks until its sender is dropped, then ends.
    struct HeldInput(std::sync::mpsc::Receiver<()>);

    impl std::io::Read for HeldInput {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            let _ = self.0.recv();
            Ok(0)
        }
    }

    /// A go-ahead that never comes, waiting at most `timeout`. Its read
    /// ends once the returned sender is dropped.
    fn held_ack(timeout: Duration) -> (super::SendAck, std::sync::mpsc::Sender<()>) {
        let (release, held) = std::sync::mpsc::channel();
        let ack = super::SendAck {
            input: Box::new(move || Box::new(std::io::BufReader::new(HeldInput(held)))),
            timeout,
        };
        (ack, release)
    }

    /// Only the line `send` is a go-ahead, as written: not another case,
    /// spacing or line ending, not a longer line, and not `send` without
    /// its newline before the end of stdin. Only the first line counts.
    #[tokio::test]
    async fn the_go_ahead_is_exactly_the_line_send() {
        let wait = |input: &'static [u8]| ack_from(input, Duration::from_secs(10)).wait();
        assert_eq!(wait(b"send\n").await, Ok(()));
        assert_eq!(wait(b"send\nanything\n").await, Ok(()));
        for closed in [&b""[..], b"send", b"sen"] {
            assert_eq!(
                wait(closed).await.unwrap_err(),
                "stdin closed before the line \"send\"",
                "{closed:?}"
            );
        }
        let long = &[b'x'; 100][..];
        for other in [
            &b"SEND\n"[..],
            b"send \n",
            b" send\n",
            b"send\r\n",
            b"sendx\n",
            b"send\0\n",
            b"\n",
            b"yes\n",
            b"anything\nsend\n",
            long,
        ] {
            assert_eq!(
                wait(other).await.unwrap_err(),
                "the line on stdin was not \"send\"",
                "{other:?}"
            );
        }

        struct Broken;
        impl std::io::Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("the pipe broke"))
            }
        }
        let broken = super::SendAck {
            input: Box::new(|| Box::new(std::io::BufReader::new(Broken))),
            timeout: Duration::from_secs(10),
        };
        assert_eq!(
            broken.wait().await.unwrap_err(),
            "reading stdin failed: the pipe broke"
        );
    }

    /// No line within the timeout is a refusal, which comes when the
    /// timeout ends, whatever the read is doing.
    #[tokio::test]
    async fn no_go_ahead_within_the_timeout_is_a_refusal() {
        let (ack, release) = held_ack(Duration::from_millis(200));
        let started = std::time::Instant::now();
        let refusal = tokio::time::timeout(Duration::from_secs(10), ack.wait())
            .await
            .expect("the wait ends with its timeout");
        assert_eq!(
            refusal.unwrap_err(),
            "no line \"send\" on stdin within 200ms"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(release);
    }

    /// With `--ack-before-send`, `call` sends its transaction only on the
    /// line `send`: then it preverifies and propagates it as without the
    /// flag. The end of stdin, another line or no line within the timeout
    /// sends nothing more: with the nonce given, the node has had the chain
    /// ID query alone, no preverify and no propagate. The error names the
    /// transaction the go-ahead was asked for and says it wasn't submitted,
    /// after the `submission failed: ` the agents read a preverify
    /// refusal's detail from.
    #[tokio::test]
    async fn call_sends_only_on_the_go_ahead() {
        use dusk_core::transfer::Transaction;
        let contract = "22".repeat(32);
        let call = |url: String, ack: super::SendAck| {
            let contract = contract.clone();
            async move {
                super::call(
                    &url,
                    None,
                    "",
                    Some("11".repeat(32)),
                    false,
                    &contract,
                    "process",
                    "0102",
                    5_000_000,
                    1,
                    7,
                    false,
                    super::CallSubmission {
                        nonce: Some(42),
                        no_wait: true,
                    },
                    Some(ack),
                )
                .await
            }
        };

        let (url, server) = mock_rues(vec![
            ("/chain_id", 200, vec![7]),
            ("/on/transactions/preverify", 200, Vec::new()),
            ("/on/transactions/propagate", 200, Vec::new()),
        ]);
        let output = call(url, ack_from(b"send\n", Duration::from_secs(10)))
            .await
            .unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3, "the chain ID, preverify and propagate");
        let sent = Transaction::from_slice(&requests[2].1).unwrap();
        let tx_id = hex::encode(sent.hash().to_bytes());
        assert_eq!(output["tx_id"], tx_id);
        assert_eq!(output["submitted"], true);

        let (held, release) = held_ack(Duration::from_millis(200));
        for (ack, reason) in [
            (
                ack_from(b"", Duration::from_secs(10)),
                "stdin closed before the line \"send\"",
            ),
            (
                ack_from(b"sned\n", Duration::from_secs(10)),
                "the line on stdin was not \"send\"",
            ),
            (held, "no line \"send\" on stdin within 200ms"),
        ] {
            // An endpoint that answers the chain ID query, then is gone: a
            // preverify would fail, with another error.
            let (url, server) = mock_rues(vec![("/chain_id", 200, vec![7])]);
            let error = tokio::time::timeout(Duration::from_secs(10), call(url, ack))
                .await
                .expect("the refusal comes by the timeout")
                .unwrap_err();
            assert_eq!(server.join().unwrap().len(), 1, "the chain ID only");
            assert_eq!(
                error,
                format!("Transaction {tx_id} submission failed: Not submitted: {reason}")
            );
        }
        drop(release);
    }

    /// With the go-ahead on the line after it, the key is read up to its
    /// newline and no further.
    #[test]
    fn a_key_line_leaves_the_rest_of_stdin_unread() {
        use std::io::Read;
        let key = "11".repeat(32);
        let mut stdin = std::io::Cursor::new(format!(" 0x{key}\r\nsend\n").into_bytes());
        assert_eq!(
            super::read_secret_key_line(&mut stdin).unwrap(),
            format!("0x{key}")
        );
        let mut rest = String::new();
        stdin.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "send\n");

        let long = format!("{}\nsend\n", "a".repeat(MAX_SECRET_KEY_STDIN_BYTES));
        assert!(
            super::read_secret_key_line(long.as_bytes())
                .unwrap_err()
                .contains("exceeds 128 bytes"),
            "a line of the most bytes is too long with its newline"
        );
        assert!(super::read_secret_key_line(&[0xff, b'\n'][..]).is_err());
    }

    /// `pay-for-gas` takes a message ID with its destination, as `payForGas`
    /// does, or a nonce to look the message up by; never both. Its gas limit
    /// defaults to the low one a top-up needs.
    #[test]
    fn pay_for_gas_takes_an_id_and_destination_or_a_nonce() {
        let igp = "11".repeat(32);
        let id = "22".repeat(32);
        let parse = |extra: &[&str]| {
            let base = [
                "dusk-tx",
                "pay-for-gas",
                "--igp",
                &igp,
                "--gas-amount",
                "50000",
            ];
            Cli::try_parse_from(base.iter().chain(extra).copied()).map(|cli| cli.command)
        };
        match parse(&[
            "--message-id",
            &id,
            "--destination",
            "11155111",
            "--expected-chain-id",
            "2",
        ])
        .unwrap()
        {
            super::Command::PayForGas {
                message_id,
                destination,
                nonce,
                gas_amount,
                max_payment,
                quote_only,
                gas_limit,
                ..
            } => {
                assert_eq!(message_id.as_deref(), Some(id.as_str()));
                assert_eq!((destination, nonce), (Some(11_155_111), None));
                assert_eq!((gas_amount, max_payment, quote_only), (50_000, None, false));
                assert_eq!(gas_limit, super::PAY_FOR_GAS_GAS_LIMIT);
            }
            _ => unreachable!("parsed pay-for-gas"),
        }
        match parse(&["--nonce", "7", "--max-payment", "9", "--quote-only"]).unwrap() {
            super::Command::PayForGas {
                message_id,
                destination,
                nonce,
                max_payment,
                quote_only,
                ..
            } => {
                assert_eq!((message_id, destination, nonce), (None, None, Some(7)));
                assert_eq!((max_payment, quote_only), (Some(9), true));
            }
            _ => unreachable!("parsed pay-for-gas"),
        }
        assert!(parse(&[
            "--nonce",
            "7",
            "--destination",
            "11155111",
            "--expected-chain-id",
            "2"
        ])
        .is_ok());
        // An ID alone could be paid for under any destination.
        assert!(parse(&["--message-id", &id]).is_err());
        assert!(parse(&["--message-id", &id, "--destination", "1", "--nonce", "7"]).is_err());
        assert!(parse(&[]).is_err());
        assert!(
            Cli::try_parse_from(["dusk-tx", "pay-for-gas", "--igp", &igp, "--nonce", "7"]).is_err()
        );
    }

    /// The arguments of every command that sends a transaction, complete
    /// but for its RUES URL, its key and `--expected-chain-id`. `wasm` is
    /// the Wasm `deploy-wasm` reads.
    fn signing_commands(wasm: &str) -> Vec<Vec<String>> {
        let id = "22".repeat(32);
        let (_, account) = crate::keys::load_from_hex(&"11".repeat(32)).unwrap();
        let account = hex::encode(account.to_bytes());
        let evm_address = "33".repeat(20);
        let commands: Vec<Vec<&str>> = vec![
            vec!["call", "--contract", &id, "--fn-name", "process"],
            vec![
                "deploy-hyperlane",
                "--default-ism",
                "testMock",
                "--igp-domain-config",
                "1:0:10000000000:1",
            ],
            vec!["deploy-wasm", "--wasm", wasm],
            vec![
                "deploy-token-route",
                "--mailbox",
                &id,
                "--token-name",
                "Dusk Euro (testnet)",
                "--token-symbol",
                "EURd",
                "--token-decimals",
                "6",
                "--token-supply",
                "1000000000000",
                "--deploy-nonce",
                "10",
            ],
            vec![
                "fund-dispatch",
                "--mailbox",
                &id,
                "--payer",
                &id,
                "--amount",
                "1",
            ],
            vec!["withdraw-dispatch", "--target", &id, "--amount", "1"],
            vec![
                "dispatch",
                "--mailbox",
                &id,
                "--test-recipient",
                &id,
                "--destination",
                "1",
                "--recipient",
                &id,
            ],
            vec!["process", "--mailbox", &id, "--message", "00"],
            vec![
                "enroll-router",
                "--warp-contract",
                &id,
                "--domain",
                "1",
                "--router",
                &id,
            ],
            vec!["claim-pending", "--warp-contract", &id],
            vec![
                "set-igp-config",
                "--igp",
                &id,
                "--igp-domain-config",
                "1:0:10000000000:1",
            ],
            vec![
                "set-beneficiary",
                "--contract",
                &id,
                "--beneficiary",
                &account,
            ],
            vec!["claim-fees", "--contract", &id],
            vec![
                "pay-for-gas",
                "--igp",
                &id,
                "--message-id",
                &id,
                "--destination",
                "1",
                "--gas-amount",
                "1",
            ],
            vec![
                "drc20-approve",
                "--token",
                &id,
                "--spender",
                &id,
                "--amount",
                "1",
            ],
            vec!["drc20-mint", "--token", &id, "--amount", "1"],
            vec![
                "transfer-remote",
                "--warp-contract",
                &id,
                "--destination",
                "1",
                "--recipient",
                &evm_address,
                "--amount",
                "1",
            ],
        ];
        commands
            .into_iter()
            .map(|args| args.into_iter().map(String::from).collect())
            .collect()
    }

    /// Every command that sends a transaction signs only for the chain the
    /// caller names, as `withdraw-dispatch` always has: none runs without
    /// `--expected-chain-id`, except `pay-for-gas --quote-only` and
    /// `deploy-token-route --plan`, which sign nothing. Every other command
    /// only reads.
    #[test]
    fn every_command_that_signs_requires_an_expected_chain_id() {
        use clap::CommandFactory;
        let parse = |args: &[String], extra: &[&str]| {
            Cli::try_parse_from(
                std::iter::once("dusk-tx")
                    .chain(args.iter().map(String::as_str))
                    .chain(extra.iter().copied()),
            )
        };
        let commands = signing_commands("contract.wasm");
        for args in &commands {
            assert!(
                parse(args, &[]).is_err(),
                "{args:?} without --expected-chain-id"
            );
            assert!(
                parse(args, &["--expected-chain-id", "2"]).is_ok(),
                "{args:?}"
            );
        }
        let pay_for_gas = commands
            .iter()
            .find(|args| args[0] == "pay-for-gas")
            .unwrap();
        assert!(parse(pay_for_gas, &["--quote-only"]).is_ok());
        let token_route = commands
            .iter()
            .find(|args| args[0] == "deploy-token-route")
            .unwrap();
        assert!(parse(token_route, &["--plan"]).is_ok());

        let signing: Vec<&str> = commands.iter().map(|args| args[0].as_str()).collect();
        let reads = [
            "query",
            "encode-message",
            "drc20-balance",
            "drc20-info",
            "dispatched-message",
            "account",
            "export-signer-key",
        ];
        for command in Cli::command().get_subcommands() {
            let name = command.get_name();
            assert!(
                signing.contains(&name) || reads.contains(&name),
                "{name} is listed neither as a command that signs nor as one that only reads"
            );
        }
    }

    /// Every command that sends a transaction, against an endpoint on
    /// another chain, stops once it has the endpoint's chain ID: before it
    /// queries a contract, opens the signer's key file or builds a
    /// transaction. The key file here doesn't exist, so reading it first
    /// would fail differently.
    #[tokio::test]
    async fn every_command_that_signs_checks_the_chain_before_anything_else() {
        let dir = std::env::temp_dir().join(format!("dusk-tx-chain-first-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wasm = dir.join("contract.wasm");
        std::fs::write(&wasm, b"\0asm\x01\0\0\0").unwrap();
        for args in signing_commands(wasm.to_str().unwrap()) {
            let (url, server) = mock_rues(vec![("/chain_id", 200, vec![1])]);
            let tail = [
                "--rues-url",
                &url,
                "--keys",
                "/nonexistent/dusk-tx-test.keys",
                "--expected-chain-id",
                "2",
            ];
            let cli = Cli::try_parse_from(
                std::iter::once("dusk-tx")
                    .chain(args.iter().map(String::as_str))
                    .chain(tail),
            )
            .unwrap();
            let error = super::run(cli.command).await.unwrap_err();
            assert!(
                error.contains("Configured Dusk chain ID 2 does not match endpoint chain ID 1"),
                "{}: {error}",
                args[0]
            );
            assert_eq!(
                server.join().unwrap().len(),
                1,
                "{} asked for nothing but the chain ID",
                args[0]
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `transfer-remote --native` tells a native route by its
    /// `state_version`, which every route answers, and never asks it for
    /// `name` or `wrapped_token`, which only DRC20 routes have: both queries
    /// failed, and the node logged each as an error. A DRC20 route is
    /// refused before the key is read.
    #[tokio::test]
    async fn native_transfers_ask_the_route_nothing_it_lacks() {
        let route = "22".repeat(32);
        let transfer = |url: &str| {
            Cli::try_parse_from([
                "dusk-tx",
                "transfer-remote",
                "--native",
                "--rues-url",
                url,
                "--keys",
                "/nonexistent/dusk-tx-test.keys",
                "--warp-contract",
                &route,
                "--destination",
                "11155111",
                "--recipient",
                &"33".repeat(20),
                "--amount",
                "1000000000",
                "--expected-chain-id",
                "2",
            ])
            .unwrap()
            .command
        };
        let (url, server) = mock_rues(vec![
            ("/chain_id", 200, vec![2]),
            ("/enrolled_router", 200, vec![0x11; 32]),
            ("/state_version", 200, super::rkyv_serialize(&5u32)),
            (
                "/quote_transfer_remote",
                200,
                super::rkyv_serialize(&6_000_000_000u64),
            ),
            ("/mailbox", 200, vec![0x44; 32]),
            ("/fee_credit", 200, super::rkyv_serialize(&0u64)),
            ("/memory_bytes", 200, super::rkyv_serialize(&(1u64 << 20))),
        ]);
        // It gets as far as the key, which doesn't exist (or its password,
        // which a test of the password's sources may hold at the time).
        let error = super::run(transfer(&url)).await.unwrap_err();
        assert!(error.to_lowercase().contains("consensus"), "{error}");
        let paths: Vec<String> = server
            .join()
            .unwrap()
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        assert_eq!(paths.len(), 7, "{paths:?}");
        assert!(
            !paths
                .iter()
                .any(|path| path.ends_with("/name") || path.ends_with("/wrapped_token")),
            "{paths:?}"
        );

        for version in [8u32, 7] {
            let (url, server) = mock_rues(vec![
                ("/chain_id", 200, vec![2]),
                ("/enrolled_router", 200, vec![0x11; 32]),
                ("/state_version", 200, super::rkyv_serialize(&version)),
            ]);
            let error = super::run(transfer(&url)).await.unwrap_err();
            assert!(error.contains("drop --native"), "{error}");
            assert_eq!(server.join().unwrap().len(), 3);
        }
    }

    /// The Mailbox takes at least 0.01 DUSK for a payer with no credit.
    /// `fund-dispatch` of less reads the payer's credit and stops before the
    /// key is read when it has none, since a failed transaction pays its
    /// whole gas limit. A payer with credit, or an amount at the minimum,
    /// goes on (here as far as the key, which doesn't exist).
    #[tokio::test]
    async fn fund_dispatch_refuses_less_than_the_minimum_for_a_new_payer() {
        let fund = |url: &str, amount: u64| {
            Cli::try_parse_from([
                "dusk-tx",
                "fund-dispatch",
                "--rues-url",
                url,
                "--keys",
                "/nonexistent/dusk-tx-test.keys",
                "--mailbox",
                &"33".repeat(32),
                "--payer",
                &"44".repeat(32),
                "--amount",
                &amount.to_string(),
                "--expected-chain-id",
                "2",
            ])
            .unwrap()
            .command
        };
        let (url, server) = mock_rues(vec![
            ("/chain_id", 200, vec![2]),
            ("/fee_credit", 200, super::rkyv_serialize(&0u64)),
        ]);
        let error = super::run(fund(&url, super::MIN_NEW_PAYER_CREDIT - 1))
            .await
            .unwrap_err();
        assert!(error.contains("takes at least 10000000 LUX"), "{error}");
        assert_eq!(server.join().unwrap().len(), 2);

        let (url, server) = mock_rues(vec![
            ("/chain_id", 200, vec![2]),
            ("/fee_credit", 200, super::rkyv_serialize(&1u64)),
        ]);
        let error = super::run(fund(&url, 1)).await.unwrap_err();
        assert!(error.to_lowercase().contains("consensus"), "{error}");
        assert_eq!(server.join().unwrap().len(), 2);

        let (url, server) = mock_rues(vec![
            ("/chain_id", 200, vec![2]),
            ("/fee_credit", 200, super::rkyv_serialize(&0u64)),
            ("/memory_bytes", 200, super::rkyv_serialize(&(1u64 << 20))),
        ]);
        let error = super::run(fund(&url, super::MIN_NEW_PAYER_CREDIT))
            .await
            .unwrap_err();
        assert!(error.to_lowercase().contains("consensus"), "{error}");
        assert_eq!(server.join().unwrap().len(), 3);
    }

    /// The Mailbox takes no new payer once a credit entry, 13 pages at
    /// most, could take its memory past 3 GiB. `fund-dispatch` for a payer
    /// with no credit, and `transfer-remote`, whose route is a new payer
    /// unless it has standing credit, read the Mailbox's memory and stop
    /// before the key is read when there is no room. A payer with credit
    /// adds no entry and goes on, and so does a transfer that pays no
    /// dispatch fee, which funds no credit.
    #[tokio::test]
    async fn new_payers_are_checked_for_room_in_the_mailbox() {
        let limit = super::NEW_ENTRY_MEMORY_LIMIT;
        let page = super::WASM_PAGE_BYTES;
        let fund = |url: &str| {
            Cli::try_parse_from([
                "dusk-tx",
                "fund-dispatch",
                "--rues-url",
                url,
                "--keys",
                "/nonexistent/dusk-tx-test.keys",
                "--mailbox",
                &"33".repeat(32),
                "--payer",
                &"44".repeat(32),
                "--amount",
                &super::MIN_NEW_PAYER_CREDIT.to_string(),
                "--expected-chain-id",
                "2",
            ])
            .unwrap()
            .command
        };
        let transfer = |url: &str| {
            Cli::try_parse_from([
                "dusk-tx",
                "transfer-remote",
                "--native",
                "--rues-url",
                url,
                "--keys",
                "/nonexistent/dusk-tx-test.keys",
                "--warp-contract",
                &"22".repeat(32),
                "--destination",
                "11155111",
                "--recipient",
                &"33".repeat(20),
                "--amount",
                "1000000000",
                "--expected-chain-id",
                "2",
            ])
            .unwrap()
            .command
        };
        let route_replies = |credit: u64, memory: Option<u64>| {
            let mut replies = vec![
                ("/chain_id", 200, vec![2]),
                ("/enrolled_router", 200, vec![0x11; 32]),
                ("/state_version", 200, super::rkyv_serialize(&5u32)),
                (
                    "/quote_transfer_remote",
                    200,
                    super::rkyv_serialize(&6_000_000_000u64),
                ),
                ("/mailbox", 200, vec![0x44; 32]),
                ("/fee_credit", 200, super::rkyv_serialize(&credit)),
            ];
            if let Some(memory) = memory {
                replies.push(("/memory_bytes", 200, super::rkyv_serialize(&memory)));
            }
            replies
        };

        // At 12 pages below the limit a new payer's 13 don't fit.
        for (command, replies) in [
            (
                fund as fn(&str) -> super::Command,
                vec![
                    ("/chain_id", 200, vec![2]),
                    ("/fee_credit", 200, super::rkyv_serialize(&0u64)),
                    (
                        "/memory_bytes",
                        200,
                        super::rkyv_serialize(&(limit - 12 * page)),
                    ),
                ],
            ),
            (
                transfer as fn(&str) -> super::Command,
                route_replies(0, Some(limit - 12 * page)),
            ),
        ] {
            let count = replies.len();
            let (url, server) = mock_rues(replies);
            let error = super::run(command(&url)).await.unwrap_err();
            assert!(error.contains("within 13 pages of 3 GiB"), "{error}");
            assert_eq!(server.join().unwrap().len(), count);
        }

        // At 13 pages below, they do: it goes on to the key.
        let (url, server) = mock_rues(vec![
            ("/chain_id", 200, vec![2]),
            ("/fee_credit", 200, super::rkyv_serialize(&0u64)),
            (
                "/memory_bytes",
                200,
                super::rkyv_serialize(&(limit - 13 * page)),
            ),
        ]);
        let error = super::run(fund(&url)).await.unwrap_err();
        assert!(error.to_lowercase().contains("consensus"), "{error}");
        assert_eq!(server.join().unwrap().len(), 3);

        // A payer, or a route, with credit isn't checked.
        let (url, server) = mock_rues(vec![
            ("/chain_id", 200, vec![2]),
            ("/fee_credit", 200, super::rkyv_serialize(&1u64)),
        ]);
        let error = super::run(fund(&url)).await.unwrap_err();
        assert!(error.to_lowercase().contains("consensus"), "{error}");
        assert_eq!(server.join().unwrap().len(), 2);
        let (url, server) = mock_rues(route_replies(5, None));
        let error = super::run(transfer(&url)).await.unwrap_err();
        assert!(error.to_lowercase().contains("consensus"), "{error}");
        assert_eq!(server.join().unwrap().len(), 6);

        // Nor is a route whose transfer pays no dispatch fee: it funds no
        // credit and adds no entry, so whatever the route's credit and the
        // Mailbox's memory, it goes on to the key without reading them.
        let mut free = route_replies(0, None);
        free.truncate(3);
        free.push(("/quote_transfer_remote", 200, super::rkyv_serialize(&0u64)));
        let (url, server) = mock_rues(free);
        let error = super::run(transfer(&url)).await.unwrap_err();
        assert!(error.to_lowercase().contains("consensus"), "{error}");
        assert_eq!(server.join().unwrap().len(), 4);
    }

    /// A view's replies as a Rusk node gives them for a contract that
    /// doesn't export it, with the contract's `control` returning the
    /// archived amount given after each, as many times as `optional_view`
    /// asks before it takes the view as missing.
    fn missing_view(view: &'static str, control: (&'static str, Vec<u8>)) -> MockReplies {
        let failed = (view, 500, crate::rues::FAILED_QUERY_BODY.to_vec());
        vec![
            failed.clone(),
            (control.0, 200, control.1.clone()),
            failed,
            (control.0, 200, control.1),
        ]
    }

    /// Replies to a view that don't show the contract lacks it: another
    /// failure than the VM's, an answer that doesn't decode, no answer, or
    /// the VM's failure and then a control that fails too, or answers
    /// without an amount, at the first try or the second. Each with what
    /// the error says.
    fn unreadable_view(
        view: &'static str,
        control: &'static str,
    ) -> Vec<(MockReplies, &'static str)> {
        let failed = crate::rues::FAILED_QUERY_BODY.to_vec();
        let no_amount = "answered, but not with an amount";
        let mut cases = vec![
            (
                vec![(view, 500, Vec::new())],
                "failed (500 Internal Server Error)",
            ),
            (
                vec![(view, 500, b"upstream error".to_vec())],
                "upstream error",
            ),
            (
                vec![(view, 502, failed.clone())],
                "failed (502 Bad Gateway)",
            ),
            (
                vec![(view, 200, vec![1, 2, 3])],
                "answered, but not with the expected value",
            ),
            (
                vec![(view, 204, Vec::new())],
                "answered, but not with the expected value",
            ),
            (vec![(view, 0, Vec::new())], "HTTP error"),
            (
                vec![(view, 500, failed.clone()), (control, 500, failed.clone())],
                "and so did",
            ),
        ];
        // Bodies longer than an amount whose last 8 bytes would decode as
        // one: text, and an amount behind 8 bytes of anything.
        let padded = [vec![0xff; 8], super::rkyv_serialize(&0u64)].concat();
        for (status, body) in [
            (200, Vec::new()),
            (200, vec![1, 2, 3]),
            (204, Vec::new()),
            (200, b"upstream failure".to_vec()),
            (200, padded),
        ] {
            cases.push((
                vec![(view, 500, failed.clone()), (control, status, body.clone())],
                no_amount,
            ));
            cases.push((
                vec![
                    (view, 500, failed.clone()),
                    (control, 200, super::rkyv_serialize(&0u64)),
                    (view, 500, failed.clone()),
                    (control, status, body),
                ],
                no_amount,
            ));
        }
        cases
    }

    /// A WarpDrc20 holds an owner to 64 allowances and takes no allowance
    /// for a new spender once one, 14 pages at most, could take its memory
    /// past 3 GiB. `drc20-approve` of a new spender checks both before it
    /// signs; an existing spender, or a token that doesn't count
    /// allowances, isn't checked. A token counts as without the count only
    /// as the node reports a missing view, twice, with the token's
    /// `allowance` returning an amount after each: any other reply, or
    /// none, stops the approval before it is signed.
    #[tokio::test]
    async fn drc20_approve_checks_the_cap_and_room_for_a_new_spender() {
        let limit = super::NEW_ENTRY_MEMORY_LIMIT;
        let page = super::WASM_PAGE_BYTES;
        let approve = |url: &str| {
            Cli::try_parse_from([
                "dusk-tx",
                "drc20-approve",
                "--rues-url",
                url,
                "--secret-key",
                &"11".repeat(32),
                "--token",
                &"22".repeat(32),
                "--spender",
                &"33".repeat(32),
                "--amount",
                "5",
                "--expected-chain-id",
                "2",
            ])
            .unwrap()
            .command
        };
        let replies = |allowance: u64, count: Option<u32>, memory: Option<u64>| {
            let mut replies = vec![
                ("/chain_id", 200, vec![2]),
                ("/allowance", 200, super::rkyv_serialize(&allowance)),
            ];
            match count {
                Some(count) => {
                    replies.push(("/allowance_count", 200, super::rkyv_serialize(&count)))
                }
                None if allowance == 0 => replies.extend(missing_view(
                    "/allowance_count",
                    ("/allowance", super::rkyv_serialize(&allowance)),
                )),
                None => {}
            }
            if let Some(memory) = memory {
                replies.push(("/memory_bytes", 200, super::rkyv_serialize(&memory)));
            }
            replies
        };
        let refused = |replies: Vec<(&'static str, u16, Vec<u8>)>, why: &'static str| async move {
            let count = replies.len();
            let (url, server) = mock_rues(replies);
            let error = super::run(approve(&url)).await.unwrap_err();
            assert!(error.contains(why), "{error}");
            assert_eq!(server.join().unwrap().len(), count);
        };
        // Past the cap, and without room.
        refused(replies(0, Some(64), None), "already holds 64 allowances").await;
        refused(
            replies(0, Some(3), Some(limit - 13 * page)),
            "within 14 pages of 3 GiB",
        )
        .await;

        // Room, an existing spender, or a token without the count: it goes
        // on to read the signer's account, which the mock doesn't answer.
        for replies in [
            replies(0, Some(63), Some(limit - 14 * page)),
            replies(1, None, None),
            replies(0, None, None),
        ] {
            let count = replies.len();
            let (url, server) = mock_rues(replies);
            let error = super::run(approve(&url)).await.unwrap_err();
            assert!(
                !error.contains("allowances") && !error.contains("3 GiB"),
                "{error}"
            );
            assert_eq!(server.join().unwrap().len(), count);
        }

        // A reply that doesn't show the token lacks the count stops it
        // before it signs: here before the signer's account is read.
        let mut generic = replies(0, Some(0), None);
        generic[2] = ("/allowance_count", 500, Vec::new());
        refused(generic, "Can't tell whether the token limits allowances").await;
        let owner = super::Drc20Account::Moonlight(
            [0x66; hyperlane_dusk_types::drc20::BLS_PUBLIC_KEY_BYTES],
        );
        let spender = super::Drc20Account::Contract(super::ContractId::from_bytes([0x33; 32]));
        for (view, why) in unreadable_view("/allowance_count", "/allowance") {
            let mut replies = vec![("/allowance", 200, super::rkyv_serialize(&0u64))];
            replies.extend(view);
            let count = replies.len();
            let (url, server) = mock_rues(replies);
            let client =
                crate::rues::RuesClient::with_test_timeout(&url, Duration::from_millis(500));
            let error = super::check_room_for_new_spender(&client, &[0x22; 32], owner, spender)
                .await
                .unwrap_err();
            assert!(
                error.contains("Can't tell whether the token limits allowances"),
                "{error}"
            );
            assert!(error.contains(why), "{why}: {error}");
            assert_eq!(server.join().unwrap().len(), count);
        }
    }

    /// A collateral route pays out with its token's `transfer`, and a
    /// WarpDrc20 refuses that to new holders past its 3 GiB reserve, so
    /// `deploy-hyperlane` won't wrap one: not `warp-drc20`, refused before
    /// anything is asked of the node, and not a token ID that turns out to
    /// be a warp route. A token counts as no route only as the node reports
    /// a missing `mailbox`, twice, with the token's `balance_of` returning
    /// a balance after each: any other reply, or none, stops the deploy.
    #[tokio::test]
    async fn collateral_routes_dont_wrap_a_warp_drc20() {
        let deploy = |url: &str, token: &str| {
            Cli::try_parse_from([
                "dusk-tx",
                "deploy-hyperlane",
                "--rues-url",
                url,
                "--default-ism",
                "testMock",
                "--igp-domain-config",
                "1:0:10000000000:1",
                "--deploy-warp-drc20",
                "--warp-collateral-token",
                token,
                "--expected-chain-id",
                "2",
            ])
            .unwrap()
            .command
        };
        let (url, server) = mock_rues(Vec::new());
        let error = super::run(deploy(&url, "warp-drc20")).await.unwrap_err();
        assert!(error.contains("warp-drc20 is refused"), "{error}");
        assert!(error.contains("3 GiB"), "{error}");
        assert!(server.join().unwrap().is_empty());

        let token = super::ContractId::from_bytes([0x55; 32]);
        let deployer = super::Drc20Account::Moonlight(
            [0x66; hyperlane_dusk_types::drc20::BLS_PUBLIC_KEY_BYTES],
        );
        let balance = || ("/balance_of", 200, super::rkyv_serialize(&0u64));
        let checked = |mailbox: MockReplies| {
            let mut replies = vec![("/owner", 200, vec![0x77; 32]), balance()];
            replies.extend(mailbox);
            let count = replies.len();
            let (url, server) = mock_rues(replies);
            let client =
                crate::rues::RuesClient::with_test_timeout(&url, Duration::from_millis(500));
            (client, server, count)
        };
        // A route answers `mailbox`, at once or once the node runs it.
        let failed = crate::rues::FAILED_QUERY_BODY.to_vec();
        for mailbox in [
            vec![("/mailbox", 200, vec![0x44; 32])],
            vec![
                ("/mailbox", 500, failed.clone()),
                balance(),
                ("/mailbox", 200, vec![0x44; 32]),
            ],
        ] {
            let (client, server, count) = checked(mailbox);
            let error = super::check_explicit_collateral_token(&client, token, deployer)
                .await
                .unwrap_err();
            assert!(error.contains("is a warp route"), "{error}");
            assert_eq!(server.join().unwrap().len(), count);
        }
        // A plain DRC20 has none, as the node reports a missing view.
        let (client, server, count) = checked(missing_view(
            "/mailbox",
            ("/balance_of", super::rkyv_serialize(&0u64)),
        ));
        super::check_explicit_collateral_token(&client, token, deployer)
            .await
            .expect("a plain DRC20 is wrapped");
        assert_eq!(server.join().unwrap().len(), count);
        // A reply that doesn't show the token has no `mailbox` stops it.
        for (mailbox, why) in unreadable_view("/mailbox", "/balance_of") {
            let (client, server, count) = checked(mailbox);
            let error = super::check_explicit_collateral_token(&client, token, deployer)
                .await
                .unwrap_err();
            assert!(
                error.contains("Can't tell whether the explicit collateral token"),
                "{error}"
            );
            assert!(error.contains(why), "{why}: {error}");
            assert_eq!(server.join().unwrap().len(), count);
        }
    }

    /// Multicall for Dusk's published owner, and the IDs a test Wasm gives
    /// with it: BLAKE2b-256(Wasm, nonce as 8 bytes LE, owner key), worked out
    /// independently for nonces 0 and 7.
    const MULTICALL_OWNER: &str = "r2h82mj2ehNhYPphK3GHVpG98MhSsZNCAF9sdSS4nwQJKqVBwihZn7bZXb9VoWVSqtu6onZ8Tkm11zdnavdCfuCD1kS2We4VPxvLeDf73a1sGiG25HUFvCwQJm3Ud8q6KLW";
    const ID_0: &str = "01f65d8939a6c50847df38fe73558052aa8e3f6aea2666add3f82bc279ff36e2";
    const ID_7: &str = "497ea159f075a1ffade355beddc1fbe3f153fb19c81ffe057a15c1143e018a74";

    /// A `deploy-wasm` of a test Wasm with Multicall for Dusk's owner and
    /// deploy nonce 0, expecting `expected`, against `url`, on chain 2.
    fn deploy_wasm(
        wasm: &std::path::Path,
        url: &str,
        expected: &str,
        extra: &[&str],
    ) -> super::Command {
        let wasm = wasm.to_str().unwrap();
        let args = [
            "dusk-tx",
            "deploy-wasm",
            "--rues-url",
            url,
            "--wasm",
            wasm,
            "--owner",
            MULTICALL_OWNER,
            "--deploy-nonce",
            "0",
            "--expected-id",
            expected,
            "--expected-chain-id",
            "2",
        ];
        Cli::try_parse_from(args.iter().chain(extra).copied())
            .unwrap()
            .command
    }

    fn test_wasm(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("dusk-tx-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wasm = dir.join("contract.wasm");
        std::fs::write(&wasm, b"\0asm\x01\0\0\0").unwrap();
        (dir, wasm)
    }

    /// `deploy-wasm` computes the contract ID as Rusk does, from the Wasm,
    /// the deploy nonce and the owner, which need not be the signer; with
    /// the owner given it checks the ID against `--expected-id` before it
    /// asks the node anything.
    #[tokio::test]
    async fn deploy_wasm_checks_the_contract_id_before_anything_is_sent() {
        let bytecode = b"\0asm\x01\0\0\0".to_vec();
        let owner = super::parse_account_arg(MULTICALL_OWNER).unwrap();
        let id_0 = super::parse_bytes32(ID_0).unwrap();
        assert_eq!(
            super::check_deploy_id(&bytecode, 0, &owner, &id_0),
            Ok(id_0)
        );
        let error = super::check_deploy_id(&bytecode, 7, &owner, &id_0).unwrap_err();
        assert!(
            error.contains(&format!("give the contract ID {ID_7}, not {ID_0}")),
            "{error}"
        );

        let (dir, wasm) = test_wasm("deploy-wasm-id");
        let (url, server) = mock_rues(vec![]);
        let error = super::run(deploy_wasm(&wasm, &url, ID_7, &[]))
            .await
            .unwrap_err();
        assert!(error.contains("nothing was sent"), "{error}");
        assert!(server.join().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(super::deploy_wasm_gas_limit(50_745), 15_074_500);
    }

    /// An existing contract at the expected ID is not trusted without
    /// verification. It is taken, without a key being read (the key file
    /// here doesn't exist), only when Rusk reports exactly the given owner
    /// for it and, with `--check-multicall`, it answers `version` with 1 and
    /// an `aggregate` of the transfer contract's `chain_id` with the chain's
    /// ID. Otherwise the command fails and sends nothing.
    #[tokio::test]
    async fn deploy_wasm_takes_an_existing_contract_only_once_it_checks_out() {
        let (dir, wasm) = test_wasm("deploy-wasm-existing");
        let owner = super::parse_account_arg(MULTICALL_OWNER)
            .unwrap()
            .to_bytes()
            .to_vec();
        let (_, other) = crate::keys::load_from_hex(&"11".repeat(32)).unwrap();
        let other = other.to_bytes().to_vec();
        let version = |v: u32| super::rkyv_serialize(&v);
        let aggregate = |results: Vec<Result<Vec<u8>, super::multicall::CallError>>| {
            super::rkyv_serialize(&super::multicall::Aggregate { height: 9, results })
        };
        let no_keys = ["--keys", "/nonexistent/dusk-tx-test.keys"];
        let check = [
            "--keys",
            "/nonexistent/dusk-tx-test.keys",
            "--check-multicall",
        ];

        // Taken: Rusk reports the owner given.
        let (url, server) = mock_rues(vec![
            ("/chain_id", 200, vec![2]),
            ("/owner", 200, owner.clone()),
        ]);
        super::run(deploy_wasm(&wasm, &url, ID_0, &no_keys))
            .await
            .unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].0.contains(ID_0), "{}", requests[1].0);

        // And with --check-multicall, once it answers as the multicall.
        let (url, server) = mock_rues(vec![
            ("/chain_id", 200, vec![2]),
            ("/owner", 200, owner.clone()),
            ("/version", 200, version(1)),
            ("/aggregate", 200, aggregate(vec![Ok(vec![2])])),
        ]);
        super::run(deploy_wasm(&wasm, &url, ID_0, &check))
            .await
            .unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 4);
        assert_eq!(
            requests[3].1,
            super::rkyv_serialize(&super::multicall::chain_id_calls())
        );

        // Refused, with nothing sent and no key read.
        type Replies = Vec<(&'static str, u16, Vec<u8>)>;
        let refusals: Vec<(Replies, &[&str], &str)> = vec![
            (
                vec![("/owner", 200, other.clone())],
                &no_keys,
                "Rusk reports its owner as",
            ),
            (
                vec![("/owner", 200, owner[..95].to_vec())],
                &no_keys,
                "Rusk reports its owner as 95 bytes",
            ),
            (
                vec![("/owner", 200, [owner.as_slice(), &[0]].concat())],
                &no_keys,
                "Rusk reports its owner as 97 bytes",
            ),
            (
                vec![("/owner", 500, b"node busy".to_vec())],
                &no_keys,
                "Can't tell whether a contract is at",
            ),
            (
                vec![
                    ("/owner", 200, owner.clone()),
                    ("/version", 200, version(2)),
                ],
                &check,
                "it answers version 2, not 1",
            ),
            (
                vec![
                    ("/owner", 200, owner.clone()),
                    ("/version", 200, version(1)),
                    ("/aggregate", 200, aggregate(vec![Ok(vec![3])])),
                ],
                &check,
                "aggregate of the transfer contract's chain_id answered",
            ),
            (
                vec![
                    ("/owner", 200, owner.clone()),
                    ("/version", 200, version(1)),
                    ("/aggregate", 500, b"no such function".to_vec()),
                ],
                &check,
                "its aggregate query failed",
            ),
        ];
        for (replies, extra, expected) in refusals {
            let (url, server) = mock_rues(
                std::iter::once(("/chain_id", 200, vec![2]))
                    .chain(replies)
                    .collect(),
            );
            let error = super::run(deploy_wasm(&wasm, &url, ID_0, extra))
                .await
                .unwrap_err();
            assert!(error.contains(expected), "{expected}: {error}");
            assert!(
                error.contains("nothing was sent") || error.contains("Nothing was sent"),
                "{error}"
            );
            let requests = server.join().unwrap();
            assert!(
                !requests
                    .iter()
                    .any(|(path, _)| path.contains("/transactions/")),
                "{requests:?}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A contract `deploy-wasm` deploys is checked as an existing one is.
    #[tokio::test]
    async fn deploy_wasm_checks_the_contract_it_deploys() {
        use dusk_core::transfer::moonlight::AccountData;
        let (dir, wasm) = test_wasm("deploy-wasm-new");
        let owner = super::parse_account_arg(MULTICALL_OWNER)
            .unwrap()
            .to_bytes()
            .to_vec();
        let account = super::rkyv_serialize(&AccountData {
            nonce: 0,
            balance: 1_000_000_000_000_000,
        });
        let executed =
            br#"{"data":{"tx":{"err":null,"gasSpent":6899157,"blockHeight":9}}}"#.to_vec();
        let accepted =
            br#"{"data":{"block":{"header":{"hash":"ab"},"status":"Accepted"}}}"#.to_vec();
        let signer = ["--secret-key", &"11".repeat(32), "--check-multicall"];
        let deploy = |answer: u8| {
            vec![
                ("/chain_id", 200, vec![2]),
                ("/owner", 404, b"Contract owner not found".to_vec()),
                ("/account", 200, account.clone()),
                ("/on/transactions/preverify", 200, Vec::new()),
                ("/on/transactions/propagate", 200, Vec::new()),
                ("/graphql", 200, executed.clone()),
                ("/graphql", 200, accepted.clone()),
                ("/owner", 200, owner.clone()),
                ("/version", 200, super::rkyv_serialize(&1u32)),
                (
                    "/aggregate",
                    200,
                    super::rkyv_serialize(&super::multicall::Aggregate {
                        height: 9,
                        results: vec![Ok(vec![answer])],
                    }),
                ),
            ]
        };
        let (url, server) = mock_rues(deploy(2));
        super::run(deploy_wasm(&wasm, &url, ID_0, &signer))
            .await
            .unwrap();
        assert_eq!(server.join().unwrap().len(), 10);

        let (url, server) = mock_rues(deploy(3));
        let error = super::run(deploy_wasm(&wasm, &url, ID_0, &signer))
            .await
            .unwrap_err();
        assert!(
            error.contains("was included, but the contract at")
                && error.contains("doesn't answer as Multicall for Dusk"),
            "{error}"
        );
        assert_eq!(server.join().unwrap().len(), 10);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `deploy-hyperlane` as `deploy/testnet.sh` runs it, against a node that
    /// takes every deploy: the read-back is the last thing it asks, and
    /// replies that differ from what it sent make it fail instead of
    /// reporting the deployment. `tamper` changes the reply to one view of
    /// one contract. Returns the command's result and the requests made.
    async fn deploy_hyperlane_with(
        tamper: Option<(&'static str, &'static str, super::deployment::Value)>,
    ) -> (Result<(), String>, MockRequests) {
        use dusk_core::transfer::moonlight::AccountData;
        let dir = std::env::temp_dir().join(format!(
            "dusk-tx-deploy-hyperlane-{}-{}",
            std::process::id(),
            tamper.is_some()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // In the order deploy-hyperlane deploys them, which gives each its
        // deploy nonce.
        let names = [
            "test_mock",
            "test_recipient",
            "merkle_tree_hook",
            "ism_multisig",
            "protocol_fee",
            "aggregation_hook",
            "mailbox",
            "validator_announce",
            "igp",
            "warp_native",
        ];
        let secret = "11".repeat(32);
        let (_, pk) = crate::keys::load_from_hex(&secret).unwrap();
        let pk_bytes = pk.to_bytes();
        let mut ids = std::collections::HashMap::new();
        for (nonce, name) in names.iter().enumerate() {
            let wasm = [b"\0asm\x01\0\0\0".as_slice(), name.as_bytes()].concat();
            std::fs::write(dir.join(format!("hyperlane_dusk_{name}.wasm")), &wasm).unwrap();
            ids.insert(
                *name,
                super::gen_contract_id(&wasm, nonce as u64, &pk_bytes),
            );
        }
        let (a, b) = (super::EthAddress([0x22; 20]), super::EthAddress([0x11; 20]));
        let pricing = super::DomainGasConfig {
            gas_overhead: 110_000,
            token_exchange_rate: 375_000,
            gas_price: 1_000_000_000,
        };
        let sent = super::deployment::Sent {
            owner_key: pk_bytes.to_vec(),
            owner: hyperlane_dusk_types::message::keccak256(&pk_bytes),
            domain: 1_146_442_572,
            test_mock: ids["test_mock"],
            test_recipient: ids["test_recipient"],
            merkle_tree_hook: ids["merkle_tree_hook"],
            ism: Some((ids["ism_multisig"], vec![b, a], 2)),
            protocol_fee: (ids["protocol_fee"], 1_000_000, 100_000_000),
            aggregation_hook: ids["aggregation_hook"],
            mailbox: ids["mailbox"],
            validator_announce: ids["validator_announce"],
            igp: (ids["igp"], vec![(11_155_111, pricing)]),
            warp_drc20: None,
            warp_native: Some(ids["warp_native"]),
            canonical_token: None,
            warp_collateral: None,
        };

        let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
        let executed =
            br#"{"data":{"tx":{"err":null,"gasSpent":18700000,"blockHeight":9}}}"#.to_vec();
        let accepted =
            br#"{"data":{"block":{"header":{"hash":"ab"},"status":"Accepted"}}}"#.to_vec();
        let mut replies = vec![
            ("/chain_id", 200, vec![2]),
            (
                "/account",
                200,
                super::rkyv_serialize(&AccountData {
                    nonce: 0,
                    balance: 1_000_000_000_000_000,
                }),
            ),
        ];
        // No contract has any of the IDs yet.
        for name in [
            "test_mock",
            "test_recipient",
            "merkle_tree_hook",
            "ism_multisig",
            "mailbox",
            "validator_announce",
            "protocol_fee",
            "aggregation_hook",
            "igp",
            "warp_native",
        ] {
            let id = hex::encode(ids[name].to_bytes());
            replies.push((
                leak(format!("{id}/owner")),
                404,
                b"Contract owner not found".to_vec(),
            ));
        }
        for _ in names {
            replies.extend([
                ("/on/transactions/preverify", 200, Vec::new()),
                ("/on/transactions/propagate", 200, Vec::new()),
                ("/graphql", 200, executed.clone()),
            ]);
        }
        replies.push(("/graphql", 200, accepted));
        // Then the read-back: the owners Rusk records, then each view.
        for (_, contract) in sent.contracts() {
            let id = hex::encode(contract.to_bytes());
            replies.push((leak(format!("{id}/owner")), 200, pk_bytes.to_vec()));
        }
        for check in super::deployment::checks(&sent) {
            let mut reply = check.expected.encode();
            if let Some((name, method, value)) = &tamper {
                if check.name == *name && check.method == *method {
                    reply = value.encode();
                }
            }
            let path = format!("{}/{}", hex::encode(check.contract), check.method);
            replies.push((leak(path), 200, reply));
        }

        let (url, server) = mock_rues(replies);
        let command = Cli::try_parse_from([
            "dusk-tx",
            "deploy-hyperlane",
            "--rues-url",
            &url,
            "--secret-key",
            &secret,
            "--domain",
            "1146442572",
            "--wasm-dir",
            dir.to_str().unwrap(),
            "--deploy-warp-native",
            "--default-ism",
            "messageIdMultisig",
            "--multisig-validators",
            &format!("0x{},0x{}", hex::encode(a.0), hex::encode(b.0)),
            "--multisig-threshold",
            "2",
            "--igp-domain-config",
            "11155111:110000:375000:1000000000",
            "--expected-chain-id",
            "2",
        ])
        .unwrap()
        .command;
        let result = super::run(command).await;
        let requests = server.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        (result, requests)
    }

    /// An honest deployment is read back whole before it is reported, and
    /// one whose Mailbox someone else initialized, or whose route came with
    /// a router enrolled, is not reported at all.
    #[tokio::test]
    async fn deploy_hyperlane_reports_only_a_deployment_that_reads_back_as_sent() {
        use super::deployment::Value;
        let (result, requests) = deploy_hyperlane_with(None).await;
        result.unwrap();
        // 2 + 10 + 30 + 1 before the read-back: its 10 owners and 40 views.
        assert_eq!(requests.len(), 93, "{requests:?}");
        assert!(requests[43..]
            .iter()
            .all(|(path, _)| !path.contains("/transactions/")));

        let attacker = [0xee; 32];
        for (name, method, value, expected) in [
            (
                "Mailbox",
                "owner",
                Value::Owner(Some(attacker)),
                "owner is the account with hash eeee",
            ),
            (
                "WarpNative",
                "enrolled_router",
                Value::Bytes32(attacker),
                "enrolled_router(1146442572) is eeee",
            ),
            (
                "IsmMultisig",
                "validators_and_threshold",
                Value::Validators(vec![super::EthAddress([0x11; 20])], 1),
                "validators_and_threshold is 1 of [0x1111",
            ),
            (
                "IGP",
                "beneficiary",
                Value::Bytes32(attacker),
                "beneficiary is eeee",
            ),
        ] {
            let (result, requests) = deploy_hyperlane_with(Some((name, method, value))).await;
            let error = result.unwrap_err();
            assert!(
                error.contains("reports none of them as deployed"),
                "{error}"
            );
            assert!(error.contains(&format!("{name} ")), "{error}");
            assert!(error.contains(expected), "{expected}: {error}");
            assert_eq!(requests.len(), 93);
        }
    }

    /// Stand-in Wasms for a token route, in a directory of their own.
    fn token_route_wasms(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dusk-tx-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(super::CANONICAL_DRC20_WASM),
            b"\0asm\x01\0\0\0token",
        )
        .unwrap();
        std::fs::write(
            dir.join("hyperlane_dusk_warp_drc20_collateral.wasm"),
            b"\0asm\x01\0\0\0route",
        )
        .unwrap();
        dir
    }

    /// `deploy-token-route` as `deploy/testnet.sh` runs it for EURd: the
    /// Mailbox `a1…`, nonce 10, Sepolia as the remote domain.
    fn token_route_args(url: &str, dir: &std::path::Path, extra: &[&str]) -> super::TokenRouteArgs {
        let secret = "11".repeat(32);
        let mailbox = "a1".repeat(32);
        let args = [
            "dusk-tx",
            "deploy-token-route",
            "--rues-url",
            url,
            "--secret-key",
            &secret,
            "--mailbox",
            &mailbox,
            "--token-name",
            "Dusk Euro (testnet)",
            "--token-symbol",
            "EURd",
            "--token-decimals",
            "6",
            "--token-supply",
            "1000000000000",
            "--deploy-nonce",
            "10",
            "--remote-domain",
            "11155111",
            "--wasm-dir",
            dir.to_str().unwrap(),
        ];
        match Cli::try_parse_from(args.iter().chain(extra).copied())
            .unwrap()
            .command
        {
            super::Command::DeployTokenRoute(args) => args,
            _ => unreachable!("parsed deploy-token-route"),
        }
    }

    /// The token route of `token_route_args`, as an honest node holds it.
    fn token_route_sent(dir: &std::path::Path) -> super::deployment::TokenRoute {
        let (_, pk) = crate::keys::load_from_hex(&"11".repeat(32)).unwrap();
        let token_wasm = std::fs::read(dir.join(super::CANONICAL_DRC20_WASM)).unwrap();
        let route_wasm =
            std::fs::read(dir.join("hyperlane_dusk_warp_drc20_collateral.wasm")).unwrap();
        let (token, route) = super::token_route_ids(&token_wasm, &route_wasm, 10, &pk).unwrap();
        let settings = super::CollateralTokenSettings {
            name: "Dusk Euro (testnet)".into(),
            symbol: "EURd".into(),
            decimals: 6,
            supply: 1_000_000_000_000,
        };
        super::deployment::TokenRoute {
            owner_key: pk.to_bytes().to_vec(),
            owner: hyperlane_dusk_types::message::keccak256(&pk.to_bytes()),
            mailbox: super::ContractId::from_bytes([0xa1; 32]),
            router_domains: vec![1_146_442_572, 11_155_111],
            token: settings.sent(token, super::Drc20Account::moonlight(&pk)),
            route,
        }
    }

    /// The replies of a node that takes the token route's deploys, with the
    /// token and the route already there as `existing` says, and the
    /// read-back's replies, one of them tampered with.
    fn token_route_replies(
        sent: &super::deployment::TokenRoute,
        existing: (bool, bool),
        tamper: Option<(&'static str, &'static str, super::deployment::Value)>,
    ) -> MockReplies {
        use super::deployment::Deployed;
        use dusk_core::transfer::moonlight::AccountData;
        let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
        let owner = |exists: bool, id: super::ContractId| {
            let path = leak(format!("{}/owner", hex::encode(id.to_bytes())));
            if exists {
                (path, 200, sent.owner_key.clone())
            } else {
                (path, 404, b"Contract owner not found".to_vec())
            }
        };
        let mut replies = vec![
            ("/chain_id", 200, vec![2]),
            owner(existing.0, sent.token.id),
            owner(existing.1, sent.route),
            (
                "/local_domain",
                200,
                super::rkyv_serialize(&1_146_442_572u32),
            ),
        ];
        // A contract that is there was created by the transaction recorded
        // for it (token_route_recorded), in a final block; one that isn't,
        // has none.
        for (there, id) in [(existing.0, sent.token.id), (existing.1, sent.route)] {
            if there {
                replies.extend(recorded_deploy_replies(owner(true, id)));
            } else {
                replies.push(owner(false, id));
            }
        }
        let deploys = usize::from(!existing.0) + usize::from(!existing.1);
        if deploys > 0 {
            replies.push((
                "/account",
                200,
                super::rkyv_serialize(&AccountData {
                    nonce: 4,
                    balance: 1_000_000_000_000_000,
                }),
            ));
            for _ in 0..deploys {
                replies.extend([
                    ("/on/transactions/preverify", 200, Vec::new()),
                    ("/on/transactions/propagate", 200, Vec::new()),
                    (
                        "/graphql",
                        200,
                        br#"{"data":{"tx":{"err":null,"gasSpent":26400000,"blockHeight":9}}}"#
                            .to_vec(),
                    ),
                ]);
            }
            replies.push((
                "/graphql",
                200,
                br#"{"data":{"block":{"header":{"hash":"ab"},"status":"Accepted"}}}"#.to_vec(),
            ));
        }
        for (_, contract) in sent.contracts() {
            replies.push(owner(true, contract));
        }
        for check in sent.checks() {
            let mut reply = check.expected.encode();
            if let Some((name, method, value)) = &tamper {
                if check.name == *name && check.method == *method {
                    reply = value.encode();
                }
            }
            replies.push((
                leak(format!("{}/{}", hex::encode(check.contract), check.method)),
                200,
                reply,
            ));
        }
        replies
    }

    /// A fence's replies at block "aa": the signer's nonce is `nonce`.
    fn fence_replies(nonce: u64) -> MockReplies {
        fence_replies_at(nonce, "aa")
    }

    /// A fence's replies at the tip block 20 with `hash`: the tip, the
    /// signer's account, with `nonce`, and the same tip again.
    fn fence_replies_at(nonce: u64, hash: &str) -> MockReplies {
        vec![tip_reply(hash), account_reply(nonce), tip_reply(hash)]
    }

    /// The node's tip: block 20, with `hash`.
    fn tip_reply(hash: &str) -> (&'static str, u16, Vec<u8>) {
        let tip = format!(r#"{{"data":{{"block":{{"header":{{"height":20,"hash":"{hash}"}}}}}}}}"#);
        ("/graphql", 200, tip.into_bytes())
    }

    /// The signer's account, with `nonce`.
    fn account_reply(nonce: u64) -> (&'static str, u16, Vec<u8>) {
        use dusk_core::transfer::moonlight::AccountData;
        (
            "/account",
            200,
            super::rkyv_serialize(&AccountData {
                nonce,
                balance: 1_000_000_000_000_000,
            }),
        )
    }

    /// The block at a height, with `hash` and `label`.
    fn block_reply(hash: &str, label: &str) -> (&'static str, u16, Vec<u8>) {
        let block = format!(
            r#"{{"data":{{"block":{{"header":{{"hash":"{hash}"}},"status":"{label}"}}}}}}"#
        );
        ("/graphql", 200, block.into_bytes())
    }

    /// The node's replies about a recorded deploy transaction that executed
    /// in block 9, which is final: the fence, its status, the contract it
    /// made (`owner`), then the finality wait's.
    fn recorded_deploy_replies(owner: (&'static str, u16, Vec<u8>)) -> MockReplies {
        let executed = br#"{"data":{"tx":{"err":null,"gasSpent":26400000,"blockHeight":9}}}"#;
        let mut replies = fence_replies(6);
        replies.extend([
            ("/graphql", 200, executed.to_vec()),
            owner,
            ("/graphql", 200, executed.to_vec()),
            (
                "/graphql",
                200,
                br#"{"data":{"block":{"header":{"hash":"ab"},"status":"Final"}}}"#.to_vec(),
            ),
            ("/graphql", 200, executed.to_vec()),
        ]);
        replies
    }

    /// The arguments that name the transactions an earlier run recorded for
    /// the contracts `existing` says are there.
    fn token_route_recorded(existing: (bool, bool)) -> Vec<String> {
        let mut args = Vec::new();
        if existing.0 {
            args.extend([
                "--token-tx".into(),
                "7a".repeat(32),
                "--token-tx-nonce".into(),
                "5".into(),
            ]);
        }
        if existing.1 {
            args.extend([
                "--route-tx".into(),
                "7b".repeat(32),
                "--route-tx-nonce".into(),
                "6".into(),
            ]);
        }
        args
    }

    /// `deploy-token-route --plan` prints the IDs the Wasms, the nonces and
    /// the key give, the token's at the nonce and the route's at the next,
    /// and whether a contract is at each, and signs nothing; it needs no
    /// chain ID.
    #[tokio::test]
    async fn deploy_token_route_plans_the_ids_and_sends_nothing() {
        let dir = token_route_wasms("token-route-plan");
        let sent = token_route_sent(&dir);
        let (token, route) = (
            hex::encode(sent.token.id.to_bytes()),
            hex::encode(sent.route.to_bytes()),
        );
        // Worked out independently: BLAKE2b-256 of the Wasm, the nonce as 8
        // bytes LE and the key.
        let (_, pk) = crate::keys::load_from_hex(&"11".repeat(32)).unwrap();
        let blake = |wasm: &[u8], nonce: u64| {
            let mut state = blake2b_simd::Params::new().hash_length(32).to_state();
            state
                .update(wasm)
                .update(&nonce.to_le_bytes())
                .update(&pk.to_bytes());
            hex::encode(state.finalize().as_bytes())
        };
        assert_eq!(token, blake(b"\0asm\x01\0\0\0token", 10));
        assert_eq!(route, blake(b"\0asm\x01\0\0\0route", 11));
        let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
        let (url, server) = mock_rues(vec![
            (leak(format!("{token}/owner")), 200, sent.owner_key.clone()),
            (
                leak(format!("{route}/owner")),
                404,
                b"Contract owner not found".to_vec(),
            ),
        ]);
        let output = super::deploy_token_route(token_route_args(&url, &dir, &["--plan"]))
            .await
            .unwrap();
        assert_eq!(
            output,
            serde_json::json!({
                "success": true,
                "deploy_nonce": 10,
                "contracts": {"token": token, "route": route},
                "exists": {"token": true, "route": false},
            })
        );
        assert_eq!(server.join().unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// IDs other than the planned ones, as a rebuilt Wasm gives, stop the
    /// command before it asks the node anything but the chain ID.
    #[tokio::test]
    async fn deploy_token_route_refuses_ids_other_than_planned() {
        let dir = token_route_wasms("token-route-planned");
        let sent = token_route_sent(&dir);
        let other = "ee".repeat(32);
        let token = hex::encode(sent.token.id.to_bytes());
        for extra in [
            [
                "--expected-token",
                other.as_str(),
                "--expected-chain-id",
                "2",
            ],
            [
                "--expected-route",
                other.as_str(),
                "--expected-chain-id",
                "2",
            ],
        ] {
            let (url, server) = mock_rues(vec![("/chain_id", 200, vec![2])]);
            let mut args = token_route_args(&url, &dir, &extra);
            args.expected_token = args.expected_token.or(Some(token.clone()));
            let error = super::deploy_token_route(args).await.unwrap_err();
            assert!(
                error.contains(&format!("not {other}, which was planned: nothing was sent")),
                "{error}"
            );
            assert_eq!(server.join().unwrap().len(), 1);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A token route deploys the token, then the route, each at its
    /// planned ID, and reports them only once both read back: the token's
    /// settings, supply and roles, the route's owner and bindings. On a
    /// resumed run it deploys only what isn't there; a contract that is
    /// there and doesn't read back stops it.
    #[tokio::test]
    async fn deploy_token_route_reports_only_a_route_that_reads_back_as_sent() {
        use super::deployment::Value;
        let dir = token_route_wasms("token-route-deploy");
        let sent = token_route_sent(&dir);
        let planned = [
            "--expected-token",
            &hex::encode(sent.token.id.to_bytes()),
            "--expected-route",
            &hex::encode(sent.route.to_bytes()),
            "--expected-chain-id",
            "2",
        ]
        .map(String::from);
        let planned: Vec<&str> = planned.iter().map(String::as_str).collect();
        for (existing, deploys) in [((false, false), 2), ((true, false), 1), ((true, true), 0)] {
            let (url, server) = mock_rues(token_route_replies(&sent, existing, None));
            let recorded = token_route_recorded(existing);
            let args: Vec<&str> = planned
                .iter()
                .copied()
                .chain(recorded.iter().map(String::as_str))
                .collect();
            let output = super::deploy_token_route(token_route_args(&url, &dir, &args))
                .await
                .unwrap_or_else(|e| panic!("{existing:?}: {e}"));
            assert_eq!(
                output["contracts"]["token"],
                hex::encode(sent.token.id.to_bytes())
            );
            assert_eq!(
                output["contracts"]["route"],
                hex::encode(sent.route.to_bytes())
            );
            assert_eq!(
                output["deployed"],
                serde_json::json!({"token": !existing.0, "route": !existing.1})
            );
            assert_eq!(
                output["token"],
                serde_json::json!({"name": "Dusk Euro (testnet)", "symbol": "EURd", "decimals": 6,
                                   "supply": "1000000000000"})
            );
            assert_eq!(
                output["router_domains"],
                serde_json::json!([1_146_442_572, 11_155_111])
            );
            let requests = server.join().unwrap();
            let sends: Vec<&(String, Vec<u8>)> = requests
                .iter()
                .filter(|(path, _)| path.ends_with("/propagate"))
                .collect();
            assert_eq!(sends.len(), deploys, "{existing:?}");
            // Each deploy is at its own nonce, with the token's init.
            for (send, (nonce, contract)) in sends.iter().zip(
                [(10u64, "Token"), (11, "WarpCollateral")]
                    .into_iter()
                    .skip(usize::from(existing.0)),
            ) {
                let tx = dusk_core::transfer::Transaction::from_slice(&send.1).unwrap();
                let dusk_core::transfer::Transaction::Moonlight(moonlight) = &tx else {
                    panic!("a Moonlight transaction");
                };
                let Some(dusk_core::transfer::data::TransactionData::Deploy(deploy)) =
                    moonlight.data()
                else {
                    panic!("{contract}: a deploy");
                };
                assert_eq!(deploy.nonce, nonce, "{contract}");
                if contract == "Token" {
                    let (_, pk) = crate::keys::load_from_hex(&"11".repeat(32)).unwrap();
                    let init = super::canonical_drc20::init(
                        super::Drc20Account::moonlight(&pk),
                        "Dusk Euro (testnet)",
                        "EURd",
                        6,
                        1_000_000_000_000,
                    );
                    assert_eq!(deploy.init_args, Some(super::rkyv_serialize(&init)));
                }
            }
        }
        // This deployment's own contracts that don't read back as sent: a
        // token without the deployer as admin, or with the supply held
        // elsewhere; a route bound to another Mailbox, or with a router.
        let attacker = [0xee; 32];
        for (existing, name, method, value, expected) in [
            (
                (true, false),
                "Token",
                "has_role",
                Value::Bool(false),
                "has_role(admin, the deployer) is false, not true",
            ),
            (
                (true, true),
                "Token",
                "balance_of",
                Value::U64(1),
                "balance_of(the deployer) is 1, not 1000000000000",
            ),
            (
                (false, true),
                "WarpCollateral",
                "mailbox",
                Value::Bytes32(attacker),
                "mailbox is eeee",
            ),
            (
                (false, false),
                "WarpCollateral",
                "enrolled_router",
                Value::Bytes32(attacker),
                "enrolled_router(1146442572) is eeee",
            ),
        ] {
            let (url, server) = mock_rues(token_route_replies(
                &sent,
                existing,
                Some((name, method, value)),
            ));
            let recorded = token_route_recorded(existing);
            let args: Vec<&str> = planned
                .iter()
                .copied()
                .chain(recorded.iter().map(String::as_str))
                .collect();
            let error = super::deploy_token_route(token_route_args(&url, &dir, &args))
                .await
                .unwrap_err();
            assert!(
                error.contains("reports none of them as deployed"),
                "{error}"
            );
            assert!(error.contains(expected), "{expected}: {error}");
            server.join().unwrap();
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A contract at a planned ID that no transaction this deployment
    /// recorded created is someone else's, even if it would read back as
    /// sent: someone who deployed there first chose its init, and could
    /// hold roles besides the deployer's, which the token can't list. The
    /// command sends nothing and names it; one recorded transaction that
    /// failed, in a final block, or never executed, proves nothing either.
    #[tokio::test]
    async fn deploy_token_route_refuses_contracts_its_transactions_didnt_create() {
        let dir = token_route_wasms("token-route-taken");
        let sent = token_route_sent(&dir);
        let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
        let (token, route) = (
            hex::encode(sent.token.id.to_bytes()),
            hex::encode(sent.route.to_bytes()),
        );
        let owner = |id: &str, exists: bool| {
            let path = leak(format!("{id}/owner"));
            if exists {
                (path, 200, sent.owner_key.clone())
            } else {
                (path, 404, b"Contract owner not found".to_vec())
            }
        };
        let failed = br#"{"data":{"tx":{"err":"Panic: contract already exists","gasSpent":2000000,"blockHeight":9}}}"#.to_vec();
        let missing = br#"{"data":{"tx":null}}"#.to_vec();
        let recorded = "7a".repeat(32);
        let final_block =
            br#"{"data":{"block":{"header":{"hash":"ab"},"status":"Final"}}}"#.to_vec();
        let cases: Vec<(bool, bool, Vec<&str>, MockReplies, &str)> = vec![
            (true, false, vec![], vec![owner(&token, true)], "token"),
            (
                false,
                true,
                vec![],
                vec![owner(&token, false), owner(&route, true)],
                "route",
            ),
            // Failed, in a final block.
            (
                true,
                true,
                vec!["--token-tx", &recorded, "--token-tx-nonce", "5"],
                {
                    let mut replies = fence_replies(6);
                    replies.extend([
                        ("/graphql", 200, failed.clone()),
                        owner(&token, true),
                        ("/graphql", 200, failed.clone()),
                        ("/graphql", 200, final_block.clone()),
                        ("/graphql", 200, failed.clone()),
                        owner(&token, true),
                    ]);
                    replies
                },
                "token",
            ),
            // Never executed: not in the ledger, before and after the
            // contract was read.
            (
                true,
                false,
                vec!["--token-tx", &recorded, "--token-tx-nonce", "5"],
                {
                    let mut replies = fence_replies(6);
                    replies.extend([
                        ("/graphql", 200, missing.clone()),
                        owner(&token, true),
                        ("/graphql", 200, missing.clone()),
                    ]);
                    replies
                },
                "token",
            ),
        ];
        for (token_there, route_there, recorded_args, provenance_replies, taken) in cases {
            let mut replies = vec![
                ("/chain_id", 200, vec![2]),
                owner(&token, token_there),
                owner(&route, route_there),
                (
                    "/local_domain",
                    200,
                    super::rkyv_serialize(&1_146_442_572u32),
                ),
            ];
            replies.extend(provenance_replies);
            let (url, server) = mock_rues(replies);
            let mut args = vec!["--expected-chain-id", "2"];
            args.extend(recorded_args);
            let output = super::deploy_token_route(token_route_args(&url, &dir, &args))
                .await
                .unwrap();
            assert_eq!(output["success"], false, "{output}");
            assert_eq!(output["taken"], taken, "{output}");
            assert!(
                output["error"]
                    .as_str()
                    .unwrap()
                    .contains("isn't this deployment's"),
                "{output}"
            );
            let requests = server.join().unwrap();
            assert!(
                requests
                    .iter()
                    .all(|(path, _)| !path.ends_with("/propagate")),
                "nothing is sent"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    thread_local! {
        /// The directories this thread has synced (sync_directory).
        pub(super) static DIRECTORY_SYNCS: std::cell::RefCell<Vec<std::path::PathBuf>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// A tx log starts with its first entry and takes each entry after on
    /// a line of its own, its directory synced on each append, a log that
    /// a first append left unfinished included.
    #[test]
    fn append_tx_log_starts_a_log_and_appends_to_it() {
        let dir = std::env::temp_dir().join(format!("dusk-tx-new-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        DIRECTORY_SYNCS.with(|syncs| syncs.borrow_mut().clear());
        let log = dir.join("route.txs");
        super::append_tx_log(&log, &serde_json::json!({"contract": "token"})).unwrap();
        super::append_tx_log(&log, &serde_json::json!({"contract": "route"})).unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "{\"contract\":\"token\"}\n{\"contract\":\"route\"}\n"
        );
        let unfinished = dir.join("unfinished.txs");
        std::fs::write(&unfinished, br#"{"contract":"tok"#).unwrap();
        super::append_tx_log(&unfinished, &serde_json::json!({"contract": "token"})).unwrap();
        assert_eq!(
            DIRECTORY_SYNCS.with(|syncs| syncs.borrow().clone()),
            vec![dir.clone(), dir.clone(), dir.clone()]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Each deploy transaction goes to the tx log before it is sent, on a
    /// line of its own even after a line a run left unfinished. A deploy
    /// that fails because a contract got to the ID between the plan and the
    /// send leaves the ID to it: nothing more is sent, and the output names
    /// it for the caller to plan other IDs.
    #[tokio::test]
    async fn deploy_token_route_logs_each_deploy_and_gives_up_an_id_taken_before_it() {
        use dusk_core::transfer::moonlight::AccountData;
        let dir = token_route_wasms("token-route-log");
        let sent = token_route_sent(&dir);
        let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
        let (token, route) = (
            hex::encode(sent.token.id.to_bytes()),
            hex::encode(sent.route.to_bytes()),
        );
        let log = dir.join("route.txs");
        // A run that stopped in the middle of a line.
        std::fs::write(&log, br#"{"contract":"token","id":"00","tx":"aa"#).unwrap();
        let failed = br#"{"data":{"tx":{"err":"Panic: contract already exists","gasSpent":2000000,"blockHeight":9}}}"#;
        let absent = |id: &str| {
            (
                leak(format!("{id}/owner")),
                404,
                b"Contract owner not found".to_vec(),
            )
        };
        let (url, server) = mock_rues(vec![
            ("/chain_id", 200, vec![2]),
            absent(&token),
            absent(&route),
            (
                "/local_domain",
                200,
                super::rkyv_serialize(&1_146_442_572u32),
            ),
            absent(&token),
            absent(&route),
            (
                "/account",
                200,
                super::rkyv_serialize(&AccountData {
                    nonce: 4,
                    balance: 1_000_000_000_000_000,
                }),
            ),
            ("/on/transactions/preverify", 200, Vec::new()),
            ("/on/transactions/propagate", 200, Vec::new()),
            ("/graphql", 200, failed.to_vec()),
            ("/graphql", 200, failed.to_vec()),
            // Someone else's token got there first.
            (leak(format!("{token}/owner")), 200, vec![0xee; 96]),
        ]);
        let args = [
            "--expected-chain-id",
            "2",
            "--tx-log",
            log.to_str().unwrap(),
        ];
        let output = super::deploy_token_route(token_route_args(&url, &dir, &args))
            .await
            .unwrap();
        assert_eq!(output["success"], false, "{output}");
        assert_eq!(output["taken"], "token", "{output}");
        assert!(
            output["error"]
                .as_str()
                .unwrap()
                .contains("got to its ID first"),
            "{output}"
        );
        let requests = server.join().unwrap();
        let sends: Vec<_> = requests
            .iter()
            .filter(|(path, _)| path.ends_with("/propagate"))
            .collect();
        assert_eq!(sends.len(), 1, "the route isn't sent");
        let tx = dusk_core::transfer::Transaction::from_slice(&sends[0].1).unwrap();
        // The unfinished line stays as it was, and the entry has its own.
        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], r#"{"contract":"token","id":"00","tx":"aa"#);
        let logged: Vec<serde_json::Value> = lines[1..]
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            logged,
            vec![serde_json::json!({"contract": "token", "id": token,
                                    "tx": hex::encode(tx.hash().to_bytes()), "nonce": 5})]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The block a fence read the nonce at is replaced again and again,
    /// each time after a poll that found it not yet final: settling keeps to
    /// one deadline for all of it, and stops, sending nothing, rather than
    /// read the nonce afresh with a new deadline each time.
    #[tokio::test]
    async fn deploy_token_route_settles_within_one_deadline_however_often_a_fence_block_is_replaced(
    ) {
        let dir = token_route_wasms("token-route-replaced");
        let sent = token_route_sent(&dir);
        let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
        let (token, route) = (
            hex::encode(sent.token.id.to_bytes()),
            hex::encode(sent.route.to_bytes()),
        );
        let absent = |id: &str| -> (&'static str, u16, Vec<u8>) {
            (
                leak(format!("{id}/owner")),
                404,
                b"Contract owner not found".to_vec(),
            )
        };
        let recorded = "7a".repeat(32);
        let missing = br#"{"data":{"tx":null}}"#.to_vec();
        let mut replies = vec![
            ("/chain_id", 200, vec![2]),
            absent(&token),
            absent(&route),
            (
                "/local_domain",
                200,
                super::rkyv_serialize(&1_146_442_572u32),
            ),
        ];
        // Each poll interval (3s) of the 8s a test build gives settling: the
        // fence's block is not final yet, then another is at its height.
        for (fenced, next) in [("aa", "bb"), ("bb", "cc")] {
            replies.extend(fence_replies_at(5, fenced));
            replies.extend([
                ("/graphql", 200, missing.clone()),
                absent(&token),
                block_reply(fenced, "Accepted"),
                block_reply(next, "Accepted"),
            ]);
        }
        replies.extend(fence_replies_at(5, "cc"));
        replies.extend([
            ("/graphql", 200, missing.clone()),
            absent(&token),
            block_reply("cc", "Accepted"),
        ]);
        let (url, server) = mock_rues(replies);
        let log = dir.join("replaced.txs");
        let args = [
            "--expected-chain-id",
            "2",
            "--token-tx",
            &recorded,
            "--token-tx-nonce",
            "5",
            "--tx-log",
            log.to_str().unwrap(),
        ];
        let started = std::time::Instant::now();
        let error = super::deploy_token_route(token_route_args(&url, &dir, &args))
            .await
            .unwrap_err();
        assert!(error.contains("wasn't final by the deadline"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(11),
            "{:?}",
            started.elapsed()
        );
        let requests = server.join().unwrap();
        assert!(
            requests
                .iter()
                .all(|(path, _)| !path.ends_with("/propagate")),
            "nothing is sent"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A recorded transaction that isn't in the ledger is settled before
    /// anything is sent again for its ID. Its nonce is read first, so one
    /// that executes while the command looks is taken, not deployed again;
    /// one whose nonce another transaction took, as of a final block, never
    /// executes; one in neither the ledger nor the mempool, its nonce not
    /// reached, a couple of blocks on, was never sent, and is marked void.
    #[tokio::test]
    async fn deploy_token_route_settles_a_recorded_transaction_before_sending_again() {
        use dusk_core::transfer::moonlight::AccountData;
        let dir = token_route_wasms("token-route-pending");
        let sent = token_route_sent(&dir);
        let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
        let (token, route) = (
            hex::encode(sent.token.id.to_bytes()),
            hex::encode(sent.route.to_bytes()),
        );
        let owner = |id: &str, exists: bool| {
            let path = leak(format!("{id}/owner"));
            if exists {
                (path, 200, sent.owner_key.clone())
            } else {
                (path, 404, b"Contract owner not found".to_vec())
            }
        };
        let recorded = "7a".repeat(32);
        let missing = br#"{"data":{"tx":null}}"#.to_vec();
        let executed =
            br#"{"data":{"tx":{"err":null,"gasSpent":26400000,"blockHeight":9}}}"#.to_vec();
        let out_of_gas =
            br#"{"data":{"tx":{"err":"Out of gas","gasSpent":1,"blockHeight":9}}}"#.to_vec();
        let final_block =
            br#"{"data":{"block":{"header":{"hash":"ab"},"status":"Final"}}}"#.to_vec();
        let account = |nonce| {
            (
                "/account",
                200,
                super::rkyv_serialize(&AccountData {
                    nonce,
                    balance: 1_000_000_000_000_000,
                }),
            )
        };
        // The next deploy fails out of gas, with nothing at the ID: what
        // matters is what was sent, and for which contract.
        let fails = |id: &str| -> MockReplies {
            vec![
                account(5),
                ("/on/transactions/preverify", 200, Vec::new()),
                ("/on/transactions/propagate", 200, Vec::new()),
                ("/graphql", 200, out_of_gas.clone()),
                ("/graphql", 200, out_of_gas.clone()),
                owner(id, false),
            ]
        };
        let executes = |nonce| -> MockReplies {
            let mut replies = fence_replies(nonce);
            replies.extend([
                ("/graphql", 200, executed.clone()),
                owner(&token, true),
                ("/graphql", 200, executed.clone()),
                ("/graphql", 200, final_block.clone()),
                ("/graphql", 200, executed.clone()),
            ]);
            replies
        };
        let cases: Vec<(&str, MockReplies, &str, bool)> = vec![
            (
                "never sent",
                {
                    let mut replies = fence_replies(4);
                    replies.extend([
                        ("/graphql", 200, missing.clone()),
                        owner(&token, false),
                        ("/graphql", 200, br#"{"data":{"mempoolTx":null}}"#.to_vec()),
                        owner(&route, false),
                    ]);
                    replies.extend(fails(&token));
                    replies
                },
                "Failed to deploy Token",
                true,
            ),
            (
                "its nonce taken",
                {
                    // The tip moves on during the first read of the nonce,
                    // so it is read again, at block "bb".
                    let mut replies = vec![tip_reply("aa"), account_reply(5), tip_reply("bb")];
                    replies.extend(fence_replies_at(5, "bb"));
                    replies.extend([
                        ("/graphql", 200, missing.clone()),
                        owner(&token, false),
                        // That very block, not yet final, then final.
                        block_reply("bb", "Accepted"),
                        block_reply("bb", "Final"),
                        ("/graphql", 200, missing.clone()),
                        owner(&route, false),
                    ]);
                    replies.extend(fails(&token));
                    replies
                },
                "Failed to deploy Token",
                false,
            ),
            (
                // The block the nonce was read at is replaced before it is
                // final, and on the chain that stays the transaction is in
                // the mempool, then executes: taken, not deployed again.
                "its fence's block replaced",
                {
                    let mut replies = fence_replies_at(5, "aa");
                    replies.extend([
                        ("/graphql", 200, missing.clone()),
                        owner(&token, false),
                        // Another block takes block 20's height.
                        block_reply("cc", "Accepted"),
                    ]);
                    replies.extend(fence_replies_at(4, "cc"));
                    replies.extend([
                        ("/graphql", 200, missing.clone()),
                        owner(&token, false),
                        (
                            "/graphql",
                            200,
                            br#"{"data":{"mempoolTx":{"id":"7a"}}}"#.to_vec(),
                        ),
                        ("/graphql", 200, executed.clone()),
                    ]);
                    replies.extend(executes(5));
                    replies.push(owner(&route, false));
                    replies.extend(fails(&route));
                    replies
                },
                "Failed to deploy WarpCollateral",
                false,
            ),
            (
                "executing from the mempool",
                {
                    let mut replies = fence_replies(4);
                    replies.extend([
                        ("/graphql", 200, missing.clone()),
                        owner(&token, false),
                        (
                            "/graphql",
                            200,
                            br#"{"data":{"mempoolTx":{"id":"7a"}}}"#.to_vec(),
                        ),
                        ("/graphql", 200, executed.clone()),
                    ]);
                    replies.extend(executes(5));
                    replies.push(owner(&route, false));
                    replies.extend(fails(&route));
                    replies
                },
                "Failed to deploy WarpCollateral",
                false,
            ),
            (
                "executing between the reads",
                {
                    let mut replies = fence_replies(4);
                    replies.extend([
                        ("/graphql", 200, missing.clone()),
                        owner(&token, true),
                        ("/graphql", 200, executed.clone()),
                    ]);
                    replies.extend(executes(5));
                    replies.push(owner(&route, false));
                    replies.extend(fails(&route));
                    replies
                },
                "Failed to deploy WarpCollateral",
                false,
            ),
        ];
        for (case, provenance_replies, expected, void) in cases {
            let log = dir.join(format!("{}.txs", case.replace(' ', "-")));
            let mut replies = vec![
                ("/chain_id", 200, vec![2]),
                owner(&token, false),
                owner(&route, false),
                (
                    "/local_domain",
                    200,
                    super::rkyv_serialize(&1_146_442_572u32),
                ),
            ];
            replies.extend(provenance_replies);
            let (url, server) = mock_rues(replies);
            let args = [
                "--expected-chain-id",
                "2",
                "--token-tx",
                &recorded,
                "--token-tx-nonce",
                "5",
                "--tx-log",
                log.to_str().unwrap(),
            ];
            let error = super::deploy_token_route(token_route_args(&url, &dir, &args))
                .await
                .unwrap_err();
            assert!(error.contains(expected), "{case}: {error}");
            let requests = server.join().unwrap();
            assert_eq!(
                requests
                    .iter()
                    .filter(|(path, _)| path.ends_with("/propagate"))
                    .count(),
                1,
                "{case}"
            );
            let logged: Vec<serde_json::Value> = std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let voided = logged.iter().any(|entry| {
                entry["void"] == true
                    && entry["tx"] == recorded.as_str()
                    && entry["contract"] == "token"
            });
            assert_eq!(voided, void, "{case}: {logged:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `drc20-mint` refuses before it signs what the token would refuse.
    #[test]
    fn mints_the_token_would_refuse_are_refused_before_signing() {
        let info = |paused, total_supply, cap, remaining_mintable| super::Drc20Info {
            name: "Dusk Euro (testnet)".into(),
            symbol: "EURd".into(),
            decimals: 6,
            total_supply,
            cap,
            remaining_mintable,
            paused,
        };
        let open = info(false, 1_000_000_000_000, 0, u64::MAX);
        assert!(super::check_mint(&open, true, 5).is_ok());
        for (info, minter, amount, expected) in [
            (
                open.clone(),
                false,
                5,
                "doesn't hold the token's minter role",
            ),
            (open.clone(), true, 0, "more than zero"),
            (info(true, 1, 0, u64::MAX), true, 5, "is paused"),
            (
                info(false, 90, 100, 10),
                true,
                11,
                "cap is 100, and 10 of it is left to mint: not 11",
            ),
            (
                info(false, u64::MAX - 1, 0, u64::MAX),
                true,
                2,
                "would overflow a u64",
            ),
        ] {
            let error = super::check_mint(&info, minter, amount).unwrap_err();
            assert!(error.contains(expected), "{expected}: {error}");
        }
    }

    /// `drc20-mint` mints with the token's own `MintCall`, to the signer by
    /// default, once the token says the signer may.
    #[tokio::test]
    async fn drc20_mint_sends_the_tokens_mint_call() {
        use dusk_core::transfer::moonlight::AccountData;
        let token = "f1".repeat(32);
        let (_, pk) = crate::keys::load_from_hex(&"11".repeat(32)).unwrap();
        let views = |minter: bool| -> MockReplies {
            vec![
                ("/chain_id", 200, vec![2]),
                (
                    "/name",
                    200,
                    super::rkyv_serialize(&String::from("Dusk Euro (testnet)")),
                ),
                ("/symbol", 200, super::rkyv_serialize(&String::from("EURd"))),
                ("/decimals", 200, super::rkyv_serialize(&6u8)),
                (
                    "/total_supply",
                    200,
                    super::rkyv_serialize(&1_000_000_000_000u64),
                ),
                ("/cap", 200, super::rkyv_serialize(&0u64)),
                ("/remaining_mintable", 200, super::rkyv_serialize(&u64::MAX)),
                ("/paused", 200, super::rkyv_serialize(&false)),
                ("/has_role", 200, super::rkyv_serialize(&minter)),
            ]
        };
        let mint = |url: String| async move {
            super::drc20_mint(
                &url,
                None,
                "",
                Some("11".repeat(32)),
                false,
                &"f1".repeat(32),
                None,
                250_000_000,
                2,
                super::MINT_GAS_LIMIT,
                1,
            )
            .await
        };
        let (url, server) = mock_rues(views(false));
        let error = mint(url).await.unwrap_err();
        assert!(error.contains("minter role"), "{error}");
        assert_eq!(server.join().unwrap().len(), 9);

        let mut replies = views(true);
        replies.extend([
            (
                "/account",
                200,
                super::rkyv_serialize(&AccountData {
                    nonce: 4,
                    balance: 1_000_000_000_000,
                }),
            ),
            ("/on/transactions/preverify", 200, Vec::new()),
            ("/on/transactions/propagate", 200, Vec::new()),
            (
                "/graphql",
                200,
                br#"{"data":{"tx":{"err":null,"gasSpent":1900000,"blockHeight":9}}}"#.to_vec(),
            ),
            (
                "/graphql",
                200,
                br#"{"data":{"block":{"header":{"hash":"ab"},"status":"Accepted"}}}"#.to_vec(),
            ),
        ]);
        let (url, server) = mock_rues(replies);
        let output = mint(url).await.unwrap();
        assert_eq!(output["total_supply"], "1000250000000");
        assert_eq!(output["to"], super::account_identity(&pk)["address"]);
        let requests = server.join().unwrap();
        assert_eq!(
            requests[8].1,
            super::rkyv_serialize(&super::canonical_drc20::RoleQuery {
                role: super::canonical_drc20::MINTER_ROLE,
                account: super::Drc20Account::moonlight(&pk),
            })
        );
        let tx = dusk_core::transfer::Transaction::from_slice(&requests[11].1).unwrap();
        let dusk_core::transfer::Transaction::Moonlight(moonlight) = &tx else {
            panic!("a Moonlight transaction");
        };
        assert_eq!(moonlight.nonce(), 5);
        assert_eq!(moonlight.gas_limit(), super::MINT_GAS_LIMIT);
        let Some(dusk_core::transfer::data::TransactionData::Call(call)) = moonlight.data() else {
            panic!("a call");
        };
        assert_eq!(
            call.contract.to_bytes(),
            super::parse_bytes32(&token).unwrap()
        );
        assert_eq!(call.fn_name, "mint");
        assert_eq!(
            call.fn_args,
            super::rkyv_serialize(&super::canonical_drc20::MintCall {
                to: super::Drc20Account::moonlight(&pk),
                amount: 250_000_000,
                authorization: None,
            })
        );
    }

    /// `drc20-info` shows the token, and an account's balance and roles.
    #[tokio::test]
    async fn drc20_info_shows_the_token_and_an_accounts_roles() {
        let (_, pk) = crate::keys::load_from_hex(&"11".repeat(32)).unwrap();
        let account = hex::encode(pk.to_bytes());
        let (url, server) = mock_rues(vec![
            (
                "/name",
                200,
                super::rkyv_serialize(&String::from("Dusk Dollar (testnet)")),
            ),
            ("/symbol", 200, super::rkyv_serialize(&String::from("USDd"))),
            ("/decimals", 200, super::rkyv_serialize(&6u8)),
            ("/total_supply", 200, super::rkyv_serialize(&7u64)),
            ("/cap", 200, super::rkyv_serialize(&0u64)),
            ("/remaining_mintable", 200, super::rkyv_serialize(&u64::MAX)),
            ("/paused", 200, super::rkyv_serialize(&false)),
            ("/balance_of", 200, super::rkyv_serialize(&5u64)),
            ("/has_role", 200, super::rkyv_serialize(&true)),
            ("/has_role", 200, super::rkyv_serialize(&true)),
            ("/has_role", 200, super::rkyv_serialize(&false)),
        ]);
        let output = super::drc20_info_output(&url, &"f2".repeat(32), Some(&account))
            .await
            .unwrap();
        let requests = server.join().unwrap();
        assert_eq!(output["symbol"], "USDd");
        assert_eq!(output["total_supply"], "7");
        assert_eq!(output["paused"], false);
        assert_eq!(output["account"]["balance"], "5");
        assert_eq!(
            (
                &output["account"]["admin"],
                &output["account"]["minter"],
                &output["account"]["pauser"]
            ),
            (
                &serde_json::json!(true),
                &serde_json::json!(true),
                &serde_json::json!(false)
            )
        );
        for (request, role) in requests[8..].iter().zip([
            super::canonical_drc20::DEFAULT_ADMIN_ROLE,
            super::canonical_drc20::MINTER_ROLE,
            super::canonical_drc20::PAUSER_ROLE,
        ]) {
            assert_eq!(
                request.1,
                super::rkyv_serialize(&super::canonical_drc20::RoleQuery {
                    role,
                    account: super::Drc20Account::moonlight(&pk),
                })
            );
        }
    }

    /// `deploy-hyperlane`'s canonical DRC20 takes its name, symbol and
    /// decimals from the command line; without them it is the demo's.
    #[test]
    fn the_canonical_drc20s_settings_come_from_the_command_line() {
        let parse = |extra: &[&str]| {
            let args = [
                "dusk-tx",
                "deploy-hyperlane",
                "--default-ism",
                "testMock",
                "--igp-domain-config",
                "1:0:10000000000:1",
                "--warp-collateral-token",
                "canonical-drc20",
                "--expected-chain-id",
                "2",
            ];
            match Cli::try_parse_from(args.iter().chain(extra).copied())
                .unwrap()
                .command
            {
                super::Command::DeployHyperlane {
                    canonical_token,
                    warp_decimals,
                    ..
                } => (
                    canonical_token.collateral_token_name,
                    canonical_token.collateral_token_symbol,
                    canonical_token
                        .collateral_token_decimals
                        .unwrap_or(warp_decimals),
                    canonical_token.collateral_token_supply,
                ),
                _ => unreachable!("parsed deploy-hyperlane"),
            }
        };
        assert_eq!(
            parse(&[]),
            (
                "Canonical DRC20".into(),
                "CDRC20".into(),
                18,
                5_000_000_000_000_000_000
            )
        );
        assert_eq!(
            parse(&[
                "--collateral-token-name",
                "Dusk Dollar (testnet)",
                "--collateral-token-symbol",
                "USDd",
                "--collateral-token-decimals",
                "6",
                "--collateral-token-supply",
                "1000000000000",
            ]),
            (
                "Dusk Dollar (testnet)".into(),
                "USDd".into(),
                6,
                1_000_000_000_000
            )
        );
        assert_eq!(parse(&["--warp-decimals", "9"]).2, 9);
        let unnamed = super::CollateralTokenSettings {
            name: " ".into(),
            symbol: "USDd".into(),
            decimals: 6,
            supply: 1,
        };
        assert!(unnamed.check().unwrap_err().contains("a name and a symbol"));
    }

    /// `top_up_quote` is the IGP's `quote_pay_for_gas`: gas x price x rate
    /// / 1e10, rounded down, with no overhead, and refuses what it refuses.
    #[test]
    fn top_up_quote_matches_the_contract_and_its_refusals() {
        let (_, config) =
            super::parse_igp_domain_config("11155111:110000:375000:1000000000").unwrap();
        // 1 gwei at 375,000 / 1e10 LUX per wei: 37,500 LUX a gas.
        assert_eq!(super::top_up_quote(config, 1, 11_155_111), Ok(37_500));
        assert_eq!(
            super::top_up_quote(config, 100_000, 11_155_111),
            Ok(3_750_000_000)
        );
        assert_eq!(
            super::igp_quote(config, 50_000),
            super::top_up_quote(config, 160_000, 11_155_111).ok(),
            "a dispatch's quote is a top-up's for its gas plus the overhead"
        );
        let rejected = |config, gas| super::top_up_quote(config, gas, 7).unwrap_err();
        assert!(rejected(config, 0).contains("at least 1"));
        assert!(rejected(config, 1_000_000_001).contains("more than the IGP prices"));
        assert!(
            rejected(hyperlane_dusk_types::DomainGasConfig::default(), 1)
                .contains("no pricing for domain 7")
        );
        let cheap = hyperlane_dusk_types::DomainGasConfig {
            gas_overhead: 10_000,
            token_exchange_rate: 1_000_000,
            gas_price: 1,
        };
        assert!(rejected(cheap, 9_999).contains("costs less than 1 LUX"));
        assert_eq!(super::top_up_quote(cheap, 10_000, 7), Ok(1));
        let huge = hyperlane_dusk_types::DomainGasConfig {
            gas_overhead: 0,
            token_exchange_rate: u64::MAX,
            gas_price: u64::MAX,
        };
        assert!(rejected(huge, 2).contains("doesn't fit a u64"));
    }

    /// The deposit is the quote unless a larger `--max-payment` is given,
    /// and the signer must hold it and the whole gas limit at its price.
    #[test]
    fn top_up_deposit_and_balance_are_checked_before_signing() {
        assert_eq!(super::top_up_deposit(100, None), Ok(100));
        assert_eq!(super::top_up_deposit(100, Some(150)), Ok(150));
        assert!(super::top_up_deposit(100, Some(99))
            .unwrap_err()
            .contains("less than the IGP's quote, 100 LUX"));
        assert_eq!(
            super::ensure_balance_covers(3_000_100, 100, 3_000_000, 1),
            Ok(())
        );
        assert!(super::ensure_balance_covers(3_000_099, 100, 3_000_000, 1)
            .unwrap_err()
            .contains("less than the 3000100"));
        assert!(super::ensure_balance_covers(u64::MAX, 1, u64::MAX, 2).is_err());
        assert!(super::ensure_balance_covers(u64::MAX, u64::MAX, 1, 1).is_err());
    }

    #[test]
    fn a_u64_query_argument_needs_a_u32_one() {
        let contract = "11".repeat(32);
        let base = [
            "dusk-tx",
            "query",
            "--contract",
            &contract,
            "--method",
            "quote_gas_payment",
            "--return-type",
            "u64",
        ];
        assert!(Cli::try_parse_from(base.iter().copied().chain(["--arg-u64", "50000"])).is_err());
        match Cli::try_parse_from(base.iter().copied().chain([
            "--arg-u32",
            "11155111",
            "--arg-u64",
            "50000",
        ]))
        .unwrap()
        .command
        {
            super::Command::Query {
                arg_u32, arg_u64, ..
            } => assert_eq!((arg_u32, arg_u64), (Some(11_155_111), Some(50_000))),
            _ => unreachable!("parsed query"),
        }
    }

    #[test]
    fn igp_config_bounds_the_paid_gas_record() {
        let excessive = format!("42:{}:1:1", u64::MAX);
        assert!(parse_igp_domain_configs(&[excessive])
            .unwrap_err()
            .contains("adjusted gas exceeds u64"));
    }

    #[test]
    fn paginated_query_arguments_encode_both_u32_values() {
        assert_eq!(parse_u32_pair("0,2").unwrap(), (0, 2));
        assert_eq!(parse_u32_pair("17,1").unwrap(), (17, 1));
        for invalid in ["", "0", "0,", ",2", "0,2,3", "x,2", "0,x"] {
            assert!(parse_u32_pair(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn bytes32_queries_preserve_bytes32_arguments() {
        let value = "42".repeat(32);
        assert_eq!(
            prepare_bytes32_query_argument(None, Some(&value)).unwrap(),
            Bytes32QueryArgument::Bytes32([0x42; 32])
        );
        assert_eq!(
            prepare_bytes32_query_argument(Some(7), None).unwrap(),
            Bytes32QueryArgument::U32(7)
        );
        assert_eq!(
            prepare_bytes32_query_argument(None, None).unwrap(),
            Bytes32QueryArgument::Unit
        );
    }

    fn clear_password_env() {
        std::env::remove_var("DUSK_CONSENSUS_PASSWORD_FILE");
        std::env::remove_var("DUSK_CONSENSUS_PASSWORD");
        std::env::remove_var("DUSK_CONSENSUS_KEYS_PASS");
    }

    #[test]
    fn password_file_has_precedence() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_password_env();

        let path =
            std::env::temp_dir().join(format!("dusk-consensus-password-{}", std::process::id()));
        std::fs::write(&path, "from-file\n").unwrap();
        std::env::set_var("DUSK_CONSENSUS_PASSWORD_FILE", &path);
        std::env::set_var("DUSK_CONSENSUS_PASSWORD", "from-env");

        let from_file = resolve_keys_password("").unwrap();
        let explicit = resolve_keys_password("from-cli").unwrap();

        clear_password_env();
        let _ = std::fs::remove_file(path);
        assert_eq!(from_file, "from-file");
        assert_eq!(explicit, "from-cli", "an explicit --password wins");
    }

    #[test]
    fn password_env_is_used_without_an_explicit_password() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_password_env();
        std::env::set_var("DUSK_CONSENSUS_KEYS_PASS", "from-legacy-env");
        std::env::set_var("DUSK_CONSENSUS_PASSWORD", "from-env");

        let from_env = resolve_keys_password("").unwrap();
        let explicit = resolve_keys_password("from-cli").unwrap();
        clear_password_env();
        let fallback = resolve_keys_password("").unwrap();

        assert_eq!(from_env, "from-env");
        assert_eq!(explicit, "from-cli", "an explicit --password wins");
        assert_eq!(fallback, "password", "the development default");
    }

    #[test]
    fn oversized_password_file_is_rejected() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_password_env();

        let path = std::env::temp_dir().join(format!(
            "dusk-consensus-password-oversized-{}",
            std::process::id()
        ));
        std::fs::write(&path, vec![b'x'; MAX_PASSWORD_FILE_BYTES + 1]).unwrap();
        std::env::set_var("DUSK_CONSENSUS_PASSWORD_FILE", &path);

        let error = resolve_keys_password("").unwrap_err();

        clear_password_env();
        let _ = std::fs::remove_file(path);
        assert!(error.contains("exceeds"));
    }

    #[test]
    fn uses_cli_password_without_environment() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_password_env();

        let password = resolve_keys_password("from-cli").unwrap();

        clear_password_env();
        assert_eq!(password, "from-cli");
    }

    #[test]
    fn moonlight_nonce_exhaustion_is_an_error() {
        assert_eq!(next_moonlight_nonce(41).unwrap(), 42);
        assert!(next_moonlight_nonce(u64::MAX)
            .unwrap_err()
            .contains("exhausted"));
    }

    #[test]
    fn submission_error_preserves_reconciliation_hash() {
        let error = submission_error_with_hash(
            "aabbcc",
            "Propagation outcome unknown: response stream closed",
        );
        assert!(error.contains("aabbcc"));
        assert!(error.contains("outcome unknown"));
        assert!(error.contains("reconcile this exact hash before retrying"));
    }

    #[test]
    fn confirmation_timeout_preserves_reconciliation_hash() {
        let error = confirmation_error_with_hash(
            "aabbcc",
            "Transaction aabbcc was not confirmed within 60s",
        );
        assert!(error.contains("confirmation outcome unknown"));
        assert!(error.contains("tx_id=aabbcc"));

        let rejected =
            confirmation_error_with_hash("aabbcc", "Transaction aabbcc failed: contract rejected");
        assert_eq!(rejected, "Transaction aabbcc failed: contract rejected");

        let incompatible = confirmation_error_with_hash(
            "aabbcc",
            "Transaction aabbcc status response is incompatible: missing data.tx.err",
        );
        assert!(incompatible.contains("confirmation outcome unknown"));
        assert!(incompatible.contains("tx_id=aabbcc"));
        assert!(incompatible.contains("reconcile this exact hash before retrying"));
    }

    #[tokio::test]
    async fn transaction_wait_retries_observation_errors_and_keeps_the_hash() {
        let mut statuses = VecDeque::from([
            Err(TransactionStatusQueryError::Retryable(
                "temporary GraphQL outage".to_string(),
            )),
            Ok(TransactionStatus::Executed(AT_10)),
        ]);

        wait_for_transaction_with(
            "aabbcc",
            || ready(statuses.pop_front().expect("status response should exist")),
            Duration::ZERO,
            Duration::from_secs(1),
        )
        .await
        .expect("a later exact-hash success should reconcile the transaction");

        let error = wait_for_transaction_with(
            "ddeeff",
            || {
                ready(Err(TransactionStatusQueryError::Retryable(
                    "archive unavailable".to_string(),
                )))
            },
            Duration::from_millis(1),
            Duration::from_millis(5),
        )
        .await
        .unwrap_err();
        assert!(error.contains("ddeeff"));
        assert!(error.contains("archive unavailable"));
    }

    #[tokio::test]
    async fn transaction_wait_fails_immediately_on_terminal_status_schema_errors() {
        let mut queries = 0usize;
        let error = wait_for_transaction_with(
            "a1b2c3",
            || {
                queries += 1;
                ready(Err(TransactionStatusQueryError::Terminal(
                    "response is missing data.tx.err".to_string(),
                )))
            },
            Duration::from_secs(60),
            Duration::from_secs(120),
        )
        .await
        .unwrap_err();
        assert_eq!(queries, 1);
        assert!(error.contains("a1b2c3"));
        assert!(error.contains("missing data.tx.err"));
    }

    #[tokio::test]
    async fn transaction_wait_checks_immediately_and_rejects_execution_failure() {
        let immediate = tokio::time::timeout(
            Duration::from_millis(100),
            wait_for_transaction_with(
                "1122",
                || ready(Ok(TransactionStatus::Executed(AT_10))),
                Duration::from_secs(60),
                Duration::from_secs(1),
            ),
        )
        .await;
        assert!(immediate.is_ok(), "the first query must not sleep");
        assert_eq!(immediate.unwrap(), Ok(AT_10));

        let error = wait_for_transaction_with(
            "3344",
            || ready(Ok(failed_at(10, "contract rejected"))),
            Duration::ZERO,
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert!(error.contains("3344"));
        assert!(error.contains("contract rejected"));
    }

    #[tokio::test]
    async fn transaction_wait_does_not_stop_at_the_old_attempt_cap() {
        let mut statuses = VecDeque::from(
            (0..20)
                .map(|_| Ok(TransactionStatus::NotFound))
                .chain([Ok(TransactionStatus::Executed(AT_10))])
                .collect::<Vec<_>>(),
        );

        wait_for_transaction_with(
            "5566",
            || ready(statuses.pop_front().expect("status response should exist")),
            Duration::ZERO,
            Duration::from_secs(1),
        )
        .await
        .expect("the absolute deadline must remain authoritative after twenty observations");
    }

    const AT_10: Inclusion = Inclusion {
        block_height: 10,
        gas_spent: 2_140_000,
    };

    fn executed_at(block_height: u64) -> TransactionStatus {
        TransactionStatus::Executed(Inclusion {
            block_height,
            ..AT_10
        })
    }

    fn failed_at(block_height: u64, error: &str) -> TransactionStatus {
        TransactionStatus::Failed {
            error: error.into(),
            inclusion: Inclusion {
                block_height,
                ..AT_10
            },
        }
    }

    fn block(label: &str) -> Option<BlockStatus> {
        Some(BlockStatus {
            hash: format!("{label}-hash"),
            label: label.into(),
        })
    }

    type StatusReply = Result<TransactionStatus, TransactionStatusQueryError>;
    type BlockReply = Result<Option<BlockStatus>, TransactionStatusQueryError>;

    /// Replies queued per query; a query with none left fails the test.
    fn scripted(
        statuses: Vec<StatusReply>,
        blocks: Vec<(u64, BlockReply)>,
    ) -> (
        impl FnMut() -> std::future::Ready<StatusReply>,
        impl FnMut(u64) -> std::future::Ready<BlockReply>,
    ) {
        let mut statuses = VecDeque::from(statuses);
        let mut blocks = VecDeque::from(blocks);
        (
            move || ready(statuses.pop_front().expect("unexpected status query")),
            move |height| {
                let (expected, reply) = blocks.pop_front().expect("unexpected block query");
                assert_eq!(height, expected, "block queried at the wrong height");
                ready(reply)
            },
        )
    }

    #[tokio::test]
    async fn finality_check_needs_the_block_final_and_the_transaction_still_in_it() {
        // Final, and still at that height when read again.
        let (status, block_at) =
            scripted(vec![Ok(executed_at(10))], vec![(10, Ok(block("Final")))]);
        let confirmation = finality_check_with(AT_10, status, block_at).await;
        assert_eq!(confirmation.final_block_hash.as_deref(), Some("Final-hash"));

        // Only inclusion is reported while the block may still be reverted.
        for label in ["Accepted", "Attested", "Confirmed"] {
            let (status, block_at) = scripted(vec![], vec![(10, Ok(block(label)))]);
            assert_eq!(
                finality_check_with(AT_10, status, block_at).await,
                Confirmation::included(AT_10),
                "{label}"
            );
        }

        // A block that is final, but the transaction moved out of it, or
        // nothing to read: not final.
        for (statuses, block_reply) in [
            (vec![Ok(executed_at(11))], Ok(block("Final"))),
            (vec![Ok(TransactionStatus::NotFound)], Ok(block("Final"))),
            (
                vec![Ok(failed_at(10, "re-run after a revert"))],
                Ok(block("Final")),
            ),
            (vec![], Ok(None)),
            (
                vec![],
                Err(TransactionStatusQueryError::Retryable("node busy".into())),
            ),
        ] {
            let (status, block_at) = scripted(statuses, vec![(10, block_reply)]);
            assert_eq!(
                finality_check_with(AT_10, status, block_at).await,
                Confirmation::included(AT_10)
            );
        }
    }

    #[tokio::test]
    async fn finality_wait_follows_the_transaction_until_its_block_is_final() {
        // Pending, included in a block that is then reverted, included again
        // in another block, which becomes final.
        let (status, block_at) = scripted(
            vec![
                Ok(TransactionStatus::NotFound),
                Ok(executed_at(10)),
                Ok(executed_at(10)),
                Ok(TransactionStatus::NotFound),
                Ok(executed_at(12)),
                Ok(executed_at(12)),
                Ok(executed_at(12)),
            ],
            vec![
                (10, Ok(block("Attested"))),
                (10, Ok(None)),
                (12, Ok(block("Accepted"))),
                (12, Ok(block("Final"))),
            ],
        );
        let confirmation = wait_for_finality_with(
            "a1",
            status,
            block_at,
            Duration::ZERO,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(confirmation.inclusion.block_height, 12);
        assert_eq!(confirmation.final_block_hash.as_deref(), Some("Final-hash"));
    }

    #[tokio::test]
    async fn finality_wait_reports_an_execution_error_only_once_it_is_final() {
        // A failure in a block that is then reverted doesn't count: the
        // transaction runs again and succeeds.
        let (status, block_at) = scripted(
            vec![
                Ok(failed_at(10, "first run")),
                Ok(executed_at(11)),
                Ok(executed_at(11)),
            ],
            vec![(10, Ok(block("Attested"))), (11, Ok(block("Final")))],
        );
        assert!(wait_for_finality_with(
            "b2",
            status,
            block_at,
            Duration::ZERO,
            Duration::from_secs(5)
        )
        .await
        .is_ok());

        // A failure in a final block is the transaction's final outcome, in
        // the form the relayer reads as an included failure.
        let (status, block_at) = scripted(
            vec![
                Ok(failed_at(10, "recipient rejected")),
                Ok(failed_at(10, "recipient rejected")),
            ],
            vec![(10, Ok(block("Final")))],
        );
        let error = wait_for_finality_with(
            "c3",
            status,
            block_at,
            Duration::ZERO,
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert_eq!(error, "Transaction c3 failed: recipient rejected");
    }

    #[tokio::test]
    async fn finality_wait_times_out_with_what_it_last_saw() {
        let statuses = std::iter::repeat_with(|| Ok(executed_at(10)));
        let mut statuses = statuses.take(1000).collect::<VecDeque<_>>();
        let error = wait_for_finality_with(
            "d4",
            || ready(statuses.pop_front().unwrap()),
            |_| ready(Ok(block("Attested"))),
            Duration::from_millis(1),
            Duration::from_millis(20),
        )
        .await
        .unwrap_err();
        assert!(
            error.starts_with("Transaction d4 was not final within 0s"),
            "{error}"
        );
        assert!(
            error.contains("last seen in block 10 (Attested)"),
            "{error}"
        );
        assert!(confirmation_error_with_hash("d4", &error).contains("confirmation outcome unknown"));
    }

    #[tokio::test]
    async fn finality_wait_stops_on_incompatible_responses() {
        let (status, block_at) = scripted(
            vec![Ok(executed_at(10))],
            vec![(
                10,
                Err(TransactionStatusQueryError::Terminal(
                    "no data.block.status".into(),
                )),
            )],
        );
        let error = wait_for_finality_with(
            "e5",
            status,
            block_at,
            Duration::ZERO,
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(error.contains("e5"), "{error}");
        assert!(error.contains("no data.block.status"), "{error}");
    }

    #[test]
    fn confirmation_adds_its_fields_and_keeps_the_others() {
        let mut output = serde_json::json!({ "success": true, "tx_id": "ab" });
        Confirmation::included(AT_10).record(&mut output);
        assert_eq!(
            output,
            serde_json::json!({
                "success": true,
                "tx_id": "ab",
                "block_height": 10,
                "gas_spent": 2_140_000,
                "finalized": false,
                "block_hash": null,
            })
        );
        Confirmation {
            inclusion: AT_10,
            final_block_hash: Some("ff".into()),
        }
        .record(&mut output);
        assert_eq!(output["finalized"], true);
        assert_eq!(output["block_hash"], "ff");
    }

    #[test]
    fn wait_finality_is_opt_in_and_global() {
        let cli =
            Cli::try_parse_from(["dusk-tx", "account", "--offline", "--address", "x"]).unwrap();
        assert!(!cli.wait_finality);
        let cli = Cli::try_parse_from([
            "dusk-tx",
            "account",
            "--offline",
            "--address",
            "x",
            "--wait-finality",
            "--finality-timeout",
            "90",
        ])
        .unwrap();
        assert!(cli.wait_finality);
        assert_eq!(cli.finality_timeout, 90);
    }
}

// ── TX construction ─────────────────────────────────────────────────────────

fn gen_contract_id(bytecode: &[u8], nonce: u64, owner: &[u8]) -> ContractId {
    let hash = blake2b_simd::Params::new()
        .hash_length(32)
        .to_state()
        .update(bytecode)
        .update(&nonce.to_le_bytes())
        .update(owner)
        .finalize();
    let hash_bytes: [u8; 32] = hash.as_bytes().try_into().unwrap();
    ContractId::from_bytes(hash_bytes)
}

fn moonlight_deployment(
    sender_sk: &BlsSecretKey,
    bytecode: Vec<u8>,
    owner: &BlsPublicKey,
    init_args: Vec<u8>,
    gas_limit: u64,
    gas_price: u64,
    moonlight_nonce: u64,
    deploy_nonce: u64,
    chain_id: u8,
) -> Result<Transaction, String> {
    let deploy = ContractDeploy {
        bytecode: ContractBytecode {
            hash: *blake3::hash(&bytecode).as_bytes(),
            bytes: bytecode,
        },
        owner: owner.to_bytes().to_vec(),
        init_args: if init_args.is_empty() {
            None
        } else {
            Some(init_args)
        },
        nonce: deploy_nonce,
    };
    let tx = MoonlightTransaction::new(
        sender_sk,
        None,
        0,
        0,
        gas_limit,
        gas_price,
        moonlight_nonce,
        chain_id,
        Some(deploy),
    )
    .map_err(|e| format!("MoonlightTransaction::new failed: {e:?}"))?;
    Ok(tx.into())
}

fn moonlight_call(
    sender_sk: &BlsSecretKey,
    contract: ContractId,
    fn_name: &str,
    fn_args: Vec<u8>,
    gas_limit: u64,
    gas_price: u64,
    moonlight_nonce: u64,
    chain_id: u8,
) -> Result<Transaction, String> {
    moonlight_call_with_deposit(
        sender_sk,
        contract,
        fn_name,
        fn_args,
        0,
        gas_limit,
        gas_price,
        moonlight_nonce,
        chain_id,
    )
}

fn moonlight_call_with_deposit(
    sender_sk: &BlsSecretKey,
    contract: ContractId,
    fn_name: &str,
    fn_args: Vec<u8>,
    deposit: u64,
    gas_limit: u64,
    gas_price: u64,
    moonlight_nonce: u64,
    chain_id: u8,
) -> Result<Transaction, String> {
    let call = ContractCall {
        contract,
        fn_name: String::from(fn_name),
        fn_args,
    };
    let tx = MoonlightTransaction::new(
        sender_sk,
        None,
        0,
        deposit,
        gas_limit,
        gas_price,
        moonlight_nonce,
        chain_id,
        Some(call),
    )
    .map_err(|e| format!("MoonlightTransaction::new failed: {e:?}"))?;
    Ok(tx.into())
}

// ── cmd_fund_dispatch ─────────────────────────────────────────────────────

async fn cmd_fund_dispatch(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    mailbox_hex: &str,
    payer_hex: &str,
    amount: u64,
    expected_chain_id: u8,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    if amount == 0 {
        return Err("Funding amount must be greater than zero".into());
    }
    let mailbox_bytes = parse_bytes32(mailbox_hex)?;
    let mailbox = ContractId::from_bytes(mailbox_bytes);
    let payer = parse_bytes32(payer_hex)?;
    let args = rkyv_serialize(&(payer, amount));
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    // A failed transaction pays its whole gas limit, so check what the
    // Mailbox would refuse first: a payer with no credit is a new entry,
    // which takes the minimum and room under the Mailbox's 3 GiB.
    let credit: u64 = client
        .contract_query(&mailbox_bytes, "fee_credit", &(payer,))
        .await
        .map_err(|e| format!("Can't read the payer's dispatch credit: {e}"))?;
    if credit == 0 {
        if amount < MIN_NEW_PAYER_CREDIT {
            return Err(format!(
                "The payer has no dispatch credit, and the Mailbox takes at least \
                 {MIN_NEW_PAYER_CREDIT} LUX (0.01 DUSK) to fund a payer with none; \
                 {amount} LUX is less"
            ));
        }
        check_room_for_new_payer(&client, &mailbox_bytes, "The payer has no dispatch credit")
            .await?;
    }
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let (nonce, _balance) = client.query_account(&pk).await?;
    let tx = moonlight_call_with_deposit(
        &sk,
        mailbox,
        "fund_dispatch",
        args,
        amount,
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;
    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;
    let mut output = json!({
        "success": true,
        "mailbox": mailbox_hex,
        "payer": payer_hex,
        "amount": amount,
        "tx_id": tx_id,
    });
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

/// Refuse when the Mailbox at `mailbox` has no room for a new payer: its
/// memory plus what a new credit entry can allocate would pass 3 GiB, where
/// it takes no new payers (its dispatch is closing). `why` says why the call
/// adds a payer.
async fn check_room_for_new_payer(
    client: &RuesClient,
    mailbox: &[u8; 32],
    why: &str,
) -> Result<(), String> {
    let memory: u64 = client
        .contract_query(mailbox, "memory_bytes", &())
        .await
        .map_err(|e| format!("Can't read the Mailbox's memory: {e}"))?;
    if memory.saturating_add(NEW_ENTRY_MEMORY_BOUND) > NEW_ENTRY_MEMORY_LIMIT {
        return Err(format!(
            "{why}, and the Mailbox's memory, {memory} bytes, is within 13 pages of 3 GiB: \
             it takes no new payers there (its dispatch is closing), so the transaction \
             would fail"
        ));
    }
    Ok(())
}

// ── cmd_withdraw_dispatch ────────────────────────────────────────────────

async fn cmd_withdraw_dispatch(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    target_hex: &str,
    amount: u64,
    recipient_public_key_hex: Option<&str>,
    expected_chain_id: u8,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    if amount == 0 {
        return Err("Withdrawal amount must be greater than zero".into());
    }
    // Reject malformed public inputs before reading or decrypting one-shot
    // signer material.
    let target = ContractId::from_bytes(parse_bytes32(target_hex)?);
    let explicit_recipient = recipient_public_key_hex
        .map(parse_account_public_key)
        .transpose()?;
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let recipient = explicit_recipient.unwrap_or(pk);
    let args = rkyv_serialize(&(recipient, amount));
    let (nonce, _balance) = client.query_account(&pk).await?;
    let tx = moonlight_call_with_deposit(
        &sk,
        target,
        "withdraw_dispatch_credit",
        args,
        0,
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;
    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;
    let mut output = json!({
        "success": true,
        "target": target_hex,
        "recipient_public_key": hex::encode(recipient.to_bytes()),
        "amount": amount,
        "tx_id": tx_id,
    });
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

fn parse_account_public_key(value: &str) -> Result<BlsPublicKey, String> {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    let bytes =
        hex::decode(value).map_err(|error| format!("Invalid recipient public key hex: {error}"))?;
    let bytes: [u8; 96] = bytes.try_into().map_err(|bytes: Vec<u8>| {
        format!(
            "Recipient public key must be 96 bytes (192 hex), got {}",
            bytes.len()
        )
    })?;
    let recipient = BlsPublicKey::from_bytes(&bytes)
        .map_err(|error| format!("Invalid recipient public key: {error:?}"))?;
    if !recipient.is_valid() {
        return Err("Invalid recipient public key: identity or invalid curve point".into());
    }
    Ok(recipient)
}

/// A canonical DRC20 to deploy: its name, symbol and decimals, and what its
/// `init` mints to the deployer, its admin.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CollateralTokenSettings {
    name: String,
    symbol: String,
    decimals: u8,
    supply: u64,
}

impl CollateralTokenSettings {
    /// Refuse a token nobody could tell apart: one without a name or a
    /// symbol.
    fn check(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.symbol.trim().is_empty() {
            return Err("The token needs a name and a symbol".into());
        }
        Ok(())
    }

    /// The token's `init` argument, with `deployer` its admin.
    fn init(&self, deployer: Drc20Account) -> canonical_drc20::Init {
        canonical_drc20::init(
            deployer,
            &self.name,
            &self.symbol,
            self.decimals,
            self.supply,
        )
    }

    /// What the read-back expects of the token at `id`, deployed by
    /// `deployer`.
    fn sent(&self, id: ContractId, deployer: Drc20Account) -> deployment::CanonicalToken {
        let role = |role| {
            rkyv_serialize(&canonical_drc20::RoleQuery {
                role,
                account: deployer,
            })
        };
        deployment::CanonicalToken {
            id,
            name: self.name.clone(),
            symbol: self.symbol.clone(),
            decimals: self.decimals,
            holder: rkyv_serialize(&Drc20BalanceOf { account: deployer }),
            supply: self.supply,
            roles: vec![
                ("admin", role(canonical_drc20::DEFAULT_ADMIN_ROLE)),
                ("minter", role(canonical_drc20::MINTER_ROLE)),
                ("pauser", role(canonical_drc20::PAUSER_ROLE)),
            ],
        }
    }
}

fn rkyv_serialize<T>(value: &T) -> Vec<u8>
where
    T: Serialize<AllocSerializer<256>>,
{
    let mut serializer = AllocSerializer::<256>::default();
    serializer
        .serialize_value(value)
        .expect("rkyv serialization should not fail");
    serializer.into_serializer().into_inner().to_vec()
}

// ── cmd_call ────────────────────────────────────────────────────────────────

/// How `call` signs and submits its transaction.
#[derive(Debug, Clone, Copy)]
struct CallSubmission {
    /// Sign with this Moonlight nonce instead of the account's next one.
    nonce: Option<u64>,
    /// Return once the node has taken the transaction, without waiting
    /// for its block.
    no_wait: bool,
}

/// How long `call --ack-before-send` waits for its go-ahead.
const SEND_ACK_TIMEOUT: Duration = Duration::from_secs(30);
/// The most bytes of the go-ahead line read: `send` and its newline, with
/// room to spare. A longer line isn't `send`.
const MAX_SEND_ACK_LINE_BYTES: u64 = 64;

/// `call --ack-before-send`: `call` names its signed transaction
/// (`Prepared TX <hash>` on stderr), then sends it only on the caller's
/// go-ahead, the line `send` read from `input` within `timeout`.
struct SendAck {
    /// Opens what the line is read from: stdin, or a test's input. It runs
    /// on the thread that reads the line.
    input: Box<dyn FnOnce() -> Box<dyn BufRead> + Send>,
    timeout: Duration,
}

impl SendAck {
    fn stdin() -> Self {
        Self {
            input: Box::new(|| Box::new(std::io::stdin().lock())),
            timeout: SEND_ACK_TIMEOUT,
        }
    }

    /// Wait for the line `send`, without its newline. The end of the
    /// input, a failed read, any other line or none within the timeout is
    /// a refusal, with its reason.
    async fn wait(self) -> Result<(), String> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let input = self.input;
        // A blocking read can't be cancelled, so it gets a thread of its
        // own: after a timeout it is left blocked, and ends with the
        // process. On the runtime's blocking pool it would hold up the
        // runtime's shutdown until the read returned.
        std::thread::spawn(move || {
            let mut line = Vec::new();
            let read = input()
                .take(MAX_SEND_ACK_LINE_BYTES)
                .read_until(b'\n', &mut line)
                .map(|_| line);
            let _ = sender.send(read);
        });
        let line = match tokio::time::timeout(self.timeout, receiver).await {
            Err(_) => {
                return Err(format!(
                    "no line \"send\" on stdin within {:?}",
                    self.timeout
                ))
            }
            Ok(Err(_)) => return Err("the stdin reader stopped without a line".into()),
            Ok(Ok(Err(error))) => return Err(format!("reading stdin failed: {error}")),
            Ok(Ok(Ok(line))) => line,
        };
        match line.strip_suffix(b"\n") {
            Some(b"send") => Ok(()),
            None if (line.len() as u64) < MAX_SEND_ACK_LINE_BYTES => {
                Err("stdin closed before the line \"send\"".into())
            }
            _ => Err("the line on stdin was not \"send\"".into()),
        }
    }
}

/// The error of a transaction `call --ack-before-send` didn't send. The
/// agents tell it from one with an unknown outcome by its detail, after
/// `submission failed: `, as they tell a preverify refusal.
fn not_submitted_error(tx_id: &str, reason: &str) -> String {
    format!("Transaction {tx_id} submission failed: Not submitted: {reason}")
}

#[allow(clippy::too_many_arguments)]
async fn cmd_call(
    rues_url: &str,
    rues_url_stdin: bool,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    contract_hex: &str,
    fn_name: &str,
    args_hex: &str,
    gas_limit: u64,
    gas_price: u64,
    expected_chain_id: u8,
    simulate_only: bool,
    submission: CallSubmission,
    ack: Option<SendAck>,
) -> Result<(), String> {
    let stdin_url;
    let rues_url = if rues_url_stdin {
        // Reject malformed public inputs before reading stdin, as `call`
        // does before reading the key.
        parse_call_inputs(contract_hex, args_hex, simulate_only, submission)?;
        stdin_url = read_rues_url(std::io::stdin().lock())?;
        &stdin_url
    } else {
        rues_url
    };
    let output = call(
        rues_url,
        keys_path,
        password,
        secret_key_hex,
        secret_key_stdin,
        contract_hex,
        fn_name,
        args_hex,
        gas_limit,
        gas_price,
        expected_chain_id,
        simulate_only,
        submission,
        ack,
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

/// The contract and the arguments `call` sends, checked before anything is
/// read from stdin or the node.
fn parse_call_inputs(
    contract_hex: &str,
    args_hex: &str,
    simulate_only: bool,
    submission: CallSubmission,
) -> Result<(ContractId, Vec<u8>), String> {
    // Reject malformed public inputs before reading one-shot signer material.
    let contract_bytes =
        hex::decode(contract_hex).map_err(|e| format!("Invalid contract hex: {e}"))?;
    if contract_bytes.len() != 32 {
        return Err(format!(
            "Contract ID must be 32 bytes (64 hex), got {}",
            contract_bytes.len()
        ));
    }
    let contract_id = ContractId::from_bytes(contract_bytes.try_into().map_err(|_| "bad length")?);

    // Parse args
    let fn_args = if args_hex.is_empty() {
        rkyv_serialize(&())
    } else {
        hex::decode(args_hex).map_err(|e| format!("Invalid args hex: {e}"))?
    };
    if fn_args.len() > MAX_CALL_ARGS_BYTES {
        return Err(format!(
            "Call arguments exceed the {MAX_CALL_ARGS_BYTES}-byte helper transport limit"
        ));
    }
    if simulate_only {
        return Err(
            "Safe simulation is unavailable: current Rusk simulation accepts an ordinary replayable signed transaction. Refusing before signer access"
                .into(),
        );
    }
    check_call_submission(submission, finality_wait())?;
    Ok((contract_id, fn_args))
}

/// `--no-wait` returns before the transaction is included, so it can't also
/// wait for its block to be final, as `--wait-finality` asks.
fn check_call_submission(
    submission: CallSubmission,
    finality_wait: Option<Duration>,
) -> Result<(), String> {
    if submission.no_wait && finality_wait.is_some() {
        return Err(
            "--no-wait returns before the transaction is included, so it can't wait for finality (--wait-finality)"
                .into(),
        );
    }
    Ok(())
}

/// Sign and submit a contract call, and return `call`'s JSON output. With
/// `ack`, the call is sent only on the caller's go-ahead (`SendAck`).
#[allow(clippy::too_many_arguments)]
async fn call(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    contract_hex: &str,
    fn_name: &str,
    args_hex: &str,
    gas_limit: u64,
    gas_price: u64,
    expected_chain_id: u8,
    simulate_only: bool,
    submission: CallSubmission,
    ack: Option<SendAck>,
) -> Result<serde_json::Value, String> {
    let (contract_id, fn_args) =
        parse_call_inputs(contract_hex, args_hex, simulate_only, submission)?;
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    // The go-ahead comes on the line after the key, so the key is one line.
    let stdin_key = if ack.is_some() {
        StdinKey::Line
    } else {
        StdinKey::Rest
    };
    let (sk, pk) = load_keys_with(
        keys_path,
        password,
        secret_key_hex,
        secret_key_stdin,
        stdin_key,
    )?;

    // Query the account only after endpoint identity is bound, and only if
    // the caller didn't choose the nonce.
    let nonce = match submission.nonce {
        Some(nonce) => nonce,
        None => next_moonlight_nonce(client.query_account(&pk).await?.0)?,
    };

    // Build and submit TX
    let tx = moonlight_call(
        &sk,
        contract_id,
        fn_name,
        fn_args,
        gas_limit,
        gas_price,
        nonce,
        chain_id,
    )?;

    let tx_id = hex::encode(tx.hash().to_bytes());
    let tx_bytes = tx.to_var_bytes();
    // A chosen nonce may be ahead of the account's next one while the
    // caller's earlier transactions are in flight: the node holds it.
    let queue_future_nonce = submission.nonce.is_some();
    let confirmation = if submission.no_wait {
        propagate_with(&client, &tx_id, &tx_bytes, queue_future_nonce, ack).await?;
        None
    } else {
        Some(propagate_and_wait_with(&client, &tx_id, &tx_bytes, queue_future_nonce, ack).await?)
    };
    Ok(call_output(
        contract_hex,
        fn_name,
        &tx_id,
        nonce,
        confirmation.as_ref(),
    ))
}

/// `call`'s output: what was sent, and where it was included, or, with
/// `--no-wait`, that it was submitted (`submitted`) and no block fields.
fn call_output(
    contract_hex: &str,
    fn_name: &str,
    tx_id: &str,
    nonce: u64,
    confirmation: Option<&Confirmation>,
) -> serde_json::Value {
    let mut output = json!({
        "success": true,
        "contract": contract_hex,
        "fn_name": fn_name,
        "tx_id": tx_id,
        "nonce": nonce,
    });
    match confirmation {
        Some(confirmation) => confirmation.record(&mut output),
        None => output["submitted"] = json!(true),
    }
    output
}

/// The chain ID a command signs for: the endpoint's, which must be
/// `expected` (`--expected-chain-id`, which every command that sends a
/// transaction requires). Each binds it before it reads the signer or
/// builds a transaction, and before it queries anything else.
async fn bind_chain_id(client: &RuesClient, expected: u8) -> Result<u8, String> {
    resolve_signing_chain_id(expected, client.query_chain_id().await?)
}

fn resolve_signing_chain_id(expected: u8, observed: u8) -> Result<u8, String> {
    if observed != expected {
        return Err(format!(
            "Configured Dusk chain ID {expected} does not match endpoint chain ID {observed}"
        ));
    }
    Ok(expected)
}

fn submission_error_with_hash(tx_id: &str, error: &str) -> String {
    format!(
        "Transaction {tx_id} submission failed: {error}; retain tx_id={tx_id} and reconcile this exact hash before retrying if the propagation outcome is unknown"
    )
}

fn confirmation_error_with_hash(tx_id: &str, error: &str) -> String {
    let execution_failure_prefix = format!("Transaction {tx_id} failed:");
    if error.starts_with(&execution_failure_prefix) {
        error.to_owned()
    } else {
        format!(
            "Transaction {tx_id} confirmation outcome unknown: {error}; retain tx_id={tx_id} and reconcile this exact hash before retrying"
        )
    }
}

// ── cmd_deploy_hyperlane ────────────────────────────────────────────────────

/// Check a DRC20 named by ID for a collateral route: it exists, answers the
/// DRC20 `balance_of`, and isn't a warp route. A WarpDrc20 is one, and every
/// route answers `mailbox`; see [`WARP_DRC20_COLLATERAL_REFUSED`]. The token
/// counts as no route only once the node has shown it has no `mailbox`
/// ([`RuesClient::optional_view`]): a reply that doesn't stops the deploy.
async fn check_explicit_collateral_token(
    client: &RuesClient,
    token: ContractId,
    deployer: Drc20Account,
) -> Result<(), String> {
    let token_hex = hex::encode(token.to_bytes());
    if !client.contract_exists(&token_hex).await? {
        return Err(format!(
            "Explicit collateral token does not exist on-chain: {token_hex}"
        ));
    }
    let balance_of = Drc20BalanceOf { account: deployer };
    client
        .contract_query::<_, u64>(&token.to_bytes(), "balance_of", &balance_of)
        .await
        .map_err(|error| {
            format!(
                "Explicit collateral token {token_hex} does not expose the required DRC20 balance_of ABI: {error}"
            )
        })?;
    let mailbox = client
        .optional_view::<_, [u8; 32]>(
            &token.to_bytes(),
            "mailbox",
            &(),
            ("balance_of", &rkyv_serialize(&balance_of)),
        )
        .await
        .map_err(|error| {
            format!(
                "Can't tell whether the explicit collateral token {token_hex} is a warp route, \
                 which it must not be, so nothing was deployed: {error}"
            )
        })?;
    if mailbox.is_some() {
        return Err(format!(
            "Explicit collateral token {token_hex} is a warp route, so a WarpDrc20: \
             {WARP_DRC20_COLLATERAL_REFUSED}"
        ));
    }
    Ok(())
}

async fn cmd_deploy_hyperlane(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    domain: u32,
    gas_limit: u64,
    gas_price: u64,
    wasm_dir: Option<PathBuf>,
    deploy_warp_drc20: bool,
    deploy_warp_native: bool,
    warp_collateral_token: Option<&str>,
    canonical_token: CollateralTokenSettings,
    warp_name: &str,
    warp_symbol: &str,
    warp_decimals: u8,
    default_ism: &str,
    igp_domain_config_args: &[String],
    multisig_validators: &str,
    multisig_threshold: u8,
    expected_chain_id: u8,
) -> Result<(), String> {
    if warp_collateral_token == Some("warp-drc20") {
        return Err(format!(
            "--warp-collateral-token warp-drc20 is refused: {WARP_DRC20_COLLATERAL_REFUSED}"
        ));
    }
    if warp_collateral_token == Some(CANONICAL_DRC20) {
        canonical_token.check()?;
    }
    let igp_domain_configs = parse_igp_domain_configs(igp_domain_config_args)?;
    let default_ism = parse_default_ism(default_ism)?;
    let (multisig_validators, multisig_threshold) = match default_ism {
        DefaultIsm::TestMock => (Vec::<EthAddress>::new(), 0u8),
        DefaultIsm::MessageIdMultisig => {
            let validators = parse_eth_addresses(multisig_validators)?;
            if validators.is_empty() {
                return Err("default_ism=messageIdMultisig requires --multisig-validators".into());
            }
            if validators.len() > MAX_MULTISIG_VALIDATORS {
                return Err(format!(
                    "messageIdMultisig supports at most {MAX_MULTISIG_VALIDATORS} validators"
                ));
            }
            if multisig_threshold == 0 {
                return Err(
                    "default_ism=messageIdMultisig requires --multisig-threshold > 0".into(),
                );
            }
            if multisig_threshold as usize > validators.len() {
                return Err(format!(
                    "multisig_threshold ({}) must be <= number of validators ({})",
                    multisig_threshold,
                    validators.len()
                ));
            }
            (validators, multisig_threshold)
        }
    };

    // The chain every deploy is signed for, before the key is read.
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let (account_nonce, balance) = client.query_account(&pk).await?;
    let pk_short = hex::encode(&pk.to_bytes()[..8]);

    eprintln!("Deploying Hyperlane contracts on Dusk");
    eprintln!("  Chain ID:    {chain_id}");
    eprintln!("  Domain:      {domain}");
    eprintln!("  Deployer:    {pk_short}...");
    eprintln!("  Balance:     {balance} LUX");
    eprintln!("  Nonce:       {account_nonce}");

    if balance == 0 {
        return Err("Deployer account has zero balance".into());
    }

    // Resolve WASM directory
    let wasm_dir = wasm_dir.unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/contract/wasm32-unknown-unknown/release")
    });

    // Load WASM bytecodes
    let load_wasm = |name: &str| -> Result<Vec<u8>, String> {
        let path = wasm_dir.join(format!("hyperlane_dusk_{name}.wasm"));
        std::fs::read(&path).map_err(|e| format!("Failed to read {}: {e}", path.display()))
    };

    let test_mock_bytes = load_wasm("test_mock")?;
    let test_recipient_bytes = load_wasm("test_recipient")?;
    let merkle_tree_hook_bytes = load_wasm("merkle_tree_hook")?;
    let aggregation_hook_bytes = load_wasm("aggregation_hook")?;
    let ism_multisig_bytes = if default_ism == DefaultIsm::MessageIdMultisig {
        Some(load_wasm("ism_multisig")?)
    } else {
        None
    };
    let mailbox_bytes = load_wasm("mailbox")?;
    let va_bytes = load_wasm("validator_announce")?;
    let protocol_fee_bytes = load_wasm("protocol_fee")?;
    let igp_bytes = load_wasm("igp")?;
    let warp_drc20_bytes = deploy_warp_drc20
        .then(|| load_wasm("warp_drc20"))
        .transpose()?;
    let warp_native_bytes = deploy_warp_native
        .then(|| load_wasm("warp_native"))
        .transpose()?;
    let deploys_canonical_token = warp_collateral_token == Some(CANONICAL_DRC20);
    let explicit_collateral_token = warp_collateral_token
        .filter(|token| *token != CANONICAL_DRC20)
        .map(parse_bytes32)
        .transpose()?
        .map(ContractId::from_bytes);
    if let Some(token) = explicit_collateral_token {
        check_explicit_collateral_token(&client, token, Drc20Account::moonlight(&pk)).await?;
    }
    let canonical_token_bytes = deploys_canonical_token
        .then(|| {
            let path = wasm_dir.join(CANONICAL_DRC20_WASM);
            std::fs::read(&path).map_err(|e| {
                format!(
                    "Failed to read {} (make canonical-drc20-fixture builds it): {e}",
                    path.display()
                )
            })
        })
        .transpose()?;
    let warp_collateral_bytes = warp_collateral_token
        .is_some()
        .then(|| load_wasm("warp_drc20_collateral"))
        .transpose()?;

    let pk_bytes = pk.to_bytes();
    let owner_h256 = hyperlane_dusk_types::message::keccak256(&pk_bytes);

    // Pre-compute contract IDs (deterministic from bytecode + deploy_nonce + owner)
    //
    // Deploy order:
    // TestMock, TestRecipient, MerkleTreeHook, optional IsmMultisig,
    // ProtocolFee, AggregationHook, Mailbox, ValidatorAnnounce, IGP,
    // then the optional synthetic, native, and DRC20-collateral routes.
    let mut nonce_counter = 0u64;
    let test_mock_id = gen_contract_id(&test_mock_bytes, nonce_counter, &pk_bytes);
    nonce_counter += 1;
    let test_recipient_id = gen_contract_id(&test_recipient_bytes, nonce_counter, &pk_bytes);
    nonce_counter += 1;
    let merkle_tree_hook_id = gen_contract_id(&merkle_tree_hook_bytes, nonce_counter, &pk_bytes);
    nonce_counter += 1;

    let ism_multisig_id = ism_multisig_bytes.as_ref().map(|bytes| {
        let id = gen_contract_id(bytes, nonce_counter, &pk_bytes);
        nonce_counter += 1;
        id
    });

    let protocol_fee_id = gen_contract_id(&protocol_fee_bytes, nonce_counter, &pk_bytes);
    nonce_counter += 1;
    let aggregation_hook_id = gen_contract_id(&aggregation_hook_bytes, nonce_counter, &pk_bytes);
    nonce_counter += 1;
    let mailbox_id = gen_contract_id(&mailbox_bytes, nonce_counter, &pk_bytes);
    nonce_counter += 1;
    let va_id = gen_contract_id(&va_bytes, nonce_counter, &pk_bytes);
    nonce_counter += 1;
    let igp_id = gen_contract_id(&igp_bytes, nonce_counter, &pk_bytes);
    nonce_counter += 1;

    let warp_drc20_id = warp_drc20_bytes.map(|warp_bytes| {
        let id = gen_contract_id(&warp_bytes, nonce_counter, &pk_bytes);
        nonce_counter += 1;
        (id, warp_bytes)
    });
    let warp_native_id = warp_native_bytes.map(|warp_bytes| {
        let id = gen_contract_id(&warp_bytes, nonce_counter, &pk_bytes);
        nonce_counter += 1;
        (id, warp_bytes)
    });
    let canonical_token_id = canonical_token_bytes.map(|token_bytes| {
        let id = gen_contract_id(&token_bytes, nonce_counter, &pk_bytes);
        nonce_counter += 1;
        (id, token_bytes)
    });
    let collateral_token = canonical_token_id
        .as_ref()
        .map(|(id, _)| *id)
        .or(explicit_collateral_token);
    let deployed_canonical_token = canonical_token_id.as_ref().map(|(id, _)| *id);
    let warp_collateral_id = warp_collateral_bytes.map(|warp_bytes| {
        let id = gen_contract_id(&warp_bytes, nonce_counter, &pk_bytes);
        (id, warp_bytes)
    });

    eprintln!("\n  Contract IDs (pre-computed):");
    eprintln!(
        "    TestMock:        {}",
        hex::encode(test_mock_id.to_bytes())
    );
    eprintln!(
        "    TestRecipient:   {}",
        hex::encode(test_recipient_id.to_bytes())
    );
    eprintln!(
        "    MerkleTreeHook:  {}",
        hex::encode(merkle_tree_hook_id.to_bytes())
    );
    if let Some(ism_id) = &ism_multisig_id {
        eprintln!("    IsmMultisig:     {}", hex::encode(ism_id.to_bytes()));
    }
    eprintln!(
        "    ProtocolFee:     {}",
        hex::encode(protocol_fee_id.to_bytes())
    );
    eprintln!(
        "    AggregationHook: {}",
        hex::encode(aggregation_hook_id.to_bytes())
    );
    eprintln!(
        "    Mailbox:         {}",
        hex::encode(mailbox_id.to_bytes())
    );
    eprintln!("    ValidatorAnnounce: {}", hex::encode(va_id.to_bytes()));
    eprintln!("    IGP:             {}", hex::encode(igp_id.to_bytes()));
    if let Some((ref id, _)) = warp_drc20_id {
        eprintln!("    WarpDrc20:       {}", hex::encode(id.to_bytes()));
    }
    if let Some((ref id, _)) = warp_native_id {
        eprintln!("    WarpNative:      {}", hex::encode(id.to_bytes()));
    }
    if let Some((ref id, _)) = canonical_token_id {
        eprintln!("    CollateralToken: {}", hex::encode(id.to_bytes()));
    }
    if let Some((ref id, _)) = warp_collateral_id {
        eprintln!("    WarpCollateral:  {}", hex::encode(id.to_bytes()));
    }

    // Refuse to deploy if any of the deterministic contract IDs already exist.
    // Re-running deploys against a non-reset chain will otherwise silently
    // "succeed" (TX included) but keep old contract state, breaking wiring.
    let mut existing: Vec<(String, String)> = Vec::new();
    let test_mock_hex = hex::encode(test_mock_id.to_bytes());
    if client.contract_exists(&test_mock_hex).await? {
        existing.push(("TestMock".into(), test_mock_hex));
    }
    let test_recipient_hex = hex::encode(test_recipient_id.to_bytes());
    if client.contract_exists(&test_recipient_hex).await? {
        existing.push(("TestRecipient".into(), test_recipient_hex));
    }
    let merkle_tree_hook_hex = hex::encode(merkle_tree_hook_id.to_bytes());
    if client.contract_exists(&merkle_tree_hook_hex).await? {
        existing.push(("MerkleTreeHook".into(), merkle_tree_hook_hex));
    }
    if let Some(ism_id) = &ism_multisig_id {
        let ism_hex = hex::encode(ism_id.to_bytes());
        if client.contract_exists(&ism_hex).await? {
            existing.push(("IsmMultisig".into(), ism_hex));
        }
    }
    let mailbox_hex = hex::encode(mailbox_id.to_bytes());
    if client.contract_exists(&mailbox_hex).await? {
        existing.push(("Mailbox".into(), mailbox_hex));
    }
    let va_hex = hex::encode(va_id.to_bytes());
    if client.contract_exists(&va_hex).await? {
        existing.push(("ValidatorAnnounce".into(), va_hex));
    }
    let protocol_fee_hex = hex::encode(protocol_fee_id.to_bytes());
    if client.contract_exists(&protocol_fee_hex).await? {
        existing.push(("ProtocolFee".into(), protocol_fee_hex));
    }
    let aggregation_hook_hex = hex::encode(aggregation_hook_id.to_bytes());
    if client.contract_exists(&aggregation_hook_hex).await? {
        existing.push(("AggregationHook".into(), aggregation_hook_hex));
    }
    let igp_hex = hex::encode(igp_id.to_bytes());
    if client.contract_exists(&igp_hex).await? {
        existing.push(("IGP".into(), igp_hex));
    }
    if let Some((ref warp_id, _)) = warp_drc20_id {
        let warp_hex = hex::encode(warp_id.to_bytes());
        if client.contract_exists(&warp_hex).await? {
            existing.push(("WarpDrc20".into(), warp_hex));
        }
    }
    if let Some((ref warp_id, _)) = warp_native_id {
        let warp_hex = hex::encode(warp_id.to_bytes());
        if client.contract_exists(&warp_hex).await? {
            existing.push(("WarpNative".into(), warp_hex));
        }
    }
    if let Some((ref token_id, _)) = canonical_token_id {
        let token_hex = hex::encode(token_id.to_bytes());
        if client.contract_exists(&token_hex).await? {
            existing.push(("CollateralToken".into(), token_hex));
        }
    }
    if let Some((ref warp_id, _)) = warp_collateral_id {
        let warp_hex = hex::encode(warp_id.to_bytes());
        if client.contract_exists(&warp_hex).await? {
            existing.push(("WarpCollateral".into(), warp_hex));
        }
    }

    if !existing.is_empty() {
        let mut msg = String::from("Refusing to deploy: contract IDs already exist on-chain:\n");
        for (name, id) in existing {
            msg.push_str(&format!("  {name}: {id}\n"));
        }
        msg.push_str(
            "This usually means you are re-running deploy-hyperlane against a non-reset chain. \
Restart rusk with a fresh state (stop-env/start-env), or use a different deployer key so contract IDs change.",
        );
        return Err(msg);
    }

    let mut mn = account_nonce; // moonlight nonce
    let mut dn = 0u64; // deploy nonce
    let mut deployed = Vec::new();

    // Deploy contracts one at a time, each once the one before is included
    // and executed. Rusk would take the next nonce as soon as the last
    // transaction is in its mempool, but a deploy that fails pays its whole
    // gas limit, and so would every deploy sent after it: stopping at the
    // first failure saves them.

    // 1. TestMock (no init)
    deploy_one(
        &client,
        &sk,
        &pk,
        test_mock_bytes,
        vec![],
        &mut mn,
        &mut dn,
        &mut deployed,
        gas_limit,
        gas_price,
        chain_id,
        "TestMock",
    )
    .await?;

    // 2. TestRecipient (no init)
    deploy_one(
        &client,
        &sk,
        &pk,
        test_recipient_bytes,
        vec![],
        &mut mn,
        &mut dn,
        &mut deployed,
        gas_limit,
        gas_price,
        chain_id,
        "TestRecipient",
    )
    .await?;

    // 3. MerkleTreeHook: child of the aggregation hook.
    let mth_init = rkyv_serialize(&(aggregation_hook_id,));
    deploy_one(
        &client,
        &sk,
        &pk,
        merkle_tree_hook_bytes,
        mth_init,
        &mut mn,
        &mut dn,
        &mut deployed,
        gas_limit,
        gas_price,
        chain_id,
        "MerkleTreeHook",
    )
    .await?;

    // 4. Optional: MessageIdMultisigISM
    let mut validators = multisig_validators;
    validators.sort();
    if let Some(ism_multisig_bytes) = ism_multisig_bytes {
        let _ism_multisig_id = ism_multisig_id.expect("ISM ID missing");
        let init = rkyv_serialize(&(owner_h256, validators.clone(), multisig_threshold));
        deploy_one(
            &client,
            &sk,
            &pk,
            ism_multisig_bytes,
            init,
            &mut mn,
            &mut dn,
            &mut deployed,
            gas_limit,
            gas_price,
            chain_id,
            "IsmMultisig",
        )
        .await?;
    }

    // 5. ProtocolFee: funded through the aggregation hook.
    let pf_init = rkyv_serialize(&(
        PROTOCOL_FEE,
        MAX_PROTOCOL_FEE,
        aggregation_hook_id,
        owner_h256,
        owner_h256,
    ));
    deploy_one(
        &client,
        &sk,
        &pk,
        protocol_fee_bytes,
        pf_init,
        &mut mn,
        &mut dn,
        &mut deployed,
        gas_limit,
        gas_price,
        chain_id,
        "ProtocolFee",
    )
    .await?;

    // 6. AggregationHook: required Merkle insertion plus protocol fee custody.
    let aggregation_init =
        rkyv_serialize(&(mailbox_id, vec![merkle_tree_hook_id, protocol_fee_id]));
    deploy_one(
        &client,
        &sk,
        &pk,
        aggregation_hook_bytes,
        aggregation_init,
        &mut mn,
        &mut dn,
        &mut deployed,
        gas_limit,
        gas_price,
        chain_id,
        "AggregationHook",
    )
    .await?;

    // 7. Mailbox: init(local_domain, owner, default_ism, default_hook, required_hook)
    let default_ism_id = ism_multisig_id.unwrap_or(test_mock_id);
    let mailbox_init = rkyv_serialize(&(
        domain,
        owner_h256,
        default_ism_id,
        igp_id,
        aggregation_hook_id,
    ));
    deploy_one(
        &client,
        &sk,
        &pk,
        mailbox_bytes,
        mailbox_init,
        &mut mn,
        &mut dn,
        &mut deployed,
        gas_limit,
        gas_price,
        chain_id,
        "Mailbox",
    )
    .await?;

    // 8. ValidatorAnnounce: init(local_domain, mailbox)
    let va_init = rkyv_serialize(&(domain, mailbox_id));
    deploy_one(
        &client,
        &sk,
        &pk,
        va_bytes,
        va_init,
        &mut mn,
        &mut dn,
        &mut deployed,
        gas_limit,
        gas_price,
        chain_id,
        "ValidatorAnnounce",
    )
    .await?;

    // 9. IGP: init(mailbox, owner, beneficiary, initial_configs)
    let igp_init = rkyv_serialize(&(
        mailbox_id,
        owner_h256,
        owner_h256,
        igp_domain_configs.clone(),
    ));
    deploy_one(
        &client,
        &sk,
        &pk,
        igp_bytes,
        igp_init,
        &mut mn,
        &mut dn,
        &mut deployed,
        gas_limit,
        gas_price,
        chain_id,
        "IGP",
    )
    .await?;

    // 10. Optional: synthetic DRC20 route.
    let deployed_warp_drc20 = if let Some((warp_id, warp_bytes)) = warp_drc20_id {
        let warp_init = rkyv_serialize(&(
            mailbox_id,
            owner_h256,
            String::from(warp_name),
            String::from(warp_symbol),
            warp_decimals,
            Vec::<(u32, [u8; 32])>::new(),
        ));
        deploy_one(
            &client,
            &sk,
            &pk,
            warp_bytes,
            warp_init,
            &mut mn,
            &mut dn,
            &mut deployed,
            gas_limit,
            gas_price,
            chain_id,
            "WarpDrc20",
        )
        .await?;
        Some(warp_id)
    } else {
        None
    };

    // 11. Optional: native DUSK collateral route.
    let deployed_warp_native = if let Some((warp_id, warp_bytes)) = warp_native_id {
        let warp_init = rkyv_serialize(&(mailbox_id, owner_h256, Vec::<(u32, [u8; 32])>::new()));
        deploy_one(
            &client,
            &sk,
            &pk,
            warp_bytes,
            warp_init,
            &mut mn,
            &mut dn,
            &mut deployed,
            gas_limit,
            gas_price,
            chain_id,
            "WarpNative",
        )
        .await?;
        Some(warp_id)
    } else {
        None
    };

    // 12. Optional: the pinned canonical DRC20 the collateral route wraps,
    // its whole supply minted to the deployer.
    if let Some((_, token_bytes)) = canonical_token_id {
        let token_init = rkyv_serialize(&canonical_token.init(Drc20Account::moonlight(&pk)));
        deploy_one(
            &client,
            &sk,
            &pk,
            token_bytes,
            token_init,
            &mut mn,
            &mut dn,
            &mut deployed,
            gas_limit,
            gas_price,
            chain_id,
            "CollateralToken",
        )
        .await?;
    }

    // 13. Optional: the DRC20 collateral route.
    let deployed_warp_collateral = if let Some((warp_id, warp_bytes)) = warp_collateral_id {
        let wrapped_token = collateral_token.expect("collateral token missing");
        let warp_init = rkyv_serialize(&(
            wrapped_token,
            mailbox_id,
            owner_h256,
            Vec::<(u32, [u8; 32])>::new(),
        ));
        deploy_one(
            &client,
            &sk,
            &pk,
            warp_bytes,
            warp_init,
            &mut mn,
            &mut dn,
            &mut deployed,
            gas_limit,
            gas_price,
            chain_id,
            "WarpCollateral",
        )
        .await?;
        Some(warp_id)
    } else {
        None
    };

    eprintln!("  All contracts confirmed on-chain");
    let confirmation = confirm_in_order(&client, &deployed).await?;
    if confirmation.final_block_hash.is_some() {
        eprintln!("  Every deploy is in a final block");
    }

    // Nothing is reported as deployed until every contract reads back as
    // what was sent: a deploy or an init someone else front-ran would have
    // chosen another owner or other bindings (deployment.rs).
    let sent = deployment::Sent {
        owner_key: pk_bytes.to_vec(),
        owner: owner_h256,
        domain,
        test_mock: test_mock_id,
        test_recipient: test_recipient_id,
        merkle_tree_hook: merkle_tree_hook_id,
        ism: ism_multisig_id.map(|id| (id, validators.clone(), multisig_threshold)),
        protocol_fee: (protocol_fee_id, PROTOCOL_FEE, MAX_PROTOCOL_FEE),
        aggregation_hook: aggregation_hook_id,
        mailbox: mailbox_id,
        validator_announce: va_id,
        igp: (igp_id, igp_domain_configs.clone()),
        warp_drc20: deployed_warp_drc20.map(|id| {
            (
                id,
                warp_name.to_string(),
                warp_symbol.to_string(),
                warp_decimals,
            )
        }),
        warp_native: deployed_warp_native,
        canonical_token: deployed_canonical_token
            .map(|id| canonical_token.sent(id, Drc20Account::moonlight(&pk))),
        warp_collateral: deployed_warp_collateral
            .map(|id| (id, collateral_token.expect("collateral token missing"))),
    };
    let read_back = deployment::read_back(&client, &sent).await?;
    eprintln!("  Read back {read_back} values: every contract holds what was sent");

    let mut contracts = json!({
        "test_mock": hex::encode(test_mock_id.to_bytes()),
        "test_recipient": hex::encode(test_recipient_id.to_bytes()),
        "merkle_tree_hook": hex::encode(merkle_tree_hook_id.to_bytes()),
        "mailbox": hex::encode(mailbox_id.to_bytes()),
        "validator_announce": hex::encode(va_id.to_bytes()),
        "protocol_fee": hex::encode(protocol_fee_id.to_bytes()),
        "aggregation_hook": hex::encode(aggregation_hook_id.to_bytes()),
        "igp": hex::encode(igp_id.to_bytes()),
    });
    if let Some(ism_id) = ism_multisig_id {
        contracts["ism_multisig"] = json!(hex::encode(ism_id.to_bytes()));
    }
    if let Some(warp_id) = deployed_warp_drc20 {
        contracts["warp_drc20"] = json!(hex::encode(warp_id.to_bytes()));
    }
    if let Some(warp_id) = deployed_warp_native {
        contracts["warp_native"] = json!(hex::encode(warp_id.to_bytes()));
    }
    if let Some(warp_id) = deployed_warp_collateral {
        contracts["warp_drc20_collateral"] = json!(hex::encode(warp_id.to_bytes()));
        if let Some(token) = collateral_token {
            contracts["collateral_token"] = json!(hex::encode(token.to_bytes()));
        }
    }

    let output = json!({
        "success": true,
        "domain": domain,
        "chain_id": chain_id,
        "igp_domain_configs": igp_domain_configs.iter().map(|(destination, config)| json!({
            "domain": destination,
            "gas_overhead": config.gas_overhead,
            "token_exchange_rate": config.token_exchange_rate,
            "gas_price": config.gas_price,
        })).collect::<Vec<_>>(),
        "contracts": contracts,
        // How many values were read back, each as sent.
        "read_back": read_back,
        // Every deploy is final once the last one is.
        "last_tx_id": deployed.last().map(|(tx_id, _)| tx_id),
        "last_block_height": confirmation.inclusion.block_height,
        "finalized": confirmation.final_block_hash.is_some(),
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

// ── cmd_deploy_token_route ──────────────────────────────────────────────────

/// The canonical DRC20 `deploy-hyperlane --warp-collateral-token
/// canonical-drc20` deploys.
#[derive(clap::Args)]
struct CanonicalTokenArgs {
    /// What the canonical DRC20 of `--warp-collateral-token
    /// canonical-drc20` mints to the deployer, in its smallest unit. The
    /// default, 5 tokens at 18 decimals, fits a signed 64-bit integer,
    /// so shell scripts can do arithmetic on it.
    #[arg(long, default_value = "5000000000000000000")]
    collateral_token_supply: u64,
    /// The name of the canonical DRC20 of `--warp-collateral-token
    /// canonical-drc20`.
    #[arg(long, default_value = "Canonical DRC20")]
    collateral_token_name: String,
    /// Its symbol.
    #[arg(long, default_value = "CDRC20")]
    collateral_token_symbol: String,
    /// Its decimals. Defaults to --warp-decimals.
    #[arg(long)]
    collateral_token_decimals: Option<u8>,
}

/// `drc20-mint`'s arguments.
#[derive(clap::Args)]
struct Drc20MintArgs {
    #[arg(long, default_value = "http://localhost:8080/")]
    rues_url: String,
    /// Path to encrypted consensus.keys file.
    #[arg(long)]
    keys: Option<PathBuf>,
    /// Password for consensus.keys. Takes precedence over
    /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
    #[arg(long)]
    password: Option<String>,
    /// Raw BLS secret key hex (alternative to --keys).
    #[arg(long)]
    secret_key: Option<String>,
    /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
    #[arg(long)]
    secret_key_stdin: bool,
    /// The token (64 hex chars).
    #[arg(long)]
    token: String,
    /// The account to mint to: a base58 Moonlight address or a 96-byte
    /// public key in hex. Defaults to the signer.
    #[arg(long)]
    to: Option<String>,
    /// The amount, in the token's smallest unit.
    #[arg(long)]
    amount: u64,
    /// Expected native Dusk chain ID. The endpoint must report it before
    /// anything is queried, the signer is read or a transaction is
    /// constructed.
    #[arg(long)]
    expected_chain_id: u8,
    /// Gas limit. A mint takes about 1.9M gas.
    #[arg(long, default_value_t = MINT_GAS_LIMIT)]
    gas_limit: u64,
    /// Gas price in LUX per unit of gas. Calls only need a price above zero.
    #[arg(long, default_value = "1")]
    gas_price: u64,
}

/// `drc20-info`'s arguments.
#[derive(clap::Args)]
struct Drc20InfoArgs {
    #[arg(long, default_value = "http://localhost:8080/")]
    rues_url: String,
    /// The token (64 hex chars).
    #[arg(long)]
    token: String,
    /// An account to show: a base58 Moonlight address or a 96-byte
    /// public key in hex.
    #[arg(long)]
    account: Option<String>,
}

/// `deploy-token-route`'s arguments.
#[derive(clap::Args)]
struct TokenRouteArgs {
    #[arg(long, default_value = "http://localhost:8080/")]
    rues_url: String,
    /// Path to encrypted consensus.keys file.
    #[arg(long)]
    keys: Option<PathBuf>,
    /// Password for consensus.keys. Takes precedence over
    /// DUSK_CONSENSUS_PASSWORD_FILE and DUSK_CONSENSUS_PASSWORD.
    #[arg(long)]
    password: Option<String>,
    /// Raw BLS secret key hex (alternative to --keys).
    #[arg(long)]
    secret_key: Option<String>,
    /// Read raw BLS secret key hex from stdin (alternative to --keys/--secret-key).
    #[arg(long)]
    secret_key_stdin: bool,
    /// The Mailbox the route sends and takes its messages through (64
    /// hex chars).
    #[arg(long)]
    mailbox: String,
    /// The token's name.
    #[arg(long)]
    token_name: String,
    /// The token's symbol.
    #[arg(long)]
    token_symbol: String,
    /// The token's decimals. A route moves amounts in the token's
    /// smallest unit, unscaled, so its remote token needs the same.
    #[arg(long)]
    token_decimals: u8,
    /// What the token mints to the signer at its init, in its smallest
    /// unit. The signer can mint more (`drc20-mint`).
    #[arg(long)]
    token_supply: u64,
    /// The token's deploy nonce, part of its ID; the route's is the next
    /// one. A deployer's contracts of one Wasm need nonces of their own.
    #[arg(long, value_parser = clap::value_parser!(u64).range(..=LAST_TOKEN_ROUTE_NONCE))]
    deploy_nonce: u64,
    /// A remote domain the route must have no router for yet, besides
    /// the Mailbox's own. Repeat for each.
    #[arg(long = "remote-domain")]
    remote_domains: Vec<u32>,
    /// The ID the token must get (64 hex chars), as --plan printed it.
    /// Nothing is sent if the Wasm, the nonce and the key give another.
    #[arg(long)]
    expected_token: Option<String>,
    /// The ID the route must get, likewise.
    #[arg(long)]
    expected_route: Option<String>,
    /// Print the IDs and whether a contract is at each; sign nothing.
    #[arg(long)]
    plan: bool,
    /// A file each deploy transaction is appended to, as a JSON line
    /// ({"contract", "id", "tx", "nonce"}) written to disk before the
    /// transaction is sent, for the caller to record: a later run takes a
    /// contract at a planned ID only if one of these created it.
    #[arg(long)]
    tx_log: Option<PathBuf>,
    /// The transaction an earlier run sent to deploy the token, as the tx
    /// log recorded it. A contract at the token's ID is taken as this
    /// deployment's only if this transaction created it: it executed, in a
    /// final block. Without it, a contract at the ID is someone else's.
    #[arg(long)]
    token_tx: Option<String>,
    /// The nonce --token-tx is signed with: once the signer's nonce has
    /// passed it, a transaction that isn't in the ledger can't execute.
    #[arg(long, requires = "token_tx")]
    token_tx_nonce: Option<u64>,
    /// The transaction an earlier run sent to deploy the route, likewise.
    #[arg(long)]
    route_tx: Option<String>,
    /// The nonce --route-tx is signed with.
    #[arg(long, requires = "route_tx")]
    route_tx_nonce: Option<u64>,
    /// Directory with compiled WASM contracts.
    #[arg(long)]
    wasm_dir: Option<PathBuf>,
    /// Gas limit per deploy. The token's Wasm is about 262 KB, which
    /// costs 26.2M gas to deploy.
    #[arg(long, default_value = "50000000")]
    gas_limit: u64,
    /// Gas price in LUX per unit of gas. Deploys must pay at least the
    /// network's minimum deployment gas price (2000 on testnet and mainnet).
    #[arg(long, default_value = "2000")]
    gas_price: u64,
    /// Expected native Dusk chain ID. The endpoint must report it before
    /// anything is queried, the signer is read or a transaction is
    /// constructed. Optional with --plan, which signs nothing.
    #[arg(long, required_unless_present = "plan")]
    expected_chain_id: Option<u8>,
}

impl TokenRouteArgs {
    /// The token to deploy.
    fn token(&self) -> CollateralTokenSettings {
        CollateralTokenSettings {
            name: self.token_name.clone(),
            symbol: self.token_symbol.clone(),
            decimals: self.token_decimals,
            supply: self.token_supply,
        }
    }
}

/// The IDs a token route of `token_wasm` and `route_wasm` gets with
/// `deploy_nonce` and the deployer `owner`: the token at the nonce, the
/// route at the next one.
fn token_route_ids(
    token_wasm: &[u8],
    route_wasm: &[u8],
    deploy_nonce: u64,
    owner: &BlsPublicKey,
) -> Result<(ContractId, ContractId), String> {
    let route_nonce = deploy_nonce
        .checked_add(1)
        .ok_or("The route's deploy nonce, the token's plus one, overflows")?;
    let owner = owner.to_bytes();
    Ok((
        gen_contract_id(token_wasm, deploy_nonce, &owner),
        gen_contract_id(route_wasm, route_nonce, &owner),
    ))
}

/// An ID the planned one must be: refuse to go on when the Wasm, the nonce
/// and the key give another.
fn check_planned_id(what: &str, id: ContractId, expected: Option<&str>) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let expected = parse_bytes32(expected)?;
    if id.to_bytes() != expected {
        return Err(format!(
            "The {what}'s Wasm, deploy nonce and the key give the ID {}, not {}, which was planned: \
             nothing was sent. Was the Wasm rebuilt from other sources?",
            hex::encode(id.to_bytes()),
            hex::encode(expected)
        ));
    }
    Ok(())
}

async fn cmd_deploy_token_route(args: TokenRouteArgs) -> Result<(), String> {
    let output = deploy_token_route(args).await?;
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    if output["success"] != json!(true) {
        // A planned ID that isn't this deployment's: the output says which.
        std::process::exit(1);
    }
    Ok(())
}

/// How long a resumed run settles a recorded deploy transaction, its waits
/// for blocks to be final included, without --wait-finality.
const RECORDED_DEPLOY_FINALITY_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(8)
} else {
    Duration::from_secs(300)
};

/// Whose a token route's contract at its planned ID is.
#[derive(Debug, PartialEq, Eq)]
enum Provenance {
    /// Nothing is at the ID: deploy it.
    Free,
    /// This deployment's own recorded transaction created it, and executed
    /// in a final block, with the init this command sends.
    Ours,
    /// Someone else's contract is at the ID, or one no transaction of this
    /// deployment's is known to have created: the reason.
    Taken(String),
}

/// How long a resumed run waits, a couple of blocks, for a recorded deploy
/// transaction that is in neither the ledger nor the mempool, and whose
/// nonce the signer hasn't reached, to show up, before it takes it as never
/// sent: a run that stopped between logging it and sending it, or one the
/// node refused before it took it.
const RECORDED_TX_GRACE: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_secs(30)
};

/// A block, by its height and hash.
type BlockId = (u64, String);

/// How long a fence waits after reading the nonce before it reads the tip
/// again. The node moves its queries to a block's state before it makes the
/// block its tip, so a block it was committing during the nonce read is the
/// tip by then, and the fence starts again.
const FENCE_TIP_DELAY: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_secs(1)
};

/// A fence: the signer's nonce as of an identified block, the node's tip,
/// the same block before and after the nonce is read.
async fn nonce_fence(client: &RuesClient, pk: &BlsPublicKey) -> Result<(u64, BlockId), String> {
    for _ in 0..5 {
        let before = client.tip_block().await?;
        let (nonce, _) = client.query_account(pk).await?;
        tokio::time::sleep(FENCE_TIP_DELAY).await;
        if client.tip_block().await? == before {
            return Ok((nonce, before));
        }
    }
    Err("The node's tip moved on during each of five reads of the signer's nonce".into())
}

/// How a fence's block ended up.
#[derive(Debug, PartialEq, Eq)]
enum FenceBlock {
    /// It is final, so the nonce read at it is.
    Final,
    /// Another block, or none, is at its height: the nonce read at it
    /// proves nothing.
    Replaced,
}

/// Wait until the fence's block, that very block, is final, or is replaced.
/// Once it is final, a transaction whose nonce the fence has reached has
/// executed or failed, in a final block, or never will. The block's hash
/// and label come from one read of the block at its height.
async fn wait_until_final(
    client: &RuesClient,
    block: &BlockId,
    deadline: tokio::time::Instant,
) -> Result<FenceBlock, String> {
    let (height, hash) = block;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "Block {height} ({hash}) wasn't final by the deadline: the signer's nonce can't \
                 be taken as final yet"
            ));
        }
        match client.query_block_status(*height).await {
            Ok(Some(found)) if found.hash != *hash => return Ok(FenceBlock::Replaced),
            Ok(None) => return Ok(FenceBlock::Replaced),
            Ok(Some(found)) if found.is_final() => return Ok(FenceBlock::Final),
            Ok(Some(_)) | Err(TransactionStatusQueryError::Retryable(_)) => {}
            Err(TransactionStatusQueryError::Terminal(error)) => return Err(error),
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        tokio::time::sleep(TRANSACTION_POLL_INTERVAL.min(left)).await;
    }
}

/// Whose the contract at a planned ID is, from the transaction an earlier
/// run `recorded` as deploying it, with its nonce. A deploy to an ID that
/// holds a contract fails, so a recorded transaction that executed created
/// the contract that is there, with the init it carries. Without one, or
/// with one that failed or never executed, a contract at the ID was
/// deployed by someone else: its init, and who holds what in it, are
/// theirs, whatever it reads back.
///
/// The transaction is read after the signer's nonce, and the contract after
/// the transaction, so that one executing between the reads is seen: once
/// the nonce has reached the transaction's, as of a final block, the reads
/// after it see how the transaction ended. A failure counts only once its
/// block is final. A recorded transaction that is in neither the ledger nor
/// the mempool, with its nonce not reached, a couple of blocks on, was never
/// sent: it is marked void in the tx log, and the ID deployed afresh.
async fn provenance(
    client: &RuesClient,
    what: &str,
    id_hex: &str,
    recorded: Option<(&str, Option<u64>)>,
    pk: &BlsPublicKey,
    tx_log: Option<&std::path::Path>,
) -> Result<Provenance, String> {
    let exists = || async { Ok::<_, String>(client.contract_owner(id_hex).await?.is_some()) };
    let Some((tx_id, tx_nonce)) = recorded else {
        return Ok(if exists().await? {
            Provenance::Taken(
                "a contract is at its planned ID that no transaction of this deployment's created"
                    .to_string(),
            )
        } else {
            Provenance::Free
        });
    };
    let tx_nonce = tx_nonce.ok_or_else(|| {
        format!("Transaction {tx_id}, recorded as deploying the {what}, has no nonce recorded")
    })?;
    let status = |tx_id: &str| {
        let tx_id = tx_id.to_owned();
        async move {
            client.query_transaction_status(&tx_id).await.map_err(|e| {
                format!("Can't read transaction {tx_id}, recorded as deploying the {what}: {e}")
            })
        }
    };
    // One deadline for settling it, each fence read again after a replaced
    // block included.
    let timeout = finality_wait().unwrap_or(RECORDED_DEPLOY_FINALITY_TIMEOUT);
    let deadline = tokio::time::Instant::now() + timeout;
    let grace = tokio::time::Instant::now() + RECORDED_TX_GRACE;
    let mut mempool_waits = 0;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err(format!(
                "Transaction {tx_id}, recorded as deploying the {what}, wasn't settled within {}s: \
                 nothing was sent. Run again.",
                timeout.as_secs()
            ));
        }
        let (fence, fence_block) = nonce_fence(client, pk).await?;
        let found = status(tx_id).await?;
        let there = exists().await?;
        match found {
            TransactionStatus::Executed(_) => {
                wait_for_finality(client, tx_id, left).await?;
                if !there {
                    return Err(format!(
                        "Transaction {tx_id}, recorded as deploying the {what}, executed, but no \
                         contract is at the {what}'s ID: nothing was sent"
                    ));
                }
                return Ok(Provenance::Ours);
            }
            TransactionStatus::Failed { error, .. } => {
                match wait_for_finality(client, tx_id, left).await {
                    Err(e) if e.starts_with(&format!("Transaction {tx_id} failed: ")) => {}
                    Err(e) => return Err(e),
                    // It executed after all, in another block: look again.
                    Ok(_) => continue,
                }
                return Ok(if exists().await? {
                    Provenance::Taken(format!(
                        "this deployment's transaction {tx_id} deploying it failed ({error}), \
                         and another contract is at its planned ID"
                    ))
                } else {
                    Provenance::Free
                });
            }
            TransactionStatus::NotFound if there => {
                // The transaction may have executed since it was read.
                if !matches!(status(tx_id).await?, TransactionStatus::NotFound) {
                    continue;
                }
                return Ok(Provenance::Taken(format!(
                    "a contract is at its planned ID, and this deployment's transaction {tx_id} \
                     deploying it never executed"
                )));
            }
            TransactionStatus::NotFound if fence >= tx_nonce => {
                // Its nonce went to another transaction. Once that is final,
                // it never executes.
                if wait_until_final(client, &fence_block, deadline).await? == FenceBlock::Replaced {
                    // The nonce was read at a block that is gone: prove it again.
                    continue;
                }
                if !matches!(status(tx_id).await?, TransactionStatus::NotFound) {
                    continue;
                }
                return Ok(Provenance::Free);
            }
            TransactionStatus::NotFound => {
                if client.in_mempool(tx_id).await? {
                    mempool_waits += 1;
                    if mempool_waits > 3 {
                        return Err(format!(
                            "Transaction {tx_id}, recorded as deploying the {what}, is still in \
                             the mempool: nothing was sent. Run again once it is in a block."
                        ));
                    }
                    // Wait for it to land, then look again.
                    let _ = wait_for_inclusion(client, tx_id).await;
                    continue;
                }
                if tokio::time::Instant::now() < grace {
                    tokio::time::sleep(TRANSACTION_POLL_INTERVAL.min(RECORDED_TX_GRACE)).await;
                    continue;
                }
                eprintln!(
                    "  Transaction {tx_id}, recorded as deploying the {what}, is in neither the \
                     ledger nor the mempool, and the signer's nonce hasn't reached its own: it \
                     was never sent. Marked void; deploying the {what} afresh."
                );
                if let Some(path) = tx_log {
                    append_tx_log(
                        path,
                        &json!({"contract": what, "id": id_hex, "tx": tx_id, "nonce": tx_nonce, "void": true}),
                    )?;
                }
                return Ok(Provenance::Free);
            }
        }
    }
}

/// Append a deploy transaction to the tx log, on disk, before it is sent.
/// A last line a run left unfinished is ended first, so it can't run into
/// this one.
fn append_tx_log(path: &std::path::Path, entry: &serde_json::Value) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let error = |e: std::io::Error| format!("Can't write the tx log {}: {e}", path.display());
    let mut file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
        .map_err(error)?;
    let length = file.metadata().map_err(error)?.len();
    let mut separator = "";
    if length > 0 {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::Start(length - 1))
            .and_then(|_| file.read_exact(&mut last))
            .map_err(error)?;
        if last[0] != b'\n' {
            separator = "\n";
        }
    }
    writeln!(file, "{separator}{entry}")
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(error)?;
    // The log is in its directory for good, after a crash of the host too,
    // only once the directory is synced: so it is on each append, which
    // also catches up a first append that stopped before it.
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => std::path::Path::new("."),
    };
    sync_directory(directory).map_err(error)
}

/// Have the entries of `directory` on disk.
fn sync_directory(directory: &std::path::Path) -> std::io::Result<()> {
    #[cfg(test)]
    tests::DIRECTORY_SYNCS.with(|syncs| syncs.borrow_mut().push(directory.to_path_buf()));
    fs::File::open(directory)?.sync_all()
}

/// Deploy one of a token route's contracts, after appending its transaction
/// to `tx_log`, and wait until it is included. Returns the transaction's
/// hash and its outcome: its inclusion, or the error it failed with.
#[allow(clippy::too_many_arguments)]
async fn deploy_logged(
    client: &RuesClient,
    sk: &BlsSecretKey,
    pk: &BlsPublicKey,
    bytecode: Vec<u8>,
    init_args: Vec<u8>,
    moonlight_nonce: &mut u64,
    deploy_nonce: u64,
    gas_limit: u64,
    gas_price: u64,
    chain_id: u8,
    name: &str,
    logged: (&str, &str, Option<&std::path::Path>),
) -> Result<(String, Result<Inclusion, String>), String> {
    let (what, id_hex, tx_log) = logged;
    eprintln!("  Deploying {name} ({} bytes)...", bytecode.len());
    *moonlight_nonce = next_moonlight_nonce(*moonlight_nonce)?;
    let tx = moonlight_deployment(
        sk,
        bytecode,
        pk,
        init_args,
        gas_limit,
        gas_price,
        *moonlight_nonce,
        deploy_nonce,
        chain_id,
    )?;
    let tx_id = hex::encode(tx.hash().to_bytes());
    let tx_bytes = tx.to_var_bytes();
    if let Some(path) = tx_log {
        append_tx_log(
            path,
            &json!({"contract": what, "id": id_hex, "tx": tx_id, "nonce": *moonlight_nonce}),
        )?;
    }
    propagate(client, &tx_id, &tx_bytes)
        .await
        .map_err(|e| format!("Failed to deploy {name}: {e}"))?;
    match wait_for_inclusion(client, &tx_id).await {
        Ok(inclusion) => {
            eprintln!("  {name} TX {tx_id} executed");
            Ok((tx_id, Ok(inclusion)))
        }
        Err(error) => match client.query_transaction_status(&tx_id).await {
            Ok(TransactionStatus::Failed { error, .. }) => Ok((tx_id, Err(error))),
            _ => Err(format!("Failed to deploy {name}: {error}")),
        },
    }
}

/// `deploy-token-route`'s output for a planned ID that isn't this
/// deployment's: nothing more is sent, and the caller plans other IDs.
fn taken_output(mut output: serde_json::Value, what: &str, reason: &str) -> serde_json::Value {
    let id = output["contracts"][what]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    output["success"] = json!(false);
    output["taken"] = json!(what);
    output["error"] = json!(format!(
        "The {what}'s planned ID {id} isn't this deployment's: {reason}. Nothing more was sent: \
         plan the route at other deploy nonces."
    ));
    output
}

/// Plan or deploy a token route, and return `deploy-token-route`'s output.
async fn deploy_token_route(args: TokenRouteArgs) -> Result<serde_json::Value, String> {
    let token_settings = args.token();
    token_settings.check()?;
    let mailbox_bytes = parse_bytes32(&args.mailbox)?;
    let mailbox = ContractId::from_bytes(mailbox_bytes);
    for expected in [
        &args.expected_token,
        &args.expected_route,
        &args.token_tx,
        &args.route_tx,
    ]
    .into_iter()
    .flatten()
    {
        parse_bytes32(expected)?;
    }
    let client = RuesClient::new(&args.rues_url)?;
    // The chain every deploy is signed for, before the key is read. A plan
    // signs nothing.
    let chain_id = match args.expected_chain_id {
        Some(expected) if !args.plan => Some(bind_chain_id(&client, expected).await?),
        _ => None,
    };
    let (sk, pk) = load_keys(
        args.keys,
        args.password.as_deref().unwrap_or_default(),
        args.secret_key,
        args.secret_key_stdin,
    )?;
    let wasm_dir = args.wasm_dir.unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/contract/wasm32-unknown-unknown/release")
    });
    let read = |name: &str, hint: &str| {
        let path = wasm_dir.join(name);
        fs::read(&path).map_err(|e| format!("Failed to read {} ({hint}): {e}", path.display()))
    };
    let token_wasm = read(
        CANONICAL_DRC20_WASM,
        "make canonical-drc20-fixture builds it",
    )?;
    let route_wasm = read(
        "hyperlane_dusk_warp_drc20_collateral.wasm",
        "make all builds it",
    )?;
    let (token, route) = token_route_ids(&token_wasm, &route_wasm, args.deploy_nonce, &pk)?;
    check_planned_id("token", token, args.expected_token.as_deref())?;
    check_planned_id("route", route, args.expected_route.as_deref())?;
    let (token_hex, route_hex) = (hex::encode(token.to_bytes()), hex::encode(route.to_bytes()));
    let exists = |owner: Option<Vec<u8>>| owner.is_some();
    let token_exists = exists(client.contract_owner(&token_hex).await?);
    let route_exists = exists(client.contract_owner(&route_hex).await?);
    let mut output = json!({
        "success": true,
        "deploy_nonce": args.deploy_nonce,
        "contracts": {"token": token_hex, "route": route_hex},
        "exists": {"token": token_exists, "route": route_exists},
    });
    if args.plan {
        return Ok(output);
    }
    let chain_id = chain_id.ok_or("--expected-chain-id is required without --plan")?;

    // The route's bindings are read back for the Mailbox's own domain too,
    // which a Mailbox must answer.
    let domain: u32 = client
        .contract_query(&mailbox_bytes, "local_domain", &())
        .await
        .map_err(|e| {
            format!(
                "Can't read the local domain of the Mailbox {}: {e}",
                args.mailbox
            )
        })?;
    let mut router_domains = vec![domain];
    for remote in &args.remote_domains {
        if !router_domains.contains(remote) {
            router_domains.push(*remote);
        }
    }

    eprintln!(
        "Deploying the token {} ({}) and its collateral route, at deploy nonces {} and {}",
        token_settings.symbol,
        token_settings.name,
        args.deploy_nonce,
        args.deploy_nonce + 1
    );
    // A contract already at a planned ID is this deployment's only if a
    // transaction an earlier run recorded created it. Both are settled
    // before anything is sent.
    let token_state = provenance(
        &client,
        "token",
        &token_hex,
        args.token_tx.as_deref().map(|tx| (tx, args.token_tx_nonce)),
        &pk,
        args.tx_log.as_deref(),
    )
    .await?;
    if let Provenance::Taken(reason) = &token_state {
        return Ok(taken_output(output, "token", reason));
    }
    let route_state = provenance(
        &client,
        "route",
        &route_hex,
        args.route_tx.as_deref().map(|tx| (tx, args.route_tx_nonce)),
        &pk,
        args.tx_log.as_deref(),
    )
    .await?;
    if let Provenance::Taken(reason) = &route_state {
        return Ok(taken_output(output, "route", reason));
    }
    let shown = |state: &Provenance| match state {
        Provenance::Ours => " (already there, deployed by this deployment's recorded transaction)",
        _ => "",
    };
    eprintln!("  Token: {token_hex}{}", shown(&token_state));
    eprintln!("  Route: {route_hex}{}", shown(&route_state));
    let deployer = Drc20Account::moonlight(&pk);
    let owner_h256 = hyperlane_dusk_types::message::keccak256(&pk.to_bytes());
    let token_free = token_state == Provenance::Free;
    let route_free = route_state == Provenance::Free;
    let mut moonlight_nonce = if token_free || route_free {
        client.query_account(&pk).await?.0
    } else {
        0
    };
    let mut deployed = Vec::new();
    let tx_log = args.tx_log.as_deref();
    // The token first: the route's init names it. Each deploy waits for the
    // one before, as deploy-hyperlane's do. A deploy that fails because a
    // contract got to its ID first leaves that ID to someone else.
    let route_init = rkyv_serialize(&(token, mailbox, owner_h256, Vec::<(u32, [u8; 32])>::new()));
    for (what, free, id_hex, wasm, init, nonce, name) in [
        (
            "token",
            token_free,
            &token_hex,
            token_wasm,
            rkyv_serialize(&token_settings.init(deployer)),
            args.deploy_nonce,
            "Token",
        ),
        (
            "route",
            route_free,
            &route_hex,
            route_wasm,
            route_init,
            args.deploy_nonce + 1,
            "WarpCollateral",
        ),
    ] {
        if !free {
            continue;
        }
        let (tx_id, outcome) = deploy_logged(
            &client,
            &sk,
            &pk,
            wasm,
            init,
            &mut moonlight_nonce,
            nonce,
            args.gas_limit,
            args.gas_price,
            chain_id,
            name,
            (what, id_hex, tx_log),
        )
        .await?;
        match outcome {
            Ok(inclusion) => deployed.push((tx_id, inclusion)),
            Err(error) if client.contract_owner(id_hex).await?.is_some() => {
                return Ok(taken_output(
                    output,
                    what,
                    &format!(
                        "this deployment's transaction {tx_id} deploying it failed ({error}): \
                         another contract got to its ID first"
                    ),
                ));
            }
            Err(error) => {
                return Err(format!(
                    "Failed to deploy {name}: Transaction {tx_id} failed: {error}"
                ))
            }
        }
    }
    if !deployed.is_empty() {
        let confirmation = confirm_in_order(&client, &deployed).await?;
        output["last_tx_id"] = json!(deployed.last().map(|(tx_id, _)| tx_id));
        output["last_block_height"] = json!(confirmation.inclusion.block_height);
        output["finalized"] = json!(confirmation.final_block_hash.is_some());
    }

    // Nothing is reported until both read back as what this command sent.
    let sent = deployment::TokenRoute {
        owner_key: pk.to_bytes().to_vec(),
        owner: owner_h256,
        mailbox,
        router_domains: router_domains.clone(),
        token: token_settings.sent(token, deployer),
        route,
    };
    let read_back = deployment::read_back(&client, &sent).await?;
    eprintln!("  Read back {read_back} values: both contracts hold what was sent");
    output["chain_id"] = json!(chain_id);
    output["mailbox"] = json!(hex::encode(mailbox_bytes));
    output["domain"] = json!(domain);
    output["router_domains"] = json!(router_domains);
    output["deployed"] = json!({"token": token_free, "route": route_free});
    output["token"] = json!({
        "name": token_settings.name,
        "symbol": token_settings.symbol,
        "decimals": token_settings.decimals,
        "supply": token_settings.supply.to_string(),
    });
    output["read_back"] = json!(read_back);
    Ok(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefaultIsm {
    TestMock,
    MessageIdMultisig,
}

fn parse_default_ism(s: &str) -> Result<DefaultIsm, String> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("testMock") || s.eq_ignore_ascii_case("test_mock") {
        return Ok(DefaultIsm::TestMock);
    }
    if s.eq_ignore_ascii_case("messageIdMultisig") || s.eq_ignore_ascii_case("message_id_multisig")
    {
        return Ok(DefaultIsm::MessageIdMultisig);
    }
    Err(format!(
        "Invalid --default-ism '{s}'. Options: testMock, messageIdMultisig"
    ))
}

fn parse_igp_domain_configs(values: &[String]) -> Result<Vec<(u32, DomainGasConfig)>, String> {
    if values.is_empty() {
        return Err("At least one --igp-domain-config is required".into());
    }

    let mut configs = Vec::with_capacity(values.len());
    for value in values {
        let parts = value.split(':').collect::<Vec<_>>();
        if parts.len() != 4 {
            return Err(format!(
                "Invalid --igp-domain-config '{value}': expected DOMAIN:GAS_OVERHEAD:TOKEN_EXCHANGE_RATE:GAS_PRICE"
            ));
        }
        let parse = |raw: &str, field: &str| {
            raw.parse::<u64>()
                .map_err(|e| format!("Invalid {field} in --igp-domain-config '{value}': {e}"))
        };
        let domain_u64 = parse(parts[0], "domain")?;
        let domain = u32::try_from(domain_u64)
            .map_err(|_| format!("IGP domain {domain_u64} exceeds u32"))?;
        let config = DomainGasConfig {
            gas_overhead: parse(parts[1], "gas overhead")?,
            token_exchange_rate: parse(parts[2], "token exchange rate")?,
            gas_price: parse(parts[3], "gas price")?,
        };
        if config.token_exchange_rate == 0 {
            return Err(format!(
                "IGP domain {domain} token exchange rate cannot be zero"
            ));
        }
        if config.gas_price == 0 {
            return Err(format!("IGP domain {domain} gas price cannot be zero"));
        }
        validate_igp_domain_config(domain, config)?;
        configs.push((domain, config));
    }
    configs.sort_by_key(|(domain, _)| *domain);
    for adjacent in configs.windows(2) {
        if adjacent[0].0 == adjacent[1].0 {
            return Err(format!("Duplicate IGP domain config: {}", adjacent[0].0));
        }
    }
    Ok(configs)
}

fn validate_igp_domain_config(domain: u32, config: DomainGasConfig) -> Result<(), String> {
    config
        .gas_overhead
        .checked_add(IGP_MAX_GAS_LIMIT)
        .ok_or_else(|| format!("IGP domain {domain} configured adjusted gas exceeds u64"))?;
    let quote = |gas_limit: u64| {
        (u128::from(gas_limit) + u128::from(config.gas_overhead))
            .checked_mul(u128::from(config.gas_price))
            .and_then(|value| value.checked_mul(u128::from(config.token_exchange_rate)))
            .map(|value| value / IGP_TOKEN_EXCHANGE_RATE_SCALE)
    };
    let minimum = quote(1)
        .ok_or_else(|| format!("IGP domain {domain} configured quote arithmetic overflows"))?;
    if minimum == 0 {
        return Err(format!(
            "IGP domain {domain} configured payment rounds to zero"
        ));
    }
    let maximum = quote(IGP_MAX_GAS_LIMIT)
        .ok_or_else(|| format!("IGP domain {domain} configured quote arithmetic overflows"))?;
    u64::try_from(maximum)
        .map(|_| ())
        .map_err(|_| format!("IGP domain {domain} configured quote exceeds u64"))
}

fn parse_eth_addresses(list: &str) -> Result<Vec<EthAddress>, String> {
    let mut addresses = parse_eth_addresses_in_order(list)?;
    addresses.sort();
    Ok(addresses)
}

fn parse_eth_addresses_in_order(list: &str) -> Result<Vec<EthAddress>, String> {
    let mut addresses = Vec::new();
    for raw in list.split(',').map(str::trim).filter(|raw| !raw.is_empty()) {
        let address = parse_eth_address(raw)?;
        if addresses.contains(&address) {
            return Err(format!(
                "Duplicate validator address: 0x{}",
                hex::encode(address.0)
            ));
        }
        addresses.push(address);
    }
    Ok(addresses)
}

fn parse_u32_pair(value: &str) -> Result<(u32, u32), String> {
    let mut parts = value.split(',');
    let first = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| "Expected --arg-u32-pair FIRST,SECOND".to_string())?
        .parse::<u32>()
        .map_err(|_| "FIRST in --arg-u32-pair is not a u32".to_string())?;
    let second = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| "Expected --arg-u32-pair FIRST,SECOND".to_string())?
        .parse::<u32>()
        .map_err(|_| "SECOND in --arg-u32-pair is not a u32".to_string())?;
    if parts.next().is_some() {
        return Err("Expected exactly two values in --arg-u32-pair FIRST,SECOND".to_string());
    }
    Ok((first, second))
}

fn parse_eth_address(s: &str) -> Result<EthAddress, String> {
    let s = s.trim();
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() != 40 {
        return Err(format!(
            "Invalid Ethereum address '{s}': expected 20 bytes (40 hex chars)"
        ));
    }
    let bytes = hex::decode(s).map_err(|e| format!("Invalid Ethereum address hex: {e}"))?;
    if bytes.len() != 20 {
        return Err(format!(
            "Invalid Ethereum address '{s}': expected 20 bytes, got {}",
            bytes.len()
        ));
    }
    let mut arr = [0u8; 20];
    arr.copy_from_slice(&bytes);
    Ok(EthAddress(arr))
}

/// Deploy a single contract via Moonlight deployment TX and wait until it is
/// included. Appends the transaction's hash and inclusion to `deployed`.
async fn deploy_one(
    client: &RuesClient,
    sk: &BlsSecretKey,
    pk: &BlsPublicKey,
    bytecode: Vec<u8>,
    init_args: Vec<u8>,
    moonlight_nonce: &mut u64,
    deploy_nonce: &mut u64,
    deployed: &mut Vec<(String, Inclusion)>,
    gas_limit: u64,
    gas_price: u64,
    chain_id: u8,
    name: &str,
) -> Result<(), String> {
    eprintln!("  Deploying {name} ({} bytes)...", bytecode.len());
    *moonlight_nonce = next_moonlight_nonce(*moonlight_nonce)?;
    let mn = *moonlight_nonce;
    let dn = *deploy_nonce;
    *deploy_nonce = deploy_nonce
        .checked_add(1)
        .ok_or_else(|| "Contract deployment nonce is exhausted".to_string())?;

    let tx = moonlight_deployment(
        sk, bytecode, pk, init_args, gas_limit, gas_price, mn, dn, chain_id,
    )?;
    let tx_id = hex::encode(tx.hash().to_bytes());
    let tx_bytes = tx.to_var_bytes();
    let inclusion = async {
        propagate(client, &tx_id, &tx_bytes).await?;
        wait_for_inclusion(client, &tx_id).await
    }
    .await
    .map_err(|e| format!("Failed to deploy {name}: {e}"))?;
    eprintln!("  {name} TX {tx_id} executed");
    deployed.push((tx_id, inclusion));
    Ok(())
}

/// The gas a deploy of `bytecode_len` bytes is charged for its bytecode,
/// 100 a byte, plus 10M for the rest.
fn deploy_wasm_gas_limit(bytecode_len: usize) -> u64 {
    (bytecode_len as u64)
        .saturating_mul(100)
        .saturating_add(10_000_000)
}

#[allow(clippy::too_many_arguments)]
async fn cmd_deploy_wasm(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    wasm: &std::path::Path,
    owner_arg: Option<&str>,
    deploy_nonce: u64,
    expected_id: Option<&str>,
    expected_chain_id: u8,
    check_multicall: bool,
    gas_limit: Option<u64>,
    gas_price: u64,
) -> Result<(), String> {
    let bytecode =
        fs::read(wasm).map_err(|e| format!("Can't read the Wasm {}: {e}", wasm.display()))?;
    let expected_id = expected_id.map(parse_bytes32).transpose()?;
    let owner = owner_arg.map(parse_account_arg).transpose()?;
    // With the owner given, the ID is checked before anything else.
    if let (Some(owner), Some(expected)) = (&owner, &expected_id) {
        check_deploy_id(&bytecode, deploy_nonce, owner, expected)?;
    }
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    let signer = match &owner {
        Some(_) => None,
        None => Some(load_keys(
            keys_path.clone(),
            password,
            secret_key_hex.clone(),
            secret_key_stdin,
        )?),
    };
    let owner = owner.unwrap_or_else(|| signer.as_ref().expect("loaded without an owner").1);
    let contract_id = match expected_id {
        Some(expected) => check_deploy_id(&bytecode, deploy_nonce, &owner, &expected)?,
        None => gen_contract_id(&bytecode, deploy_nonce, &owner.to_bytes()).to_bytes(),
    };
    let contract_hex = hex::encode(contract_id);
    let mut output = json!({
        "success": true,
        "contract_id": contract_hex,
        "owner": account_identity(&owner)["address"].clone(),
        "deploy_nonce": deploy_nonce,
        "wasm_bytes": bytecode.len(),
    });
    // An existing contract at the expected ID is not trusted without
    // verification: it is taken only when it checks out, and the command
    // fails, with nothing sent, when it doesn't or can't be checked.
    let existing = client.contract_owner(&contract_hex).await.map_err(|e| {
        format!(
            "Can't tell whether a contract is at {contract_hex}, or whose it is: {e}. \
             Nothing was sent"
        )
    })?;
    if let Some(found) = existing {
        let verified = verify_deployed(
            &client,
            &contract_id,
            &found,
            &owner,
            check_multicall,
            chain_id,
        )
        .await
        .map_err(|e| {
            format!(
                "A contract is already at {contract_hex}, but {e}. An existing contract at \
                         the expected ID is not trusted without verification: nothing was sent"
            )
        })?;
        output["deployed"] = json!(false);
        output["verified"] = json!(verified);
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
        return Ok(());
    }

    let (sk, pk) = match signer {
        Some(signer) => signer,
        None => load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?,
    };
    let gas_limit = gas_limit.unwrap_or_else(|| deploy_wasm_gas_limit(bytecode.len()));
    let (account_nonce, balance) = client.query_account(&pk).await?;
    let gas = gas_limit
        .checked_mul(gas_price)
        .ok_or("The gas limit at its gas price doesn't fit a u64")?;
    if balance < gas {
        return Err(format!(
            "The signer holds {balance} LUX, less than the {gas} the deploy's gas limit needs up front"
        ));
    }
    let tx = moonlight_deployment(
        &sk,
        bytecode,
        &owner,
        Vec::new(),
        gas_limit,
        gas_price,
        next_moonlight_nonce(account_nonce)?,
        deploy_nonce,
        chain_id,
    )?;
    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;
    let found = client.contract_owner(&contract_hex).await?.ok_or_else(|| {
        format!("Transaction {tx_id} was included, but there is no contract at {contract_hex}")
    })?;
    let verified = verify_deployed(
        &client,
        &contract_id,
        &found,
        &owner,
        check_multicall,
        chain_id,
    )
    .await
    .map_err(|e| {
        format!("Transaction {tx_id} was included, but the contract at {contract_hex}: {e}")
    })?;
    output["deployed"] = json!(true);
    output["verified"] = json!(verified);
    output["tx_id"] = json!(tx_id);
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

/// Check the contract at `contract_id`, whose owner Rusk reports as
/// `found`: it must be `owner`, byte for byte, and with `check_multicall`
/// the contract must answer as Multicall for Dusk. Returns what was checked.
async fn verify_deployed(
    client: &RuesClient,
    contract_id: &[u8; 32],
    found: &[u8],
    owner: &BlsPublicKey,
    check_multicall: bool,
    chain_id: u8,
) -> Result<Vec<&'static str>, String> {
    if found != owner.to_bytes().as_slice() {
        return Err(format!(
            "Rusk reports its owner as {}, not {}",
            describe_owner(found),
            account_identity(owner)["address"]
                .as_str()
                .unwrap_or_default()
        ));
    }
    if !check_multicall {
        return Ok(vec!["owner"]);
    }
    multicall::check(client, contract_id, chain_id)
        .await
        .map_err(|e| format!("it doesn't answer as Multicall for Dusk: {e}"))?;
    Ok(vec!["owner", "version", "aggregate"])
}

/// An owner as Rusk reports it: a Moonlight address when it is a public
/// key's 96 bytes, otherwise its length and first bytes.
fn describe_owner(owner: &[u8]) -> String {
    if owner.len() == 96 {
        return bs58::encode(owner).into_string();
    }
    let shown = &owner[..owner.len().min(32)];
    let more = if owner.len() > shown.len() { "..." } else { "" };
    format!("{} bytes ({}{more})", owner.len(), hex::encode(shown))
}

/// The ID a deploy of `bytecode` with `nonce` and `owner` gives, which must
/// be `expected`.
fn check_deploy_id(
    bytecode: &[u8],
    nonce: u64,
    owner: &BlsPublicKey,
    expected: &[u8; 32],
) -> Result<[u8; 32], String> {
    let id = gen_contract_id(bytecode, nonce, &owner.to_bytes()).to_bytes();
    if &id != expected {
        return Err(format!(
            "The Wasm, deploy nonce {nonce} and owner give the contract ID {}, not {}: nothing was sent",
            hex::encode(id),
            hex::encode(expected)
        ));
    }
    Ok(id)
}

/// Parse a 32-byte hex string into a fixed-size array.
fn parse_bytes32(hex_str: &str) -> Result<[u8; 32], String> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(hex_str).map_err(|e| format!("Invalid hex: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!("Expected 32 bytes (64 hex), got {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

fn next_moonlight_nonce(current: u64) -> Result<u64, String> {
    current
        .checked_add(1)
        .ok_or_else(|| "Moonlight account nonce is exhausted".to_string())
}

/// What dusk-tx saw of a transaction it sent: the block it is in, the gas
/// it used, and whether that block is final.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Confirmation {
    inclusion: Inclusion,
    /// The block's hash, once the block is final and so can't be reverted.
    /// `None`: only the transaction's inclusion was seen.
    final_block_hash: Option<String>,
}

impl Confirmation {
    fn included(inclusion: Inclusion) -> Self {
        Self {
            inclusion,
            final_block_hash: None,
        }
    }

    /// Add what was seen to a command's JSON output, next to its other
    /// fields.
    fn record(&self, output: &mut serde_json::Value) {
        output["block_height"] = json!(self.inclusion.block_height);
        output["gas_spent"] = json!(self.inclusion.gas_spent);
        output["finalized"] = json!(self.final_block_hash.is_some());
        output["block_hash"] = json!(self.final_block_hash);
    }
}

/// `--wait-finality`'s timeout, or `None` to only report finality.
fn finality_wait() -> Option<Duration> {
    FINALITY_WAIT.get().copied().flatten()
}

/// Submit a transaction, wait until it is included, and say whether its
/// block is final; with `--wait-finality`, wait until it is. Every error
/// keeps the exact hash to reconcile.
async fn propagate_and_wait(
    client: &RuesClient,
    tx_id: &str,
    tx_bytes: &[u8],
) -> Result<Confirmation, String> {
    propagate_and_wait_with(client, tx_id, tx_bytes, false, None).await
}

/// [`propagate_and_wait`], submitting with [`propagate_with`].
async fn propagate_and_wait_with(
    client: &RuesClient,
    tx_id: &str,
    tx_bytes: &[u8],
    queue_future_nonce: bool,
    ack: Option<SendAck>,
) -> Result<Confirmation, String> {
    propagate_with(client, tx_id, tx_bytes, queue_future_nonce, ack).await?;
    match finality_wait() {
        Some(timeout) => wait_for_finality(client, tx_id, timeout).await,
        None => {
            let inclusion = wait_for_inclusion(client, tx_id).await?;
            Ok(check_finality(client, tx_id, inclusion).await)
        }
    }
}

/// Submit a transaction while preserving its exact reconciliation hash.
async fn propagate(client: &RuesClient, tx_id: &str, tx_bytes: &[u8]) -> Result<(), String> {
    propagate_with(client, tx_id, tx_bytes, false, None).await
}

/// [`propagate`]. With `queue_future_nonce`, a preverify refusal only
/// because the nonce is ahead of the account's next one doesn't stop the
/// transaction: the node holds it until the nonces before it arrive. With
/// `ack`, the transaction reaches the node, preverify included, only on the
/// caller's go-ahead; without one it fails, with `Not submitted: <reason>`
/// on stderr, and the node never gets it.
async fn propagate_with(
    client: &RuesClient,
    tx_id: &str,
    tx_bytes: &[u8],
    queue_future_nonce: bool,
    ack: Option<SendAck>,
) -> Result<(), String> {
    // Emit the locally computed reconciliation identity before the first
    // remote write. This gives an interrupted operator a concrete hash to
    // reconcile before deciding whether a non-idempotent operation may be
    // retried.
    eprintln!("{}", prepared_line(tx_id, moonlight_nonce(tx_bytes)));
    if let Some(ack) = ack {
        // The caller answers once it has that hash, so the line must be out
        // of any buffer before the wait starts.
        let refusal = match std::io::Write::flush(&mut std::io::stderr()) {
            Ok(()) => ack.wait().await.err(),
            Err(error) => Some(format!(
                "writing the prepared line to stderr failed: {error}"
            )),
        };
        if let Some(reason) = refusal {
            eprintln!("Not submitted: {reason}");
            return Err(not_submitted_error(tx_id, &reason));
        }
    }
    client
        .propagate_tx(tx_bytes, queue_future_nonce)
        .await
        .map_err(|error| submission_error_with_hash(tx_id, &error))
}

/// The line that names a transaction before anything sends it: its hash
/// and, for a Moonlight transaction, the nonce it is signed with, so a
/// caller can tell when it can no longer execute: once a transaction with
/// that nonce is in a final block. The nonce comes after the hash's `;`, so
/// a caller that reads the hash as everything before the first `;` reads
/// it as before.
fn prepared_line(tx_id: &str, nonce: Option<u64>) -> String {
    let nonce = nonce
        .map(|nonce| format!(" nonce {nonce};"))
        .unwrap_or_default();
    format!(
        "  Prepared TX {tx_id};{nonce} reconcile this exact hash before retrying if submission is interrupted"
    )
}

/// The nonce a transaction's bytes are signed with, if they are a Moonlight
/// transaction.
fn moonlight_nonce(tx_bytes: &[u8]) -> Option<u64> {
    match Transaction::from_slice(tx_bytes) {
        Ok(Transaction::Moonlight(tx)) => Some(tx.nonce()),
        _ => None,
    }
}

/// Wait for the exact transaction to be persisted and fail closed on a
/// contract execution error. A Moonlight nonce also advances for failed
/// executions, so nonce polling alone cannot establish success.
async fn wait_for_inclusion(client: &RuesClient, tx_id: &str) -> Result<Inclusion, String> {
    wait_for_transaction_with(
        tx_id,
        || client.query_transaction_status(tx_id),
        TRANSACTION_POLL_INTERVAL,
        INCLUSION_TIMEOUT,
    )
    .await
    .map_err(|error| confirmation_error_with_hash(tx_id, &error))
}

/// One look at whether an included transaction's block is final. It gives
/// up after `FINALITY_CHECK_TIMEOUT` and never fails: the transaction is
/// included either way, and only a block seen final counts as final.
async fn check_finality(client: &RuesClient, tx_id: &str, inclusion: Inclusion) -> Confirmation {
    tokio::time::timeout(
        FINALITY_CHECK_TIMEOUT,
        finality_check_with(
            inclusion,
            || client.query_transaction_status(tx_id),
            |height| client.query_block_status(height),
        ),
    )
    .await
    .unwrap_or_else(|_| Confirmation::included(inclusion))
}

/// Poll until the transaction's block is final (`wait_for_finality_with`).
async fn wait_for_finality(
    client: &RuesClient,
    tx_id: &str,
    timeout: Duration,
) -> Result<Confirmation, String> {
    wait_for_finality_with(
        tx_id,
        || client.query_transaction_status(tx_id),
        |height| client.query_block_status(height),
        TRANSACTION_POLL_INTERVAL,
        timeout,
    )
    .await
    .map_err(|error| confirmation_error_with_hash(tx_id, &error))
}

/// Confirm transactions one signer sent in nonce order, each already seen
/// included: report or wait for the finality of the last. Finality comes in
/// height order, and an earlier nonce can't be in a later block, so once
/// the last is final every earlier one is in a final block too. With
/// `--wait-finality`, check that each of those still executed.
async fn confirm_in_order(
    client: &RuesClient,
    transactions: &[(String, Inclusion)],
) -> Result<Confirmation, String> {
    let ((last_tx_id, last), earlier) = transactions
        .split_last()
        .ok_or("No transaction to confirm")?;
    let Some(timeout) = finality_wait() else {
        return Ok(check_finality(client, last_tx_id, *last).await);
    };
    let confirmation = wait_for_finality(client, last_tx_id, timeout).await?;
    for (tx_id, _) in earlier {
        match client.query_transaction_status(tx_id).await {
            Ok(TransactionStatus::Executed(_)) => {}
            Ok(TransactionStatus::Failed { error, .. }) => {
                return Err(format!("Transaction {tx_id} failed: {error}"));
            }
            Ok(TransactionStatus::NotFound) => {
                return Err(confirmation_error_with_hash(
                    tx_id,
                    "it is not in the ledger, though a later transaction is final",
                ));
            }
            Err(error) => return Err(confirmation_error_with_hash(tx_id, &error.to_string())),
        }
    }
    Ok(confirmation)
}

async fn wait_for_transaction_with<F, Fut>(
    tx_id: &str,
    mut query_status: F,
    poll_interval: Duration,
    timeout: Duration,
) -> Result<Inclusion, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<TransactionStatus, TransactionStatusQueryError>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last_query_error = None;
    let mut attempt = 0usize;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        attempt = attempt.saturating_add(1);

        match tokio::time::timeout(remaining, query_status()).await {
            Ok(Ok(TransactionStatus::Executed(inclusion))) => return Ok(inclusion),
            Ok(Ok(TransactionStatus::Failed { error, .. })) => {
                return Err(format!("Transaction {tx_id} failed: {error}"));
            }
            Ok(Ok(TransactionStatus::NotFound)) => {}
            Ok(Err(TransactionStatusQueryError::Retryable(error))) => {
                last_query_error = Some(error);
            }
            Ok(Err(TransactionStatusQueryError::Terminal(error))) => {
                return Err(format!(
                    "Transaction {tx_id} status response is incompatible: {error}"
                ));
            }
            Err(_) => break,
        }

        if attempt % 5 == 0 {
            eprintln!("  [transaction pending, attempt {attempt}; waiting until deadline]");
        }

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(poll_interval.min(remaining)).await;
    }

    let detail = last_query_error
        .map(|error| format!("; last status query error: {error}"))
        .unwrap_or_default();
    Err(format!(
        "Transaction {tx_id} was not confirmed within {}s{detail}",
        timeout.as_secs()
    ))
}

/// Whether the block an included transaction is in is final. The node reads
/// a block and its label separately from the transaction, so after seeing
/// the block at the transaction's height final, read the transaction again:
/// a final block can't change, so if the ledger still has the transaction
/// at that height, it is in that block for good.
async fn finality_check_with<S, SF, B, BF>(
    inclusion: Inclusion,
    mut query_status: S,
    mut query_block: B,
) -> Confirmation
where
    S: FnMut() -> SF,
    SF: Future<Output = Result<TransactionStatus, TransactionStatusQueryError>>,
    B: FnMut(u64) -> BF,
    BF: Future<Output = Result<Option<BlockStatus>, TransactionStatusQueryError>>,
{
    let Ok(Some(block)) = query_block(inclusion.block_height).await else {
        return Confirmation::included(inclusion);
    };
    if !block.is_final() {
        return Confirmation::included(inclusion);
    }
    match query_status().await {
        Ok(TransactionStatus::Executed(now)) if now.block_height == inclusion.block_height => {
            Confirmation {
                inclusion: now,
                final_block_hash: Some(block.hash),
            }
        }
        _ => Confirmation::included(inclusion),
    }
}

/// Poll a submitted transaction until the block it is in is final, then
/// report it, or its execution error, as final. Until then a revert can drop
/// the transaction or run it again in another block, with another result,
/// so neither inclusion nor an execution error is final before its block
/// is. Errors name the transaction; the caller adds how to reconcile it.
async fn wait_for_finality_with<S, SF, B, BF>(
    tx_id: &str,
    mut query_status: S,
    mut query_block: B,
    poll_interval: Duration,
    timeout: Duration,
) -> Result<Confirmation, String>
where
    S: FnMut() -> SF,
    SF: Future<Output = Result<TransactionStatus, TransactionStatusQueryError>>,
    B: FnMut(u64) -> BF,
    BF: Future<Output = Result<Option<BlockStatus>, TransactionStatusQueryError>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    let incompatible =
        |error: String| format!("Transaction {tx_id} status response is incompatible: {error}");
    let mut seen = String::from("not in a block");
    let mut last_query_error = None;
    let mut attempt = 0usize;

    'poll: loop {
        attempt = attempt.saturating_add(1);
        let Some(status) = before(deadline, query_status()).await else {
            break;
        };
        match status {
            Err(TransactionStatusQueryError::Terminal(error)) => return Err(incompatible(error)),
            Err(TransactionStatusQueryError::Retryable(error)) => last_query_error = Some(error),
            Ok(TransactionStatus::NotFound) => seen = String::from("not in a block"),
            Ok(status) => {
                let inclusion = status.inclusion().expect("an included status");
                let height = inclusion.block_height;
                let failed = if matches!(status, TransactionStatus::Failed { .. }) {
                    ", failed"
                } else {
                    ""
                };
                let Some(block) = before(deadline, query_block(height)).await else {
                    break;
                };
                match block {
                    Err(TransactionStatusQueryError::Terminal(error)) => {
                        return Err(incompatible(error));
                    }
                    Err(TransactionStatusQueryError::Retryable(error)) => {
                        last_query_error = Some(error);
                    }
                    Ok(None) => seen = format!("in block {height}, which the node no longer has"),
                    Ok(Some(block)) if !block.is_final() => {
                        seen = format!("in block {height} ({}{failed})", block.label);
                    }
                    Ok(Some(block)) => {
                        let Some(now) = before(deadline, query_status()).await else {
                            break 'poll;
                        };
                        match now {
                            Ok(TransactionStatus::Executed(now)) if now.block_height == height => {
                                return Ok(Confirmation {
                                    inclusion: now,
                                    final_block_hash: Some(block.hash),
                                });
                            }
                            Ok(TransactionStatus::Failed { error, inclusion })
                                if inclusion.block_height == height =>
                            {
                                return Err(format!("Transaction {tx_id} failed: {error}"));
                            }
                            Err(TransactionStatusQueryError::Terminal(error)) => {
                                return Err(incompatible(error));
                            }
                            Err(TransactionStatusQueryError::Retryable(error)) => {
                                last_query_error = Some(error);
                            }
                            // Moved since: look again.
                            Ok(_) => seen = format!("moved out of block {height}"),
                        }
                    }
                }
            }
        }

        if attempt.is_multiple_of(5) {
            eprintln!("  [waiting for finality, attempt {attempt}: {seen}]");
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(poll_interval.min(remaining)).await;
    }

    let detail = last_query_error
        .map(|error| format!("; last query error: {error}"))
        .unwrap_or_default();
    Err(format!(
        "Transaction {tx_id} was not final within {}s: last seen {seen}{detail}",
        timeout.as_secs()
    ))
}

/// `future`'s output, or `None` if `deadline` passes first.
async fn before<T>(deadline: tokio::time::Instant, future: impl Future<Output = T>) -> Option<T> {
    tokio::time::timeout_at(deadline, future).await.ok()
}

// ── cmd_query ─────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn cmd_query(
    rues_url: &str,
    contract_hex: &str,
    method: &str,
    return_type: &str,
    arg_u32: Option<u32>,
    arg_u64: Option<u64>,
    arg_u32_pair: Option<&str>,
    arg_bytes32: Option<String>,
    arg_eth_address: Option<String>,
) -> Result<(), String> {
    let client = RuesClient::new(rues_url)?;
    let contract_id = parse_bytes32(contract_hex)?;
    // --arg-u64 goes with --arg-u32 (clap enforces that) as one argument.
    let argument_count = usize::from(arg_u32.is_some())
        + usize::from(arg_u32_pair.is_some())
        + usize::from(arg_bytes32.is_some())
        + usize::from(arg_eth_address.is_some());
    if argument_count > 1 {
        return Err("Query accepts at most one argument option".to_string());
    }
    if arg_u64.is_some() && return_type != "u64" {
        return Err("--arg-u64 is supported only with --return-type u64".to_string());
    }
    let arg_u32_pair = arg_u32_pair.map(parse_u32_pair).transpose()?;
    if method == "get_announced_validators" {
        let (_, limit) = arg_u32_pair.ok_or_else(|| {
            "get_announced_validators requires --arg-u32-pair START,LIMIT".to_string()
        })?;
        if return_type != "eth-address-list" {
            return Err(
                "get_announced_validators requires --return-type eth-address-list".to_string(),
            );
        }
        if !(1..=MAX_VALIDATOR_ANNOUNCE_PAGE).contains(&limit) {
            return Err(format!(
                "get_announced_validators LIMIT must be 1 to {MAX_VALIDATOR_ANNOUNCE_PAGE}"
            ));
        }
    } else if arg_u32_pair.is_some() && !matches!(return_type, "eth-address-list" | "bytes32-list")
    {
        return Err(
            "--arg-u32-pair is supported only for eth-address-list and bytes32-list queries"
                .to_string(),
        );
    }

    match return_type {
        "u8" => {
            let result: u8 = if let Some(val) = arg_u32 {
                let arg = u8::try_from(val).map_err(|_| "u8 query argument is out of range")?;
                client.contract_query(&contract_id, method, &arg).await?
            } else {
                client.contract_query(&contract_id, method, &()).await?
            };
            let output = json!({ "success": true, "value": result });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        "u32" => {
            let result: u32 = if let Some(val) = arg_u32 {
                client.contract_query(&contract_id, method, &val).await?
            } else if let Some(ref hex) = arg_bytes32 {
                let arg = parse_bytes32(hex)?;
                client.contract_query(&contract_id, method, &arg).await?
            } else {
                client.contract_query(&contract_id, method, &()).await?
            };
            let output = json!({ "success": true, "value": result });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        "u64" => {
            let result: u64 = if let (Some(first), Some(second)) = (arg_u32, arg_u64) {
                client
                    .contract_query(&contract_id, method, &(first, second))
                    .await?
            } else if let Some(val) = arg_u32 {
                client.contract_query(&contract_id, method, &val).await?
            } else if let Some(ref hex) = arg_bytes32 {
                let arg = parse_bytes32(hex)?;
                client.contract_query(&contract_id, method, &arg).await?
            } else {
                client.contract_query(&contract_id, method, &()).await?
            };
            let output = json!({ "success": true, "value": result });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        "bool" => {
            let result: bool = if let Some(ref hex) = arg_bytes32 {
                let arg = parse_bytes32(hex)?;
                client.contract_query(&contract_id, method, &arg).await?
            } else if let Some(val) = arg_u32 {
                client.contract_query(&contract_id, method, &val).await?
            } else {
                client.contract_query(&contract_id, method, &()).await?
            };
            let output = json!({ "success": true, "value": result });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        "bytes32" => {
            let result: [u8; 32] =
                match prepare_bytes32_query_argument(arg_u32, arg_bytes32.as_deref())? {
                    Bytes32QueryArgument::U32(value) => {
                        client.contract_query(&contract_id, method, &value).await?
                    }
                    Bytes32QueryArgument::Bytes32(value) => {
                        client.contract_query(&contract_id, method, &value).await?
                    }
                    Bytes32QueryArgument::Unit => {
                        client.contract_query(&contract_id, method, &()).await?
                    }
                };
            let output = json!({ "success": true, "value": hex::encode(result) });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        "option-bytes32" => {
            let result: Option<[u8; 32]> = client.contract_query(&contract_id, method, &()).await?;
            let output = json!({
                "success": true,
                "value": result.map(hex::encode),
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        "contract-id-list" => {
            let result: Vec<ContractId> = client.contract_query(&contract_id, method, &()).await?;
            let output = json!({
                "success": true,
                "value": result
                    .iter()
                    .map(|id| hex::encode(id.to_bytes()))
                    .collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        "eth-address-list" => {
            let result: Vec<EthAddress> = if let Some(pair) = arg_u32_pair {
                client.contract_query(&contract_id, method, &pair).await?
            } else {
                client.contract_query(&contract_id, method, &()).await?
            };
            let output = json!({
                "success": true,
                "value": result
                    .iter()
                    .map(|address| format!("0x{}", hex::encode(address.0)))
                    .collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        "bytes32-list" => {
            let result: Vec<[u8; 32]> = if let Some(pair) = arg_u32_pair {
                client.contract_query(&contract_id, method, &pair).await?
            } else if let Some(address) = arg_eth_address.as_deref() {
                let address = parse_eth_address(address)?;
                client
                    .contract_query(&contract_id, method, &address)
                    .await?
            } else {
                return Err(
                    "--return-type bytes32-list requires --arg-u32-pair or --arg-eth-address"
                        .to_string(),
                );
            };
            let output = json!({
                "success": true,
                "value": result.iter().map(hex::encode).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        "validators-and-threshold" => {
            let result: (Vec<EthAddress>, u8) =
                client.contract_query(&contract_id, method, &()).await?;
            let output = json!({
                "success": true,
                "value": {
                    "validators": result.0
                        .iter()
                        .map(|address| format!("0x{}", hex::encode(address.0)))
                        .collect::<Vec<_>>(),
                    "threshold": result.1,
                },
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        "bytes" => {
            let result: Vec<u8> = if let Some(val) = arg_u32 {
                client.contract_query(&contract_id, method, &val).await?
            } else if let Some(ref hex) = arg_bytes32 {
                let arg = parse_bytes32(hex)?;
                client.contract_query(&contract_id, method, &arg).await?
            } else {
                client.contract_query(&contract_id, method, &()).await?
            };
            let output = json!({
                "success": true,
                "value": hex::encode(&result),
                "length": result.len(),
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        "string" => {
            let result: String = if let Some(val) = arg_u32 {
                client.contract_query(&contract_id, method, &val).await?
            } else {
                client.contract_query(&contract_id, method, &()).await?
            };
            let output = json!({ "success": true, "value": result });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        "domain-gas-config" => {
            let domain = arg_u32.ok_or_else(|| {
                "--return-type domain-gas-config requires --arg-u32 <domain>".to_string()
            })?;
            let result: DomainGasConfig =
                client.contract_query(&contract_id, method, &domain).await?;
            let output = json!({
                "success": true,
                "value": {
                    "gas_overhead": result.gas_overhead,
                    "token_exchange_rate": result.token_exchange_rate,
                    "gas_price": result.gas_price,
                }
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        _ => {
            return Err(format!(
                "Unsupported return type: {return_type}. Use: u8, u32, u64, bool, bytes32, option-bytes32, contract-id-list, eth-address-list, bytes32-list, validators-and-threshold, bytes, string, domain-gas-config"
            ));
        }
    }

    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum Bytes32QueryArgument {
    U32(u32),
    Bytes32([u8; 32]),
    Unit,
}

fn prepare_bytes32_query_argument(
    arg_u32: Option<u32>,
    arg_bytes32: Option<&str>,
) -> Result<Bytes32QueryArgument, String> {
    if let Some(value) = arg_u32 {
        Ok(Bytes32QueryArgument::U32(value))
    } else if let Some(value) = arg_bytes32 {
        Ok(Bytes32QueryArgument::Bytes32(parse_bytes32(value)?))
    } else {
        Ok(Bytes32QueryArgument::Unit)
    }
}

// ── cmd_dispatch ──────────────────────────────────────────────────────────

async fn cmd_dispatch(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    mailbox_hex: &str,
    test_recipient_hex: &str,
    destination: u32,
    recipient_hex: &str,
    body: &str,
    expected_chain_id: u8,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let (test_recipient_id, dispatch_args) = prepare_dispatch_call(
        mailbox_hex,
        test_recipient_hex,
        destination,
        recipient_hex,
        body,
    )?;
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let (nonce, _balance) = client.query_account(&pk).await?;

    let tx = moonlight_call(
        &sk,
        test_recipient_id,
        "dispatch_message",
        dispatch_args,
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;

    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;

    let mut output = json!({
        "success": true,
        "fn_name": "dispatch_message",
        "test_recipient": test_recipient_hex,
        "destination": destination,
        "recipient": recipient_hex,
        "tx_id": tx_id,
    });
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

fn prepare_dispatch_call(
    mailbox_hex: &str,
    test_recipient_hex: &str,
    destination: u32,
    recipient_hex: &str,
    body: &str,
) -> Result<(ContractId, Vec<u8>), String> {
    let mailbox_id = ContractId::from_bytes(parse_bytes32(mailbox_hex)?);
    let test_recipient_id = ContractId::from_bytes(parse_bytes32(test_recipient_hex)?);
    let recipient = parse_bytes32(recipient_hex)?;
    let body_bytes = if let Some(body_hex) = body.strip_prefix("0x") {
        hex::decode(body_hex).map_err(|e| format!("Invalid body hex: {e}"))?
    } else {
        body.as_bytes().to_vec()
    };
    let dispatch_args = rkyv_serialize(&(mailbox_id, destination, recipient, body_bytes));
    if dispatch_args.len() > MAX_CALL_ARGS_BYTES {
        return Err(format!(
            "Dispatch arguments exceed the {MAX_CALL_ARGS_BYTES}-byte helper transport limit"
        ));
    }
    Ok((test_recipient_id, dispatch_args))
}

// ── cmd_process ───────────────────────────────────────────────────────────

async fn cmd_process(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    mailbox_hex: &str,
    message_hex: &str,
    metadata_hex: &str,
    expected_chain_id: u8,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let mailbox_id = ContractId::from_bytes(parse_bytes32(mailbox_hex)?);
    let (process_args, message_id) = prepare_process_call(message_hex, metadata_hex)?;

    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let (nonce, _balance) = client.query_account(&pk).await?;

    let tx = moonlight_call(
        &sk,
        mailbox_id,
        "process",
        process_args,
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;

    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;

    let mut output = json!({
        "success": true,
        "fn_name": "process",
        "mailbox": mailbox_hex,
        "message_id": hex::encode(message_id),
        "tx_id": tx_id,
    });
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

fn prepare_process_call(
    message_hex: &str,
    metadata_hex: &str,
) -> Result<(Vec<u8>, [u8; 32]), String> {
    let message_hex = message_hex.strip_prefix("0x").unwrap_or(message_hex);
    let encoded_message =
        hex::decode(message_hex).map_err(|e| format!("Invalid message hex: {e}"))?;
    let metadata_hex = metadata_hex.strip_prefix("0x").unwrap_or(metadata_hex);
    let metadata = hex::decode(metadata_hex).map_err(|e| format!("Invalid metadata hex: {e}"))?;

    // Compute message ID for output
    let message_id = hyperlane_dusk_types::message::id(&encoded_message);

    // Serialize args: (metadata: Vec<u8>, encoded_message: Vec<u8>)
    let process_args = rkyv_serialize(&(metadata, encoded_message));
    if process_args.len() > MAX_CALL_ARGS_BYTES {
        return Err(format!(
            "Process arguments exceed the {MAX_CALL_ARGS_BYTES}-byte helper transport limit"
        ));
    }
    Ok((process_args, message_id))
}

// ── cmd_encode_message ────────────────────────────────────────────────────

fn cmd_encode_message(
    version: u8,
    nonce: u32,
    origin: u32,
    sender_hex: &str,
    destination: u32,
    recipient_hex: &str,
    body_hex: &str,
) -> Result<(), String> {
    let sender = parse_bytes32(sender_hex)?;
    let recipient = parse_bytes32(recipient_hex)?;

    let body_hex = body_hex.strip_prefix("0x").unwrap_or(body_hex);
    let body = if body_hex.is_empty() {
        Vec::new()
    } else {
        hex::decode(body_hex).map_err(|e| format!("Invalid body hex: {e}"))?
    };

    let encoded = hyperlane_dusk_types::message::encode(
        version,
        nonce,
        origin,
        sender,
        destination,
        recipient,
        &body,
    );
    let message_id = hyperlane_dusk_types::message::id(&encoded);

    let output = json!({
        "success": true,
        "encoded": hex::encode(&encoded),
        "message_id": hex::encode(message_id),
        "length": encoded.len(),
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

// ── cmd_enroll_router ─────────────────────────────────────────────────────

async fn cmd_enroll_router(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    warp_contract_hex: &str,
    domain: u32,
    router_hex: &str,
    expected_chain_id: u8,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let warp_id = ContractId::from_bytes(parse_bytes32(warp_contract_hex)?);
    let router = parse_bytes32(router_hex)?;

    // Serialize args: (domain: u32, router: H256)
    let enroll_args = rkyv_serialize(&(domain, router));

    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let (nonce, _balance) = client.query_account(&pk).await?;

    let tx = moonlight_call(
        &sk,
        warp_id,
        "enroll_remote_router",
        enroll_args,
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;

    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;

    let mut output = json!({
        "success": true,
        "fn_name": "enroll_remote_router",
        "warp_contract": warp_contract_hex,
        "domain": domain,
        "router": router_hex,
        "tx_id": tx_id,
    });
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

// ── cmd_claim_pending ─────────────────────────────────────────────────────

/// The route call a claim makes.
#[derive(Debug, PartialEq)]
struct ClaimCall {
    /// `claim_pending` or `claim_pending_for`.
    fn_name: &'static str,
    /// The call's rkyv-encoded arguments.
    args: Vec<u8>,
    /// The account paid, whose escrow is claimed.
    account: BlsPublicKey,
    gas_limit: u64,
}

/// The signer's own escrow is claimed with `claim_pending`, which pays and
/// registers the transaction's sender and costs about 1.9M gas. Only
/// another account's needs `claim_pending_for`, whose on-chain key decoding
/// takes about 59M. `for_account` naming the signer counts as the signer's
/// own claim: both keys are decoded, so equal compressed bytes are the same
/// key. An explicit `gas_limit` overrides either default.
fn claim_call(
    signer: &BlsPublicKey,
    for_account: Option<BlsPublicKey>,
    gas_limit: Option<u64>,
) -> ClaimCall {
    match for_account {
        Some(account) if account.to_bytes() != signer.to_bytes() => ClaimCall {
            fn_name: "claim_pending_for",
            args: rkyv_serialize(&(account,)),
            account,
            gas_limit: gas_limit.unwrap_or(CLAIM_PENDING_FOR_GAS_LIMIT),
        },
        _ => ClaimCall {
            fn_name: "claim_pending",
            args: Vec::new(),
            account: *signer,
            gas_limit: gas_limit.unwrap_or(CLAIM_PENDING_GAS_LIMIT),
        },
    }
}

#[allow(clippy::too_many_arguments)]
async fn cmd_claim_pending(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    warp_contract_hex: &str,
    for_account: Option<&str>,
    expected_chain_id: u8,
    gas_limit: Option<u64>,
    gas_price: u64,
) -> Result<(), String> {
    let account = for_account.map(parse_account_arg).transpose()?;
    let warp_bytes = parse_bytes32(warp_contract_hex)?;
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let claim = claim_call(&pk, account, gas_limit);
    let target = claim.account;
    let identity = account_identity(&target);

    // Check first: a call that fails on chain is charged its whole gas limit.
    let h256 = hyperlane_dusk_types::message::keccak256(&target.to_bytes());
    let pending: u64 = client
        .contract_query(&warp_bytes, "pending_balance", &(h256,))
        .await
        .map_err(|e| {
            format!("Can't read the escrow of {warp_contract_hex} (is it a warp route?): {e}")
        })?;
    if pending == 0 {
        return Err(format!(
            "Nothing is in escrow for {} on this route",
            identity["address"].as_str().unwrap_or_default()
        ));
    }

    // Either call also registers the account, so later transfers to it are
    // paid out directly.
    let (nonce, _balance) = client.query_account(&pk).await?;
    let tx = moonlight_call(
        &sk,
        ContractId::from_bytes(warp_bytes),
        claim.fn_name,
        claim.args,
        claim.gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;
    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;

    let mut output = identity;
    output["success"] = json!(true);
    output["fn_name"] = json!(claim.fn_name);
    output["warp_contract"] = json!(warp_contract_hex);
    output["claimed"] = json!(pending.to_string());
    output["tx_id"] = json!(tx_id);
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

// ── Fee contracts: pricing, beneficiary, claims ──────────────────────────

/// One `DOMAIN:GAS_OVERHEAD:TOKEN_EXCHANGE_RATE:GAS_PRICE` pricing, checked
/// as the IGP checks it.
fn parse_igp_domain_config(value: &str) -> Result<(u32, DomainGasConfig), String> {
    let mut configs = parse_igp_domain_configs(&[value.to_owned()])?;
    Ok(configs.remove(0))
}

/// What an IGP with this pricing charges, in LUX, for `gas_limit` gas, as
/// its `quote_gas_payment` computes it: (gas_limit + overhead) x gas price x
/// exchange rate / 1e10. `None` if that doesn't fit a u64.
fn igp_quote(config: DomainGasConfig, gas_limit: u64) -> Option<u64> {
    let gas = u128::from(gas_limit) + u128::from(config.gas_overhead);
    let lux = gas
        .checked_mul(u128::from(config.gas_price))?
        .checked_mul(u128::from(config.token_exchange_rate))?
        / IGP_TOKEN_EXCHANGE_RATE_SCALE;
    u64::try_from(lux).ok()
}

/// A destination's pricing, with what it charges a message without hook
/// metadata, which the IGP reads as 50,000 gas. The warp routes send none.
fn domain_gas_config_json(config: DomainGasConfig) -> serde_json::Value {
    json!({
        "gas_overhead": config.gas_overhead,
        "token_exchange_rate": config.token_exchange_rate,
        "gas_price": config.gas_price,
        "quote_default_gas": igp_quote(config, hyperlane_dusk_types::metadata::DEFAULT_GAS_USAGE),
    })
}

/// Name a ProtocolFee or IGP contract by its hook type, or fail.
async fn fee_contract_kind(
    client: &RuesClient,
    contract: &[u8; 32],
    contract_hex: &str,
    accept_protocol_fee: bool,
) -> Result<&'static str, String> {
    let hook_type: u8 = client
        .contract_query(contract, "hook_type", &())
        .await
        .map_err(|e| format!("Can't query {contract_hex} (is it a fee contract?): {e}"))?;
    match hook_type {
        IGP_HOOK_TYPE => Ok("IGP"),
        PROTOCOL_FEE_HOOK_TYPE if accept_protocol_fee => Ok("ProtocolFee"),
        _ if accept_protocol_fee => Err(format!(
            "{contract_hex} has hook type {hook_type}: neither an IGP ({IGP_HOOK_TYPE}) nor a ProtocolFee ({PROTOCOL_FEE_HOOK_TYPE})"
        )),
        _ => Err(format!(
            "{contract_hex} has hook type {hook_type}, not an IGP's ({IGP_HOOK_TYPE})"
        )),
    }
}

/// Fail unless the signer owns `contract`: an owner-only call from anyone
/// else fails on chain and is charged its whole gas limit.
async fn ensure_signer_owns(
    client: &RuesClient,
    contract: &[u8; 32],
    contract_hex: &str,
    signer: &BlsPublicKey,
) -> Result<(), String> {
    let owner: Option<[u8; 32]> = client
        .contract_query(contract, "owner", &())
        .await
        .map_err(|e| format!("Can't read the owner of {contract_hex}: {e}"))?;
    let signer_h256 = hyperlane_dusk_types::message::keccak256(&signer.to_bytes());
    match owner {
        Some(owner) if owner == signer_h256 => Ok(()),
        Some(owner) => Err(format!(
            "The signer (account hash {}) doesn't own {contract_hex}; its owner is {}",
            hex::encode(signer_h256),
            hex::encode(owner)
        )),
        None => Err(format!("{contract_hex} has no owner")),
    }
}

#[allow(clippy::too_many_arguments)]
async fn cmd_set_igp_config(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    igp_hex: &str,
    config_arg: &str,
    expected_chain_id: u8,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let (domain, config) = parse_igp_domain_config(config_arg)?;
    let igp = parse_bytes32(igp_hex)?;
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    fee_contract_kind(&client, &igp, igp_hex, false).await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    ensure_signer_owns(&client, &igp, igp_hex, &pk).await?;
    let previous: DomainGasConfig = client
        .contract_query(&igp, "domain_gas_config", &domain)
        .await?;

    let (nonce, _balance) = client.query_account(&pk).await?;
    let tx = moonlight_call(
        &sk,
        ContractId::from_bytes(igp),
        "set_domain_gas_config",
        rkyv_serialize(&(domain, config)),
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;
    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;

    let current: DomainGasConfig = client
        .contract_query(&igp, "domain_gas_config", &domain)
        .await?;
    if current != config {
        return Err(format!(
            "Transaction {tx_id} was included, but the IGP now prices domain {domain} at \
             {current:?}, not {config:?}"
        ));
    }
    let mut output = json!({
        "success": true,
        "fn_name": "set_domain_gas_config",
        "igp": igp_hex,
        "domain": domain,
        "previous": domain_gas_config_json(previous),
        "config": domain_gas_config_json(config),
        "tx_id": tx_id,
    });
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn cmd_set_beneficiary(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    contract_hex: &str,
    beneficiary_arg: &str,
    expected_chain_id: u8,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let account = parse_account_arg(beneficiary_arg)?;
    let beneficiary = hyperlane_dusk_types::message::keccak256(&account.to_bytes());
    let contract = parse_bytes32(contract_hex)?;
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    let kind = fee_contract_kind(&client, &contract, contract_hex, true).await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    ensure_signer_owns(&client, &contract, contract_hex, &pk).await?;
    let previous: [u8; 32] = client.contract_query(&contract, "beneficiary", &()).await?;

    let mut output = account_identity(&account);
    output["success"] = json!(true);
    output["contract"] = json!(contract_hex);
    output["kind"] = json!(kind);
    output["previous_beneficiary"] = json!(hex::encode(previous));
    if previous == beneficiary {
        output["unchanged"] = json!(true);
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
        return Ok(());
    }

    let (nonce, _balance) = client.query_account(&pk).await?;
    let tx = moonlight_call(
        &sk,
        ContractId::from_bytes(contract),
        "set_beneficiary",
        rkyv_serialize(&(beneficiary,)),
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;
    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;
    let current: [u8; 32] = client.contract_query(&contract, "beneficiary", &()).await?;
    if current != beneficiary {
        return Err(format!(
            "Transaction {tx_id} was included, but the beneficiary of {kind} {contract_hex} is \
             now {}",
            hex::encode(current)
        ));
    }
    output["fn_name"] = json!("set_beneficiary");
    output["unchanged"] = json!(false);
    output["tx_id"] = json!(tx_id);
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn cmd_claim_fees(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    contract_hex: &str,
    expected_chain_id: u8,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let contract = parse_bytes32(contract_hex)?;
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    let kind = fee_contract_kind(&client, &contract, contract_hex, true).await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let identity = account_identity(&pk);

    // Check first: a claim that fails on chain is charged its whole gas
    // limit. The contract pays the account that signs, and only if it is
    // the beneficiary.
    let beneficiary: [u8; 32] = client.contract_query(&contract, "beneficiary", &()).await?;
    let signer_h256 = hyperlane_dusk_types::message::keccak256(&pk.to_bytes());
    if beneficiary != signer_h256 {
        return Err(format!(
            "The signer (account hash {}) isn't the beneficiary of {kind} {contract_hex}, {}. \
             Sign with the beneficiary's key, or have the owner change it with set-beneficiary.",
            hex::encode(signer_h256),
            hex::encode(beneficiary)
        ));
    }
    let claimable: u64 = client
        .contract_query(&contract, "claimable_fees", &())
        .await?;
    if claimable == 0 {
        return Err(format!("{kind} {contract_hex} holds no fees to claim"));
    }

    let (nonce, balance) = client.query_account(&pk).await?;
    let tx = moonlight_call(
        &sk,
        ContractId::from_bytes(contract),
        "claim",
        Vec::new(),
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;
    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;
    let (_, balance_after) = client.query_account(&pk).await?;

    let mut output = identity;
    output["success"] = json!(true);
    output["fn_name"] = json!("claim");
    output["contract"] = json!(contract_hex);
    output["kind"] = json!(kind);
    output["claimed"] = json!(claimable.to_string());
    output["claimed_dusk"] = json!(format_dusk(claimable));
    output["balance_before"] = json!(balance.to_string());
    output["balance_after"] = json!(balance_after.to_string());
    output["tx_id"] = json!(tx_id);
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

// ── pay-for-gas ──────────────────────────────────────────────────────────

/// Which message `pay-for-gas` pays for.
#[derive(Debug, PartialEq, Eq)]
enum TopUpMessage {
    /// An ID and a destination as given, unchecked, as `payForGas` takes
    /// them.
    Id { id: String, destination: u32 },
    /// The message the IGP's Mailbox dispatched at `nonce`, checked against
    /// `destination` when one is given.
    Nonce {
        nonce: u32,
        destination: Option<u32>,
    },
}

/// What `pay_for_gas` charges for `gas_amount` gas to `destination` at
/// `config`, as the IGP computes it: gas x gas price x exchange rate / 1e10,
/// rounded down, with no overhead. An error, saying why, where the IGP
/// would refuse the payment.
fn top_up_quote(config: DomainGasConfig, gas_amount: u64, destination: u32) -> Result<u64, String> {
    if gas_amount == 0 {
        return Err("--gas-amount must be at least 1".into());
    }
    if gas_amount > IGP_MAX_GAS_LIMIT {
        return Err(format!(
            "--gas-amount {gas_amount} is more than the IGP prices, {IGP_MAX_GAS_LIMIT}"
        ));
    }
    // The IGP only stores pricing with both above zero, and reports an
    // unpriced domain as all zeros.
    if config.gas_price == 0 || config.token_exchange_rate == 0 {
        return Err(format!(
            "The IGP has no pricing for domain {destination}, so it takes no payment for it"
        ));
    }
    let lux = u128::from(gas_amount)
        .checked_mul(u128::from(config.gas_price))
        .and_then(|cost| cost.checked_mul(u128::from(config.token_exchange_rate)))
        .map(|cost| cost / IGP_TOKEN_EXCHANGE_RATE_SCALE)
        .and_then(|lux| u64::try_from(lux).ok())
        .ok_or_else(|| {
            format!("The price of {gas_amount} gas to domain {destination} doesn't fit a u64")
        })?;
    if lux == 0 {
        return Err(format!(
            "{gas_amount} gas to domain {destination} costs less than 1 LUX, which the IGP \
             refuses: pay for more gas"
        ));
    }
    Ok(lux)
}

/// The deposit `pay-for-gas` sends: `max_payment`, or the quote.
fn top_up_deposit(quote: u64, max_payment: Option<u64>) -> Result<u64, String> {
    let deposit = max_payment.unwrap_or(quote);
    if deposit < quote {
        return Err(format!(
            "--max-payment {deposit} LUX is less than the IGP's quote, {quote} LUX"
        ));
    }
    Ok(deposit)
}

/// Fail unless `balance` covers `deposit` and the whole gas limit at its
/// price, which the node holds back before running the transaction.
fn ensure_balance_covers(
    balance: u64,
    deposit: u64,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let gas = gas_limit
        .checked_mul(gas_price)
        .ok_or("The gas limit at its gas price doesn't fit a u64")?;
    let needed = deposit
        .checked_add(gas)
        .ok_or("The deposit and the gas don't fit a u64")?;
    if balance < needed {
        return Err(format!(
            "The signer holds {balance} LUX, less than the {needed} the transaction needs: the \
             {deposit} LUX deposit and the gas limit at its price, {gas} LUX"
        ));
    }
    Ok(())
}

/// The IGP's record of the top-up a transaction included at
/// `block_height` made with `deposit`: the latest record there for this
/// message, destination and gas amount that the deposit paid for. The IGP
/// charges its price when the transaction runs, so this says what was paid.
async fn find_top_up_record(
    client: &RuesClient,
    igp: &[u8; 32],
    block_height: u64,
    (message_id, destination, gas_amount): ([u8; 32], u32, u64),
    deposit: u64,
) -> Result<Option<GasPaymentRecord>, String> {
    let count: u32 = client.contract_query(igp, "gas_payment_count", &()).await?;
    let start = count.saturating_sub(MAX_IGP_PAGE);
    let page: Vec<GasPaymentRecord> = client
        .contract_query(igp, "gas_payments", &(start, count - start))
        .await?;
    Ok(page.into_iter().rev().find(|record| {
        record.block_height == block_height
            && record.message_id == message_id
            && record.destination == destination
            && record.gas_limit == gas_amount
            && record.payment <= deposit
    }))
}

#[allow(clippy::too_many_arguments)]
async fn cmd_pay_for_gas(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    igp_hex: &str,
    message: TopUpMessage,
    gas_amount: u64,
    max_payment: Option<u64>,
    quote_only: bool,
    expected_chain_id: Option<u8>,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let igp = parse_bytes32(igp_hex)?;
    let given_id = match &message {
        TopUpMessage::Id { id, .. } => Some(parse_bytes32(id)?),
        TopUpMessage::Nonce { .. } => None,
    };
    let client = RuesClient::new(rues_url)?;
    // Only --quote-only, which signs nothing, may leave the chain unbound.
    let chain_id = match expected_chain_id {
        Some(expected) => Some(bind_chain_id(&client, expected).await?),
        None if quote_only => None,
        None => return Err("--expected-chain-id is required to pay".into()),
    };

    // Check what the IGP checks before signing anything: a transaction that
    // fails on chain pays its whole gas limit, and the node reports a failed
    // query only as an internal error.
    fee_contract_kind(&client, &igp, igp_hex, false).await?;
    let version: u32 = client
        .contract_query(&igp, "state_version", &())
        .await
        .map_err(|e| format!("Can't read the state version of {igp_hex}: {e}"))?;
    if version < IGP_TOP_UP_STATE_VERSION {
        return Err(format!(
            "The IGP {igp_hex} is at state_version {version} and takes no top-ups; \
             pay_for_gas came with state_version {IGP_TOP_UP_STATE_VERSION}"
        ));
    }
    let (message_id, destination, nonce) = match message {
        TopUpMessage::Id { destination, .. } => {
            (given_id.expect("parsed above"), destination, None)
        }
        TopUpMessage::Nonce { nonce, destination } => {
            let mailbox: [u8; 32] = client.contract_query(&igp, "mailbox", &()).await?;
            let (id, _, encoded) = dispatched_message(&client, &mailbox, nonce).await?;
            let to = hyperlane_dusk_types::message::destination(&encoded);
            if let Some(expected) = destination.filter(|expected| *expected != to) {
                return Err(format!(
                    "The message at nonce {nonce} goes to domain {to}, not {expected}"
                ));
            }
            (id, to, Some(nonce))
        }
    };
    let config: DomainGasConfig = client
        .contract_query(&igp, "domain_gas_config", &(destination,))
        .await?;
    top_up_quote(config, gas_amount, destination)?;
    let quote: u64 = client
        .contract_query(&igp, "quote_pay_for_gas", &(destination, gas_amount))
        .await
        .map_err(|e| {
            format!("The IGP's quote for {gas_amount} gas to domain {destination} failed: {e}")
        })?;
    let deposit = top_up_deposit(quote, max_payment)?;

    let mut output = json!({
        "success": true,
        "igp": igp_hex,
        "message_id": hex::encode(message_id),
        "destination": destination,
        "gas_amount": gas_amount,
        "quote": quote.to_string(),
        "quote_dusk": format_dusk(quote),
        "deposit": deposit.to_string(),
    });
    if let Some(nonce) = nonce {
        output["nonce"] = json!(nonce);
    }
    if quote_only {
        output["quote_only"] = json!(true);
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
        return Ok(());
    }

    let chain_id = chain_id.ok_or("--expected-chain-id is required to pay")?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let (account_nonce, balance) = client.query_account(&pk).await?;
    ensure_balance_covers(balance, deposit, gas_limit, gas_price)?;
    let tx = moonlight_call_with_deposit(
        &sk,
        ContractId::from_bytes(igp),
        "pay_for_gas",
        rkyv_serialize(&(message_id, destination, gas_amount, deposit)),
        deposit,
        gas_limit,
        gas_price,
        next_moonlight_nonce(account_nonce)?,
        chain_id,
    )?;
    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;
    // The payment is made; what it cost is only reported, so a failed
    // lookup leaves `paid` out rather than failing the command.
    let record = find_top_up_record(
        &client,
        &igp,
        confirmation.inclusion.block_height,
        (message_id, destination, gas_amount),
        deposit,
    )
    .await
    .ok()
    .flatten();
    if let Some(record) = record {
        output["paid"] = json!(record.payment.to_string());
        output["refunded"] = json!((deposit - record.payment).to_string());
    }
    output["fn_name"] = json!("pay_for_gas");
    output["payer"] = account_identity(&pk)["address"].clone();
    output["tx_id"] = json!(tx_id);
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

// ── DRC20 helpers ────────────────────────────────────────────────────────

async fn cmd_drc20_approve(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    token_hex: &str,
    spender_hex: &str,
    amount: u64,
    expected_chain_id: u8,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let token = ContractId::from_bytes(parse_bytes32(token_hex)?);
    let spender = ContractId::from_bytes(parse_bytes32(spender_hex)?);
    let args = rkyv_serialize(&Drc20ApproveCall {
        spender: Drc20Account::Contract(spender),
        amount,
    });
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    if amount > 0 {
        check_room_for_new_spender(
            &client,
            &token.to_bytes(),
            Drc20Account::moonlight(&pk),
            Drc20Account::Contract(spender),
        )
        .await?;
    }
    let (nonce, _) = client.query_account(&pk).await?;
    let tx = moonlight_call(
        &sk,
        token,
        "approve",
        args,
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;
    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;
    let mut output = json!({
        "success": true,
        "token": token_hex,
        "spender": spender_hex,
        "amount": amount,
        "tx_id": tx_id,
    });
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

/// Refuse an approval a WarpDrc20 would refuse: an allowance for a new
/// spender when the owner already holds 64, or when the token's memory plus
/// what the allowance can allocate would pass 3 GiB. A token that has no
/// `allowance_count` isn't a WarpDrc20 and has neither limit, but it counts
/// as having none only once the node has shown it
/// ([`RuesClient::optional_view`]): a reply that doesn't stops the approval
/// before it is signed.
async fn check_room_for_new_spender(
    client: &RuesClient,
    token: &[u8; 32],
    owner: Drc20Account,
    spender: Drc20Account,
) -> Result<(), String> {
    let allowance_args = Drc20Allowance { owner, spender };
    let allowance: u64 = client
        .contract_query(token, "allowance", &allowance_args)
        .await
        .map_err(|e| format!("Can't read the current allowance: {e}"))?;
    if allowance > 0 {
        return Ok(());
    }
    let count = client
        .optional_view::<_, u32>(
            token,
            "allowance_count",
            &owner,
            ("allowance", &rkyv_serialize(&allowance_args)),
        )
        .await
        .map_err(|e| {
            format!(
                "Can't tell whether the token limits allowances, as a WarpDrc20 does, so \
                 nothing was signed: {e}"
            )
        })?;
    let Some(count) = count else {
        return Ok(());
    };
    if count >= MAX_ALLOWANCES_PER_OWNER {
        return Err(format!(
            "The signer already holds {count} allowances on this WarpDrc20, the most an owner \
             holds: approve 0 for one of them first"
        ));
    }
    let memory: u64 = client
        .contract_query(token, "memory_bytes", &())
        .await
        .map_err(|e| format!("Can't read the token's memory: {e}"))?;
    if memory.saturating_add(NEW_ALLOWANCE_MEMORY_BOUND) > NEW_ENTRY_MEMORY_LIMIT {
        return Err(format!(
            "The WarpDrc20's memory, {memory} bytes, is within 14 pages of 3 GiB, where it \
             takes no allowances for new spenders, so the approval would fail"
        ));
    }
    Ok(())
}

async fn cmd_drc20_balance(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    token_hex: &str,
    account_contract_hex: Option<&str>,
) -> Result<(), String> {
    let account = if let Some(contract_hex) = account_contract_hex {
        Drc20Account::Contract(ContractId::from_bytes(parse_bytes32(contract_hex)?))
    } else {
        let (_, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
        Drc20Account::moonlight(&pk)
    };
    let client = RuesClient::new(rues_url)?;
    let token = parse_bytes32(token_hex)?;
    let balance: u64 = client
        .contract_query(&token, "balance_of", &Drc20BalanceOf { account })
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "success": true,
            "token": token_hex,
            "balance": balance,
        }))
        .unwrap()
    );
    Ok(())
}

/// What a canonical DRC20 holds: its settings, supply and cap, and whether
/// it is paused.
#[derive(Debug, Clone)]
struct Drc20Info {
    name: String,
    symbol: String,
    decimals: u8,
    total_supply: u64,
    cap: u64,
    remaining_mintable: u64,
    paused: bool,
}

async fn drc20_info(client: &RuesClient, token: &[u8; 32]) -> Result<Drc20Info, String> {
    Ok(Drc20Info {
        name: drc20_view(client, token, "name").await?,
        symbol: drc20_view(client, token, "symbol").await?,
        decimals: drc20_view(client, token, "decimals").await?,
        total_supply: drc20_view(client, token, "total_supply").await?,
        cap: drc20_view(client, token, "cap").await?,
        remaining_mintable: drc20_view(client, token, "remaining_mintable").await?,
        paused: drc20_view(client, token, "paused").await?,
    })
}

/// A view of a canonical DRC20 that takes no argument.
async fn drc20_view<O>(client: &RuesClient, token: &[u8; 32], method: &str) -> Result<O, String>
where
    O: rkyv::Archive,
    O::Archived: rkyv::Deserialize<O, rkyv::Infallible>
        + for<'b> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
{
    client
        .contract_query(token, method, &())
        .await
        .map_err(|e| format!("Can't read the token's {method} (is it a canonical DRC20?): {e}"))
}

/// Whether `account` holds `role` on the canonical DRC20 `token`.
async fn drc20_has_role(
    client: &RuesClient,
    token: &[u8; 32],
    role: canonical_drc20::Role,
    account: Drc20Account,
) -> Result<bool, String> {
    client
        .contract_query(
            token,
            "has_role",
            &canonical_drc20::RoleQuery { role, account },
        )
        .await
        .map_err(|e| format!("Can't ask the token whether the account holds a role: {e}"))
}

/// `drc20-info`'s output.
async fn drc20_info_output(
    rues_url: &str,
    token_hex: &str,
    account_arg: Option<&str>,
) -> Result<serde_json::Value, String> {
    let token = parse_bytes32(token_hex)?;
    let account = account_arg.map(parse_account_arg).transpose()?;
    let client = RuesClient::new(rues_url)?;
    let info = drc20_info(&client, &token).await?;
    let mut output = json!({
        "success": true,
        "token": hex::encode(token),
        "name": info.name,
        "symbol": info.symbol,
        "decimals": info.decimals,
        "total_supply": info.total_supply.to_string(),
        "cap": info.cap.to_string(),
        "paused": info.paused,
    });
    if let Some(pk) = account {
        let principal = Drc20Account::moonlight(&pk);
        let balance: u64 = client
            .contract_query(&token, "balance_of", &Drc20BalanceOf { account: principal })
            .await
            .map_err(|e| format!("Can't read the account's balance: {e}"))?;
        let mut entry = account_identity(&pk);
        entry["balance"] = json!(balance.to_string());
        for (name, role) in [
            ("admin", canonical_drc20::DEFAULT_ADMIN_ROLE),
            ("minter", canonical_drc20::MINTER_ROLE),
            ("pauser", canonical_drc20::PAUSER_ROLE),
        ] {
            entry[name] = json!(drc20_has_role(&client, &token, role, principal).await?);
        }
        output["account"] = entry;
    }
    Ok(output)
}

/// What `drc20-mint` checks before it signs: the token would refuse a
/// signer without the minter role, a mint while it is paused, and one past
/// its cap or a u64 of supply.
fn check_mint(info: &Drc20Info, minter: bool, amount: u64) -> Result<(), String> {
    if amount == 0 {
        return Err("The amount must be more than zero".into());
    }
    if !minter {
        return Err(
            "The signer doesn't hold the token's minter role, so the token would refuse the mint"
                .into(),
        );
    }
    if info.paused {
        return Err("The token is paused, and mints nothing until it is unpaused".into());
    }
    if amount > info.remaining_mintable {
        return Err(format!(
            "The token's cap is {}, and {} of it is left to mint: not {amount}",
            info.cap, info.remaining_mintable
        ));
    }
    if info.total_supply.checked_add(amount).is_none() {
        return Err(format!(
            "The token's supply is {}, and {amount} more would overflow a u64",
            info.total_supply
        ));
    }
    Ok(())
}

/// Mint, and return `drc20-mint`'s output.
#[allow(clippy::too_many_arguments)]
async fn drc20_mint(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    token_hex: &str,
    to_arg: Option<&str>,
    amount: u64,
    expected_chain_id: u8,
    gas_limit: u64,
    gas_price: u64,
) -> Result<serde_json::Value, String> {
    // Reject malformed public inputs before reading signer material.
    let token_bytes = parse_bytes32(token_hex)?;
    let to = to_arg.map(parse_account_arg).transpose()?;
    if amount == 0 {
        return Err("The amount must be more than zero".into());
    }
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let to = to.unwrap_or(pk);
    let info = drc20_info(&client, &token_bytes).await?;
    let minter = drc20_has_role(
        &client,
        &token_bytes,
        canonical_drc20::MINTER_ROLE,
        Drc20Account::moonlight(&pk),
    )
    .await?;
    check_mint(&info, minter, amount)?;
    let args = rkyv_serialize(&canonical_drc20::MintCall {
        to: Drc20Account::moonlight(&to),
        amount,
        authorization: None,
    });
    let (nonce, _) = client.query_account(&pk).await?;
    let tx = moonlight_call(
        &sk,
        ContractId::from_bytes(token_bytes),
        "mint",
        args,
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;
    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;
    let mut output = json!({
        "success": true,
        "token": hex::encode(token_bytes),
        "symbol": info.symbol,
        "to": account_identity(&to)["address"].clone(),
        "amount": amount.to_string(),
        "total_supply": (info.total_supply + amount).to_string(),
        "tx_id": tx_id,
    });
    confirmation.record(&mut output);
    Ok(output)
}

// ── cmd_transfer_remote ───────────────────────────────────────────────────

async fn cmd_transfer_remote(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    warp_contract_hex: &str,
    destination: u32,
    recipient_arg: &str,
    raw_recipient: bool,
    amount: u64,
    native: bool,
    expected_chain_id: u8,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let warp_bytes = parse_bytes32(warp_contract_hex)?;
    let warp_id = ContractId::from_bytes(warp_bytes);
    let recipient = parse_recipient(recipient_arg, raw_recipient)?;
    if amount == 0 {
        return Err("Amount must be greater than zero".into());
    }
    let client = RuesClient::new(rues_url)?;
    let chain_id = bind_chain_id(&client, expected_chain_id).await?;

    // Check the route before signing anything: a transaction that fails on
    // chain is charged its whole gas limit, and the node reports a failed
    // quote only as "Internal server error".
    let router: [u8; 32] = client
        .contract_query(&warp_bytes, "enrolled_router", &(destination,))
        .await
        .map_err(|e| format!("Can't query {warp_contract_hex} (is it a warp route?): {e}"))?;
    if router == [0u8; 32] {
        return Err(format!(
            "The route has no router enrolled for destination domain {destination}"
        ));
    }
    // A native route takes the amount with the transaction, a DRC20 route
    // from the token: either kind sent the other way fails on chain. A
    // native route is told by its state_version, which every route answers,
    // so it is never asked for a view only a DRC20 route has: that query
    // would only fail, and the node logs every failed query as an error.
    if native {
        let version: u32 = client
            .contract_query(&warp_bytes, "state_version", &())
            .await
            .map_err(|e| format!("Can't read the state version of {warp_contract_hex}: {e}"))?;
        if version != WARP_NATIVE_STATE_VERSION {
            return Err(format!(
                "The route reports state_version {version}, not WarpNative's \
                 {WARP_NATIVE_STATE_VERSION}: if it bridges a DRC20 token, drop --native"
            ));
        }
    } else {
        // A synthetic route has a `name`, a collateral route a
        // `wrapped_token`.
        let bridges_token = client
            .contract_query::<_, String>(&warp_bytes, "name", &())
            .await
            .is_ok()
            || client
                .contract_query::<_, [u8; 32]>(&warp_bytes, "wrapped_token", &())
                .await
                .is_ok();
        if !bridges_token {
            return Err(
                "This route bridges native DUSK: add --native so the amount goes with the transaction"
                    .into(),
            );
        }
    }
    let recipient_hex = hex::encode(recipient);

    // Quote before loading signing material. The transfer call repeats the
    // quote and checks its Mailbox credit is unchanged, so a changed quote
    // fails atomically instead of consuming shared route credit.
    let dispatch_fee: u64 = client
        .contract_query(
            &warp_bytes,
            "quote_transfer_remote",
            &(destination, recipient, amount),
        )
        .await?;
    let deposit = if native {
        amount
            .checked_add(dispatch_fee)
            .ok_or_else(|| "Native transfer deposit overflows u64".to_string())?
    } else {
        dispatch_fee
    };
    // The route funds its dispatch credit for each transfer that pays a fee,
    // and its credit is spent to zero, which leaves no entry: so it is a new
    // payer each time unless someone gave it standing credit. A free
    // dispatch funds nothing and adds no entry.
    if dispatch_fee > 0 {
        let mailbox: [u8; 32] = client
            .contract_query(&warp_bytes, "mailbox", &())
            .await
            .map_err(|e| format!("Can't read the route's Mailbox: {e}"))?;
        let route_credit: u64 = client
            .contract_query(&mailbox, "fee_credit", &(warp_bytes,))
            .await
            .map_err(|e| format!("Can't read the route's dispatch credit: {e}"))?;
        if route_credit == 0 {
            check_room_for_new_payer(
                &client,
                &mailbox,
                "The route funds new dispatch credit for each transfer",
            )
            .await?;
        }
    }

    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;

    // Serialize args: (destination: u32, recipient: H256, amount: u64)
    let transfer_args = rkyv_serialize(&(destination, recipient, amount));

    let (nonce, _balance) = client.query_account(&pk).await?;

    let tx = moonlight_call_with_deposit(
        &sk,
        warp_id,
        "transfer_remote",
        transfer_args,
        deposit,
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;

    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;

    let mut output = json!({
        "success": true,
        "fn_name": "transfer_remote",
        "warp_contract": warp_contract_hex,
        "destination": destination,
        "recipient": recipient_hex,
        "amount": amount,
        "native": native,
        "dispatch_fee": dispatch_fee,
        "deposit": deposit,
        "tx_id": tx_id,
    });
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

// ── cmd_account ───────────────────────────────────────────────────────────

const LUX_PER_DUSK: u64 = 1_000_000_000;

/// Parse a base58 Moonlight address (a 96-byte BLS public key).
fn parse_moonlight_address(value: &str) -> Result<BlsPublicKey, String> {
    let bytes = bs58::decode(value.trim())
        .into_vec()
        .map_err(|error| format!("Invalid Moonlight address: {error}"))?;
    if bytes.len() != 96 {
        return Err(format!(
            "Moonlight address must decode to 96 bytes, got {}",
            bytes.len()
        ));
    }
    parse_account_public_key(&hex::encode(bytes))
        .map_err(|error| error.replace("Recipient public key", "Moonlight address"))
}

/// Parse an account given as a base58 Moonlight address or as a 96-byte
/// public key in hex.
fn parse_account_arg(value: &str) -> Result<BlsPublicKey, String> {
    let hex_part = value.strip_prefix("0x").unwrap_or(value);
    if hex_part.len() == 192 && hex_part.bytes().all(|b| b.is_ascii_hexdigit()) {
        parse_account_public_key(value)
    } else {
        parse_moonlight_address(value)
    }
}

/// Parse a transfer recipient: a 20-byte EVM address, left-padded to 32 bytes
/// as Hyperlane encodes it, or 32 bytes as given. A 32-byte value whose first
/// 12 bytes aren't zero needs `raw`, because an EVM route can't deliver to it
/// and the tokens would stay locked.
fn parse_recipient(value: &str, raw: bool) -> Result<[u8; 32], String> {
    let hex_part = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    let bytes = hex::decode(hex_part).map_err(|e| format!("Invalid recipient hex: {e}"))?;
    let recipient: [u8; 32] = match bytes.len() {
        20 => {
            let mut padded = [0u8; 32];
            padded[12..].copy_from_slice(&bytes);
            padded
        }
        32 => bytes.try_into().expect("length checked"),
        n => {
            return Err(format!(
                "Recipient must be a 20-byte address or 32 bytes, got {n} bytes"
            ))
        }
    };
    if recipient == [0u8; 32] {
        return Err("Recipient cannot be zero".into());
    }
    if !raw && recipient[..12] != [0u8; 12] {
        return Err(
            "Recipient's first 12 bytes aren't zero, so it isn't an EVM address and an \
                    EVM route would never deliver it. Pass the 20-byte address, or \
                    --raw-recipient for a non-EVM destination."
                .into(),
        );
    }
    Ok(recipient)
}

/// Format a LUX amount as DUSK with all nine decimals.
fn format_dusk(lux: u64) -> String {
    format!("{}.{:09}", lux / LUX_PER_DUSK, lux % LUX_PER_DUSK)
}

/// The account's address and the recipient hash warp routes deliver to:
/// `keccak256` of the 96-byte public key, which a route maps back to the
/// account once the account claims its first transfer there.
fn account_identity(pk: &BlsPublicKey) -> serde_json::Value {
    let pk_bytes = pk.to_bytes();
    json!({
        "address": bs58::encode(pk_bytes).into_string(),
        "public_key": hex::encode(pk_bytes),
        "account_h256": hex::encode(hyperlane_dusk_types::message::keccak256(&pk_bytes)),
    })
}

async fn cmd_account(
    rues_url: &str,
    pk: &BlsPublicKey,
    warp_contract_hex: Option<&str>,
    offline: bool,
) -> Result<(), String> {
    let mut output = account_identity(pk);
    output["success"] = json!(true);
    if !offline {
        let client = RuesClient::new(rues_url)?;
        let chain_id = client.query_chain_id().await?;
        let (nonce, balance) = client.query_account(pk).await?;
        output["chain_id"] = json!(chain_id);
        output["balance"] = json!(balance.to_string());
        output["balance_dusk"] = json!(format_dusk(balance));
        output["nonce"] = json!(nonce.to_string());
        if let Some(warp_contract_hex) = warp_contract_hex {
            let warp_bytes = parse_bytes32(warp_contract_hex)?;
            let h256 = hyperlane_dusk_types::message::keccak256(&pk.to_bytes());
            let registered: bool = client
                .contract_query(&warp_bytes, "is_registered", &(h256,))
                .await
                .map_err(|e| {
                    format!("Can't query {warp_contract_hex} (is it a warp route?): {e}")
                })?;
            let pending: u64 = client
                .contract_query(&warp_bytes, "pending_balance", &(h256,))
                .await?;
            output["route"] = json!({
                "warp_contract": warp_contract_hex,
                "registered": registered,
                "pending": pending.to_string(),
            });
        }
    }
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

// ── cmd_export_signer_key ─────────────────────────────────────────────────

/// Create `path` (which must not exist) with mode 0600 and write the secret
/// key as `0x`-prefixed hex.
fn write_signer_key_file(path: &std::path::Path, sk: &BlsSecretKey) -> Result<(), String> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("Failed to create {}: {e}", path.display()))?;
    writeln!(file, "0x{}", hex::encode(sk.to_bytes()))
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

fn cmd_export_signer_key(
    keys_path: PathBuf,
    password: &str,
    out: &std::path::Path,
) -> Result<(), String> {
    let (sk, pk) = load_keys(Some(keys_path), password, None, false)?;
    write_signer_key_file(out, &sk)?;
    let mut output = account_identity(&pk);
    output["success"] = json!(true);
    output["out"] = json!(out.display().to_string());
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

// ── cmd_dispatched_message ────────────────────────────────────────────────

async fn cmd_dispatched_message(
    rues_url: &str,
    mailbox_hex: &str,
    nonce: u32,
) -> Result<(), String> {
    let client = RuesClient::new(rues_url)?;
    let mailbox = parse_bytes32(mailbox_hex)?;
    let (id, height, message) = dispatched_message(&client, &mailbox, nonce).await?;
    let output = json!({
        "success": true,
        "nonce": nonce,
        "message_id": hex::encode(id),
        "block_height": height,
        "message": hex::encode(&message),
        "length": message.len(),
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

/// The message `mailbox` dispatched at `nonce`: its ID, the height of its
/// block, and the message itself, from that block's `Dispatch` event.
async fn dispatched_message(
    client: &RuesClient,
    mailbox: &[u8; 32],
    nonce: u32,
) -> Result<([u8; 32], u64, Vec<u8>), String> {
    let count: u32 = client.contract_query(mailbox, "nonce", &()).await?;
    if nonce >= count {
        return Err(format!(
            "The Mailbox has dispatched {count} messages; nonce {nonce} doesn't exist yet"
        ));
    }
    let id: [u8; 32] = client
        .contract_query(mailbox, "dispatched_id", &(nonce,))
        .await?;
    let height: u64 = client
        .contract_query(mailbox, "dispatched_block_height", &(nonce,))
        .await?;

    // The archive can trail the block by a moment.
    for attempt in 0..15 {
        let events = client.block_events(height).await?;
        if let Some(message) = find_dispatched_message(&events, mailbox, &id) {
            return Ok((id, height, message));
        }
        if attempt < 14 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }
    Err(format!(
        "No Dispatch event with message ID {} in block {height}; the node may not serve the archive",
        hex::encode(id)
    ))
}

/// The message with ID `id` among a block's `events`, taken from a `Dispatch`
/// event that `mailbox` emitted and that wasn't reverted.
fn find_dispatched_message(
    events: &[serde_json::Value],
    mailbox: &[u8; 32],
    id: &[u8; 32],
) -> Option<Vec<u8>> {
    use hyperlane_dusk_types::events::Dispatch;
    use hyperlane_dusk_types::message;

    events.iter().find_map(|event| {
        let field = |name: &str| event.get(name).and_then(serde_json::Value::as_str);
        // Compare bytes: either side may be written with `0x` or in upper case.
        let from_mailbox = field("source")
            .is_some_and(|source| parse_bytes32(source).is_ok_and(|bytes| bytes == *mailbox));
        let reverted = event
            .get("reverted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !from_mailbox || reverted || field("topic") != Some(Dispatch::TOPIC) {
            return None;
        }
        let data = hex::decode(field("data")?.trim_start_matches("0x")).ok()?;
        let dispatch = rues::rkyv_deserialize::<Dispatch>(&data).ok()?;
        (message::id(&dispatch.message) == *id).then_some(dispatch.message)
    })
}
