// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Gas and memory costs of the operations that grow contract storage, for
// docs/storage.md. Ignored by default, and slow:
//
//   make all
//   cargo test -p hyperlane-dusk-integration-tests --test storage_costs -- --ignored --nocapture
//
// Each measurement prints one `cost|<operation>|<metric>|<value>` line.
//
// - `tx gas`: the gas a Moonlight transaction making one such call spends,
//   the 1.75M signature check included: what a user, relayer or validator
//   pays per operation.
// - `marginal gas`: the gas one more call adds to a transaction that makes
//   many, from a contract: what someone filling a contract's memory pays.
//   Deliveries and announcements are a transaction's own calls, which no
//   contract can make, so they report `tx gas` only.
// - `<contract> bytes`: how much the contract's memory grew per call, over
//   many calls, measured in whole 64 KiB pages.
//
// HYPERLANE_WASM_DIR=<dir> measures the contract WASMs in <dir> instead of
// this checkout's build, for example an earlier commit's, to compare. The
// harness only makes calls that both versions have.

use std::path::PathBuf;
use std::sync::LazyLock;

use dusk_bytes::Serializable;
use dusk_core::abi::{ContractError, ContractId};
use dusk_core::dusk;
use dusk_core::signatures::bls::{PublicKey as AccountPublicKey, SecretKey as AccountSecretKey};
use dusk_vm::{CallReceipt, ContractData};
use rand::rngs::StdRng;
use rand::SeedableRng;
use secp256k1::{Message as SecpMessage, Secp256k1, SecretKey as SecpSecretKey};

use hyperlane_dusk_types::drc20::{Account, ApproveCall, TransferCall};
use hyperlane_dusk_types::{
    checkpoint, message, token_message, DomainGasConfig, EthAddress, MessageId, H256, VERSION,
};

mod test_session;
use test_session::TestSession;

const MAILBOX_ID: ContractId = ContractId::from_bytes([10; 32]);
const MERKLE_TREE_HOOK_ID: ContractId = ContractId::from_bytes([11; 32]);
const TEST_MOCK_ID: ContractId = ContractId::from_bytes([12; 32]);
const PROTOCOL_FEE_ID: ContractId = ContractId::from_bytes([14; 32]);
const IGP_ID: ContractId = ContractId::from_bytes([15; 32]);
const ISM_MULTISIG_ID: ContractId = ContractId::from_bytes([16; 32]);
const AGGREGATION_HOOK_ID: ContractId = ContractId::from_bytes([19; 32]);
const VALIDATOR_ANNOUNCE_ID: ContractId = ContractId::from_bytes([20; 32]);
const WARP_DRC20_ID: ContractId = ContractId::from_bytes([21; 32]);
const WARP_NATIVE_ID: ContractId = ContractId::from_bytes([22; 32]);
const ATTACKER_ID: ContractId = ContractId::from_bytes([40; 32]);

const DEPLOYER: [u8; 64] = [0u8; 64];
const LOCAL_DOMAIN: u32 = 4242;
const REMOTE_DOMAIN: u32 = 1;
const REMOTE_ROUTER: H256 = [0xBB; 32];
const NO_HOOK: ContractId = ContractId::from_bytes([0; 32]);
const WASM_PAGE: u64 = 64 * 1024;

static OWNER_SK: LazyLock<AccountSecretKey> =
    LazyLock::new(|| AccountSecretKey::random(&mut StdRng::seed_from_u64(0x48595045)));
static OWNER_PK: LazyLock<AccountPublicKey> = LazyLock::new(|| AccountPublicKey::from(&*OWNER_SK));
static OWNER_ID: LazyLock<H256> = LazyLock::new(|| message::keccak256(&OWNER_PK.to_bytes()));
static RELAYER_SK: LazyLock<AccountSecretKey> =
    LazyLock::new(|| AccountSecretKey::random(&mut StdRng::seed_from_u64(0xDE1A7E00)));
static RELAYER_PK: LazyLock<AccountPublicKey> =
    LazyLock::new(|| AccountPublicKey::from(&*RELAYER_SK));

