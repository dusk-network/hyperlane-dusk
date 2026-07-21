// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Integration tests for Hyperlane Dusk contracts.
//
// These tests deploy the Mailbox, MerkleTreeHook, TestMock (NullISM + NoopHook),
// and TestRecipient contracts into a dusk-vm session and test the full
// dispatch → process lifecycle.
//
// IMPORTANT: Contract WASMs must be built before running these tests.
//   Run `make all` in the dusk/ directory first.

extern crate alloc;

use std::sync::LazyLock;

use dusk_bytes::Serializable;
use dusk_core::abi::{ContractError, ContractId};
use dusk_core::dusk;
use dusk_core::signatures::bls::{PublicKey as AccountPublicKey, SecretKey as AccountSecretKey};
use dusk_core::transfer::ReceiveFromContract;
use dusk_data_driver::ConvertibleContract;
use dusk_vm::{CallReceipt, Error as VMError};
use rand::rngs::StdRng;
use rand::SeedableRng;
use secp256k1::{Message as SecpMessage, Secp256k1, SecretKey as SecpSecretKey};

use hyperlane_dusk_data_driver::HyperlaneDataDriver;
use hyperlane_dusk_types::drc20::{
    Account as Drc20Account, Allowance as Drc20Allowance, ApproveCall as Drc20ApproveCall,
    BalanceOf as Drc20BalanceOf,
};
use hyperlane_dusk_types::{
    events, message, DomainGasConfig, EthAddress, GasPaymentRecord, MessageId, H256, VERSION,
};

mod test_session;
use test_session::{assert_contract_panic, TestSession};

fn assert_contract_panic_contains<R>(
    call_result: Result<CallReceipt<R>, ContractError>,
    expected_panic_part: &str,
) where
    R: rkyv::Archive,
    R::Archived: rkyv::Deserialize<R, rkyv::Infallible>
        + for<'b> rkyv::bytecheck::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
{
    let contract_err = match call_result {
        Ok(_) => panic!("Contract call shouldn't pass"),
        Err(error) => error,
    };

    if let ContractError::Panic(panic_msg) = contract_err {
        assert!(
            panic_msg.contains(expected_panic_part),
            "panic `{panic_msg}` did not contain `{expected_panic_part}`"
        );
    } else {
        panic!("Expected contract panic, got error: {contract_err}");
    }
}

// =============================================================================
// Contract bytecodes (must be built via `make all` first)
// =============================================================================

const MAILBOX_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_mailbox.wasm"
);
const MERKLE_TREE_HOOK_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_merkle_tree_hook.wasm"
);
const AGGREGATION_HOOK_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_aggregation_hook.wasm"
);
const TEST_MOCK_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_test_mock.wasm"
);
const TEST_RECIPIENT_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_test_recipient.wasm"
);
const PROTOCOL_FEE_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_protocol_fee.wasm"
);
const IGP_BYTECODE: &[u8] =
    include_bytes!("../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_igp.wasm");
const ISM_MULTISIG_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_ism_multisig.wasm"
);
const VALIDATOR_ANNOUNCE_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_validator_announce.wasm"
);
const CANONICAL_DRC20_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/canonical_drc20_roles_pausable.wasm"
);

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone)]
#[archive_attr(derive(bytecheck::CheckBytes))]
struct CanonicalDrc20InitBalance {
    account: Drc20Account,
    amount: u64,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone)]
#[archive_attr(derive(bytecheck::CheckBytes))]
struct CanonicalDrc20TokenInit {
    name: alloc::string::String,
    symbol: alloc::string::String,
    decimals: u8,
    initial_balances: Vec<CanonicalDrc20InitBalance>,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone)]
#[archive_attr(derive(bytecheck::CheckBytes))]
struct CanonicalDrc20ExampleInit {
    admin: Drc20Account,
    token: CanonicalDrc20TokenInit,
    cap: u64,
}

// =============================================================================
// Contract IDs (fixed for deterministic tests)
// =============================================================================

const MAILBOX_ID: ContractId = ContractId::from_bytes([10; 32]);
const MERKLE_TREE_HOOK_ID: ContractId = ContractId::from_bytes([11; 32]);
const TEST_MOCK_ID: ContractId = ContractId::from_bytes([12; 32]);
const TEST_RECIPIENT_ID: ContractId = ContractId::from_bytes([13; 32]);
const PROTOCOL_FEE_ID: ContractId = ContractId::from_bytes([14; 32]);
const IGP_ID: ContractId = ContractId::from_bytes([15; 32]);
const ISM_MULTISIG_ID: ContractId = ContractId::from_bytes([16; 32]);
const AGGREGATION_HOOK_ID: ContractId = ContractId::from_bytes([19; 32]);
const VALIDATOR_ANNOUNCE_ID: ContractId = ContractId::from_bytes([20; 32]);

const DEPLOYER: [u8; 64] = [0u8; 64];
const INITIAL_DUSK_BALANCE: u64 = dusk(1_000.0);
const WITHDRAW_DEFAULT_GAS_LIMIT: u64 = 30_000_000;

/// Local domain for the Dusk chain in tests.
const LOCAL_DOMAIN: u32 = 4242;

/// Remote (origin) domain for incoming messages in tests.
const REMOTE_DOMAIN: u32 = 1;

// =============================================================================
// Test accounts
// =============================================================================

static OWNER_SK: LazyLock<AccountSecretKey> = LazyLock::new(|| {
    let mut rng = StdRng::seed_from_u64(0x48595045); // "HYPE"
    AccountSecretKey::random(&mut rng)
});

static OWNER_PK: LazyLock<AccountPublicKey> = LazyLock::new(|| AccountPublicKey::from(&*OWNER_SK));

static OWNER_ID: LazyLock<H256> = LazyLock::new(|| message::keccak256(&OWNER_PK.to_bytes()));

static RELAYER_SK: LazyLock<AccountSecretKey> = LazyLock::new(|| {
    let mut rng = StdRng::seed_from_u64(0xDE1A7E00); // "DELAYE"
    AccountSecretKey::random(&mut rng)
});

static RELAYER_PK: LazyLock<AccountPublicKey> =
    LazyLock::new(|| AccountPublicKey::from(&*RELAYER_SK));

static RELAYER_ID: LazyLock<H256> = LazyLock::new(|| message::keccak256(&RELAYER_PK.to_bytes()));

// =============================================================================
// Test session wrapper
// =============================================================================

/// Wraps a TestSession with deployed Hyperlane contracts.
struct HyperlaneSession {
    session: TestSession,
}

impl HyperlaneSession {
    /// Deploy and initialize all Hyperlane contracts.
    ///
    /// The `#[dusk_forge::contract]` macro wires the `init()` method as the
    /// WASM init export, so `init_arg` during deployment IS the initialization.
    fn new() -> Self {
        let mut session = TestSession::instantiate(vec![
            (&*OWNER_PK, INITIAL_DUSK_BALANCE),
            (&*RELAYER_PK, INITIAL_DUSK_BALANCE),
        ]);

        // Deploy contracts without init methods first (no init_arg needed)
        session
            .deploy(
                TEST_MOCK_BYTECODE,
                dusk_vm::ContractData::builder()
                    .owner(DEPLOYER)
                    .contract_id(TEST_MOCK_ID),
            )
            .expect("Deploying TestMock should succeed");

        session
            .deploy(
                TEST_RECIPIENT_BYTECODE,
                dusk_vm::ContractData::builder()
                    .owner(DEPLOYER)
                    .contract_id(TEST_RECIPIENT_ID),
            )
            .expect("Deploying TestRecipient should succeed");

        // Deploy MerkleTreeHook with init_arg (calls init(mailbox))
        session
            .deploy(
                MERKLE_TREE_HOOK_BYTECODE,
                dusk_vm::ContractData::builder()
                    .owner(DEPLOYER)
                    .init_arg(&(MAILBOX_ID,))
                    .contract_id(MERKLE_TREE_HOOK_ID),
            )
            .expect("Deploying MerkleTreeHook should succeed");

        // Deploy Mailbox with init_arg (calls init(local_domain, owner, ism, hook, required_hook))
        session
            .deploy(
                MAILBOX_BYTECODE,
                dusk_vm::ContractData::builder()
                    .owner(DEPLOYER)
                    .init_arg(&(
                        LOCAL_DOMAIN,
                        *OWNER_ID,           // owner = deployer Moonlight account
                        TEST_MOCK_ID,        // default ISM
                        TEST_MOCK_ID,        // default hook (noop)
                        MERKLE_TREE_HOOK_ID, // required hook
                    ))
                    .contract_id(MAILBOX_ID),
            )
            .expect("Deploying Mailbox should succeed");

        Self { session }
    }

    // =================================================================
    // Mailbox queries
    // =================================================================

    fn mailbox_local_domain(&mut self) -> u32 {
        self.session
            .direct_call::<_, u32>(MAILBOX_ID, "local_domain", &())
            .expect("local_domain should succeed")
            .data
    }

    fn mailbox_nonce(&mut self) -> u32 {
        self.session
            .direct_call::<_, u32>(MAILBOX_ID, "nonce", &())
            .expect("nonce should succeed")
            .data
    }

    fn mailbox_latest_dispatched_id(&mut self) -> MessageId {
        self.session
            .direct_call::<_, MessageId>(MAILBOX_ID, "latest_dispatched_id", &())
            .expect("latest_dispatched_id should succeed")
            .data
    }

    fn mailbox_delivered(&mut self, id: MessageId) -> bool {
        self.session
            .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(id,))
            .expect("delivered should succeed")
            .data
    }

    fn mailbox_default_ism(&mut self) -> ContractId {
        self.session
            .direct_call::<_, ContractId>(MAILBOX_ID, "default_ism", &())
            .expect("default_ism should succeed")
            .data
    }

    fn mailbox_default_hook(&mut self) -> ContractId {
        self.session
            .direct_call::<_, ContractId>(MAILBOX_ID, "default_hook", &())
            .expect("default_hook should succeed")
            .data
    }

    fn mailbox_required_hook(&mut self) -> ContractId {
        self.session
            .direct_call::<_, ContractId>(MAILBOX_ID, "required_hook", &())
            .expect("required_hook should succeed")
            .data
    }

    fn mailbox_dispatched_message(&mut self, nonce: u32) -> Vec<u8> {
        self.session
            .direct_call::<_, Vec<u8>>(MAILBOX_ID, "dispatched_message", &(nonce,))
            .expect("dispatched_message should succeed")
            .data
    }

    fn mailbox_processed_at_index(&mut self, index: u32) -> MessageId {
        self.session
            .direct_call::<_, MessageId>(MAILBOX_ID, "processed_at_index", &(index,))
            .expect("processed_at_index should succeed")
            .data
    }

    fn mailbox_processed_count(&mut self) -> u32 {
        self.session
            .direct_call::<_, u32>(MAILBOX_ID, "processed_count", &())
            .expect("processed_count should succeed")
            .data
    }

    // =================================================================
    // Mailbox dispatch
    // =================================================================

    /// Dispatch via Moonlight TX (sender = keccak256(BLS public key)).
    fn mailbox_dispatch_via_tx(
        &mut self,
        sender_sk: &AccountSecretKey,
        destination: u32,
        recipient: H256,
        body: Vec<u8>,
    ) -> Result<CallReceipt<MessageId>, dusk_core::abi::ContractError> {
        self.session.call_public::<_, MessageId>(
            sender_sk,
            MAILBOX_ID,
            "dispatch_default",
            &(destination, recipient, body),
        )
    }

    /// Dispatch via TestRecipient proxy (sender = TestRecipient ContractId).
    fn mailbox_dispatch_via_recipient(
        &mut self,
        sender_sk: &AccountSecretKey,
        destination: u32,
        recipient: H256,
        body: Vec<u8>,
    ) -> Result<CallReceipt<MessageId>, dusk_core::abi::ContractError> {
        self.session.call_public::<_, MessageId>(
            sender_sk,
            TEST_RECIPIENT_ID,
            "dispatch_message",
            &(MAILBOX_ID, destination, recipient, body),
        )
    }

    fn mailbox_process(
        &mut self,
        metadata: Vec<u8>,
        encoded_message: Vec<u8>,
    ) -> Result<CallReceipt<()>, dusk_core::abi::ContractError> {
        self.session
            .direct_call::<_, ()>(MAILBOX_ID, "process", &(metadata, encoded_message))
    }

    fn mailbox_process_via_tx(
        &mut self,
        metadata: Vec<u8>,
        encoded_message: Vec<u8>,
    ) -> Result<CallReceipt<()>, dusk_core::abi::ContractError> {
        self.session.call_public::<_, ()>(
            &RELAYER_SK,
            MAILBOX_ID,
            "process",
            &(metadata, encoded_message),
        )
    }

    // =================================================================
    // MerkleTreeHook queries
    // =================================================================

    #[allow(dead_code)]
    fn merkle_root(&mut self) -> H256 {
        self.session
            .direct_call::<_, H256>(MERKLE_TREE_HOOK_ID, "root", &())
            .expect("root should succeed")
            .data
    }

    fn merkle_count(&mut self) -> u32 {
        self.session
            .direct_call::<_, u32>(MERKLE_TREE_HOOK_ID, "count", &())
            .expect("count should succeed")
            .data
    }

    #[allow(dead_code)]
    fn merkle_latest_checkpoint(&mut self) -> (H256, u32) {
        self.session
            .direct_call::<_, (H256, u32)>(MERKLE_TREE_HOOK_ID, "latest_checkpoint", &())
            .expect("latest_checkpoint should succeed")
            .data
    }

    // =================================================================
    // TestRecipient queries
    // =================================================================

    fn recipient_last_origin(&mut self) -> u32 {
        self.session
            .direct_call::<_, u32>(TEST_RECIPIENT_ID, "last_origin", &())
            .expect("last_origin should succeed")
            .data
    }

    fn recipient_last_sender(&mut self) -> H256 {
        self.session
            .direct_call::<_, H256>(TEST_RECIPIENT_ID, "last_sender", &())
            .expect("last_sender should succeed")
            .data
    }

    fn recipient_last_body(&mut self) -> Vec<u8> {
        self.session
            .direct_call::<_, Vec<u8>>(TEST_RECIPIENT_ID, "last_body", &())
            .expect("last_body should succeed")
            .data
    }

    fn recipient_handled_count(&mut self) -> u32 {
        self.session
            .direct_call::<_, u32>(TEST_RECIPIENT_ID, "handled_count", &())
            .expect("handled_count should succeed")
            .data
    }

    // =================================================================
    // TestMock queries
    // =================================================================

    fn mock_verify_count(&mut self) -> u32 {
        self.session
            .direct_call::<_, u32>(TEST_MOCK_ID, "verify_count", &())
            .expect("verify_count should succeed")
            .data
    }

    #[allow(dead_code)]
    fn mock_post_dispatch_count(&mut self) -> u32 {
        self.session
            .direct_call::<_, u32>(TEST_MOCK_ID, "post_dispatch_count", &())
            .expect("post_dispatch_count should succeed")
            .data
    }
}

fn assert_deploy_panic(result: Result<ContractId, VMError>, expected_panic: &str) {
    match result {
        Err(VMError::Panic(panic_msg)) => assert_eq!(panic_msg, expected_panic),
        Err(error) => panic!("Expected deploy panic, got error: {error}"),
        Ok(contract_id) => panic!("Deploy should have failed, got {contract_id:?}"),
    }
}

fn sample_encoded_message(recipient: ContractId) -> Vec<u8> {
    message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        LOCAL_DOMAIN,
        recipient.to_bytes(),
        b"hello",
    )
}

// =============================================================================
// Tests: Mailbox initialization
// =============================================================================

#[test]
fn test_mailbox_init() {
    let mut s = HyperlaneSession::new();

    for (contract, label) in [
        (MAILBOX_ID, "Mailbox"),
        (TEST_MOCK_ID, "TestMock"),
        (TEST_RECIPIENT_ID, "TestRecipient"),
    ] {
        let version = s
            .session
            .direct_call::<_, u32>(contract, "state_version", &())
            .unwrap_or_else(|_| panic!("{label} state_version should succeed"))
            .data;
        let expected = if contract == MAILBOX_ID { 2 } else { 1 };
        assert_eq!(version, expected, "unexpected {label} state version");
    }

    assert_eq!(s.mailbox_local_domain(), LOCAL_DOMAIN);
    assert_eq!(s.mailbox_nonce(), 0);
    assert_eq!(s.mailbox_latest_dispatched_id(), [0u8; 32]);
    assert_eq!(s.mailbox_default_ism(), TEST_MOCK_ID);
    assert_eq!(s.mailbox_default_hook(), TEST_MOCK_ID);
    assert_eq!(s.mailbox_required_hook(), MERKLE_TREE_HOOK_ID);
}

#[test]
fn test_mailbox_double_init_panics() {
    // The VM disallows calling `init` after deployment, so we verify that
    // the init export is a one-shot entry point enforced by the VM itself.
    let mut s = HyperlaneSession::new();

    let result = s.session.direct_call::<_, ()>(
        MAILBOX_ID,
        "init",
        &(
            LOCAL_DOMAIN,
            *OWNER_ID,
            TEST_MOCK_ID,
            TEST_MOCK_ID,
            MERKLE_TREE_HOOK_ID,
        ),
    );

    // The VM prevents calling init after deployment
    assert!(result.is_err(), "Calling init after deployment should fail");
}

#[test]
fn test_mailbox_rejects_duplicate_required_hook_configuration() {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);
    let result = session.deploy(
        MAILBOX_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                LOCAL_DOMAIN,
                *OWNER_ID,
                TEST_MOCK_ID,
                MERKLE_TREE_HOOK_ID,
                MERKLE_TREE_HOOK_ID,
            ))
            .contract_id(MAILBOX_ID),
    );
    assert_deploy_panic(result, "Mailbox: default and required hooks must differ");
}

