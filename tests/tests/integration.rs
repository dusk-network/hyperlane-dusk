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
use dusk_vm::{CallReceipt, Error as VMError};
use rand::rngs::StdRng;
use rand::SeedableRng;

use hyperlane_dusk_types::{message, DomainGasConfig, EthAddress, H256, MessageId, VERSION};

mod test_session;
use test_session::{assert_contract_panic, TestSession};

fn assert_contract_panic_contains<R>(
    call_result: Result<CallReceipt<R>, ContractError>,
    expected_panic_part: &str,
) where
    R: rkyv::Archive,
    R::Archived: rkyv::Deserialize<R, rkyv::Infallible>
        + for<'b> rkyv::bytecheck::CheckBytes<
            rkyv::validation::validators::DefaultValidator<'b>,
        >,
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
const TEST_MOCK_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_test_mock.wasm"
);
const TEST_RECIPIENT_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_test_recipient.wasm"
);
const PROTOCOL_FEE_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_protocol_fee.wasm"
);
const IGP_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_igp.wasm"
);
const ISM_MULTISIG_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_ism_multisig.wasm"
);

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

const DEPLOYER: [u8; 64] = [0u8; 64];
const INITIAL_DUSK_BALANCE: u64 = dusk(1_000.0);

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

static OWNER_PK: LazyLock<AccountPublicKey> =
    LazyLock::new(|| AccountPublicKey::from(&*OWNER_SK));

static RELAYER_SK: LazyLock<AccountSecretKey> = LazyLock::new(|| {
    let mut rng = StdRng::seed_from_u64(0xDE1A7E00); // "DELAYE"
    AccountSecretKey::random(&mut rng)
});

static RELAYER_PK: LazyLock<AccountPublicKey> =
    LazyLock::new(|| AccountPublicKey::from(&*RELAYER_SK));

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
                        MAILBOX_ID,          // owner = mailbox itself
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
        self.session.direct_call::<_, ()>(
            MAILBOX_ID,
            "process",
            &(metadata, encoded_message),
        )
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
        &(LOCAL_DOMAIN, MAILBOX_ID, TEST_MOCK_ID, TEST_MOCK_ID, MERKLE_TREE_HOOK_ID),
    );

    // The VM prevents calling init after deployment
    assert!(result.is_err(), "Calling init after deployment should fail");
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

    let result = s.session.direct_call::<_, ()>(
        MERKLE_TREE_HOOK_ID,
        "init",
        &(MAILBOX_ID,),
    );

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
        0,              // nonce
        REMOTE_DOMAIN,  // origin
        sender,
        LOCAL_DOMAIN,   // destination
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
    assert!(
        !process_events.is_empty(),
        "process should emit events"
    );
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
    assert_eq!(s.merkle_count(), 0);

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
    assert_ne!(msg.sender, [0u8; 32], "Sender should be non-zero");

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
fn test_multisig_ism_init_rejects_invalid_threshold() {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);

    let result = session.deploy(
        ISM_MULTISIG_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(MAILBOX_ID.to_bytes(), vec![EthAddress([1; 20])], 2u8))
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
                MAILBOX_ID.to_bytes(),
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
            .init_arg(&(MAILBOX_ID.to_bytes(), Vec::<EthAddress>::new(), 1u8))
            .contract_id(ISM_MULTISIG_ID),
    );

    assert_deploy_panic(result, "MultisigISM: no validators");
}

#[test]
fn test_multisig_ism_verify_rejects_short_metadata() {
    let mut session = session_with_multisig_ism(
        MAILBOX_ID.to_bytes(),
        vec![EthAddress([1; 20])],
        1,
    );

    let result = session.direct_call::<_, bool>(
        ISM_MULTISIG_ID,
        "verify",
        &(vec![0u8; 67], sample_encoded_message(TEST_RECIPIENT_ID)),
    );

    assert_contract_panic(result, "MultisigISM: metadata too short");
}