/// The directory the contract WASMs are read from.
static WASM_DIR: LazyLock<PathBuf> = LazyLock::new(|| {
    std::env::var_os("HYPERLANE_WASM_DIR").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../target/contract/wasm32-unknown-unknown/release")
        },
        PathBuf::from,
    )
});

fn wasm(name: &str) -> Vec<u8> {
    let path = WASM_DIR.join(format!("hyperlane_dusk_{name}.wasm"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn deploy<A>(session: &mut TestSession, name: &str, id: ContractId, init: Option<&A>)
where
    A: for<'b> rkyv::Serialize<dusk_core::abi::StandardBufSerializer<'b>>,
{
    let bytecode = wasm(name);
    let data = ContractData::builder().owner(DEPLOYER).contract_id(id);
    let result = match init {
        Some(init) => session.deploy(&bytecode, data.init_arg(init)),
        None => session.deploy(&bytecode, data),
    };
    result.unwrap_or_else(|e| panic!("deploying {name} failed: {e}"));
}

fn deploy_plain(session: &mut TestSession, name: &str, id: ContractId) {
    deploy::<()>(session, name, id, None);
}

/// The red-team contract at `id`, whose `init` makes no calls.
fn deploy_attacker(session: &mut TestSession, id: ContractId) {
    deploy(session, "redteam_attacker", id, Some(&Vec::<Action>::new()));
}

fn report(operation: &str, metric: &str, value: impl std::fmt::Display) {
    println!("cost|{operation}|{metric}|{value}");
}

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

/// One call the attacker contract replays: target, function, archived
/// argument.
type Action = (ContractId, String, Vec<u8>);

fn action<A>(target: ContractId, fn_name: &str, args: &A) -> Action
where
    A: rkyv::Serialize<rkyv::ser::serializers::AllocSerializer<1024>>,
{
    (
        target,
        String::from(fn_name),
        rkyv::to_bytes::<_, 1024>(args).unwrap().to_vec(),
    )
}

/// The attacker contract makes `actions` in one transaction signed by the
/// owner; any failed call fails the transaction.
fn run(session: &mut TestSession, actions: Vec<Action>) -> CallReceipt<Vec<u8>> {
    let args = rkyv::to_bytes::<_, 4096>(&(actions, true))
        .expect("the batch should serialize")
        .to_vec();
    session
        .call_public_raw(&OWNER_SK, ATTACKER_ID, "run", args)
        .unwrap_or_else(|e| panic!("the batch should succeed: {e}"))
}

/// Runs `total` calls from the attacker contract, `per_batch` to a
/// transaction, `make(i)` being the `i`-th. Reports the marginal gas of one
/// call, and the memory each contract in `grows` gained per call, measured
/// after a first batch has warmed it up.
fn measure_batches(
    session: &mut TestSession,
    operation: &str,
    total: u32,
    per_batch: u32,
    grows: &[(&str, ContractId)],
    mut make: impl FnMut(u32) -> Action,
) {
    // One call alone, then a full batch: the difference is per_batch - 1
    // calls with the transaction's own cost taken out.
    let single = run(session, vec![make(0)]).gas_spent;
    let batch = run(session, (1..=per_batch).map(&mut make).collect()).gas_spent;
    let marginal = (batch - single) / u64::from(per_batch - 1);
    report(operation, "marginal gas", marginal);

    let before: Vec<u64> = grows
        .iter()
        .map(|(_, id)| session.memory_len(*id))
        .collect();
    let mut next = per_batch + 1;
    while next < per_batch + 1 + total {
        let count = per_batch.min(per_batch + 1 + total - next);
        run(session, (next..next + count).map(&mut make).collect());
        next += count;
    }
    for ((name, id), before) in grows.iter().zip(before) {
        let grown = session.memory_len(*id) - before;
        report(
            operation,
            &format!("{name} bytes"),
            format!("{:.1}", grown as f64 / f64::from(total)),
        );
    }
}

/// Runs calls that must each be a transaction's own, `make(i)` being the
/// `i`-th. Reports the gas of a transaction making one, over 16, and the
/// memory each contract in `grows` gained per call over `total` more, made
/// as direct calls: they run with nothing below them, as a transaction's
/// own call does, without its signature check.
fn measure_top_level(
    session: &mut TestSession,
    operation: &str,
    total: u32,
    grows: &[(&str, ContractId)],
    mut make: impl FnMut(u32) -> Action,
) {
    const SAMPLES: u32 = 16;
    let gas = average_tx_gas(SAMPLES, |i| {
        let (target, fn_name, args) = make(i);
        session
            .call_public_raw(&RELAYER_SK, target, &fn_name, args)
            .map(|receipt| receipt.gas_spent)
    });
    report(operation, "tx gas", gas);

    let before: Vec<u64> = grows
        .iter()
        .map(|(_, id)| session.memory_len(*id))
        .collect();
    for i in SAMPLES..SAMPLES + total {
        let (target, fn_name, args) = make(i);
        session
            .direct_call_raw(target, &fn_name, args)
            .unwrap_or_else(|e| panic!("{operation} {i} failed: {e}"));
    }
    for ((name, id), before) in grows.iter().zip(before) {
        let grown = session.memory_len(*id) - before;
        report(
            operation,
            &format!("{name} bytes"),
            format!("{:.1}", grown as f64 / f64::from(total)),
        );
    }
}

/// The deployed hook set: MerkleTreeHook and ProtocolFee aggregated as the
/// required hook, the IGP as the default hook, TestMock as the default ISM.
fn session_with_deployed_hooks() -> TestSession {
    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, dusk(1_000_000.0)),
        (&*RELAYER_PK, dusk(1_000.0)),
    ]);
    deploy_plain(&mut session, "test_mock", TEST_MOCK_ID);
    deploy(
        &mut session,
        "merkle_tree_hook",
        MERKLE_TREE_HOOK_ID,
        Some(&(AGGREGATION_HOOK_ID,)),
    );
    deploy(
        &mut session,
        "protocol_fee",
        PROTOCOL_FEE_ID,
        Some(&(
            1_000u64,
            10_000u64,
            AGGREGATION_HOOK_ID,
            *OWNER_ID,
            *OWNER_ID,
        )),
    );
    deploy(
        &mut session,
        "aggregation_hook",
        AGGREGATION_HOOK_ID,
        Some(&(MAILBOX_ID, vec![MERKLE_TREE_HOOK_ID, PROTOCOL_FEE_ID])),
    );
    let pricing = DomainGasConfig {
        gas_overhead: 0,
        token_exchange_rate: 10_000_000_000,
        gas_price: 1,
    };
    deploy(
        &mut session,
        "igp",
        IGP_ID,
        Some(&(
            MAILBOX_ID,
            *OWNER_ID,
            *OWNER_ID,
            vec![(REMOTE_DOMAIN, pricing)],
        )),
    );
    deploy(
        &mut session,
        "mailbox",
        MAILBOX_ID,
        Some(&(
            LOCAL_DOMAIN,
            *OWNER_ID,
            TEST_MOCK_ID,
            IGP_ID,
            AGGREGATION_HOOK_ID,
        )),
    );
    deploy_attacker(&mut session, ATTACKER_ID);
    session
}

