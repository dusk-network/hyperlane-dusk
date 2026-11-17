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
    events, message, token_message, DomainGasConfig, EthAddress, GasPaymentRecord, MessageId, H256,
    VERSION,
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
const REENTRANT_HOOK_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_reentrant_hook.wasm"
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
const REENTRANT_HOOK_ID: ContractId = ContractId::from_bytes([24; 32]);

const DEPLOYER: [u8; 64] = [0u8; 64];
const INITIAL_DUSK_BALANCE: u64 = dusk(1_000.0);
/// `dusk-tx withdraw-dispatch`'s default gas limit: the Mailbox decodes the
/// recipient key, about 57M gas.
const WITHDRAW_DEFAULT_GAS_LIMIT: u64 = 80_000_000;

/// The least dispatch credit the Mailbox takes for a payer with none, 0.01
/// DUSK (`MIN_NEW_PAYER_CREDIT`).
const MIN_NEW_PAYER_CREDIT: u64 = 10_000_000;

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
        Self::with_mailbox(MAILBOX_BYTECODE)
    }

    /// As [`Self::new`], with the Mailbox deployed from `mailbox_bytecode`.
    fn with_mailbox(mailbox_bytecode: &[u8]) -> Self {
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
                mailbox_bytecode,
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

    fn mailbox_dispatched_id(&mut self, nonce: u32) -> MessageId {
        self.session
            .direct_call::<_, MessageId>(MAILBOX_ID, "dispatched_id", &(nonce,))
            .expect("dispatched_id should succeed")
            .data
    }

    /// The message a dispatch receipt's `Dispatch` event carries for `nonce`,
    /// checked against the ID the Mailbox stored for that nonce.
    fn dispatched_message<R>(&mut self, receipt: &CallReceipt<R>, nonce: u32) -> Vec<u8> {
        let encoded = receipt
            .events
            .iter()
            .filter(|e| e.source == MAILBOX_ID && e.topic == events::Dispatch::TOPIC)
            .map(|e| test_session::rkyv_deserialize::<events::Dispatch>(&e.data).message)
            .find(|m| message::decode(m).is_some_and(|d| d.nonce == nonce))
            .expect("the receipt should carry the Dispatch event");
        assert_eq!(message::id(&encoded), self.mailbox_dispatched_id(nonce));
        encoded
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

fn state_version(session: &mut TestSession, contract: ContractId) -> u32 {
    session
        .direct_call::<_, u32>(contract, "state_version", &())
        .expect("state_version should succeed")
        .data
}

/// The message IDs of the deliveries a receipt records: its `ProcessId`
/// events from the Mailbox that weren't rolled back. Agents index deliveries
/// from these events; the Mailbox keeps no list of them.
fn delivered_ids<R>(receipt: &CallReceipt<R>) -> Vec<MessageId> {
    receipt
        .events
        .iter()
        .filter(|e| e.source == MAILBOX_ID && e.topic == events::ProcessId::TOPIC && !e.reverted)
        .map(|e| test_session::rkyv_deserialize::<events::ProcessId>(&e.data).message_id)
        .collect()
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

    for (contract, label, expected) in [
        (MAILBOX_ID, "Mailbox", 6),
        (TEST_MOCK_ID, "TestMock", 1),
        (TEST_RECIPIENT_ID, "TestRecipient", 1),
    ] {
        let version = s
            .session
            .direct_call::<_, u32>(contract, "state_version", &())
            .unwrap_or_else(|_| panic!("{label} state_version should succeed"))
            .data;
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
fn test_process_rejects_replay_during_recipient_ism_resolution() {
    let mut s = HyperlaneSession::new();
    s.session
        .deploy(
            REENTRANT_HOOK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID,))
                .contract_id(REENTRANT_HOOK_ID),
        )
        .unwrap();
    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        LOCAL_DOMAIN,
        REENTRANT_HOOK_ID.to_bytes(),
        b"deliver only once even when resolving the recipient ISM",
    );
    s.session
        .direct_call::<_, ()>(REENTRANT_HOOK_ID, "configure_inbound", &(encoded.clone(),))
        .unwrap();
    let receipt = s
        .mailbox_process_via_tx(Vec::new(), encoded.clone())
        .unwrap();
    assert!(
        s.session
            .direct_call::<_, bool>(REENTRANT_HOOK_ID, "attempted", &())
            .unwrap()
            .data
    );
    assert_eq!(
        s.session
            .direct_call::<_, u32>(REENTRANT_HOOK_ID, "handled_count", &())
            .unwrap()
            .data,
        1,
        "recipient ISM resolution must not permit a second delivery of the same message"
    );
    assert!(
        !s.session
            .direct_call::<_, bool>(REENTRANT_HOOK_ID, "nested_succeeded", &())
            .unwrap()
            .data
    );
    assert!(s.mailbox_delivered(message::id(&encoded)));
    assert_eq!(delivered_ids(&receipt), vec![message::id(&encoded)]);
    assert_eq!(
        receipt
            .events
            .iter()
            .filter(|event| event.source == MAILBOX_ID && event.topic == events::ProcessId::TOPIC)
            .count(),
        1
    );
}

#[test]
fn test_process_rejected_ism_rolls_back_delivery_reservation() {
    let mut s = HyperlaneSession::new();
    s.session
        .deploy(
            ISM_MULTISIG_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(*OWNER_ID, vec![EthAddress([0x11; 20])], 1u8))
                .contract_id(ISM_MULTISIG_ID),
        )
        .unwrap();
    s.session
        .call_public::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "set_default_ism",
            &(ISM_MULTISIG_ID,),
        )
        .unwrap();
    let encoded = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        LOCAL_DOMAIN,
        TEST_RECIPIENT_ID.to_bytes(),
        b"retry after failed verification",
    );
    let result = s.mailbox_process_via_tx(Vec::new(), encoded.clone());
    assert_contract_panic(
        result,
        r#"Mailbox: ISM call failed: Panic("MultisigISM: metadata too short")"#,
    );
    assert!(!s.mailbox_delivered(message::id(&encoded)));
    assert_eq!(s.recipient_handled_count(), 0);
    s.session
        .call_public::<_, ()>(&OWNER_SK, MAILBOX_ID, "set_default_ism", &(TEST_MOCK_ID,))
        .unwrap();
    let receipt = s
        .mailbox_process_via_tx(Vec::new(), encoded.clone())
        .unwrap();
    assert!(s.mailbox_delivered(message::id(&encoded)));
    assert_eq!(delivered_ids(&receipt), vec![message::id(&encoded)]);
    assert_eq!(s.recipient_handled_count(), 1);
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
// Tests: Delivery indexing from events
// =============================================================================

/// The Mailbox keeps no list of deliveries (`processed_count`,
/// `processed_at_index` and `delivered_at` are gone). Each delivery emits one
/// `Process` event, with the origin, sender and recipient, and one
/// `ProcessId` event with the message ID; agents index those.
#[test]
fn test_process_events_index_deliveries() {
    let mut s = HyperlaneSession::new();
    for name in [
        "processed_count",
        "processed_at_index",
        "processed_block_height_at_index",
        "delivered_at",
    ] {
        let result = s.session.direct_call::<_, u32>(MAILBOX_ID, name, &(0u32,));
        assert!(result.is_err(), "{name} should no longer exist");
    }

    for i in 0u32..3 {
        let sender = [0xA0 + i as u8; 32];
        let encoded = message::encode(
            VERSION,
            i,
            REMOTE_DOMAIN,
            sender,
            LOCAL_DOMAIN,
            TEST_RECIPIENT_ID.to_bytes(),
            format!("message {i}").as_bytes(),
        );
        let id = message::id(&encoded);
        assert!(!s.mailbox_delivered(id));
        let receipt = s
            .mailbox_process_via_tx(Vec::new(), encoded)
            .expect("process should succeed");
        assert!(s.mailbox_delivered(id));
        assert_eq!(delivered_ids(&receipt), vec![id]);
        let process: Vec<events::Process> = receipt
            .events
            .iter()
            .filter(|e| e.source == MAILBOX_ID && e.topic == events::Process::TOPIC)
            .map(|e| test_session::rkyv_deserialize(&e.data))
            .collect();
        assert_eq!(
            process,
            vec![events::Process {
                origin: REMOTE_DOMAIN,
                sender,
                recipient: TEST_RECIPIENT_ID.to_bytes(),
            }]
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
        6
    );
    assert_eq!(s.merkle_count(), 0);
    assert_eq!(
        s.session
            .direct_call::<_, u32>(MERKLE_TREE_HOOK_ID, "state_version", &())
            .expect("state_version should succeed")
            .data,
        2
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
    // The hook no longer stores a root per insertion; validators compute it
    // from the stored message IDs.
    assert!(s
        .session
        .direct_call::<_, H256>(MERKLE_TREE_HOOK_ID, "root_at", &(0u32,))
        .is_err());
    let mut tree = hyperlane_dusk_types::merkle::IncrementalMerkle::new();
    tree.insert(message_id);
    assert_eq!(s.merkle_root(), tree.root());
    assert_eq!(
        s.session
            .direct_call::<_, Vec<H256>>(MERKLE_TREE_HOOK_ID, "message_ids", &(0u32, 256u32))
            .expect("message_ids should succeed")
            .data,
        vec![message_id]
    );

    // The Dispatch event carries the message, and the Mailbox stores its ID
    let encoded = s.dispatched_message(&receipt, 0);
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
    let encoded = s.dispatched_message(&receipt, 0);
    let msg = message::decode(&encoded).expect("decode should succeed");
    assert_eq!(msg.sender, TEST_RECIPIENT_ID.to_bytes());
    assert_eq!(msg.body, body);
}

#[test]
fn test_dispatch_rejects_quote_hook_reentrancy_without_corrupting_nonce_order() {
    assert_dispatch_reentry_rejected(0, 0);
}

#[test]
fn test_dispatch_rejects_post_hook_reentrancy_without_corrupting_nonce_order() {
    assert_dispatch_reentry_rejected(1, 0);
}

#[test]
fn test_dispatch_rejects_payment_reentrancy_without_corrupting_nonce_order() {
    assert_dispatch_reentry_rejected(2, MIN_NEW_PAYER_CREDIT);
}

fn assert_dispatch_reentry_rejected(stage: u8, fee: u64) {
    let mut s = HyperlaneSession::new();
    s.session
        .deploy(
            REENTRANT_HOOK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID,))
                .contract_id(REENTRANT_HOOK_ID),
        )
        .expect("Deploying ReentrantHook should succeed");
    s.session
        .direct_call::<_, ()>(REENTRANT_HOOK_ID, "configure", &(stage, fee, false))
        .expect("hook configuration should succeed");
    if fee > 0 {
        s.session
            .call_public_with_deposit::<_, ()>(
                &OWNER_SK,
                MAILBOX_ID,
                "fund_dispatch",
                &(*OWNER_ID, fee),
                fee,
            )
            .expect("outer fee funding should succeed");
    }

    let destination = REMOTE_DOMAIN;
    let recipient = [0xD5u8; 32];
    let body = b"outer dispatch survives rejected reentry".to_vec();
    let receipt = s
        .session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            MAILBOX_ID,
            "dispatch",
            &(
                destination,
                recipient,
                body.clone(),
                Vec::<u8>::new(),
                REENTRANT_HOOK_ID,
            ),
        )
        .expect("outer dispatch should succeed");

    assert!(
        s.session
            .direct_call::<_, bool>(REENTRANT_HOOK_ID, "attempted", &())
            .expect("attempted query should succeed")
            .data
    );
    assert!(
        !s.session
            .direct_call::<_, bool>(REENTRANT_HOOK_ID, "nested_succeeded", &())
            .expect("nested_succeeded query should succeed")
            .data,
        "nested dispatch must be rejected"
    );
    assert_eq!(
        s.session
            .direct_call::<_, u32>(REENTRANT_HOOK_ID, "post_dispatch_count", &())
            .expect("post_dispatch_count query should succeed")
            .data,
        1
    );

    assert_eq!(
        s.session.contract_balance(&REENTRANT_HOOK_ID).unwrap(),
        fee,
        "only the outer dispatch's quoted fee may leave Mailbox custody"
    );
    assert_eq!(s.session.contract_balance(&MAILBOX_ID).unwrap(), 0);
    assert_eq!(
        s.session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(*OWNER_ID,))
            .unwrap()
            .data,
        0
    );
    assert_eq!(s.mailbox_nonce(), 1);
    let encoded = s.dispatched_message(&receipt, 0);
    let decoded = message::decode(&encoded).expect("stored message should decode");
    assert_eq!(decoded.nonce, 0);
    assert_eq!(decoded.destination, destination);
    assert_eq!(decoded.recipient, recipient);
    assert_eq!(decoded.body, body);
    assert_eq!(receipt.data, message::id(&encoded));
    assert_eq!(s.mailbox_latest_dispatched_id(), receipt.data);

    let dispatch_topics: Vec<&str> = receipt
        .events
        .iter()
        .filter(|event| {
            event.source == MAILBOX_ID
                && (event.topic == events::Dispatch::TOPIC
                    || event.topic == events::DispatchId::TOPIC)
        })
        .map(|event| event.topic.as_str())
        .collect();
    assert_eq!(
        dispatch_topics,
        vec![events::Dispatch::TOPIC, events::DispatchId::TOPIC]
    );

    let control_body = b"guard cleared for later dispatch".to_vec();
    let control_receipt = s
        .mailbox_dispatch_via_tx(&OWNER_SK, destination, recipient, control_body.clone())
        .expect("later legitimate dispatch should succeed");
    assert_eq!(s.mailbox_nonce(), 2);
    let control = message::decode(&s.dispatched_message(&control_receipt, 1))
        .expect("later stored message should decode");
    assert_eq!(control.nonce, 1);
    assert_eq!(control.body, control_body);
}

#[test]
fn test_failed_post_dispatch_rolls_back_nonce_credit_custody_and_guard() {
    let mut s = HyperlaneSession::new();
    s.session
        .deploy(
            REENTRANT_HOOK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID,))
                .contract_id(REENTRANT_HOOK_ID),
        )
        .unwrap();
    let fee = MIN_NEW_PAYER_CREDIT;
    s.session
        .direct_call::<_, ()>(REENTRANT_HOOK_ID, "configure", &(1u8, fee, true))
        .unwrap();
    s.session
        .call_public_with_deposit::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(*OWNER_ID, fee),
            fee,
        )
        .unwrap();
    let args = (
        REMOTE_DOMAIN,
        [0xD5u8; 32],
        b"retry after rejected callback".to_vec(),
        Vec::<u8>::new(),
        REENTRANT_HOOK_ID,
    );
    let result = s
        .session
        .call_public::<_, MessageId>(&OWNER_SK, MAILBOX_ID, "dispatch", &args);
    assert_contract_panic(
        result,
        r#"Mailbox: hook post_dispatch failed: Panic("ReentrantHook: rejected callback")"#,
    );
    assert_eq!(s.mailbox_nonce(), 0);
    assert_eq!(s.mailbox_latest_dispatched_id(), [0u8; 32]);
    assert_eq!(s.session.contract_balance(&MAILBOX_ID).unwrap(), fee);
    assert_eq!(s.session.contract_balance(&REENTRANT_HOOK_ID).unwrap(), 0);
    assert_eq!(
        s.session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(*OWNER_ID,))
            .unwrap()
            .data,
        fee
    );
    s.session
        .direct_call::<_, ()>(REENTRANT_HOOK_ID, "configure", &(1u8, fee, false))
        .unwrap();
    let receipt = s
        .session
        .call_public::<_, MessageId>(&OWNER_SK, MAILBOX_ID, "dispatch", &args)
        .expect("retry must succeed after the entire failed callback rolls back");
    assert_eq!(s.mailbox_nonce(), 1);
    assert_eq!(s.mailbox_dispatched_id(0), receipt.data);
    assert_eq!(s.session.contract_balance(&MAILBOX_ID).unwrap(), 0);
    assert_eq!(s.session.contract_balance(&REENTRANT_HOOK_ID).unwrap(), fee);
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
    validator_announcement_with_index(location, 7)
}

fn validator_announcement_with_index(location: &str, index: u64) -> (EthAddress, Vec<u8>) {
    let secp = Secp256k1::new();
    let mut secret_bytes = [0u8; 32];
    secret_bytes[24..].copy_from_slice(&index.to_be_bytes());
    let secret = SecpSecretKey::from_byte_array(secret_bytes)
        .expect("validator test secret should be valid");
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
fn test_validator_announce_has_paginated_discovery_without_global_enrollment_cap() {
    let mut session = session_with_validator_announce();
    const ABOVE_HISTORICAL_CAP: u32 = 1_025;
    let location = alloc::string::String::from("s3://validator/checkpoints");

    for index in 1..=ABOVE_HISTORICAL_CAP {
        let (validator, signature) = validator_announcement_with_index(&location, u64::from(index));
        session
            .call_public::<_, bool>(
                &OWNER_SK,
                VALIDATOR_ANNOUNCE_ID,
                "announce",
                &(validator, location.clone(), signature),
            )
            .expect("a storage-paying validator must not be blocked by a global registry cap");
    }

    let count = session
        .direct_call::<_, u32>(VALIDATOR_ANNOUNCE_ID, "announced_validator_count", &())
        .expect("validator count query should succeed")
        .data;
    assert_eq!(count, ABOVE_HISTORICAL_CAP);

    let last_page = session
        .direct_call::<_, Vec<EthAddress>>(
            VALIDATOR_ANNOUNCE_ID,
            "get_announced_validators",
            &(ABOVE_HISTORICAL_CAP - 2, 2u32),
        )
        .expect("the final bounded page should remain discoverable")
        .data;
    assert_eq!(last_page.len(), 2);
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

/// Pages of validators hold 1 to 256. The queries that returned location
/// strings are gone: the contract only stores their hashes.
#[test]
fn test_validator_announce_pages_hold_up_to_256_validators() {
    let mut session = session_with_validator_announce();
    let location = alloc::string::String::from("s3://validator/checkpoints");
    let mut expected = Vec::new();
    for index in 1..=3 {
        let (validator, signature) = validator_announcement_with_index(&location, index);
        session
            .call_public::<_, bool>(
                &OWNER_SK,
                VALIDATOR_ANNOUNCE_ID,
                "announce",
                &(validator, location.clone(), signature),
            )
            .expect("announcement should succeed");
        expected.push(validator);
    }
    let page = |session: &mut TestSession, start: u32, limit: u32| {
        session.direct_call::<_, Vec<EthAddress>>(
            VALIDATOR_ANNOUNCE_ID,
            "get_announced_validators",
            &(start, limit),
        )
    };
    assert_eq!(page(&mut session, 0, 256).unwrap().data, expected);
    assert_eq!(page(&mut session, 1, 256).unwrap().data, expected[1..]);
    assert!(page(&mut session, 3, 256).unwrap().data.is_empty());
    assert_contract_panic(
        page(&mut session, 0, 257),
        "ValidatorAnnounce: query batch too large",
    );
    assert_contract_panic(
        page(&mut session, 0, 0),
        "ValidatorAnnounce: query limit is zero",
    );
    for name in [
        "get_announced_storage_locations",
        "get_announced_storage_locations_for_validator",
    ] {
        assert!(session
            .direct_call::<_, Vec<alloc::string::String>>(
                VALIDATOR_ANNOUNCE_ID,
                name,
                &(expected[0],),
            )
            .is_err());
    }
}

/// The location in a receipt's `ValidatorAnnouncement` event.
fn announced_location<R>(receipt: &CallReceipt<R>) -> events::ValidatorAnnouncement {
    let events: Vec<_> = receipt
        .events
        .iter()
        .filter(|e| {
            e.source == VALIDATOR_ANNOUNCE_ID && e.topic == events::ValidatorAnnouncement::TOPIC
        })
        .map(|e| test_session::rkyv_deserialize::<events::ValidatorAnnouncement>(&e.data))
        .collect();
    assert_eq!(events.len(), 1, "one announcement per call");
    events[0].clone()
}

fn announced_location_hashes(session: &mut TestSession, validator: EthAddress) -> Vec<H256> {
    session
        .direct_call::<_, Vec<H256>>(
            VALIDATOR_ANNOUNCE_ID,
            "get_announced_storage_location_hashes",
            &(validator,),
        )
        .expect("location hash query should succeed")
        .data
}

#[test]
fn test_validator_announce_bounds_signed_location_history() {
    let mut session = session_with_validator_announce();
    let mut expected = Vec::new();
    let mut validator = EthAddress([0u8; 20]);
    assert_eq!(state_version(&mut session, VALIDATOR_ANNOUNCE_ID), 3);

    for index in 0..16 {
        let location = alloc::format!("s3://validator/checkpoints/{index}");
        let (signed_validator, signature) = validator_announcement(&location);
        validator = signed_validator;
        let receipt = session
            .call_public::<_, bool>(
                &OWNER_SK,
                VALIDATOR_ANNOUNCE_ID,
                "announce",
                &(validator, location.clone(), signature),
            )
            .expect("a valid bounded validator announcement should succeed");
        // The location is only in the event; the contract keeps its hash.
        assert_eq!(
            announced_location(&receipt),
            events::ValidatorAnnouncement {
                validator,
                storage_location: location.clone(),
            }
        );
        expected.push(message::keccak256(location.as_bytes()));
    }

    assert_eq!(announced_location_hashes(&mut session, validator), expected);
    assert!(announced_location_hashes(&mut session, EthAddress([9; 20])).is_empty());

    let overflow_location = alloc::string::String::from("s3://validator/checkpoints/16");
    let (_, overflow_signature) = validator_announcement(&overflow_location);
    let result = session.call_public::<_, bool>(
        &OWNER_SK,
        VALIDATOR_ANNOUNCE_ID,
        "announce",
        &(validator, overflow_location, overflow_signature),
    );
    assert_contract_panic(result, "ValidatorAnnounce: location limit reached");

    let count = session
        .direct_call::<_, u32>(VALIDATOR_ANNOUNCE_ID, "announced_validator_count", &())
        .expect("validator registry count should succeed")
        .data;
    assert_eq!(count, 1);

    assert_eq!(announced_location_hashes(&mut session, validator), expected);

    let validators = session
        .direct_call::<_, Vec<EthAddress>>(
            VALIDATOR_ANNOUNCE_ID,
            "get_announced_validators",
            &(0u32, 2u32),
        )
        .expect("validator registry query should succeed")
        .data;
    assert_eq!(validators, vec![validator]);

    let empty = session
        .direct_call::<_, Vec<EthAddress>>(
            VALIDATOR_ANNOUNCE_ID,
            "get_announced_validators",
            &(1u32, 2u32),
        )
        .expect("an exhausted validator page should succeed")
        .data;
    assert!(empty.is_empty());
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

// Real signatures exercise threshold counting separately from malformed-byte tests.
fn checkpoint_signers() -> Vec<(EthAddress, SecpSecretKey)> {
    checkpoint_signer_set(4)
}

/// `count` validator keys, sorted by address as the ISM requires.
fn checkpoint_signer_set(count: u8) -> Vec<(EthAddress, SecpSecretKey)> {
    let secp = Secp256k1::new();
    let mut signers: Vec<_> = (1u8..=count)
        .map(|index| {
            let mut bytes = [0u8; 32];
            bytes[31] = index;
            let secret = SecpSecretKey::from_byte_array(bytes).unwrap();
            let public =
                secp256k1::PublicKey::from_secret_key(&secp, &secret).serialize_uncompressed();
            let hash = message::keccak256(&public[1..]);
            let address = EthAddress(hash[12..].try_into().unwrap());
            (address, secret)
        })
        .collect();
    signers.sort_by_key(|(address, _)| address.0);
    signers
}

fn signed_checkpoint_metadata(message: &[u8], signers: &[&SecpSecretKey]) -> Vec<u8> {
    let hook = [0x21; 32];
    let root = [0x42; 32];
    let index = 7u32;
    let digest = hyperlane_dusk_types::checkpoint::checkpoint_digest(
        message::origin(message),
        &hook,
        &root,
        index,
        &message::id(message),
    );
    let mut metadata = Vec::from(hook);
    metadata.extend_from_slice(&root);
    metadata.extend_from_slice(&index.to_be_bytes());
    let secp = Secp256k1::new();
    for signer in signers {
        let signature = secp.sign_ecdsa_recoverable(SecpMessage::from_digest(digest), signer);
        let (recovery, compact) = signature.serialize_compact();
        metadata.extend_from_slice(&compact);
        metadata.push(u8::try_from(i32::from(recovery)).unwrap() + 27);
    }
    metadata
}

#[test]
fn test_multisig_ism_counts_distinct_enrolled_signers_in_order() {
    let signers = checkpoint_signers();
    let validators = signers[..3].iter().map(|(address, _)| *address).collect();
    let mut session = session_with_multisig_ism(*OWNER_ID, validators, 2);
    let message = sample_encoded_message(TEST_RECIPIENT_ID);
    let valid = signed_checkpoint_metadata(&message, &[&signers[0].1, &signers[2].1]);
    assert!(
        session
            .direct_call::<_, bool>(ISM_MULTISIG_ID, "verify", &(valid, message.clone()))
            .unwrap()
            .data
    );

    for selected in [[0usize, 0], [2, 0], [0, 3]] {
        let metadata = signed_checkpoint_metadata(
            &message,
            &[&signers[selected[0]].1, &signers[selected[1]].1],
        );
        assert_contract_panic(
            session.direct_call::<_, bool>(ISM_MULTISIG_ID, "verify", &(metadata, message.clone())),
            "MultisigISM: insufficient valid signatures",
        );
    }
    let one_signature = signed_checkpoint_metadata(&message, &[&signers[0].1]);
    assert_contract_panic(
        session.direct_call::<_, bool>(ISM_MULTISIG_ID, "verify", &(one_signature, message)),
        "MultisigISM: not enough signatures",
    );
}

#[test]
fn test_multisig_ism_signatures_bind_checkpoint_and_complete_message() {
    let signers = checkpoint_signers();
    let validators = signers[..3].iter().map(|(address, _)| *address).collect();
    let mut session = session_with_multisig_ism(*OWNER_ID, validators, 2);
    let message = sample_encoded_message(TEST_RECIPIENT_ID);
    let metadata = signed_checkpoint_metadata(&message, &[&signers[0].1, &signers[2].1]);
    assert!(
        session
            .direct_call::<_, bool>(
                ISM_MULTISIG_ID,
                "verify",
                &(metadata.clone(), message.clone())
            )
            .unwrap()
            .data
    );

    // Hook, root, and index are each signed independently of the message.
    for offset in [0, 32, 67] {
        let mut changed = metadata.clone();
        changed[offset] ^= 1;
        assert_contract_panic(
            session.direct_call::<_, bool>(ISM_MULTISIG_ID, "verify", &(changed, message.clone())),
            "MultisigISM: insufficient valid signatures",
        );
    }
    // Nonce, origin, sender, destination, recipient, and body are all bound.
    for offset in [1, 5, 9, 41, 45, 77] {
        let mut changed = message.clone();
        changed[offset] ^= 1;
        assert_contract_panic(
            session.direct_call::<_, bool>(ISM_MULTISIG_ID, "verify", &(metadata.clone(), changed)),
            "MultisigISM: insufficient valid signatures",
        );
    }
    assert!(
        session
            .direct_call::<_, bool>(ISM_MULTISIG_ID, "verify", &(metadata, message))
            .unwrap()
            .data
    );
}

#[test]
fn test_multisig_ism_threshold_update_takes_effect_for_real_signatures() {
    let signers = checkpoint_signers();
    let validators: Vec<_> = signers[..3].iter().map(|(address, _)| *address).collect();
    let mut session = session_with_multisig_ism(*OWNER_ID, validators.clone(), 2);
    let message = sample_encoded_message(TEST_RECIPIENT_ID);
    let two = signed_checkpoint_metadata(&message, &[&signers[0].1, &signers[2].1]);
    assert!(
        session
            .direct_call::<_, bool>(ISM_MULTISIG_ID, "verify", &(two.clone(), message.clone()))
            .unwrap()
            .data
    );
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            ISM_MULTISIG_ID,
            "set_validators_and_threshold",
            &(validators, 3u8),
        )
        .unwrap();
    assert_contract_panic(
        session.direct_call::<_, bool>(ISM_MULTISIG_ID, "verify", &(two, message.clone())),
        "MultisigISM: not enough signatures",
    );
    let three =
        signed_checkpoint_metadata(&message, &[&signers[0].1, &signers[1].1, &signers[2].1]);
    assert!(
        session
            .direct_call::<_, bool>(ISM_MULTISIG_ID, "verify", &(three, message))
            .unwrap()
            .data
    );
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

    // There is no setter: anyone could otherwise point the deployed
    // TestRecipient at TestMock and have unsigned messages delivered.
    assert!(s
        .session
        .direct_call::<_, ()>(
            TEST_RECIPIENT_ID,
            "set_interchain_security_module",
            &(TEST_MOCK_ID,),
        )
        .is_err());
    let ism: ContractId = s
        .session
        .direct_call::<_, ContractId>(TEST_RECIPIENT_ID, "interchain_security_module", &())
        .expect("query should succeed")
        .data;
    assert_eq!(ism, ContractId::from_bytes([0u8; 32]));
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
    session_with_hooks_and_igp(IGP_BYTECODE, protocol_fee, max_protocol_fee, igp_configs)
}

/// [`session_with_hooks_fee_and_igp_config`] with the IGP deployed from
/// `igp_bytecode`.
fn session_with_hooks_and_igp(
    igp_bytecode: &[u8],
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
            igp_bytecode,
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
fn test_warp_user_contributes_exact_dispatch_fee_without_spending_sponsor_credit() {
    let mut session = session_with_hooks();
    let remote_router = [0xA5u8; 32];
    let mint_amount = 1_000_000u64;
    const SPONSOR_CREDIT: u64 = 2 * MIN_NEW_PAYER_CREDIT;

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

    let owner_hash = message::keccak256(&OWNER_PK.to_bytes());
    let inbound = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &token_message::encode(owner_hash, mint_amount),
    );
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), inbound))
        .expect("the owner's synthetic tokens should wait in escrow");
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_ID, "claim_pending", &())
        .expect("the owner should claim its synthetic tokens");

    session
        .call_public_with_deposit::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(WARP_DRC20_ID.to_bytes(), SPONSOR_CREDIT),
            SPONSOR_CREDIT,
        )
        .expect("operational sponsor credit should be funded");
    let args = (REMOTE_DOMAIN, [0xD5u8; 32], 1u64);
    let fee = session
        .direct_call::<_, u64>(WARP_DRC20_ID, "quote_transfer_remote", &args)
        .expect("the route quote should succeed")
        .data;
    assert!(fee > 0);

    let unfunded =
        session.call_public::<_, MessageId>(&OWNER_SK, WARP_DRC20_ID, "transfer_remote", &args);
    assert_contract_panic_contains(unfunded, "dispatch fee deposit failed");
    assert_eq!(
        session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(WARP_DRC20_ID.to_bytes(),),)
            .expect("route credit should remain queryable")
            .data,
        SPONSOR_CREDIT
    );

    session
        .call_public_with_deposit::<_, MessageId>(
            &OWNER_SK,
            WARP_DRC20_ID,
            "transfer_remote",
            &args,
            fee,
        )
        .expect("an exact user fee contribution should dispatch");
    assert_eq!(
        session
            .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(WARP_DRC20_ID.to_bytes(),),)
            .expect("route credit should remain queryable")
            .data,
        SPONSOR_CREDIT,
        "the user's transfer must not decrement shared sponsor credit"
    );
}