#[test]
fn test_mailbox_admin_accepts_owner_and_rejects_non_owner() {
    let mut s = HyperlaneSession::new();

    s.session
        .call_public::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "set_default_hook",
            &(TEST_RECIPIENT_ID,),
        )
        .expect("Mailbox owner should update the default hook");
    assert_eq!(s.mailbox_default_hook(), TEST_RECIPIENT_ID);

    let result = s.session.call_public::<_, ()>(
        &RELAYER_SK,
        MAILBOX_ID,
        "set_default_hook",
        &(TEST_MOCK_ID,),
    );
    assert_contract_panic(result, "Mailbox: caller is not the owner");

    let result = s.session.call_public::<_, ()>(
        &OWNER_SK,
        MAILBOX_ID,
        "set_default_hook",
        &(MERKLE_TREE_HOOK_ID,),
    );
    assert_contract_panic(result, "Mailbox: default and required hooks must differ");

    let result = s.session.call_public::<_, ()>(
        &OWNER_SK,
        MAILBOX_ID,
        "set_required_hook",
        &(TEST_RECIPIENT_ID,),
    );
    assert_contract_panic(result, "Mailbox: default and required hooks must differ");
}

// =============================================================================
// Tests: MerkleTreeHook initialization
// =============================================================================

#[test]
fn test_merkle_tree_hook_init() {
    let mut s = HyperlaneSession::new();

    assert_eq!(s.merkle_count(), 0);

    let mailbox = s
        .session
        .direct_call::<_, ContractId>(MERKLE_TREE_HOOK_ID, "mailbox", &())
        .expect("mailbox query should succeed")
        .data;
    assert_eq!(mailbox, MAILBOX_ID);
}

#[test]
fn test_merkle_tree_hook_double_init_panics() {
    // The VM disallows calling `init` after deployment.
    let mut s = HyperlaneSession::new();

    let result = s
        .session
        .direct_call::<_, ()>(MERKLE_TREE_HOOK_ID, "init", &(MAILBOX_ID,));

    assert!(result.is_err(), "Calling init after deployment should fail");
}

// =============================================================================
// Tests: Message processing (inbound)
// =============================================================================

#[test]
fn test_process_delivers_message() {
    let mut s = HyperlaneSession::new();

    // Build an inbound message from REMOTE_DOMAIN → LOCAL_DOMAIN
    let sender = [0xABu8; 32];
    let body = b"Hello from Ethereum!".to_vec();

    let encoded = message::encode(
        VERSION,
        0,             // nonce
        REMOTE_DOMAIN, // origin
        sender,
        LOCAL_DOMAIN,                 // destination
        TEST_RECIPIENT_ID.to_bytes(), // recipient
        &body,
    );

    let message_id = message::id(&encoded);

    // Process (deliver) the message
    let receipt = s
        .mailbox_process(Vec::new(), encoded)
        .expect("process should succeed");

    // Verify message was delivered
    assert!(s.mailbox_delivered(message_id));

    // Verify TestRecipient received the message
    assert_eq!(s.recipient_last_origin(), REMOTE_DOMAIN);
    assert_eq!(s.recipient_last_sender(), sender);
    assert_eq!(s.recipient_last_body(), body);
    assert_eq!(s.recipient_handled_count(), 1);

    // Verify NullISM's verify was called
    assert_eq!(s.mock_verify_count(), 1);

    // Verify events were emitted
    let process_events: Vec<_> = receipt
        .events
        .iter()
        .filter(|e| e.topic.contains("process"))
        .collect();
    assert!(!process_events.is_empty(), "process should emit events");
}

#[test]
fn test_process_rejects_wrong_destination() {
    let mut s = HyperlaneSession::new();

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        42, // wrong destination
        TEST_RECIPIENT_ID.to_bytes(),
        b"hello",
    );

    let result = s.mailbox_process(Vec::new(), encoded);
    assert_contract_panic(result, "Mailbox: unexpected destination");
}

#[test]
fn test_process_rejects_wrong_version() {
    let mut s = HyperlaneSession::new();

    // Build a message with wrong version (99). The types crate's decode()
    // rejects non-matching versions by returning None, so the Mailbox
    // panics with "invalid message encoding".
    let encoded = message::encode(
        99, // wrong version
        0,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        LOCAL_DOMAIN,
        TEST_RECIPIENT_ID.to_bytes(),
        b"hello",
    );

    let result = s.mailbox_process(Vec::new(), encoded);
    assert_contract_panic(result, "Mailbox: invalid message encoding");
}

#[test]
fn test_process_rejects_duplicate_delivery() {
    let mut s = HyperlaneSession::new();

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        LOCAL_DOMAIN,
        TEST_RECIPIENT_ID.to_bytes(),
        b"hello",
    );

    // First delivery succeeds
    s.mailbox_process(Vec::new(), encoded.clone())
        .expect("first process should succeed");

    // Second delivery fails
    let result = s.mailbox_process(Vec::new(), encoded);
    assert_contract_panic(result, "Mailbox: already delivered");
}

#[test]
fn test_process_multiple_messages() {
    let mut s = HyperlaneSession::new();

    for i in 0u32..3 {
        let encoded = message::encode(
            VERSION,
            i,
            REMOTE_DOMAIN,
            [0xABu8; 32],
            LOCAL_DOMAIN,
            TEST_RECIPIENT_ID.to_bytes(),
            format!("message {i}").as_bytes(),
        );

        s.mailbox_process(Vec::new(), encoded)
            .expect("process should succeed");
    }

    assert_eq!(s.recipient_handled_count(), 3);
    assert_eq!(s.mock_verify_count(), 3);
    assert_eq!(s.recipient_last_body(), b"message 2");
}

// =============================================================================
// Tests: Indexer query methods (dispatched_message, processed_at_index)
// =============================================================================

#[test]
fn test_indexer_queries() {
    let mut s = HyperlaneSession::new();

    // Initial state: no dispatched messages, no processed messages
    assert_eq!(s.mailbox_processed_count(), 0);

    // Process three messages with different nonces
    let mut message_ids = Vec::new();
    for i in 0u32..3 {
        let body = format!("message {i}");
        let encoded = message::encode(
            VERSION,
            i,
            REMOTE_DOMAIN,
            [0xABu8; 32],
            LOCAL_DOMAIN,
            TEST_RECIPIENT_ID.to_bytes(),
            body.as_bytes(),
        );
        let id = message::id(&encoded);
        message_ids.push(id);

        s.mailbox_process(Vec::new(), encoded)
            .expect("process should succeed");
    }

    // Verify processed_count
    assert_eq!(s.mailbox_processed_count(), 3);

    // Verify processed_at_index returns the correct message IDs in order
    for i in 0u32..3 {
        assert_eq!(
            s.mailbox_processed_at_index(i),
            message_ids[i as usize],
            "processed_at_index({i}) should match"
        );
    }
}

// =============================================================================
// Tests: Process via Moonlight transaction (simulates relayer)
// =============================================================================

#[test]
fn test_process_via_transaction() {
    let mut s = HyperlaneSession::new();

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        LOCAL_DOMAIN,
        TEST_RECIPIENT_ID.to_bytes(),
        b"relayed message",
    );

    let message_id = message::id(&encoded);

    // Process via Moonlight transaction (simulates relayer submitting tx)
    s.mailbox_process_via_tx(Vec::new(), encoded)
        .expect("process via tx should succeed");

    assert!(s.mailbox_delivered(message_id));
    assert_eq!(s.recipient_last_body(), b"relayed message");
    assert_eq!(s.recipient_handled_count(), 1);
}

// =============================================================================
// Tests: Dispatch via Moonlight transaction
// =============================================================================

#[test]
fn test_dispatch_via_transaction() {
    let mut s = HyperlaneSession::new();

    assert_eq!(s.mailbox_nonce(), 0);
    assert_eq!(
        s.session
            .direct_call::<_, u32>(MAILBOX_ID, "state_version", &())
            .expect("Mailbox state_version should succeed")
            .data,
        2
    );
    assert_eq!(s.merkle_count(), 0);
    assert_eq!(
        s.session
            .direct_call::<_, u32>(MERKLE_TREE_HOOK_ID, "state_version", &())
            .expect("state_version should succeed")
            .data,
        1
    );

    let destination = REMOTE_DOMAIN;
    let recipient = [0xBBu8; 32];
    let body = b"outbound from Dusk".to_vec();

    // Dispatch via direct Moonlight TX (sender = keccak256(OWNER_PK))
    let receipt = s
        .mailbox_dispatch_via_tx(&OWNER_SK, destination, recipient, body.clone())
        .expect("dispatch via tx should succeed");

    let message_id = receipt.data;
    assert_ne!(message_id, [0u8; 32], "Message ID should be non-zero");

    // Nonce and merkle count should increment
    assert_eq!(s.mailbox_nonce(), 1);
    assert_eq!(s.merkle_count(), 1);
    assert_eq!(
        s.session
            .direct_call::<_, H256>(MERKLE_TREE_HOOK_ID, "message_id_at", &(0u32,))
            .expect("message_id_at should succeed")
            .data,
        message_id
    );
    let insertion_height = s
        .session
        .direct_call::<_, u64>(MERKLE_TREE_HOOK_ID, "inserted_block_height", &(0u32,))
        .expect("inserted_block_height should succeed")
        .data;
    assert_eq!(insertion_height, s.session.block_height());
    let root_at: H256 = s
        .session
        .direct_call::<_, H256>(MERKLE_TREE_HOOK_ID, "root_at", &(0u32,))
        .expect("root_at should succeed")
        .data;
    let root: H256 = s
        .session
        .direct_call::<_, H256>(MERKLE_TREE_HOOK_ID, "root", &())
        .expect("root should succeed")
        .data;
    assert_eq!(root_at, root);
    assert_eq!(
        s.session
            .direct_call::<_, Vec<H256>>(MERKLE_TREE_HOOK_ID, "message_ids", &(0u32, 256u32))
            .expect("message_ids should succeed")
            .data,
        vec![message_id]
    );

    // Verify dispatched_message is stored and decodable
    let encoded = s.mailbox_dispatched_message(0);
    let msg = message::decode(&encoded).expect("decode should succeed");
    assert_eq!(msg.version, VERSION);
    assert_eq!(msg.nonce, 0);
    assert_eq!(msg.origin, LOCAL_DOMAIN);
    assert_eq!(msg.destination, destination);
    assert_eq!(msg.recipient, recipient);
    assert_eq!(msg.body, body);

    // Sender should be keccak256(OWNER_PK.to_bytes()) — a 32-byte hash
    // (not the TRANSFER_CONTRACT or a ContractId)
    assert_eq!(msg.sender, message::keccak256(&OWNER_PK.to_bytes()));

    // Verify latest_dispatched_id matches
    assert_eq!(s.mailbox_latest_dispatched_id(), message_id);
}

#[test]
fn test_dispatch_via_recipient_proxy() {
    let mut s = HyperlaneSession::new();

    assert_eq!(s.mailbox_nonce(), 0);

    let destination = REMOTE_DOMAIN;
    let recipient = [0xCCu8; 32];
    let body = b"proxied dispatch".to_vec();

    // Dispatch via TestRecipient proxy (sender = TEST_RECIPIENT_ID)
    let receipt = s
        .mailbox_dispatch_via_recipient(&OWNER_SK, destination, recipient, body.clone())
        .expect("dispatch via recipient proxy should succeed");

    let message_id = receipt.data;
    assert_ne!(message_id, [0u8; 32]);

    assert_eq!(s.mailbox_nonce(), 1);
    assert_eq!(s.merkle_count(), 1);

    // Verify the message sender is TestRecipient's ContractId
    let encoded = s.mailbox_dispatched_message(0);
    let msg = message::decode(&encoded).expect("decode should succeed");
    assert_eq!(msg.sender, TEST_RECIPIENT_ID.to_bytes());
    assert_eq!(msg.body, body);
}

// =============================================================================
// Tests: TestMock (NullISM + NoopHook)
// =============================================================================

#[test]
fn test_mock_verify_always_true() {
    let mut s = HyperlaneSession::new();

    let result: bool = s
        .session
        .direct_call::<_, bool>(
            TEST_MOCK_ID,
            "verify",
            &(Vec::<u8>::new(), Vec::<u8>::new()),
        )
        .expect("verify should succeed")
        .data;

    assert!(result);
    assert_eq!(s.mock_verify_count(), 1);
}

#[test]
fn test_mock_module_type() {
    let mut s = HyperlaneSession::new();

    let module_type: u8 = s
        .session
        .direct_call::<_, u8>(TEST_MOCK_ID, "module_type", &())
        .expect("module_type should succeed")
        .data;

    assert_eq!(module_type, 6); // IsmType::Null
}

#[test]
fn test_mock_quote_dispatch_zero() {
    let mut s = HyperlaneSession::new();

    let quote: u64 = s
        .session
        .direct_call::<_, u64>(
            TEST_MOCK_ID,
            "quote_dispatch",
            &(Vec::<u8>::new(), Vec::<u8>::new()),
        )
        .expect("quote_dispatch should succeed")
        .data;

    assert_eq!(quote, 0);
}

// =============================================================================
// Tests: ValidatorAnnounce bounded query surface
// =============================================================================

fn session_with_validator_announce() -> TestSession {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);
    session
        .deploy(
            VALIDATOR_ANNOUNCE_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(LOCAL_DOMAIN, MAILBOX_ID))
                .contract_id(VALIDATOR_ANNOUNCE_ID),
        )
        .expect("Deploying ValidatorAnnounce should succeed");
    session
}

fn validator_announcement(location: &str) -> (EthAddress, Vec<u8>) {
    let secp = Secp256k1::new();
    let secret =
        SecpSecretKey::from_byte_array([7u8; 32]).expect("validator test secret should be valid");
    let public = secp256k1::PublicKey::from_secret_key(&secp, &secret).serialize_uncompressed();
    let public_hash = message::keccak256(&public[1..]);
    let mut address = [0u8; 20];
    address.copy_from_slice(&public_hash[12..]);

    let digest = hyperlane_dusk_types::checkpoint::announcement_digest(
        LOCAL_DOMAIN,
        &MAILBOX_ID.to_bytes(),
        location,
    );
    let signature = secp.sign_ecdsa_recoverable(SecpMessage::from_digest(digest), &secret);
    let (recovery_id, compact) = signature.serialize_compact();
    let mut encoded = vec![0u8; 65];
    encoded[..64].copy_from_slice(&compact);
    encoded[64] = i32::from(recovery_id) as u8 + 27;
    (EthAddress(address), encoded)
}

#[test]
fn test_validator_announce_rejects_oversized_location_before_storage() {
    let mut session = session_with_validator_announce();
    let result = session.call_public::<_, bool>(
        &OWNER_SK,
        VALIDATOR_ANNOUNCE_ID,
        "announce",
        &(EthAddress([0x11; 20]), "x".repeat(1_025), vec![0u8; 65]),
    );
    assert_contract_panic(result, "ValidatorAnnounce: storage location too long");
}

#[test]
fn test_validator_announce_rejects_unbounded_legacy_batch_query() {
    let mut session = session_with_validator_announce();
    let result = session.direct_call::<_, Vec<Vec<alloc::string::String>>>(
        VALIDATOR_ANNOUNCE_ID,
        "get_announced_storage_locations",
        &(vec![
            EthAddress([1; 20]),
            EthAddress([2; 20]),
            EthAddress([3; 20]),
        ],),
    );
    assert_contract_panic(result, "ValidatorAnnounce: query batch too large");
}

#[test]
fn test_validator_announce_bounds_signed_location_history() {
    let mut session = session_with_validator_announce();
    let mut expected = Vec::new();
    let mut validator = EthAddress([0u8; 20]);

    for index in 0..16 {
        let location = alloc::format!("s3://validator/checkpoints/{index}");
        let (signed_validator, signature) = validator_announcement(&location);
        validator = signed_validator;
        session
            .call_public::<_, bool>(
                &OWNER_SK,
                VALIDATOR_ANNOUNCE_ID,
                "announce",
                &(validator, location.clone(), signature),
            )
            .expect("a valid bounded validator announcement should succeed");
        expected.push(location);
    }

    let locations = session
        .direct_call::<_, Vec<alloc::string::String>>(
            VALIDATOR_ANNOUNCE_ID,
            "get_announced_storage_locations_for_validator",
            &(validator,),
        )
        .expect("the per-validator bounded query should succeed")
        .data;
    assert_eq!(locations, expected);

    let overflow_location = alloc::string::String::from("s3://validator/checkpoints/16");
    let (_, overflow_signature) = validator_announcement(&overflow_location);
    let result = session.call_public::<_, bool>(
        &OWNER_SK,
        VALIDATOR_ANNOUNCE_ID,
        "announce",
        &(validator, overflow_location, overflow_signature),
    );
    assert_contract_panic(result, "ValidatorAnnounce: location limit reached");

    let validators = session
        .direct_call::<_, Vec<EthAddress>>(VALIDATOR_ANNOUNCE_ID, "get_announced_validators", &())
        .expect("validator registry query should succeed")
        .data;
    assert_eq!(validators, vec![validator]);
}

// =============================================================================
// Tests: MessageIdMultisigISM
// =============================================================================

fn session_with_multisig_ism(
    owner: [u8; 32],
    validators: Vec<EthAddress>,
    threshold: u8,
) -> TestSession {
    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, INITIAL_DUSK_BALANCE),
        (&*RELAYER_PK, INITIAL_DUSK_BALANCE),
    ]);

    session
        .deploy(
            ISM_MULTISIG_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(owner, validators, threshold))
                .contract_id(ISM_MULTISIG_ID),
        )
        .expect("Deploying MessageIdMultisigISM should succeed");

    session
}

#[test]
fn test_multisig_ism_state_version() {
    let mut session = session_with_multisig_ism(*OWNER_ID, vec![EthAddress([1; 20])], 1);
    let version = session
        .direct_call::<_, u32>(ISM_MULTISIG_ID, "state_version", &())
        .expect("state_version should succeed")
        .data;
    assert_eq!(version, 1);
}

#[test]
fn test_multisig_ism_init_rejects_invalid_threshold() {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);

    let result = session.deploy(
        ISM_MULTISIG_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(*OWNER_ID, vec![EthAddress([1; 20])], 2u8))
            .contract_id(ISM_MULTISIG_ID),
    );

    assert_deploy_panic(result, "MultisigISM: invalid threshold");
}