/// A Mailbox with free hooks (TestMock by default, MerkleTreeHook required).
fn session_with_free_hooks() -> TestSession {
    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, dusk(1_000_000.0)),
        (&*RELAYER_PK, dusk(1_000.0)),
    ]);
    deploy_plain(&mut session, "test_mock", TEST_MOCK_ID);
    deploy(
        &mut session,
        "merkle_tree_hook",
        MERKLE_TREE_HOOK_ID,
        Some(&(MAILBOX_ID,)),
    );
    deploy(
        &mut session,
        "mailbox",
        MAILBOX_ID,
        Some(&(
            LOCAL_DOMAIN,
            *OWNER_ID,
            TEST_MOCK_ID,
            TEST_MOCK_ID,
            MERKLE_TREE_HOOK_ID,
        )),
    );
    deploy_attacker(&mut session, ATTACKER_ID);
    session
}

fn fund_dispatch(session: &mut TestSession, payer: H256, amount: u64) {
    session
        .call_public_with_deposit::<_, ()>(
            &OWNER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(payer, amount),
            amount,
        )
        .expect("funding dispatch credit should succeed");
}

/// The average gas of `count` transactions `make(i)` sends.
fn average_tx_gas(count: u32, mut make: impl FnMut(u32) -> Result<u64, ContractError>) -> u64 {
    let total: u64 = (0..count)
        .map(|i| make(i).unwrap_or_else(|e| panic!("transaction {i} failed: {e}")))
        .sum();
    total / u64::from(count)
}