#[test]
fn test_multisig_ism_verify_rejects_partial_signature_bytes() {
    let mut session = session_with_multisig_ism(
        MAILBOX_ID.to_bytes(),
        vec![EthAddress([1; 20])],
        1,
    );

    let result = session.direct_call::<_, bool>(
        ISM_MULTISIG_ID,
        "verify",
        &(vec![0u8; 69], sample_encoded_message(TEST_RECIPIENT_ID)),
    );

    assert_contract_panic(result, "MultisigISM: metadata signature length mismatch");
}

#[test]
fn test_multisig_ism_verify_rejects_insufficient_signatures() {
    let mut session = session_with_multisig_ism(
        MAILBOX_ID.to_bytes(),
        vec![EthAddress([1; 20])],
        1,
    );

    let result = session.direct_call::<_, bool>(
        ISM_MULTISIG_ID,
        "verify",
        &(vec![0u8; 68], sample_encoded_message(TEST_RECIPIENT_ID)),
    );

    assert_contract_panic(result, "MultisigISM: not enough signatures");
}

#[test]
fn test_multisig_ism_verify_rejects_corrupt_signature_bytes() {
    let mut session = session_with_multisig_ism(
        MAILBOX_ID.to_bytes(),
        vec![EthAddress([1; 20])],
        1,
    );

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
    let mut session = session_with_multisig_ism(
        MAILBOX_ID.to_bytes(),
        vec![EthAddress([1; 20])],
        1,
    );

    let result = session.call_public::<_, ()>(
        &RELAYER_SK,
        ISM_MULTISIG_ID,
        "set_validators_and_threshold",
        &(vec![EthAddress([2; 20])], 1u8),
    );

    assert_contract_panic(result, "MultisigISM: caller is not owner");
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
        .direct_call::<_, ContractId>(
            TEST_RECIPIENT_ID,
            "interchain_security_module",
            &(),
        )
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
        .direct_call::<_, ContractId>(
            TEST_RECIPIENT_ID,
            "interchain_security_module",
            &(),
        )
        .expect("query should succeed")
        .data;
    assert_eq!(ism, TEST_MOCK_ID);
}

// =============================================================================
// Tests: ProtocolFee hook
// =============================================================================

/// Session with production hooks: ProtocolFee as required_hook, IGP as default_hook.
fn session_with_hooks() -> TestSession {
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
                .init_arg(&(MAILBOX_ID,))
                .contract_id(MERKLE_TREE_HOOK_ID),
        )
        .expect("Deploying MerkleTreeHook should succeed");

    // Deploy ProtocolFee: fee=1000 LUX, max=10000 LUX
    session
        .deploy(
            PROTOCOL_FEE_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(1000u64, 10000u64, MERKLE_TREE_HOOK_ID, MAILBOX_ID))
                .contract_id(PROTOCOL_FEE_ID),
        )
        .expect("Deploying ProtocolFee should succeed");

    // Deploy IGP: owner=MAILBOX_ID, beneficiary=MERKLE_TREE_HOOK_ID, no initial configs
    let no_configs: Vec<(u32, DomainGasConfig)> = Vec::new();
    session
        .deploy(
            IGP_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID, MERKLE_TREE_HOOK_ID, no_configs))
                .contract_id(IGP_ID),
        )
        .expect("Deploying IGP should succeed");

    // Deploy Mailbox with ProtocolFee as required_hook, IGP as default_hook
    session
        .deploy(
            MAILBOX_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    LOCAL_DOMAIN,
                    MAILBOX_ID,      // owner
                    TEST_MOCK_ID,    // default ISM
                    IGP_ID,          // default hook = IGP
                    PROTOCOL_FEE_ID, // required hook = ProtocolFee
                ))
                .contract_id(MAILBOX_ID),
        )
        .expect("Deploying Mailbox should succeed");

    session
}

