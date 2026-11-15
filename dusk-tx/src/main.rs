//! dusk-tx — CLI tool for Hyperlane contract deployment and interaction on Dusk.
//!
//! Subcommands:
//!   call               Call a contract method via Moonlight TX
//!   deploy-hyperlane   Deploy the full Hyperlane contract stack
//!   query              Query contract state via RUES
//!   dispatch           Dispatch a message via TestRecipient proxy
//!   process            Process an inbound message on the Mailbox
//!   encode-message     Encode a Hyperlane message (no TX, pure encoding)
//!   enroll-router      Enroll a remote router on a warp route
//!   transfer-remote    Send tokens or native DUSK through a warp route
//!   set-igp-config     Set an IGP's pricing for a destination (owner)
//!   set-beneficiary    Set who can claim a fee contract's fees (owner)
//!   claim-fees         Claim a ProtocolFee's or IGP's fees (beneficiary)
//!   account            Show an account's address, recipient hash and balance
//!   export-signer-key  Write an agent signer key file from consensus.keys
//!
//! Every command that sends a transaction waits until the transaction is
//! included in a block, then says whether that block is final (`finalized`).
//! With `--wait-finality` it waits until it is.

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
    Account as Drc20Account, ApproveCall as Drc20ApproveCall, BalanceOf as Drc20BalanceOf,
};
use hyperlane_dusk_types::{DomainGasConfig, EthAddress};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::ser::Serializer;
use rkyv::Serialize;
use serde_json::json;

mod keys;
mod rues;

use rues::{BlockStatus, Inclusion, RuesClient, TransactionStatus, TransactionStatusQueryError};
const MAX_PASSWORD_FILE_BYTES: usize = 4 * 1024;
const MAX_SECRET_KEY_STDIN_BYTES: usize = 128;
const MAX_RUES_URL_STDIN_BYTES: usize = 8 * 1024;
const MAX_MULTISIG_VALIDATORS: usize = u8::MAX as usize;
const MAX_CALL_ARGS_BYTES: usize = 60 * 1024;
// Keep this synchronized with validator-announce's MAX_VALIDATOR_PAGE.
const MAX_VALIDATOR_ANNOUNCE_PAGE: u32 = 256;
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
/// The hook types the fee contracts report (Hyperlane's HookType).
const IGP_HOOK_TYPE: u8 = 4;
const PROTOCOL_FEE_HOOK_TYPE: u8 = 6;
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
        #[arg(long, name = "fn")]
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
        /// Expected native Dusk chain ID. The endpoint must match before the
        /// signer is read or a transaction is constructed.
        #[arg(long)]
        expected_chain_id: Option<u8>,
        /// Reserved until Rusk supports a non-replayable simulation envelope.
        #[arg(long)]
        simulate_only: bool,
    },
    /// Deploy the full Hyperlane contract stack.
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
        /// Deploy a collateral route wrapping a DRC20 ID, or `warp-drc20`
        /// to wrap the synthetic route deployed by this command.
        #[arg(long)]
        warp_collateral_token: Option<String>,
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
    },
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
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Register the caller's BLS key on a warp route contract.
    RegisterAccount {
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
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        /// Gas price in LUX per unit of gas. Calls only need a price above zero.
        #[arg(long, default_value = "1")]
        gas_price: u64,
    },
    /// Claim tokens held in escrow on a warp route because they arrived before
    /// the recipient registered.
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
        /// Gas limit. A claim takes about 1.87M gas.
        #[arg(long, default_value_t = OWNER_CALL_GAS_LIMIT)]
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
    let result = match cli.command {
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
            warp_name,
            warp_symbol,
            warp_decimals,
            default_ism,
            igp_domain_configs,
            multisig_validators,
            multisig_threshold,
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
                &warp_name,
                &warp_symbol,
                warp_decimals,
                &default_ism,
                &igp_domain_configs,
                &multisig_validators,
                multisig_threshold,
            )
            .await
        }
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
                gas_limit,
                gas_price,
            )
            .await
        }
        Command::RegisterAccount {
            rues_url,
            keys,
            password,
            secret_key,
            warp_contract,
            secret_key_stdin,
            gas_limit,
            gas_price,
        } => {
            cmd_register_account(
                &rues_url,
                keys,
                password.as_deref().unwrap_or_default(),
                secret_key,
                secret_key_stdin,
                &warp_contract,
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
            let account = if let Some(address) = address {
                parse_moonlight_address(&address)
            } else if let Some(public_key) = public_key {
                parse_account_public_key(&public_key)
            } else {
                load_keys(
                    keys,
                    password.as_deref().unwrap_or_default(),
                    secret_key,
                    secret_key_stdin,
                )
                .map(|(_, pk)| pk)
            };
            match account {
                Ok(pk) => cmd_account(&rues_url, &pk, warp_contract.as_deref(), offline).await,
                Err(e) => Err(e),
            }
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
    };

    if let Err(e) = result {
        let output = json!({ "success": false, "error": e });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
        std::process::exit(1);
    }
}