fn dispatch_action(index: u32, hook: ContractId) -> Action {
    action(
        MAILBOX_ID,
        "dispatch",
        &(
            REMOTE_DOMAIN,
            [0xD1u8; 32],
            index.to_le_bytes().to_vec(),
            Vec::<u8>::new(),
            hook,
        ),
    )
}

#[test]
#[ignore = "measurement for docs/storage.md; slow"]
fn dispatch_costs() {
    let mut session = session_with_deployed_hooks();
    fund_dispatch(&mut session, *OWNER_ID, dusk(100.0));
    fund_dispatch(&mut session, ATTACKER_ID.to_bytes(), dusk(10_000.0));

    // A dispatch from an account through the deployed hook set, the first
    // two of each 32 skipped so the logs' first chunks are allocated.
    let gas = average_tx_gas(32, |i| {
        session
            .call_public::<_, MessageId>(
                &OWNER_SK,
                MAILBOX_ID,
                "dispatch_default",
                &(REMOTE_DOMAIN, [0xD1u8; 32], i.to_le_bytes().to_vec()),
            )
            .map(|receipt| receipt.gas_spent)
    });
    report("dispatch, deployed hooks", "tx gas", gas);
    let gas = average_tx_gas(32, |i| {
        session
            .call_public::<_, MessageId>(
                &OWNER_SK,
                MAILBOX_ID,
                "dispatch",
                &(
                    REMOTE_DOMAIN,
                    [0xD1u8; 32],
                    i.to_le_bytes().to_vec(),
                    Vec::<u8>::new(),
                    TEST_MOCK_ID,
                ),
            )
            .map(|receipt| receipt.gas_spent)
    });
    report("dispatch, free hook instead of the IGP", "tx gas", gas);

    let grows = [
        ("Mailbox", MAILBOX_ID),
        ("MerkleTreeHook", MERKLE_TREE_HOOK_ID),
        ("IGP", IGP_ID),
    ];
    measure_batches(
        &mut session,
        "dispatch, deployed hooks",
        32_768,
        128,
        &grows,
        |i| dispatch_action(i, NO_HOOK),
    );
    measure_batches(
        &mut session,
        "dispatch, free hook instead of the IGP",
        32_768,
        128,
        &grows[..2],
        |i| dispatch_action(i, TEST_MOCK_ID),
    );

    // The Mailbox and the MerkleTreeHook alone: the hook is the required
    // hook, and the default hook charges nothing.
    let mut session = session_with_free_hooks();
    measure_batches(
        &mut session,
        "dispatch, MerkleTreeHook only",
        32_768,
        128,
        &grows[..2],
        |i| dispatch_action(i, NO_HOOK),
    );
}

/// An inbound transfer for the native route.
fn inbound_transfer(nonce: u32, recipient: H256, amount: u64) -> Vec<u8> {
    message::encode(
        VERSION,
        nonce,
        REMOTE_DOMAIN,
        REMOTE_ROUTER,
        LOCAL_DOMAIN,
        WARP_NATIVE_ID.to_bytes(),
        &token_message::encode(recipient, amount),
    )
}