#[test]
fn test_multisig_ism_init_rejects_unsorted_validators() {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);

    let result = session.deploy(
        ISM_MULTISIG_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                *OWNER_ID,
                vec![EthAddress([2; 20]), EthAddress([1; 20])],
                1u8,
            ))
            .contract_id(ISM_MULTISIG_ID),
    );

    assert_deploy_panic(result, "MultisigISM: validators not sorted");
}

#[test]
fn test_multisig_ism_init_rejects_no_validators() {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);

    let result = session.deploy(
        ISM_MULTISIG_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(*OWNER_ID, Vec::<EthAddress>::new(), 1u8))
            .contract_id(ISM_MULTISIG_ID),
    );

    assert_deploy_panic(result, "MultisigISM: no validators");
}

#[test]
fn test_multisig_ism_verify_rejects_short_metadata() {
    let mut session = session_with_multisig_ism(*OWNER_ID, vec![EthAddress([1; 20])], 1);

    let result = session.direct_call::<_, bool>(
        ISM_MULTISIG_ID,
        "verify",
        &(vec![0u8; 67], sample_encoded_message(TEST_RECIPIENT_ID)),
    );

    assert_contract_panic(result, "MultisigISM: metadata too short");
}

#[test]
fn test_multisig_ism_verify_rejects_partial_signature_bytes() {
    let mut session = session_with_multisig_ism(*OWNER_ID, vec![EthAddress([1; 20])], 1);

    let result = session.direct_call::<_, bool>(
        ISM_MULTISIG_ID,
        "verify",
        &(vec![0u8; 69], sample_encoded_message(TEST_RECIPIENT_ID)),
    );

    assert_contract_panic(result, "MultisigISM: metadata signature length mismatch");
}

#[test]
fn test_multisig_ism_verify_rejects_insufficient_signatures() {
    let mut session = session_with_multisig_ism(*OWNER_ID, vec![EthAddress([1; 20])], 1);

    let result = session.direct_call::<_, bool>(
        ISM_MULTISIG_ID,
        "verify",
        &(vec![0u8; 68], sample_encoded_message(TEST_RECIPIENT_ID)),
    );

    assert_contract_panic(result, "MultisigISM: not enough signatures");
}

#[test]
fn test_multisig_ism_verify_rejects_corrupt_signature_bytes() {
    let mut session = session_with_multisig_ism(*OWNER_ID, vec![EthAddress([1; 20])], 1);

    let mut metadata = vec![0u8; 68 + 65];
    metadata[132] = 27;

    let result = session.direct_call::<_, bool>(
        ISM_MULTISIG_ID,
        "verify",
        &(metadata, sample_encoded_message(TEST_RECIPIENT_ID)),
    );

    assert_contract_panic(result, "MultisigISM: ecrecover failed");
}

#[test]
fn test_multisig_ism_admin_rejects_unauthorized_caller() {
    let mut session = session_with_multisig_ism(*OWNER_ID, vec![EthAddress([1; 20])], 1);

    let result = session.call_public::<_, ()>(
        &RELAYER_SK,
        ISM_MULTISIG_ID,
        "set_validators_and_threshold",
        &(vec![EthAddress([2; 20])], 1u8),
    );

    assert_contract_panic(result, "MultisigISM: caller is not owner");
}

#[test]
fn test_multisig_ism_admin_accepts_owner_moonlight_sender() {
    let mut session = session_with_multisig_ism(*OWNER_ID, vec![EthAddress([1; 20])], 1);

    session
        .call_public::<_, ()>(
            &OWNER_SK,
            ISM_MULTISIG_ID,
            "set_validators_and_threshold",
            &(vec![EthAddress([2; 20])], 1u8),
        )
        .expect("MultisigISM owner should update the validator set");

    assert_eq!(
        session
            .direct_call::<_, (Vec<EthAddress>, u8)>(
                ISM_MULTISIG_ID,
                "validators_and_threshold",
                &(),
            )
            .expect("combined validator configuration query should succeed")
            .data,
        (vec![EthAddress([2; 20])], 1)
    );
}

// =============================================================================
// Tests: TestRecipient
// =============================================================================

#[test]
fn test_recipient_initial_state() {
    let mut s = HyperlaneSession::new();

    assert_eq!(s.recipient_last_origin(), 0);
    assert_eq!(s.recipient_last_sender(), [0u8; 32]);
    assert_eq!(s.recipient_last_body(), Vec::<u8>::new());
    assert_eq!(s.recipient_handled_count(), 0);
}

#[test]
fn test_recipient_ism_override() {
    let mut s = HyperlaneSession::new();

    // Default: no ISM override (zero)
    let ism: ContractId = s
        .session
        .direct_call::<_, ContractId>(TEST_RECIPIENT_ID, "interchain_security_module", &())
        .expect("query should succeed")
        .data;
    assert_eq!(ism, ContractId::from_bytes([0u8; 32]));

    // Set ISM override
    s.session
        .direct_call::<_, ()>(
            TEST_RECIPIENT_ID,
            "set_interchain_security_module",
            &(TEST_MOCK_ID,),
        )
        .expect("set_ism should succeed");

    let ism: ContractId = s
        .session
        .direct_call::<_, ContractId>(TEST_RECIPIENT_ID, "interchain_security_module", &())
        .expect("query should succeed")
        .data;
    assert_eq!(ism, TEST_MOCK_ID);
}

// =============================================================================
// Tests: ProtocolFee hook
// =============================================================================

/// Session with production hooks: ProtocolFee as required_hook, IGP as default_hook.
fn session_with_hooks() -> TestSession {
    session_with_hooks_fee_and_igp_config(
        1000,
        10000,
        vec![(
            REMOTE_DOMAIN,
            DomainGasConfig {
                gas_overhead: 0,
                token_exchange_rate: 10_000_000_000,
                gas_price: 1,
            },
        )],
    )
}

/// Session with production hooks, including IGP with pre-configured gas configs.
fn session_with_hooks_and_igp_config(igp_configs: Vec<(u32, DomainGasConfig)>) -> TestSession {
    session_with_hooks_fee_and_igp_config(1000, 10000, igp_configs)
}

/// Session with production hooks, configurable protocol fee, and IGP gas configs.
fn session_with_hooks_fee_and_igp_config(
    protocol_fee: u64,
    max_protocol_fee: u64,
    igp_configs: Vec<(u32, DomainGasConfig)>,
) -> TestSession {
    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, INITIAL_DUSK_BALANCE),
        (&*RELAYER_PK, INITIAL_DUSK_BALANCE),
    ]);

    // Deploy TestMock (NullISM)
    session
        .deploy(
            TEST_MOCK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_MOCK_ID),
        )
        .expect("Deploying TestMock should succeed");

    // Deploy TestRecipient
    session
        .deploy(
            TEST_RECIPIENT_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_RECIPIENT_ID),
        )
        .expect("Deploying TestRecipient should succeed");

    // Deploy MerkleTreeHook
    session
        .deploy(
            MERKLE_TREE_HOOK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(AGGREGATION_HOOK_ID,))
                .contract_id(MERKLE_TREE_HOOK_ID),
        )
        .expect("Deploying MerkleTreeHook should succeed");

    // Deploy ProtocolFee
    session
        .deploy(
            PROTOCOL_FEE_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    protocol_fee,
                    max_protocol_fee,
                    AGGREGATION_HOOK_ID,
                    *OWNER_ID,
                    *OWNER_ID,
                ))
                .contract_id(PROTOCOL_FEE_ID),
        )
        .expect("Deploying ProtocolFee should succeed");

    // Deploy the required aggregation of MerkleTreeHook + ProtocolFee.
    session
        .deploy(
            AGGREGATION_HOOK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID, vec![MERKLE_TREE_HOOK_ID, PROTOCOL_FEE_ID]))
                .contract_id(AGGREGATION_HOOK_ID),
        )
        .expect("Deploying AggregationHook should succeed");

    // Deploy IGP with provided gas configs
    session
        .deploy(
            IGP_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID, *OWNER_ID, *OWNER_ID, igp_configs))
                .contract_id(IGP_ID),
        )
        .expect("Deploying IGP should succeed");

    // Deploy Mailbox with the aggregation as required_hook and IGP as default_hook.
    session
        .deploy(
            MAILBOX_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    LOCAL_DOMAIN,
                    *OWNER_ID,           // owner
                    TEST_MOCK_ID,        // default ISM
                    IGP_ID,              // default hook = IGP
                    AGGREGATION_HOOK_ID, // required = MerkleTreeHook + ProtocolFee
                ))
                .contract_id(MAILBOX_ID),
        )
        .expect("Deploying Mailbox should succeed");

    const PREPAID_FEES: u64 = 100_000_000;
    session
        .call_public_with_deposit::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(TEST_RECIPIENT_ID.to_bytes(), PREPAID_FEES),
            PREPAID_FEES,
        )
        .expect("funding the TestRecipient dispatch credit should succeed");

    session
}

#[test]
fn test_protocol_fee_init() {
    let mut s = HyperlaneSession::new();

    // Deploy ProtocolFee standalone
    s.session
        .deploy(
            PROTOCOL_FEE_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(500u64, 5000u64, MAILBOX_ID, *OWNER_ID, *OWNER_ID))
                .contract_id(PROTOCOL_FEE_ID),
        )
        .expect("Deploying ProtocolFee should succeed");

    // Verify state
    let quote: u64 = s
        .session
        .direct_call::<_, u64>(
            PROTOCOL_FEE_ID,
            "quote_dispatch",
            &(Vec::<u8>::new(), Vec::<u8>::new()),
        )
        .expect("quote_dispatch should succeed")
        .data;
    assert_eq!(quote, 500);

    let collected: u64 = s
        .session
        .direct_call::<_, u64>(PROTOCOL_FEE_ID, "collected_fees", &())
        .expect("collected_fees should succeed")
        .data;
    assert_eq!(collected, 0);

    let hook_type: u8 = s
        .session
        .direct_call::<_, u8>(PROTOCOL_FEE_ID, "hook_type", &())
        .expect("hook_type should succeed")
        .data;
    assert_eq!(hook_type, 6); // HookType::ProtocolFee

    let version = s
        .session
        .direct_call::<_, u32>(PROTOCOL_FEE_ID, "state_version", &())
        .expect("state_version should succeed")
        .data;
    assert_eq!(version, 1);
}

#[test]
fn test_protocol_fee_charges_on_dispatch() {
    let mut session = session_with_hooks();

    // Check initial state
    let collected: u64 = session
        .direct_call::<_, u64>(PROTOCOL_FEE_ID, "collected_fees", &())
        .expect("collected_fees should succeed")
        .data;
    assert_eq!(collected, 0);
    assert_eq!(
        session
            .contract_balance(&PROTOCOL_FEE_ID)
            .expect("ProtocolFee balance query should succeed"),
        0
    );

    // Dispatch a message via TestRecipient proxy
    session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            TEST_RECIPIENT_ID,
            "dispatch_message",
            &(
                MAILBOX_ID,
                REMOTE_DOMAIN,
                [0xBBu8; 32],
                b"testing protocol fee".to_vec(),
            ),
        )
        .expect("dispatch should succeed");

    // ProtocolFee should have recorded 1000 LUX
    let collected: u64 = session
        .direct_call::<_, u64>(PROTOCOL_FEE_ID, "collected_fees", &())
        .expect("collected_fees should succeed")
        .data;
    assert_eq!(collected, 1000);
    let claimable = session
        .direct_call::<_, u64>(PROTOCOL_FEE_ID, "claimable_fees", &())
        .expect("claimable_fees should succeed")
        .data;
    assert_eq!(claimable, 1000);
    assert_eq!(
        session
            .contract_balance(&PROTOCOL_FEE_ID)
            .expect("ProtocolFee balance query should succeed"),
        1000
    );
    let merkle_count: u32 = session
        .direct_call::<_, u32>(MERKLE_TREE_HOOK_ID, "count", &())
        .expect("aggregation should invoke the MerkleTreeHook")
        .data;
    assert_eq!(merkle_count, 1);

    session
        .call_public::<_, ()>(&OWNER_SK, PROTOCOL_FEE_ID, "claim", &())
        .expect("beneficiary should claim the collected native DUSK");
    assert_eq!(
        session
            .contract_balance(&PROTOCOL_FEE_ID)
            .expect("ProtocolFee balance query should succeed"),
        0
    );
    let claimable = session
        .direct_call::<_, u64>(PROTOCOL_FEE_ID, "claimable_fees", &())
        .expect("claimable_fees should succeed")
        .data;
    assert_eq!(claimable, 0);
}

#[test]
fn test_aggregation_hook_wiring_and_callback_authentication() {
    let mut session = session_with_hooks();

    let hook_type = session
        .direct_call::<_, u8>(AGGREGATION_HOOK_ID, "hook_type", &())
        .expect("hook_type should succeed")
        .data;
    assert_eq!(hook_type, 2);
    let version = session
        .direct_call::<_, u32>(AGGREGATION_HOOK_ID, "state_version", &())
        .expect("state_version should succeed")
        .data;
    assert_eq!(version, 1);
    let hooks = session
        .direct_call::<_, Vec<ContractId>>(AGGREGATION_HOOK_ID, "hooks", &())
        .expect("hooks should succeed")
        .data;
    assert_eq!(hooks, vec![MERKLE_TREE_HOOK_ID, PROTOCOL_FEE_ID]);

    let result = session.call_public::<_, ()>(
        &OWNER_SK,
        AGGREGATION_HOOK_ID,
        "receive_payment",
        &(ReceiveFromContract {
            value: 1_000,
            contract: MAILBOX_ID,
            data: Vec::new(),
        },),
    );
    assert_contract_panic(result, "AggregationHook: unauthenticated payment");
}

#[test]
fn test_dispatch_rejects_sender_without_native_fee_credit() {
    let mut session = session_with_hooks();

    let result = session.call_public::<_, MessageId>(
        &OWNER_SK,
        MAILBOX_ID,
        "dispatch_default",
        &(
            REMOTE_DOMAIN,
            [0xBBu8; 32],
            b"unfunded direct dispatch".to_vec(),
        ),
    );
    assert_contract_panic(result, "Mailbox: insufficient fee credit");
    assert_eq!(
        session
            .contract_balance(&PROTOCOL_FEE_ID)
            .expect("ProtocolFee balance query should succeed"),
        0
    );
}

#[test]
fn test_dispatch_credit_withdrawal_is_payer_owned_and_value_backed() {
    let mut s = HyperlaneSession::new();
    let funded = 4_000_000u64;
    let partial = 1_500_000u64;

    // Funding is intentionally permissionless. Funding another payer does not
    // grant the funder authority over that payer's resulting credit.
    s.session
        .call_public_with_deposit::<_, ()>(
            &RELAYER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(*OWNER_ID, funded),
            funded,
        )
        .expect("third-party dispatch funding should succeed");

    assert_eq!(
        s.session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(*OWNER_ID,))
            .expect("fee_credit should succeed")
            .data,
        funded
    );
    assert_eq!(
        s.session
            .contract_balance(&MAILBOX_ID)
            .expect("Mailbox balance query should succeed"),
        funded
    );

    let relayer_nonce_before = s
        .session
        .account(&RELAYER_PK)
        .expect("relayer account query should succeed")
        .nonce;
    let result = s.session.call_public::<_, ()>(
        &RELAYER_SK,
        MAILBOX_ID,
        "withdraw_dispatch_credit",
        &(*RELAYER_PK, partial),
    );
    assert_contract_panic(result, "Mailbox: insufficient fee credit");
    let relayer_nonce_after = s
        .session
        .account(&RELAYER_PK)
        .expect("relayer account query should succeed")
        .nonce;
    assert_eq!(
        relayer_nonce_after,
        relayer_nonce_before + 1,
        "a rejected contract call still spends its Moonlight nonce"
    );

    let result = s.session.call_public::<_, ()>(
        &OWNER_SK,
        MAILBOX_ID,
        "withdraw_dispatch_credit",
        &(*RELAYER_PK, 0u64),
    );
    assert_contract_panic(result, "Mailbox: withdrawal amount is zero");

    let mut identity_bytes = [0u8; 96];
    identity_bytes[0] = 0xc0;
    let invalid_recipient = AccountPublicKey::from_bytes(&identity_bytes)
        .expect("compressed identity should decode for semantic validation");
    assert!(!invalid_recipient.is_valid());
    let result = s.session.call_public::<_, ()>(
        &OWNER_SK,
        MAILBOX_ID,
        "withdraw_dispatch_credit",
        &(invalid_recipient, partial),
    );
    assert_contract_panic(result, "Mailbox: invalid withdrawal recipient");
    assert_eq!(
        s.session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(*OWNER_ID,))
            .expect("fee_credit should succeed")
            .data,
        funded,
        "invalid recipient must not debit dispatch credit"
    );
    assert_eq!(
        s.session
            .contract_balance(&MAILBOX_ID)
            .expect("Mailbox balance query should succeed"),
        funded,
        "invalid recipient must not change Mailbox custody"
    );

    let relayer_balance_before = s
        .session
        .account(&RELAYER_PK)
        .expect("relayer account query should succeed")
        .balance;
    let receipt = s
        .session
        .call_public::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "withdraw_dispatch_credit",
            &(*RELAYER_PK, partial),
        )
        .expect("payer should withdraw its own dispatch credit");
    assert!(
        receipt.gas_spent < WITHDRAW_DEFAULT_GAS_LIMIT,
        "direct withdrawal spent {} gas, exceeding the CLI default {}",
        receipt.gas_spent,
        WITHDRAW_DEFAULT_GAS_LIMIT
    );
    let event = receipt
        .events
        .iter()
        .find(|event| {
            event.source == MAILBOX_ID && event.topic == events::DispatchFeeWithdrawn::TOPIC
        })
        .expect("withdrawal receipt should contain the Mailbox event");
    let decoded = HyperlaneDataDriver
        .decode_event(&event.topic, &event.data)
        .expect("the explorer driver should decode the real VM receipt event");
    assert_eq!(
        decoded["payer"],
        dusk_data_driver::to_json(*OWNER_ID).unwrap()
    );
    assert_eq!(
        decoded["recipient"],
        dusk_data_driver::to_json(message::keccak256(&RELAYER_PK.to_bytes())).unwrap()
    );
    assert_eq!(decoded["amount"].as_u64(), Some(partial));
    let relayer_balance_after = s
        .session
        .account(&RELAYER_PK)
        .expect("relayer account query should succeed")
        .balance;
    assert_eq!(relayer_balance_after, relayer_balance_before + partial);
    assert_eq!(
        s.session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(*OWNER_ID,))
            .expect("fee_credit should succeed")
            .data,
        funded - partial
    );
    assert_eq!(
        s.session
            .contract_balance(&MAILBOX_ID)
            .expect("Mailbox balance query should succeed"),
        funded - partial
    );

    s.session
        .call_public::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "withdraw_dispatch_credit",
            &(*OWNER_PK, funded - partial),
        )
        .expect("payer should withdraw the remaining dispatch credit");
    assert_eq!(
        s.session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(*OWNER_ID,))
            .expect("fee_credit should succeed")
            .data,
        0
    );
    assert_eq!(
        s.session
            .contract_balance(&MAILBOX_ID)
            .expect("Mailbox balance query should succeed"),
        0
    );
}