#[test]
fn test_mailbox_rejects_forged_contract_dispatch_funding_callback() {
    let mut session = session_with_hooks();
    let result = session.direct_call::<_, ()>(
        MAILBOX_ID,
        "receive_dispatch_funding",
        &(ReceiveFromContract {
            contract: WARP_DRC20_ID,
            value: 1_000,
            data: Vec::new(),
        },),
    );
    assert_contract_panic(result, "Mailbox: unauthenticated dispatch funding");
}

#[test]
fn test_protocol_fee_contract_shaped_beneficiary_cannot_redirect_claim() {
    let mut session = session_with_hooks();
    session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            TEST_RECIPIENT_ID,
            "dispatch_message",
            &(
                MAILBOX_ID,
                REMOTE_DOMAIN,
                [0xBBu8; 32],
                b"contract beneficiary".to_vec(),
            ),
        )
        .expect("dispatch should collect protocol fees");
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            PROTOCOL_FEE_ID,
            "set_beneficiary",
            &(TEST_RECIPIENT_ID.to_bytes(),),
        )
        .expect("owner should set the beneficiary");

    let result = session.direct_call::<_, ()>(
        TEST_RECIPIENT_ID,
        "claim_protocol_fees",
        &(PROTOCOL_FEE_ID,),
    );
    assert!(result.is_err(), "nested beneficiary claim must fail");
    assert_eq!(
        session
            .direct_call::<_, u64>(PROTOCOL_FEE_ID, "claimable_fees", &())
            .expect("claimable_fees should succeed")
            .data,
        1000
    );
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

    session
        .call_public_with_deposit::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(*OWNER_ID, MIN_NEW_PAYER_CREDIT),
            MIN_NEW_PAYER_CREDIT,
        )
        .expect("funding after the failed dispatch should succeed");
    session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            MAILBOX_ID,
            "dispatch_default",
            &(
                REMOTE_DOMAIN,
                [0xBBu8; 32],
                b"funded retry after rollback".to_vec(),
            ),
        )
        .expect("a later dispatch should prove the reentrancy guard rolled back");
}

#[test]
fn test_dispatch_credit_withdrawal_is_payer_owned_and_value_backed() {
    let mut s = HyperlaneSession::new();
    let funded = 40_000_000u64;
    let partial = 15_000_000u64;

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
    // The data driver encodes u64 as decimal strings so JavaScript keeps them exact.
    assert_eq!(
        decoded["amount"].as_str(),
        Some(partial.to_string().as_str())
    );
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
    let funded = 20_000_000u64;

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
    // caller correctly after the nested transfer panic rolled back. The
    // payer has credit, so a top-up of 1 LUX is enough.
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
    let owner_credit = 40_000_000u64;
    let relayer_credit = 60_000_000u64;
    let owner_withdrawal = 15_000_000u64;

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

    // 4: `pay_for_gas` and `quote_pay_for_gas`.
    let version = session
        .direct_call::<_, u32>(IGP_ID, "state_version", &())
        .expect("state_version should succeed")
        .data;
    assert_eq!(version, 4);

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
                gas_overhead: 0,
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
fn test_igp_contract_shaped_beneficiary_cannot_redirect_claim() {
    let mut session = session_with_hooks_and_igp_config(vec![(
        REMOTE_DOMAIN,
        DomainGasConfig {
            gas_overhead: 0,
            token_exchange_rate: 10_000_000_000,
            gas_price: 1,
        },
    )]);
    session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            TEST_RECIPIENT_ID,
            "dispatch_message",
            &(
                MAILBOX_ID,
                REMOTE_DOMAIN,
                [0xBBu8; 32],
                b"contract IGP beneficiary".to_vec(),
            ),
        )
        .expect("dispatch should collect IGP fees");
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            IGP_ID,
            "set_beneficiary",
            &(TEST_RECIPIENT_ID.to_bytes(),),
        )
        .expect("owner should set the beneficiary");

    let result = session.direct_call::<_, ()>(TEST_RECIPIENT_ID, "claim_igp_fees", &(IGP_ID,));
    assert!(result.is_err(), "nested beneficiary claim must fail");
    assert_eq!(
        session
            .direct_call::<_, u64>(IGP_ID, "claimable_fees", &())
            .expect("claimable_fees should succeed")
            .data,
        50_000
    );
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
// Tests: IGP top-ups (pay_for_gas)
// =============================================================================

/// `dusk-tx pay-for-gas`'s default gas limit. A failed transaction pays its
/// whole limit, so the limit stays near what a top-up costs, about 1.85M.
const PAY_FOR_GAS_GAS_LIMIT: u64 = 3_000_000;

/// Pricing for the top-up tests: 1 LUX per gas on the destination, and an
/// overhead that a dispatch pays for on top of its gas limit.
const TOP_UP_PRICING: DomainGasConfig = DomainGasConfig {
    gas_overhead: 100_000,
    token_exchange_rate: 10_000_000_000,
    gas_price: 1,
};

/// The IGP's `TOP_UP_MEMORY_LIMIT`.
const TOP_UP_MEMORY_LIMIT: u64 = 3 * 1024 * 1024 * 1024;

/// A session whose IGP, the Mailbox's default hook, prices `REMOTE_DOMAIN`
/// at [`TOP_UP_PRICING`].
fn session_for_top_ups() -> TestSession {
    session_with_hooks_and_igp_config(vec![(REMOTE_DOMAIN, TOP_UP_PRICING)])
}

/// Dispatches a message from the TestRecipient through the Mailbox's default
/// hook, the IGP, and returns the receipt. With no hook metadata it pays for
/// 50,000 gas plus the destination's overhead.
fn dispatch_through_igp(session: &mut TestSession, body: &[u8]) -> CallReceipt<MessageId> {
    session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            TEST_RECIPIENT_ID,
            "dispatch_message",
            &(MAILBOX_ID, REMOTE_DOMAIN, [0xBBu8; 32], body.to_vec()),
        )
        .expect("the dispatch should pay the IGP")
}

/// `payer`'s Moonlight transaction calling
/// `pay_for_gas(id, destination, gas_amount, declared)` with a deposit of
/// `deposit`, at `dusk-tx pay-for-gas`'s default gas limit.
fn pay_for_gas(
    session: &mut TestSession,
    payer: &AccountSecretKey,
    (id, destination, gas_amount): (MessageId, u32, u64),
    declared: u64,
    deposit: u64,
) -> Result<CallReceipt<()>, ContractError> {
    session.call_public_with_deposit_and_gas_limit::<_, ()>(
        payer,
        IGP_ID,
        "pay_for_gas",
        &(id, destination, gas_amount, declared),
        deposit,
        PAY_FOR_GAS_GAS_LIMIT,
    )
}

fn quote_pay_for_gas(session: &mut TestSession, destination: u32, gas_amount: u64) -> u64 {
    query(
        session,
        IGP_ID,
        "quote_pay_for_gas",
        &(destination, gas_amount),
    )
}

/// Every payment record the IGP holds, at most a page of them.
fn igp_payment_records(session: &mut TestSession) -> Vec<GasPaymentRecord> {
    let count: u32 = query(session, IGP_ID, "gas_payment_count", &());
    assert!(count <= 256, "{count} records are more than a page");
    query(session, IGP_ID, "gas_payments", &(0u32, count))
}

/// What the relayer adds up for a message: the gas and the LUX of every
/// payment for its ID and destination (hyperlane-base keys gas payments by
/// both).
fn paid_for(records: &[GasPaymentRecord], id: MessageId, destination: u32) -> (u64, u64) {
    records
        .iter()
        .filter(|record| record.message_id == id && record.destination == destination)
        .fold((0, 0), |(gas, lux), record| {
            (gas + record.gas_limit, lux + record.payment)
        })
}

/// The IGP holds what its records say it was paid, all of it claimable.
fn assert_igp_holds_what_it_recorded(session: &mut TestSession) {
    let total: u64 = query(session, IGP_ID, "total_gas_payments", &());
    let claimable: u64 = query(session, IGP_ID, "claimable_fees", &());
    let recorded: u64 = igp_payment_records(session)
        .iter()
        .map(|record| record.payment)
        .sum();
    assert_eq!(total, recorded);
    assert_eq!(claimable, total);
    assert_eq!(
        session
            .contract_balance(&IGP_ID)
            .expect("IGP balance query should succeed"),
        claimable
    );
}

/// The panic message of a call that must fail.
fn panic_message<R>(result: Result<CallReceipt<R>, ContractError>) -> String
where
    R: rkyv::Archive,
    R::Archived: rkyv::Deserialize<R, rkyv::Infallible>
        + for<'b> rkyv::bytecheck::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
{
    match result {
        Err(ContractError::Panic(message)) => message,
        Err(error) => panic!("expected a contract panic, got {error}"),
        Ok(_) => panic!("the call should fail"),
    }
}

/// The events the transfer contract emitted with `topic`, decoded.
fn transfer_events<E>(receipt: &CallReceipt<()>, topic: &str) -> Vec<E>
where
    E: rkyv::Archive,
    E::Archived: rkyv::Deserialize<E, rkyv::Infallible>
        + for<'b> rkyv::bytecheck::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
{
    receipt
        .events
        .iter()
        .filter(|e| {
            e.source == dusk_core::transfer::TRANSFER_CONTRACT && e.topic == topic && !e.reverted
        })
        .map(|e| test_session::rkyv_deserialize::<E>(&e.data))
        .collect()
}

/// A message whose dispatch paid for less gas than its delivery takes can be
/// topped up by anyone. The IGP charges exactly its quote for the missing
/// gas, with no overhead, and records it under the message's ID and
/// destination; the payments the relayer adds up then cover the delivery,
/// at the price one dispatch asking for all that gas would have paid.
#[test]
fn test_igp_top_up_pays_the_rest_of_an_underpaid_dispatch() {
    let mut session = session_for_top_ups();
    let id = dispatch_through_igp(&mut session, b"a handle that takes more gas").data;
    let dispatch_paid = paid_for(&igp_payment_records(&mut session), id, REMOTE_DOMAIN);
    assert_eq!(
        dispatch_paid,
        (150_000, 150_000),
        "50,000 gas plus the 100,000 overhead, at 1 LUX"
    );

    // Its delivery takes 400,000 gas. Anyone may pay for the rest: here the
    // relayer's account, not the sender.
    let needed = 400_000u64;
    let missing = needed - dispatch_paid.0;
    let quote = quote_pay_for_gas(&mut session, REMOTE_DOMAIN, missing);
    assert_eq!(quote, missing, "no overhead on a top-up, at 1 LUX per gas");
    let payer_before = moonlight_balance(&mut session, &RELAYER_PK);
    let igp_before = session.contract_balance(&IGP_ID).unwrap();
    let receipt = pay_for_gas(
        &mut session,
        &RELAYER_SK,
        (id, REMOTE_DOMAIN, missing),
        quote,
        quote,
    )
    .expect("a top-up of exactly its quote should succeed");
    assert!(
        receipt.gas_spent * 3 / 2 <= PAY_FOR_GAS_GAS_LIMIT,
        "pay_for_gas took {} gas, too close to {PAY_FOR_GAS_GAS_LIMIT}",
        receipt.gas_spent
    );
    assert_eq!(
        payer_before - moonlight_balance(&mut session, &RELAYER_PK),
        quote + receipt.gas_spent
    );
    assert_eq!(
        session.contract_balance(&IGP_ID).unwrap(),
        igp_before + quote
    );

    let records = igp_payment_records(&mut session);
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[1],
        GasPaymentRecord {
            message_id: id,
            destination: REMOTE_DOMAIN,
            gas_limit: missing,
            payment: quote,
            block_height: session.block_height(),
        }
    );
    let (gas, lux) = paid_for(&records, id, REMOTE_DOMAIN);
    assert_eq!(gas, needed);
    let as_one_dispatch: u64 = query(
        &mut session,
        IGP_ID,
        "quote_gas_payment",
        &(REMOTE_DOMAIN, needed - TOP_UP_PRICING.gas_overhead),
    );
    assert_eq!(lux, as_one_dispatch);
    assert_igp_holds_what_it_recorded(&mut session);

    // Payments keep adding up, from anyone: the sender adds 1,000 gas.
    pay_for_gas(
        &mut session,
        &OWNER_SK,
        (id, REMOTE_DOMAIN, 1_000),
        1_000,
        1_000,
    )
    .expect("a second top-up should succeed");
    assert_eq!(
        paid_for(&igp_payment_records(&mut session), id, REMOTE_DOMAIN),
        (needed + 1_000, as_one_dispatch + 1_000)
    );
    assert_igp_holds_what_it_recorded(&mut session);
}

/// The IGP charges the price at execution and sends the rest of the deposit
/// back to the account that signed, in the same transaction. A deposit with
/// a margin survives a price update between quoting and inclusion; one
/// without is refused, and comes back whole.
#[test]
fn test_igp_top_up_refunds_the_deposit_beyond_its_price() {
    let mut session = session_for_top_ups();
    let id = dispatch_through_igp(&mut session, b"refund").data;
    let gas = 100_000u64;
    let quote = quote_pay_for_gas(&mut session, REMOTE_DOMAIN, gas);
    let deposit = quote + 1_234_567;
    let before = moonlight_balance(&mut session, &RELAYER_PK);
    let receipt = pay_for_gas(
        &mut session,
        &RELAYER_SK,
        (id, REMOTE_DOMAIN, gas),
        deposit,
        deposit,
    )
    .expect("a top-up with a margin should succeed");
    assert!(
        receipt.gas_spent * 3 / 2 <= PAY_FOR_GAS_GAS_LIMIT,
        "pay_for_gas with a refund took {} gas, too close to {PAY_FOR_GAS_GAS_LIMIT}",
        receipt.gas_spent
    );
    assert_eq!(
        before - moonlight_balance(&mut session, &RELAYER_PK),
        quote + receipt.gas_spent
    );
    // The transfer contract moved the whole deposit to the IGP, and the IGP
    // sent the margin back to the signer.
    let deposits: Vec<dusk_core::transfer::DepositEvent> =
        transfer_events(&receipt, dusk_core::transfer::DEPOSIT_TOPIC);
    assert_eq!(deposits.len(), 1);
    assert_eq!(deposits[0].sender, Some(*RELAYER_PK));
    assert_eq!(deposits[0].receiver, IGP_ID);
    assert_eq!(deposits[0].value, deposit);
    let refunds: Vec<dusk_core::transfer::ContractToAccountEvent> =
        transfer_events(&receipt, dusk_core::transfer::CONTRACT_TO_ACCOUNT_TOPIC);
    assert_eq!(refunds.len(), 1);
    assert_eq!(refunds[0].sender, IGP_ID);
    assert_eq!(refunds[0].receiver, *RELAYER_PK);
    assert_eq!(refunds[0].value, deposit - quote);
    assert_eq!(igp_payment_records(&mut session)[1].payment, quote);
    assert_igp_holds_what_it_recorded(&mut session);

    // The owner doubles the price after the payer quoted it.
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            IGP_ID,
            "set_domain_gas_config",
            &(
                REMOTE_DOMAIN,
                DomainGasConfig {
                    gas_price: 2,
                    ..TOP_UP_PRICING
                },
            ),
        )
        .expect("the owner should reprice the destination");
    let records = igp_payment_records(&mut session);
    let before = moonlight_balance(&mut session, &RELAYER_PK);
    let result = pay_for_gas(
        &mut session,
        &RELAYER_SK,
        (id, REMOTE_DOMAIN, gas),
        quote,
        quote,
    );
    assert_contract_panic(result, "IGP: insufficient interchain gas payment");
    assert_eq!(
        before - moonlight_balance(&mut session, &RELAYER_PK),
        PAY_FOR_GAS_GAS_LIMIT,
        "the refused transaction pays its gas limit and gets its deposit back"
    );
    assert_eq!(igp_payment_records(&mut session), records);

    let before = moonlight_balance(&mut session, &RELAYER_PK);
    let receipt = pay_for_gas(
        &mut session,
        &RELAYER_SK,
        (id, REMOTE_DOMAIN, gas),
        3 * quote,
        3 * quote,
    )
    .expect("a deposit with room for the new price should pay it");
    assert_eq!(
        before - moonlight_balance(&mut session, &RELAYER_PK),
        2 * quote + receipt.gas_spent
    );
    let charged = igp_payment_records(&mut session).last().unwrap().payment;
    assert_eq!(charged, 2 * quote);
    assert_igp_holds_what_it_recorded(&mut session);
}

/// A deposit below the quote is refused, and the transaction gets it back:
/// it pays only its gas, and nothing is recorded.
#[test]
fn test_igp_top_up_rejects_underpayment() {
    let mut session = session_for_top_ups();
    let id = dispatch_through_igp(&mut session, b"underpaid").data;
    let gas = 250_000u64;
    let quote = quote_pay_for_gas(&mut session, REMOTE_DOMAIN, gas);
    let records = igp_payment_records(&mut session);
    let igp_before = session.contract_balance(&IGP_ID).unwrap();
    let before = moonlight_balance(&mut session, &RELAYER_PK);
    for short in [quote - 1, 1, 0] {
        let result = pay_for_gas(
            &mut session,
            &RELAYER_SK,
            (id, REMOTE_DOMAIN, gas),
            short,
            short,
        );
        assert_contract_panic(result, "IGP: insufficient interchain gas payment");
    }
    assert_eq!(
        before - moonlight_balance(&mut session, &RELAYER_PK),
        3 * PAY_FOR_GAS_GAS_LIMIT
    );
    assert_eq!(igp_payment_records(&mut session), records);
    assert_eq!(session.contract_balance(&IGP_ID).unwrap(), igp_before);
    assert_igp_holds_what_it_recorded(&mut session);
}

/// A top-up is recorded only after the transfer contract has moved the
/// transaction's deposit into the IGP, and a deposit pays for one top-up
/// only. The declared deposit must be the transaction's exactly: declaring
/// more would refund DUSK that never arrived. A transaction without a
/// deposit pays nothing, and a contract can't spend the deposit of the
/// transaction that calls it. Every refused transaction gets its deposit
/// back.
#[test]
fn test_igp_top_up_takes_the_transaction_deposit_exactly_once() {
    let mut session = session_for_top_ups();
    rt_deploy(&mut session);
    let id = dispatch_through_igp(&mut session, b"deposit").data;
    let gas = 10_000u64;
    let quote = quote_pay_for_gas(&mut session, REMOTE_DOMAIN, gas);
    let records = igp_payment_records(&mut session);
    let igp_before = session.contract_balance(&IGP_ID).unwrap();

    let before = moonlight_balance(&mut session, &RELAYER_PK);
    for (declared, deposit, reason) in [
        (quote, 0, "There is no deposit in the transaction"),
        (
            quote + 1,
            quote,
            "doesn't match the value in the transaction",
        ),
        (
            quote,
            quote + 1,
            "doesn't match the value in the transaction",
        ),
    ] {
        let message = panic_message(pay_for_gas(
            &mut session,
            &RELAYER_SK,
            (id, REMOTE_DOMAIN, gas),
            declared,
            deposit,
        ));
        assert!(
            message.contains("IGP: deposit failed") && message.contains(reason),
            "declared {declared}, deposited {deposit}: {message}"
        );
    }
    assert_eq!(
        before - moonlight_balance(&mut session, &RELAYER_PK),
        3 * PAY_FOR_GAS_GAS_LIMIT
    );
    assert_eq!(igp_payment_records(&mut session), records);
    assert_eq!(session.contract_balance(&IGP_ID).unwrap(), igp_before);

    // A contract that the transaction calls, with the deposit, can't pay
    // with it: the deposit is the contract's to take, not the IGP's.
    let pay = vec![rt_action(
        IGP_ID,
        "pay_for_gas",
        &(id, REMOTE_DOMAIN, gas, quote),
    )];
    let args = |strict: bool| {
        rkyv::to_bytes::<_, 4096>(&(pay.clone(), strict))
            .expect("the batch should serialize")
            .to_vec()
    };
    let message = panic_message(session.call_public_raw_with_deposit(
        &RELAYER_SK,
        REDTEAM_ID,
        "run",
        args(true),
        quote,
    ));
    assert!(
        message.contains("IGP: payer must call directly"),
        "{message}"
    );
    let before = moonlight_balance(&mut session, &RELAYER_PK);
    let receipt = session
        .call_public_raw_with_deposit(&RELAYER_SK, REDTEAM_ID, "run", args(false), quote)
        .expect("the red-team contract records the failed call");
    assert_eq!(rt_results(&mut session), vec![false]);
    assert_eq!(
        before - moonlight_balance(&mut session, &RELAYER_PK),
        receipt.gas_spent,
        "nobody took the deposit, so it came back whole"
    );
    assert_eq!(igp_payment_records(&mut session), records);
    assert_eq!(session.contract_balance(&IGP_ID).unwrap(), igp_before);

    // Each transaction pays for its own top-up.
    let before = moonlight_balance(&mut session, &RELAYER_PK);
    let first = pay_for_gas(
        &mut session,
        &RELAYER_SK,
        (id, REMOTE_DOMAIN, gas),
        quote,
        quote,
    )
    .expect("the first top-up should succeed");
    let second = pay_for_gas(
        &mut session,
        &RELAYER_SK,
        (id, REMOTE_DOMAIN, gas),
        quote,
        quote,
    )
    .expect("the second top-up should succeed");
    assert_eq!(
        before - moonlight_balance(&mut session, &RELAYER_PK),
        2 * quote + first.gas_spent + second.gas_spent
    );
    assert_eq!(
        session.contract_balance(&IGP_ID).unwrap(),
        igp_before + 2 * quote
    );
    assert_eq!(igp_payment_records(&mut session).len(), records.len() + 2);
    assert_igp_holds_what_it_recorded(&mut session);
}

/// A top-up is priced by its destination's pricing. A destination without
/// any, no gas, more gas than the IGP prices, or a payment that rounds to
/// zero is refused before the deposit is taken, and nothing is recorded.
/// Once the owner prices a destination, top-ups to it go through, and are
/// recorded under that destination, not the message's.
#[test]
fn test_igp_top_up_needs_a_priced_destination() {
    let unpriced = 42u32;
    let cheap = 43u32;
    // 1 LUX per 10,000 gas, and an overhead that keeps the IGP's smallest
    // dispatch quote at 1 LUX.
    let cheap_pricing = DomainGasConfig {
        gas_overhead: 10_000,
        token_exchange_rate: 1_000_000,
        gas_price: 1,
    };
    let mut session = session_with_hooks_and_igp_config(vec![
        (REMOTE_DOMAIN, TOP_UP_PRICING),
        (cheap, cheap_pricing),
    ]);
    let id = dispatch_through_igp(&mut session, b"priced").data;
    assert_contract_panic(
        session.direct_call::<_, u64>(IGP_ID, "quote_pay_for_gas", &(unpriced, 1_000u64)),
        "IGP: destination is not configured",
    );
    let records = igp_payment_records(&mut session);
    let igp_before = session.contract_balance(&IGP_ID).unwrap();
    let before = moonlight_balance(&mut session, &RELAYER_PK);
    for ((destination, gas), expected) in [
        ((unpriced, 1_000u64), "IGP: destination is not configured"),
        ((REMOTE_DOMAIN, 0), "IGP: gas amount cannot be zero"),
        (
            (REMOTE_DOMAIN, 1_000_000_001),
            "IGP: gas amount exceeds supported maximum",
        ),
        ((cheap, 9_999), "IGP: payment rounds to zero"),
    ] {
        let result = pay_for_gas(
            &mut session,
            &RELAYER_SK,
            (id, destination, gas),
            1_000_000,
            1_000_000,
        );
        assert_contract_panic(result, expected);
    }
    assert_eq!(
        before - moonlight_balance(&mut session, &RELAYER_PK),
        4 * PAY_FOR_GAS_GAS_LIMIT
    );
    assert_eq!(igp_payment_records(&mut session), records);
    assert_eq!(session.contract_balance(&IGP_ID).unwrap(), igp_before);
    assert_eq!(quote_pay_for_gas(&mut session, cheap, 10_000), 1);
    assert_eq!(
        quote_pay_for_gas(&mut session, REMOTE_DOMAIN, 1_000_000_000),
        1_000_000_000
    );

    session
        .call_public::<_, ()>(
            &OWNER_SK,
            IGP_ID,
            "set_domain_gas_config",
            &(unpriced, TOP_UP_PRICING),
        )
        .expect("the owner should price the destination");
    pay_for_gas(
        &mut session,
        &RELAYER_SK,
        (id, unpriced, 1_000),
        1_000,
        1_000,
    )
    .expect("a top-up to a priced destination should succeed");
    let records = igp_payment_records(&mut session);
    assert_eq!(paid_for(&records, id, unpriced), (1_000, 1_000));
    assert_eq!(
        paid_for(&records, id, REMOTE_DOMAIN),
        (150_000, 150_000),
        "a payment for another destination doesn't add to the message's"
    );
    assert_igp_holds_what_it_recorded(&mut session);
}