fn checkpoint_signers(count: u8) -> Vec<(EthAddress, SecpSecretKey)> {
    let secp = Secp256k1::new();
    let mut signers: Vec<_> = (1u8..=count)
        .map(|index| {
            let mut bytes = [0u8; 32];
            bytes[31] = index;
            let secret = SecpSecretKey::from_byte_array(bytes).unwrap();
            let public =
                secp256k1::PublicKey::from_secret_key(&secp, &secret).serialize_uncompressed();
            let hash = message::keccak256(&public[1..]);
            (EthAddress(hash[12..].try_into().unwrap()), secret)
        })
        .collect();
    signers.sort_by_key(|(address, _)| address.0);
    signers
}

fn signed_metadata(encoded: &[u8], signers: &[(EthAddress, SecpSecretKey)]) -> Vec<u8> {
    let hook = [0x21; 32];
    let root = [0x42; 32];
    let index = 7u32;
    let digest = checkpoint::checkpoint_digest(
        message::origin(encoded),
        &hook,
        &root,
        index,
        &message::id(encoded),
    );
    let mut metadata = Vec::from(hook);
    metadata.extend_from_slice(&root);
    metadata.extend_from_slice(&index.to_be_bytes());
    let secp = Secp256k1::new();
    for (_, signer) in signers {
        let signature = secp.sign_ecdsa_recoverable(SecpMessage::from_digest(digest), signer);
        let (recovery, compact) = signature.serialize_compact();
        metadata.extend_from_slice(&compact);
        metadata.push(u8::try_from(i32::from(recovery)).unwrap() + 27);
    }
    metadata
}

#[test]
#[ignore = "measurement for docs/storage.md; slow"]
fn process_costs() {
    // A relayed native transfer to a registered account, checked by a
    // multisig ISM with 1 and with 6 signatures.
    for threshold in [1u8, 6] {
        let mut session = session_with_free_hooks();
        let signers = checkpoint_signers(threshold);
        let validators: Vec<EthAddress> = signers.iter().map(|(address, _)| *address).collect();
        deploy(
            &mut session,
            "ism_multisig",
            ISM_MULTISIG_ID,
            Some(&(*OWNER_ID, validators, threshold)),
        );
        session
            .call_public::<_, ()>(
                &OWNER_SK,
                MAILBOX_ID,
                "set_default_ism",
                &(ISM_MULTISIG_ID,),
            )
            .expect("setting the default ISM should succeed");
        deploy(
            &mut session,
            "warp_native",
            WARP_NATIVE_ID,
            Some(&(MAILBOX_ID, *OWNER_ID, vec![(REMOTE_DOMAIN, REMOTE_ROUTER)])),
        );
        session
            .call_public_with_deposit::<_, MessageId>(
                &OWNER_SK,
                WARP_NATIVE_ID,
                "transfer_remote",
                &(REMOTE_DOMAIN, [0xDD; 32], dusk(1.0)),
                dusk(1.0),
            )
            .expect("an outbound transfer gives the route custody");
        // The route registers the owner when it claims a first transfer.
        let first = inbound_transfer(1_000, *OWNER_ID, 1_000);
        let metadata = signed_metadata(&first, &signers);
        session
            .call_public::<_, ()>(&RELAYER_SK, MAILBOX_ID, "process", &(metadata, first))
            .expect("a first transfer waits in escrow");
        session
            .call_public::<_, ()>(&OWNER_SK, WARP_NATIVE_ID, "claim_pending", &())
            .expect("claim_pending should succeed");
        let gas = average_tx_gas(16, |nonce| {
            let inbound = inbound_transfer(nonce, *OWNER_ID, 1_000);
            let metadata = signed_metadata(&inbound, &signers);
            session
                .call_public::<_, ()>(&RELAYER_SK, MAILBOX_ID, "process", &(metadata, inbound))
                .map(|receipt| receipt.gas_spent)
        });
        report(
            &format!("process, native payout, {threshold} signature(s)"),
            "tx gas",
            gas,
        );
    }

    // The cheapest delivery: a recipient that names itself as its ISM.
    let mut session = session_with_free_hooks();
    session
        .direct_call::<_, ()>(ATTACKER_ID, "set_returns", &(0u64, ATTACKER_ID))
        .expect("set_returns should succeed");
    measure_top_level(
        &mut session,
        "process, self-verifying recipient",
        65_536,
        &[("Mailbox", MAILBOX_ID)],
        |nonce| {
            let inbound = message::encode(
                VERSION,
                nonce,
                REMOTE_DOMAIN,
                [0xAB; 32],
                LOCAL_DOMAIN,
                ATTACKER_ID.to_bytes(),
                b"",
            );
            action(MAILBOX_ID, "process", &(Vec::<u8>::new(), inbound))
        },
    );
}