#[test]
fn test_dispatch_credit_withdrawal_rolls_back_after_transfer_failure() {
    let mut s = HyperlaneSession::new();
    let funded = 2_000_000u64;

    s.session
        .call_public_with_deposit::<_, ()>(
            &RELAYER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(*OWNER_ID, funded),
            funded,
        )
        .expect("third-party dispatch funding should succeed");

    // Saturate the recipient balance so transfer-contract account crediting
    // fails after Mailbox has tentatively debited the payer's fee credit.
    s.session
        .direct_call::<_, ()>(
            dusk_core::transfer::TRANSFER_CONTRACT,
            "add_account_balance",
            &(*RELAYER_PK, u64::MAX),
        )
        .expect("test setup should saturate recipient balance");
    let recipient_balance = s
        .session
        .account(&RELAYER_PK)
        .expect("recipient account query should succeed")
        .balance;
    assert_eq!(recipient_balance, u64::MAX);

    let result = s.session.call_public::<_, ()>(
        &OWNER_SK,
        MAILBOX_ID,
        "withdraw_dispatch_credit",
        &(*RELAYER_PK, funded),
    );
    assert_contract_panic_contains(result, "attempt to add with overflow");

    assert_eq!(
        s.session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(*OWNER_ID,))
            .expect("fee_credit should succeed")
            .data,
        funded,
        "failed transfer must roll back the tentative credit debit"
    );
    assert_eq!(
        s.session
            .contract_balance(&MAILBOX_ID)
            .expect("Mailbox balance query should succeed"),
        funded,
        "failed transfer must preserve Mailbox custody"
    );
    assert_eq!(
        s.session
            .account(&RELAYER_PK)
            .expect("recipient account query should succeed")
            .balance,
        u64::MAX,
        "failed transfer must preserve recipient balance"
    );

    // A later caller-sensitive operation must still resolve the Moonlight
    // caller correctly after the nested transfer panic rolled back.
    s.session
        .call_public_with_deposit::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(*OWNER_ID, 1u64),
            1,
        )
        .expect("caller context should be restored after rollback");
    assert_eq!(
        s.session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(*OWNER_ID,))
            .expect("fee_credit should succeed")
            .data,
        funded + 1
    );
}

#[test]
fn test_dispatch_credit_withdrawals_preserve_multi_payer_solvency() {
    let mut s = HyperlaneSession::new();
    let owner_credit = 4_000_000u64;
    let relayer_credit = 6_000_000u64;
    let owner_withdrawal = 1_500_000u64;

    s.session
        .call_public_with_deposit::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(*OWNER_ID, owner_credit),
            owner_credit,
        )
        .expect("owner dispatch funding should succeed");
    s.session
        .call_public_with_deposit::<_, ()>(
            &RELAYER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(*RELAYER_ID, relayer_credit),
            relayer_credit,
        )
        .expect("relayer dispatch funding should succeed");

    assert_eq!(
        s.session
            .contract_balance(&MAILBOX_ID)
            .expect("Mailbox balance query should succeed"),
        owner_credit + relayer_credit
    );

    s.session
        .call_public::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "withdraw_dispatch_credit",
            &(*OWNER_PK, owner_withdrawal),
        )
        .expect("owner withdrawal should succeed");

    assert_eq!(
        s.session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(*OWNER_ID,))
            .expect("owner fee_credit should succeed")
            .data,
        owner_credit - owner_withdrawal
    );
    assert_eq!(
        s.session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(*RELAYER_ID,))
            .expect("relayer fee_credit should succeed")
            .data,
        relayer_credit,
        "withdrawing one payer must not change another payer's liability"
    );
    assert_eq!(
        s.session
            .contract_balance(&MAILBOX_ID)
            .expect("Mailbox balance query should succeed"),
        owner_credit + relayer_credit - owner_withdrawal
    );

    s.session
        .call_public::<_, ()>(
            &RELAYER_SK,
            MAILBOX_ID,
            "withdraw_dispatch_credit",
            &(*RELAYER_PK, relayer_credit),
        )
        .expect("relayer withdrawal should succeed");
    assert_eq!(
        s.session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(*RELAYER_ID,))
            .expect("relayer fee_credit should succeed")
            .data,
        0
    );
    assert_eq!(
        s.session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(*OWNER_ID,))
            .expect("owner fee_credit should succeed")
            .data,
        owner_credit - owner_withdrawal
    );
    assert_eq!(
        s.session
            .contract_balance(&MAILBOX_ID)
            .expect("Mailbox balance query should succeed"),
        owner_credit - owner_withdrawal,
        "Mailbox custody must equal the sum of remaining payer liabilities"
    );
}

#[test]
fn test_protocol_fee_rejects_unbacked_direct_post_dispatch() {
    let mut s = HyperlaneSession::new();

    s.session
        .deploy(
            PROTOCOL_FEE_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(500u64, 5000u64, MAILBOX_ID, *OWNER_ID, *OWNER_ID))
                .contract_id(PROTOCOL_FEE_ID),
        )
        .expect("Deploying ProtocolFee should succeed");

    let encoded = message::encode(
        VERSION,
        0,
        LOCAL_DOMAIN,
        [0xAAu8; 32],
        REMOTE_DOMAIN,
        [0xBBu8; 32],
        b"unbacked protocol fee",
    );

    let result = s.session.call_public::<_, ()>(
        &OWNER_SK,
        PROTOCOL_FEE_ID,
        "post_dispatch",
        &(Vec::<u8>::new(), encoded),
    );

    assert_contract_panic(result, "ProtocolFee: caller is not mailbox");
    let collected = s
        .session
        .direct_call::<_, u64>(PROTOCOL_FEE_ID, "collected_fees", &())
        .expect("collected_fees should succeed")
        .data;
    assert_eq!(collected, 0);

    let result = s.session.call_public::<_, ()>(
        &OWNER_SK,
        PROTOCOL_FEE_ID,
        "receive_payment",
        &(ReceiveFromContract {
            contract: MAILBOX_ID,
            value: 500,
            data: Vec::new(),
        },),
    );
    assert_contract_panic(result, "ProtocolFee: unauthenticated payment");
}

#[test]
fn test_protocol_fee_init_rejects_fee_above_max() {
    let mut s = HyperlaneSession::new();

    // Try to deploy with fee > max — should panic during init
    let result = s.session.deploy(
        PROTOCOL_FEE_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(20000u64, 10000u64, MAILBOX_ID, *OWNER_ID, *OWNER_ID))
            .contract_id(PROTOCOL_FEE_ID),
    );
    assert!(result.is_err(), "init with fee > max should fail");
}

#[test]
fn test_protocol_fee_different_fee_values() {
    let mut s = HyperlaneSession::new();

    // Deploy with fee=2500
    s.session
        .deploy(
            PROTOCOL_FEE_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(2500u64, 5000u64, MAILBOX_ID, *OWNER_ID, *OWNER_ID))
                .contract_id(PROTOCOL_FEE_ID),
        )
        .expect("Deploying ProtocolFee should succeed");

    let quote: u64 = s
        .session
        .direct_call::<_, u64>(
            PROTOCOL_FEE_ID,
            "quote_dispatch",
            &(Vec::<u8>::new(), Vec::<u8>::new()),
        )
        .expect("quote_dispatch should succeed")
        .data;
    assert_eq!(quote, 2500);

    let max: u64 = s
        .session
        .direct_call::<_, u64>(PROTOCOL_FEE_ID, "max_protocol_fee", &())
        .expect("max_protocol_fee should succeed")
        .data;
    assert_eq!(max, 5000);
}

// =============================================================================
// Tests: IGP hook
// =============================================================================

#[test]
fn test_igp_init() {
    let mut session = session_with_hooks();

    let hook_type: u8 = session
        .direct_call::<_, u8>(IGP_ID, "hook_type", &())
        .expect("hook_type should succeed")
        .data;
    assert_eq!(hook_type, 4); // HookType::Igp

    let version = session
        .direct_call::<_, u32>(IGP_ID, "state_version", &())
        .expect("state_version should succeed")
        .data;
    assert_eq!(version, 2);

    let total: u64 = session
        .direct_call::<_, u64>(IGP_ID, "total_gas_payments", &())
        .expect("total_gas_payments should succeed")
        .data;
    assert_eq!(total, 0);
}

#[test]
fn test_fee_contract_admin_paths_accept_owner_and_reject_non_owner() {
    let mut session = session_with_hooks();

    session
        .call_public::<_, ()>(&OWNER_SK, PROTOCOL_FEE_ID, "set_protocol_fee", &(750u64,))
        .expect("ProtocolFee owner should update the fee");
    let result =
        session.call_public::<_, ()>(&RELAYER_SK, PROTOCOL_FEE_ID, "set_protocol_fee", &(500u64,));
    assert_contract_panic(result, "ProtocolFee: caller is not the owner");

    let config = DomainGasConfig {
        gas_overhead: 1,
        token_exchange_rate: 10_000_000_000,
        gas_price: 2,
    };
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            IGP_ID,
            "set_domain_gas_config",
            &(REMOTE_DOMAIN, config),
        )
        .expect("IGP owner should update a domain config");
    let result = session.call_public::<_, ()>(
        &RELAYER_SK,
        IGP_ID,
        "set_domain_gas_config",
        &(REMOTE_DOMAIN, config),
    );
    assert_contract_panic(result, "IGP: caller is not the owner");
}

#[test]
fn test_igp_quote_no_config() {
    let mut session = session_with_hooks();

    // An active IGP must never silently price an unknown destination at zero.
    let metadata = 100_000u64.to_le_bytes().to_vec();
    let encoded = message::encode(VERSION, 0, LOCAL_DOMAIN, [0u8; 32], 42, [0u8; 32], &[]);
    let result = session.direct_call::<_, u64>(IGP_ID, "quote_dispatch", &(metadata, encoded));
    assert_contract_panic(result, "IGP: destination is not configured");
}

#[test]
fn test_igp_rejects_zero_pricing_inputs() {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);
    let result = session.deploy(
        IGP_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                MAILBOX_ID,
                *OWNER_ID,
                *OWNER_ID,
                vec![(
                    REMOTE_DOMAIN,
                    DomainGasConfig {
                        gas_overhead: 50_000,
                        token_exchange_rate: 0,
                        gas_price: 1,
                    },
                )],
            ))
            .contract_id(IGP_ID),
    );
    assert_deploy_panic(result, "IGP: token exchange rate cannot be zero");
}

#[test]
fn test_igp_rejects_pricing_outside_executable_quote_domain() {
    for (config, expected) in [
        (
            DomainGasConfig {
                gas_overhead: 0,
                token_exchange_rate: 1,
                gas_price: 1,
            },
            "IGP: configured payment rounds to zero",
        ),
        (
            DomainGasConfig {
                gas_overhead: 0,
                token_exchange_rate: 10_000_000_000,
                gas_price: u64::MAX,
            },
            "IGP: configured quote exceeds u64",
        ),
        (
            DomainGasConfig {
                gas_overhead: u64::MAX,
                token_exchange_rate: u64::MAX,
                gas_price: u64::MAX,
            },
            "IGP: configured quote arithmetic overflows",
        ),
    ] {
        let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);
        let result = session.deploy(
            IGP_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    MAILBOX_ID,
                    *OWNER_ID,
                    *OWNER_ID,
                    vec![(REMOTE_DOMAIN, config)],
                ))
                .contract_id(IGP_ID),
        );
        assert_deploy_panic(result, expected);
    }
}

#[test]
fn test_mailbox_rejects_required_hook_as_selected_hook() {
    let mut session = session_with_hooks();
    let result = session.call_public::<_, MessageId>(
        &OWNER_SK,
        MAILBOX_ID,
        "dispatch",
        &(
            REMOTE_DOMAIN,
            [0xBBu8; 32],
            b"duplicate required hook".to_vec(),
            100_000u64.to_le_bytes().to_vec(),
            AGGREGATION_HOOK_ID,
        ),
    );
    assert_contract_panic(result, "Mailbox: selected hook cannot equal required hook");
    assert_eq!(
        session
            .direct_call::<_, u32>(MERKLE_TREE_HOOK_ID, "count", &())
            .expect("count should succeed")
            .data,
        0
    );
}

#[test]
fn test_igp_quote_with_config() {
    // Deploy IGP with initial gas config for REMOTE_DOMAIN
    let mut session = session_with_hooks_and_igp_config(vec![(
        REMOTE_DOMAIN,
        DomainGasConfig {
            gas_overhead: 50_000,
            token_exchange_rate: 10_000_000_000u64, // 1:1
            gas_price: 20u64,                       // 20 wei
        },
    )]);

    // gas_limit=100_000, overhead=50_000 → adjusted=150_000
    // cost = 150_000 * 20 * 10_000_000_000 / 10_000_000_000
    //      = 150_000 * 20 = 3_000_000
    let metadata = 100_000u64.to_le_bytes().to_vec();
    let encoded = message::encode(
        VERSION,
        0,
        LOCAL_DOMAIN,
        [0u8; 32],
        REMOTE_DOMAIN,
        [0u8; 32],
        &[],
    );
    let quote: u64 = session
        .direct_call::<_, u64>(IGP_ID, "quote_dispatch", &(metadata, encoded))
        .expect("quote_dispatch should succeed")
        .data;

    assert_eq!(quote, 3_000_000);
}

#[test]
fn test_mailbox_quote_dispatch_rejects_fee_overflow() {
    let mut session = session_with_hooks_fee_and_igp_config(
        u64::MAX,
        u64::MAX,
        vec![(
            REMOTE_DOMAIN,
            DomainGasConfig {
                gas_overhead: 0,
                token_exchange_rate: 10_000_000_000u64,
                gas_price: 1u64,
            },
        )],
    );

    let metadata = 1u64.to_le_bytes().to_vec();
    let result = session.call_public::<_, u64>(
        &OWNER_SK,
        MAILBOX_ID,
        "quote_dispatch",
        &(
            REMOTE_DOMAIN,
            [0xBBu8; 32],
            b"fee overflow".to_vec(),
            metadata,
            IGP_ID,
        ),
    );

    assert_contract_panic(result, "Mailbox: fee overflow");
}

#[test]
fn test_igp_records_payment_on_dispatch() {
    // Deploy IGP with simple gas config: 1 wei gas price, 1:1 exchange, no overhead
    let mut session = session_with_hooks_and_igp_config(vec![(
        REMOTE_DOMAIN,
        DomainGasConfig {
            gas_overhead: 0,
            token_exchange_rate: 10_000_000_000u64, // 1:1
            gas_price: 1u64,                        // 1 wei
        },
    )]);

    // Dispatch a message (default hook = IGP)
    session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            TEST_RECIPIENT_ID,
            "dispatch_message",
            &(
                MAILBOX_ID,
                REMOTE_DOMAIN,
                [0xBBu8; 32],
                b"testing igp".to_vec(),
            ),
        )
        .expect("dispatch should succeed");

    // With empty metadata (from dispatch_default), gas_limit defaults to 50_000
    // cost = 50_000 * 1 * 10_000_000_000 / 10_000_000_000 = 50_000
    let total: u64 = session
        .direct_call::<_, u64>(IGP_ID, "total_gas_payments", &())
        .expect("total_gas_payments should succeed")
        .data;
    assert_eq!(total, 50_000);
    assert_eq!(
        session
            .contract_balance(&IGP_ID)
            .expect("IGP balance query should succeed"),
        50_000
    );
    let records = session
        .direct_call::<_, Vec<GasPaymentRecord>>(IGP_ID, "gas_payments", &(0u32, 256u32))
        .expect("gas_payments should succeed")
        .data;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].payment, 50_000);
}

#[test]
fn test_igp_rejects_explicit_zero_gas_limit() {
    let mut session = session_with_hooks_and_igp_config(vec![(
        REMOTE_DOMAIN,
        DomainGasConfig {
            gas_overhead: 50_000,
            token_exchange_rate: 10_000_000_000,
            gas_price: 1,
        },
    )]);
    let encoded = message::encode(
        VERSION,
        0,
        LOCAL_DOMAIN,
        [0xAA; 32],
        REMOTE_DOMAIN,
        [0xBB; 32],
        b"zero gas",
    );
    let result = session.direct_call::<_, u64>(
        IGP_ID,
        "quote_dispatch",
        &(0u64.to_le_bytes().to_vec(), encoded),
    );
    assert_contract_panic(result, "IGP: gas limit cannot be zero");
}

#[test]
fn test_igp_rejects_gas_limit_above_supported_domain() {
    let mut session = session_with_hooks_and_igp_config(vec![(
        REMOTE_DOMAIN,
        DomainGasConfig {
            gas_overhead: 50_000,
            token_exchange_rate: 10_000_000_000,
            gas_price: 1,
        },
    )]);
    let encoded = message::encode(
        VERSION,
        0,
        LOCAL_DOMAIN,
        [0xAA; 32],
        REMOTE_DOMAIN,
        [0xBB; 32],
        b"oversized gas",
    );
    let result = session.direct_call::<_, u64>(
        IGP_ID,
        "quote_dispatch",
        &(1_000_000_001u64.to_le_bytes().to_vec(), encoded),
    );
    assert_contract_panic(result, "IGP: gas limit exceeds supported maximum");
}