/// A top-up emits the event a dispatch's payment does: the same topic, the
/// same `GasPayment` struct, the same bytes for the same fields, and those
/// fields are its record's, as hyperlane-dusk's IGP indexer rebuilds the
/// event from `gas_payments` to find it. Agents index top-ups unchanged.
#[test]
fn test_igp_top_up_emits_the_gas_payment_event_of_a_dispatch() {
    // No overhead, so topping up a dispatch's 50,000 gas has its fields.
    let mut session = session_with_hooks_and_igp_config(vec![(
        REMOTE_DOMAIN,
        DomainGasConfig {
            gas_overhead: 0,
            ..TOP_UP_PRICING
        },
    )]);
    let dispatch = dispatch_through_igp(&mut session, b"same event");
    let id = dispatch.data;
    let top_up = pay_for_gas(
        &mut session,
        &RELAYER_SK,
        (id, REMOTE_DOMAIN, 50_000),
        50_000,
        50_000,
    )
    .expect("the top-up should succeed");

    let igp_events = |events: &[dusk_core::abi::Event]| -> Vec<dusk_core::abi::Event> {
        events
            .iter()
            .filter(|e| e.source == IGP_ID && !e.reverted)
            .cloned()
            .collect()
    };
    let from_dispatch = igp_events(&dispatch.events);
    let from_top_up = igp_events(&top_up.events);
    assert_eq!(from_dispatch.len(), 1);
    assert_eq!(from_top_up.len(), 1, "a top-up emits one event, GasPayment");
    assert_eq!(from_top_up[0].topic, events::GasPayment::TOPIC);
    assert_eq!(from_top_up[0].topic, from_dispatch[0].topic);
    assert_eq!(from_top_up[0].data, from_dispatch[0].data);

    let records = igp_payment_records(&mut session);
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0], records[1],
        "both paid 50,000 LUX for 50,000 gas"
    );
    for (record, event) in records.iter().zip([&from_dispatch[0], &from_top_up[0]]) {
        let rebuilt = test_session::rkyv_serialize(&events::GasPayment {
            message_id: record.message_id,
            gas_limit: record.gas_limit,
            payment: record.payment,
        });
        assert_eq!(event.data, rebuilt);
        let decoded = HyperlaneDataDriver
            .decode_event(&event.topic, &event.data)
            .unwrap();
        assert_eq!(decoded["gas_limit"].as_str(), Some("50000"));
        assert_eq!(decoded["payment"].as_str(), Some("50000"));
    }
}

/// As on EVM, the IGP doesn't look a top-up's message up: a payment for an
/// ID the Mailbox never dispatched is recorded and only costs its payer.
/// The relayer only looks up payments for messages it has indexed, so the
/// record never counts for anything.
#[test]
fn test_igp_top_up_for_an_undispatched_id_only_costs_its_payer() {
    let mut session = session_for_top_ups();
    let never_dispatched = [0x5Au8; 32];
    let quote = quote_pay_for_gas(&mut session, REMOTE_DOMAIN, 5_000);
    let before = moonlight_balance(&mut session, &RELAYER_PK);
    let receipt = pay_for_gas(
        &mut session,
        &RELAYER_SK,
        (never_dispatched, REMOTE_DOMAIN, 5_000),
        quote,
        quote,
    )
    .expect("a top-up for any ID should succeed");
    assert_eq!(
        before - moonlight_balance(&mut session, &RELAYER_PK),
        quote + receipt.gas_spent
    );
    assert_eq!(
        paid_for(
            &igp_payment_records(&mut session),
            never_dispatched,
            REMOTE_DOMAIN
        ),
        (5_000, quote)
    );
    assert_eq!(query::<_, u32>(&mut session, MAILBOX_ID, "nonce", &()), 0);
    assert_igp_holds_what_it_recorded(&mut session);
}

/// The most a top-up can still grow the IGP's memory, the bound it checks:
/// the chunk of records and the larger chunk table the next record may
/// start, each with dlmalloc's overhead, in whole pages. `log` mirrors the
/// IGP's. The chunk table is twice as large here as in wasm32, which the
/// page rounding hides while it stays under 32 KiB.
fn top_up_memory_bound(log: &hyperlane_dusk_types::storage::ChunkedLog<GasPaymentRecord>) -> u64 {
    let [chunk, table] = log.push_allocations();
    assert!(table < 32 * 1024);
    [chunk, table]
        .into_iter()
        .map(|bytes| {
            if bytes == 0 {
                0
            } else {
                (bytes as u64 + 64).next_multiple_of(WASM_PAGE)
            }
        })
        .sum()
}

/// `pay_for_gas` refuses a top-up unless what it can allocate fits under
/// 3 GiB, so top-ups never take the IGP past that, and the last GiB stays
/// for the payments dispatches make. The IGP here starts a few pages below
/// the limit, as if it were nearly full. Every top-up it admits leaves its
/// memory at or below 3 GiB, and it admits exactly the top-ups whose record
/// fits. Once top-ups are closed, dispatches still pay the IGP, past 3 GiB,
/// and top-ups stay closed there.
#[test]
fn test_igp_top_ups_close_at_3_gib_and_dispatches_still_pay() {
    let limit_pages = TOP_UP_MEMORY_LIMIT / WASM_PAGE;
    let igp = with_initial_memory_pages(IGP_BYTECODE, limit_pages - 6);
    let mut session =
        session_with_hooks_and_igp(&igp, 1000, 10000, vec![(REMOTE_DOMAIN, TOP_UP_PRICING)]);
    rt_deploy(&mut session);
    // The red-team contract dispatches to fill chunks with dispatches'
    // payments: each pays 1,000 LUX of protocol fee and 150,000 to the IGP.
    let credit = 2_000_000_000u64;
    session
        .call_public_with_deposit::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(REDTEAM_ID.to_bytes(), credit),
            credit,
        )
        .expect("funding the red-team contract's dispatch credit should succeed");
    let start = session.memory_len(IGP_ID);
    assert!(
        (TOP_UP_MEMORY_LIMIT - 6 * WASM_PAGE..=TOP_UP_MEMORY_LIMIT - 4 * WASM_PAGE)
            .contains(&start),
        "starts at {start}"
    );

    let placeholder = GasPaymentRecord {
        message_id: [0; 32],
        destination: 0,
        gas_limit: 0,
        payment: 0,
        block_height: 0,
    };
    let per_chunk = hyperlane_dusk_types::storage::ChunkedLog::<GasPaymentRecord>::PER_CHUNK;
    let mut log = hyperlane_dusk_types::storage::ChunkedLog::<GasPaymentRecord>::new();
    let mut nonce = 0u32;
    let mut dispatch = |session: &mut TestSession,
                        log: &mut hyperlane_dusk_types::storage::ChunkedLog<GasPaymentRecord>,
                        count: u32| {
        dispatch_batch(session, nonce, count).expect("dispatches should pay the IGP");
        nonce += count;
        for _ in 0..count {
            log.push(placeholder);
        }
    };
    let top_up = ([0x7Bu8; 32], REMOTE_DOMAIN, 1_000u64);
    let mut admitted = 0;
    loop {
        let memory = session.memory_len(IGP_ID);
        let bound = top_up_memory_bound(&log);
        let fits = memory + bound <= TOP_UP_MEMORY_LIMIT;
        // Up to the next chunk, records allocate nothing: dispatches fill
        // the chunk, 256 to a transaction, but for one top-up.
        let room = per_chunk - log.len() % per_chunk;
        if bound == 0 && room > 1 {
            dispatch(&mut session, &mut log, (room - 1).min(256));
            continue;
        }
        match pay_for_gas(&mut session, &RELAYER_SK, top_up, 1_000, 1_000) {
            Ok(_) => {
                assert!(fits, "record {}: admitted at {memory} + {bound}", log.len());
                log.push(placeholder);
                admitted += 1;
                let after = session.memory_len(IGP_ID);
                assert!(
                    after <= TOP_UP_MEMORY_LIMIT,
                    "record {}: a top-up took the IGP to {after} bytes",
                    log.len()
                );
            }
            Err(error) => {
                assert!(
                    !fits,
                    "record {}: refused at {memory} + {bound}: {error}",
                    log.len()
                );
                assert_contract_panic(
                    Err::<CallReceipt<()>, _>(error),
                    "IGP: storage full, top-ups are closed",
                );
                break;
            }
        }
    }
    // Each admitted top-up that started a chunk took 2 to 3 pages; the IGP
    // started 4 to 6 below the limit, so at least the first chunk fit.
    assert!(admitted >= 2, "admitted {admitted} top-ups");
    let closed_at = session.memory_len(IGP_ID);
    assert!(closed_at <= TOP_UP_MEMORY_LIMIT);
    let count: u32 = query(&mut session, IGP_ID, "gas_payment_count", &());
    assert_eq!(count, log.len());

    // Dispatches go on paying the IGP until its memory is past 3 GiB.
    let mut batches = 0;
    while session.memory_len(IGP_ID) <= TOP_UP_MEMORY_LIMIT {
        assert!(
            batches < 16,
            "dispatches from {closed_at} bytes never passed the limit"
        );
        dispatch(&mut session, &mut log, 256);
        batches += 1;
    }
    let count: u32 = query(&mut session, IGP_ID, "gas_payment_count", &());
    assert_eq!(count, log.len());
    let last: GasPaymentRecord = query(&mut session, IGP_ID, "gas_payment_at", &(count - 1,));
    assert_eq!((last.destination, last.gas_limit), (REMOTE_DOMAIN, 150_000));
    // And top-ups stay closed there, even one that starts no chunk.
    assert_eq!(top_up_memory_bound(&log), 0);
    assert_contract_panic(
        pay_for_gas(&mut session, &RELAYER_SK, top_up, 1_000, 1_000),
        "IGP: storage full, top-ups are closed",
    );
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
    let funded = 30_000_000u64;
    let withdrawn = 10_000_000u64;
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

    // The Mailbox decodes the recipient's key behind the route's wrapper
    // too: a forged twin of the key pays the account that signs with it.
    for (name, twin) in forged_twins(&FRESH_PK) {
        let before = moonlight_balance(&mut session, &FRESH_PK);
        session
            .call_public::<_, ()>(
                &OWNER_SK,
                route,
                "withdraw_dispatch_credit",
                &(twin, 1_000u64),
            )
            .unwrap_or_else(|e| panic!("{name}: the route's withdrawal should succeed: {e}"));
        assert_eq!(
            moonlight_balance(&mut session, &FRESH_PK),
            before + 1_000,
            "{name}"
        );
        assert_eq!(moonlight_balance(&mut session, &twin), 0, "{name}");
    }
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
        8
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
        5
    );
}

/// A route registers an account only when the account's escrow is claimed
/// (`assert_self_claim_registers_the_sender`), so every entry in its
/// registry stands for a transfer someone sent it from the remote chain.
/// A funded key can't register itself: routes have no `register_account`,
/// and `claim_pending` with nothing in escrow reverts
/// (`assert_self_claim_without_escrow_registers_nothing`).
#[test]
fn test_a_funded_key_cannot_register_itself() {
    // Each call fails and pays its whole limit.
    const GAS_LIMIT: u64 = 3_000_000;
    for route in [Route::Native, Route::Synthetic, Route::Collateral] {
        let (mut session, _) = route.session(1_000);
        let sk = AccountSecretKey::random(&mut StdRng::seed_from_u64(0x5E1F_0000));
        let pk = AccountPublicKey::from(&sk);
        let id = message::keccak256(&pk.to_bytes());
        session.transfer(&RELAYER_SK, &pk, 4 * GAS_LIMIT);
        assert!(
            session
                .call_public_with_gas_limit::<_, ()>(
                    &sk,
                    route.id(),
                    "register_account",
                    &(),
                    GAS_LIMIT
                )
                .is_err(),
            "{route:?}: routes don't export register_account"
        );
        assert!(
            session
                .call_public_with_gas_limit::<_, ()>(
                    &sk,
                    route.id(),
                    "register_account_for",
                    &(pk,),
                    GAS_LIMIT
                )
                .is_err(),
            "{route:?}: routes don't export register_account_for"
        );
        assert_contract_panic(
            session.call_public_with_gas_limit::<_, ()>(
                &sk,
                route.id(),
                "claim_pending",
                &(),
                GAS_LIMIT,
            ),
            &format!("{}: no pending transfers", route.name()),
        );
        assert!(
            !route_is_registered(&mut session, route.id(), id),
            "{route:?}"
        );
        assert!(
            !route_is_registered(&mut session, route.id(), [0xFF; 32]),
            "{route:?}"
        );
    }
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
    assert_eq!(state_version, 7);
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
    session_with_warp_drc20_flow_from(WARP_DRC20_BYTECODE)
}

/// As [`session_with_warp_drc20_flow`], with WarpDrc20 deployed from
/// `warp_drc20_bytecode`.
fn session_with_warp_drc20_flow_from(warp_drc20_bytecode: &[u8]) -> (TestSession, H256) {
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
            warp_drc20_bytecode,
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
    // The owner registers first, by claiming 1 token unit.
    register_by_claim(
        &mut session,
        WARP_DRC20_ID,
        remote_router,
        100,
        &OWNER_SK,
        1,
    );
    let reserved = u64::MAX - 6;

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
        reserved + 1
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

    // OWNER_PK registers by claiming a first transfer of 1 unit.
    register_by_claim(
        &mut session,
        WARP_DRC20_ID,
        remote_router,
        100,
        &OWNER_SK,
        1,
    );

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

    // Verify tokens were minted, with no escrow
    let supply: u64 = session
        .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
        .expect("total_supply should succeed")
        .data;
    assert_eq!(supply, 1 + mint_amount);
    assert_eq!(
        warp_drc20_balance_of(&mut session, Drc20Account::moonlight(&OWNER_PK)),
        1 + mint_amount
    );
    assert_eq!(
        route_u64(
            &mut session,
            WARP_DRC20_ID,
            "pending_balance",
            recipient_h256
        ),
        0
    );
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
        .expect("inbound native transfer to a new account should wait in escrow");
    assert_eq!(
        session
            .contract_balance(&WARP_NATIVE_ID)
            .expect("WarpNative balance query should succeed"),
        amount
    );
    let claim = session
        .call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "claim_pending", &())
        .expect("the recipient claims its escrow");

    assert_eq!(
        session
            .contract_balance(&WARP_NATIVE_ID)
            .expect("WarpNative balance query should succeed"),
        0
    );
    // The claimer pays its gas at 1 LUX from the same account.
    assert_eq!(
        session
            .account(&OWNER_PK)
            .expect("recipient account query should succeed")
            .balance,
        account_balance_before + amount - claim.gas_spent
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

    use dusk_bytes::Serializable;
    let pk_bytes = OWNER_PK.to_bytes();
    let recipient_h256 = message::keccak256(&pk_bytes);

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
        .expect("inbound delivery to a new account should wait in escrow");
    assert_eq!(canonical_drc20_balance_of(&mut session, custody), outbound);
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_COLLATERAL_ID, "claim_pending", &())
        .expect("the claim should release canonical DRC20 custody");

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

#[test]
fn test_warp_routes_reject_zero_recipient_outbound() {
    let (mut drc20_session, _) = session_with_warp_drc20_flow();
    let result = drc20_session.direct_call::<_, MessageId>(
        WARP_DRC20_ID,
        "transfer_remote",
        &(REMOTE_DOMAIN, [0u8; 32], 1u64),
    );
    assert_contract_panic(result, "WarpDrc20: recipient cannot be zero");

    let (mut native_session, _) = session_with_warp_native_flow();
    let result = native_session.direct_call::<_, MessageId>(
        WARP_NATIVE_ID,
        "transfer_remote",
        &(REMOTE_DOMAIN, [0u8; 32], 1u64),
    );
    assert_contract_panic(result, "WarpNative: recipient cannot be zero");

    let (mut collateral_session, _) = session_with_warp_collateral_flow();
    let result = collateral_session.direct_call::<_, MessageId>(
        WARP_DRC20_COLLATERAL_ID,
        "transfer_remote",
        &(REMOTE_DOMAIN, [0u8; 32], 1u64),
    );
    assert_contract_panic(result, "WarpCollateral: recipient cannot be zero");
}

#[test]
fn test_warp_routes_reject_zero_recipient_inbound_without_state_changes() {
    let recipient = [0u8; 32];
    let body = hyperlane_dusk_types::token_message::encode(recipient, 1);

    let (mut drc20_session, drc20_router) = session_with_warp_drc20_flow();
    let message = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        drc20_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &body,
    );
    let result =
        drc20_session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), message));
    assert_contract_panic_contains(result, "WarpDrc20: recipient cannot be zero");
    assert_eq!(
        drc20_session
            .direct_call::<_, u64>(WARP_DRC20_ID, "total_supply", &())
            .expect("total_supply should succeed")
            .data,
        0
    );

    let (mut native_session, native_router) = session_with_warp_native_flow();
    let message = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        native_router,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &body,
    );
    let result =
        native_session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), message));
    assert_contract_panic_contains(result, "WarpNative: recipient cannot be zero");
    assert_eq!(
        native_session
            .direct_call::<_, u64>(WARP_NATIVE_ID, "pending_balance", &(recipient,))
            .expect("pending_balance should succeed")
            .data,
        0
    );

    let (mut collateral_session, collateral_router) = session_with_warp_collateral_funded_flow();
    let message = message::encode(
        VERSION,
        1,
        REMOTE_DOMAIN,
        collateral_router,
        LOCAL_DOMAIN,
        WARP_DRC20_COLLATERAL_ID.to_bytes(),
        &body,
    );
    let result = collateral_session.direct_call::<_, ()>(
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), message),
    );
    assert_contract_panic_contains(result, "WarpCollateral: recipient cannot be zero");
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
fn test_warp_native_contract_recipient_claims_authenticated_escrow() {
    let (mut session, remote_router) = session_with_warp_native_flow();
    let recipient = TEST_RECIPIENT_ID.to_bytes();
    let amount = 750_000u64;
    fund_warp_native(&mut session, amount);

    let encoded = message::encode(
        VERSION,
        1,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &hyperlane_dusk_types::token_message::encode(recipient, amount),
    );
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        .expect("contract-recipient native DUSK should enter escrow");
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_NATIVE_ID, "pending_balance", &(recipient,))
            .expect("pending_balance should succeed")
            .data,
        amount
    );

    session
        .direct_call::<_, ()>(
            TEST_RECIPIENT_ID,
            "claim_native_pending",
            &(WARP_NATIVE_ID,),
        )
        .expect("the recipient contract should claim its own native escrow");

    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_NATIVE_ID, "pending_balance", &(recipient,))
            .expect("pending_balance should succeed")
            .data,
        0
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_NATIVE_ID, "pending_total", &())
            .expect("pending_total should succeed")
            .data,
        0
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(TEST_RECIPIENT_ID, "native_pending_received", &())
            .expect("native receipt query should succeed")
            .data,
        amount
    );
    assert_eq!(
        session
            .contract_balance(&TEST_RECIPIENT_ID)
            .expect("recipient contract balance query should succeed"),
        amount
    );
}

#[test]
fn test_warp_native_contract_claim_rejects_root_moonlight_caller() {
    let (mut session, _remote_router) = session_with_warp_native_flow();
    let result =
        session.call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "claim_pending_contract", &());
    assert_contract_panic(
        result,
        "WarpNative: claim_pending_contract requires contract caller",
    );
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
    fund_warp_native(&mut session, 2_000_001);
    register_by_claim(
        &mut session,
        WARP_NATIVE_ID,
        remote_router,
        100,
        &OWNER_SK,
        1,
    );

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
    let (mut session, remote_router) = session_with_warp_native_flow();
    fund_warp_native(&mut session, 1);

    // Neither before the account is registered, nor after.
    let result = session.call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "claim_pending", &());
    assert_contract_panic(result, "WarpNative: no pending transfers");
    register_by_claim(
        &mut session,
        WARP_NATIVE_ID,
        remote_router,
        100,
        &OWNER_SK,
        1,
    );
    let result = session.call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "claim_pending", &());
    assert_contract_panic(result, "WarpNative: no pending transfers");
}

// --- WarpDrc20Collateral: External account registration ---

#[test]
fn test_warp_collateral_registers_by_claim() {
    let (mut session, remote_router) = session_with_warp_collateral_funded_flow();
    register_by_claim(
        &mut session,
        WARP_DRC20_COLLATERAL_ID,
        remote_router,
        100,
        &OWNER_SK,
        1,
    );
    assert!(!route_is_registered(
        &mut session,
        WARP_DRC20_COLLATERAL_ID,
        [0xFF; 32]
    ));
}

#[test]
fn test_warp_collateral_handle_rejects_insufficient_locked_balance() {
    let (mut session, remote_router) = session_with_warp_collateral_flow();

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
        .expect("the fixture's collateral should wait in escrow");
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_ID, "claim_pending", &())
        .expect("the owner claims, and so registers on, the underlying token");
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

    // The fixture registered the owner on the underlying token.
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

    // OWNER_PK registers on the collateral route by claiming 1 unit.
    register_by_claim(
        &mut session,
        WARP_DRC20_COLLATERAL_ID,
        remote_router,
        100,
        &OWNER_SK,
        1,
    );
    let owner_before = warp_drc20_balance_of(&mut session, Drc20Account::moonlight(&OWNER_PK));

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
    assert_eq!(
        warp_drc20_balance_of(&mut session, Drc20Account::moonlight(&OWNER_PK)),
        owner_before + unlock_amount
    );
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
fn test_warp_collateral_claim_pending_registers_and_pays() {
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
    assert!(!route_is_registered(
        &mut session,
        WARP_DRC20_COLLATERAL_ID,
        recipient_h256
    ));

    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_COLLATERAL_ID, "claim_pending", &())
        .expect("claim_pending should succeed");
    assert!(route_is_registered(
        &mut session,
        WARP_DRC20_COLLATERAL_ID,
        recipient_h256
    ));

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

    register_by_claim(
        &mut session,
        WARP_DRC20_COLLATERAL_ID,
        remote_router,
        100,
        &OWNER_SK,
        1,
    );
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

#[test]
fn test_igp_credits_the_overhead_in_paid_gas() {
    let mut session = session_with_hooks_and_igp_config(vec![(
        REMOTE_DOMAIN,
        DomainGasConfig {
            gas_overhead: 100_000,
            token_exchange_rate: 10_000_000_000,
            gas_price: 1,
        },
    )]);
    let receipt = session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            TEST_RECIPIENT_ID,
            "dispatch_message",
            &(
                MAILBOX_ID,
                REMOTE_DOMAIN,
                [0xBBu8; 32],
                b"paid gas includes overhead".to_vec(),
            ),
        )
        .expect("funded dispatch should succeed");
    let records = session
        .direct_call::<_, Vec<GasPaymentRecord>>(IGP_ID, "gas_payments", &(0u32, 256u32))
        .unwrap()
        .data;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].message_id, receipt.data);
    assert_eq!(records[0].payment, 150_000);
    assert_eq!(session.contract_balance(&IGP_ID).unwrap(), 150_000);
    assert_eq!(
        records[0].gas_limit, 150_000,
        "all quoted destination gas must be credited"
    );
    let event = receipt
        .events
        .iter()
        .find(|event| event.source == IGP_ID && event.topic == events::GasPayment::TOPIC)
        .expect("authenticated payment must emit its gas credit");
    let decoded = HyperlaneDataDriver
        .decode_event(&event.topic, &event.data)
        .unwrap();
    // The data driver encodes u64 as decimal strings so JavaScript keeps them exact.
    assert_eq!(decoded["gas_limit"].as_str(), Some("150000"));
    assert_eq!(decoded["payment"].as_str(), Some("150000"));
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            IGP_ID,
            "set_domain_gas_config",
            &(
                REMOTE_DOMAIN,
                DomainGasConfig {
                    gas_overhead: 0,
                    token_exchange_rate: 10_000_000_000,
                    gas_price: 1,
                },
            ),
        )
        .unwrap();
    let historical = session
        .direct_call::<_, GasPaymentRecord>(IGP_ID, "gas_payment_at", &(0u32,))
        .unwrap()
        .data;
    assert_eq!(
        historical.gas_limit, 150_000,
        "later oracle settings cannot change historical paid gas"
    );
}

#[test]
fn test_igp_rejects_config_with_unrepresentable_adjusted_gas() {
    let mut session = session_with_hooks_and_igp_config(vec![]);
    let result = session.call_public::<_, ()>(
        &OWNER_SK,
        IGP_ID,
        "set_domain_gas_config",
        &(
            REMOTE_DOMAIN,
            DomainGasConfig {
                gas_overhead: u64::MAX,
                token_exchange_rate: 1,
                gas_price: 1,
            },
        ),
    );
    assert_contract_panic(result, "IGP: configured adjusted gas exceeds u64");
}

// =============================================================================
// Red team: verification (forger persona)
//
// The forger runs a relayer, holds at most `threshold - 1` enrolled validator
// keys plus any number of outsider keys, and may call ValidatorAnnounce.
// Tests named `redteam_*` assert the secure or Solidity-compatible behaviour;
// `redteam_held_*` tests document attacks that the current code resists.
// =============================================================================

/// secp256k1 group order `n`, big-endian.
const REDTEAM_SECP256K1_N: [u8; 32] = [
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE,
    0xBA, 0xAE, 0xDC, 0xE6, 0xAF, 0x48, 0xA0, 0x3B, 0xBF, 0xD2, 0x5E, 0x8C, 0xD0, 0x36, 0x41, 0x41,
];

/// Turn a canonical low-s `r || s || v` signature into its high-s twin
/// `r || n - s || v'`. Both recover the same signer; OpenZeppelin's
/// `ECDSA.recover` (used by Solidity `AbstractMultisigIsm` and
/// `ValidatorAnnounce`) rejects the high-s form.
fn redteam_malleate_high_s(signature: &[u8]) -> Vec<u8> {
    assert_eq!(signature.len(), 65);
    let mut out = signature.to_vec();
    let mut borrow = 0i16;
    for i in (0..32).rev() {
        let mut diff = i16::from(REDTEAM_SECP256K1_N[i]) - i16::from(signature[32 + i]) - borrow;
        if diff < 0 {
            diff += 256;
            borrow = 1;
        } else {
            borrow = 0;
        }
        out[32 + i] = u8::try_from(diff).unwrap();
    }
    assert_eq!(borrow, 0, "s must be below n");
    // Negating s negates R, so the recovery parity flips.
    out[64] = match signature[64] {
        27 => 28,
        28 => 27,
        0 => 1,
        1 => 0,
        v => v,
    };
    out
}

