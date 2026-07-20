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

use std::future::Future;
use std::path::PathBuf;
use std::{env, fs, io::Read};

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

use rues::{RuesClient, TransactionStatus};

const MAX_PASSWORD_FILE_BYTES: usize = 4 * 1024;
const MAX_SECRET_KEY_STDIN_BYTES: usize = 128;
const MAX_MULTISIG_VALIDATORS: usize = u8::MAX as usize;

// ── CLI definition ──────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "dusk-tx", about = "Hyperlane Dusk transaction tool")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Call a contract method via Moonlight transaction.
    Call {
        #[arg(long, default_value = "http://localhost:18090/")]
        rues_url: String,
        /// Path to encrypted consensus.keys file.
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys.
        #[arg(long, default_value = "password")]
        password: String,
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
        /// Gas price in LUX.
        #[arg(long, default_value = "2000")]
        gas_price: u64,
    },
    /// Deploy the full Hyperlane contract stack.
    DeployHyperlane {
        #[arg(long, default_value = "http://localhost:18090/")]
        rues_url: String,
        /// Path to encrypted consensus.keys file.
        #[arg(long)]
        keys: Option<PathBuf>,
        /// Password for consensus.keys.
        #[arg(long, default_value = "password")]
        password: String,
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
        /// Gas price in LUX.
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
        #[arg(long, default_value = "http://localhost:18090/")]
        rues_url: String,
        /// Contract ID (64 hex chars).
        #[arg(long)]
        contract: String,
        /// Method name.
        #[arg(long)]
        method: String,
        /// Return type: u32, u64, bool, bytes32, bytes.
        #[arg(long, name = "return-type", default_value = "u32")]
        return_type: String,
        /// Optional u32 argument.
        #[arg(long, name = "arg-u32")]
        arg_u32: Option<u32>,
        /// Optional bytes32 argument (64 hex chars).
        #[arg(long, name = "arg-bytes32")]
        arg_bytes32: Option<String>,
    },
    /// Deposit native DUSK into a Mailbox dispatch-fee credit.
    FundDispatch {
        #[arg(long, default_value = "http://localhost:18090/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        #[arg(long, default_value = "password")]
        password: String,
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
        #[arg(long, default_value = "2000")]
        gas_price: u64,
    },
    /// Dispatch a message via TestRecipient proxy.
    Dispatch {
        #[arg(long, default_value = "http://localhost:18090/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        #[arg(long, default_value = "password")]
        password: String,
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
        #[arg(long, default_value = "2000")]
        gas_price: u64,
    },
    /// Process an inbound message on the Dusk Mailbox.
    Process {
        #[arg(long, default_value = "http://localhost:18090/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        #[arg(long, default_value = "password")]
        password: String,
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
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        #[arg(long, default_value = "2000")]
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
        #[arg(long, default_value = "http://localhost:18090/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        #[arg(long, default_value = "password")]
        password: String,
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
        #[arg(long, default_value = "2000")]
        gas_price: u64,
    },
    /// Register the caller's BLS key on a warp route contract.
    RegisterAccount {
        #[arg(long, default_value = "http://localhost:18090/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        #[arg(long, default_value = "password")]
        password: String,
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
        #[arg(long, default_value = "2000")]
        gas_price: u64,
    },
    /// Approve a DRC20 contract spender using the current Dusk ABI.
    Drc20Approve {
        #[arg(long, default_value = "http://localhost:18090/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        #[arg(long, default_value = "password")]
        password: String,
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
        #[arg(long, default_value = "2000")]
        gas_price: u64,
    },
    /// Query a DRC20 balance for a Moonlight or contract account.
    Drc20Balance {
        #[arg(long, default_value = "http://localhost:18090/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        #[arg(long, default_value = "password")]
        password: String,
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
    /// Call transfer_remote on a WarpDrc20 contract.
    TransferRemote {
        #[arg(long, default_value = "http://localhost:18090/")]
        rues_url: String,
        #[arg(long)]
        keys: Option<PathBuf>,
        #[arg(long, default_value = "password")]
        password: String,
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
        /// Recipient H256 (64 hex chars).
        #[arg(long)]
        recipient: String,
        /// Amount (decimal).
        #[arg(long)]
        amount: u64,
        /// Attach `amount` as a native DUSK deposit for WarpNative.
        #[arg(long)]
        native: bool,
        #[arg(long, default_value = "30000000")]
        gas_limit: u64,
        #[arg(long, default_value = "2000")]
        gas_price: u64,
    },
}

// ── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Call {
            rues_url,
            keys,
            password,
            secret_key,
            secret_key_stdin,
            contract,
            fn_name,
            args,
            gas_limit,
            gas_price,
        } => {
            cmd_call(
                &rues_url,
                keys,
                &password,
                secret_key,
                secret_key_stdin,
                &contract,
                &fn_name,
                &args,
                gas_limit,
                gas_price,
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
            multisig_validators,
            multisig_threshold,
        } => {
            cmd_deploy_hyperlane(
                &rues_url,
                keys,
                &password,
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
            arg_bytes32,
        } => {
            cmd_query(
                &rues_url,
                &contract,
                &method,
                &return_type,
                arg_u32,
                arg_bytes32,
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
                &password,
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
                &password,
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
            secret_key_stdin,
            gas_limit,
            gas_price,
        } => {
            cmd_process(
                &rues_url,
                keys,
                &password,
                secret_key,
                secret_key_stdin,
                &mailbox,
                &message,
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
                &password,
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
                &password,
                secret_key,
                secret_key_stdin,
                &warp_contract,
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
                &password,
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
                &password,
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
            amount,
            native,
            gas_limit,
            gas_price,
        } => {
            cmd_transfer_remote(
                &rues_url,
                keys,
                &password,
                secret_key,
                secret_key_stdin,
                &warp_contract,
                destination,
                &recipient,
                amount,
                native,
                gas_limit,
                gas_price,
            )
            .await
        }
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

fn resolve_keys_password(cli_password: &str) -> Result<String, String> {
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

    Ok(cli_password.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        next_moonlight_nonce, read_secret_key_hex, resolve_keys_password,
        wait_for_transaction_with, MAX_PASSWORD_FILE_BYTES, MAX_SECRET_KEY_STDIN_BYTES,
    };
    use crate::rues::TransactionStatus;
    use std::collections::VecDeque;
    use std::future::ready;
    use std::sync::Mutex;
    use std::time::Duration;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn secret_key_stdin_is_bounded() {
        let oversized = vec![b'a'; MAX_SECRET_KEY_STDIN_BYTES + 1];
        assert!(read_secret_key_hex(oversized.as_slice())
            .unwrap_err()
            .contains("exceeds"));

        let key = format!("0x{}\n", "11".repeat(32));
        assert_eq!(read_secret_key_hex(key.as_bytes()).unwrap(), key.trim());
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

        let password = resolve_keys_password("from-cli").unwrap();

        clear_password_env();
        let _ = std::fs::remove_file(path);
        assert_eq!(password, "from-file");
    }

    #[test]
    fn password_env_has_precedence_over_cli() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_password_env();
        std::env::set_var("DUSK_CONSENSUS_KEYS_PASS", "from-legacy-env");
        std::env::set_var("DUSK_CONSENSUS_PASSWORD", "from-env");

        let password = resolve_keys_password("from-cli").unwrap();

        clear_password_env();
        assert_eq!(password, "from-env");
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

        let error = resolve_keys_password("from-cli").unwrap_err();

        clear_password_env();
        let _ = std::fs::remove_file(path);
        assert!(error.contains("exceeds"));
    }

    #[test]
    fn falls_back_to_cli_password() {
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

    #[tokio::test]
    async fn transaction_wait_retries_observation_errors_and_keeps_the_hash() {
        let mut statuses = VecDeque::from([
            Err("temporary GraphQL outage".to_string()),
            Ok(TransactionStatus::Executed),
        ]);

        wait_for_transaction_with(
            "aabbcc",
            || ready(statuses.pop_front().expect("status response should exist")),
            2,
            Duration::ZERO,
            Duration::from_secs(1),
        )
        .await
        .expect("a later exact-hash success should reconcile the transaction");

        let mut statuses = VecDeque::from([Err("archive unavailable".to_string())]);
        let error = wait_for_transaction_with(
            "ddeeff",
            || ready(statuses.pop_front().expect("status response should exist")),
            1,
            Duration::ZERO,
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert!(error.contains("ddeeff"));
        assert!(error.contains("archive unavailable"));
    }

    #[tokio::test]
    async fn transaction_wait_checks_immediately_and_rejects_execution_failure() {
        let immediate = tokio::time::timeout(
            Duration::from_millis(100),
            wait_for_transaction_with(
                "1122",
                || ready(Ok(TransactionStatus::Executed)),
                1,
                Duration::from_secs(60),
                Duration::from_secs(1),
            ),
        )
        .await;
        assert!(immediate.is_ok(), "the first query must not sleep");
        assert!(immediate.unwrap().is_ok());

        let error = wait_for_transaction_with(
            "3344",
            || ready(Ok(TransactionStatus::Failed("contract rejected".into()))),
            1,
            Duration::ZERO,
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert!(error.contains("3344"));
        assert!(error.contains("contract rejected"));
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
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let client = RuesClient::new(rues_url)?;
    let mailbox = ContractId::from_bytes(parse_bytes32(mailbox_hex)?);
    let payer = parse_bytes32(payer_hex)?;
    let args = rkyv_serialize(&(payer, amount));
    let chain_id = client.query_chain_id().await?;
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
    client.propagate_tx(&tx.to_var_bytes()).await?;
    wait_for_transaction(&client, &tx_id).await?;
    let output = json!({
        "success": true,
        "mailbox": mailbox_hex,
        "payer": payer_hex,
        "amount": amount,
        "tx_id": tx_id,
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
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
    keys_path: Option<PathBuf>,
    password: &str,
    secret_key_hex: Option<String>,
    secret_key_stdin: bool,
    contract_hex: &str,
    fn_name: &str,
    args_hex: &str,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let client = RuesClient::new(rues_url)?;

    // Parse contract ID
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

    // Query chain ID and account nonce
    let chain_id = client.query_chain_id().await?;
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
    client.propagate_tx(&tx_bytes).await?;
    wait_for_transaction(&client, &tx_id).await?;

    let output = json!({
        "success": true,
        "contract": contract_hex,
        "fn_name": fn_name,
        "tx_id": tx_id,
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
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
    multisig_validators: &str,
    multisig_threshold: u8,
) -> Result<(), String> {
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
    eprintln!("  RUES URL:    {rues_url}");
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
                    account: Drc20Account::External(pk),
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
        Vec::<(u32, DomainGasConfig)>::new(),
    ));
    deploy_one(
        &client, &sk, &pk, igp_bytes, igp_init, &mut mn, &mut dn, gas_limit, gas_price, chain_id,
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
        "contracts": contracts,
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

fn parse_eth_addresses(list: &str) -> Result<Vec<EthAddress>, String> {
    let list = list.trim();
    if list.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for raw in list.split(',') {
        let s = raw.trim();
        if s.is_empty() {
            continue;
        }
        out.push(parse_eth_address(s)?);
    }
    out.sort();

    for i in 1..out.len() {
        if out[i - 1] == out[i] {
            return Err(format!(
                "Duplicate validator address: 0x{}",
                hex::encode(out[i].0)
            ));
        }
    }

    Ok(out)
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

/// Deploy a single contract via Moonlight deployment TX.
async fn deploy_one(
    client: &RuesClient,
    sk: &BlsSecretKey,
    pk: &BlsPublicKey,
    bytecode: Vec<u8>,
    init_args: Vec<u8>,
    moonlight_nonce: &mut u64,
    deploy_nonce: &mut u64,
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
    client
        .propagate_tx(&tx_bytes)
        .await
        .map_err(|e| format!("Failed to deploy {name}: {e}"))?;
    wait_for_transaction(client, &tx_id)
        .await
        .map_err(|e| format!("Failed to deploy {name}: {e}"))?;
    eprintln!("  {name} TX {tx_id} executed");
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

/// Wait for the exact transaction to be persisted and fail closed on a
/// contract execution error. A Moonlight nonce also advances for failed
/// executions, so nonce polling alone cannot establish success.
async fn wait_for_transaction(client: &RuesClient, tx_id: &str) -> Result<(), String> {
    wait_for_transaction_with(
        tx_id,
        || client.query_transaction_status(tx_id),
        20,
        std::time::Duration::from_secs(3),
        std::time::Duration::from_secs(60),
    )
    .await
}

async fn wait_for_transaction_with<F, Fut>(
    tx_id: &str,
    mut query_status: F,
    max_attempts: usize,
    poll_interval: std::time::Duration,
    timeout: std::time::Duration,
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<TransactionStatus, String>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last_query_error = None;

    for attempt in 1..=max_attempts {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        match tokio::time::timeout(remaining, query_status()).await {
            Ok(Ok(TransactionStatus::Executed)) => return Ok(()),
            Ok(Ok(TransactionStatus::Failed(error))) => {
                return Err(format!("Transaction {tx_id} failed: {error}"));
            }
            Ok(Ok(TransactionStatus::NotFound)) => {}
            Ok(Err(error)) => last_query_error = Some(error),
            Err(_) => break,
        }

        if attempt % 5 == 0 {
            eprintln!("  [transaction pending, attempt {attempt}/{max_attempts}]");
        }
        if attempt == max_attempts {
            break;
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

// ── cmd_query ─────────────────────────────────────────────────────────────

async fn cmd_query(
    rues_url: &str,
    contract_hex: &str,
    method: &str,
    return_type: &str,
    arg_u32: Option<u32>,
    arg_bytes32: Option<String>,
) -> Result<(), String> {
    let client = RuesClient::new(rues_url)?;
    let contract_id = parse_bytes32(contract_hex)?;

    match return_type {
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
            let result: u64 = if let Some(val) = arg_u32 {
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
            let result: [u8; 32] = if let Some(val) = arg_u32 {
                client.contract_query(&contract_id, method, &val).await?
            } else {
                client.contract_query(&contract_id, method, &()).await?
            };
            let output = json!({ "success": true, "value": hex::encode(result) });
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
        _ => {
            return Err(format!(
            "Unsupported return type: {return_type}. Use: u32, u64, bool, bytes32, bytes, string"
        ))
        }
    }

    Ok(())
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
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let client = RuesClient::new(rues_url)?;

    let mailbox_id = ContractId::from_bytes(parse_bytes32(mailbox_hex)?);
    let test_recipient_id = ContractId::from_bytes(parse_bytes32(test_recipient_hex)?);
    let recipient = parse_bytes32(recipient_hex)?;

    // Parse body: if starts with 0x, treat as hex; otherwise UTF-8
    let body_bytes: Vec<u8> = if body.starts_with("0x") {
        hex::decode(&body[2..]).map_err(|e| format!("Invalid body hex: {e}"))?
    } else {
        body.as_bytes().to_vec()
    };

    // Serialize args: (mailbox_id, destination, recipient, body)
    let dispatch_args = rkyv_serialize(&(mailbox_id, destination, recipient, body_bytes));

    let chain_id = client.query_chain_id().await?;
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
    client.propagate_tx(&tx.to_var_bytes()).await?;
    wait_for_transaction(&client, &tx_id).await?;

    let output = json!({
        "success": true,
        "fn_name": "dispatch_message",
        "test_recipient": test_recipient_hex,
        "destination": destination,
        "recipient": recipient_hex,
        "tx_id": tx_id,
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
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
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let client = RuesClient::new(rues_url)?;

    let mailbox_id = ContractId::from_bytes(parse_bytes32(mailbox_hex)?);

    let message_hex = message_hex.strip_prefix("0x").unwrap_or(message_hex);
    let encoded_message =
        hex::decode(message_hex).map_err(|e| format!("Invalid message hex: {e}"))?;

    // Compute message ID for output
    let message_id = hyperlane_dusk_types::message::id(&encoded_message);

    // Serialize args: (metadata: Vec<u8>, encoded_message: Vec<u8>)
    let empty_metadata: Vec<u8> = Vec::new();
    let process_args = rkyv_serialize(&(empty_metadata, encoded_message));

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
    client.propagate_tx(&tx.to_var_bytes()).await?;
    wait_for_transaction(&client, &tx_id).await?;

    let output = json!({
        "success": true,
        "fn_name": "process",
        "mailbox": mailbox_hex,
        "message_id": hex::encode(message_id),
        "tx_id": tx_id,
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
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
    client.propagate_tx(&tx.to_var_bytes()).await?;
    wait_for_transaction(&client, &tx_id).await?;

    let output = json!({
        "success": true,
        "fn_name": "enroll_remote_router",
        "warp_contract": warp_contract_hex,
        "domain": domain,
        "router": router_hex,
        "tx_id": tx_id,
    });
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

    let warp_id = ContractId::from_bytes(parse_bytes32(warp_contract_hex)?);

    // No args — the contract reads the sender via abi::public_sender()
    let register_args = vec![];

    let chain_id = client.query_chain_id().await?;
    let (nonce, _balance) = client.query_account(&pk).await?;

    let tx = moonlight_call(
        &sk,
        warp_id,
        "register_account",
        register_args,
        gas_limit,
        gas_price,
        next_moonlight_nonce(nonce)?,
        chain_id,
    )?;

    let tx_id = hex::encode(tx.hash().to_bytes());
    client.propagate_tx(&tx.to_var_bytes()).await?;
    wait_for_transaction(&client, &tx_id).await?;

    // Compute the H256 = keccak256(pk.to_bytes()) for display
    let pk_bytes = pk.to_bytes();
    let h256 = hyperlane_dusk_types::message::keccak256(&pk_bytes);

    let output = json!({
        "success": true,
        "fn_name": "register_account",
        "warp_contract": warp_contract_hex,
        "account_h256": hex::encode(h256),
        "tx_id": tx_id,
    });
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
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let client = RuesClient::new(rues_url)?;
    let token = ContractId::from_bytes(parse_bytes32(token_hex)?);
    let spender = ContractId::from_bytes(parse_bytes32(spender_hex)?);
    let args = rkyv_serialize(&Drc20ApproveCall {
        spender: Drc20Account::Contract(spender),
        value: amount,
    });
    let chain_id = client.query_chain_id().await?;
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
    client.propagate_tx(&tx.to_var_bytes()).await?;
    wait_for_transaction(&client, &tx_id).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "success": true,
            "token": token_hex,
            "spender": spender_hex,
            "amount": amount,
            "tx_id": tx_id,
        }))
        .unwrap()
    );
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
        Drc20Account::External(pk)
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
    recipient_hex: &str,
    amount: u64,
    native: bool,
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let client = RuesClient::new(rues_url)?;

    let warp_id = ContractId::from_bytes(parse_bytes32(warp_contract_hex)?);
    let recipient = parse_bytes32(recipient_hex)?;

    // Serialize args: (destination: u32, recipient: H256, amount: u64)
    let transfer_args = rkyv_serialize(&(destination, recipient, amount));

    let chain_id = client.query_chain_id().await?;
    let (nonce, _balance) = client.query_account(&pk).await?;

    let deposit = if native { amount } else { 0 };
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
    client.propagate_tx(&tx.to_var_bytes()).await?;
    wait_for_transaction(&client, &tx_id).await?;

    let output = json!({
        "success": true,
        "fn_name": "transfer_remote",
        "warp_contract": warp_contract_hex,
        "destination": destination,
        "recipient": recipient_hex,
        "amount": amount,
        "native": native,
        "tx_id": tx_id,
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}