#[test]
fn test_igp_rejects_unbacked_direct_post_dispatch() {
    let mut s = HyperlaneSession::new();

    s.session
        .deploy(
            IGP_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    MAILBOX_ID,
                    *OWNER_ID,
                    *OWNER_ID,
                    vec![(
                        REMOTE_DOMAIN,
                        DomainGasConfig {
                            gas_overhead: 0,
                            token_exchange_rate: 10_000_000_000u64,
                            gas_price: 1u64,
                        },
                    )],
                ))
                .contract_id(IGP_ID),
        )
        .expect("Deploying IGP should succeed");

    let encoded = message::encode(
        VERSION,
        0,
        LOCAL_DOMAIN,
        [0xAAu8; 32],
        REMOTE_DOMAIN,
        [0xBBu8; 32],
        b"unbacked igp payment",
    );

    let result = s.session.call_public::<_, ()>(
        &OWNER_SK,
        IGP_ID,
        "post_dispatch",
        &(1u64.to_le_bytes().to_vec(), encoded),
    );

    assert_contract_panic(result, "IGP: caller is not mailbox");
    let total = s
        .session
        .direct_call::<_, u64>(IGP_ID, "total_gas_payments", &())
        .expect("total_gas_payments should succeed")
        .data;
    assert_eq!(total, 0);

    let result = s.session.call_public::<_, ()>(
        &OWNER_SK,
        IGP_ID,
        "receive_payment",
        &(ReceiveFromContract {
            contract: MAILBOX_ID,
            value: 1,
            data: Vec::new(),
        },),
    );
    assert_contract_panic(result, "IGP: unauthenticated payment");
}

// =============================================================================
// Tests: WarpDrc20 (Synthetic token)
// =============================================================================

const WARP_DRC20_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_warp_drc20.wasm"
);
const WARP_DRC20_COLLATERAL_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_warp_drc20_collateral.wasm"
);
const WARP_NATIVE_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_warp_native.wasm"
);

const WARP_DRC20_ID: ContractId = ContractId::from_bytes([20; 32]);
const WARP_DRC20_COLLATERAL_ID: ContractId = ContractId::from_bytes([21; 32]);
const WARP_NATIVE_ID: ContractId = ContractId::from_bytes([22; 32]);

fn warp_drc20_balance_of(session: &mut TestSession, account: Drc20Account) -> u64 {
    session
        .direct_call::<_, u64>(WARP_DRC20_ID, "balance_of", &Drc20BalanceOf { account })
        .expect("balance_of should succeed")
        .data
}

fn assert_route_dispatch_credit_withdrawal(
    mut session: TestSession,
    route: ContractId,
    owner_error: &str,
) {
    let funded = 3_000_000u64;
    let withdrawn = 1_000_000u64;
    let payer = route.to_bytes();

    session
        .call_public_with_deposit::<_, ()>(
            &RELAYER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(payer, funded),
            funded,
        )
        .expect("third-party route funding should succeed");

    let result = session.call_public::<_, ()>(
        &RELAYER_SK,
        route,
        "withdraw_dispatch_credit",
        &(*RELAYER_PK, withdrawn),
    );
    assert_contract_panic(result, owner_error);
    assert_eq!(
        session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(payer,))
            .expect("route fee_credit should succeed")
            .data,
        funded
    );

    let recipient_balance_before = session
        .account(&RELAYER_PK)
        .expect("recipient account query should succeed")
        .balance;
    let receipt = session
        .call_public::<_, ()>(
            &OWNER_SK,
            route,
            "withdraw_dispatch_credit",
            &(*RELAYER_PK, withdrawn),
        )
        .expect("route owner should withdraw the route's dispatch credit");
    assert!(
        receipt.gas_spent < WITHDRAW_DEFAULT_GAS_LIMIT,
        "proxied withdrawal spent {} gas, exceeding the CLI default {}",
        receipt.gas_spent,
        WITHDRAW_DEFAULT_GAS_LIMIT
    );
    let recipient_balance_after = session
        .account(&RELAYER_PK)
        .expect("recipient account query should succeed")
        .balance;

    assert_eq!(
        recipient_balance_after,
        recipient_balance_before + withdrawn
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(payer,))
            .expect("route fee_credit should succeed")
            .data,
        funded - withdrawn
    );
    assert_eq!(
        session
            .contract_balance(&MAILBOX_ID)
            .expect("Mailbox balance query should succeed"),
        funded - withdrawn
    );

    let recipient_balance_before_rejection = session
        .account(&RELAYER_PK)
        .expect("recipient account query should succeed")
        .balance;
    let result = session.call_public::<_, ()>(
        &OWNER_SK,
        route,
        "withdraw_dispatch_credit",
        &(*RELAYER_PK, funded - withdrawn + 1),
    );
    assert_contract_panic_contains(result, "Mailbox: insufficient fee credit");
    assert_eq!(
        session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(payer,))
            .expect("route fee_credit should succeed")
            .data,
        funded - withdrawn,
        "downstream Mailbox rejection must preserve route credit"
    );
    assert_eq!(
        session
            .account(&RELAYER_PK)
            .expect("recipient account query should succeed")
            .balance,
        recipient_balance_before_rejection,
        "downstream Mailbox rejection must not pay the recipient"
    );
}

fn canonical_drc20_balance_of(session: &mut TestSession, account: Drc20Account) -> u64 {
    session
        .direct_call::<_, u64>(WARP_DRC20_ID, "balance_of", &Drc20BalanceOf { account })
        .expect("canonical DRC20 balance_of should succeed")
        .data
}

/// Deploy a WarpDrc20 alongside Mailbox with TestMock hooks.
fn session_with_warp_drc20() -> TestSession {
    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, INITIAL_DUSK_BALANCE),
        (&*RELAYER_PK, INITIAL_DUSK_BALANCE),
    ]);

    // Deploy TestMock (NullISM + NoopHook)
    session
        .deploy(
            TEST_MOCK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_MOCK_ID),
        )
        .expect("Deploying TestMock should succeed");

    // Deploy MerkleTreeHook
    session
        .deploy(
            MERKLE_TREE_HOOK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID,))
                .contract_id(MERKLE_TREE_HOOK_ID),
        )
        .expect("Deploying MerkleTreeHook should succeed");

    // Deploy WarpDrc20
    session
        .deploy(
            WARP_DRC20_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    MAILBOX_ID,
                    *OWNER_ID,
                    alloc::string::String::from("Wrapped ETH"),
                    alloc::string::String::from("WETH"),
                    18u8,
                    Vec::<(u32, H256)>::new(), // no initial routers
                ))
                .contract_id(WARP_DRC20_ID),
        )
        .expect("Deploying WarpDrc20 should succeed");

    // Deploy Mailbox with TestMock as ISM and hooks
    session
        .deploy(
            MAILBOX_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    LOCAL_DOMAIN,
                    *OWNER_ID,           // owner
                    TEST_MOCK_ID,        // default ISM
                    TEST_MOCK_ID,        // default hook (noop)
                    MERKLE_TREE_HOOK_ID, // required hook
                ))
                .contract_id(MAILBOX_ID),
        )
        .expect("Deploying Mailbox should succeed");

    session
}

#[test]
fn test_warp_drc20_init() {
    let mut session = session_with_warp_drc20();

    let name: alloc::string::String = session
        .direct_call::<_, alloc::string::String>(WARP_DRC20_ID, "name", &())
        .expect("name should succeed")
        .data;
    assert_eq!(name, "Wrapped ETH");

    let symbol: alloc::string::String = session
        .direct_call::<_, alloc::string::String>(WARP_DRC20_ID, "symbol", &())
        .expect("symbol should succeed")
        .data;
    assert_eq!(symbol, "WETH");

    let decimals: u8 = session
        .direct_call::<_, u8>(WARP_DRC20_ID, "decimals", &())
        .expect("decimals should succeed")
        .data;
    assert_eq!(decimals, 18);

    let supply: u64 = session
        .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
        .expect("total_supply should succeed")
        .data;
    assert_eq!(supply, 0);

    let mailbox: ContractId = session
        .direct_call::<_, ContractId>(WARP_DRC20_ID, "mailbox", &())
        .expect("mailbox should succeed")
        .data;
    assert_eq!(mailbox, MAILBOX_ID);
    assert_eq!(
        session
            .direct_call::<_, u32>(WARP_DRC20_ID, "state_version", &())
            .expect("state_version should succeed")
            .data,
        4
    );
}

#[test]
fn test_warp_drc20_zero_initial_supply() {
    let mut session = session_with_warp_drc20();

    // Verify supply starts at zero (no tokens minted)
    let supply: u64 = session
        .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
        .expect("total_supply should succeed")
        .data;
    assert_eq!(supply, 0);
}

#[test]
fn test_warp_drc20_enrolled_router_query() {
    let mut session = session_with_warp_drc20();

    // Query enrolled router for a domain that has no router → should return zero
    let router: H256 = session
        .direct_call::<_, H256>(WARP_DRC20_ID, "enrolled_router", &(REMOTE_DOMAIN,))
        .expect("enrolled_router should succeed")
        .data;
    assert_eq!(router, [0u8; 32]);
}

#[test]
fn test_warp_drc20_ism_override() {
    let mut session = session_with_warp_drc20();

    // Default: no ISM override (zero)
    let ism: ContractId = session
        .direct_call::<_, ContractId>(WARP_DRC20_ID, "interchain_security_module", &())
        .expect("ism should succeed")
        .data;
    assert_eq!(ism, ContractId::from_bytes([0u8; 32]));
}

#[test]
fn test_warp_drc20_admin_rejects_non_owner() {
    let mut session = session_with_warp_drc20();

    let result =
        session.call_public::<_, ()>(&RELAYER_SK, WARP_DRC20_ID, "set_ism", &(TEST_MOCK_ID,));

    assert_contract_panic(result, "WarpDrc20: caller is not the owner");
}

#[test]
fn test_warp_drc20_admin_accepts_owner_moonlight_sender() {
    let mut session = session_with_warp_drc20();

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_ID, "set_ism", &(TEST_MOCK_ID,))
        .expect("set_ism should succeed for owner");

    let ism: ContractId = session
        .direct_call::<_, ContractId>(WARP_DRC20_ID, "interchain_security_module", &())
        .expect("ism should succeed")
        .data;
    assert_eq!(ism, TEST_MOCK_ID);
}

#[test]
fn test_warp_drc20_owner_can_withdraw_route_dispatch_credit() {
    assert_route_dispatch_credit_withdrawal(
        session_with_warp_drc20(),
        WARP_DRC20_ID,
        "WarpDrc20: caller is not the owner",
    );
}

// =============================================================================
// Tests: WarpNative
// =============================================================================

#[test]
fn test_warp_native_init() {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);

    session
        .deploy(
            WARP_NATIVE_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID, *OWNER_ID, Vec::<(u32, H256)>::new()))
                .contract_id(WARP_NATIVE_ID),
        )
        .expect("Deploying WarpNative should succeed");

    let mailbox: ContractId = session
        .direct_call::<_, ContractId>(WARP_NATIVE_ID, "mailbox", &())
        .expect("mailbox should succeed")
        .data;
    assert_eq!(mailbox, MAILBOX_ID);

    let hook: ContractId = session
        .direct_call::<_, ContractId>(WARP_NATIVE_ID, "hook", &())
        .expect("hook should succeed")
        .data;
    assert_eq!(hook, ContractId::from_bytes([0u8; 32]));
    assert_eq!(
        session
            .direct_call::<_, u32>(WARP_NATIVE_ID, "state_version", &())
            .expect("state_version should succeed")
            .data,
        2
    );
}

#[test]
fn test_warp_native_register_account() {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);

    session
        .deploy(
            WARP_NATIVE_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID, *OWNER_ID, Vec::<(u32, H256)>::new()))
                .contract_id(WARP_NATIVE_ID),
        )
        .expect("Deploying WarpNative should succeed");

    // Register OWNER_PK via Moonlight TX (so abi::public_sender() is set)
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "register_account", &())
        .expect("register_account should succeed");

    // Compute the expected H256 = keccak256(pk.to_bytes())
    // The contract uses dusk_bytes::Serializable::to_bytes() (96 bytes).
    use dusk_bytes::Serializable;
    let pk_bytes = OWNER_PK.to_bytes();
    let h = message::keccak256(&pk_bytes);

    let registered: bool = session
        .direct_call::<_, bool>(WARP_NATIVE_ID, "is_registered", &(h,))
        .expect("is_registered should succeed")
        .data;
    assert!(registered, "Account should be registered");

    // Non-registered H256 should return false
    let not_registered: bool = session
        .direct_call::<_, bool>(WARP_NATIVE_ID, "is_registered", &([0xFFu8; 32],))
        .expect("is_registered should succeed")
        .data;
    assert!(!not_registered, "Random H256 should not be registered");
}

// =============================================================================
// Tests: WarpDrc20Collateral
// =============================================================================

#[test]
fn test_warp_collateral_init() {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);

    // Deploy a fake wrapped token (use TestMock as placeholder)
    session
        .deploy(
            TEST_MOCK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_MOCK_ID),
        )
        .expect("Deploying TestMock should succeed");

    session
        .deploy(
            WARP_DRC20_COLLATERAL_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    TEST_MOCK_ID,
                    MAILBOX_ID,
                    *OWNER_ID,
                    Vec::<(u32, H256)>::new(),
                ))
                .contract_id(WARP_DRC20_COLLATERAL_ID),
        )
        .expect("Deploying WarpDrc20Collateral should succeed");

    let wrapped: ContractId = session
        .direct_call::<_, ContractId>(WARP_DRC20_COLLATERAL_ID, "wrapped_token", &())
        .expect("wrapped_token should succeed")
        .data;
    assert_eq!(wrapped, TEST_MOCK_ID);

    let mailbox: ContractId = session
        .direct_call::<_, ContractId>(WARP_DRC20_COLLATERAL_ID, "mailbox", &())
        .expect("mailbox should succeed")
        .data;
    assert_eq!(mailbox, MAILBOX_ID);

    let state_version: u32 = session
        .direct_call::<_, u32>(WARP_DRC20_COLLATERAL_ID, "state_version", &())
        .expect("state_version should succeed")
        .data;
    assert_eq!(state_version, 3);
}

#[test]
fn test_warp_collateral_rejects_zero_token() {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);

    let result = session.deploy(
        WARP_DRC20_COLLATERAL_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                ContractId::from_bytes([0u8; 32]),
                MAILBOX_ID,
                *OWNER_ID,
                Vec::<(u32, H256)>::new(),
            ))
            .contract_id(WARP_DRC20_COLLATERAL_ID),
    );
    assert_deploy_panic(result, "WarpCollateral: wrapped token cannot be zero");
}

#[test]
fn test_owned_contracts_reject_zero_owner_at_initialization() {
    let zero_owner = [0u8; 32];
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);

    let mailbox = session.deploy(
        MAILBOX_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                LOCAL_DOMAIN,
                zero_owner,
                TEST_MOCK_ID,
                TEST_MOCK_ID,
                MERKLE_TREE_HOOK_ID,
            ))
            .contract_id(MAILBOX_ID),
    );
    assert_deploy_panic(mailbox, "Mailbox: owner cannot be zero");

    let igp = session.deploy(
        IGP_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                MAILBOX_ID,
                zero_owner,
                *OWNER_ID,
                Vec::<(u32, DomainGasConfig)>::new(),
            ))
            .contract_id(IGP_ID),
    );
    assert_deploy_panic(igp, "IGP: owner cannot be zero");

    let protocol_fee = session.deploy(
        PROTOCOL_FEE_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(1u64, 2u64, MAILBOX_ID, *OWNER_ID, zero_owner))
            .contract_id(PROTOCOL_FEE_ID),
    );
    assert_deploy_panic(protocol_fee, "ProtocolFee: owner cannot be zero");

    let multisig = session.deploy(
        ISM_MULTISIG_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(zero_owner, vec![EthAddress([1u8; 20])], 1u8))
            .contract_id(ISM_MULTISIG_ID),
    );
    assert_deploy_panic(multisig, "MultisigISM: owner cannot be zero");

    let synthetic = session.deploy(
        WARP_DRC20_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                MAILBOX_ID,
                zero_owner,
                alloc::string::String::from("Token"),
                alloc::string::String::from("TOK"),
                18u8,
                Vec::<(u32, H256)>::new(),
            ))
            .contract_id(WARP_DRC20_ID),
    );
    assert_deploy_panic(synthetic, "WarpDrc20: owner cannot be zero");

    let native = session.deploy(
        WARP_NATIVE_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(MAILBOX_ID, zero_owner, Vec::<(u32, H256)>::new()))
            .contract_id(WARP_NATIVE_ID),
    );
    assert_deploy_panic(native, "WarpNative: owner cannot be zero");

    let collateral = session.deploy(
        WARP_DRC20_COLLATERAL_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                TEST_MOCK_ID,
                MAILBOX_ID,
                zero_owner,
                Vec::<(u32, H256)>::new(),
            ))
            .contract_id(WARP_DRC20_COLLATERAL_ID),
    );
    assert_deploy_panic(collateral, "WarpCollateral: owner cannot be zero");
}

#[test]
fn test_warp_routes_reject_zero_mailbox_at_initialization() {
    let zero_mailbox = ContractId::from_bytes([0u8; 32]);
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);

    let synthetic = session.deploy(
        WARP_DRC20_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                zero_mailbox,
                *OWNER_ID,
                alloc::string::String::from("Token"),
                alloc::string::String::from("TOK"),
                18u8,
                Vec::<(u32, H256)>::new(),
            ))
            .contract_id(WARP_DRC20_ID),
    );
    assert_deploy_panic(synthetic, "WarpDrc20: mailbox cannot be zero");

    let native = session.deploy(
        WARP_NATIVE_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(zero_mailbox, *OWNER_ID, Vec::<(u32, H256)>::new()))
            .contract_id(WARP_NATIVE_ID),
    );
    assert_deploy_panic(native, "WarpNative: mailbox cannot be zero");

    let collateral = session.deploy(
        WARP_DRC20_COLLATERAL_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                TEST_MOCK_ID,
                zero_mailbox,
                *OWNER_ID,
                Vec::<(u32, H256)>::new(),
            ))
            .contract_id(WARP_DRC20_COLLATERAL_ID),
    );
    assert_deploy_panic(collateral, "WarpCollateral: mailbox cannot be zero");
}