/// A Dusk deployment shaped like `dusk-tx deploy-hyperlane --default-ism
/// messageIdMultisig`: the Mailbox defaults to a real 2-of-3 multisig ISM, and
/// the TestMock (NullISM) and TestRecipient fixtures are deployed alongside it.
fn redteam_session_with_multisig_default_ism() -> (TestSession, Vec<(EthAddress, SecpSecretKey)>) {
    let signers = checkpoint_signers();
    let validators: Vec<EthAddress> = signers[..3].iter().map(|(address, _)| *address).collect();
    let mut session = session_with_multisig_ism(*OWNER_ID, validators, 2);
    session
        .deploy(
            TEST_MOCK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_MOCK_ID),
        )
        .unwrap();
    session
        .deploy(
            TEST_RECIPIENT_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(TEST_RECIPIENT_ID),
        )
        .unwrap();
    session
        .deploy(
            MERKLE_TREE_HOOK_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID,))
                .contract_id(MERKLE_TREE_HOOK_ID),
        )
        .unwrap();
    session
        .deploy(
            MAILBOX_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    LOCAL_DOMAIN,
                    *OWNER_ID,
                    ISM_MULTISIG_ID,
                    TEST_MOCK_ID,
                    MERKLE_TREE_HOOK_ID,
                ))
                .contract_id(MAILBOX_ID),
        )
        .unwrap();
    (session, signers)
}

/// FINDING: the deployed TestRecipient lets anyone choose its ISM, and the
/// deploy always ships a NullISM (TestMock). A forger with no quorum gets the
/// canonical Mailbox to accept, mark delivered and emit `Process` for a
/// message no validator signed. Solidity's TestRecipient gates
/// `setInterchainSecurityModule` with `onlyOwner`.
#[test]
fn redteam_forger_cannot_downgrade_deployed_recipient_ism() {
    let (mut session, signers) = redteam_session_with_multisig_default_ism();
    let forged = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        [0xEE; 32],
        LOCAL_DOMAIN,
        TEST_RECIPIENT_ID.to_bytes(),
        b"never signed by the validator quorum",
    );
    let id = message::id(&forged);

    // Baseline: with the real default ISM the forger's best metadata
    // (threshold - 1 enrolled signer + an outsider) and empty metadata fail.
    let partial = signed_checkpoint_metadata(&forged, &[&signers[0].1, &signers[3].1]);
    for metadata in [partial, Vec::new()] {
        assert!(session
            .call_public::<_, ()>(
                &RELAYER_SK,
                MAILBOX_ID,
                "process",
                &(metadata, forged.clone())
            )
            .is_err());
    }

    // Attack: the forger (not the deployer, not any owner) points the deployed
    // TestRecipient at the deployed NullISM, then relays with empty metadata.
    let downgrade = session.call_public::<_, ()>(
        &RELAYER_SK,
        TEST_RECIPIENT_ID,
        "set_interchain_security_module",
        &(TEST_MOCK_ID,),
    );
    let process = session.call_public::<_, ()>(
        &RELAYER_SK,
        MAILBOX_ID,
        "process",
        &(Vec::<u8>::new(), forged.clone()),
    );
    let delivered = session
        .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(id,))
        .unwrap()
        .data;
    assert!(
        process.is_err() && !delivered,
        "unsigned message accepted by the Mailbox: ism downgrade ok={}, process ok={}, delivered={delivered}",
        downgrade.is_ok(),
        process.is_ok(),
    );
}

/// FINDING (compatibility): a high-s twin of a validator signature verifies on
/// Dusk. OpenZeppelin `ECDSA.recover` reverts with "invalid signature 's'
/// value", so Dusk accepts metadata the Solidity ISM rejects.
#[test]
fn redteam_multisig_rejects_high_s_signature() {
    let signers = checkpoint_signers();
    let mut session = session_with_multisig_ism(*OWNER_ID, vec![signers[0].0], 1);
    let message = sample_encoded_message(TEST_RECIPIENT_ID);
    let metadata = signed_checkpoint_metadata(&message, &[&signers[0].1]);
    assert!(
        session
            .direct_call::<_, bool>(
                ISM_MULTISIG_ID,
                "verify",
                &(metadata.clone(), message.clone())
            )
            .unwrap()
            .data
    );

    let mut malleated = metadata.clone();
    let high_s = redteam_malleate_high_s(&metadata[68..133]);
    malleated[68..133].copy_from_slice(&high_s);
    let result = session.direct_call::<_, bool>(ISM_MULTISIG_ID, "verify", &(malleated, message));
    assert!(
        result.is_err(),
        "high-s signature accepted by MultisigISM (OpenZeppelin ECDSA.recover rejects s > n/2)"
    );
}

/// FINDING (compatibility): `v` of 0 or 1 verifies on Dusk. OpenZeppelin
/// `ECDSA.recover` only accepts 27 or 28.
#[test]
fn redteam_multisig_rejects_raw_recovery_id_v() {
    let signers = checkpoint_signers();
    let mut session = session_with_multisig_ism(*OWNER_ID, vec![signers[0].0], 1);
    let message = sample_encoded_message(TEST_RECIPIENT_ID);
    let mut metadata = signed_checkpoint_metadata(&message, &[&signers[0].1]);
    metadata[132] -= 27;
    let result = session.direct_call::<_, bool>(ISM_MULTISIG_ID, "verify", &(metadata, message));
    assert!(
        result.is_err(),
        "v in {{0,1}} accepted by MultisigISM (OpenZeppelin ECDSA.recover requires 27 or 28)"
    );
}

/// FINDING (compatibility): ValidatorAnnounce shares the permissive recovery.
#[test]
fn redteam_validator_announce_rejects_high_s_signature() {
    let mut session = session_with_validator_announce();
    let location = alloc::string::String::from("s3://validator/checkpoints");
    let (validator, signature) = validator_announcement(&location);
    let result = session.call_public::<_, bool>(
        &OWNER_SK,
        VALIDATOR_ANNOUNCE_ID,
        "announce",
        &(validator, location, redteam_malleate_high_s(&signature)),
    );
    assert!(
        result.is_err(),
        "high-s announcement signature accepted (OpenZeppelin ECDSA.recover rejects s > n/2)"
    );
}

/// HELD: one enrolled key presented twice under different encodings (low-s
/// and high-s, or v=27 and v=0) is still one signer.
#[test]
fn redteam_held_same_signer_two_encodings_not_double_counted() {
    let signers = checkpoint_signers();
    let validators = signers[..3].iter().map(|(address, _)| *address).collect();
    let mut session = session_with_multisig_ism(*OWNER_ID, validators, 2);
    let message = sample_encoded_message(TEST_RECIPIENT_ID);
    let single = signed_checkpoint_metadata(&message, &[&signers[0].1]);
    let signature = single[68..133].to_vec();

    let mut raw_v = signature.clone();
    raw_v[64] -= 27;
    for twin in [redteam_malleate_high_s(&signature), raw_v] {
        let mut metadata = single.clone();
        metadata.extend_from_slice(&twin);
        // Non-canonical twins are now rejected outright, before counting.
        assert_contract_panic(
            session.direct_call::<_, bool>(ISM_MULTISIG_ID, "verify", &(metadata, message.clone())),
            "MultisigISM: non-canonical signature",
        );
    }
}

/// HELD: only the configured caller can insert leaves into the tree that
/// validators sign.
#[test]
fn redteam_held_merkle_tree_hook_rejects_foreign_insert() {
    let mut s = HyperlaneSession::new();
    let forged = sample_encoded_message(TEST_RECIPIENT_ID);
    assert_contract_panic(
        s.session.call_public::<_, ()>(
            &RELAYER_SK,
            MERKLE_TREE_HOOK_ID,
            "post_dispatch",
            &(Vec::<u8>::new(), forged.clone()),
        ),
        "MerkleTreeHook: caller is not mailbox",
    );
    // Selecting the required hook as the custom hook is also refused, so a
    // dispatch cannot insert its own leaf twice.
    let result = s.session.call_public::<_, MessageId>(
        &RELAYER_SK,
        MAILBOX_ID,
        "dispatch",
        &(
            REMOTE_DOMAIN,
            [0x01u8; 32],
            b"x".to_vec(),
            Vec::<u8>::new(),
            MERKLE_TREE_HOOK_ID,
        ),
    );
    assert_contract_panic(result, "Mailbox: selected hook cannot equal required hook");
    assert_eq!(s.merkle_count(), 0);
}

/// HELD: an announcement signed for one (domain, Mailbox) does not verify on a
/// ValidatorAnnounce bound to another domain or Mailbox.
#[test]
fn redteam_held_announcement_bound_to_domain_and_mailbox() {
    let mut session = session_with_validator_announce();
    let other_mailbox_va = ContractId::from_bytes([0x71; 32]);
    let other_domain_va = ContractId::from_bytes([0x72; 32]);
    for (id, init) in [
        (other_mailbox_va, (LOCAL_DOMAIN, TEST_MOCK_ID)),
        (other_domain_va, (REMOTE_DOMAIN, MAILBOX_ID)),
    ] {
        session
            .deploy(
                VALIDATOR_ANNOUNCE_BYTECODE,
                dusk_vm::ContractData::builder()
                    .owner(DEPLOYER)
                    .init_arg(&init)
                    .contract_id(id),
            )
            .unwrap();
    }
    let location = alloc::string::String::from("s3://validator/checkpoints");
    let (validator, signature) = validator_announcement(&location);
    for id in [other_mailbox_va, other_domain_va] {
        assert_contract_panic(
            session.call_public::<_, bool>(
                &OWNER_SK,
                id,
                "announce",
                &(validator, location.clone(), signature.clone()),
            ),
            "ValidatorAnnounce: invalid signature",
        );
    }
    session
        .call_public::<_, bool>(
            &OWNER_SK,
            VALIDATOR_ANNOUNCE_ID,
            "announce",
            &(validator, location.clone(), signature.clone()),
        )
        .unwrap();
    assert_contract_panic(
        session.call_public::<_, bool>(
            &OWNER_SK,
            VALIDATOR_ANNOUNCE_ID,
            "announce",
            &(validator, location, redteam_malleate_high_s(&signature)),
        ),
        "ValidatorAnnounce: replay",
    );
}

// =============================================================================
// Red team: economics
// =============================================================================

/// Encode a Hyperlane TokenMessage body exactly as an EVM `TokenRouter` does:
/// `recipient (32) || uint256 amount (32, big-endian)`.
fn redteam_evm_token_body(recipient: H256, amount: u128) -> Vec<u8> {
    let mut body = recipient.to_vec();
    let mut word = [0u8; 32];
    word[16..].copy_from_slice(&amount.to_be_bytes());
    body.extend_from_slice(&word);
    body
}

fn redteam_inbound_synthetic(
    session: &mut TestSession,
    remote_router: H256,
    nonce: u32,
    recipient: H256,
    amount: u128,
) -> (MessageId, Result<CallReceipt<()>, ContractError>) {
    let encoded = message::encode(
        VERSION,
        nonce,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        WARP_DRC20_ID.to_bytes(),
        &redteam_evm_token_body(recipient, amount),
    );
    let id = message::id(&encoded);
    let result = session.direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded));
    (id, result)
}

/// One whole token at the 18 decimals the synthetic route and its EVM
/// HypERC20 counterpart use (`TOKEN_DECIMALS=18`, scale 1/1 in deploy.sh).
const REDTEAM_WAD: u128 = 1_000_000_000_000_000_000;

/// An EVM user burns 20 tokens on the remote HypERC20 router. The Dusk side
/// must be able to deliver them; otherwise the burned value is unrecoverable
/// (no refund path, the message stays undeliverable forever).
#[test]
#[ignore = "known limitation (docs/security.md): u64 amounts cap an 18-decimal synthetic at ~18.4 tokens; synthetic routes must carry at most 9 decimals"]
fn redteam_synthetic_inbound_above_u64_is_deliverable() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();
    register_by_claim(
        &mut session,
        WARP_DRC20_ID,
        remote_router,
        100,
        &OWNER_SK,
        1,
    );
    let recipient = message::keccak256(&OWNER_PK.to_bytes());

    let amount = 20 * REDTEAM_WAD; // 2e19 > u64::MAX (~1.8447e19)
    let (id, result) = redteam_inbound_synthetic(&mut session, remote_router, 0, recipient, amount);
    if let Err(error) = &result {
        panic!("20 tokens burned on EVM cannot be delivered on Dusk: {error}");
    }
    assert!(
        session
            .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(id,))
            .unwrap()
            .data
    );
}

/// An attacker with ~18.4 tokens parks them in an escrow key that no BLS key
/// or contract can ever claim. The route's u64 supply-plus-pending capacity
/// is then exhausted, so every later EVM->Dusk transfer from other users is
/// burned on EVM and can never be delivered.
#[test]
#[ignore = "known limitation (docs/security.md): u64 amounts cap an 18-decimal synthetic at ~18.4 tokens; synthetic routes must carry at most 9 decimals"]
fn redteam_synthetic_supply_cap_cannot_freeze_inbound() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();
    register_by_claim(
        &mut session,
        WARP_DRC20_ID,
        remote_router,
        100,
        &OWNER_SK,
        1,
    );

    let unclaimable_sink: H256 = [0xEE; 32];
    let attacker_amount = 18 * REDTEAM_WAD + 400_000_000_000_000_000; // 18.4 tokens
    let (_, result) = redteam_inbound_synthetic(
        &mut session,
        remote_router,
        0,
        unclaimable_sink,
        attacker_amount,
    );
    result.expect("attacker's 18.4-token transfer fits in u64 and is escrowed");

    let victim = message::keccak256(&OWNER_PK.to_bytes());
    let victim_amount = REDTEAM_WAD / 10; // 0.1 token
    let (id, result) =
        redteam_inbound_synthetic(&mut session, remote_router, 1, victim, victim_amount);
    if let Err(error) = &result {
        panic!("victim's 0.1-token transfer is frozen by the attacker's escrow: {error}");
    }
    assert!(
        session
            .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(id,))
            .unwrap()
            .data
    );
}

/// WarpDrc20 accepts the canonical `Phoenix` principal as a transfer target,
/// but unlike the canonical DRC20 it has no signed-authorization path, and
/// `sender_account()` never resolves to a Phoenix principal. Tokens sent there
/// can never be transferred, approved, or bridged back.
#[test]
fn redteam_synthetic_rejects_unspendable_phoenix_recipient() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();
    register_by_claim(
        &mut session,
        WARP_DRC20_ID,
        remote_router,
        100,
        &OWNER_SK,
        1,
    );
    let holder = message::keccak256(&OWNER_PK.to_bytes());
    let amount = 1_000_000u64;
    let (_, result) =
        redteam_inbound_synthetic(&mut session, remote_router, 0, holder, u128::from(amount));
    result.expect("mint to the registered holder should succeed");

    let phoenix = Drc20Account::Phoenix([0x07; 32]);
    let result = session.call_public::<_, ()>(
        &OWNER_SK,
        WARP_DRC20_ID,
        "transfer",
        &hyperlane_dusk_types::drc20::TransferCall {
            to: phoenix,
            amount,
        },
    );
    let stranded = warp_drc20_balance_of(&mut session, phoenix);
    assert!(
        result.is_err() && stranded == 0,
        "WarpDrc20 moved {stranded} units to a Phoenix principal that no call can ever spend"
    );
}

// =============================================================================
// Red team: attacker-controlled callbacks (hook, recipient, nominated ISM,
// native escrow claimant, route-nominated hook)
// =============================================================================

const REDTEAM_ATTACKER_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_redteam_attacker.wasm"
);
const REDTEAM_ID: ContractId = ContractId::from_bytes([40; 32]);

/// Callback points understood by the red-team attacker contract.
const RT_QUOTE: u8 = 0;
const RT_POST: u8 = 1;
const RT_PAYMENT: u8 = 2;
const RT_ISM_LOOKUP: u8 = 3;
const RT_VERIFY: u8 = 4;
const RT_HANDLE: u8 = 5;
const RT_NATIVE_PENDING: u8 = 6;

type RtAction = (ContractId, String, Vec<u8>);

fn rt_action<A>(target: ContractId, fn_name: &str, args: &A) -> RtAction
where
    A: for<'b> rkyv::Serialize<dusk_core::abi::StandardBufSerializer<'b>>,
    A::Archived: for<'b> bytecheck::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
{
    (
        target,
        String::from(fn_name),
        test_session::rkyv_serialize(args),
    )
}

fn rt_deploy(session: &mut TestSession) {
    rt_deploy_at(session, REDTEAM_ID, Vec::new())
        .expect("Deploying the red-team attacker should succeed");
}

/// Deploys a red-team attacker at `id` whose `init` makes `actions`, any
/// failure failing the deployment.
fn rt_deploy_at(
    session: &mut TestSession,
    id: ContractId,
    actions: Vec<RtAction>,
) -> Result<ContractId, VMError> {
    session.deploy(
        REDTEAM_ATTACKER_BYTECODE,
        dusk_vm::ContractData::builder()
            .owner(DEPLOYER)
            .init_arg(&actions)
            .contract_id(id),
    )
}

fn rt_configure(session: &mut TestSession, point: u8, actions: Vec<RtAction>, panic_after: bool) {
    session
        .direct_call::<_, ()>(REDTEAM_ID, "configure", &(point, actions, panic_after))
        .expect("attacker configuration should succeed");
}

fn rt_set_returns(session: &mut TestSession, quote_fee: u64, ism: ContractId) {
    session
        .direct_call::<_, ()>(REDTEAM_ID, "set_returns", &(quote_fee, ism))
        .expect("attacker return configuration should succeed");
}

fn rt_results(session: &mut TestSession) -> Vec<bool> {
    session
        .direct_call::<_, Vec<bool>>(REDTEAM_ID, "results", &())
        .expect("results query should succeed")
        .data
}

/// Returns `(handled, posts, received)`.
fn rt_counters(session: &mut TestSession) -> (u32, u32, u64) {
    session
        .direct_call::<_, (u32, u32, u64)>(REDTEAM_ID, "counters", &())
        .expect("counters query should succeed")
        .data
}

fn rt_u32(session: &mut TestSession, contract: ContractId, fn_name: &str) -> u32 {
    session
        .direct_call::<_, u32>(contract, fn_name, &())
        .unwrap_or_else(|e| panic!("{fn_name} query failed: {e}"))
        .data
}

fn rt_delivered(session: &mut TestSession, id: MessageId) -> bool {
    session
        .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(id,))
        .expect("delivered query should succeed")
        .data
}

fn rt_fee_credit(session: &mut TestSession, payer: H256) -> u64 {
    session
        .direct_call::<_, u64>(MAILBOX_ID, "fee_credit", &(payer,))
        .expect("fee_credit query should succeed")
        .data
}

/// A recipient's ISM lookup may fail and the Mailbox then falls back to the
/// default ISM. The attacker uses that tolerated failure for speculative work:
/// a nested dispatch, a nested delivery of someone else's message and a
/// same-message replay, then panics. The deliveries are refused anyway, since
/// a contract can't call `process`. None of it may survive, and the honest
/// message must remain deliverable afterwards.
#[test]
fn redteam_speculative_ism_lookup_leaves_no_partial_state() {
    let mut s = HyperlaneSession::new();
    rt_deploy(&mut s.session);

    let attacker_msg = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        LOCAL_DOMAIN,
        REDTEAM_ID.to_bytes(),
        b"attacker inbound",
    );
    let honest_msg = message::encode(
        VERSION,
        1,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        LOCAL_DOMAIN,
        TEST_RECIPIENT_ID.to_bytes(),
        b"honest inbound",
    );
    rt_configure(
        &mut s.session,
        RT_ISM_LOOKUP,
        vec![
            rt_action(
                MAILBOX_ID,
                "dispatch_default",
                &(REMOTE_DOMAIN, [0xEEu8; 32], b"speculative".to_vec()),
            ),
            rt_action(
                MAILBOX_ID,
                "process",
                &(Vec::<u8>::new(), honest_msg.clone()),
            ),
            rt_action(
                MAILBOX_ID,
                "process",
                &(Vec::<u8>::new(), attacker_msg.clone()),
            ),
        ],
        true,
    );

    let receipt = s
        .mailbox_process_via_tx(Vec::new(), attacker_msg.clone())
        .expect("outer delivery falls back to the default ISM");

    assert_eq!(rt_counters(&mut s.session).0, 1, "delivered exactly once");
    assert_eq!(s.mailbox_nonce(), 0, "speculative dispatch must roll back");
    assert_eq!(
        s.merkle_count(),
        0,
        "speculative tree insert must roll back"
    );
    assert!(!rt_delivered(&mut s.session, message::id(&honest_msg)));
    assert!(rt_delivered(&mut s.session, message::id(&attacker_msg)));
    assert_eq!(s.recipient_handled_count(), 0);
    assert_eq!(delivered_ids(&receipt), vec![message::id(&attacker_msg)]);
    // Piecrust keeps the rolled-back events in the receipt but flags them;
    // the Dusk agent skips flagged events and cross-checks contract state.
    for event in &receipt.events {
        if event.topic == events::Dispatch::TOPIC
            || event.topic == events::DispatchId::TOPIC
            || event.topic == events::InsertedIntoTree::TOPIC
        {
            assert!(
                event.reverted,
                "speculative {} must be flagged",
                event.topic
            );
        }
    }
    assert_eq!(
        receipt
            .events
            .iter()
            .filter(|event| event.topic == events::ProcessId::TOPIC && !event.reverted)
            .count(),
        1
    );

    let receipt = s
        .mailbox_process_via_tx(Vec::new(), honest_msg.clone())
        .expect("the honest message must stay deliverable");
    assert_eq!(s.recipient_handled_count(), 1);
    assert_eq!(delivered_ids(&receipt), vec![message::id(&honest_msg)]);
}

/// A recipient nominates itself as ISM and replays the message from both
/// `verify` and `handle`, the HIGH-6 case. Both are refused: a contract can't
/// call `process`, and the delivery is reserved before any call anyway.
#[test]
fn redteam_nominated_ism_and_handle_replay_rejected() {
    let mut s = HyperlaneSession::new();
    rt_deploy(&mut s.session);
    rt_set_returns(&mut s.session, 0, REDTEAM_ID);

    let msg = message::encode(
        VERSION,
        7,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        LOCAL_DOMAIN,
        REDTEAM_ID.to_bytes(),
        b"replay me",
    );
    let replay = rt_action(MAILBOX_ID, "process", &(Vec::<u8>::new(), msg.clone()));
    rt_configure(&mut s.session, RT_VERIFY, vec![replay.clone()], false);
    rt_configure(&mut s.session, RT_HANDLE, vec![replay], false);

    let receipt = s
        .mailbox_process_via_tx(Vec::new(), msg.clone())
        .expect("outer delivery succeeds");

    assert_eq!(rt_results(&mut s.session), vec![false, false]);
    assert_eq!(rt_counters(&mut s.session).0, 1);
    assert_eq!(delivered_ids(&receipt), vec![message::id(&msg)]);
}

/// The attacker contract is both the dispatch sender and its own hook, and
/// tries to withdraw its credit from each hook callback. If the Mailbox read
/// the credit before the quote callbacks, it would pay the hooks out of other
/// payers' pooled DUSK.
#[test]
fn redteam_hook_callbacks_cannot_double_spend_dispatch_credit() {
    // Production-like hooks: required = Aggregation(Merkle, ProtocolFee 1000),
    // default = IGP. The helper also funds 100_000_000 of TestRecipient credit,
    // which sits in the same Mailbox pool.
    let mut session = session_with_hooks();
    rt_deploy(&mut session);
    // The attacker funds exactly what one dispatch costs, the least a payer
    // with no credit can fund.
    let hook_fee = MIN_NEW_PAYER_CREDIT - 1000;
    let total_fee = 1000 + hook_fee;
    rt_set_returns(&mut session, hook_fee, ContractId::from_bytes([0; 32]));
    session
        .call_public_with_deposit::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(REDTEAM_ID.to_bytes(), total_fee),
            total_fee,
        )
        .expect("funding the attacker's own credit should succeed");
    let pool_before = session.contract_balance(&MAILBOX_ID).unwrap();
    let relayer_before = session.account(&RELAYER_PK).unwrap().balance;

    let dispatch = rt_action(
        MAILBOX_ID,
        "dispatch",
        &(
            REMOTE_DOMAIN,
            [0xA1u8; 32],
            b"pay with withdrawn credit".to_vec(),
            Vec::<u8>::new(),
            REDTEAM_ID,
        ),
    );
    let withdraw_all = rt_action(
        MAILBOX_ID,
        "withdraw_dispatch_credit",
        &(*RELAYER_PK, total_fee),
    );
    let withdraw_one = rt_action(MAILBOX_ID, "withdraw_dispatch_credit", &(*RELAYER_PK, 1u64));

    // 1. Withdraw everything during the quote, before the Mailbox charges.
    rt_configure(&mut session, RT_QUOTE, vec![withdraw_all], false);
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            REDTEAM_ID,
            "run",
            &(vec![dispatch.clone()], false),
        )
        .expect("attacker transaction itself succeeds");
    assert_eq!(rt_results(&mut session), vec![false], "dispatch must fail");
    assert_eq!(rt_u32(&mut session, MAILBOX_ID, "nonce"), 0);
    assert_eq!(
        rt_fee_credit(&mut session, REDTEAM_ID.to_bytes()),
        total_fee
    );
    assert_eq!(session.contract_balance(&MAILBOX_ID).unwrap(), pool_before);
    assert_eq!(
        session.account(&RELAYER_PK).unwrap().balance,
        relayer_before
    );

    // 2. Withdraw again from post-dispatch and payment, after the charge.
    rt_configure(&mut session, RT_QUOTE, Vec::new(), false);
    rt_configure(&mut session, RT_POST, vec![withdraw_one.clone()], false);
    rt_configure(&mut session, RT_PAYMENT, vec![withdraw_one], false);
    session
        .call_public::<_, ()>(&OWNER_SK, REDTEAM_ID, "run", &(vec![dispatch], true))
        .expect("honest-priced dispatch succeeds");
    assert_eq!(rt_results(&mut session), vec![false, false, false, true]);
    assert_eq!(rt_u32(&mut session, MAILBOX_ID, "nonce"), 1);
    assert_eq!(rt_u32(&mut session, MERKLE_TREE_HOOK_ID, "count"), 1);
    assert_eq!(rt_fee_credit(&mut session, REDTEAM_ID.to_bytes()), 0);
    assert_eq!(
        session.contract_balance(&MAILBOX_ID).unwrap(),
        pool_before - total_fee,
        "only the attacker's own credit may leave the pool"
    );
    assert_eq!(
        rt_fee_credit(&mut session, TEST_RECIPIENT_ID.to_bytes()),
        100_000_000
    );
    assert_eq!(
        session.account(&RELAYER_PK).unwrap().balance,
        relayer_before
    );
    assert_eq!(rt_counters(&mut session).2, hook_fee);
    assert_eq!(session.contract_balance(&PROTOCOL_FEE_ID).unwrap(), 1000);

    // The dispatch spent all of the attacker's credit, which leaves no
    // entry: funding it again takes the minimum again.
    assert_contract_panic(
        session.call_public_with_deposit::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(REDTEAM_ID.to_bytes(), 1u64),
            1,
        ),
        "Mailbox: a payer with no credit takes at least 0.01 DUSK",
    );
}