// ── Key loading helper ──────────────────────────────────────────────────────

fn load_keys(
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
) -> Result<(BlsSecretKey, BlsPublicKey), String> {
    if secret_key_stdin {
        if secret_key_hex.is_some() || keys_path.is_some() {
            return Err("Provide only one of --keys, --secret-key, or --secret-key-stdin".into());
        }
        let mut s = read_secret_key_hex(std::io::stdin().lock())?;
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

    /// Routes register an account for someone else only by claiming its
    /// escrow, so only `claim-pending` takes another account.
    #[test]
    fn only_claim_pending_takes_another_account() {
        let route = "11".repeat(32);
        let account = "22".repeat(96);
        let register = ["dusk-tx", "register-account", "--warp-contract", &route];
        assert!(Cli::try_parse_from(register).is_ok());
        assert!(Cli::try_parse_from(register.into_iter().chain(["--for", &account])).is_err());
        assert!(Cli::try_parse_from([
            "dusk-tx",
            "claim-pending",
            "--warp-contract",
            &route,
            "--for",
            &account,
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
            let args = ["dusk-tx", "claim-pending", "--warp-contract", &route];
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
        assert_eq!(resolve_signing_chain_id(None, 1).unwrap(), 1);
        assert_eq!(resolve_signing_chain_id(Some(1), 1).unwrap(), 1);
        assert!(resolve_signing_chain_id(Some(1), 2)
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
        ])
        .unwrap()
        {
            super::Command::SetIgpConfig { gas_limit, .. } => {
                assert_eq!(gas_limit, super::OWNER_CALL_GAS_LIMIT);
            }
            _ => unreachable!("parsed set-igp-config"),
        }
        match parse(&["dusk-tx", "claim-fees", "--contract", &contract]).unwrap() {
            super::Command::ClaimFees { gas_limit, .. } => {
                assert_eq!(gas_limit, super::OWNER_CALL_GAS_LIMIT);
            }
            _ => unreachable!("parsed claim-fees"),
        }
        assert!(parse(&["dusk-tx", "set-beneficiary", "--contract", &contract]).is_err());
        assert!(parse(&["dusk-tx", "set-igp-config", "--igp", &contract]).is_err());
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
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    if amount == 0 {
        return Err("Funding amount must be greater than zero".into());
    }
    let mailbox = ContractId::from_bytes(parse_bytes32(mailbox_hex)?);
    let payer = parse_bytes32(payer_hex)?;
    let args = rkyv_serialize(&(payer, amount));
    let client = RuesClient::new(rues_url)?;
    let chain_id = client.query_chain_id().await?;
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
    let observed_chain_id = client.query_chain_id().await?;
    let chain_id = resolve_signing_chain_id(Some(expected_chain_id), observed_chain_id)?;
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
    expected_chain_id: Option<u8>,
    simulate_only: bool,
) -> Result<(), String> {
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
    let stdin_url;
    let rues_url = if rues_url_stdin {
        stdin_url = read_rues_url(std::io::stdin().lock())?;
        &stdin_url
    } else {
        rues_url
    };
    let client = RuesClient::new(rues_url)?;
    let observed_chain_id = client.query_chain_id().await?;
    let chain_id = resolve_signing_chain_id(expected_chain_id, observed_chain_id)?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;

    // Query the account only after endpoint identity is bound.
    let (nonce, _balance) = client.query_account(&pk).await?;

    // Build and submit TX
    let tx = moonlight_call(
        &sk,
        contract_id,
        fn_name,
        fn_args,
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;

    let tx_id = hex::encode(tx.hash().to_bytes());
    let tx_bytes = tx.to_var_bytes();
    let confirmation = propagate_and_wait(&client, &tx_id, &tx_bytes).await?;

    let mut output = json!({
        "success": true,
        "contract": contract_hex,
        "fn_name": fn_name,
        "tx_id": tx_id,
    });
    confirmation.record(&mut output);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}

fn resolve_signing_chain_id(expected: Option<u8>, observed: u8) -> Result<u8, String> {
    if let Some(expected) = expected {
        if observed != expected {
            return Err(format!(
                "Configured Dusk chain ID {expected} does not match endpoint chain ID {observed}"
            ));
        }
        return Ok(expected);
    }
    Ok(observed)
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
    warp_name: &str,
    warp_symbol: &str,
    warp_decimals: u8,
    default_ism: &str,
    igp_domain_config_args: &[String],
    multisig_validators: &str,
    multisig_threshold: u8,
) -> Result<(), String> {
    let igp_domain_configs = parse_igp_domain_configs(igp_domain_config_args)?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let client = RuesClient::new(rues_url)?;

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

    // Get chain info
    let chain_id = client.query_chain_id().await?;
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
    let wraps_deployed_warp = warp_collateral_token == Some("warp-drc20");
    if wraps_deployed_warp && !deploy_warp_drc20 {
        return Err("--warp-collateral-token warp-drc20 requires --deploy-warp-drc20".into());
    }
    let explicit_collateral_token = warp_collateral_token
        .filter(|token| *token != "warp-drc20")
        .map(parse_bytes32)
        .transpose()?
        .map(ContractId::from_bytes);
    if let Some(token) = explicit_collateral_token {
        let token_hex = hex::encode(token.to_bytes());
        if !client.contract_exists(&token_hex).await? {
            return Err(format!(
                "Explicit collateral token does not exist on-chain: {token_hex}"
            ));
        }
        client
            .contract_query::<_, u64>(
                &token.to_bytes(),
                "balance_of",
                &Drc20BalanceOf {
                    account: Drc20Account::moonlight(&pk),
                },
            )
            .await
            .map_err(|error| {
                format!(
                    "Explicit collateral token {token_hex} does not expose the required DRC20 balance_of ABI: {error}"
                )
            })?;
    }
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
    let collateral_token = if wraps_deployed_warp {
        warp_drc20_id.as_ref().map(|(id, _)| *id)
    } else {
        explicit_collateral_token
    };
    let warp_native_id = warp_native_bytes.map(|warp_bytes| {
        let id = gen_contract_id(&warp_bytes, nonce_counter, &pk_bytes);
        nonce_counter += 1;
        (id, warp_bytes)
    });
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

    // Deploy contracts one at a time, waiting for block inclusion between each.
    // Rusk requires sequential nonce confirmation — a TX with nonce N+1 won't be
    // accepted until nonce N is included in a block.

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
    if let Some(ism_multisig_bytes) = ism_multisig_bytes {
        let _ism_multisig_id = ism_multisig_id.expect("ISM ID missing");
        let mut validators = multisig_validators;
        validators.sort();
        let init = rkyv_serialize(&(owner_h256, validators, multisig_threshold));
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
        1_000_000u64,
        100_000_000u64,
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

    // 12. Optional: existing DRC20 collateral route.
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
        // Every deploy is final once the last one is.
        "last_tx_id": deployed.last().map(|(tx_id, _)| tx_id),
        "last_block_height": confirmation.inclusion.block_height,
        "finalized": confirmation.final_block_hash.is_some(),
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
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
    propagate(client, tx_id, tx_bytes).await?;
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
    // Emit the locally computed reconciliation identity before the first
    // remote write. This gives an interrupted operator a concrete hash to
    // reconcile before deciding whether a non-idempotent operation may be
    // retried.
    eprintln!(
        "  Prepared TX {tx_id}; reconcile this exact hash before retrying if submission is interrupted"
    );
    client
        .propagate_tx(tx_bytes)
        .await
        .map_err(|error| submission_error_with_hash(tx_id, &error))
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
    let chain_id = client.query_chain_id().await?;
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
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let mailbox_id = ContractId::from_bytes(parse_bytes32(mailbox_hex)?);
    let (process_args, message_id) = prepare_process_call(message_hex, metadata_hex)?;

    let client = RuesClient::new(rues_url)?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let chain_id = client.query_chain_id().await?;
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
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let client = RuesClient::new(rues_url)?;

    let warp_id = ContractId::from_bytes(parse_bytes32(warp_contract_hex)?);
    let router = parse_bytes32(router_hex)?;

    // Serialize args: (domain: u32, router: H256)
    let enroll_args = rkyv_serialize(&(domain, router));

    let chain_id = client.query_chain_id().await?;
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

// ── cmd_register_account ──────────────────────────────────────────────────

async fn cmd_register_account(
    rues_url: &str,
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    warp_contract_hex: &str,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let client = RuesClient::new(rues_url)?;

    let warp_bytes = parse_bytes32(warp_contract_hex)?;
    let warp_id = ContractId::from_bytes(warp_bytes);

    // The route registers the transaction signer. Only a claim registers an
    // account for someone else (claim-pending --for).
    let chain_id = client.query_chain_id().await?;
    let (nonce, _balance) = client.query_account(&pk).await?;

    let tx = moonlight_call(
        &sk,
        warp_id,
        "register_account",
        Vec::new(),
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;

    let tx_id = hex::encode(tx.hash().to_bytes());
    let confirmation = propagate_and_wait(&client, &tx_id, &tx.to_var_bytes()).await?;

    let h256 = hyperlane_dusk_types::message::keccak256(&pk.to_bytes());
    let pending: u64 = client
        .contract_query(&warp_bytes, "pending_balance", &(h256,))
        .await
        .unwrap_or(0);

    let mut output = account_identity(&pk);
    output["success"] = json!(true);
    output["fn_name"] = json!("register_account");
    output["warp_contract"] = json!(warp_contract_hex);
    output["pending"] = json!(pending.to_string());
    output["tx_id"] = json!(tx_id);
    confirmation.record(&mut output);
    if pending > 0 {
        output["next"] =
            json!("Tokens are waiting in escrow for this account: run dusk-tx claim-pending");
    }
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
    gas_limit: Option<u64>,
    gas_price: u64,
) -> Result<(), String> {
    let account = for_account.map(parse_account_arg).transpose()?;
    let client = RuesClient::new(rues_url)?;
    let warp_bytes = parse_bytes32(warp_contract_hex)?;
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
    let chain_id = client.query_chain_id().await?;
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
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let (domain, config) = parse_igp_domain_config(config_arg)?;
    let igp = parse_bytes32(igp_hex)?;
    let client = RuesClient::new(rues_url)?;
    fee_contract_kind(&client, &igp, igp_hex, false).await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    ensure_signer_owns(&client, &igp, igp_hex, &pk).await?;
    let previous: DomainGasConfig = client
        .contract_query(&igp, "domain_gas_config", &domain)
        .await?;

    let chain_id = client.query_chain_id().await?;
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
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let account = parse_account_arg(beneficiary_arg)?;
    let beneficiary = hyperlane_dusk_types::message::keccak256(&account.to_bytes());
    let contract = parse_bytes32(contract_hex)?;
    let client = RuesClient::new(rues_url)?;
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

    let chain_id = client.query_chain_id().await?;
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
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let contract = parse_bytes32(contract_hex)?;
    let client = RuesClient::new(rues_url)?;
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

    let chain_id = client.query_chain_id().await?;
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
    let chain_id = client.query_chain_id().await?;
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
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
    let bridges_token = client
        .contract_query::<_, String>(&warp_bytes, "name", &())
        .await
        .is_ok()
        || client
            .contract_query::<_, [u8; 32]>(&warp_bytes, "wrapped_token", &())
            .await
            .is_ok();
    if native && bridges_token {
        return Err("This route bridges a DRC20 token, not native DUSK: drop --native".into());
    }
    if !native && !bridges_token {
        return Err(
            "This route bridges native DUSK: add --native so the amount goes with the transaction"
                .into(),
        );
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

    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;

    // Serialize args: (destination: u32, recipient: H256, amount: u64)
    let transfer_args = rkyv_serialize(&(destination, recipient, amount));

    let chain_id = client.query_chain_id().await?;
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
/// `keccak256` of the 96-byte public key, which `register_account` maps back
/// to the account.
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
    let count: u32 = client.contract_query(&mailbox, "nonce", &()).await?;
    if nonce >= count {
        return Err(format!(
            "The Mailbox has dispatched {count} messages; nonce {nonce} doesn't exist yet"
        ));
    }
    let id: [u8; 32] = client
        .contract_query(&mailbox, "dispatched_id", &(nonce,))
        .await?;
    let height: u64 = client
        .contract_query(&mailbox, "dispatched_block_height", &(nonce,))
        .await?;

    // The archive can trail the block by a moment.
    for attempt in 0..15 {
        let events = client.block_events(height).await?;
        if let Some(message) = find_dispatched_message(&events, &mailbox, &id) {
            let output = json!({
                "success": true,
                "nonce": nonce,
                "message_id": hex::encode(id),
                "block_height": height,
                "message": hex::encode(&message),
                "length": message.len(),
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
            return Ok(());
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