/// A validator key and its announcement of `location`.
fn announcement(index: u32, location: &str) -> (EthAddress, Vec<u8>) {
    let secp = Secp256k1::new();
    let mut secret = [0u8; 32];
    secret[24..].copy_from_slice(&(u64::from(index) + 1).to_be_bytes());
    let secret = SecpSecretKey::from_byte_array(secret).unwrap();
    let public = secp256k1::PublicKey::from_secret_key(&secp, &secret).serialize_uncompressed();
    let hash = message::keccak256(&public[1..]);
    let validator = EthAddress(hash[12..].try_into().unwrap());
    let digest = checkpoint::announcement_digest(LOCAL_DOMAIN, &MAILBOX_ID.to_bytes(), location);
    let signature = secp.sign_ecdsa_recoverable(SecpMessage::from_digest(digest), &secret);
    let (recovery, compact) = signature.serialize_compact();
    let mut encoded = compact.to_vec();
    encoded.push(u8::try_from(i32::from(recovery)).unwrap() + 27);
    (validator, encoded)
}

#[test]
#[ignore = "measurement for docs/storage.md; slow"]
fn announce_costs() {
    let mut session = TestSession::instantiate(vec![
        (&*OWNER_PK, dusk(1_000_000.0)),
        (&*RELAYER_PK, dusk(1_000.0)),
    ]);
    deploy(
        &mut session,
        "validator_announce",
        VALIDATOR_ANNOUNCE_ID,
        Some(&(LOCAL_DOMAIN, MAILBOX_ID)),
    );

    // A validator's announcement of a typical S3 location.
    let location = |i: u32| format!("s3://hyperlane-validator-{i:08}/eu-west-1");
    let gas = average_tx_gas(16, |i| {
        let location = location(1_000_000 + i);
        let (validator, signature) = announcement(1_000_000 + i, &location);
        session
            .call_public::<_, bool>(
                &OWNER_SK,
                VALIDATOR_ANNOUNCE_ID,
                "announce",
                &(validator, location, signature),
            )
            .map(|receipt| receipt.gas_spent)
    });
    report("announce, new validator, 46-byte location", "tx gas", gas);

    // Fresh keys with the longest location accepted, 1 KiB.
    measure_top_level(
        &mut session,
        "announce, new validator, 1 KiB location",
        32_768,
        &[("ValidatorAnnounce", VALIDATOR_ANNOUNCE_ID)],
        |i| {
            let mut location = format!("s3://{i:08}/");
            location.extend(std::iter::repeat_n('x', 1024 - location.len()));
            let (validator, signature) = announcement(i, &location);
            action(
                VALIDATOR_ANNOUNCE_ID,
                "announce",
                &(validator, location, signature),
            )
        },
    );
}

fn fresh_moonlight(index: u32, seed: u8) -> Account {
    let mut key = [seed; 193];
    key[..4].copy_from_slice(&index.to_le_bytes());
    Account::Moonlight(key)
}