/// A contract claimant of native escrow re-enters the route from the
/// transfer-contract callback: a second claim, a nested inbound delivery that
/// would escrow more DUSK to itself, and a claim of that. The second claim
/// finds nothing and the delivery is refused, since a contract can't call
/// `process`. It receives exactly what was escrowed, the route's custody
/// still covers its liability, and the message is delivered later as a
/// transaction's own call.
#[test]
fn redteam_native_escrow_callback_cannot_double_release() {
    let (mut session, remote_router) = session_with_warp_native_flow();
    rt_deploy(&mut session);
    let custody = 3_000_000u64;
    let first = 1_000_000u64;
    let second = 700_000u64;
    fund_warp_native(&mut session, custody);

    let inbound = |nonce: u32, amount: u64| {
        message::encode(
            VERSION,
            nonce,
            REMOTE_DOMAIN,
            remote_router,
            LOCAL_DOMAIN,
            WARP_NATIVE_ID.to_bytes(),
            &hyperlane_dusk_types::token_message::encode(REDTEAM_ID.to_bytes(), amount),
        )
    };
    session
        .direct_call::<_, ()>(
            MAILBOX_ID,
            "process",
            &(Vec::<u8>::new(), inbound(1, first)),
        )
        .expect("first escrow");

    let claim = rt_action(WARP_NATIVE_ID, "claim_pending_contract", &());
    rt_configure(
        &mut session,
        RT_NATIVE_PENDING,
        vec![
            claim.clone(),
            rt_action(
                MAILBOX_ID,
                "process",
                &(Vec::<u8>::new(), inbound(2, second)),
            ),
            claim.clone(),
        ],
        false,
    );
    session
        .direct_call::<_, ()>(REDTEAM_ID, "run", &(vec![claim.clone()], true))
        .expect("outer claim succeeds");

    assert_eq!(rt_results(&mut session), vec![false, false, false, true]);
    assert!(!rt_delivered(
        &mut session,
        message::id(&inbound(2, second))
    ));
    assert_eq!(rt_counters(&mut session).2, first);
    assert_eq!(session.contract_balance(&REDTEAM_ID).unwrap(), first);
    assert_eq!(
        session.contract_balance(&WARP_NATIVE_ID).unwrap(),
        custody - first
    );

    // Delivered as a transaction's own call, it escrows, and the next claim
    // takes exactly that.
    rt_configure(&mut session, RT_NATIVE_PENDING, Vec::new(), false);
    session
        .call_public::<_, ()>(
            &RELAYER_SK,
            MAILBOX_ID,
            "process",
            &(Vec::<u8>::new(), inbound(2, second)),
        )
        .expect("second escrow");
    session
        .direct_call::<_, ()>(REDTEAM_ID, "run", &(vec![claim], true))
        .expect("second claim succeeds");
    assert_eq!(rt_counters(&mut session).2, first + second);
    assert_eq!(
        session.contract_balance(&REDTEAM_ID).unwrap(),
        first + second
    );
    assert_eq!(
        session.contract_balance(&WARP_NATIVE_ID).unwrap(),
        custody - first - second
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(WARP_NATIVE_ID, "pending_total", &())
            .unwrap()
            .data,
        0
    );
}

/// A route owner nominates a third-party hook. Its quote callback runs before
/// the route takes the user's deposit, re-enters `transfer_remote` with a
/// matching amount, takes the deposit and bridges it to the attacker. The
/// outer transfer must then fail, rolling back the nested one.
#[test]
fn redteam_route_hook_cannot_steal_native_deposit_during_quote() {
    let (mut session, _remote_router) = session_with_warp_native_flow();
    rt_deploy(&mut session);
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "set_hook", &(REDTEAM_ID,))
        .expect("route owner nominates the hook");
    let amount = 1_000_000_000u64; // larger than the maximum gas charge
    rt_configure(
        &mut session,
        RT_QUOTE,
        vec![rt_action(
            WARP_NATIVE_ID,
            "transfer_remote",
            &(REMOTE_DOMAIN, [0xAAu8; 32], amount),
        )],
        false,
    );
    let user_before = session.account(&OWNER_PK).unwrap().balance;

    let result = session.call_public_with_deposit::<_, MessageId>(
        &OWNER_SK,
        WARP_NATIVE_ID,
        "transfer_remote",
        &(REMOTE_DOMAIN, [0xDDu8; 32], amount),
        amount,
    );
    assert!(result.is_err(), "outer transfer must not succeed");
    assert_eq!(rt_u32(&mut session, MAILBOX_ID, "nonce"), 0);
    assert_eq!(rt_u32(&mut session, MERKLE_TREE_HOOK_ID, "count"), 0);
    assert_eq!(session.contract_balance(&WARP_NATIVE_ID).unwrap(), 0);
    assert!(
        user_before - session.account(&OWNER_PK).unwrap().balance < amount,
        "the user's deposit must be refunded"
    );
}

/// The attacker's hook tries to deliver an inbound message to itself from
/// `post_dispatch`, whose `handle` would dispatch while the outer dispatch
/// is still running. A contract can't call `process`, so the delivery is
/// refused. Nonces, stored messages and the tree stay aligned, and the
/// guard is clear afterwards. Delivered later as a transaction's own call,
/// the message's `handle` dispatches with no outer dispatch running.
#[test]
fn redteam_hook_process_during_dispatch_keeps_indexes_aligned() {
    let mut s = HyperlaneSession::new();
    rt_deploy(&mut s.session);
    let inbound = message::encode(
        VERSION,
        3,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        LOCAL_DOMAIN,
        REDTEAM_ID.to_bytes(),
        b"delivered mid-dispatch",
    );
    rt_configure(
        &mut s.session,
        RT_POST,
        vec![rt_action(
            MAILBOX_ID,
            "process",
            &(Vec::<u8>::new(), inbound.clone()),
        )],
        false,
    );
    rt_configure(
        &mut s.session,
        RT_HANDLE,
        vec![rt_action(
            MAILBOX_ID,
            "dispatch_default",
            &(REMOTE_DOMAIN, [0xEEu8; 32], b"nested".to_vec()),
        )],
        false,
    );

    let body = b"outer".to_vec();
    let receipt = s
        .session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            MAILBOX_ID,
            "dispatch",
            &(
                REMOTE_DOMAIN,
                [0xD5u8; 32],
                body.clone(),
                Vec::<u8>::new(),
                REDTEAM_ID,
            ),
        )
        .expect("outer dispatch succeeds");

    assert_eq!(rt_results(&mut s.session), vec![false]);
    assert_eq!(rt_counters(&mut s.session).0, 0);
    assert!(!rt_delivered(&mut s.session, message::id(&inbound)));
    assert_eq!(s.mailbox_nonce(), 1);
    assert_eq!(s.merkle_count(), 1);
    let stored = s.dispatched_message(&receipt, 0);
    assert_eq!(message::decode(&stored).unwrap().body, body);
    assert_eq!(receipt.data, message::id(&stored));

    s.mailbox_dispatch_via_tx(&OWNER_SK, REMOTE_DOMAIN, [0xD5u8; 32], b"later".to_vec())
        .expect("guard must be clear");
    assert_eq!(s.mailbox_nonce(), 2);
    assert_eq!(s.merkle_count(), 2);

    s.mailbox_process_via_tx(Vec::new(), inbound.clone())
        .expect("a transaction delivers the message");
    assert_eq!(rt_results(&mut s.session), vec![false, true]);
    assert_eq!(rt_counters(&mut s.session).0, 1);
    assert!(rt_delivered(&mut s.session, message::id(&inbound)));
    assert_eq!(s.mailbox_nonce(), 3);
    assert_eq!(s.merkle_count(), 3);
}

// =============================================================================
// Red team: a collateral route re-entered from its token's callback
// =============================================================================

const CALLBACK_FEE_TOKEN_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_callback_fee_token.wasm"
);
/// The collateral route built without its reentrancy guard
/// (`make warp-drc20-collateral-unguarded`), to show what the guard stops.
const UNGUARDED_COLLATERAL_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_warp_drc20_collateral_unguarded.wasm"
);
const CALLBACK_FEE_TOKEN_ID: ContractId = ContractId::from_bytes([42; 32]);
/// The attacker's callback point for the token's `tokens_to_send`.
const RT_TOKENS_TO_SEND: u8 = 7;
/// 10% of each transfer, rounded down: a transfer of 9 or less pays no fee.
const TEN_PERCENT_FEE: u16 = 1_000;
const ATTACKER_TOKENS: u64 = 1_000;
const COLLATERAL_ROUTER: H256 = [0xCC; 32];

/// A collateral route (`route_bytecode`) over the callback fee token, behind
/// a Mailbox whose hooks charge nothing, so `transfer_remote` takes no
/// deposit and a contract can call it. The attacker contract holds
/// `ATTACKER_TOKENS`, has approved the route for all of them and gets the
/// token's `tokens_to_send` callback.
fn session_with_callback_token_route(route_bytecode: &[u8], fee_bps: u16) -> TestSession {
    let mut session = TestSession::instantiate(vec![(&*OWNER_PK, INITIAL_DUSK_BALANCE)]);
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
    session
        .deploy(
            CALLBACK_FEE_TOKEN_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    fee_bps,
                    vec![(Drc20Account::Contract(REDTEAM_ID), ATTACKER_TOKENS)],
                ))
                .contract_id(CALLBACK_FEE_TOKEN_ID),
        )
        .expect("Deploying the callback fee token should succeed");
    session
        .deploy(
            route_bytecode,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(
                    CALLBACK_FEE_TOKEN_ID,
                    MAILBOX_ID,
                    *OWNER_ID,
                    vec![(REMOTE_DOMAIN, COLLATERAL_ROUTER)],
                ))
                .contract_id(WARP_DRC20_COLLATERAL_ID),
        )
        .expect("Deploying the collateral route should succeed");
    rt_deploy(&mut session);
    let approve = rt_action(
        CALLBACK_FEE_TOKEN_ID,
        "approve",
        &Drc20ApproveCall {
            spender: Drc20Account::Contract(WARP_DRC20_COLLATERAL_ID),
            amount: ATTACKER_TOKENS,
        },
    );
    let register_hook = rt_action(CALLBACK_FEE_TOKEN_ID, "register_send_hook", &());
    session
        .direct_call::<_, ()>(REDTEAM_ID, "run", &(vec![approve, register_hook], true))
        .expect("the attacker approves the route and registers its token hook");
    session
}

fn callback_token_balance(session: &mut TestSession, account: Drc20Account) -> u64 {
    session
        .direct_call::<_, u64>(
            CALLBACK_FEE_TOKEN_ID,
            "balance_of",
            &Drc20BalanceOf { account },
        )
        .expect("balance_of should succeed")
        .data
}

fn collateral_transfer(amount: u64) -> RtAction {
    rt_action(
        WARP_DRC20_COLLATERAL_ID,
        "transfer_remote",
        &(REMOTE_DOMAIN, [0xAAu8; 32], amount),
    )
}

/// The attack: the attacker sends 100 through the route, and from the
/// token's callback, while the route's `transfer_from` is still running,
/// sends 9 and then 1 more. Those two pay no fee, and land in custody
/// between the outer transfer's before and after balances, where they make
/// up exactly the outer transfer's fee of 10.
fn attempt_fee_token_reentry(session: &mut TestSession) -> Result<CallReceipt<()>, ContractError> {
    rt_configure(
        session,
        RT_TOKENS_TO_SEND,
        vec![collateral_transfer(9), collateral_transfer(1)],
        false,
    );
    session.direct_call::<_, ()>(REDTEAM_ID, "run", &(vec![collateral_transfer(100)], true))
}

/// The amounts of the token messages a receipt's `Dispatch` events carry:
/// what the remote route will mint.
fn dispatched_token_amounts<R>(receipt: &CallReceipt<R>) -> Vec<u64> {
    receipt
        .events
        .iter()
        .filter(|e| e.source == MAILBOX_ID && e.topic == events::Dispatch::TOPIC)
        .map(|e| {
            let dispatched = test_session::rkyv_deserialize::<events::Dispatch>(&e.data);
            let decoded = message::decode(&dispatched.message).expect("dispatched message");
            token_message::decode(&decoded.body)
                .expect("dispatched token message")
                .amount
        })
        .collect()
}

/// Without the guard, the attack works: the route dispatches 110 tokens'
/// worth of remote mints and holds 100.
#[test]
fn redteam_unguarded_collateral_route_mints_unbacked_tokens_on_fee_token_reentry() {
    let mut session =
        session_with_callback_token_route(UNGUARDED_COLLATERAL_BYTECODE, TEN_PERCENT_FEE);
    let custody = Drc20Account::Contract(WARP_DRC20_COLLATERAL_ID);

    let receipt = attempt_fee_token_reentry(&mut session)
        .expect("without the guard, the nested transfers satisfy both custody checks");

    // The session's approval and hook registration, the two nested
    // transfers, then the outer one.
    assert_eq!(rt_results(&mut session), vec![true; 5]);
    assert_eq!(dispatched_token_amounts(&receipt), vec![9, 1, 100]);
    assert_eq!(rt_u32(&mut session, MAILBOX_ID, "nonce"), 3);
    assert_eq!(callback_token_balance(&mut session, custody), 100);
    assert_eq!(
        callback_token_balance(&mut session, Drc20Account::Contract(REDTEAM_ID)),
        ATTACKER_TOKENS - 110
    );
}

/// With the guard, the nested transfers are refused, the outer transfer gets
/// 90 of its 100 into custody, and its custody check reverts the whole call.
/// The guard is released afterwards.
#[test]
fn redteam_collateral_route_rejects_fee_token_reentry() {
    let mut session =
        session_with_callback_token_route(WARP_DRC20_COLLATERAL_BYTECODE, TEN_PERCENT_FEE);
    let custody = Drc20Account::Contract(WARP_DRC20_COLLATERAL_ID);

    assert_contract_panic_contains(
        attempt_fee_token_reentry(&mut session),
        "WarpCollateral: transfer_from did not deliver exact collateral",
    );
    assert_eq!(rt_u32(&mut session, MAILBOX_ID, "nonce"), 0);
    assert_eq!(callback_token_balance(&mut session, custody), 0);
    assert_eq!(
        callback_token_balance(&mut session, Drc20Account::Contract(REDTEAM_ID)),
        ATTACKER_TOKENS
    );
    assert_eq!(
        session
            .direct_call::<_, u64>(CALLBACK_FEE_TOKEN_ID, "total_supply", &())
            .expect("total_supply should succeed")
            .data,
        ATTACKER_TOKENS
    );

    // A transfer small enough to pay no fee still works.
    rt_configure(&mut session, RT_TOKENS_TO_SEND, Vec::new(), false);
    let receipt = session
        .direct_call::<_, ()>(REDTEAM_ID, "run", &(vec![collateral_transfer(9)], true))
        .expect("the guard was released with the failed call");
    assert_eq!(dispatched_token_amounts(&receipt), vec![9]);
    assert_eq!(callback_token_balance(&mut session, custody), 9);
}

/// From the token's callback during an honest `transfer_remote`, the
/// attacker calls each entry point that takes the guard: `transfer_remote`,
/// `handle` (through the Mailbox's `process`, which refuses a contract
/// caller before the guard does), `claim_pending_contract` and
/// `claim_pending_for`. Every nested call is refused while the outer
/// transfer completes, and each of them works on its own afterwards, the
/// delivery as a call of its own.
#[test]
fn test_collateral_route_guard_refuses_nested_transfer_handle_and_claims() {
    let mut session = session_with_callback_token_route(WARP_DRC20_COLLATERAL_BYTECODE, 0);
    let route = WARP_DRC20_COLLATERAL_ID;
    let custody = Drc20Account::Contract(route);
    let escrowed = 50u64;

    // Custody to pay from, and escrow for the attacker contract and for an
    // account that hasn't registered.
    session
        .direct_call::<_, ()>(REDTEAM_ID, "run", &(vec![collateral_transfer(500)], true))
        .expect("an honest transfer locks custody");
    for (nonce, recipient) in [(0, REDTEAM_ID.to_bytes()), (1, *FRESH_ID)] {
        let inbound = inbound_transfer(route, COLLATERAL_ROUTER, nonce, recipient, escrowed);
        session
            .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), inbound))
            .expect("inbound transfer to an unregistered recipient goes to escrow");
    }
    let pending_total = session
        .direct_call::<_, u64>(route, "pending_total", &())
        .expect("pending_total should succeed")
        .data;
    assert_eq!(pending_total, 2 * escrowed);

    let late = inbound_transfer(route, COLLATERAL_ROUTER, 2, *FRESH_ID, escrowed);
    let nested = vec![
        collateral_transfer(10),
        rt_action(MAILBOX_ID, "process", &(Vec::<u8>::new(), late.clone())),
        rt_action(route, "claim_pending_contract", &()),
        rt_action(route, "claim_pending_for", &(*FRESH_PK,)),
    ];
    rt_configure(&mut session, RT_TOKENS_TO_SEND, nested.clone(), false);
    let receipt = session
        .direct_call::<_, ()>(REDTEAM_ID, "run", &(vec![collateral_transfer(100)], true))
        .expect("the outer transfer completes");

    // The session's approval and hook registration and the transfer of
    // 500, then the four nested calls, then the outer transfer.
    assert_eq!(
        rt_results(&mut session),
        vec![true, true, true, false, false, false, false, true],
        "every nested call is refused, the outer transfer succeeds"
    );
    assert_eq!(dispatched_token_amounts(&receipt), vec![100]);
    assert_eq!(callback_token_balance(&mut session, custody), 600);
    assert!(!rt_delivered(&mut session, message::id(&late)));
    assert_eq!(
        route_u64(&mut session, route, "pending_balance", *FRESH_ID),
        escrowed
    );
    assert_eq!(
        route_u64(
            &mut session,
            route,
            "pending_balance",
            REDTEAM_ID.to_bytes()
        ),
        escrowed
    );

    // On their own, the same calls go through.
    rt_configure(&mut session, RT_TOKENS_TO_SEND, Vec::new(), false);
    session
        .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), late.clone()))
        .expect("the delivery works as a call of its own");
    let mut nested = nested;
    nested.remove(1);
    session
        .direct_call::<_, ()>(REDTEAM_ID, "run", &(nested, true))
        .expect("each call works outside another route call");
    assert!(rt_delivered(&mut session, message::id(&late)));
    assert_eq!(
        route_u64(&mut session, route, "pending_balance", *FRESH_ID),
        0
    );
    assert_eq!(
        route_u64(
            &mut session,
            route,
            "pending_balance",
            REDTEAM_ID.to_bytes()
        ),
        0
    );
    assert_eq!(
        callback_token_balance(&mut session, Drc20Account::moonlight(&FRESH_PK)),
        2 * escrowed
    );
}

// =============================================================================
// Sponsored escrow claims, bounded recipient ISM lookup, Mailbox memory
// =============================================================================

/// A recipient that holds no DUSK at all.
static FRESH_SK: LazyLock<AccountSecretKey> = LazyLock::new(|| {
    let mut rng = StdRng::seed_from_u64(0xF2E5_4000);
    AccountSecretKey::random(&mut rng)
});
static FRESH_PK: LazyLock<AccountPublicKey> = LazyLock::new(|| AccountPublicKey::from(&*FRESH_SK));
static FRESH_ID: LazyLock<H256> = LazyLock::new(|| message::keccak256(&FRESH_PK.to_bytes()));

const PLAIN_RECIPIENT_BYTECODE: &[u8] = include_bytes!(
    "../../target/contract/wasm32-unknown-unknown/release/hyperlane_dusk_plain_recipient.wasm"
);
const PLAIN_RECIPIENT_ID: ContractId = ContractId::from_bytes([41; 32]);

fn inbound_transfer(
    route: ContractId,
    remote_router: H256,
    nonce: u32,
    recipient: H256,
    amount: u64,
) -> Vec<u8> {
    message::encode(
        VERSION,
        nonce,
        REMOTE_DOMAIN,
        remote_router,
        LOCAL_DOMAIN,
        route.to_bytes(),
        &hyperlane_dusk_types::token_message::encode(recipient, amount),
    )
}

/// Registers `sk`'s account on `route` the one way a route registers an
/// account: a transfer of `amount` to it, from `remote_router` with message
/// nonce `nonce`, waits in escrow, and the account claims it. The route must
/// be able to pay `amount` out.
fn register_by_claim(
    session: &mut TestSession,
    route: ContractId,
    remote_router: H256,
    nonce: u32,
    sk: &AccountSecretKey,
    amount: u64,
) {
    let id = message::keccak256(&AccountPublicKey::from(sk).to_bytes());
    assert!(!route_is_registered(session, route, id));
    session
        .direct_call::<_, ()>(
            MAILBOX_ID,
            "process",
            &(
                Vec::<u8>::new(),
                inbound_transfer(route, remote_router, nonce, id, amount),
            ),
        )
        .expect("a transfer to an unregistered account waits in escrow");
    assert_eq!(route_u64(session, route, "pending_balance", id), amount);
    session
        .call_public::<_, ()>(sk, route, "claim_pending", &())
        .expect("the account claims its escrow");
    assert!(route_is_registered(session, route, id));
}

fn route_u64(session: &mut TestSession, route: ContractId, fn_name: &str, h: H256) -> u64 {
    session
        .direct_call::<_, u64>(route, fn_name, &(h,))
        .unwrap_or_else(|e| panic!("{fn_name} query failed: {e}"))
        .data
}

fn route_is_registered(session: &mut TestSession, route: ContractId, h: H256) -> bool {
    session
        .direct_call::<_, bool>(route, "is_registered", &(h,))
        .expect("is_registered query should succeed")
        .data
}

fn moonlight_balance(session: &mut TestSession, pk: &AccountPublicKey) -> u64 {
    session
        .account(pk)
        .expect("account query should succeed")
        .balance
}

/// A recipient whose only DUSK is in escrow can't pay for `claim_pending`.
/// Anyone can pay for `claim_pending_for` instead, and the DUSK still only
/// goes to the escrowed key.
#[test]
fn test_warp_native_sponsor_claims_escrow_for_unfunded_recipient() {
    let (mut session, remote_router) = session_with_warp_native_flow();
    let amount = 700_000u64;
    fund_warp_native(&mut session, amount);
    session
        .direct_call::<_, ()>(
            MAILBOX_ID,
            "process",
            &(
                Vec::<u8>::new(),
                inbound_transfer(WARP_NATIVE_ID, remote_router, 0, *FRESH_ID, amount),
            ),
        )
        .expect("a transfer to an unregistered recipient goes to escrow");
    assert_eq!(moonlight_balance(&mut session, &FRESH_PK), 0);
    assert_eq!(
        route_u64(&mut session, WARP_NATIVE_ID, "pending_balance", *FRESH_ID),
        amount
    );

    // A claim for a different key finds nothing and leaves the escrow alone.
    let wrong = session.call_public::<_, ()>(
        &RELAYER_SK,
        WARP_NATIVE_ID,
        "claim_pending_for",
        &(*OWNER_PK,),
    );
    assert_contract_panic(wrong, "WarpNative: no pending transfers");
    assert_eq!(
        route_u64(&mut session, WARP_NATIVE_ID, "pending_balance", *FRESH_ID),
        amount
    );

    let sponsor_before = moonlight_balance(&mut session, &RELAYER_PK);
    session
        .call_public::<_, ()>(
            &RELAYER_SK,
            WARP_NATIVE_ID,
            "claim_pending_for",
            &(*FRESH_PK,),
        )
        .expect("anyone can claim escrow on the recipient's behalf");
    assert_eq!(moonlight_balance(&mut session, &FRESH_PK), amount);
    assert!(
        moonlight_balance(&mut session, &RELAYER_PK) < sponsor_before,
        "the sponsor pays the gas and receives nothing"
    );
    assert_eq!(
        route_u64(&mut session, WARP_NATIVE_ID, "pending_balance", *FRESH_ID),
        0
    );
    assert!(route_is_registered(&mut session, WARP_NATIVE_ID, *FRESH_ID));

    let again = session.call_public::<_, ()>(
        &RELAYER_SK,
        WARP_NATIVE_ID,
        "claim_pending_for",
        &(*FRESH_PK,),
    );
    assert_contract_panic(again, "WarpNative: no pending transfers");
}

/// A sponsor can't register a recipient up front: only a claim of existing
/// escrow registers someone else's key. The first transfer waits in escrow;
/// claiming it for the recipient registers them, and the next transfer is
/// paid out directly. A key can't register itself up front either
/// (`test_a_funded_key_cannot_register_itself`).
#[test]
fn test_warp_native_sponsor_registers_recipient_only_by_claiming_escrow() {
    let (mut session, remote_router) = session_with_warp_native_flow();
    let amount = 400_000u64;
    fund_warp_native(&mut session, 2 * amount);
    let register = session.call_public::<_, ()>(
        &RELAYER_SK,
        WARP_NATIVE_ID,
        "register_account_for",
        &(*FRESH_PK,),
    );
    assert!(
        register.is_err(),
        "routes don't export register_account_for"
    );
    assert!(!route_is_registered(
        &mut session,
        WARP_NATIVE_ID,
        *FRESH_ID
    ));

    let deliver = |session: &mut TestSession, nonce| {
        session
            .direct_call::<_, ()>(
                MAILBOX_ID,
                "process",
                &(
                    Vec::<u8>::new(),
                    inbound_transfer(WARP_NATIVE_ID, remote_router, nonce, *FRESH_ID, amount),
                ),
            )
            .expect("inbound transfer should be delivered");
    };
    deliver(&mut session, 0);
    assert_eq!(moonlight_balance(&mut session, &FRESH_PK), 0);
    assert_eq!(
        route_u64(&mut session, WARP_NATIVE_ID, "pending_balance", *FRESH_ID),
        amount
    );

    session
        .call_public::<_, ()>(
            &RELAYER_SK,
            WARP_NATIVE_ID,
            "claim_pending_for",
            &(*FRESH_PK,),
        )
        .expect("anyone can claim escrow on the recipient's behalf");
    assert!(route_is_registered(&mut session, WARP_NATIVE_ID, *FRESH_ID));

    deliver(&mut session, 1);
    assert_eq!(moonlight_balance(&mut session, &FRESH_PK), 2 * amount);
    assert_eq!(
        route_u64(&mut session, WARP_NATIVE_ID, "pending_balance", *FRESH_ID),
        0
    );
}

/// Each entry a route's registry adds stands for a transfer someone sent
/// from the remote chain and paid the relaying of, claimed by its key: the
/// escrow's delivery, a transfer that funds the key, and the key's claim
/// (MEDIUM-10). This pins the Dusk gas of the last two, which
/// docs/security.md uses to estimate what filling a route's registry takes:
/// about 1.85M and 1.87M, almost all of it the transaction's signature check
/// (1.75M).
#[test]
fn test_a_registration_costs_an_inbound_transfer_and_two_transactions() {
    // Enough for the claim's gas at 1 LUX.
    const FUNDING: u64 = 3_000_000;
    let (mut session, remote_router) = session_with_warp_native_flow();
    fund_warp_native(&mut session, 3);
    for seed in 0..3u64 {
        let sk = AccountSecretKey::random(&mut StdRng::seed_from_u64(0x5E1F_0000 + seed));
        let pk = AccountPublicKey::from(&sk);
        let id = message::keccak256(&pk.to_bytes());
        session
            .direct_call::<_, ()>(
                MAILBOX_ID,
                "process",
                &(
                    Vec::<u8>::new(),
                    inbound_transfer(
                        WARP_NATIVE_ID,
                        remote_router,
                        u32::try_from(seed).unwrap(),
                        id,
                        1,
                    ),
                ),
            )
            .expect("a transfer to a new key waits in escrow");
        let funding = session.transfer(&RELAYER_SK, &pk, FUNDING);
        let claim = session
            .call_public_with_gas_limit::<_, ()>(&sk, WARP_NATIVE_ID, "claim_pending", &(), FUNDING)
            .expect("a funded key claims its escrow")
            .gas_spent;
        assert!(route_is_registered(&mut session, WARP_NATIVE_ID, id));
        // docs/security.md's estimate assumes about 1.85M gas for each.
        for (what, gas) in [("funding transfer", funding), ("claim", claim)] {
            assert!(
                (1_800_000..1_900_000).contains(&gas),
                "{what} took {gas} gas"
            );
        }
    }
}

