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

use std::{env, fs, io::Read};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use dusk_bytes::Serializable;
use dusk_core::abi::ContractId;
use dusk_core::signatures::bls::{
    PublicKey as BlsPublicKey, SecretKey as BlsSecretKey,
};
use dusk_core::transfer::data::{ContractBytecode, ContractCall, ContractDeploy};
use dusk_core::transfer::moonlight::Transaction as MoonlightTransaction;
use dusk_core::transfer::Transaction;
use hyperlane_dusk_types::{DomainGasConfig, EthAddress};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::ser::Serializer;
use rkyv::Serialize;
use serde_json::json;

mod keys;
mod rues;

use rues::RuesClient;

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
        /// Token name for WarpDrc20.
        #[arg(long, default_value = "Wrapped Ether")]
        warp_name: String,
        /// Token symbol for WarpDrc20.
        #[arg(long, default_value = "WETH")]
        warp_symbol: String,
        /// Token decimals for WarpDrc20.
        #[arg(long, default_value = "18")]
        warp_decimals: u8,
        /// Mailbox default ISM. Options: `testMock` (default), `messageIdMultisig`.
        #[arg(long, default_value = "testMock")]
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
                &rues_url, keys, &password, secret_key, secret_key_stdin, &contract, &fn_name,
                &args, gas_limit, gas_price,
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
            warp_name,
            warp_symbol,
            warp_decimals,
            default_ism,
            multisig_validators,
            multisig_threshold,
        } => {
            cmd_deploy_hyperlane(
                &rues_url, keys, &password, secret_key, secret_key_stdin, domain, gas_limit,
                gas_price, wasm_dir, deploy_warp_drc20, &warp_name,
                &warp_symbol, warp_decimals, &default_ism, &multisig_validators, multisig_threshold,
            )
            .await
        }
        Command::Query {
            rues_url, contract, method, return_type, arg_u32, arg_bytes32,
        } => {
            cmd_query(&rues_url, &contract, &method, &return_type, arg_u32, arg_bytes32).await
        }
        Command::Dispatch {
            rues_url, keys, password, secret_key, mailbox, test_recipient,
            secret_key_stdin,
            destination, recipient, body, gas_limit, gas_price,
        } => {
            cmd_dispatch(
                &rues_url, keys, &password, secret_key, secret_key_stdin, &mailbox, &test_recipient,
                destination, &recipient, &body, gas_limit, gas_price,
            ).await
        }
        Command::Process {
            rues_url, keys, password, secret_key, mailbox, message,
            secret_key_stdin,
            gas_limit, gas_price,
        } => {
            cmd_process(
                &rues_url, keys, &password, secret_key, secret_key_stdin, &mailbox, &message,
                gas_limit, gas_price,
            ).await
        }
        Command::EncodeMessage {
            version, nonce, origin, sender, destination, recipient, body,
        } => {
            cmd_encode_message(version, nonce, origin, &sender, destination, &recipient, &body)
        }
        Command::EnrollRouter {
            rues_url, keys, password, secret_key, warp_contract,
            secret_key_stdin,
            domain, router, gas_limit, gas_price,
        } => {
            cmd_enroll_router(
                &rues_url, keys, &password, secret_key, secret_key_stdin, &warp_contract,
                domain, &router, gas_limit, gas_price,
            ).await
        }
        Command::RegisterAccount {
            rues_url, keys, password, secret_key, warp_contract,
            secret_key_stdin,
            gas_limit, gas_price,
        } => {
            cmd_register_account(
                &rues_url, keys, &password, secret_key, secret_key_stdin, &warp_contract,
                gas_limit, gas_price,
            ).await
        }
        Command::TransferRemote {
            rues_url, keys, password, secret_key, warp_contract,
            secret_key_stdin,
            destination, recipient, amount, gas_limit, gas_price,
        } => {
            cmd_transfer_remote(
                &rues_url, keys, &password, secret_key, secret_key_stdin, &warp_contract,
                destination, &recipient, amount, gas_limit, gas_price,
            ).await
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
            return Err(
                "Provide only one of --keys, --secret-key, or --secret-key-stdin".into(),
            );
        }
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("Failed to read secret key from stdin: {e}"))?;
        let mut s = buf.trim().to_string();
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