#[test]
fn test_mailbox_rejects_zero_dependencies_at_initialization() {
    let zero = ContractId::from_bytes([0u8; 32]);

    for (default_ism, default_hook, required_hook, expected) in [
        (
            zero,
            TEST_MOCK_ID,
            MERKLE_TREE_HOOK_ID,
            "Mailbox: default ISM cannot be zero",
        ),
        (
            TEST_MOCK_ID,
            zero,
            MERKLE_TREE_HOOK_ID,
            "Mailbox: default hook cannot be zero",
        ),
        (
            TEST_MOCK_ID,
            TEST_MOCK_ID,
            zero,
            "Mailbox: required hook cannot be zero",
        ),
    ] {
        let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);
        let result = session.deploy(
            MAILBOX_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    LOCAL_DOMAIN,
                    *OWNER_ID,
                    default_ism,
                    default_hook,
                    required_hook,
                ))
                .contract_id(MAILBOX_ID),
        );
        assert_deploy_panic(result, expected);
    }
}

#[test]
fn test_warp_routes_reject_zero_router_at_initialization() {
    let zero_router = [0u8; 32];

    let mut synthetic_session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);
    let synthetic = synthetic_session.deploy(
        WARP_DRC20_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                MAILBOX_ID,
                *OWNER_ID,
                alloc::string::String::from("Token"),
                alloc::string::String::from("TOK"),
                18u8,
                vec![(REMOTE_DOMAIN, zero_router)],
            ))
            .contract_id(WARP_DRC20_ID),
    );
    assert_deploy_panic(synthetic, "WarpDrc20: router cannot be zero");

    let mut native_session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);
    let native = native_session.deploy(
        WARP_NATIVE_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(MAILBOX_ID, *OWNER_ID, vec![(REMOTE_DOMAIN, zero_router)]))
            .contract_id(WARP_NATIVE_ID),
    );
    assert_deploy_panic(native, "WarpNative: router cannot be zero");

    let mut collateral_session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);
    let collateral = collateral_session.deploy(
        WARP_DRC20_COLLATERAL_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                TEST_MOCK_ID,
                MAILBOX_ID,
                *OWNER_ID,
                vec![(REMOTE_DOMAIN, zero_router)],
            ))
            .contract_id(WARP_DRC20_COLLATERAL_ID),
    );
    assert_deploy_panic(collateral, "WarpCollateral: router cannot be zero");
}

#[test]
fn test_multisig_rejects_more_validators_than_threshold_abi_can_address() {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);
    let validators = (0u16..=u8::MAX as u16)
        .map(|index| {
            let mut address = [0u8; 20];
            address[18..].copy_from_slice(&index.to_be_bytes());
            EthAddress(address)
        })
        .collect::<Vec<_>>();

    let result = session.deploy(
        ISM_MULTISIG_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(*OWNER_ID, validators, 1u8))
            .contract_id(ISM_MULTISIG_ID),
    );
    assert_deploy_panic(result, "MultisigISM: too many validators");
}

// =============================================================================
// Tests: Warp route flow tests (handle / transfer_remote via Mailbox)
// =============================================================================

/// Deploy the full stack with WarpDrc20 enrolled for REMOTE_DOMAIN.
fn session_with_warp_drc20_flow() -> (TestSession, H256) {
    let remote_router: H256 = [0xAA; 32];

    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, INITIAL_DUSK_BALANCE),
        (&*RELAYER_PK, INITIAL_DUSK_BALANCE),
    ]);

    // Deploy TestMock (NullISM + NoopHook)
    session
        .deploy(
            TEST_MOCK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_MOCK_ID),
        )
        .expect("Deploying TestMock should succeed");

    // Deploy TestRecipient
    session
        .deploy(
            TEST_RECIPIENT_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_RECIPIENT_ID),
        )
        .expect("Deploying TestRecipient should succeed");

    // Deploy MerkleTreeHook
    session
        .deploy(
            MERKLE_TREE_HOOK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID,))
                .contract_id(MERKLE_TREE_HOOK_ID),
        )
        .expect("Deploying MerkleTreeHook should succeed");

    // Deploy WarpDrc20 with enrolled router for REMOTE_DOMAIN
    session
        .deploy(
            WARP_DRC20_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    MAILBOX_ID,
                    *OWNER_ID,
                    alloc::string::String::from("Wrapped ETH"),
                    alloc::string::String::from("WETH"),
                    18u8,
                    vec![(REMOTE_DOMAIN, remote_router)],
                ))
                .contract_id(WARP_DRC20_ID),
        )
        .expect("Deploying WarpDrc20 should succeed");

    // Deploy Mailbox
    session
        .deploy(
            MAILBOX_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    LOCAL_DOMAIN,
                    *OWNER_ID,           // owner
                    TEST_MOCK_ID,        // default ISM
                    TEST_MOCK_ID,        // default hook (noop)
                    MERKLE_TREE_HOOK_ID, // required hook
                ))
                .contract_id(MAILBOX_ID),
        )
        .expect("Deploying Mailbox should succeed");

    (session, remote_router)
}

#[test]
fn test_warp_drc20_init_with_enrolled_routers() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();

    // Verify the router was enrolled at init time
    let router: H256 = session
        .direct_call::<_, H256>(WARP_DRC20_ID, "enrolled_router", &(REMOTE_DOMAIN,))
        .expect("enrolled_router should succeed")
        .data;
    assert_eq!(router, remote_router);

    // Unknown domain returns zero
    let unknown: H256 = session
        .direct_call::<_, H256>(WARP_DRC20_ID, "enrolled_router", &(42u32,))
        .expect("enrolled_router should succeed")
        .data;
    assert_eq!(unknown, [0u8; 32]);
}

#[test]
fn test_warp_drc20_handle_mints_tokens() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();

    // Verify initial state
    let supply: u64 = session
        .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
        .expect("total_supply should succeed")
        .data;
    assert_eq!(supply, 0);

    // Build inbound message: origin=REMOTE_DOMAIN, sender=remote_router,
    // dest=LOCAL_DOMAIN, recipient=WARP_DRC20_ID
    // Body = TokenMessage(recipient=TEST_RECIPIENT_ID, amount=1_000_000)
    let mint_amount = 1_000_000u64;
    let token_body =
        hyperlane_dusk_types::token_message::encode(TEST_RECIPIENT_ID.to_bytes(), mint_amount);

    let encoded = message::encode(
        VERSION,
        0, // nonce
        REMOTE_DOMAIN,
        remote_router, // sender = enrolled router
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(), // recipient = warp contract
        &token_body,
    );

    let message_id = message::id(&encoded);

    // Process the message via Mailbox → calls WarpDrc20.handle()
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("process should succeed");

    // Verify message was delivered
    let delivered: bool = session
        .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(message_id,))
        .expect("delivered should succeed")
        .data;
    assert!(delivered);

    // An unregistered H256 is ambiguous (contract ID vs account hash), so
    // the route must hold it pending until the contract proves ownership.
    let supply: u64 = session
        .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
        .expect("total_supply should succeed")
        .data;
    assert_eq!(supply, 0);
    assert_eq!(
        session
            .direct_call::<_, u64>(
                WARP_DRC20_ID,
                "pending_balance",
                &(TEST_RECIPIENT_ID.to_bytes(),),
            )
            .expect("pending_balance should succeed")
            .data,
        mint_amount
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_ID, "pending_total", &())
            .expect("pending_total should succeed")
            .data,
        mint_amount
    );

    session
        .direct_call::<_, ()>(
            TEST_RECIPIENT_ID,
            "claim_synthetic_pending",
            &(WARP_DRC20_ID,),
        )
        .expect("recipient contract should claim its pending synthetic tokens");
    assert_eq!(
        warp_drc20_balance_of(&mut session, Drc20Account::Contract(TEST_RECIPIENT_ID)),
        mint_amount
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
            .expect("total_supply should succeed")
            .data,
        mint_amount
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_ID, "pending_total", &())
            .expect("pending_total should succeed")
            .data,
        0
    );
}

#[test]
fn test_warp_drc20_handle_multiple_mints() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();

    // Process three inbound transfers
    for i in 0u32..3 {
        let amount = (i as u64 + 1) * 500_000;
        let token_body =
            hyperlane_dusk_types::token_message::encode(TEST_RECIPIENT_ID.to_bytes(), amount);

        let encoded = message::encode(
            VERSION,
            i,
            REMOTE_DOMAIN,
            remote_router,
            LOCAL_DOMAIN,
            WARP_DRC20_ID.to_bytes(),
            &token_body,
        );

        session
            .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
            .expect("process should succeed");
    }

    // Total pending: 500_000 + 1_000_000 + 1_500_000 = 3_000_000.
    // Supply stays unchanged until the recipient proves its account type.
    let supply: u64 = session
        .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
        .expect("total_supply should succeed")
        .data;
    assert_eq!(supply, 0);
    assert_eq!(
        session
            .direct_call::<_, u64>(
                WARP_DRC20_ID,
                "pending_balance",
                &(TEST_RECIPIENT_ID.to_bytes(),),
            )
            .expect("pending_balance should succeed")
            .data,
        3_000_000
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_ID, "pending_total", &())
            .expect("pending_total should succeed")
            .data,
        3_000_000
    );

    session
        .direct_call::<_, ()>(
            TEST_RECIPIENT_ID,
            "claim_synthetic_pending",
            &(WARP_DRC20_ID,),
        )
        .expect("recipient contract should claim its accumulated pending balance");
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
            .expect("total_supply should succeed")
            .data,
        3_000_000
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_ID, "pending_total", &())
            .expect("pending_total should succeed")
            .data,
        0
    );
}

#[test]
fn test_warp_drc20_pending_supply_capacity_is_reserved() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();
    let pending_recipient = TEST_RECIPIENT_ID.to_bytes();
    let reserved = u64::MAX - 5;

    let first = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &hyperlane_dusk_types::token_message::encode(pending_recipient, reserved),
    );
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), first))
        .expect("first pending transfer should reserve supply capacity");
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_ID, "pending_total", &())
            .expect("pending_total should succeed")
            .data,
        reserved
    );

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_ID, "register_account", &())
        .expect("register_account should succeed");
    let registered_recipient = message::keccak256(&OWNER_PK.to_bytes());
    let direct = message::encode(
        VERSION,
        1,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &hyperlane_dusk_types::token_message::encode(registered_recipient, 6),
    );
    let direct_id = message::id(&direct);
    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), direct));
    assert_contract_panic_contains(result, "WarpDrc20: insufficient supply capacity");
    assert!(
        !session
            .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(direct_id,))
            .expect("delivered should succeed")
            .data
    );

    let second_pending = message::encode(
        VERSION,
        2,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &hyperlane_dusk_types::token_message::encode([0xA5; 32], 6),
    );
    let second_pending_id = message::id(&second_pending);
    let result =
        session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), second_pending));
    assert_contract_panic_contains(result, "WarpDrc20: insufficient supply capacity");
    assert!(
        !session
            .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(second_pending_id,))
            .expect("delivered should succeed")
            .data
    );

    session
        .direct_call::<_, ()>(
            TEST_RECIPIENT_ID,
            "claim_synthetic_pending",
            &(WARP_DRC20_ID,),
        )
        .expect("reserved pending transfer should remain claimable");
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
            .expect("total_supply should succeed")
            .data,
        reserved
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_ID, "pending_total", &())
            .expect("pending_total should succeed")
            .data,
        0
    );
}

#[test]
fn test_warp_drc20_handle_rejects_unenrolled_sender() {
    let (mut session, _remote_router) = session_with_warp_drc20_flow();

    // Build inbound message with wrong sender (not the enrolled router)
    let wrong_sender = [0xBBu8; 32];
    let token_body =
        hyperlane_dusk_types::token_message::encode(TEST_RECIPIENT_ID.to_bytes(), 1000);

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN, // origin is enrolled, but sender is wrong
        wrong_sender,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded));
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpDrc20: sender is not enrolled router for origin\")",
    );
}

#[test]
fn test_warp_drc20_handle_rejects_unenrolled_origin() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();

    // Build inbound message from unknown origin domain (42)
    let token_body =
        hyperlane_dusk_types::token_message::encode(TEST_RECIPIENT_ID.to_bytes(), 1000);

    let encoded = message::encode(
        VERSION,
        0,
        42, // unknown origin domain
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded));
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpDrc20: sender is not enrolled router for origin\")",
    );
}

#[test]
fn test_warp_drc20_handle_rejects_invalid_token_message() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();

    // Build inbound message with body too short to be a valid TokenMessage
    let short_body = vec![0u8; 32]; // only 32 bytes, need 64

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &short_body,
    );
    let message_id = message::id(&encoded);

    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded));
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpDrc20: invalid token message\")",
    );

    let delivered: bool = session
        .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(message_id,))
        .expect("delivered query should succeed")
        .data;
    assert!(
        !delivered,
        "malformed warp message must not be marked delivered"
    );

    let supply: u64 = session
        .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
        .expect("total_supply should succeed")
        .data;
    assert_eq!(supply, 0, "malformed warp message must not mint tokens");
}

#[test]
fn test_warp_drc20_handle_mints_to_registered_external_account() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();

    // Register OWNER_PK on WarpDrc20 via Moonlight TX
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_ID, "register_account", &())
        .expect("register_account should succeed");

    // Compute recipient H256 = keccak256(pk.to_bytes())
    use dusk_bytes::Serializable;
    let pk_bytes = OWNER_PK.to_bytes();
    let recipient_h256 = message::keccak256(&pk_bytes);

    // Verify it's registered
    let registered: bool = session
        .direct_call::<_, bool>(WARP_DRC20_ID, "is_registered", &(recipient_h256,))
        .expect("is_registered should succeed")
        .data;
    assert!(registered);

    // Build inbound message with recipient = registered account H256
    let mint_amount = 500_000u64;
    let token_body = hyperlane_dusk_types::token_message::encode(recipient_h256, mint_amount);

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &token_body,
    );

    // Process — should mint to the canonical Moonlight principal.
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("process should succeed");

    // Verify tokens were minted
    let supply: u64 = session
        .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
        .expect("total_supply should succeed")
        .data;
    assert_eq!(supply, mint_amount);
}

#[test]
fn test_warp_drc20_unregistered_external_claims_pending() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();
    let recipient_h256 = message::keccak256(&OWNER_PK.to_bytes());
    let mint_amount = 750_000u64;
    let token_body = hyperlane_dusk_types::token_message::encode(recipient_h256, mint_amount);
    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &token_body,
    );

    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("unregistered external transfer should become pending");
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_ID, "pending_balance", &(recipient_h256,))
            .expect("pending_balance should succeed")
            .data,
        mint_amount
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
            .expect("total_supply should succeed")
            .data,
        0
    );

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_ID, "claim_pending", &())
        .expect("authenticated external account should claim its pending tokens");
    assert_eq!(
        warp_drc20_balance_of(&mut session, Drc20Account::moonlight(&OWNER_PK)),
        mint_amount
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_ID, "pending_balance", &(recipient_h256,))
            .expect("pending_balance should succeed")
            .data,
        0
    );
}

// =============================================================================
// Tests: WarpNative flow tests
// =============================================================================

/// Deploy WarpNative with enrolled router for REMOTE_DOMAIN.
fn session_with_warp_native_flow() -> (TestSession, H256) {
    let remote_router: H256 = [0xBB; 32];

    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, INITIAL_DUSK_BALANCE),
        (&*RELAYER_PK, INITIAL_DUSK_BALANCE),
    ]);

    // Deploy TestMock
    session
        .deploy(
            TEST_MOCK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_MOCK_ID),
        )
        .expect("Deploying TestMock should succeed");

    // Deploy MerkleTreeHook
    session
        .deploy(
            MERKLE_TREE_HOOK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID,))
                .contract_id(MERKLE_TREE_HOOK_ID),
        )
        .expect("Deploying MerkleTreeHook should succeed");

    // Deploy WarpNative with enrolled router
    session
        .deploy(
            WARP_NATIVE_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID, *OWNER_ID, vec![(REMOTE_DOMAIN, remote_router)]))
                .contract_id(WARP_NATIVE_ID),
        )
        .expect("Deploying WarpNative should succeed");

    // Deploy Mailbox
    session
        .deploy(
            MAILBOX_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    LOCAL_DOMAIN,
                    *OWNER_ID,
                    TEST_MOCK_ID,
                    TEST_MOCK_ID,
                    MERKLE_TREE_HOOK_ID,
                ))
                .contract_id(MAILBOX_ID),
        )
        .expect("Deploying Mailbox should succeed");

    (session, remote_router)
}

#[test]
fn test_warp_native_init_with_enrolled_routers() {
    let (mut session, remote_router) = session_with_warp_native_flow();

    let router: H256 = session
        .direct_call::<_, H256>(WARP_NATIVE_ID, "enrolled_router", &(REMOTE_DOMAIN,))
        .expect("enrolled_router should succeed")
        .data;
    assert_eq!(router, remote_router);
}

#[test]
fn test_warp_native_admin_accepts_owner_and_rejects_non_owner() {
    let (mut session, _) = session_with_warp_native_flow();

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "set_ism", &(TEST_MOCK_ID,))
        .expect("WarpNative owner should update the ISM");
    let result = session.call_public::<_, ()>(
        &RELAYER_SK,
        WARP_NATIVE_ID,
        "set_ism",
        &(MERKLE_TREE_HOOK_ID,),
    );
    assert_contract_panic(result, "WarpNative: caller is not the owner");
}

#[test]
fn test_warp_native_owner_can_withdraw_route_dispatch_credit() {
    let (session, _) = session_with_warp_native_flow();
    assert_route_dispatch_credit_withdrawal(
        session,
        WARP_NATIVE_ID,
        "WarpNative: caller is not the owner",
    );
}