#[test]
#[ignore = "measurement for docs/storage.md; slow"]
fn drc20_costs() {
    let mut session = session_with_free_hooks();
    deploy(
        &mut session,
        "warp_drc20",
        WARP_DRC20_ID,
        Some(&(
            MAILBOX_ID,
            *OWNER_ID,
            String::from("Wrapped"),
            String::from("W"),
            9u8,
            vec![(REMOTE_DOMAIN, REMOTE_ROUTER)],
        )),
    );
    // Mint to the owner, which registers by claiming it, and to the
    // attacker contract, through escrow.
    for (nonce, recipient) in [(0u32, *OWNER_ID), (1, ATTACKER_ID.to_bytes())] {
        let inbound = message::encode(
            VERSION,
            nonce,
            REMOTE_DOMAIN,
            REMOTE_ROUTER,
            LOCAL_DOMAIN,
            WARP_DRC20_ID.to_bytes(),
            &token_message::encode(recipient, 1_000_000_000),
        );
        session
            .direct_call::<_, ()>(MAILBOX_ID, "process", &(Vec::<u8>::new(), inbound))
            .expect("the inbound transfer should be delivered");
    }
    session
        .call_public::<_, ()>(&OWNER_SK, WARP_DRC20_ID, "claim_pending", &())
        .expect("claim_pending should succeed");
    run(
        &mut session,
        vec![action(WARP_DRC20_ID, "claim_pending_contract", &())],
    );

    let gas = average_tx_gas(16, |i| {
        session
            .call_public::<_, ()>(
                &OWNER_SK,
                WARP_DRC20_ID,
                "transfer",
                &TransferCall {
                    to: fresh_moonlight(i, 0x61),
                    amount: 1,
                },
            )
            .map(|receipt| receipt.gas_spent)
    });
    report("DRC20 transfer to a new holder", "tx gas", gas);
    let gas = average_tx_gas(16, |i| {
        session
            .call_public::<_, ()>(
                &OWNER_SK,
                WARP_DRC20_ID,
                "approve",
                &ApproveCall {
                    spender: fresh_moonlight(i, 0x62),
                    amount: 1,
                },
            )
            .map(|receipt| receipt.gas_spent)
    });
    report("DRC20 approve a new spender", "tx gas", gas);

    let grows = [("WarpDrc20", WARP_DRC20_ID)];
    measure_batches(
        &mut session,
        "DRC20 transfer to a new holder",
        32_768,
        128,
        &grows,
        |i| {
            action(
                WARP_DRC20_ID,
                "transfer",
                &TransferCall {
                    to: fresh_moonlight(i, 0x63),
                    amount: 1,
                },
            )
        },
    );
    // An owner holds at most 64 allowances, so 512 contracts approve 64
    // spenders each, in a direct call apiece.
    let owners = 512u32;
    let before = session.memory_len(WARP_DRC20_ID);
    for owner in 0..owners {
        let mut id = [0x80; 32];
        id[..4].copy_from_slice(&owner.to_le_bytes());
        let attacker = ContractId::from_bytes(id);
        deploy_attacker(&mut session, attacker);
        let actions: Vec<Action> = (0..64)
            .map(|index| {
                action(
                    WARP_DRC20_ID,
                    "approve",
                    &ApproveCall {
                        spender: fresh_moonlight(owner * 64 + index, 0x64),
                        amount: 1,
                    },
                )
            })
            .collect();
        let args = rkyv::to_bytes::<_, 4096>(&(actions, true))
            .expect("the batch should serialize")
            .to_vec();
        session
            .direct_call_raw(attacker, "run", args)
            .unwrap_or_else(|e| panic!("owner {owner}'s approvals failed: {e}"));
    }
    report(
        "DRC20 approve a new spender",
        "WarpDrc20 bytes",
        format!(
            "{:.1}",
            (session.memory_len(WARP_DRC20_ID) - before) as f64 / f64::from(owners * 64)
        ),
    );
}

#[test]
#[ignore = "measurement for docs/storage.md; slow"]
fn withdrawal_costs() {
    let mut session = session_with_free_hooks();
    fund_dispatch(&mut session, *OWNER_ID, dusk(1.0));
    let gas = average_tx_gas(4, |_| {
        session
            .call_public::<_, ()>(
                &OWNER_SK,
                MAILBOX_ID,
                "withdraw_dispatch_credit",
                &(*RELAYER_PK, 1_000u64),
            )
            .map(|receipt| receipt.gas_spent)
    });
    report("withdraw dispatch credit", "tx gas", gas);
    let memory = query::<_, u64>(&mut session, MAILBOX_ID, "memory_bytes", &());
    assert!(memory % WASM_PAGE == 0);
}