fn resolve_keys_password(cli_password: &str) -> Result<String, String> {
    if let Ok(path) = env::var("DUSK_CONSENSUS_PASSWORD_FILE") {
        let password = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read DUSK_CONSENSUS_PASSWORD_FILE {path}: {e}"))?;
        let password = password.trim_end_matches(&['\r', '\n'][..]).to_string();
        if password.is_empty() {
            return Err("DUSK_CONSENSUS_PASSWORD_FILE is empty".into());
        }
        return Ok(password);
    }

    if let Ok(password) = env::var("DUSK_CONSENSUS_PASSWORD")
        .or_else(|_| env::var("DUSK_CONSENSUS_KEYS_PASS"))
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
    use super::resolve_keys_password;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_password_env() {
        std::env::remove_var("DUSK_CONSENSUS_PASSWORD_FILE");
        std::env::remove_var("DUSK_CONSENSUS_PASSWORD");
        std::env::remove_var("DUSK_CONSENSUS_KEYS_PASS");
    }

    #[test]
    fn password_file_has_precedence() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_password_env();

        let path = std::env::temp_dir().join(format!(
            "dusk-consensus-password-{}",
            std::process::id()
        ));
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
    fn falls_back_to_cli_password() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_password_env();

        let password = resolve_keys_password("from-cli").unwrap();

        clear_password_env();
        assert_eq!(password, "from-cli");
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
    let call = ContractCall {
        contract,
        fn_name: String::from(fn_name),
        fn_args,
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
        Some(call),
    )
    .map_err(|e| format!("MoonlightTransaction::new failed: {e:?}"))?;
    Ok(tx.into())
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
    let client = RuesClient::new(rues_url);

    // Parse contract ID
    let contract_bytes = hex::decode(contract_hex)
        .map_err(|e| format!("Invalid contract hex: {e}"))?;
    if contract_bytes.len() != 32 {
        return Err(format!(
            "Contract ID must be 32 bytes (64 hex), got {}",
            contract_bytes.len()
        ));
    }
    let contract_id = ContractId::from_bytes(
        contract_bytes.try_into().map_err(|_| "bad length")?,
    );

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
        nonce + 1,
        chain_id,
    )?;

    let tx_id = hex::encode(tx.hash().to_bytes());
    let tx_bytes = tx.to_var_bytes();
    client.propagate_tx(&tx_bytes).await?;

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
    warp_name: &str,
    warp_symbol: &str,
    warp_decimals: u8,
    default_ism: &str,
    multisig_validators: &str,
    multisig_threshold: u8,
) -> Result<(), String> {
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let client = RuesClient::new(rues_url);

    let default_ism = parse_default_ism(default_ism)?;
    let (multisig_validators, multisig_threshold) = match default_ism {
        DefaultIsm::TestMock => (Vec::<EthAddress>::new(), 0u8),
        DefaultIsm::MessageIdMultisig => {
            let validators = parse_eth_addresses(multisig_validators)?;
            if validators.is_empty() {
                return Err(
                    "default_ism=messageIdMultisig requires --multisig-validators".into(),
                );
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
        std::fs::read(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))
    };

    let test_mock_bytes = load_wasm("test_mock")?;
    let test_recipient_bytes = load_wasm("test_recipient")?;
    let merkle_tree_hook_bytes = load_wasm("merkle_tree_hook")?;
    let ism_multisig_bytes = if default_ism == DefaultIsm::MessageIdMultisig {
        Some(load_wasm("ism_multisig")?)
    } else {
        None
    };
    let mailbox_bytes = load_wasm("mailbox")?;
    let va_bytes = load_wasm("validator_announce")?;
    let protocol_fee_bytes = load_wasm("protocol_fee")?;
    let igp_bytes = load_wasm("igp")?;

    let pk_bytes = pk.to_bytes();

    // Pre-compute contract IDs (deterministic from bytecode + deploy_nonce + owner)
    //
    // Deploy order:
    // - default_ism=testMock:
    //   TestMock(0), TestRecipient(1), MerkleTreeHook(2), Mailbox(3),
    //   ValidatorAnnounce(4), ProtocolFee(5), IGP(6), [WarpDrc20(7)]
    // - default_ism=messageIdMultisig:
    //   TestMock(0), TestRecipient(1), MerkleTreeHook(2), IsmMultisig(3), Mailbox(4),
    //   ValidatorAnnounce(5), ProtocolFee(6), IGP(7), [WarpDrc20(8)]
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

    let mailbox_id = gen_contract_id(&mailbox_bytes, nonce_counter, &pk_bytes);
    nonce_counter += 1;
    let va_id = gen_contract_id(&va_bytes, nonce_counter, &pk_bytes);
    nonce_counter += 1;
    let protocol_fee_id = gen_contract_id(&protocol_fee_bytes, nonce_counter, &pk_bytes);
    nonce_counter += 1;
    let igp_id = gen_contract_id(&igp_bytes, nonce_counter, &pk_bytes);
    nonce_counter += 1;

    let warp_drc20_id = if deploy_warp_drc20 {
        let warp_bytes = load_wasm("warp_drc20")?;
        let id = gen_contract_id(&warp_bytes, nonce_counter, &pk_bytes);
        Some((id, warp_bytes))
    } else {
        None
    };

    eprintln!("\n  Contract IDs (pre-computed):");
    eprintln!("    TestMock:        {}", hex::encode(test_mock_id.to_bytes()));
    eprintln!("    TestRecipient:   {}", hex::encode(test_recipient_id.to_bytes()));
    eprintln!("    MerkleTreeHook:  {}", hex::encode(merkle_tree_hook_id.to_bytes()));
    if let Some(ism_id) = &ism_multisig_id {
        eprintln!("    IsmMultisig:     {}", hex::encode(ism_id.to_bytes()));
    }
    eprintln!("    Mailbox:         {}", hex::encode(mailbox_id.to_bytes()));
    eprintln!("    ValidatorAnnounce: {}", hex::encode(va_id.to_bytes()));
    eprintln!("    ProtocolFee:     {}", hex::encode(protocol_fee_id.to_bytes()));
    eprintln!("    IGP:             {}", hex::encode(igp_id.to_bytes()));
    if let Some((ref id, _)) = warp_drc20_id {
        eprintln!("    WarpDrc20:       {}", hex::encode(id.to_bytes()));
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
    let mut dn = 0u64;         // deploy nonce

    // Deploy contracts one at a time, waiting for block inclusion between each.
    // Rusk requires sequential nonce confirmation — a TX with nonce N+1 won't be
    // accepted until nonce N is included in a block.

    // 1. TestMock (no init)
    deploy_one(&client, &sk, &pk, test_mock_bytes, vec![], &mut mn, &mut dn, gas_limit, gas_price, chain_id, "TestMock").await?;
    wait_for_nonce(&client, &pk, mn).await?;

    // 2. TestRecipient (no init)
    deploy_one(&client, &sk, &pk, test_recipient_bytes, vec![], &mut mn, &mut dn, gas_limit, gas_price, chain_id, "TestRecipient").await?;
    wait_for_nonce(&client, &pk, mn).await?;

    // 3. MerkleTreeHook: init(mailbox)
    let mth_init = rkyv_serialize(&(mailbox_id,));
    deploy_one(&client, &sk, &pk, merkle_tree_hook_bytes, mth_init, &mut mn, &mut dn, gas_limit, gas_price, chain_id, "MerkleTreeHook").await?;
    wait_for_nonce(&client, &pk, mn).await?;

    // 4. Optional: MessageIdMultisigISM
    if let Some(ism_multisig_bytes) = ism_multisig_bytes {
        let _ism_multisig_id = ism_multisig_id.expect("ISM ID missing");
        let mut validators = multisig_validators;
        validators.sort();
        let owner = mailbox_id.to_bytes();
        let init = rkyv_serialize(&(owner, validators, multisig_threshold));
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
        wait_for_nonce(&client, &pk, mn).await?;
    }

    // 5. Mailbox: init(local_domain, owner, default_ism, default_hook, required_hook)
    let default_ism_id = ism_multisig_id.unwrap_or(test_mock_id);
    let mailbox_init =
        rkyv_serialize(&(domain, mailbox_id, default_ism_id, test_mock_id, merkle_tree_hook_id));
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
    wait_for_nonce(&client, &pk, mn).await?;

    // 6. ValidatorAnnounce: init(local_domain, mailbox)
    let va_init = rkyv_serialize(&(domain, mailbox_id));
    deploy_one(&client, &sk, &pk, va_bytes, va_init, &mut mn, &mut dn, gas_limit, gas_price, chain_id, "ValidatorAnnounce").await?;
    wait_for_nonce(&client, &pk, mn).await?;

    // 7. ProtocolFee: init(protocol_fee, max_protocol_fee, beneficiary, owner)
    let pf_init = rkyv_serialize(&(1_000_000u64, 100_000_000u64, mailbox_id, mailbox_id));
    deploy_one(&client, &sk, &pk, protocol_fee_bytes, pf_init, &mut mn, &mut dn, gas_limit, gas_price, chain_id, "ProtocolFee").await?;
    wait_for_nonce(&client, &pk, mn).await?;

    // 8. IGP: init(owner, beneficiary, initial_configs)
    let igp_init = rkyv_serialize(&(mailbox_id, mailbox_id, Vec::<(u32, DomainGasConfig)>::new()));
    deploy_one(&client, &sk, &pk, igp_bytes, igp_init, &mut mn, &mut dn, gas_limit, gas_price, chain_id, "IGP").await?;
    wait_for_nonce(&client, &pk, mn).await?;

    // 9. Optional: WarpDrc20
    if let Some((warp_id, warp_bytes)) = warp_drc20_id {
        let owner_h256 = hyperlane_dusk_types::message::keccak256(&pk.to_bytes());
        let warp_init = rkyv_serialize(&(mailbox_id, owner_h256, String::from(warp_name), String::from(warp_symbol), warp_decimals, Vec::<(u32, [u8; 32])>::new()));
        deploy_one(&client, &sk, &pk, warp_bytes, warp_init, &mut mn, &mut dn, gas_limit, gas_price, chain_id, "WarpDrc20").await?;
        wait_for_nonce(&client, &pk, mn).await?;

        eprintln!("  All contracts confirmed on-chain");

        // Output includes warp route
        let mut contracts = json!({
            "test_mock": hex::encode(test_mock_id.to_bytes()),
            "test_recipient": hex::encode(test_recipient_id.to_bytes()),
            "merkle_tree_hook": hex::encode(merkle_tree_hook_id.to_bytes()),
            "mailbox": hex::encode(mailbox_id.to_bytes()),
            "validator_announce": hex::encode(va_id.to_bytes()),
            "protocol_fee": hex::encode(protocol_fee_id.to_bytes()),
            "igp": hex::encode(igp_id.to_bytes()),
            "warp_drc20": hex::encode(warp_id.to_bytes()),
        });
        if let Some(ism_id) = ism_multisig_id {
            contracts["ism_multisig"] = json!(hex::encode(ism_id.to_bytes()));
        }

        let output = json!({
            "success": true,
            "domain": domain,
            "chain_id": chain_id,
            "contracts": contracts,
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
        return Ok(());
    }

    // IGP was the last deploy — wait_for_nonce already confirmed it above
    eprintln!("  All contracts confirmed on-chain");

    let mut contracts = json!({
        "test_mock": hex::encode(test_mock_id.to_bytes()),
        "test_recipient": hex::encode(test_recipient_id.to_bytes()),
        "merkle_tree_hook": hex::encode(merkle_tree_hook_id.to_bytes()),
        "mailbox": hex::encode(mailbox_id.to_bytes()),
        "validator_announce": hex::encode(va_id.to_bytes()),
        "protocol_fee": hex::encode(protocol_fee_id.to_bytes()),
        "igp": hex::encode(igp_id.to_bytes()),
    });
    if let Some(ism_id) = ism_multisig_id {
        contracts["ism_multisig"] = json!(hex::encode(ism_id.to_bytes()));
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
    if s.eq_ignore_ascii_case("messageIdMultisig")
        || s.eq_ignore_ascii_case("message_id_multisig")
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
    *moonlight_nonce += 1;
    let mn = *moonlight_nonce;
    let dn = *deploy_nonce;
    *deploy_nonce += 1;

    let tx = moonlight_deployment(sk, bytecode, pk, init_args, gas_limit, gas_price, mn, dn, chain_id)?;
    let tx_bytes = tx.to_var_bytes();
    client
        .propagate_tx(&tx_bytes)
        .await
        .map_err(|e| format!("Failed to deploy {name}: {e}"))?;
    eprintln!("  {name} TX propagated");
    Ok(())
}

/// Parse a 32-byte hex string into a fixed-size array.
fn parse_bytes32(hex_str: &str) -> Result<[u8; 32], String> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(hex_str)
        .map_err(|e| format!("Invalid hex: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!("Expected 32 bytes (64 hex), got {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

/// Wait for the account's moonlight nonce to reach `expected_nonce`.
/// This ensures a TX is included in a block before sending the next one.
async fn wait_for_nonce(
    client: &RuesClient,
    pk: &BlsPublicKey,
    expected_nonce: u64,
) -> Result<(), String> {
    for attempt in 1..=20 {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let (current_nonce, _) = client.query_account(pk).await?;
        if current_nonce >= expected_nonce {
            return Ok(());
        }
        if attempt % 5 == 0 {
            eprintln!("  [nonce {current_nonce}/{expected_nonce}, attempt {attempt}/20]");
        }
    }
    Err(format!(
        "Account nonce did not reach {expected_nonce} after 60s"
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
    let client = RuesClient::new(rues_url);
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
        _ => return Err(format!("Unsupported return type: {return_type}. Use: u32, u64, bool, bytes32, bytes, string")),
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
    let client = RuesClient::new(rues_url);

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
        &sk, test_recipient_id, "dispatch_message", dispatch_args,
        gas_limit, gas_price, nonce + 1, chain_id,
    )?;

    client.propagate_tx(&tx.to_var_bytes()).await?;

    let output = json!({
        "success": true,
        "fn_name": "dispatch_message",
        "test_recipient": test_recipient_hex,
        "destination": destination,
        "recipient": recipient_hex,
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
    let client = RuesClient::new(rues_url);

    let mailbox_id = ContractId::from_bytes(parse_bytes32(mailbox_hex)?);

    let message_hex = message_hex.strip_prefix("0x").unwrap_or(message_hex);
    let encoded_message = hex::decode(message_hex)
        .map_err(|e| format!("Invalid message hex: {e}"))?;

    // Compute message ID for output
    let message_id = hyperlane_dusk_types::message::id(&encoded_message);

    // Serialize args: (metadata: Vec<u8>, encoded_message: Vec<u8>)
    let empty_metadata: Vec<u8> = Vec::new();
    let process_args = rkyv_serialize(&(empty_metadata, encoded_message));

    let chain_id = client.query_chain_id().await?;
    let (nonce, _balance) = client.query_account(&pk).await?;

    let tx = moonlight_call(
        &sk, mailbox_id, "process", process_args,
        gas_limit, gas_price, nonce + 1, chain_id,
    )?;

    client.propagate_tx(&tx.to_var_bytes()).await?;

    let output = json!({
        "success": true,
        "fn_name": "process",
        "mailbox": mailbox_hex,
        "message_id": hex::encode(message_id),
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
        version, nonce, origin, sender, destination, recipient, &body,
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
    let client = RuesClient::new(rues_url);

    let warp_id = ContractId::from_bytes(parse_bytes32(warp_contract_hex)?);
    let router = parse_bytes32(router_hex)?;

    // Serialize args: (domain: u32, router: H256)
    let enroll_args = rkyv_serialize(&(domain, router));

    let chain_id = client.query_chain_id().await?;
    let (nonce, _balance) = client.query_account(&pk).await?;

    let tx = moonlight_call(
        &sk, warp_id, "enroll_remote_router", enroll_args,
        gas_limit, gas_price, nonce + 1, chain_id,
    )?;

    client.propagate_tx(&tx.to_var_bytes()).await?;

    let output = json!({
        "success": true,
        "fn_name": "enroll_remote_router",
        "warp_contract": warp_contract_hex,
        "domain": domain,
        "router": router_hex,
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
    let client = RuesClient::new(rues_url);

    let warp_id = ContractId::from_bytes(parse_bytes32(warp_contract_hex)?);

    // No args — the contract reads the sender via abi::public_sender()
    let register_args = vec![];

    let chain_id = client.query_chain_id().await?;
    let (nonce, _balance) = client.query_account(&pk).await?;

    let tx = moonlight_call(
        &sk, warp_id, "register_account", register_args,
        gas_limit, gas_price, nonce + 1, chain_id,
    )?;

    client.propagate_tx(&tx.to_var_bytes()).await?;

    // Compute the H256 = keccak256(pk.to_bytes()) for display
    let pk_bytes = pk.to_bytes();
    let h256 = hyperlane_dusk_types::message::keccak256(&pk_bytes);

    let output = json!({
        "success": true,
        "fn_name": "register_account",
        "warp_contract": warp_contract_hex,
        "account_h256": hex::encode(h256),
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
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
    gas_limit: u64,
    gas_price: u64,
) -> Result<(), String> {
    let (sk, pk) = load_keys(keys_path, password, secret_key_hex, secret_key_stdin)?;
    let client = RuesClient::new(rues_url);

    let warp_id = ContractId::from_bytes(parse_bytes32(warp_contract_hex)?);
    let recipient = parse_bytes32(recipient_hex)?;

    // Serialize args: (destination: u32, recipient: H256, amount: u64)
    let transfer_args = rkyv_serialize(&(destination, recipient, amount));

    let chain_id = client.query_chain_id().await?;
    let (nonce, _balance) = client.query_account(&pk).await?;

    let tx = moonlight_call(
        &sk, warp_id, "transfer_remote", transfer_args,
        gas_limit, gas_price, nonce + 1, chain_id,
    )?;

    client.propagate_tx(&tx.to_var_bytes()).await?;

    let output = json!({
        "success": true,
        "fn_name": "transfer_remote",
        "warp_contract": warp_contract_hex,
        "destination": destination,
        "recipient": recipient_hex,
        "amount": amount,
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}