/// Session with production hooks, including IGP with pre-configured gas configs.
fn session_with_hooks_and_igp_config(igp_configs: Vec<(u32, DomainGasConfig)>) -> TestSession {
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
                .init_arg(&(MAILBOX_ID,))
                .contract_id(MERKLE_TREE_HOOK_ID),
        )
        .expect("Deploying MerkleTreeHook should succeed");

    // Deploy ProtocolFee: fee=1000 LUX, max=10000 LUX
    session
        .deploy(
            PROTOCOL_FEE_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(1000u64, 10000u64, MERKLE_TREE_HOOK_ID, MAILBOX_ID))
                .contract_id(PROTOCOL_FEE_ID),
        )
        .expect("Deploying ProtocolFee should succeed");

    // Deploy IGP with provided gas configs
    session
        .deploy(
            IGP_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID, MERKLE_TREE_HOOK_ID, igp_configs))
                .contract_id(IGP_ID),
        )
        .expect("Deploying IGP should succeed");

    // Deploy Mailbox with ProtocolFee as required_hook, IGP as default_hook
    session
        .deploy(
            MAILBOX_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    LOCAL_DOMAIN,
                    MAILBOX_ID,      // owner
                    TEST_MOCK_ID,    // default ISM
                    IGP_ID,          // default hook = IGP
                    PROTOCOL_FEE_ID, // required hook = ProtocolFee
                ))
                .contract_id(MAILBOX_ID),
        )
        .expect("Deploying Mailbox should succeed");

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
                .init_arg(&(500u64, 5000u64, MAILBOX_ID, TEST_MOCK_ID))
                .contract_id(PROTOCOL_FEE_ID),
        )
        .expect("Deploying ProtocolFee should succeed");

    // Verify state
    let quote: u64 = s.session
        .direct_call::<_, u64>(
            PROTOCOL_FEE_ID,
            "quote_dispatch",
            &(Vec::<u8>::new(), Vec::<u8>::new()),
        )
        .expect("quote_dispatch should succeed")
        .data;
    assert_eq!(quote, 500);

    let collected: u64 = s.session
        .direct_call::<_, u64>(PROTOCOL_FEE_ID, "collected_fees", &())
        .expect("collected_fees should succeed")
        .data;
    assert_eq!(collected, 0);

    let hook_type: u8 = s.session
        .direct_call::<_, u8>(PROTOCOL_FEE_ID, "hook_type", &())
        .expect("hook_type should succeed")
        .data;
    assert_eq!(hook_type, 6); // HookType::ProtocolFee
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

    // Dispatch a message via TestRecipient proxy
    session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            TEST_RECIPIENT_ID,
            "dispatch_message",
            &(MAILBOX_ID, REMOTE_DOMAIN, [0xBBu8; 32], b"testing protocol fee".to_vec()),
        )
        .expect("dispatch should succeed");

    // ProtocolFee should have recorded 1000 LUX
    let collected: u64 = session
        .direct_call::<_, u64>(PROTOCOL_FEE_ID, "collected_fees", &())
        .expect("collected_fees should succeed")
        .data;
    assert_eq!(collected, 1000);
}

#[test]
fn test_protocol_fee_init_rejects_fee_above_max() {
    let mut s = HyperlaneSession::new();

    // Try to deploy with fee > max — should panic during init
    let result = s.session.deploy(
        PROTOCOL_FEE_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(20000u64, 10000u64, MAILBOX_ID, TEST_MOCK_ID))
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
                .init_arg(&(2500u64, 5000u64, MAILBOX_ID, TEST_MOCK_ID))
                .contract_id(PROTOCOL_FEE_ID),
        )
        .expect("Deploying ProtocolFee should succeed");

    let quote: u64 = s.session
        .direct_call::<_, u64>(
            PROTOCOL_FEE_ID,
            "quote_dispatch",
            &(Vec::<u8>::new(), Vec::<u8>::new()),
        )
        .expect("quote_dispatch should succeed")
        .data;
    assert_eq!(quote, 2500);

    let max: u64 = s.session
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

    let total: u64 = session
        .direct_call::<_, u64>(IGP_ID, "total_gas_payments", &())
        .expect("total_gas_payments should succeed")
        .data;
    assert_eq!(total, 0);
}