#[test]
fn test_warp_drc20_sponsor_claims_escrow_for_unfunded_recipient() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();
    let amount = 650_000u64;
    session
        .direct_call::<_, ()>(
            MAILBOX_ID,
            "process",
            &(
                Vec::<u8>::new(),
                inbound_transfer(WARP_DRC20_ID, remote_router, 0, *FRESH_ID, amount),
            ),
        )
        .expect("a transfer to an unregistered recipient goes to escrow");

    let wrong = session.call_public::<_, ()>(
        &RELAYER_SK,
        WARP_DRC20_ID,
        "claim_pending_for",
        &(*OWNER_PK,),
    );
    assert_contract_panic(wrong, "WarpDrc20: no pending transfers");

    session
        .call_public::<_, ()>(
            &RELAYER_SK,
            WARP_DRC20_ID,
            "claim_pending_for",
            &(*FRESH_PK,),
        )
        .expect("anyone can claim escrow on the recipient's behalf");
    assert_eq!(
        warp_drc20_balance_of(&mut session, Drc20Account::moonlight(&FRESH_PK)),
        amount
    );
    assert_eq!(
        warp_drc20_balance_of(&mut session, Drc20Account::moonlight(&RELAYER_PK)),
        0
    );
    assert_eq!(
        route_u64(&mut session, WARP_DRC20_ID, "pending_balance", *FRESH_ID),
        0
    );
    assert!(route_is_registered(&mut session, WARP_DRC20_ID, *FRESH_ID));
}

#[test]
fn test_warp_collateral_sponsor_claims_escrow_for_unfunded_recipient() {
    let (mut session, remote_router) = session_with_warp_collateral_funded_flow();
    let amount = 300_000u64;
    session
        .direct_call::<_, ()>(
            MAILBOX_ID,
            "process",
            &(
                Vec::<u8>::new(),
                inbound_transfer(
                    WARP_DRC20_COLLATERAL_ID,
                    remote_router,
                    1,
                    *FRESH_ID,
                    amount,
                ),
            ),
        )
        .expect("a transfer to an unregistered recipient goes to escrow");

    let wrong = session.call_public::<_, ()>(
        &RELAYER_SK,
        WARP_DRC20_COLLATERAL_ID,
        "claim_pending_for",
        &(*OWNER_PK,),
    );
    assert_contract_panic(wrong, "WarpCollateral: no pending transfers");

    session
        .call_public::<_, ()>(
            &RELAYER_SK,
            WARP_DRC20_COLLATERAL_ID,
            "claim_pending_for",
            &(*FRESH_PK,),
        )
        .expect("anyone can claim escrow on the recipient's behalf");
    assert_eq!(
        warp_drc20_balance_of(&mut session, Drc20Account::moonlight(&FRESH_PK)),
        amount
    );
    assert_eq!(
        route_u64(
            &mut session,
            WARP_DRC20_COLLATERAL_ID,
            "pending_balance",
            *FRESH_ID
        ),
        0
    );
    assert!(route_is_registered(
        &mut session,
        WARP_DRC20_COLLATERAL_ID,
        *FRESH_ID
    ));
}

// =============================================================================
// Forged account keys in sponsored claims
// =============================================================================
//
// A `PublicKey` argument passes the archive check with any limbs. Routes find
// a recipient by `keccak256(key.to_bytes())`, the compressed encoding, while
// the transfer contract and DRC20 balances pay `key.to_raw_bytes()`. A forged
// key can share the recipient's compressed bytes but not its raw bytes. These
// tests check that every payment still reaches the recipient's real key.

/// The BLS12-381 base field modulus p, as little-endian 64-bit limbs.
const BLS12_381_P: [u64; 6] = [
    0xb9fe_ffff_ffff_aaab,
    0x1eab_fffe_b153_ffff,
    0x6730_d2a0_f6b0_f624,
    0x6477_4b84_f385_12bf,
    0x4b1b_a7b6_434b_acd7,
    0x1a01_11ea_397f_e69a,
];

/// `pk` with one limb of y changed. The point is off the curve, so nobody
/// can sign for it, but its compressed encoding is still `pk`'s.
fn off_curve_twin(pk: &AccountPublicKey) -> AccountPublicKey {
    let mut raw = pk.to_raw_bytes();
    // Change the lowest byte of y.c0.
    raw[96] ^= 0x01;
    // SAFETY: a 193-byte raw key encoding; skipping the checks is the point.
    unsafe { AccountPublicKey::from_slice_unchecked(&raw) }
}

/// `pk`'s own point with x.c0 stored as x.c0 + p. It passes `is_valid()`
/// and has `pk`'s compressed encoding, but not `pk`'s raw bytes.
fn unreduced_twin(pk: &AccountPublicKey) -> AccountPublicKey {
    let mut raw = pk.to_raw_bytes();
    let mut carry = 0u128;
    for (limb, p) in raw[..48].chunks_exact_mut(8).zip(BLS12_381_P) {
        let sum = u128::from(u64::from_le_bytes(limb.try_into().unwrap())) + u128::from(p) + carry;
        limb.copy_from_slice(&(sum as u64).to_le_bytes());
        carry = sum >> 64;
    }
    assert_eq!(carry, 0, "x.c0 + p fits in six limbs");
    // SAFETY: a 193-byte raw key encoding; skipping the checks is the point.
    unsafe { AccountPublicKey::from_slice_unchecked(&raw) }
}

/// Both forged forms of `pk`.
fn forged_twins(pk: &AccountPublicKey) -> [(&'static str, AccountPublicKey); 2] {
    [
        ("off-curve y", off_curve_twin(pk)),
        ("unreduced x", unreduced_twin(pk)),
    ]
}

fn account_key(seed: u64) -> AccountPublicKey {
    AccountPublicKey::from(&AccountSecretKey::random(&mut StdRng::seed_from_u64(seed)))
}

/// The premise of the attack: each twin has the recipient's hash but other
/// raw bytes, and `is_valid()` alone doesn't tell the unreduced one apart.
#[test]
fn test_forged_account_keys_share_the_recipient_hash() {
    for (name, twin) in forged_twins(&FRESH_PK) {
        assert_eq!(twin.to_bytes(), FRESH_PK.to_bytes(), "{name}");
        assert_eq!(message::keccak256(&twin.to_bytes()), *FRESH_ID, "{name}");
        assert_ne!(twin.to_raw_bytes(), FRESH_PK.to_raw_bytes(), "{name}");
        let decoded = AccountPublicKey::from_bytes(&twin.to_bytes()).unwrap();
        assert_eq!(decoded.to_raw_bytes(), FRESH_PK.to_raw_bytes(), "{name}");
    }
    assert!(!off_curve_twin(&FRESH_PK).is_valid());
    assert!(unreduced_twin(&FRESH_PK).is_valid());
}

/// The three warp routes, the collateral one over two tokens, each with
/// custody for inbound transfers.
#[derive(Clone, Copy, Debug)]
enum Route {
    Native,
    Synthetic,
    /// WarpDrc20Collateral locking a WarpDrc20 token.
    Collateral,
    /// WarpDrc20Collateral locking the pinned upstream DRC20, the kind of
    /// token a real collateral route locks. Its transfers cost more than
    /// the WarpDrc20 token's.
    CanonicalCollateral,
}

impl Route {
    fn id(self) -> ContractId {
        match self {
            Route::Native => WARP_NATIVE_ID,
            Route::Synthetic => WARP_DRC20_ID,
            Route::Collateral | Route::CanonicalCollateral => WARP_DRC20_COLLATERAL_ID,
        }
    }

    /// The prefix of the route's panic messages.
    fn name(self) -> &'static str {
        match self {
            Route::Native => "WarpNative",
            Route::Synthetic => "WarpDrc20",
            Route::Collateral | Route::CanonicalCollateral => "WarpCollateral",
        }
    }

    /// A session with the route deployed and able to pay out `custody`.
    fn session(self, custody: u64) -> (TestSession, H256) {
        match self {
            Route::Native => {
                let (mut session, remote_router) = session_with_warp_native_flow();
                fund_warp_native(&mut session, custody);
                (session, remote_router)
            }
            Route::Synthetic => session_with_warp_drc20_flow(),
            Route::Collateral => {
                assert!(custody <= 10_000_000, "the fixture locks 10M");
                session_with_warp_collateral_funded_flow()
            }
            Route::CanonicalCollateral => {
                // The owner locks `custody` tokens and keeps as many, so a
                // payout to it updates a balance, which costs more than
                // creating one.
                let (mut session, remote_router) =
                    session_with_canonical_drc20_collateral(2 * custody);
                session
                    .call_public::<_, ()>(
                        &OWNER_SK,
                        WARP_DRC20_ID,
                        "approve",
                        &Drc20ApproveCall {
                            spender: Drc20Account::Contract(WARP_DRC20_COLLATERAL_ID),
                            amount: custody,
                        },
                    )
                    .expect("the owner approves the route");
                session
                    .call_public::<_, MessageId>(
                        &OWNER_SK,
                        WARP_DRC20_COLLATERAL_ID,
                        "transfer_remote",
                        &(REMOTE_DOMAIN, [0xAB; 32], custody),
                    )
                    .expect("the route locks the owner's tokens");
                (session, remote_router)
            }
        }
    }

    /// What `pk`'s raw account holds: DUSK for WarpNative, otherwise the
    /// token balance of its Moonlight principal.
    fn paid_to(self, session: &mut TestSession, pk: &AccountPublicKey) -> u64 {
        match self {
            Route::Native => moonlight_balance(session, pk),
            Route::Synthetic | Route::Collateral => {
                warp_drc20_balance_of(session, Drc20Account::moonlight(pk))
            }
            Route::CanonicalCollateral => {
                canonical_drc20_balance_of(session, Drc20Account::moonlight(pk))
            }
        }
    }

    fn deliver(
        self,
        session: &mut TestSession,
        remote_router: H256,
        nonce: u32,
        recipient: H256,
        amount: u64,
    ) {
        session
            .direct_call::<_, ()>(
                MAILBOX_ID,
                "process",
                &(
                    Vec::<u8>::new(),
                    inbound_transfer(self.id(), remote_router, nonce, recipient, amount),
                ),
            )
            .expect("inbound transfer should be delivered");
    }

    fn pending(self, session: &mut TestSession, recipient: H256) -> u64 {
        route_u64(session, self.id(), "pending_balance", recipient)
    }

    fn claim_for(
        self,
        session: &mut TestSession,
        key: &AccountPublicKey,
    ) -> Result<CallReceipt<()>, ContractError> {
        session.call_public::<_, ()>(&RELAYER_SK, self.id(), "claim_pending_for", &(*key,))
    }
}

/// `claim_pending_for` with a forged twin of the recipient's key pays the
/// recipient's real account and registers its real key. Keys that don't
/// decode to a valid key revert and leave their escrow alone.
fn assert_sponsored_claim_pays_the_decoded_key(route: Route) {
    let amount = 300_000u64;
    let (mut session, remote_router) = route.session(6 * amount);
    let invalid_message = format!("{}: invalid account public key", route.name());

    // The identity decodes but nobody can sign for it. Changing x moves the
    // point off the curve or out of the subgroup, so it doesn't decode.
    let identity = AccountPublicKey::default();
    let mut raw = FRESH_PK.to_raw_bytes();
    // Change the lowest byte of x.c0.
    raw[0] ^= 0x01;
    // SAFETY: a 193-byte raw key encoding; skipping the checks is the point.
    let undecodable = unsafe { AccountPublicKey::from_slice_unchecked(&raw) };
    assert!(AccountPublicKey::from_bytes(&undecodable.to_bytes()).is_err());
    for (nonce, key) in [(100, identity), (101, undecodable)] {
        let h = message::keccak256(&key.to_bytes());
        route.deliver(&mut session, remote_router, nonce, h, amount);
        assert_contract_panic(route.claim_for(&mut session, &key), &invalid_message);
        assert_eq!(route.pending(&mut session, h), amount);
        assert!(!route_is_registered(&mut session, route.id(), h));
    }

    // One victim per forged form.
    let victims = [*FRESH_PK, account_key(0xF2E5_4001)];
    let forged = [
        ("off-curve y", off_curve_twin(&victims[0])),
        ("unreduced x", unreduced_twin(&victims[1])),
    ];
    for ((nonce, victim), (name, twin)) in (102..).zip(&victims).zip(forged) {
        let h = message::keccak256(&victim.to_bytes());
        route.deliver(&mut session, remote_router, nonce, h, amount);
        route.claim_for(&mut session, &twin).unwrap_or_else(|e| {
            panic!("{name}: claim for a forged key should pay the real one: {e}")
        });
        assert_eq!(route.paid_to(&mut session, victim), amount, "{name}");
        assert_eq!(route.paid_to(&mut session, &twin), 0, "{name}");
        assert_eq!(route.pending(&mut session, h), 0, "{name}");
        assert!(route_is_registered(&mut session, route.id(), h), "{name}");
    }
    assert_eq!(
        session
            .direct_call::<_, u64>(route.id(), "pending_total", &())
            .unwrap()
            .data,
        2 * amount,
        "only the invalid keys' escrow is left"
    );

    // The registrations hold the real keys: later transfers reach them.
    for (nonce, victim) in (110..).zip(&victims) {
        let h = message::keccak256(&victim.to_bytes());
        route.deliver(&mut session, remote_router, nonce, h, amount);
        assert_eq!(route.paid_to(&mut session, victim), 2 * amount);
        assert_eq!(route.pending(&mut session, h), 0);
        for (name, twin) in forged_twins(victim) {
            assert_eq!(route.paid_to(&mut session, &twin), 0, "{name}");
        }
    }
}

/// A recipient that registered by claiming its escrow keeps that
/// registration. Routes have no call that registers someone else's key
/// outright, and a forged key passed to `claim_pending_for` decodes to the
/// recipient's own key, so neither escrow waiting for it nor later transfers
/// can be diverted.
fn assert_forged_key_cannot_redirect_registered_account(route: Route) {
    let amount = 400_000u64;
    let (mut session, remote_router) = route.session(2 * amount + 1);
    register_by_claim(&mut session, route.id(), remote_router, 99, &OWNER_SK, 1);
    let before = route.paid_to(&mut session, &OWNER_PK);

    for (name, twin) in forged_twins(&OWNER_PK) {
        let overwrite =
            session.call_public::<_, ()>(&RELAYER_SK, route.id(), "register_account_for", &(twin,));
        assert!(
            overwrite.is_err(),
            "{name}: routes don't export register_account_for"
        );
        // With nothing in escrow a claim reverts, and registers nothing.
        assert_contract_panic(
            route.claim_for(&mut session, &twin),
            &format!("{}: no pending transfers", route.name()),
        );
    }

    route.deliver(&mut session, remote_router, 100, *OWNER_ID, amount);
    route.deliver(&mut session, remote_router, 101, *OWNER_ID, amount);
    assert_eq!(route.paid_to(&mut session, &OWNER_PK) - before, 2 * amount);
    assert_eq!(route.pending(&mut session, *OWNER_ID), 0);
    for (name, twin) in forged_twins(&OWNER_PK) {
        assert_eq!(route.paid_to(&mut session, &twin), 0, "{name}");
    }
}

#[test]
fn test_warp_native_sponsored_claim_pays_the_decoded_key() {
    assert_sponsored_claim_pays_the_decoded_key(Route::Native);
}

#[test]
fn test_warp_drc20_sponsored_claim_pays_the_decoded_key() {
    assert_sponsored_claim_pays_the_decoded_key(Route::Synthetic);
}

#[test]
fn test_warp_collateral_sponsored_claim_pays_the_decoded_key() {
    assert_sponsored_claim_pays_the_decoded_key(Route::Collateral);
}

#[test]
fn test_warp_native_forged_key_cannot_redirect_registered_account() {
    assert_forged_key_cannot_redirect_registered_account(Route::Native);
}

#[test]
fn test_warp_drc20_forged_key_cannot_redirect_registered_account() {
    assert_forged_key_cannot_redirect_registered_account(Route::Synthetic);
}

#[test]
fn test_warp_collateral_forged_key_cannot_redirect_registered_account() {
    assert_forged_key_cannot_redirect_registered_account(Route::Collateral);
}

/// `pk` with its raw infinity byte set to 2. The archive check accepts any
/// byte there; a real key has 0, and the identity 1.
fn malformed_infinity_key(pk: &AccountPublicKey) -> AccountPublicKey {
    let mut raw = pk.to_raw_bytes();
    raw[192] = 2;
    // SAFETY: a 193-byte raw key encoding; skipping the checks is the point.
    unsafe { AccountPublicKey::from_slice_unchecked(&raw) }
}

/// The compressed encoding the route computes for
/// [`malformed_infinity_key`]`(pk)`, and so the hash it looked up before it
/// decoded keys. `to_bytes` selects in constant time on the infinity byte.
/// With the byte at 2, those selects keep only bit 0 of each x limb and set
/// the infinity flag, next to an x that isn't zero: an encoding no key
/// decodes from. (This repeats dusk-bls12_381 0.14's `to_bytes`: calling it
/// here trips `subtle`'s debug assertion that a choice is 0 or 1, which the
/// release-built contract doesn't have.)
fn malformed_infinity_encoding(pk: &AccountPublicKey) -> [u8; 96] {
    let mut raw = pk.to_raw_bytes();
    for limb in raw[..96].chunks_exact_mut(8) {
        let low_bit = u64::from_le_bytes(limb.try_into().unwrap()) & 1;
        limb.copy_from_slice(&low_bit.to_le_bytes());
    }
    raw[192] = 0;
    // SAFETY: a 193-byte raw key encoding.
    let mut encoding = unsafe { AccountPublicKey::from_slice_unchecked(&raw) }.to_bytes();
    encoding[0] |= 1 << 6;
    encoding
}

/// A key whose point is on the curve but outside the prime-order subgroup,
/// so no secret key has it. Its compressed encoding is well formed, and
/// decoding it fails only the subgroup check. x is the first small integer
/// with a point on the curve.
fn non_subgroup_key() -> AccountPublicKey {
    use dusk_bls12_381::G2Affine;
    let point = (1u8..=u8::MAX)
        .find_map(|x| {
            let mut compressed = [0u8; 96];
            compressed[0] = 0x80; // compressed, not infinity, smaller y
            compressed[95] = x; // x = x + 0u
            Option::<G2Affine>::from(G2Affine::from_compressed_unchecked(&compressed))
        })
        .expect("a small x has a point on the curve");
    assert!(bool::from(point.is_on_curve()));
    assert!(!bool::from(point.is_torsion_free()));
    // SAFETY: a 193-byte raw point encoding of a point on the curve.
    unsafe { AccountPublicKey::from_slice_unchecked(&point.to_raw_bytes()) }
}

/// Neither fixture decodes, and each one's hash is distinct from the
/// recipient's.
#[test]
fn test_malformed_and_non_subgroup_keys_do_not_decode() {
    let malformed = malformed_infinity_encoding(&FRESH_PK);
    assert_eq!(malformed[0] & 0xC0, 0xC0, "compressed, at infinity");
    let mut x = malformed;
    x[0] &= 0x1F;
    assert_ne!(x, [0u8; 96], "an infinity flag next to a non-zero x");
    assert!(AccountPublicKey::from_bytes(&malformed).is_err());

    let key = non_subgroup_key();
    assert!(!key.is_valid());
    assert!(AccountPublicKey::from_bytes(&key.to_bytes()).is_err());

    for encoding in [malformed, key.to_bytes()] {
        assert_ne!(message::keccak256(&encoding), *FRESH_ID);
    }
}

/// `claim_pending_for` reverts for a key with a malformed infinity byte and
/// for a point outside the subgroup, even with escrow waiting under the hash
/// the route computes for it, and changes nothing: escrow, registrations and
/// balances stay as they were.
fn assert_claim_for_undecodable_keys_changes_nothing(route: Route) {
    let amount = 200_000u64;
    let (mut session, remote_router) = route.session(3 * amount);
    let invalid_message = format!("{}: invalid account public key", route.name());
    route.deliver(&mut session, remote_router, 100, *FRESH_ID, amount);

    let non_subgroup = non_subgroup_key();
    let keys = [
        (
            "malformed infinity byte",
            malformed_infinity_key(&FRESH_PK),
            message::keccak256(&malformed_infinity_encoding(&FRESH_PK)),
        ),
        (
            "outside the subgroup",
            non_subgroup,
            message::keccak256(&non_subgroup.to_bytes()),
        ),
    ];
    for (nonce, (_, _, h)) in (101..).zip(&keys) {
        route.deliver(&mut session, remote_router, nonce, *h, amount);
    }
    let pending_total = |session: &mut TestSession| {
        session
            .direct_call::<_, u64>(route.id(), "pending_total", &())
            .unwrap()
            .data
    };
    assert_eq!(pending_total(&mut session), 3 * amount);

    for (name, key, h) in &keys {
        assert_contract_panic(route.claim_for(&mut session, key), &invalid_message);
        for escrowed in [*h, *FRESH_ID] {
            assert_eq!(route.pending(&mut session, escrowed), amount, "{name}");
            assert!(
                !route_is_registered(&mut session, route.id(), escrowed),
                "{name}"
            );
        }
        assert_eq!(pending_total(&mut session), 3 * amount, "{name}");
        for pk in [key, &*FRESH_PK] {
            assert_eq!(route.paid_to(&mut session, pk), 0, "{name}");
        }
    }
}

#[test]
fn test_warp_native_claim_for_undecodable_keys_changes_nothing() {
    assert_claim_for_undecodable_keys_changes_nothing(Route::Native);
}

#[test]
fn test_warp_drc20_claim_for_undecodable_keys_changes_nothing() {
    assert_claim_for_undecodable_keys_changes_nothing(Route::Synthetic);
}

#[test]
fn test_warp_collateral_claim_for_undecodable_keys_changes_nothing() {
    assert_claim_for_undecodable_keys_changes_nothing(Route::Collateral);
}

// =============================================================================
// Self claims
// =============================================================================

/// `dusk-tx claim-pending`'s default gas limit when the signer claims its own
/// escrow (`claim_pending`). A failed transaction pays its whole limit, so
/// the limit stays near the cost of a claim.
const SELF_CLAIM_GAS_LIMIT: u64 = 3_000_000;

/// A recipient that claims its own escrow with `claim_pending` is registered,
/// as one claimed for with `claim_pending_for` is, so its next transfer is
/// paid out directly instead of waiting in escrow again. The claim succeeds
/// with `dusk-tx claim-pending`'s default gas limit, and costs at most two
/// thirds of it.
fn assert_self_claim_registers_the_sender(route: Route) {
    let amount = 300_000u64;
    let (mut session, remote_router) = route.session(2 * amount);
    route.deliver(&mut session, remote_router, 100, *OWNER_ID, amount);
    assert!(!route_is_registered(&mut session, route.id(), *OWNER_ID));
    let before = route.paid_to(&mut session, &OWNER_PK);

    let claim = session
        .call_public_with_gas_limit::<_, ()>(
            &OWNER_SK,
            route.id(),
            "claim_pending",
            &(),
            SELF_CLAIM_GAS_LIMIT,
        )
        .unwrap_or_else(|e| panic!("{route:?}: a recipient claims its own escrow: {e}"));
    assert!(
        claim.gas_spent * 3 / 2 <= SELF_CLAIM_GAS_LIMIT,
        "{route:?}: claim_pending took {} gas, too close to {SELF_CLAIM_GAS_LIMIT}",
        claim.gas_spent
    );
    // A WarpNative claimer receives DUSK and pays its gas from the same
    // balance, at 1 LUX.
    let gas_paid = match route {
        Route::Native => claim.gas_spent,
        _ => 0,
    };
    assert_eq!(
        route.paid_to(&mut session, &OWNER_PK) + gas_paid,
        before + amount
    );
    assert_eq!(route.pending(&mut session, *OWNER_ID), 0);
    assert!(route_is_registered(&mut session, route.id(), *OWNER_ID));

    // The next transfer is paid out directly.
    route.deliver(&mut session, remote_router, 101, *OWNER_ID, amount);
    assert_eq!(
        route.paid_to(&mut session, &OWNER_PK) + gas_paid,
        before + 2 * amount
    );
    assert_eq!(route.pending(&mut session, *OWNER_ID), 0);
    assert_eq!(
        session
            .direct_call::<_, u64>(route.id(), "pending_total", &())
            .unwrap()
            .data,
        0
    );
}

/// A self-claim with nothing in escrow for the sender reverts, and the
/// revert takes the registration with it, so `claim_pending` can't register
/// a key nobody sent anything to. Escrow for someone else stays.
fn assert_self_claim_without_escrow_registers_nothing(route: Route) {
    let amount = 300_000u64;
    let (mut session, remote_router) = route.session(amount);
    route.deliver(&mut session, remote_router, 100, *FRESH_ID, amount);

    let claim = session.call_public::<_, ()>(&OWNER_SK, route.id(), "claim_pending", &());
    assert_contract_panic(claim, &format!("{}: no pending transfers", route.name()));
    assert!(!route_is_registered(&mut session, route.id(), *OWNER_ID));
    assert_eq!(route.pending(&mut session, *FRESH_ID), amount);
}

#[test]
fn test_warp_native_self_claim_registers_the_sender() {
    assert_self_claim_registers_the_sender(Route::Native);
    assert_self_claim_without_escrow_registers_nothing(Route::Native);
}

#[test]
fn test_warp_drc20_self_claim_registers_the_sender() {
    assert_self_claim_registers_the_sender(Route::Synthetic);
    assert_self_claim_without_escrow_registers_nothing(Route::Synthetic);
}

#[test]
fn test_warp_collateral_self_claim_registers_the_sender() {
    for route in [Route::Collateral, Route::CanonicalCollateral] {
        assert_self_claim_registers_the_sender(route);
        assert_self_claim_without_escrow_registers_nothing(route);
    }
}

// =============================================================================
// Relayer gas limit
// =============================================================================

/// The relayer submits `process` with a fixed gas limit, and a failed Dusk
/// transaction pays all of it, for example when someone else delivered the
/// message first. So the limit is kept low: `DUSK_RELAYER_GAS_LIMIT`, 5M in
/// `deploy/testnet.env.example` and `demo/gen-agent-configs.sh`. Below the
/// real cost every delivery would run out of gas, which the relayer's
/// simulation can't see. A relayed transfer must fit with 1.5x headroom at
/// every threshold the demo runs (1 to 5), signed by the validators at the
/// end of the list. It costs about 1.95M gas plus 0.22M per signature,
/// the estimate `deploy/testnet.sh` checks the limit against.
#[test]
fn test_relayed_process_fits_the_relayer_gas_limit() {
    const RELAYER_GAS_LIMIT: u64 = 5_000_000;
    let signers = checkpoint_signer_set(5);
    let validators: Vec<EthAddress> = signers.iter().map(|(address, _)| *address).collect();
    let (mut session, remote_router) = session_with_warp_native_flow();
    let amount = 1_000u64;
    fund_warp_native(&mut session, 20 * amount + 1);
    register_by_claim(
        &mut session,
        WARP_NATIVE_ID,
        remote_router,
        1_000,
        &OWNER_SK,
        1,
    );
    session
        .deploy(
            ISM_MULTISIG_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(*OWNER_ID, validators.clone(), 1u8))
                .contract_id(ISM_MULTISIG_ID),
        )
        .expect("Deploying MessageIdMultisigISM should succeed");
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "set_default_ism",
            &(ISM_MULTISIG_ID,),
        )
        .expect("the owner sets the default ISM");

    let mut nonce = 0u32;
    for threshold in 1u8..=5 {
        session
            .call_public::<_, ()>(
                &OWNER_SK,
                ISM_MULTISIG_ID,
                "set_validators_and_threshold",
                &(validators.clone(), threshold),
            )
            .expect("the owner sets the threshold");
        let signing: Vec<&SecpSecretKey> = signers[signers.len() - usize::from(threshold)..]
            .iter()
            .map(|(_, key)| key)
            .collect();
        // A payout to a registered account and a new escrow entry.
        for recipient in [*OWNER_ID, [0x70 + threshold; 32]] {
            let inbound = inbound_transfer(WARP_NATIVE_ID, remote_router, nonce, recipient, amount);
            nonce += 1;
            let metadata = signed_checkpoint_metadata(&inbound, &signing);
            let receipt = session
                .call_public::<_, ()>(&RELAYER_SK, MAILBOX_ID, "process", &(metadata, inbound))
                .expect("a relayed transfer should be delivered");
            assert!(
                receipt.gas_spent * 3 / 2 <= RELAYER_GAS_LIMIT,
                "threshold {threshold}: process took {} gas, too close to {RELAYER_GAS_LIMIT}",
                receipt.gas_spent
            );
        }
    }
}

