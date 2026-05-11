//! End-to-end test for Hyperlane on Dusk.
//!
//! Deploys Hyperlane contracts to a live Rusk node (rusk-duskevm Docker)
//! and validates RUES queries using the same rkyv serialization protocol
//! as the hyperlane-dusk agent crate.

use dusk_bytes::Serializable;
use dusk_core::abi::ContractId;
use dusk_core::signatures::bls::{
    PublicKey as BlsPublicKey, SecretKey as BlsSecretKey,
};
use dusk_core::transfer::data::{ContractBytecode, ContractCall, ContractDeploy};
use dusk_core::transfer::moonlight::Transaction as MoonlightTransaction;
use dusk_core::transfer::Transaction;
use rkyv::ser::serializers::AllocSerializer;
use rkyv::ser::Serializer;
use rkyv::Serialize;

mod keys;
mod rues;

use rues::RuesClient;

// Contract WASM paths (built with `make` in dusk/)
const MAILBOX_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_mailbox.wasm"
);
const MERKLE_TREE_HOOK_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_merkle_tree_hook.wasm"
);
const ISM_MULTISIG_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_ism_multisig.wasm"
);
const VALIDATOR_ANNOUNCE_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_validator_announce.wasm"
);
const TEST_RECIPIENT_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_test_recipient.wasm"
);
const TEST_MOCK_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_test_mock.wasm"
);

// Default RUES URL for the rusk-duskevm Docker container
const DEFAULT_RUES_URL: &str = "http://localhost:18090/";

// Consensus keys file and password
const CONSENSUS_KEYS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/consensus.keys"
);
const CONSENSUS_KEYS_PASSWORD: &str = "password";

/// Generate a contract ID from bytecode, nonce, and owner.
/// This mirrors `dusk-vm`'s `gen_contract_id` using blake2b.
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

/// Build a Moonlight deployment transaction.
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
        None, // no receiver
        0,    // no transfer value
        0,    // no deposit
        gas_limit,
        gas_price,
        moonlight_nonce,
        chain_id,
        Some(deploy),
    )
    .map_err(|e| format!("MoonlightTransaction::new failed: {e:?}"))?;
    Ok(tx.into())
}