#[test]
fn test_igp_quote_no_config() {
    let mut session = session_with_hooks();

    // No gas config set for domain 42 → should return 0
    let metadata = 100_000u64.to_le_bytes().to_vec();
    let encoded = message::encode(VERSION, 0, LOCAL_DOMAIN, [0u8; 32], 42, [0u8; 32], &[]);
    let quote: u64 = session
        .direct_call::<_, u64>(IGP_ID, "quote_dispatch", &(metadata, encoded))
        .expect("quote_dispatch should succeed")
        .data;
    assert_eq!(quote, 0);
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
    let encoded = message::encode(VERSION, 0, LOCAL_DOMAIN, [0u8; 32], REMOTE_DOMAIN, [0u8; 32], &[]);
    let quote: u64 = session
        .direct_call::<_, u64>(IGP_ID, "quote_dispatch", &(metadata, encoded))
        .expect("quote_dispatch should succeed")
        .data;

    assert_eq!(quote, 3_000_000);
}

#[test]
fn test_igp_records_payment_on_dispatch() {
    // Deploy IGP with simple gas config: 1 wei gas price, 1:1 exchange, no overhead
    let mut session = session_with_hooks_and_igp_config(vec![(
        REMOTE_DOMAIN,
        DomainGasConfig {
            gas_overhead: 0,
            token_exchange_rate: 10_000_000_000u64, // 1:1
            gas_price: 1u64,                         // 1 wei
        },
    )]);

    // Dispatch a message (default hook = IGP)
    session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            TEST_RECIPIENT_ID,
            "dispatch_message",
            &(MAILBOX_ID, REMOTE_DOMAIN, [0xBBu8; 32], b"testing igp".to_vec()),
        )
        .expect("dispatch should succeed");

    // With empty metadata (from dispatch_default), gas_limit defaults to 50_000
    // cost = 50_000 * 1 * 10_000_000_000 / 10_000_000_000 = 50_000
    let total: u64 = session
        .direct_call::<_, u64>(IGP_ID, "total_gas_payments", &())
        .expect("total_gas_payments should succeed")
        .data;
    assert_eq!(total, 50_000);
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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
#[archive_attr(derive(rkyv::bytecheck::CheckBytes))]
enum Drc20Account {
    External(AccountPublicKey),
    Contract(ContractId),
}

fn warp_drc20_balance_of(session: &mut TestSession, account: Drc20Account) -> u64 {
    session
        .direct_call::<_, u64>(WARP_DRC20_ID, "balance_of", &(account,))
        .expect("balance_of should succeed")
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
                    message::keccak256(&OWNER_PK.to_bytes()), // owner = keccak256(deployer BLS key)
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
                    MAILBOX_ID,          // owner
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

    let result = session.call_public::<_, ()>(
        &RELAYER_SK,
        WARP_DRC20_ID,
        "set_ism",
        &(TEST_MOCK_ID,),
    );

    assert_contract_panic(result, "WarpDrc20: caller is not the owner");
}

// =============================================================================
// Tests: WarpNative
// =============================================================================

#[test]
fn test_warp_native_init() {
    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, INITIAL_DUSK_BALANCE),
    ]);

    session
        .deploy(
            WARP_NATIVE_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID, MAILBOX_ID, Vec::<(u32, H256)>::new()))
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
}

#[test]
fn test_warp_native_register_account() {
    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, INITIAL_DUSK_BALANCE),
    ]);

    session
        .deploy(
            WARP_NATIVE_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID, MAILBOX_ID, Vec::<(u32, H256)>::new()))
                .contract_id(WARP_NATIVE_ID),
        )
        .expect("Deploying WarpNative should succeed");

    // Register OWNER_PK via Moonlight TX (so abi::public_sender() is set)
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_NATIVE_ID,
            "register_account",
            &(),
        )
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
    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, INITIAL_DUSK_BALANCE),
    ]);

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
                .init_arg(&(TEST_MOCK_ID, MAILBOX_ID, MAILBOX_ID, Vec::<(u32, H256)>::new()))
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
}