/// `deploy/testnet.sh` accepts a 5M relayer limit up to a threshold of 6,
/// where it needs 4,905,000. At that threshold a relayed transfer must
/// succeed in a transaction whose gas limit really is 5M, not just cost less
/// than 5M under a bigger limit, on every route and both delivery paths: a
/// payout (or mint) to a registered account and a new escrow entry. The
/// costliest is a payout from the collateral route that locks the upstream
/// DRC20, at about 3.27M gas.
#[test]
fn test_relayed_process_at_threshold_6_fits_a_5m_gas_limit() {
    const RELAYER_GAS_LIMIT: u64 = 5_000_000;
    const THRESHOLD: u8 = 6;
    let amount = 1_000u64;
    let signers = checkpoint_signer_set(THRESHOLD);
    let validators: Vec<EthAddress> = signers.iter().map(|(address, _)| *address).collect();
    let signing: Vec<&SecpSecretKey> = signers.iter().map(|(_, key)| key).collect();

    for route in [
        Route::Native,
        Route::Synthetic,
        Route::Collateral,
        Route::CanonicalCollateral,
    ] {
        let (mut session, remote_router) = route.session(10 * amount);
        register_by_claim(&mut session, route.id(), remote_router, 1_000, &OWNER_SK, 1);
        session
            .deploy(
                ISM_MULTISIG_BYTECODE,
                dusk_vm::ContractData::builder()
                    .owner(DEPLOYER)
                    .init_arg(&(*OWNER_ID, validators.clone(), THRESHOLD))
                    .contract_id(ISM_MULTISIG_ID),
            )
            .expect("Deploying MessageIdMultisigISM should succeed");
        session
            .call_public::<_, ()>(
                &OWNER_SK,
                MAILBOX_ID,
                "set_default_ism",
                &(ISM_MULTISIG_ID,),
            )
            .expect("the owner sets the default ISM");

        let escrow = [0x7E; 32];
        let paid_before = route.paid_to(&mut session, &OWNER_PK);
        for (nonce, (path, recipient)) in
            (0u32..).zip([("payout", *OWNER_ID), ("new escrow", escrow)])
        {
            let inbound = inbound_transfer(route.id(), remote_router, nonce, recipient, amount);
            let metadata = signed_checkpoint_metadata(&inbound, &signing);
            let receipt = session
                .call_public_with_gas_limit::<_, ()>(
                    &RELAYER_SK,
                    MAILBOX_ID,
                    "process",
                    &(metadata, inbound.clone()),
                    RELAYER_GAS_LIMIT,
                )
                .unwrap_or_else(|e| panic!("{route:?} {path}: process failed with 5M gas: {e}"));
            assert!(
                session
                    .direct_call::<_, bool>(MAILBOX_ID, "delivered", &(message::id(&inbound),))
                    .unwrap()
                    .data,
                "{route:?} {path}"
            );
            // The guard's 1.5x headroom holds at its highest threshold.
            assert!(
                receipt.gas_spent * 3 / 2 <= RELAYER_GAS_LIMIT,
                "{route:?} {path}: process took {} gas, too close to {RELAYER_GAS_LIMIT}",
                receipt.gas_spent
            );
        }
        assert_eq!(
            route.paid_to(&mut session, &OWNER_PK),
            paid_before + amount,
            "{route:?}"
        );
        assert_eq!(route.pending(&mut session, escrow), amount, "{route:?}");
    }
}

/// A recipient without `interchain_security_module` gets the default ISM.
/// The failed lookup costs at most its fixed allowance; an unbounded call
/// would be charged most of the transaction's gas.
#[test]
fn test_process_recipient_without_ism_getter_uses_default_ism_cheaply() {
    let mut s = HyperlaneSession::new();
    s.session
        .deploy(
            PLAIN_RECIPIENT_BYTECODE,
            dusk_vm::ContractData::builder()
                .owner(DEPLOYER)
                .contract_id(PLAIN_RECIPIENT_ID),
        )
        .expect("Deploying PlainRecipient should succeed");

    let sender = [0xABu8; 32];
    let to_test_recipient = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        sender,
        LOCAL_DOMAIN,
        TEST_RECIPIENT_ID.to_bytes(),
        b"baseline",
    );
    let to_plain = message::encode(
        VERSION,
        1,
        REMOTE_DOMAIN,
        sender,
        LOCAL_DOMAIN,
        PLAIN_RECIPIENT_ID.to_bytes(),
        b"baseline",
    );
    let baseline = s
        .mailbox_process(Vec::new(), to_test_recipient)
        .expect("delivery to TestRecipient should succeed");
    let plain = s
        .mailbox_process(Vec::new(), to_plain.clone())
        .expect("delivery to a recipient without an ISM getter should succeed");

    assert!(s.mailbox_delivered(message::id(&to_plain)));
    assert_eq!(
        rt_u32(&mut s.session, PLAIN_RECIPIENT_ID, "handled_count"),
        1
    );
    assert_eq!(
        s.mock_verify_count(),
        2,
        "both deliveries use the default ISM"
    );
    assert!(
        plain.gas_spent <= baseline.gas_spent + 1_000_000,
        "a missing ISM getter must cost at most its lookup allowance: {} vs baseline {}",
        plain.gas_spent,
        baseline.gas_spent
    );
}

/// A recipient whose ISM getter runs out of its allowance does not fall back
/// to the default ISM: otherwise the submitter's gas limit could choose which
/// ISM verifies the message.
#[test]
fn test_process_rejects_message_when_recipient_ism_lookup_runs_out_of_gas() {
    let mut s = HyperlaneSession::new();
    rt_deploy(&mut s.session);
    s.session
        .direct_call::<_, ()>(REDTEAM_ID, "set_ism_burn", &(u64::MAX,))
        .expect("attacker configuration should succeed");

    let inbound = message::encode(
        VERSION,
        0,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        LOCAL_DOMAIN,
        REDTEAM_ID.to_bytes(),
        b"expensive getter",
    );
    let result =
        s.session
            .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), inbound.clone()));
    assert_contract_panic(result, "Mailbox: recipient ISM lookup ran out of gas");
    assert!(!rt_delivered(&mut s.session, message::id(&inbound)));
    assert_eq!(rt_counters(&mut s.session).0, 0);
    assert_eq!(s.mock_verify_count(), 0);
}

#[test]
fn test_mailbox_reports_memory_below_dispatch_limit() {
    let mut s = HyperlaneSession::new();
    s.mailbox_dispatch_via_tx(&OWNER_SK, REMOTE_DOMAIN, [0xBBu8; 32], b"sized".to_vec())
        .expect("dispatch should succeed");
    let bytes = s
        .session
        .direct_call::<_, u64>(MAILBOX_ID, "memory_bytes", &())
        .expect("memory_bytes should succeed")
        .data;
    assert!(
        bytes > 0 && bytes % (64 * 1024) == 0,
        "whole wasm pages: {bytes}"
    );
    assert!(bytes < 3 * 1024 * 1024 * 1024);
}

// =============================================================================
// Storage layout (docs/storage.md)
// =============================================================================

/// Size of a wasm32 memory page.
const WASM_PAGE: u64 = 64 * 1024;

/// The Mailbox's `DISPATCH_MEMORY_LIMIT`.
const DISPATCH_MEMORY_LIMIT: u64 = 3 * 1024 * 1024 * 1024;

fn query<A, R>(session: &mut TestSession, contract: ContractId, fn_name: &str, args: &A) -> R
where
    A: for<'b> rkyv::Serialize<dusk_core::abi::StandardBufSerializer<'b>>,
    A::Archived: for<'b> bytecheck::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
    R: rkyv::Archive,
    R::Archived: rkyv::Deserialize<R, rkyv::Infallible>
        + for<'b> bytecheck::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
{
    session
        .direct_call::<A, R>(contract, fn_name, args)
        .unwrap_or_else(|e| panic!("{fn_name} should succeed: {e}"))
        .data
}

fn mailbox_memory(session: &mut TestSession) -> u64 {
    query(session, MAILBOX_ID, "memory_bytes", &())
}

/// The IDs of the messages a receipt dispatched, in order, from its
/// `DispatchId` events.
fn dispatched_ids<R>(receipt: &CallReceipt<R>) -> Vec<MessageId> {
    receipt
        .events
        .iter()
        .filter(|e| e.source == MAILBOX_ID && e.topic == events::DispatchId::TOPIC && !e.reverted)
        .map(|e| test_session::rkyv_deserialize::<events::DispatchId>(&e.data).message_id)
        .collect()
}

/// Has the red-team contract make `actions` in one transaction; any failure
/// fails them all.
fn run_batch(
    session: &mut TestSession,
    actions: Vec<RtAction>,
) -> Result<CallReceipt<Vec<u8>>, ContractError> {
    let args = rkyv::to_bytes::<_, 4096>(&(actions, true))
        .expect("the batch should serialize")
        .to_vec();
    session.call_public_raw(&OWNER_SK, REDTEAM_ID, "run", args)
}

/// The red-team contract dispatches messages `first..first + count` in one
/// transaction, each with its index as body; any failure fails them all.
fn dispatch_batch(
    session: &mut TestSession,
    first: u32,
    count: u32,
) -> Result<Vec<MessageId>, ContractError> {
    let actions: Vec<RtAction> = (first..first + count)
        .map(|index| {
            rt_action(
                MAILBOX_ID,
                "dispatch_default",
                &(REMOTE_DOMAIN, [0xD1u8; 32], index.to_le_bytes().to_vec()),
            )
        })
        .collect();
    let receipt = run_batch(session, actions)?;
    let ids = dispatched_ids(&receipt);
    assert_eq!(ids.len(), count as usize);
    Ok(ids)
}

/// The Mailbox and the Merkle hook keep message IDs in 64 KiB chunks of 2048
/// and block heights once per block. Every index reads back across chunk and
/// block boundaries, and the root a validator computes from the stored IDs is
/// the hook's root each time.
#[test]
fn test_dispatch_logs_read_back_across_chunks_and_blocks() {
    let mut s = HyperlaneSession::new();
    rt_deploy(&mut s.session);
    let mut ids = Vec::new();
    let mut heights = Vec::new();
    let mut tree = hyperlane_dusk_types::merkle::IncrementalMerkle::new();
    // Nine blocks, three apart, of 256 messages each: 2304 messages, past the
    // first chunk of 2048.
    let first_height = s.session.block_height() + 1;
    for block in 0..9u64 {
        let height = first_height + 3 * block;
        s.session.set_block_height(height);
        let batch = dispatch_batch(&mut s.session, u32::try_from(ids.len()).unwrap(), 256)
            .expect("the batch of dispatches should succeed");
        for id in &batch {
            tree.insert(*id);
        }
        heights.extend(core::iter::repeat_n(height, batch.len()));
        ids.extend(batch);
        // The root a validator computes for the last index so far: 255, 511,
        // ..., 2047 (the end of the first chunk) and 2303.
        assert_eq!(s.merkle_root(), tree.root(), "index {}", ids.len() - 1);
    }

    let count = u32::try_from(ids.len()).unwrap();
    assert_eq!(s.mailbox_nonce(), count);
    assert_eq!(s.merkle_count(), count);
    for (index, (id, height)) in (0u32..).zip(ids.iter().zip(&heights)) {
        let session = &mut s.session;
        assert_eq!(
            query::<_, MessageId>(session, MAILBOX_ID, "dispatched_id", &(index,)),
            *id
        );
        assert_eq!(
            query::<_, u64>(session, MAILBOX_ID, "dispatched_block_height", &(index,)),
            *height
        );
        assert_eq!(
            query::<_, H256>(session, MERKLE_TREE_HOOK_ID, "message_id_at", &(index,)),
            *id
        );
        assert_eq!(
            query::<_, u64>(
                session,
                MERKLE_TREE_HOOK_ID,
                "inserted_block_height",
                &(index,)
            ),
            *height
        );
    }
    let mut paged = Vec::new();
    for start in (0..count).step_by(256) {
        paged.extend(query::<_, Vec<H256>>(
            &mut s.session,
            MERKLE_TREE_HOOK_ID,
            "message_ids",
            &(start, 256u32),
        ));
    }
    assert_eq!(paged, ids);
    // A page can start in one chunk and end in the next.
    assert_eq!(
        query::<_, Vec<H256>>(
            &mut s.session,
            MERKLE_TREE_HOOK_ID,
            "message_ids",
            &(2040u32, 16u32)
        ),
        ids[2040..2056]
    );
    assert!(query::<_, Vec<H256>>(
        &mut s.session,
        MERKLE_TREE_HOOK_ID,
        "message_ids",
        &(count, 256u32)
    )
    .is_empty());

    for (contract, name, error) in [
        (MAILBOX_ID, "dispatched_id", "Mailbox: nonce out of range"),
        (
            MAILBOX_ID,
            "dispatched_block_height",
            "Mailbox: nonce out of range",
        ),
        (
            MERKLE_TREE_HOOK_ID,
            "message_id_at",
            "MerkleTreeHook: insertion index out of bounds",
        ),
        (
            MERKLE_TREE_HOOK_ID,
            "inserted_block_height",
            "MerkleTreeHook: insertion index out of bounds",
        ),
    ] {
        assert_contract_panic(
            s.session.direct_call::<_, H256>(contract, name, &(count,)),
            error,
        );
    }
    assert_contract_panic(
        s.session.direct_call::<_, Vec<H256>>(
            MERKLE_TREE_HOOK_ID,
            "message_ids",
            &(count + 1, 1u32),
        ),
        "MerkleTreeHook: insertion index out of bounds",
    );
    assert_contract_panic(
        s.session
            .direct_call::<_, Vec<H256>>(MERKLE_TREE_HOOK_ID, "message_ids", &(0u32, 257u32)),
        "MerkleTreeHook: page too large",
    );
}

/// `wasm` with its memory's initial size set to `pages`. Piecrust gives a
/// new contract the memory its module declares, so this is the deployed code
/// in a memory that is as large as if it had grown that far.
fn with_initial_memory_pages(wasm: &[u8], pages: u64) -> Vec<u8> {
    fn read_leb(bytes: &[u8], position: &mut usize) -> u64 {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = bytes[*position];
            *position += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
        }
        panic!("LEB128 value too long");
    }
    fn write_leb(out: &mut Vec<u8>, mut value: u64) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    assert_eq!(&wasm[..8], b"\0asm\x01\0\0\0", "a wasm module");
    let mut out = wasm[..8].to_vec();
    let mut position = 8;
    let mut patched = false;
    while position < wasm.len() {
        let id = wasm[position];
        position += 1;
        let size = usize::try_from(read_leb(wasm, &mut position)).unwrap();
        let payload = &wasm[position..position + size];
        position += size;
        let payload = if id == 5 {
            // The memory section: one 32-bit memory, its limits flag, its
            // minimum and, if the flag says so, its maximum.
            let mut at = 0;
            assert_eq!(read_leb(payload, &mut at), 1, "one memory");
            let flags = payload[at];
            at += 1;
            assert_eq!(flags & !1, 0, "a 32-bit, unshared memory");
            read_leb(payload, &mut at);
            let mut section = vec![1, flags];
            write_leb(&mut section, pages);
            section.extend_from_slice(&payload[at..]);
            patched = true;
            section
        } else {
            payload.to_vec()
        };
        out.push(id);
        write_leb(&mut out, payload.len() as u64);
        out.extend_from_slice(&payload);
    }
    assert!(patched, "the module defines its memory");
    out
}

/// The most a dispatch of an `encoded`-byte message with `metadata` bytes of
/// metadata can still grow the Mailbox's memory once it is quoted, the bound
/// the Mailbox checks: each allocation it makes, plus dlmalloc's overhead,
/// in whole pages. `ids` and `heights` mirror the Mailbox's logs. The chunk
/// table is twice as large here as in wasm32, which the page rounding hides
/// while it stays under 32 KiB.
fn dispatch_memory_bound(
    ids: &hyperlane_dusk_types::storage::ChunkedLog<MessageId>,
    heights: &hyperlane_dusk_types::storage::BlockHeights,
    height: u64,
    encoded: usize,
    metadata: usize,
) -> u64 {
    let [ids_chunk, ids_table] = ids.push_allocations();
    let [heights_chunk, heights_table] = heights.push_allocations(height);
    assert!(ids_table < 32 * 1024 && heights_table < 32 * 1024);
    [
        ids_chunk,
        ids_table,
        heights_chunk,
        heights_table,
        encoded,
        metadata,
        encoded,
        "receive_payment".len(),
        "receive_payment".len(),
    ]
    .into_iter()
    .map(|bytes| {
        if bytes == 0 {
            0
        } else {
            (bytes as u64 + 64).next_multiple_of(WASM_PAGE)
        }
    })
    .sum()
}

/// `dispatch` refuses a message unless everything it can still allocate fits
/// under 3 GiB, so it never takes the Mailbox past that and the last GiB
/// stays free for `process`. The Mailbox here starts 12 pages below the
/// limit, as if it were nearly full. Every dispatch it admits leaves its
/// memory at or below 3 GiB, and it admits exactly the dispatches whose
/// allocations fit. Once it closes, deliveries go on until they have taken
/// the memory past 3 GiB, and a fresh message is still reserved, handled
/// and refused as a replay there.
#[test]
fn test_dispatch_never_takes_the_mailbox_past_3_gib() {
    let limit_pages = DISPATCH_MEMORY_LIMIT / WASM_PAGE;
    let mailbox = with_initial_memory_pages(MAILBOX_BYTECODE, limit_pages - 12);
    let mut s = HyperlaneSession::with_mailbox(&mailbox);
    rt_deploy(&mut s.session);
    // Deploying takes a page for the allocator's first block.
    let start = mailbox_memory(&mut s.session);
    assert!(
        (DISPATCH_MEMORY_LIMIT - 12 * WASM_PAGE..=DISPATCH_MEMORY_LIMIT - 10 * WASM_PAGE)
            .contains(&start),
        "starts at {start}"
    );

    let mut ids = hyperlane_dusk_types::storage::ChunkedLog::<MessageId>::new();
    let mut heights = hyperlane_dusk_types::storage::BlockHeights::new();
    let height = s.session.block_height();
    // `dispatch_batch` sends 4-byte bodies and no metadata.
    let encoded = message::MIN_MESSAGE_LENGTH + 4;
    let per_chunk = hyperlane_dusk_types::storage::ChunkedLog::<MessageId>::PER_CHUNK;
    let mut admitted_chunks = 0;
    loop {
        let nonce = s.mailbox_nonce();
        assert_eq!(nonce, ids.len());
        let memory = mailbox_memory(&mut s.session);
        let bound = dispatch_memory_bound(&ids, &heights, height, encoded, 0);
        let starts_chunk = ids.push_allocations()[0] > 0;
        let fits = memory + bound <= DISPATCH_MEMORY_LIMIT;
        // Up to the next chunk, dispatches keep nothing new beyond their
        // IDs, so they go in batches; the rest go one at a time.
        let count = if fits && !starts_chunk {
            (per_chunk - nonce % per_chunk).min(256)
        } else {
            1
        };
        match dispatch_batch(&mut s.session, nonce, count) {
            Ok(batch) => {
                assert!(fits, "nonce {nonce}: admitted at {memory} + {bound}");
                for id in batch {
                    ids.push(id);
                    heights.push(height);
                }
                admitted_chunks += u32::from(starts_chunk);
                let after = mailbox_memory(&mut s.session);
                assert!(
                    after <= DISPATCH_MEMORY_LIMIT,
                    "nonce {nonce}: dispatch took the Mailbox to {after} bytes"
                );
            }
            Err(error) => {
                assert!(
                    !fits,
                    "nonce {nonce}: refused at {memory} + {bound}: {error}"
                );
                assert_contract_panic_contains(
                    Err::<CallReceipt<()>, _>(error),
                    "Mailbox: storage full, dispatch is closed",
                );
                break;
            }
        }
    }
    // It ran until the next chunk no longer fit: with less room left than
    // a dispatch here (no metadata, one block) can take when it starts a
    // chunk and grows the chunk table, 7 pages.
    assert!(admitted_chunks >= 2, "admitted {admitted_chunks} chunks");
    let closed_at = mailbox_memory(&mut s.session);
    assert!(DISPATCH_MEMORY_LIMIT - closed_at < 7 * WASM_PAGE);
    assert_contract_panic_contains(
        s.mailbox_dispatch_via_tx(&OWNER_SK, REMOTE_DOMAIN, [0xD1u8; 32], b"late".to_vec()),
        "Mailbox: storage full, dispatch is closed",
    );

    assert_deliveries_continue_past_the_dispatch_limit(&mut s, closed_at);
}

/// Delivers messages until the Mailbox's memory is past 3 GiB, the limit
/// dispatch stops at, then delivers one more fresh message there. From
/// where dispatch closes, 5 pages below the limit, that takes about 18,000
/// deliveries at 22 to 26 bytes each, to the red-team contract, a recipient
/// that names itself as its ISM. No contract can deliver, so each is a
/// direct call, which runs `process` with no caller as a query does.
fn assert_deliveries_continue_past_the_dispatch_limit(s: &mut HyperlaneSession, closed_at: u64) {
    const PER_BATCH: u32 = 256;
    // 7 pages at 26 bytes a delivery, with room for the allocator's free
    // space; the loop stops at the first batch past the limit.
    const MAX_DELIVERIES: u32 = 64 * 1024;
    rt_set_returns(&mut s.session, 0, REDTEAM_ID);
    let filler = |nonce: u32| {
        message::encode(
            VERSION,
            nonce,
            REMOTE_DOMAIN,
            [0xABu8; 32],
            LOCAL_DOMAIN,
            REDTEAM_ID.to_bytes(),
            b"",
        )
    };
    let mut delivered = 0u32;
    while mailbox_memory(&mut s.session) <= DISPATCH_MEMORY_LIMIT {
        assert!(
            delivered < MAX_DELIVERIES,
            "{delivered} deliveries from {closed_at} bytes never passed the limit"
        );
        for nonce in delivered..delivered + PER_BATCH {
            s.mailbox_process(Vec::new(), filler(nonce))
                .unwrap_or_else(|e| panic!("delivery {nonce} failed: {e}"));
        }
        delivered += PER_BATCH;
    }
    let crossed = mailbox_memory(&mut s.session);
    assert!(
        crossed > DISPATCH_MEMORY_LIMIT,
        "{delivered} deliveries took the Mailbox from {closed_at} to {crossed} bytes"
    );
    assert_eq!(rt_counters(&mut s.session).0, delivered);
    assert_contract_panic_contains(
        s.mailbox_dispatch_via_tx(&OWNER_SK, REMOTE_DOMAIN, [0xD1u8; 32], b"later".to_vec()),
        "Mailbox: storage full, dispatch is closed",
    );

    // Past the limit, a relayed message is still reserved and handled, and
    // its replay refused.
    let body = b"delivered past 3 GiB".to_vec();
    let fresh = message::encode(
        VERSION,
        delivered,
        REMOTE_DOMAIN,
        [0xCDu8; 32],
        LOCAL_DOMAIN,
        TEST_RECIPIENT_ID.to_bytes(),
        &body,
    );
    let id = message::id(&fresh);
    assert!(!s.mailbox_delivered(id));
    let receipt = s
        .mailbox_process_via_tx(Vec::new(), fresh.clone())
        .expect("a fresh message is delivered past the dispatch limit");
    assert_eq!(delivered_ids(&receipt), vec![id]);
    assert!(s.mailbox_delivered(id));
    assert_eq!(s.recipient_handled_count(), 1);
    assert_eq!(s.recipient_last_origin(), REMOTE_DOMAIN);
    assert_eq!(s.recipient_last_sender(), [0xCDu8; 32]);
    assert_eq!(s.recipient_last_body(), body);
    assert_contract_panic(
        s.mailbox_process(Vec::new(), fresh),
        "Mailbox: already delivered",
    );
    assert_eq!(s.recipient_handled_count(), 1);
    assert!(mailbox_memory(&mut s.session) > DISPATCH_MEMORY_LIMIT);
}

/// The Mailbox records a delivered message by the first 16 bytes of its ID.
/// The same message is refused a second time and distinct messages are
/// delivered. An ID that shares only the last 16 bytes with a delivered one
/// is not reported delivered; one that shares the first 16 is, which for a
/// real message would take a second preimage of 128 bits of keccak256.
#[test]
fn test_delivered_set_keys_on_the_first_16_id_bytes() {
    let mut s = HyperlaneSession::new();
    let messages: Vec<Vec<u8>> = (0..300u32)
        .map(|nonce| {
            message::encode(
                VERSION,
                nonce,
                REMOTE_DOMAIN,
                [0xABu8; 32],
                LOCAL_DOMAIN,
                TEST_RECIPIENT_ID.to_bytes(),
                b"same body, distinct nonces",
            )
        })
        .collect();
    for encoded in &messages {
        assert!(!s.mailbox_delivered(message::id(encoded)));
        s.mailbox_process(Vec::new(), encoded.clone())
            .expect("a distinct message should be delivered");
    }
    assert_eq!(s.recipient_handled_count(), 300);
    for encoded in &messages {
        assert!(s.mailbox_delivered(message::id(encoded)));
        assert_contract_panic(
            s.mailbox_process(Vec::new(), encoded.clone()),
            "Mailbox: already delivered",
        );
    }
    assert_eq!(s.recipient_handled_count(), 300);

    let id = message::id(&messages[0]);
    let mut same_prefix = id;
    same_prefix[31] ^= 0xff;
    assert!(s.mailbox_delivered(same_prefix));
    let mut other_prefix = id;
    other_prefix[15] ^= 0xff;
    assert!(!s.mailbox_delivered(other_prefix));
}