/// Build a Moonlight contract-call transaction.
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
        None, // no receiver
        0,    // no transfer value
        0,    // no deposit
        gas_limit,
        gas_price,
        moonlight_nonce,
        chain_id,
        Some(call),
    )
    .map_err(|e| format!("MoonlightTransaction::new failed: {e:?}"))?;
    Ok(tx.into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rues_url = std::env::var("RUES_URL").unwrap_or(DEFAULT_RUES_URL.into());
    println!("=== Hyperlane-Dusk E2E Test ===");
    println!("RUES URL: {rues_url}");

    let client = RuesClient::new(&rues_url);

    // Step 1: Test basic RUES connectivity
    println!("\n--- Step 1: Test RUES connectivity ---");
    let chain_id = client.query_chain_id().await?;
    println!("Chain ID: {chain_id}");

    // Step 2: Load deployer key
    println!("\n--- Step 2: Load deployer key ---");
    let keys_path =
        std::env::var("CONSENSUS_KEYS").unwrap_or(CONSENSUS_KEYS_PATH.into());
    let keys_password =
        std::env::var("CONSENSUS_KEYS_PASS").unwrap_or(CONSENSUS_KEYS_PASSWORD.into());
    let (sk, pk) = keys::load_bls_keys(&keys_path, &keys_password)?;
    let pk_short = hex::encode(&pk.to_bytes()[..8]);
    println!("Deployer public key: {pk_short}...");

    // Step 3: Check deployer account
    println!("\n--- Step 3: Check deployer account ---");
    let (nonce, balance) = client.query_account(&pk).await?;
    println!("Account nonce: {nonce}, balance: {balance} LUX");
    if balance == 0 {
        return Err("Deployer account has zero balance — cannot deploy contracts".into());
    }

    // Step 4: Deploy contracts
    println!("\n--- Step 4: Deploy Hyperlane contracts ---");
    let gas_limit = 500_000_000u64;
    let gas_price = 2000u64;
    let mut deploy_nonce = 0u64;
    let mut moonlight_nonce = nonce;

    // Helper: deploy a contract and return its ID
    async fn deploy_contract(
        client: &RuesClient,
        sk: &BlsSecretKey,
        pk: &BlsPublicKey,
        wasm_path: &str,
        init_args: Vec<u8>,
        moonlight_nonce: &mut u64,
        deploy_nonce: &mut u64,
        chain_id: u8,
        gas_limit: u64,
        gas_price: u64,
        name: &str,
    ) -> Result<ContractId, Box<dyn std::error::Error>> {
        let bytecode = std::fs::read(wasm_path)
            .map_err(|e| format!("Failed to read {name} WASM at {wasm_path}: {e}"))?;
        println!("  Deploying {name} ({} bytes)...", bytecode.len());

        *moonlight_nonce += 1;
        let tx = moonlight_deployment(
            sk,
            bytecode.clone(),
            pk,
            init_args,
            gas_limit,
            gas_price,
            *moonlight_nonce,
            *deploy_nonce,
            chain_id,
        )?;

        let contract_id =
            gen_contract_id(&bytecode, *deploy_nonce, &pk.to_bytes());
        *deploy_nonce += 1;

        // Serialize and propagate
        let tx_bytes = tx.to_var_bytes();
        client
            .propagate_tx(&tx_bytes)
            .await
            .map_err(|e| format!("Failed to propagate {name} deploy TX: {e}"))?;

        println!(
            "  {name} deployed! Contract ID: {}",
            hex::encode(&contract_id.to_bytes())
        );
        Ok(contract_id)
    }

    let local_domain: u32 = 4242; // test domain
    let pk_bytes = pk.to_bytes();

    // Pre-compute all contract IDs (deterministic from bytecode + deploy_nonce + owner_pk)
    let test_mock_bytecode = std::fs::read(TEST_MOCK_WASM)?;
    let test_recipient_bytecode = std::fs::read(TEST_RECIPIENT_WASM)?;
    let merkle_tree_hook_bytecode = std::fs::read(MERKLE_TREE_HOOK_WASM)?;
    let mailbox_bytecode = std::fs::read(MAILBOX_WASM)?;
    let va_bytecode = std::fs::read(VALIDATOR_ANNOUNCE_WASM)?;

    // Deploy order: TestMock(0), TestRecipient(1), MerkleTreeHook(2), Mailbox(3), VA(4)
    let test_mock_id = gen_contract_id(&test_mock_bytecode, 0, &pk_bytes);
    let test_recipient_id = gen_contract_id(&test_recipient_bytecode, 1, &pk_bytes);
    let merkle_tree_hook_id = gen_contract_id(&merkle_tree_hook_bytecode, 2, &pk_bytes);
    let mailbox_id = gen_contract_id(&mailbox_bytecode, 3, &pk_bytes);
    let _va_id = gen_contract_id(&va_bytecode, 4, &pk_bytes);

    println!("  Pre-computed contract IDs:");
    println!("    TestMock:        {}", hex::encode(&test_mock_id.to_bytes()));
    println!("    MerkleTreeHook:  {}", hex::encode(&merkle_tree_hook_id.to_bytes()));
    println!("    Mailbox:         {}", hex::encode(&mailbox_id.to_bytes()));

    // 1. Deploy TestMock (no init args)
    deploy_contract(
        &client, &sk, &pk, TEST_MOCK_WASM, vec![],
        &mut moonlight_nonce, &mut deploy_nonce,
        chain_id, gas_limit, gas_price, "TestMock",
    ).await?;

    // 2. Deploy TestRecipient (no init args)
    deploy_contract(
        &client, &sk, &pk, TEST_RECIPIENT_WASM, vec![],
        &mut moonlight_nonce, &mut deploy_nonce,
        chain_id, gas_limit, gas_price, "TestRecipient",
    ).await?;

    // 3. Deploy MerkleTreeHook: init(mailbox: ContractId)
    let mth_init = rkyv_serialize(&(mailbox_id,));
    deploy_contract(
        &client, &sk, &pk, MERKLE_TREE_HOOK_WASM, mth_init,
        &mut moonlight_nonce, &mut deploy_nonce,
        chain_id, gas_limit, gas_price, "MerkleTreeHook",
    ).await?;

    // 4. Deploy Mailbox: init(local_domain, owner, default_ism, default_hook, required_hook)
    let mailbox_init = rkyv_serialize(&(
        local_domain,
        mailbox_id,            // owner = mailbox itself
        test_mock_id,          // default ISM
        test_mock_id,          // default hook (noop)
        merkle_tree_hook_id,   // required hook
    ));
    deploy_contract(
        &client, &sk, &pk, MAILBOX_WASM, mailbox_init,
        &mut moonlight_nonce, &mut deploy_nonce,
        chain_id, gas_limit, gas_price, "Mailbox",
    ).await?;

    // 5. Deploy ValidatorAnnounce: init(local_domain, mailbox)
    let va_init = rkyv_serialize(&(local_domain, mailbox_id));
    deploy_contract(
        &client, &sk, &pk, VALIDATOR_ANNOUNCE_WASM, va_init,
        &mut moonlight_nonce, &mut deploy_nonce,
        chain_id, gas_limit, gas_price, "ValidatorAnnounce",
    ).await?;

    // Wait for all contracts to be deployed (poll Mailbox nonce query)
    println!("  Waiting for block inclusion (polling)...");
    for attempt in 1..=15 {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        let result: Result<u32, _> =
            client.contract_query(&mailbox_id.to_bytes(), "nonce", &()).await;
        if result.is_ok() {
            println!("  Contracts confirmed on-chain (attempt {attempt})");
            break;
        }
        if attempt == 15 {
            return Err("Contracts not deployed after 60s".into());
        }
        println!("  [attempt {attempt}] Not yet...");
    }

    // Step 5: Test RUES queries against deployed contracts
    println!("\n--- Step 5: Test RUES queries ---");

    // Test mailbox nonce
    let mailbox_bytes = mailbox_id.to_bytes();
    let nonce_result: u32 =
        client.contract_query(&mailbox_bytes, "nonce", &()).await?;
    println!("  Mailbox nonce: {nonce_result}");
    assert_eq!(nonce_result, 0, "Initial nonce should be 0");

    // Test mailbox local_domain
    let domain: u32 = client
        .contract_query(&mailbox_bytes, "local_domain", &())
        .await?;
    println!("  Mailbox local_domain: {domain}");
    assert_eq!(domain, local_domain, "Local domain mismatch");

    // Test mailbox default_ism
    let default_ism: [u8; 32] = client
        .contract_query(&mailbox_bytes, "default_ism", &())
        .await?;
    println!(
        "  Mailbox default_ism: {}",
        hex::encode(&default_ism[..8])
    );
    assert_eq!(
        default_ism,
        test_mock_id.to_bytes(),
        "Default ISM mismatch"
    );

    // Test merkle tree hook count
    let hook_bytes = merkle_tree_hook_id.to_bytes();
    let count: u32 = client.contract_query(&hook_bytes, "count", &()).await?;
    println!("  MerkleTreeHook count: {count}");
    assert_eq!(count, 0, "Initial merkle tree count should be 0");

    // Test delivered (should be false for random message ID)
    let random_id = [42u8; 32];
    let delivered: bool = client
        .contract_query(&mailbox_bytes, "delivered", &random_id)
        .await?;
    println!("  Delivered(random): {delivered}");
    assert!(!delivered, "Random message should not be delivered");

    // Step 6: Test dispatch via TestRecipient proxy
    // (Direct Moonlight TX → Mailbox.dispatch would panic because
    //  abi::caller() returns None for external calls. Instead, we call
    //  TestRecipient.dispatch_message which forwards to Mailbox, so
    //  abi::caller() returns TestRecipient's ContractId.)
    println!("\n--- Step 6: Test dispatch via TestRecipient proxy ---");
    {
        let dest_domain: u32 = 1; // Ethereum
        let recipient: [u8; 32] = [0xAA; 32];
        let body: Vec<u8> = b"Hello from Dusk!".to_vec();
        let dispatch_args = rkyv_serialize(&(mailbox_id, dest_domain, recipient, body));

        moonlight_nonce += 1;
        let tx = moonlight_call(
            &sk,
            test_recipient_id,
            "dispatch_message",
            dispatch_args,
            gas_limit,
            gas_price,
            moonlight_nonce,
            chain_id,
        )?;

        let tx_bytes = tx.to_var_bytes();
        client
            .propagate_tx(&tx_bytes)
            .await
            .map_err(|e| format!("Failed to propagate dispatch TX: {e}"))?;

        println!("  Dispatch TX propagated, waiting for inclusion...");
    }

    // Wait for dispatch to be included, polling with retries
    let mut new_nonce: u32 = 0;
    for attempt in 1..=5 {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        new_nonce = client
            .contract_query(&mailbox_bytes, "nonce", &())
            .await?;
        println!("  [attempt {attempt}] Mailbox nonce: {new_nonce}");
        if new_nonce > 0 {
            break;
        }
    }
    assert_eq!(new_nonce, 1, "Nonce should be 1 after dispatch");

    // Verify merkle tree hook count incremented
    let new_count: u32 = client.contract_query(&hook_bytes, "count", &()).await?;
    println!("  MerkleTreeHook count after dispatch: {new_count}");
    assert_eq!(
        new_count, 1,
        "Merkle tree count should be 1 after dispatch"
    );

    // Test dispatched_message query (agent indexer path)
    let nonce_to_query: u32 = 0;
    let dispatched_msg: Vec<u8> = client
        .contract_query(&mailbox_bytes, "dispatched_message", &nonce_to_query)
        .await?;
    println!("  dispatched_message(0): {} bytes", dispatched_msg.len());
    assert!(
        !dispatched_msg.is_empty(),
        "Dispatched message should not be empty"
    );

    // Decode the message using hyperlane-dusk-types
    if let Some(msg) = hyperlane_dusk_types::message::decode(&dispatched_msg) {
        println!("  Decoded message:");
        println!("    version: {}", msg.version);
        println!("    nonce: {}", msg.nonce);
        println!("    origin: {}", msg.origin);
        println!("    sender: {}", hex::encode(&msg.sender[..8]));
        println!("    destination: {}", msg.destination);
        println!("    recipient: {}", hex::encode(&msg.recipient[..8]));
        println!("    body length: {}", msg.body.len());
        assert_eq!(msg.nonce, 0);
        assert_eq!(msg.origin, local_domain);
        assert_eq!(msg.destination, 1); // Ethereum
    } else {
        return Err("Failed to decode dispatched message".into());
    }

    // Test latest_checkpoint on merkle tree hook
    let checkpoint: ([u8; 32], u32) = client
        .contract_query(&hook_bytes, "latest_checkpoint", &())
        .await?;
    println!(
        "  MerkleTreeHook latest_checkpoint: root={}, index={}",
        hex::encode(&checkpoint.0[..8]),
        checkpoint.1
    );
    assert_eq!(
        checkpoint.1, 0,
        "Checkpoint index should be 0 (one message, zero-indexed)"
    );

    // Step 7: Test direct Moonlight TX dispatch (sender = keccak256(BLS pk))
    println!("\n--- Step 7: Test direct dispatch via Moonlight TX ---");
    {
        let dest_domain: u32 = 2;
        let recipient: [u8; 32] = [0xBB; 32];
        let body: Vec<u8> = b"Direct from Moonlight!".to_vec();
        let dispatch_args = rkyv_serialize(&(dest_domain, recipient, body));

        moonlight_nonce += 1;
        let tx = moonlight_call(
            &sk,
            mailbox_id,
            "dispatch_default",
            dispatch_args,
            gas_limit,
            gas_price,
            moonlight_nonce,
            chain_id,
        )?;

        let tx_bytes = tx.to_var_bytes();
        client
            .propagate_tx(&tx_bytes)
            .await
            .map_err(|e| format!("Failed to propagate direct dispatch TX: {e}"))?;

        println!("  Direct dispatch TX propagated, waiting for inclusion...");
    }

    // Wait for the second dispatch
    for attempt in 1..=5 {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        new_nonce = client
            .contract_query(&mailbox_bytes, "nonce", &())
            .await?;
        println!("  [attempt {attempt}] Mailbox nonce: {new_nonce}");
        if new_nonce >= 2 {
            break;
        }
    }
    assert_eq!(new_nonce, 2, "Nonce should be 2 after second dispatch");

    // Verify the second dispatched message
    let nonce_to_query: u32 = 1;
    let dispatched_msg2: Vec<u8> = client
        .contract_query(&mailbox_bytes, "dispatched_message", &nonce_to_query)
        .await?;
    if let Some(msg) = hyperlane_dusk_types::message::decode(&dispatched_msg2) {
        println!("  Decoded direct dispatch message:");
        println!("    sender (keccak(BLS pk)): {}", hex::encode(&msg.sender));
        println!("    destination: {}", msg.destination);
        assert_eq!(msg.nonce, 1);
        assert_eq!(msg.destination, 2);
        // Sender should be keccak256 of the BLS public key, not a ContractId
        assert_ne!(msg.sender, [0u8; 32], "Sender should be non-zero");
        assert_ne!(
            msg.sender,
            test_recipient_id.to_bytes(),
            "Sender should NOT be TestRecipient (this is a direct call)"
        );
    } else {
        return Err("Failed to decode second dispatched message".into());
    }

    println!("\n=== ALL E2E TESTS PASSED ===");
    Ok(())
}

/// Serialize a value using rkyv.
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