#[test]
fn test_warp_collateral_rejects_zero_token() {
    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, INITIAL_DUSK_BALANCE),
    ]);

    let result = session.deploy(
        WARP_DRC20_COLLATERAL_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&(
                ContractId::from_bytes([0u8; 32]),
                MAILBOX_ID,
                MAILBOX_ID,
                Vec::<(u32, H256)>::new(),
            ))
            .contract_id(WARP_DRC20_COLLATERAL_ID),
    );
    assert!(result.is_err(), "init with zero wrapped_token should fail");
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
                    message::keccak256(&OWNER_PK.to_bytes()), // owner = keccak256(deployer BLS key)
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
                    MAILBOX_ID,          // owner
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
    let token_body = hyperlane_dusk_types::token_message::encode(
        TEST_RECIPIENT_ID.to_bytes(),
        mint_amount,
    );

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

    // Verify tokens were minted
    let supply: u64 = session
        .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
        .expect("total_supply should succeed")
        .data;
    assert_eq!(supply, mint_amount);
}

#[test]
fn test_warp_drc20_handle_multiple_mints() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();

    // Process three inbound transfers
    for i in 0u32..3 {
        let amount = (i as u64 + 1) * 500_000;
        let token_body = hyperlane_dusk_types::token_message::encode(
            TEST_RECIPIENT_ID.to_bytes(),
            amount,
        );

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

    // Total minted: 500_000 + 1_000_000 + 1_500_000 = 3_000_000
    let supply: u64 = session
        .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
        .expect("total_supply should succeed")
        .data;
    assert_eq!(supply, 3_000_000);
}

#[test]
fn test_warp_drc20_handle_rejects_unenrolled_sender() {
    let (mut session, _remote_router) = session_with_warp_drc20_flow();

    // Build inbound message with wrong sender (not the enrolled router)
    let wrong_sender = [0xBBu8; 32];
    let token_body = hyperlane_dusk_types::token_message::encode(
        TEST_RECIPIENT_ID.to_bytes(),
        1000,
    );

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN, // origin is enrolled, but sender is wrong
        wrong_sender,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), encoded),
    );
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpDrc20: sender is not enrolled router for origin\")",
    );
}

#[test]
fn test_warp_drc20_handle_rejects_unenrolled_origin() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();

    // Build inbound message from unknown origin domain (42)
    let token_body = hyperlane_dusk_types::token_message::encode(
        TEST_RECIPIENT_ID.to_bytes(),
        1000,
    );

    let encoded = message::encode(
        VERSION,
        0,
        42, // unknown origin domain
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), encoded),
    );
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

    let result = session.direct_call::<_, ()>(
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), encoded),
    );
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpDrc20: invalid token message\")",
    );
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
    let token_body = hyperlane_dusk_types::token_message::encode(
        recipient_h256,
        mint_amount,
    );

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &token_body,
    );

    // Process — should mint to Account::External(OWNER_PK)
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
                .init_arg(&(MAILBOX_ID, MAILBOX_ID, vec![(REMOTE_DOMAIN, remote_router)]))
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
                    MAILBOX_ID,
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

    let token_body = hyperlane_dusk_types::token_message::encode(
        recipient_h256,
        1000,
    );

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        wrong_sender, // not the enrolled router
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), encoded),
    );
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

    let token_body = hyperlane_dusk_types::token_message::encode(
        recipient_h256,
        1000,
    );

    let encoded = message::encode(
        VERSION,
        0,
        42, // unknown origin domain
        remote_router,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), encoded),
    );
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpNative: sender is not enrolled router for origin\")",
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
                    MAILBOX_ID,
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
                    WARP_DRC20_ID,  // wrapped token = WarpDrc20
                    MAILBOX_ID,
                    MAILBOX_ID,     // owner
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
                    MAILBOX_ID,
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
fn test_warp_collateral_handle_rejects_unenrolled_sender() {
    let (mut session, _remote_router) = session_with_warp_collateral_flow();

    let wrong_sender = [0xDDu8; 32];
    let token_body = hyperlane_dusk_types::token_message::encode(
        TEST_RECIPIENT_ID.to_bytes(),
        1000,
    );

    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        wrong_sender,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), encoded),
    );
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpCollateral: sender is not enrolled router for origin\")",
    );
}