#[test]
fn test_warp_native_roundtrip_moves_real_dusk() {
    let (mut session, remote_router) = session_with_warp_native_flow();
    let amount = 1_000_000u64;
    let recipient = message::keccak256(&OWNER_PK.to_bytes());

    assert_eq!(
        session
            .contract_balance(&WARP_NATIVE_ID)
            .expect("WarpNative balance query should succeed"),
        0
    );

    session
        .call_public_with_deposit::<_, MessageId>(
            &OWNER_SK,
            WARP_NATIVE_ID,
            "transfer_remote",
            &(REMOTE_DOMAIN, [0xDD; 32], amount),
            amount,
        )
        .expect("outbound native transfer should lock its exact DUSK deposit");

    assert_eq!(
        session
            .contract_balance(&WARP_NATIVE_ID)
            .expect("WarpNative balance query should succeed"),
        amount
    );

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "register_account", &())
        .expect("recipient registration should succeed");
    let account_balance_before = session
        .account(&OWNER_PK)
        .expect("recipient account query should succeed")
        .balance;

    let body = hyperlane_dusk_types::token_message::encode(recipient, amount);
    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &body,
    );
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("inbound native transfer should unlock DUSK to the registered account");

    assert_eq!(
        session
            .contract_balance(&WARP_NATIVE_ID)
            .expect("WarpNative balance query should succeed"),
        0
    );
    assert_eq!(
        session
            .account(&OWNER_PK)
            .expect("recipient account query should succeed")
            .balance,
        account_balance_before + amount
    );
}

#[test]
fn test_warp_native_rejects_mismatched_deposit_without_custody() {
    let (mut session, _remote_router) = session_with_warp_native_flow();
    let amount = 1_000_000u64;

    let result = session.call_public_with_deposit::<_, MessageId>(
        &OWNER_SK,
        WARP_NATIVE_ID,
        "transfer_remote",
        &(REMOTE_DOMAIN, [0xDD; 32], amount),
        amount + 1,
    );
    assert_contract_panic_contains(result, "WarpNative: deposit failed");
    assert_eq!(
        session
            .contract_balance(&WARP_NATIVE_ID)
            .expect("WarpNative balance query should succeed"),
        0
    );
}

#[test]
fn test_warp_native_handle_rejects_unenrolled_sender() {
    let (mut session, _remote_router) = session_with_warp_native_flow();

    let wrong_sender = [0xCCu8; 32];

    // Register a BLS key first so we don't fail on "recipient not registered"
    use dusk_bytes::Serializable;
    let pk_bytes = OWNER_PK.to_bytes();
    let recipient_h256 = message::keccak256(&pk_bytes);

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "register_account", &())
        .expect("register_account should succeed");

    let token_body = hyperlane_dusk_types::token_message::encode(recipient_h256, 1000);

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        wrong_sender, // not the enrolled router
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded));
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpNative: sender is not enrolled router for origin\")",
    );
}

#[test]
fn test_warp_native_handle_rejects_unenrolled_origin() {
    let (mut session, remote_router) = session_with_warp_native_flow();

    use dusk_bytes::Serializable;
    let pk_bytes = OWNER_PK.to_bytes();
    let recipient_h256 = message::keccak256(&pk_bytes);

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "register_account", &())
        .expect("register_account should succeed");

    let token_body = hyperlane_dusk_types::token_message::encode(recipient_h256, 1000);

    let encoded = message::encode(
        VERSION,
        0,
        42, // unknown origin domain
        remote_router,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded));
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpNative: sender is not enrolled router for origin\")",
    );
}

#[test]
fn test_warp_native_handle_rejects_invalid_token_message() {
    let (mut session, remote_router) = session_with_warp_native_flow();

    let short_body = vec![0u8; 32]; // only 32 bytes, need 64
    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &short_body,
    );
    let message_id = message::id(&encoded);

    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded));
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpNative: invalid token message\")",
    );

    let delivered: bool = session
        .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(message_id,))
        .expect("delivered query should succeed")
        .data;
    assert!(
        !delivered,
        "malformed native warp message must not be marked delivered"
    );

    let pending: u64 = session
        .direct_call::<_, u64>(WARP_NATIVE_ID, "pending_balance", &([0x11u8; 32],))
        .expect("pending_balance should succeed")
        .data;
    assert_eq!(
        pending, 0,
        "malformed native warp message must not escrow funds"
    );
}

// =============================================================================
// Tests: WarpDrc20Collateral flow tests
// =============================================================================

/// Deploy WarpDrc20Collateral with enrolled router and a WarpDrc20 as the
/// underlying wrapped token (so we have a real DRC20 to lock/unlock).
fn session_with_warp_collateral_flow() -> (TestSession, H256) {
    let remote_router: H256 = [0xCC; 32];

    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, INITIAL_DUSK_BALANCE),
        (&*RELAYER_PK, INITIAL_DUSK_BALANCE),
    ]);

    // Deploy TestMock
    session
        .deploy(
            TEST_MOCK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_MOCK_ID),
        )
        .expect("Deploying TestMock should succeed");

    // Deploy TestRecipient
    session
        .deploy(
            TEST_RECIPIENT_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_RECIPIENT_ID),
        )
        .expect("Deploying TestRecipient should succeed");

    // Deploy MerkleTreeHook
    session
        .deploy(
            MERKLE_TREE_HOOK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID,))
                .contract_id(MERKLE_TREE_HOOK_ID),
        )
        .expect("Deploying MerkleTreeHook should succeed");

    // Deploy WarpDrc20 as the underlying token (for collateral to lock/unlock)
    session
        .deploy(
            WARP_DRC20_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    MAILBOX_ID,
                    *OWNER_ID,
                    alloc::string::String::from("Test Token"),
                    alloc::string::String::from("TEST"),
                    18u8,
                    Vec::<(u32, H256)>::new(),
                ))
                .contract_id(WARP_DRC20_ID),
        )
        .expect("Deploying WarpDrc20 should succeed");

    // Deploy WarpDrc20Collateral with enrolled router
    session
        .deploy(
            WARP_DRC20_COLLATERAL_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    WARP_DRC20_ID, // wrapped token = WarpDrc20
                    MAILBOX_ID,
                    *OWNER_ID, // owner
                    vec![(REMOTE_DOMAIN, remote_router)],
                ))
                .contract_id(WARP_DRC20_COLLATERAL_ID),
        )
        .expect("Deploying WarpDrc20Collateral should succeed");

    // Deploy Mailbox
    session
        .deploy(
            MAILBOX_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    LOCAL_DOMAIN,
                    *OWNER_ID,
                    TEST_MOCK_ID,
                    TEST_MOCK_ID,
                    MERKLE_TREE_HOOK_ID,
                ))
                .contract_id(MAILBOX_ID),
        )
        .expect("Deploying Mailbox should succeed");

    (session, remote_router)
}

/// Deploy the exact upstream Dusk contract-standards DRC20 example at
/// `bc1b00ee0af059975e158b7b580b4d0c0f1bdf9f` as collateral.
fn session_with_canonical_drc20_collateral(initial_token_balance: u64) -> (TestSession, H256) {
    let remote_router: H256 = [0xCC; 32];
    let owner = Drc20Account::moonlight(&OWNER_PK);

    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, INITIAL_DUSK_BALANCE),
        (&*RELAYER_PK, INITIAL_DUSK_BALANCE),
    ]);

    session
        .deploy(
            TEST_MOCK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_MOCK_ID),
        )
        .expect("Deploying TestMock should succeed");
    session
        .deploy(
            MERKLE_TREE_HOOK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID,))
                .contract_id(MERKLE_TREE_HOOK_ID),
        )
        .expect("Deploying MerkleTreeHook should succeed");
    session
        .deploy(
            CANONICAL_DRC20_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&CanonicalDrc20ExampleInit {
                    admin: owner,
                    token: CanonicalDrc20TokenInit {
                        name: alloc::string::String::from("Canonical Dusk Token"),
                        symbol: alloc::string::String::from("CDRC20"),
                        decimals: 9,
                        initial_balances: vec![CanonicalDrc20InitBalance {
                            account: owner,
                            amount: initial_token_balance,
                        }],
                    },
                    cap: 0,
                })
                .contract_id(WARP_DRC20_ID),
        )
        .expect("Deploying the pinned canonical DRC20 should succeed");
    session
        .deploy(
            WARP_DRC20_COLLATERAL_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    WARP_DRC20_ID,
                    MAILBOX_ID,
                    *OWNER_ID,
                    vec![(REMOTE_DOMAIN, remote_router)],
                ))
                .contract_id(WARP_DRC20_COLLATERAL_ID),
        )
        .expect("Deploying WarpDrc20Collateral should succeed");
    session
        .deploy(
            MAILBOX_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    LOCAL_DOMAIN,
                    *OWNER_ID,
                    TEST_MOCK_ID,
                    TEST_MOCK_ID,
                    MERKLE_TREE_HOOK_ID,
                ))
                .contract_id(MAILBOX_ID),
        )
        .expect("Deploying Mailbox should succeed");

    (session, remote_router)
}

#[test]
fn test_warp_collateral_round_trip_with_pinned_canonical_drc20() {
    let initial = 5_000_000u64;
    let outbound = 3_000_000u64;
    let inbound = 1_250_000u64;
    let owner = Drc20Account::moonlight(&OWNER_PK);
    let custody = Drc20Account::Contract(WARP_DRC20_COLLATERAL_ID);
    let (mut session, remote_router) = session_with_canonical_drc20_collateral(initial);

    assert_eq!(canonical_drc20_balance_of(&mut session, owner), initial);
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_ID,
            "approve",
            &Drc20ApproveCall {
                spender: custody,
                amount: outbound,
            },
        )
        .expect("the pinned canonical DRC20 approve ABI should succeed");
    session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            WARP_DRC20_COLLATERAL_ID,
            "transfer_remote",
            &(REMOTE_DOMAIN, [0xAB; 32], outbound),
        )
        .expect("the route should take exact custody and dispatch");

    assert_eq!(
        canonical_drc20_balance_of(&mut session, owner),
        initial - outbound
    );
    assert_eq!(canonical_drc20_balance_of(&mut session, custody), outbound);
    assert_eq!(
        session
            .direct_call::<_, u64>(
                WARP_DRC20_ID,
                "allowance",
                &Drc20Allowance {
                    owner,
                    spender: custody,
                },
            )
            .expect("canonical allowance query should succeed")
            .data,
        0
    );

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_COLLATERAL_ID, "register_account", &())
        .expect("the inbound Moonlight recipient should register");
    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &hyperlane_dusk_types::token_message::encode(*OWNER_ID, inbound),
    );
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("inbound delivery should release canonical DRC20 custody");

    assert_eq!(
        canonical_drc20_balance_of(&mut session, owner),
        initial - outbound + inbound
    );
    assert_eq!(
        canonical_drc20_balance_of(&mut session, custody),
        outbound - inbound
    );
}

#[test]
fn test_warp_collateral_init_with_enrolled_routers() {
    let (mut session, remote_router) = session_with_warp_collateral_flow();

    let router: H256 = session
        .direct_call::<_, H256>(
            WARP_DRC20_COLLATERAL_ID,
            "enrolled_router",
            &(REMOTE_DOMAIN,),
        )
        .expect("enrolled_router should succeed")
        .data;
    assert_eq!(router, remote_router);
}

#[test]
fn test_warp_routes_reject_zero_router_enrollment() {
    let zero_router = [0u8; 32];

    let (mut synthetic, _) = session_with_warp_drc20_flow();
    let result = synthetic.call_public::<_, ()>(
        &OWNER_SK,
        WARP_DRC20_ID,
        "enroll_remote_router",
        &(42u32, zero_router),
    );
    assert_contract_panic(result, "WarpDrc20: router cannot be zero");

    let (mut native, _) = session_with_warp_native_flow();
    let result = native.call_public::<_, ()>(
        &OWNER_SK,
        WARP_NATIVE_ID,
        "enroll_remote_router",
        &(42u32, zero_router),
    );
    assert_contract_panic(result, "WarpNative: router cannot be zero");

    let (mut collateral, _) = session_with_warp_collateral_flow();
    let result = collateral.call_public::<_, ()>(
        &OWNER_SK,
        WARP_DRC20_COLLATERAL_ID,
        "enroll_remote_router",
        &(42u32, zero_router),
    );
    assert_contract_panic(result, "WarpCollateral: router cannot be zero");
}

#[test]
fn test_warp_collateral_admin_accepts_owner_and_rejects_non_owner() {
    let (mut session, _) = session_with_warp_collateral_flow();

    session
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_COLLATERAL_ID,
            "set_ism",
            &(TEST_MOCK_ID,),
        )
        .expect("WarpCollateral owner should update the ISM");
    let result = session.call_public::<_, ()>(
        &RELAYER_SK,
        WARP_DRC20_COLLATERAL_ID,
        "set_ism",
        &(MERKLE_TREE_HOOK_ID,),
    );
    assert_contract_panic(result, "WarpCollateral: caller is not the owner");
}

#[test]
fn test_warp_collateral_owner_can_withdraw_route_dispatch_credit() {
    let (session, _) = session_with_warp_collateral_flow();
    assert_route_dispatch_credit_withdrawal(
        session,
        WARP_DRC20_COLLATERAL_ID,
        "WarpCollateral: caller is not the owner",
    );
}

#[test]
fn test_warp_collateral_handle_rejects_unenrolled_sender() {
    let (mut session, _remote_router) = session_with_warp_collateral_flow();

    let wrong_sender = [0xDDu8; 32];
    let token_body =
        hyperlane_dusk_types::token_message::encode(TEST_RECIPIENT_ID.to_bytes(), 1000);

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        wrong_sender,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded));
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpCollateral: sender is not enrolled router for origin\")",
    );
}

#[test]
fn test_warp_collateral_handle_rejects_unenrolled_origin() {
    let (mut session, remote_router) = session_with_warp_collateral_flow();

    let token_body =
        hyperlane_dusk_types::token_message::encode(TEST_RECIPIENT_ID.to_bytes(), 1000);

    let encoded = message::encode(
        VERSION,
        0,
        42, // unknown origin
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded));
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpCollateral: sender is not enrolled router for origin\")",
    );
}

#[test]
fn test_warp_collateral_handle_rejects_invalid_token_message() {
    let (mut session, remote_router) = session_with_warp_collateral_flow();

    let short_body = vec![0u8; 32]; // only 32 bytes, need 64
    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &short_body,
    );
    let message_id = message::id(&encoded);

    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded));
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpCollateral: invalid token message\")",
    );

    let delivered: bool = session
        .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(message_id,))
        .expect("delivered query should succeed")
        .data;
    assert!(
        !delivered,
        "malformed collateral warp message must not be marked delivered"
    );

    let pending: u64 = session
        .direct_call::<_, u64>(
            WARP_DRC20_COLLATERAL_ID,
            "pending_balance",
            &([0x11u8; 32],),
        )
        .expect("pending_balance should succeed")
        .data;
    assert_eq!(
        pending, 0,
        "malformed collateral warp message must not escrow funds"
    );
}

// =============================================================================
// Tests: Security fix verification
// =============================================================================

// --- WarpDrc20: zero-amount rejection ---

#[test]
fn test_warp_drc20_transfer_remote_rejects_zero_amount() {
    let (mut session, _remote_router) = session_with_warp_drc20_flow();

    // The assert fires before sender_account(), so direct_call works
    let result = session.direct_call::<_, MessageId>(
        WARP_DRC20_ID,
        "transfer_remote",
        &(REMOTE_DOMAIN, [0xFFu8; 32], 0u64),
    );
    assert_contract_panic(result, "WarpDrc20: amount must be > 0");
}

#[test]
fn test_warp_native_transfer_remote_rejects_zero_amount() {
    let (mut session, _remote_router) = session_with_warp_native_flow();

    // The assert fires before deposit claiming, so direct_call is sufficient.
    let result = session.direct_call::<_, MessageId>(
        WARP_NATIVE_ID,
        "transfer_remote",
        &(REMOTE_DOMAIN, [0xFFu8; 32], 0u64),
    );
    assert_contract_panic(result, "WarpNative: amount must be > 0");
}

#[test]
fn test_warp_collateral_transfer_remote_rejects_zero_amount() {
    let (mut session, _remote_router) = session_with_warp_collateral_flow();

    let result = session.direct_call::<_, MessageId>(
        WARP_DRC20_COLLATERAL_ID,
        "transfer_remote",
        &(REMOTE_DOMAIN, [0xFFu8; 32], 0u64),
    );
    assert_contract_panic(result, "WarpCollateral: amount must be > 0");
}

#[test]
fn test_warp_routes_reject_zero_amount_inbound_without_state_changes() {
    let recipient = [0xEE; 32];

    let (mut drc20_session, drc20_router) = session_with_warp_drc20_flow();
    let drc20_body = hyperlane_dusk_types::token_message::encode(recipient, 0);
    let drc20_message = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        drc20_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &drc20_body,
    );
    let result = drc20_session.direct_call::<_, ()>(
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), drc20_message),
    );
    assert_contract_panic_contains(result, "WarpDrc20: amount must be > 0");
    assert_eq!(
        drc20_session
            .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
            .expect("total_supply should succeed")
            .data,
        0
    );

    let (mut native_session, native_router) = session_with_warp_native_flow();
    let native_body = hyperlane_dusk_types::token_message::encode(recipient, 0);
    let native_message = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        native_router,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &native_body,
    );
    let result = native_session.direct_call::<_, ()>(
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), native_message),
    );
    assert_contract_panic_contains(result, "WarpNative: amount must be > 0");
    assert_eq!(
        native_session
            .direct_call::<_, u64>(WARP_NATIVE_ID, "pending_balance", &(recipient,))
            .expect("pending_balance should succeed")
            .data,
        0
    );

    let (mut collateral_session, collateral_router) = session_with_warp_collateral_funded_flow();
    let collateral_body = hyperlane_dusk_types::token_message::encode(recipient, 0);
    let collateral_message = message::encode(
        VERSION,
        1,
        REMOTE_DOMAIN,
        collateral_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &collateral_body,
    );
    let result = collateral_session.direct_call::<_, ()>(
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), collateral_message),
    );
    assert_contract_panic_contains(result, "WarpCollateral: amount must be > 0");
    assert_eq!(
        collateral_session
            .direct_call::<_, u64>(WARP_DRC20_COLLATERAL_ID, "pending_balance", &(recipient,),)
            .expect("pending_balance should succeed")
            .data,
        0
    );
}

// --- WarpNative: escrow for unregistered recipients ---