/// A read-only query has no caller, so `quote_dispatch`, which quotes for
/// its caller, fails there. `quote_dispatch_for_sender` takes the sender
/// instead, and quotes what `dispatch` then charges it, through the deployed
/// hook set (MerkleTreeHook and ProtocolFee required, IGP by default).
#[test]
fn test_quote_dispatch_for_sender_works_without_a_signer() {
    let mut session = session_with_hooks();
    let sender = TEST_RECIPIENT_ID.to_bytes();
    let recipient = [0xBBu8; 32];
    let body = b"quoted without a signer".to_vec();
    let no_hook = ContractId::from_bytes([0u8; 32]);

    assert_contract_panic(
        session.direct_call::<_, u64>(
            MAILBOX_ID,
            "quote_dispatch",
            &(
                REMOTE_DOMAIN,
                recipient,
                body.clone(),
                Vec::<u8>::new(),
                no_hook,
            ),
        ),
        "Hyperlane: caller unavailable in query context",
    );
    let quote: u64 = query(
        &mut session,
        MAILBOX_ID,
        "quote_dispatch_for_sender",
        &(
            sender,
            REMOTE_DOMAIN,
            recipient,
            body.clone(),
            Vec::<u8>::new(),
            no_hook,
        ),
    );
    assert!(
        quote > 1000,
        "the protocol fee and the IGP's quote: {quote}"
    );

    let credit_before: u64 = query(&mut session, MAILBOX_ID, "fee_credit", &(sender,));
    session
        .call_public::<_, MessageId>(
            &OWNER_SK,
            TEST_RECIPIENT_ID,
            "dispatch_message",
            &(MAILBOX_ID, REMOTE_DOMAIN, recipient, body),
        )
        .expect("dispatch should succeed");
    let credit_after: u64 = query(&mut session, MAILBOX_ID, "fee_credit", &(sender,));
    assert_eq!(credit_before - credit_after, quote);
}

/// LOW-7: the Mailbox decodes a withdrawal's recipient key. A payer who names
/// a forged twin of the recipient's key, which has the recipient's
/// compressed encoding but other raw limbs, pays the recipient's real
/// account, and the event names it. The routes' owner-only wrappers go
/// through the same decoding (`assert_route_dispatch_credit_withdrawal`).
#[test]
fn test_withdrawal_to_a_forged_twin_pays_the_canonical_key() {
    let mut s = HyperlaneSession::new();
    let amount = MIN_NEW_PAYER_CREDIT;
    s.session
        .call_public_with_deposit::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(*OWNER_ID, 2 * amount),
            2 * amount,
        )
        .expect("funding should succeed");

    let mut paid = 0;
    for (name, twin) in forged_twins(&FRESH_PK) {
        let receipt = s
            .session
            .call_public::<_, ()>(
                &OWNER_SK,
                MAILBOX_ID,
                "withdraw_dispatch_credit",
                &(twin, amount),
            )
            .unwrap_or_else(|e| panic!("{name}: withdrawal should succeed: {e}"));
        paid += amount;
        assert_eq!(moonlight_balance(&mut s.session, &FRESH_PK), paid, "{name}");
        assert_eq!(moonlight_balance(&mut s.session, &twin), 0, "{name}");
        let event = receipt
            .events
            .iter()
            .find(|e| e.source == MAILBOX_ID && e.topic == events::DispatchFeeWithdrawn::TOPIC)
            .map(|e| test_session::rkyv_deserialize::<events::DispatchFeeWithdrawn>(&e.data))
            .expect("the withdrawal event");
        assert_eq!(
            event,
            events::DispatchFeeWithdrawn {
                payer: *OWNER_ID,
                recipient: *FRESH_ID,
                amount,
            },
            "{name}"
        );
        assert!(
            receipt.gas_spent < WITHDRAW_DEFAULT_GAS_LIMIT,
            "{name}: withdrawal spent {} gas, over the CLI default {WITHDRAW_DEFAULT_GAS_LIMIT}",
            receipt.gas_spent
        );
    }
    assert_eq!(
        query::<_, u64>(&mut s.session, MAILBOX_ID, "fee_credit", &(*OWNER_ID,)),
        0
    );
}

/// WarpDrc20 stores balances and allowances under `account_id`, a 32-byte
/// hash of the principal, but takes and emits principals as before. A
/// transfer to oneself changes no balance, and balances and allowances
/// moved or set to zero read back as zero.
#[test]
fn test_warp_drc20_balances_and_allowances_use_account_ids() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();
    assert_eq!(state_version(&mut session, WARP_DRC20_ID), 8);
    let owner = Drc20Account::moonlight(&OWNER_PK);
    let relayer = Drc20Account::moonlight(&RELAYER_PK);
    let contract = Drc20Account::Contract(TEST_RECIPIENT_ID);
    register_by_claim(
        &mut session,
        WARP_DRC20_ID,
        remote_router,
        0,
        &OWNER_SK,
        1_000,
    );
    assert_eq!(warp_drc20_balance_of(&mut session, owner), 1_000);

    let transfer = |session: &mut TestSession, sk: &AccountSecretKey, to, amount| {
        let receipt = session
            .call_public::<_, ()>(
                sk,
                WARP_DRC20_ID,
                "transfer",
                &hyperlane_dusk_types::drc20::TransferCall { to, amount },
            )
            .expect("transfer should succeed");
        receipt
            .events
            .iter()
            .filter(|e| e.source == WARP_DRC20_ID && e.topic == events::Drc20Transfer::TOPIC)
            .map(|e| test_session::rkyv_deserialize::<events::Drc20Transfer>(&e.data))
            .collect::<Vec<_>>()
    };

    // To oneself: no balance changes, and the event names the principal.
    assert_eq!(
        transfer(&mut session, &OWNER_SK, owner, 400),
        vec![events::Drc20Transfer {
            from: owner,
            to: owner,
            amount: 400,
        }]
    );
    assert_eq!(warp_drc20_balance_of(&mut session, owner), 1_000);

    // The whole balance there and back.
    transfer(&mut session, &OWNER_SK, relayer, 1_000);
    assert_eq!(warp_drc20_balance_of(&mut session, owner), 0);
    assert_eq!(warp_drc20_balance_of(&mut session, relayer), 1_000);
    transfer(&mut session, &RELAYER_SK, owner, 1_000);
    assert_eq!(warp_drc20_balance_of(&mut session, owner), 1_000);
    assert_eq!(warp_drc20_balance_of(&mut session, relayer), 0);
    assert_eq!(
        query::<_, u64>(&mut session, WARP_DRC20_ID, "total_supply", &()),
        1_000
    );

    let allowance = |session: &mut TestSession, owner, spender| {
        query::<_, u64>(
            session,
            WARP_DRC20_ID,
            "allowance",
            &Drc20Allowance { owner, spender },
        )
    };
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_ID,
            "approve",
            &Drc20ApproveCall {
                spender: relayer,
                amount: 300,
            },
        )
        .expect("approve should succeed");
    assert_eq!(allowance(&mut session, owner, relayer), 300);
    assert_eq!(allowance(&mut session, relayer, owner), 0);
    session
        .call_public::<_, ()>(
            &RELAYER_SK,
            WARP_DRC20_ID,
            "transfer_from",
            &hyperlane_dusk_types::drc20::TransferFromCall {
                owner,
                to: contract,
                amount: 100,
            },
        )
        .expect("transfer_from should succeed");
    assert_eq!(allowance(&mut session, owner, relayer), 200);
    assert_eq!(warp_drc20_balance_of(&mut session, owner), 900);
    assert_eq!(warp_drc20_balance_of(&mut session, contract), 100);
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_ID,
            "approve",
            &Drc20ApproveCall {
                spender: relayer,
                amount: 0,
            },
        )
        .expect("approving zero should succeed");
    assert_eq!(allowance(&mut session, owner, relayer), 0);
    assert_contract_panic(
        session.call_public::<_, ()>(
            &RELAYER_SK,
            WARP_DRC20_ID,
            "transfer_from",
            &hyperlane_dusk_types::drc20::TransferFromCall {
                owner,
                to: contract,
                amount: 1,
            },
        ),
        "WarpDrc20: allowance too low",
    );
}

/// An allowance set to zero leaves no entry: a contract that approves 64
/// spenders, the most an owner holds, and zeroes them, can approve 64 new
/// ones, round after round, and after the first round its allowances take
/// no more memory.
#[test]
fn test_warp_drc20_zeroed_allowances_free_their_entries() {
    let (mut session, _) = session_with_warp_drc20_flow();
    rt_deploy(&mut session);
    let approve_round = |session: &mut TestSession, round: u32, amount: u64| {
        let actions: Vec<RtAction> = (0..64u32)
            .map(|index| drc20_approve(drc20_spender(0x51, round * 64 + index), amount))
            .collect();
        run_direct(session, actions).expect("a round of approvals should succeed");
    };
    approve_round(&mut session, 0, 1);
    approve_round(&mut session, 0, 0);
    let after_first = session.memory_len(WARP_DRC20_ID);
    for round in 1..32 {
        approve_round(&mut session, round, 1);
        approve_round(&mut session, round, 0);
    }
    assert_eq!(session.memory_len(WARP_DRC20_ID), after_first);

    let attacker = Drc20Account::Contract(REDTEAM_ID);
    approve_round(&mut session, 32, 1);
    assert_eq!(
        drc20_allowance(&mut session, attacker, drc20_spender(0x51, 7)),
        0
    );
    assert_eq!(
        drc20_allowance(&mut session, attacker, drc20_spender(0x51, 32 * 64 + 7)),
        1
    );
}

// =============================================================================
// Storage growth limits (docs/storage.md)
// =============================================================================

/// The red-team contract makes `actions` as a direct call, with nothing
/// below it, as a contract called by a query or a deployment's `init` runs;
/// any failure fails them all.
fn run_direct(
    session: &mut TestSession,
    actions: Vec<RtAction>,
) -> Result<CallReceipt<Vec<u8>>, ContractError> {
    let args = rkyv::to_bytes::<_, 4096>(&(actions, true))
        .expect("the batch should serialize")
        .to_vec();
    session.direct_call_raw(REDTEAM_ID, "run", args)
}

/// A made-up Moonlight principal, the `index`-th of `seed`.
fn drc20_spender(seed: u8, index: u32) -> Drc20Account {
    let mut key = [seed; hyperlane_dusk_types::drc20::BLS_PUBLIC_KEY_BYTES];
    key[..4].copy_from_slice(&index.to_le_bytes());
    Drc20Account::Moonlight(key)
}

fn drc20_approve(spender: Drc20Account, amount: u64) -> RtAction {
    rt_action(
        WARP_DRC20_ID,
        "approve",
        &Drc20ApproveCall { spender, amount },
    )
}

fn drc20_transfer(to: Drc20Account, amount: u64) -> RtAction {
    rt_action(
        WARP_DRC20_ID,
        "transfer",
        &hyperlane_dusk_types::drc20::TransferCall { to, amount },
    )
}

fn drc20_allowance(session: &mut TestSession, owner: Drc20Account, spender: Drc20Account) -> u64 {
    query(
        session,
        WARP_DRC20_ID,
        "allowance",
        &Drc20Allowance { owner, spender },
    )
}

/// The red-team contract gets `amount` WarpDrc20 tokens the way any
/// contract does: a transfer to it waits in escrow and it claims it.
fn give_attacker_tokens(session: &mut TestSession, remote_router: H256, nonce: u32, amount: u64) {
    session
        .direct_call::<_, ()>(
            MAILBOX_ID,
            "process",
            &(
                Vec::<u8>::new(),
                inbound_transfer(
                    WARP_DRC20_ID,
                    remote_router,
                    nonce,
                    REDTEAM_ID.to_bytes(),
                    amount,
                ),
            ),
        )
        .expect("a transfer to the contract waits in escrow");
    run_direct(
        session,
        vec![rt_action(WARP_DRC20_ID, "claim_pending_contract", &())],
    )
    .expect("the contract claims its tokens");
}

/// A fresh message from `REMOTE_DOMAIN` to the red-team contract, which
/// names itself as its ISM when `rt_set_returns` says so.
fn redteam_inbound(nonce: u32, body: &[u8]) -> Vec<u8> {
    message::encode(
        VERSION,
        nonce,
        REMOTE_DOMAIN,
        [0xABu8; 32],
        LOCAL_DOMAIN,
        REDTEAM_ID.to_bytes(),
        body,
    )
}

/// `process` runs only as a transaction's own call or a query, here a
/// direct call. A contract can't call it: not from a transaction, not with
/// nothing below it, as a contract called by a query or a deployment's
/// `init` runs, and not from a recipient's `handle` during a delivery (the
/// HIGH-6 case, with a fresh message the Mailbox would otherwise deliver).
/// So no contract can make many deliveries in one transaction.
#[test]
fn test_contracts_cannot_call_process() {
    let mut s = HyperlaneSession::new();
    rt_deploy(&mut s.session);
    rt_set_returns(&mut s.session, 0, REDTEAM_ID);
    let refused = "Mailbox: process must be a transaction's own call";
    let first = redteam_inbound(1, b"through a contract");
    let deliver =
        |encoded: &Vec<u8>| rt_action(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded.clone()));

    assert_contract_panic_contains(run_batch(&mut s.session, vec![deliver(&first)]), refused);
    assert_contract_panic_contains(run_direct(&mut s.session, vec![deliver(&first)]), refused);
    assert_deploy_panic(
        rt_deploy_at(
            &mut s.session,
            ContractId::from_bytes([43; 32]),
            vec![deliver(&first)],
        ),
        &format!("RedteamAttacker: process failed: Panic(\"{refused}\")"),
    );
    assert!(!s.mailbox_delivered(message::id(&first)));
    assert_eq!(rt_counters(&mut s.session).0, 0);

    // The recipient's `handle` delivers another message during its own.
    let outer = redteam_inbound(2, b"outer");
    let nested = redteam_inbound(3, b"nested");
    rt_configure(&mut s.session, RT_HANDLE, vec![deliver(&nested)], false);
    let receipt = s
        .mailbox_process_via_tx(Vec::new(), outer.clone())
        .expect("the outer delivery succeeds");
    assert_eq!(rt_results(&mut s.session), vec![false]);
    assert_eq!(delivered_ids(&receipt), vec![message::id(&outer)]);
    assert!(!s.mailbox_delivered(message::id(&nested)));
    assert_eq!(rt_counters(&mut s.session).0, 1);

    // A transaction's own call and a direct call deliver both.
    rt_configure(&mut s.session, RT_HANDLE, Vec::new(), false);
    s.mailbox_process_via_tx(Vec::new(), first.clone())
        .expect("a transaction delivers");
    s.mailbox_process(Vec::new(), nested.clone())
        .expect("a direct call delivers");
    assert!(s.mailbox_delivered(message::id(&first)));
    assert!(s.mailbox_delivered(message::id(&nested)));
    assert_eq!(rt_counters(&mut s.session).0, 3);
}

/// `announce` runs only as a transaction's own call or a query, so every
/// announcement takes a transaction: a contract can't announce, from a
/// transaction or with nothing below it.
#[test]
fn test_contracts_cannot_announce() {
    let mut session = session_with_validator_announce();
    rt_deploy(&mut session);
    let refused = "ValidatorAnnounce: announce must be a transaction's own call";
    let location = alloc::string::String::from("s3://made-by-a-contract/eu-west-1");
    let (validator, signature) = validator_announcement(&location);
    let announce = rt_action(
        VALIDATOR_ANNOUNCE_ID,
        "announce",
        &(validator, location.clone(), signature.clone()),
    );
    let count = |session: &mut TestSession| -> u32 {
        query(
            session,
            VALIDATOR_ANNOUNCE_ID,
            "announced_validator_count",
            &(),
        )
    };

    assert_contract_panic_contains(run_batch(&mut session, vec![announce.clone()]), refused);
    assert_contract_panic_contains(run_direct(&mut session, vec![announce.clone()]), refused);
    assert_deploy_panic(
        rt_deploy_at(
            &mut session,
            ContractId::from_bytes([43; 32]),
            vec![announce],
        ),
        &format!("RedteamAttacker: announce failed: Panic(\"{refused}\")"),
    );
    assert_eq!(count(&mut session), 0);

    session
        .call_public::<_, bool>(
            &OWNER_SK,
            VALIDATOR_ANNOUNCE_ID,
            "announce",
            &(validator, location, signature),
        )
        .expect("a transaction announces");
    assert_eq!(count(&mut session), 1);
}

/// A payer with no dispatch credit takes at least 0.01 DUSK; a payer with
/// credit takes any amount. Credit withdrawn to zero leaves no entry, so the
/// payer needs the minimum again (as after a dispatch spends all of it,
/// `redteam_hook_callbacks_cannot_double_spend_dispatch_credit`). Routes
/// fund their own credit through the transfer contract, for each transfer,
/// with no minimum.
#[test]
fn test_a_new_dispatch_payer_funds_at_least_a_hundredth_of_a_dusk() {
    let mut s = HyperlaneSession::new();
    let refused = "Mailbox: a payer with no credit takes at least 0.01 DUSK";
    let fund = |s: &mut HyperlaneSession, amount: u64| {
        s.session.call_public_with_deposit::<_, ()>(
            &RELAYER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(*OWNER_ID, amount),
            amount,
        )
    };
    let credit = |s: &mut HyperlaneSession| -> u64 {
        query(&mut s.session, MAILBOX_ID, "fee_credit", &(*OWNER_ID,))
    };

    assert_contract_panic(fund(&mut s, MIN_NEW_PAYER_CREDIT - 1), refused);
    assert_eq!(credit(&mut s), 0);
    assert_eq!(s.session.contract_balance(&MAILBOX_ID).unwrap(), 0);

    fund(&mut s, MIN_NEW_PAYER_CREDIT).expect("the minimum funds a new payer");
    fund(&mut s, 1).expect("a payer with credit takes any amount");
    assert_eq!(credit(&mut s), MIN_NEW_PAYER_CREDIT + 1);

    s.session
        .call_public_with_gas_limit::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "withdraw_dispatch_credit",
            &(*OWNER_PK, MIN_NEW_PAYER_CREDIT + 1),
            WITHDRAW_DEFAULT_GAS_LIMIT,
        )
        .expect("the payer withdraws all its credit");
    assert_eq!(credit(&mut s), 0);
    assert_contract_panic(fund(&mut s, 1), refused);
    fund(&mut s, MIN_NEW_PAYER_CREDIT).expect("the minimum funds the payer again");
    assert_eq!(credit(&mut s), MIN_NEW_PAYER_CREDIT);
    assert_eq!(
        s.session.contract_balance(&MAILBOX_ID).unwrap(),
        MIN_NEW_PAYER_CREDIT
    );
}

/// An owner holds at most 64 allowances. Approving a 65th spender reverts;
/// changing an allowance it holds doesn't. An allowance set to zero frees
/// its place, whether the owner approves zero or a spender uses it up with
/// `transfer_from`. Each owner has its own 64.
#[test]
fn test_warp_drc20_owner_holds_at_most_64_allowances() {
    let (mut session, remote_router) = session_with_warp_drc20_flow();
    rt_deploy(&mut session);
    give_attacker_tokens(&mut session, remote_router, 0, 1_000);
    let attacker = Drc20Account::Contract(REDTEAM_ID);
    let relayer = Drc20Account::moonlight(&RELAYER_PK);
    let spender = |index| drc20_spender(0x53, index);
    let limit = "WarpDrc20: an owner holds at most 64 allowances";

    // 63 made-up spenders and the relayer, which can spend.
    let mut actions: Vec<RtAction> = (0..63).map(|i| drc20_approve(spender(i), 1)).collect();
    actions.push(drc20_approve(relayer, 10));
    run_direct(&mut session, actions).expect("an owner holds 64 allowances");
    assert_contract_panic_contains(
        run_direct(&mut session, vec![drc20_approve(spender(63), 1)]),
        limit,
    );
    run_direct(&mut session, vec![drc20_approve(spender(0), 5)])
        .expect("changing an allowance it holds works");
    assert_eq!(drc20_allowance(&mut session, attacker, spender(0)), 5);

    run_direct(&mut session, vec![drc20_approve(spender(1), 0)])
        .expect("approving zero removes an allowance");
    run_direct(&mut session, vec![drc20_approve(spender(63), 1)])
        .expect("which makes room for one new spender");
    assert_contract_panic_contains(
        run_direct(&mut session, vec![drc20_approve(spender(64), 1)]),
        limit,
    );

    session
        .call_public::<_, ()>(
            &RELAYER_SK,
            WARP_DRC20_ID,
            "transfer_from",
            &hyperlane_dusk_types::drc20::TransferFromCall {
                owner: attacker,
                to: relayer,
                amount: 10,
            },
        )
        .expect("the relayer uses up its allowance");
    assert_eq!(drc20_allowance(&mut session, attacker, relayer), 0);
    run_direct(&mut session, vec![drc20_approve(spender(64), 1)])
        .expect("an allowance used up makes room too");
    assert_contract_panic_contains(
        run_direct(&mut session, vec![drc20_approve(spender(65), 1)]),
        limit,
    );

    session
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_ID,
            "approve",
            &Drc20ApproveCall {
                spender: spender(0),
                amount: 1,
            },
        )
        .expect("another owner has its own 64");
}

/// The most memory a transfer to a new holder can allocate: 13 B-tree
/// nodes, a page each (the WarpDrc20's `NEW_BALANCE_MEMORY_BOUND`).
const NEW_BALANCE_MEMORY_BOUND: u64 = 13 * WASM_PAGE;
/// The most an approval of a new spender can allocate, one node more
/// (`NEW_ALLOWANCE_MEMORY_BOUND`).
const NEW_ALLOWANCE_MEMORY_BOUND: u64 = 14 * WASM_PAGE;
/// The memory WarpDrc20's transfers to new holders and approvals of new
/// spenders never pass (`HOLDER_MEMORY_LIMIT`).
const HOLDER_MEMORY_LIMIT: u64 = 3 * 1024 * 1024 * 1024;

/// Transfers to new holders and approvals of new spenders stop where they
/// could take WarpDrc20's memory past 3 GiB, and the last GiB stays for the
/// bridge. The token here starts 16 pages below the limit, as if holders had
/// filled it. Every transfer to a new holder it admits leaves its memory at
/// or below 3 GiB, and it admits exactly those whose bound, 13 pages, fits.
/// Then transfers between holders and changes to allowances still work,
/// and the bridge goes on past 3 GiB: escrow for new recipients, mints to
/// accounts with no balance, and claims that register their account.
#[test]
fn test_warp_drc20_new_holders_and_allowances_stop_at_3_gib() {
    let limit_pages = HOLDER_MEMORY_LIMIT / WASM_PAGE;
    let token = with_initial_memory_pages(WARP_DRC20_BYTECODE, limit_pages - 16);
    let (mut session, remote_router) = session_with_warp_drc20_flow_from(&token);
    rt_deploy(&mut session);
    let memory = |session: &mut TestSession| session.memory_len(WARP_DRC20_ID);
    let start = memory(&mut session);
    assert!(
        (HOLDER_MEMORY_LIMIT - 16 * WASM_PAGE..=HOLDER_MEMORY_LIMIT - 14 * WASM_PAGE)
            .contains(&start),
        "starts at {start}"
    );
    give_attacker_tokens(&mut session, remote_router, 0, 1_000_000);
    let attacker = Drc20Account::Contract(REDTEAM_ID);
    let allowed = drc20_spender(0x5A, 0);
    run_direct(&mut session, vec![drc20_approve(allowed, 10)])
        .expect("an allowance made while there is room");
    let holder = |index| drc20_spender(0x61, index);

    // Up to the limit, transfers go 64 to a call; one that would cross it
    // fails as a whole, and they go one at a time from there. Approvals of
    // new spenders, whose bound is a page more, close a page earlier.
    let mut holders = 0u32;
    let mut approvals_closed_first = false;
    loop {
        let before = memory(&mut session);
        let fits = before + NEW_BALANCE_MEMORY_BOUND <= HOLDER_MEMORY_LIMIT;
        if fits && before + NEW_ALLOWANCE_MEMORY_BOUND > HOLDER_MEMORY_LIMIT {
            assert_contract_panic_contains(
                run_direct(&mut session, vec![drc20_approve(drc20_spender(0x5A, 1), 1)]),
                "WarpDrc20: storage full, new allowances are closed",
            );
            approvals_closed_first = true;
        }
        if before + NEW_BALANCE_MEMORY_BOUND + WASM_PAGE <= HOLDER_MEMORY_LIMIT {
            let batch = (holders..holders + 64)
                .map(|i| drc20_transfer(holder(i), 1))
                .collect();
            if run_direct(&mut session, batch).is_ok() {
                holders += 64;
                assert!(memory(&mut session) <= HOLDER_MEMORY_LIMIT);
                continue;
            }
        }
        match run_direct(&mut session, vec![drc20_transfer(holder(holders), 1)]) {
            Ok(_) => {
                assert!(fits, "holder {holders}: admitted at {before}");
                holders += 1;
                let after = memory(&mut session);
                assert!(
                    after <= HOLDER_MEMORY_LIMIT,
                    "holder {holders}: a transfer took the token to {after} bytes"
                );
            }
            Err(error) => {
                assert!(!fits, "holder {holders}: refused at {before}: {error}");
                assert_contract_panic_contains(
                    Err::<CallReceipt<()>, _>(error),
                    "WarpDrc20: storage full, transfers to new holders are closed",
                );
                break;
            }
        }
    }
    let closed_at = memory(&mut session);
    assert!(holders > 1_000, "{holders} new holders");
    assert!(approvals_closed_first);
    assert!(HOLDER_MEMORY_LIMIT - closed_at < NEW_BALANCE_MEMORY_BOUND);
    assert_contract_panic_contains(
        run_direct(&mut session, vec![drc20_approve(drc20_spender(0x5A, 1), 1)]),
        "WarpDrc20: storage full, new allowances are closed",
    );

    // Between holders, and changes to allowances, nothing is added.
    run_direct(
        &mut session,
        vec![
            drc20_transfer(holder(0), 5),
            drc20_approve(allowed, 20),
            drc20_approve(allowed, 0),
        ],
    )
    .expect("transfers to holders and allowance changes still work");
    assert_eq!(warp_drc20_balance_of(&mut session, holder(0)), 6);
    assert_eq!(drc20_allowance(&mut session, attacker, allowed), 0);

    // The bridge escrows transfers to new recipients until the memory is
    // past 3 GiB.
    let mut nonce = 1u32;
    while memory(&mut session) <= HOLDER_MEMORY_LIMIT {
        let mut recipient = [0xE5; 32];
        recipient[..4].copy_from_slice(&nonce.to_le_bytes());
        session
            .direct_call::<_, ()>(
                MAILBOX_ID,
                "process",
                &(
                    Vec::<u8>::new(),
                    inbound_transfer(WARP_DRC20_ID, remote_router, nonce, recipient, 1),
                ),
            )
            .unwrap_or_else(|e| panic!("escrow {nonce} failed: {e}"));
        nonce += 1;
        assert!(nonce < 64 * 1024, "escrow never passed the limit");
    }
    // Past it, a claim registers its account and mints to it, a holder
    // that had none, and the next transfer mints to it directly.
    register_by_claim(
        &mut session,
        WARP_DRC20_ID,
        remote_router,
        nonce,
        &OWNER_SK,
        7,
    );
    session
        .direct_call::<_, ()>(
            MAILBOX_ID,
            "process",
            &(
                Vec::<u8>::new(),
                inbound_transfer(WARP_DRC20_ID, remote_router, nonce + 1, *OWNER_ID, 3),
            ),
        )
        .expect("a mint past 3 GiB");
    let owner = Drc20Account::moonlight(&OWNER_PK);
    assert_eq!(warp_drc20_balance_of(&mut session, owner), 10);
    assert!(memory(&mut session) > HOLDER_MEMORY_LIMIT);

    // Holders can still pay each other, but not new accounts.
    session
        .call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_ID,
            "transfer",
            &hyperlane_dusk_types::drc20::TransferCall {
                to: holder(1),
                amount: 4,
            },
        )
        .expect("a transfer to a holder past 3 GiB");
    assert_contract_panic_contains(
        session.call_public::<_, ()>(
            &OWNER_SK,
            WARP_DRC20_ID,
            "transfer",
            &hyperlane_dusk_types::drc20::TransferCall {
                to: holder(holders),
                amount: 1,
            },
        ),
        "WarpDrc20: storage full, transfers to new holders are closed",
    );
}