#[test]
fn test_warp_collateral_handle_rejects_unenrolled_origin() {
    let (mut session, remote_router) = session_with_warp_collateral_flow();

    let token_body = hyperlane_dusk_types::token_message::encode(
        TEST_RECIPIENT_ID.to_bytes(),
        1000,
    );

    let encoded = message::encode(
        VERSION,
        0,
        42, // unknown origin
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &token_body,
    );

    let result = session.direct_call::<_, ()>(
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), encoded),
    );
    assert_contract_panic(
        result,
        "Mailbox: recipient handle failed: Panic(\"WarpCollateral: sender is not enrolled router for origin\")",
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

// --- WarpNative: escrow for unregistered recipients ---

#[test]
fn test_warp_native_handle_escrows_unregistered_recipient() {
    let (mut session, remote_router) = session_with_warp_native_flow();

    // Inbound message with a recipient H256 that is NOT registered
    let unregistered: H256 = [0xEE; 32];
    let amount = 1_000_000u64;
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

    // Process two inbound messages to the same unregistered recipient
    for nonce in 0u32..2 {
        let amount = (u64::from(nonce) + 1) * 500_000;
        let token_body =
            hyperlane_dusk_types::token_message::encode(unregistered, amount);

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
fn test_warp_native_claim_pending_requires_pending() {
    let (mut session, _remote_router) = session_with_warp_native_flow();

    // Register the account first
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "register_account", &())
        .expect("register_account should succeed");

    // Try to claim with no pending transfers
    let result =
        session.call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "claim_pending", &());
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
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_COLLATERAL_ID,
            "register_account",
            &(),
        )
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
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_COLLATERAL_ID,
            "register_account",
            &(),
        )
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

    let result = session.direct_call::<_, ()>(
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), encoded),
    );
    assert_contract_panic_contains(
        result,
        "WarpDrc20: insufficient balance",
    );
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
                    MAILBOX_ID.to_bytes(),
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
                    MAILBOX_ID,
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
                    MAILBOX_ID,
                    TEST_MOCK_ID,
                    TEST_MOCK_ID,
                    MERKLE_TREE_HOOK_ID,
                ))
                .contract_id(MAILBOX_ID),
        )
        .expect("Deploying Mailbox should succeed");

    // Pre-fund: mint DRC20 tokens to Account::Contract(WARP_DRC20_COLLATERAL_ID)
    // by processing an inbound message to WarpDrc20
    let fund_amount = 10_000_000u64;
    let token_body = hyperlane_dusk_types::token_message::encode(
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        fund_amount,
    );
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
        .expect("Pre-funding collateral should succeed");

    (session, remote_router)
}

#[test]
fn test_warp_collateral_handle_resolves_registered_external() {
    let (mut session, remote_router) = session_with_warp_collateral_funded_flow();

    // Register OWNER_PK on collateral contract
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_COLLATERAL_ID,
            "register_account",
            &(),
        )
        .expect("register_account should succeed");

    // Inbound message to collateral: recipient = keccak256(OWNER_PK)
    let pk_bytes = OWNER_PK.to_bytes();
    let recipient_h256 = message::keccak256(&pk_bytes);

    let unlock_amount = 500_000u64;
    let token_body =
        hyperlane_dusk_types::token_message::encode(recipient_h256, unlock_amount);

    let encoded = message::encode(
        VERSION,
        1, // nonce=1 (nonce=0 used for pre-funding)
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &token_body,
    );

    // Process — collateral resolves to Account::External(OWNER_PK)
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
        .direct_call::<_, u64>(WARP_DRC20_COLLATERAL_ID, "pending_balance", &(unregistered,))
        .expect("pending_balance should succeed")
        .data;
    assert_eq!(pending, unlock_amount);

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
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_COLLATERAL_ID,
            "register_account",
            &(),
        )
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

    let external_balance =
        warp_drc20_balance_of(&mut session, Drc20Account::External(*OWNER_PK));
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