fn fund_warp_native(session: &mut TestSession, amount: u64) {
    session
        .call_public_with_deposit::<_, MessageId>(
            &OWNER_SK,
            WARP_NATIVE_ID,
            "transfer_remote",
            &(REMOTE_DOMAIN, [0xDD; 32], amount),
            amount,
        )
        .expect("outbound native transfer should establish real DUSK custody");
}

#[test]
fn test_warp_native_rejects_unbacked_inbound_escrow() {
    let (mut session, remote_router) = session_with_warp_native_flow();
    let unregistered: H256 = [0xEE; 32];
    let amount = 1_000_000u64;
    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &hyperlane_dusk_types::token_message::encode(unregistered, amount),
    );
    let message_id = message::id(&encoded);

    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded));
    assert_contract_panic_contains(result, "WarpNative: insufficient unreserved DUSK");
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_NATIVE_ID, "pending_balance", &(unregistered,))
            .expect("pending_balance should succeed")
            .data,
        0
    );
    assert!(
        !session
            .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(message_id,))
            .expect("delivered should succeed")
            .data
    );
}

#[test]
fn test_warp_native_handle_escrows_unregistered_recipient() {
    let (mut session, remote_router) = session_with_warp_native_flow();

    // Inbound message with a recipient H256 that is NOT registered
    let unregistered: H256 = [0xEE; 32];
    let amount = 1_000_000u64;
    fund_warp_native(&mut session, amount);
    let token_body = hyperlane_dusk_types::token_message::encode(unregistered, amount);

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &token_body,
    );

    // Previously panicked with "recipient not registered". Now it escrows.
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("process should succeed — unregistered recipient goes to escrow");

    // Verify pending balance
    let pending: u64 = session
        .direct_call::<_, u64>(WARP_NATIVE_ID, "pending_balance", &(unregistered,))
        .expect("pending_balance should succeed")
        .data;
    assert_eq!(pending, amount);
}

#[test]
fn test_warp_native_escrow_accumulates() {
    let (mut session, remote_router) = session_with_warp_native_flow();

    let unregistered: H256 = [0xEE; 32];
    fund_warp_native(&mut session, 1_500_000);

    // Process two inbound messages to the same unregistered recipient
    for nonce in 0u32..2 {
        let amount = (u64::from(nonce) + 1) * 500_000;
        let token_body = hyperlane_dusk_types::token_message::encode(unregistered, amount);

        let encoded = message::encode(
            VERSION,
            nonce,
            REMOTE_DOMAIN,
            remote_router,
            LOCAL_DOMAIN,
            WARP_NATIVE_ID.to_bytes(),
            &token_body,
        );

        session
            .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
            .expect("process should succeed");
    }

    // Pending should be 500K + 1M = 1.5M
    let pending: u64 = session
        .direct_call::<_, u64>(WARP_NATIVE_ID, "pending_balance", &(unregistered,))
        .expect("pending_balance should succeed")
        .data;
    assert_eq!(pending, 1_500_000);
}

#[test]
fn test_warp_native_pending_liability_has_priority_over_direct_delivery() {
    let (mut session, remote_router) = session_with_warp_native_flow();
    let unregistered: H256 = [0xEE; 32];
    let registered = message::keccak256(&OWNER_PK.to_bytes());
    fund_warp_native(&mut session, 2_000_000);
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "register_account", &())
        .expect("recipient registration should succeed");

    let escrow = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &hyperlane_dusk_types::token_message::encode(unregistered, 1_500_000),
    );
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), escrow))
        .expect("backed pending transfer should succeed");
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_NATIVE_ID, "pending_total", &())
            .expect("pending_total should succeed")
            .data,
        1_500_000
    );

    let balance_before = session
        .account(&OWNER_PK)
        .expect("recipient account query should succeed")
        .balance;
    let oversized = message::encode(
        VERSION,
        1,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &hyperlane_dusk_types::token_message::encode(registered, 1_000_000),
    );
    let result =
        session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), oversized));
    assert_contract_panic_contains(result, "WarpNative: insufficient unreserved DUSK");
    assert_eq!(
        session
            .account(&OWNER_PK)
            .expect("recipient account query should succeed")
            .balance,
        balance_before
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_NATIVE_ID, "pending_total", &())
            .expect("pending_total should succeed")
            .data,
        1_500_000
    );

    let available = message::encode(
        VERSION,
        2,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &hyperlane_dusk_types::token_message::encode(registered, 500_000),
    );
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), available))
        .expect("delivery within unreserved custody should succeed");
    assert_eq!(
        session
            .account(&OWNER_PK)
            .expect("recipient account query should succeed")
            .balance,
        balance_before + 500_000
    );
}

#[test]
fn test_warp_native_claim_pending_requires_pending() {
    let (mut session, _remote_router) = session_with_warp_native_flow();

    // Register the account first
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "register_account", &())
        .expect("register_account should succeed");

    // Try to claim with no pending transfers
    let result = session.call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "claim_pending", &());
    assert_contract_panic(result, "WarpNative: no pending transfers");
}

// --- WarpDrc20Collateral: External account registration ---

#[test]
fn test_warp_collateral_register_account() {
    let (mut session, _remote_router) = session_with_warp_collateral_flow();

    let pk_bytes = OWNER_PK.to_bytes();
    let recipient_h256 = message::keccak256(&pk_bytes);

    // Before registration
    let registered: bool = session
        .direct_call::<_, bool>(
            WARP_DRC20_COLLATERAL_ID,
            "is_registered",
            &(recipient_h256,),
        )
        .expect("is_registered should succeed")
        .data;
    assert!(!registered);

    // Register via Moonlight TX
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_COLLATERAL_ID, "register_account", &())
        .expect("register_account should succeed");

    // After registration
    let registered: bool = session
        .direct_call::<_, bool>(
            WARP_DRC20_COLLATERAL_ID,
            "is_registered",
            &(recipient_h256,),
        )
        .expect("is_registered should succeed")
        .data;
    assert!(registered);
}

#[test]
fn test_warp_collateral_handle_rejects_insufficient_locked_balance() {
    let (mut session, remote_router) = session_with_warp_collateral_flow();

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_COLLATERAL_ID, "register_account", &())
        .expect("register_account should succeed");

    let recipient_h256 = message::keccak256(&OWNER_PK.to_bytes());
    let token_body = hyperlane_dusk_types::token_message::encode(recipient_h256, 500_000);

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded));
    assert_contract_panic_contains(result, "WarpCollateral: insufficient unreserved collateral");
}

/// Deploy WarpDrc20Collateral with funded DRC20 balance so handle() can
/// unlock tokens to registered External accounts.
fn session_with_warp_collateral_funded_flow() -> (TestSession, H256) {
    let remote_router: H256 = [0xCC; 32];

    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, INITIAL_DUSK_BALANCE),
        (&*RELAYER_PK, INITIAL_DUSK_BALANCE),
    ]);

    // Deploy TestMock
    session
        .deploy(
            TEST_MOCK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_MOCK_ID),
        )
        .expect("Deploying TestMock should succeed");

    // Deploy TestRecipient
    session
        .deploy(
            TEST_RECIPIENT_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_RECIPIENT_ID),
        )
        .expect("Deploying TestRecipient should succeed");

    // Deploy MerkleTreeHook
    session
        .deploy(
            MERKLE_TREE_HOOK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID,))
                .contract_id(MERKLE_TREE_HOOK_ID),
        )
        .expect("Deploying MerkleTreeHook should succeed");

    // Deploy WarpDrc20 WITH enrolled routers (so we can mint via handle)
    session
        .deploy(
            WARP_DRC20_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    MAILBOX_ID,
                    *OWNER_ID,
                    alloc::string::String::from("Test Token"),
                    alloc::string::String::from("TEST"),
                    18u8,
                    vec![(REMOTE_DOMAIN, remote_router)],
                ))
                .contract_id(WARP_DRC20_ID),
        )
        .expect("Deploying WarpDrc20 should succeed");

    // Deploy WarpDrc20Collateral
    session
        .deploy(
            WARP_DRC20_COLLATERAL_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    WARP_DRC20_ID,
                    MAILBOX_ID,
                    *OWNER_ID,
                    vec![(REMOTE_DOMAIN, remote_router)],
                ))
                .contract_id(WARP_DRC20_COLLATERAL_ID),
        )
        .expect("Deploying WarpDrc20Collateral should succeed");

    // Deploy Mailbox
    session
        .deploy(
            MAILBOX_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    LOCAL_DOMAIN,
                    *OWNER_ID,
                    TEST_MOCK_ID,
                    TEST_MOCK_ID,
                    MERKLE_TREE_HOOK_ID,
                ))
                .contract_id(MAILBOX_ID),
        )
        .expect("Deploying Mailbox should succeed");

    // Pre-fund through the real collateral path: mint to an authenticated
    // external owner, approve the route, then lock the tokens with
    // transfer_remote. An unregistered H256 is intentionally pending in the
    // synthetic route and therefore cannot be used to fabricate contract
    // custody for this fixture.
    let fund_amount = 10_000_000u64;
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_ID, "register_account", &())
        .expect("underlying token owner registration should succeed");
    let token_body = hyperlane_dusk_types::token_message::encode(*OWNER_ID, fund_amount);
    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &token_body,
    );
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("minting the fixture's collateral should succeed");
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_ID,
            "approve",
            &Drc20ApproveCall {
                spender: Drc20Account::Contract(WARP_DRC20_COLLATERAL_ID),
                amount: fund_amount,
            },
        )
        .expect("collateral route approval should succeed");
    session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            WARP_DRC20_COLLATERAL_ID,
            "transfer_remote",
            &(REMOTE_DOMAIN, [0xAB; 32], fund_amount),
        )
        .expect("collateral pre-funding should lock real DRC20 custody");

    (session, remote_router)
}

#[test]
fn test_warp_collateral_transfer_remote_uses_current_drc20_allowance_and_custody() {
    let (mut session, remote_router) = session_with_warp_collateral_funded_flow();
    let amount = 1_000_000u64;

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_ID, "register_account", &())
        .expect("underlying token account registration should succeed");
    let owner = Drc20Account::moonlight(&OWNER_PK);
    let token_body = hyperlane_dusk_types::token_message::encode(*OWNER_ID, amount);
    let encoded = message::encode(
        VERSION,
        1,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &token_body,
    );
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("minting collateral to the owner should succeed");

    let spender = Drc20Account::Contract(WARP_DRC20_COLLATERAL_ID);
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_ID,
            "approve",
            &Drc20ApproveCall { spender, amount },
        )
        .expect("current DRC20 approve ABI should succeed");
    let allowance = session
        .direct_call::<_, u64>(
            WARP_DRC20_ID,
            "allowance",
            &Drc20Allowance { owner, spender },
        )
        .expect("allowance query should succeed")
        .data;
    assert_eq!(allowance, amount);

    let collateral_before = warp_drc20_balance_of(&mut session, spender);
    session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            WARP_DRC20_COLLATERAL_ID,
            "transfer_remote",
            &(REMOTE_DOMAIN, [0xAB; 32], amount),
        )
        .expect("collateral transfer should lock approved DRC20 tokens");

    assert_eq!(warp_drc20_balance_of(&mut session, owner), 0);
    assert_eq!(
        warp_drc20_balance_of(&mut session, spender),
        collateral_before + amount
    );
    let allowance = session
        .direct_call::<_, u64>(
            WARP_DRC20_ID,
            "allowance",
            &Drc20Allowance { owner, spender },
        )
        .expect("allowance query should succeed")
        .data;
    assert_eq!(allowance, 0);
}

#[test]
fn test_warp_collateral_handle_resolves_registered_external() {
    let (mut session, remote_router) = session_with_warp_collateral_funded_flow();

    // Register OWNER_PK on collateral contract
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_COLLATERAL_ID, "register_account", &())
        .expect("register_account should succeed");

    // Inbound message to collateral: recipient = keccak256(OWNER_PK)
    let pk_bytes = OWNER_PK.to_bytes();
    let recipient_h256 = message::keccak256(&pk_bytes);

    let unlock_amount = 500_000u64;
    let token_body = hyperlane_dusk_types::token_message::encode(recipient_h256, unlock_amount);

    let encoded = message::encode(
        VERSION,
        1, // nonce=1 (nonce=0 used for pre-funding)
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &token_body,
    );

    // Process — collateral resolves to the canonical Moonlight principal.
    // and transfers DRC20 tokens from its own balance
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("process should succeed — collateral unlocks to External account");
}

#[test]
fn test_warp_collateral_handle_escrows_unregistered_recipient() {
    let (mut session, remote_router) = session_with_warp_collateral_funded_flow();

    let unregistered: H256 = [0xEE; 32];
    let unlock_amount = 500_000u64;
    let contract_balance_before = warp_drc20_balance_of(
        &mut session,
        Drc20Account::Contract(WARP_DRC20_COLLATERAL_ID),
    );

    let token_body = hyperlane_dusk_types::token_message::encode(unregistered, unlock_amount);
    let encoded = message::encode(
        VERSION,
        1,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &token_body,
    );

    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("process should succeed — unregistered collateral recipient goes to escrow");

    let pending: u64 = session
        .direct_call::<_, u64>(
            WARP_DRC20_COLLATERAL_ID,
            "pending_balance",
            &(unregistered,),
        )
        .expect("pending_balance should succeed")
        .data;
    assert_eq!(pending, unlock_amount);
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_COLLATERAL_ID, "pending_total", &())
            .expect("pending_total should succeed")
            .data,
        unlock_amount
    );

    let contract_balance_after = warp_drc20_balance_of(
        &mut session,
        Drc20Account::Contract(WARP_DRC20_COLLATERAL_ID),
    );
    assert_eq!(contract_balance_after, contract_balance_before);
}

#[test]
fn test_warp_collateral_claim_pending_transfers_after_registration() {
    let (mut session, remote_router) = session_with_warp_collateral_funded_flow();

    let recipient_h256 = message::keccak256(&OWNER_PK.to_bytes());
    let unlock_amount = 500_000u64;
    let contract_balance_before = warp_drc20_balance_of(
        &mut session,
        Drc20Account::Contract(WARP_DRC20_COLLATERAL_ID),
    );

    let token_body = hyperlane_dusk_types::token_message::encode(recipient_h256, unlock_amount);
    let encoded = message::encode(
        VERSION,
        1,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &token_body,
    );

    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("process should escrow before registration");

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_COLLATERAL_ID, "register_account", &())
        .expect("register_account should succeed");
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_COLLATERAL_ID, "claim_pending", &())
        .expect("claim_pending should succeed");

    let pending: u64 = session
        .direct_call::<_, u64>(
            WARP_DRC20_COLLATERAL_ID,
            "pending_balance",
            &(recipient_h256,),
        )
        .expect("pending_balance should succeed")
        .data;
    assert_eq!(pending, 0);
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_COLLATERAL_ID, "pending_total", &())
            .expect("pending_total should succeed")
            .data,
        0
    );

    let external_balance = warp_drc20_balance_of(&mut session, Drc20Account::moonlight(&OWNER_PK));
    assert_eq!(external_balance, unlock_amount);

    let contract_balance_after = warp_drc20_balance_of(
        &mut session,
        Drc20Account::Contract(WARP_DRC20_COLLATERAL_ID),
    );
    assert_eq!(
        contract_balance_after,
        contract_balance_before - unlock_amount
    );
}

#[test]
fn test_warp_collateral_reserves_pending_custody_from_direct_delivery() {
    let (mut session, remote_router) = session_with_warp_collateral_funded_flow();
    let pending_recipient = [0xE1; 32];
    let pending_amount = 8_000_000u64;

    let pending_body =
        hyperlane_dusk_types::token_message::encode(pending_recipient, pending_amount);
    let pending_message = message::encode(
        VERSION,
        1,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &pending_body,
    );
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), pending_message))
        .expect("backed pending collateral should be accepted");

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_COLLATERAL_ID, "register_account", &())
        .expect("register_account should succeed");
    let recipient = message::keccak256(&OWNER_PK.to_bytes());
    let direct_amount = 3_000_000u64;
    let direct_body = hyperlane_dusk_types::token_message::encode(recipient, direct_amount);
    let direct_message = message::encode(
        VERSION,
        2,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &direct_body,
    );
    let direct_id = message::id(&direct_message);
    let result =
        session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), direct_message));
    assert_contract_panic_contains(result, "WarpCollateral: insufficient unreserved collateral");

    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_COLLATERAL_ID, "pending_total", &())
            .expect("pending_total should succeed")
            .data,
        pending_amount
    );
    assert!(
        !session
            .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(direct_id,))
            .expect("delivered should succeed")
            .data
    );
}

#[test]
fn test_warp_collateral_contract_recipient_claims_authenticated_escrow() {
    let (mut session, remote_router) = session_with_warp_collateral_funded_flow();
    let recipient = TEST_RECIPIENT_ID.to_bytes();
    let unlock_amount = 500_000u64;

    let token_body = hyperlane_dusk_types::token_message::encode(recipient, unlock_amount);
    let encoded = message::encode(
        VERSION,
        1,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &token_body,
    );
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("contract-recipient collateral should enter escrow");
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_COLLATERAL_ID, "pending_balance", &(recipient,),)
            .expect("pending_balance should succeed")
            .data,
        unlock_amount
    );

    session
        .direct_call::<_, ()>(
            TEST_RECIPIENT_ID,
            "claim_collateral_pending",
            &(WARP_DRC20_COLLATERAL_ID,),
        )
        .expect("the recipient contract should claim its own collateral escrow");

    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_DRC20_COLLATERAL_ID, "pending_balance", &(recipient,),)
            .expect("pending_balance should succeed")
            .data,
        0
    );
    assert_eq!(
        warp_drc20_balance_of(&mut session, Drc20Account::Contract(TEST_RECIPIENT_ID)),
        unlock_amount
    );
}

#[test]
fn test_warp_collateral_contract_claim_rejects_root_moonlight_caller() {
    let (mut session, _remote_router) = session_with_warp_collateral_funded_flow();
    let result = session.call_public::<_, ()>(
        &OWNER_SK,
        WARP_DRC20_COLLATERAL_ID,
        "claim_pending_contract",
        &(),
    );
    assert_contract_panic(
        result,
        "WarpCollateral: claim_pending_contract requires contract caller",
    );
}
