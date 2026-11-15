// SPDX-License-Identifier: MIT OR Apache-2.0
//
// The cost profile of the production contracts, checked against budgets
// (docs/costs.md).
//
//   make all canonical-drc20-fixture
//   cargo test -p hyperlane-dusk-integration-tests --test cost_profile -- --nocapture
//
// The default tests run every public entry point of the production contracts
// and the main user flows once, in the configuration deploy/testnet.sh
// deploys, and fail if a measured value exceeds its budget in `BUDGETS` or a
// platform limit. The ignored test measures how much memory each
// permissionless growth path takes per entry, over many entries; it takes
// about 6 minutes in release mode:
//
//   cargo test --release -p hyperlane-dusk-integration-tests --test cost_profile \
//       growth -- --ignored --nocapture
//
// Every measured value is printed as `cost|<row>|<metric>|<value>`.
//
// Metrics of a transaction row:
//
// - `tx gas`: what the Moonlight transaction spends, the transfer contract's
//   signature check included: about 1.8M plus 80 gas per transaction byte.
// - `call gas`: the part the called contract and the calls it makes spend:
//   `tx gas` minus the transfer contract's own work, mostly the signature
//   check.
// - `calls`: the inter-contract calls the called contract makes, directly
//   and through other contracts, from the receipt's call tree. Calls that
//   fail and are caught don't show in the tree.
// - `depth`: the deepest stack of contract frames, the transfer contract's
//   included. The VM allows 48.
// - `events`, `event bytes`: the events the transaction leaves, the
//   transfer contract's included, and their topic and data bytes.
// - `tx bytes`: the transaction's size as the network carries it
//   (`Transaction::to_var_bytes`).
//
// Budgets are the measured value plus a margin: 5% for gas, 10% for bytes,
// none for calls, depth and events, which only change when the code does.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use dusk_bytes::Serializable;
use dusk_core::abi::ContractId;
use dusk_core::dusk;
use dusk_core::signatures::bls::{PublicKey as AccountPublicKey, SecretKey as AccountSecretKey};
use dusk_core::transfer::data::ContractCall;
use dusk_core::transfer::Transaction;
use dusk_core::LUX;
use dusk_vm::{CallReceipt, CallTree, ContractData};
use rand::rngs::StdRng;
use rand::SeedableRng;
use rkyv::ser::serializers::AllocSerializer;
use secp256k1::{Message as SecpMessage, Secp256k1, SecretKey as SecpSecretKey};

use hyperlane_dusk_types::drc20::{Account, ApproveCall, TransferCall, TransferFromCall};
use hyperlane_dusk_types::{
    checkpoint, message, token_message, DomainGasConfig, EthAddress, H256, VERSION,
};

mod test_session;
use test_session::TestSession;

/// Archives a call's arguments, however large.
fn archive<A>(args: &A) -> Vec<u8>
where
    A: rkyv::Serialize<AllocSerializer<4096>>,
{
    rkyv::to_bytes::<_, 4096>(args)
        .expect("the arguments should archive")
        .to_vec()
}

// =============================================================================
// Deployment
// =============================================================================

const MAILBOX_ID: ContractId = ContractId::from_bytes([10; 32]);
const MERKLE_TREE_HOOK_ID: ContractId = ContractId::from_bytes([11; 32]);
const TEST_MOCK_ID: ContractId = ContractId::from_bytes([12; 32]);
const PROTOCOL_FEE_ID: ContractId = ContractId::from_bytes([14; 32]);
const IGP_ID: ContractId = ContractId::from_bytes([15; 32]);
const ISM_ID: ContractId = ContractId::from_bytes([16; 32]);
const AGGREGATION_HOOK_ID: ContractId = ContractId::from_bytes([19; 32]);
const VALIDATOR_ANNOUNCE_ID: ContractId = ContractId::from_bytes([20; 32]);
const SYNTHETIC_ID: ContractId = ContractId::from_bytes([21; 32]);
const NATIVE_ID: ContractId = ContractId::from_bytes([22; 32]);
const COLLATERAL_ID: ContractId = ContractId::from_bytes([23; 32]);
const TOKEN_ID: ContractId = ContractId::from_bytes([24; 32]);
const BIG_ISM_ID: ContractId = ContractId::from_bytes([25; 32]);
const ATTACKER_ID: ContractId = ContractId::from_bytes([40; 32]);

/// The contracts whose memory is watched, by name.
const CONTRACTS: [(&str, ContractId); 12] = [
    ("Mailbox", MAILBOX_ID),
    ("MerkleTreeHook", MERKLE_TREE_HOOK_ID),
    ("ProtocolFee", PROTOCOL_FEE_ID),
    ("AggregationHook", AGGREGATION_HOOK_ID),
    ("IGP", IGP_ID),
    ("ISM", ISM_ID),
    ("ValidatorAnnounce", VALIDATOR_ANNOUNCE_ID),
    ("WarpNative", NATIVE_ID),
    ("WarpDrc20", SYNTHETIC_ID),
    ("WarpDrc20Collateral", COLLATERAL_ID),
    ("DRC20 token", TOKEN_ID),
    ("attacker", ATTACKER_ID),
];

const DEPLOYER: [u8; 64] = [0u8; 64];
/// deploy/testnet.env.example's DUSK_DOMAIN and Sepolia's domain.
const LOCAL_DOMAIN: u32 = 1_146_442_572;
const REMOTE_DOMAIN: u32 = 11_155_111;
const NATIVE_ROUTER: H256 = [0xA1; 32];
const SYNTHETIC_ROUTER: H256 = [0xA2; 32];
const COLLATERAL_ROUTER: H256 = [0xA3; 32];
const NO_HOOK: ContractId = ContractId::from_bytes([0; 32]);

/// What `dusk-tx deploy-hyperlane` sets up: a 0.001 DUSK protocol fee, and
/// IGP pricing for the EVM chain of 200,000 gas overhead at 1 gwei, as
/// deploy/testnet.env.example has it, but at an exchange rate of 1 (1e-10
/// LUX per wei) instead of its 375,000, so a transfer's fee stays at 25,000
/// LUX. A call's gas doesn't depend on the price; tests/tests/fee_pricing.rs
/// checks the configured one.
const PROTOCOL_FEE: u64 = 1_000_000;
const MAX_PROTOCOL_FEE: u64 = 100_000_000;
const EVM_PRICING: DomainGasConfig = DomainGasConfig {
    gas_overhead: 200_000,
    token_exchange_rate: 1,
    gas_price: 1_000_000_000,
};

/// The relayer's gas limit for `process` (DUSK_RELAYER_GAS_LIMIT).
const RELAYER_GAS_LIMIT: u64 = 5_000_000;
/// The block gas limit, which is also what a query may spend.
const BLOCK_GAS_LIMIT: u64 = 3_000_000_000;
/// The VM's argument buffer: the most a call's arguments, a query's reply
/// or a serialized transaction can take.
const ARGBUF_LEN: usize = 64 * 1024;
/// The deepest call stack the VM allows.
const MAX_CALL_DEPTH: u64 = 48;
/// The test session's chain ID.
const CHAIN_ID: u8 = 1;

/// The validator set of the EVM-origin ISM; thresholds up to 6 are measured.
const VALIDATORS: u8 = 6;

static OWNER_SK: LazyLock<AccountSecretKey> =
    LazyLock::new(|| AccountSecretKey::random(&mut StdRng::seed_from_u64(0x48595045)));
static OWNER_PK: LazyLock<AccountPublicKey> = LazyLock::new(|| AccountPublicKey::from(&*OWNER_SK));
static OWNER_ID: LazyLock<H256> = LazyLock::new(|| message::keccak256(&OWNER_PK.to_bytes()));
static RELAYER_SK: LazyLock<AccountSecretKey> =
    LazyLock::new(|| AccountSecretKey::random(&mut StdRng::seed_from_u64(0xDE1A7E00)));
static RELAYER_PK: LazyLock<AccountPublicKey> =
    LazyLock::new(|| AccountPublicKey::from(&*RELAYER_SK));
static USER_SK: LazyLock<AccountSecretKey> =
    LazyLock::new(|| AccountSecretKey::random(&mut StdRng::seed_from_u64(0x05E2)));
static USER_PK: LazyLock<AccountPublicKey> = LazyLock::new(|| AccountPublicKey::from(&*USER_SK));
static USER_ID: LazyLock<H256> = LazyLock::new(|| message::keccak256(&USER_PK.to_bytes()));

/// A key with nothing but escrow on a route, and its hash.
fn fresh_key(seed: u64) -> (AccountSecretKey, AccountPublicKey, H256) {
    let sk = AccountSecretKey::random(&mut StdRng::seed_from_u64(0xF2E5_0000 + seed));
    let pk = AccountPublicKey::from(&sk);
    let id = message::keccak256(&pk.to_bytes());
    (sk, pk, id)
}

/// The contract WASMs: this checkout's build, or the ones in
/// `HYPERLANE_WASM_DIR`, for example another commit's, to profile those.
fn wasm(name: &str) -> Vec<u8> {
    let dir = std::env::var_os("HYPERLANE_WASM_DIR").map_or_else(
        || {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../target/contract/wasm32-unknown-unknown/release")
        },
        std::path::PathBuf::from,
    );
    let path = dir.join(format!("{name}.wasm"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e} (run make all)", path.display()))
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

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone)]
#[archive_attr(derive(bytecheck::CheckBytes))]
struct CanonicalDrc20InitBalance {
    account: Account,
    amount: u64,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone)]
#[archive_attr(derive(bytecheck::CheckBytes))]
struct CanonicalDrc20TokenInit {
    name: String,
    symbol: String,
    decimals: u8,
    initial_balances: Vec<CanonicalDrc20InitBalance>,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone)]
#[archive_attr(derive(bytecheck::CheckBytes))]
struct CanonicalDrc20ExampleInit {
    admin: Account,
    token: CanonicalDrc20TokenInit,
    cap: u64,
}

/// Validator keys, sorted by address as the ISM requires.
fn validator_keys(count: u8) -> Vec<(EthAddress, SecpSecretKey)> {
    let secp = Secp256k1::new();
    let mut keys: Vec<_> = (1u8..=count)
        .map(|index| {
            let mut bytes = [0u8; 32];
            bytes[30] = 0x77;
            bytes[31] = index;
            let secret = SecpSecretKey::from_byte_array(bytes).unwrap();
            let public =
                secp256k1::PublicKey::from_secret_key(&secp, &secret).serialize_uncompressed();
            let hash = message::keccak256(&public[1..]);
            (EthAddress(hash[12..].try_into().unwrap()), secret)
        })
        .collect();
    keys.sort_by_key(|(address, _)| address.0);
    keys
}

/// The contracts as deploy/testnet.sh deploys them, plus the other two
/// routes, a canonical DRC20 for the collateral route to lock, and the
/// red-team contract to batch calls and to stand in for a recipient
/// contract.
struct Net {
    s: TestSession,
    validators: Vec<(EthAddress, SecpSecretKey)>,
    threshold: u8,
    inbound_nonce: u32,
}

impl Net {
    /// The contracts deployed, with the user registered on every route and
    /// the collateral route approved to take its tokens.
    fn deployed() -> Self {
        let mut s = TestSession::instantiate(vec![
            (&*OWNER_PK, dusk(1_000_000.0)),
            (&*RELAYER_PK, dusk(1_000.0)),
            (&*USER_PK, dusk(1_000.0)),
        ]);
        let validators = validator_keys(VALIDATORS);
        let addresses: Vec<EthAddress> = validators.iter().map(|(a, _)| *a).collect();
        deploy::<()>(&mut s, "hyperlane_dusk_test_mock", TEST_MOCK_ID, None);
        deploy(
            &mut s,
            "hyperlane_dusk_merkle_tree_hook",
            MERKLE_TREE_HOOK_ID,
            Some(&(AGGREGATION_HOOK_ID,)),
        );
        deploy(
            &mut s,
            "hyperlane_dusk_ism_multisig",
            ISM_ID,
            Some(&(*OWNER_ID, addresses, 1u8)),
        );
        deploy(
            &mut s,
            "hyperlane_dusk_protocol_fee",
            PROTOCOL_FEE_ID,
            Some(&(
                PROTOCOL_FEE,
                MAX_PROTOCOL_FEE,
                AGGREGATION_HOOK_ID,
                *OWNER_ID,
                *OWNER_ID,
            )),
        );
        deploy(
            &mut s,
            "hyperlane_dusk_aggregation_hook",
            AGGREGATION_HOOK_ID,
            Some(&(MAILBOX_ID, vec![MERKLE_TREE_HOOK_ID, PROTOCOL_FEE_ID])),
        );
        deploy(
            &mut s,
            "hyperlane_dusk_mailbox",
            MAILBOX_ID,
            Some(&(LOCAL_DOMAIN, *OWNER_ID, ISM_ID, IGP_ID, AGGREGATION_HOOK_ID)),
        );
        deploy(
            &mut s,
            "hyperlane_dusk_validator_announce",
            VALIDATOR_ANNOUNCE_ID,
            Some(&(LOCAL_DOMAIN, MAILBOX_ID)),
        );
        deploy(
            &mut s,
            "hyperlane_dusk_igp",
            IGP_ID,
            Some(&(
                MAILBOX_ID,
                *OWNER_ID,
                *OWNER_ID,
                vec![(REMOTE_DOMAIN, EVM_PRICING)],
            )),
        );
        deploy(
            &mut s,
            "hyperlane_dusk_warp_native",
            NATIVE_ID,
            Some(&(MAILBOX_ID, *OWNER_ID, vec![(REMOTE_DOMAIN, NATIVE_ROUTER)])),
        );
        deploy(
            &mut s,
            "hyperlane_dusk_warp_drc20",
            SYNTHETIC_ID,
            Some(&(
                MAILBOX_ID,
                *OWNER_ID,
                String::from("Wrapped Ether"),
                String::from("WETH"),
                9u8,
                vec![(REMOTE_DOMAIN, SYNTHETIC_ROUTER)],
            )),
        );
        let user = Account::moonlight(&USER_PK);
        deploy(
            &mut s,
            "canonical_drc20_roles_pausable",
            TOKEN_ID,
            Some(&CanonicalDrc20ExampleInit {
                admin: user,
                token: CanonicalDrc20TokenInit {
                    name: String::from("Canonical Dusk Token"),
                    symbol: String::from("CDRC20"),
                    decimals: 9,
                    initial_balances: vec![CanonicalDrc20InitBalance {
                        account: user,
                        amount: 1_000_000_000_000,
                    }],
                },
                cap: 0,
            }),
        );
        deploy(
            &mut s,
            "hyperlane_dusk_warp_drc20_collateral",
            COLLATERAL_ID,
            Some(&(
                TOKEN_ID,
                MAILBOX_ID,
                *OWNER_ID,
                vec![(REMOTE_DOMAIN, COLLATERAL_ROUTER)],
            )),
        );
        deploy::<()>(&mut s, "hyperlane_dusk_redteam_attacker", ATTACKER_ID, None);
        let mut net = Self {
            s,
            validators,
            threshold: 1,
            inbound_nonce: 0,
        };
        for route in [NATIVE_ID, SYNTHETIC_ID, COLLATERAL_ID] {
            net.call(&USER_SK, route, "register_account", &(), 0);
        }
        net.call(
            &USER_SK,
            TOKEN_ID,
            "approve",
            &ApproveCall {
                spender: Account::Contract(COLLATERAL_ID),
                amount: u64::MAX,
            },
            0,
        );
        net
    }

    /// As [`Self::deployed`], with custody on each route for inbound
    /// transfers, and synthetic tokens for the user: an outbound transfer
    /// locks DUSK and collateral, an inbound one mints.
    fn new() -> Self {
        let mut net = Self::deployed();
        let fee = net.quote(NATIVE_ID, 1);
        net.call(
            &OWNER_SK,
            NATIVE_ID,
            "transfer_remote",
            &(REMOTE_DOMAIN, [0xEE; 32], dusk(100.0)),
            dusk(100.0) + fee,
        );
        let fee = net.quote(COLLATERAL_ID, 1);
        net.call(
            &USER_SK,
            COLLATERAL_ID,
            "transfer_remote",
            &(REMOTE_DOMAIN, [0xEE; 32], 100_000_000_000u64),
            fee,
        );
        net.deliver(SYNTHETIC_ID, *USER_ID, 100_000_000_000);
        net
    }

    fn set_threshold(&mut self, threshold: u8) {
        let addresses: Vec<EthAddress> = self.validators.iter().map(|(a, _)| *a).collect();
        self.call(
            &OWNER_SK,
            ISM_ID,
            "set_validators_and_threshold",
            &(addresses, threshold),
            0,
        );
        self.threshold = threshold;
    }

    /// A Moonlight transaction that must succeed.
    fn call<A>(
        &mut self,
        sk: &AccountSecretKey,
        contract: ContractId,
        fn_name: &str,
        args: &A,
        deposit: u64,
    ) -> CallReceipt<Vec<u8>>
    where
        A: rkyv::Serialize<AllocSerializer<4096>>,
    {
        self.call_raw(sk, contract, fn_name, archive(args), deposit)
    }

    fn call_raw(
        &mut self,
        sk: &AccountSecretKey,
        contract: ContractId,
        fn_name: &str,
        args: Vec<u8>,
        deposit: u64,
    ) -> CallReceipt<Vec<u8>> {
        self.s
            .call_public_raw_with_deposit(sk, contract, fn_name, args, deposit)
            .unwrap_or_else(|e| panic!("{fn_name} on {contract:?} failed: {e}"))
    }

    /// What `transfer_remote` of `amount` on `route` charges on top.
    fn quote(&mut self, route: ContractId, amount: u64) -> u64 {
        self.query(
            route,
            "quote_transfer_remote",
            &(REMOTE_DOMAIN, [0xEE; 32], amount),
        )
    }

    fn query<A, R>(&mut self, contract: ContractId, fn_name: &str, args: &A) -> R
    where
        A: rkyv::Serialize<AllocSerializer<4096>>,
        R: rkyv::Archive,
        R::Archived: rkyv::Deserialize<R, rkyv::Infallible>
            + for<'b> bytecheck::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
    {
        let reply = self
            .s
            .query_raw(contract, fn_name, archive(args), BLOCK_GAS_LIMIT)
            .unwrap_or_else(|e| panic!("{fn_name} should succeed: {e}"))
            .data;
        test_session::rkyv_deserialize(&reply)
    }

    /// An inbound transfer from the remote router of `route`.
    fn inbound(&mut self, route: ContractId, recipient: H256, amount: u64) -> Vec<u8> {
        let router = match route {
            NATIVE_ID => NATIVE_ROUTER,
            SYNTHETIC_ID => SYNTHETIC_ROUTER,
            _ => COLLATERAL_ROUTER,
        };
        self.message(router, route, &token_message::encode(recipient, amount))
    }

    fn message(&mut self, sender: H256, recipient: ContractId, body: &[u8]) -> Vec<u8> {
        self.inbound_nonce += 1;
        message::encode(
            VERSION,
            self.inbound_nonce,
            REMOTE_DOMAIN,
            sender,
            LOCAL_DOMAIN,
            recipient.to_bytes(),
            body,
        )
    }

    /// The checkpoint metadata the relayer sends: signatures of the last
    /// `threshold` validators, the worst case for the ISM's scan.
    fn metadata(&self, encoded: &[u8]) -> Vec<u8> {
        let signers = &self.validators[self.validators.len() - usize::from(self.threshold)..];
        signed_metadata(encoded, signers)
    }

    /// Delivers an inbound transfer as the relayer does.
    fn deliver(&mut self, route: ContractId, recipient: H256, amount: u64) {
        let encoded = self.inbound(route, recipient, amount);
        let metadata = self.metadata(&encoded);
        self.call(&RELAYER_SK, MAILBOX_ID, "process", &(metadata, encoded), 0);
    }

    fn memory(&mut self) -> Vec<u64> {
        CONTRACTS
            .iter()
            .map(|(_, id)| self.s.memory_len(*id))
            .collect()
    }
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

/// A validator key and its signed announcement of `location`.
fn announcement(seed: u64, location: &str) -> (EthAddress, Vec<u8>) {
    let secp = Secp256k1::new();
    let mut secret = [0u8; 32];
    secret[16..24].copy_from_slice(&0xA11CE_u64.to_be_bytes());
    secret[24..].copy_from_slice(&(seed + 1).to_be_bytes());
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

// =============================================================================
// Measuring
// =============================================================================

/// What one transaction cost.
#[derive(Clone, Debug, Default)]
struct Measured {
    tx_gas: u64,
    call_gas: u64,
    calls: u64,
    depth: u64,
    events: u64,
    event_bytes: u64,
    tx_bytes: u64,
    /// Memory each contract gained, in bytes (whole pages).
    growth: Vec<(&'static str, u64)>,
}

/// The calls a receipt's call tree records below the called contract, the
/// deepest stack of frames, and the gas spent below the transfer contract.
fn call_tree_shape(tree: &CallTree) -> (u64, u64, u64) {
    // `{:#?}` draws the tree one frame per line: the root bare, each child
    // indented 5 characters per level below the root's children.
    let drawing = format!("{tree:#?}");
    let mut frames = 0u64;
    let mut depth = 0u64;
    for line in drawing.lines().filter(|line| !line.trim().is_empty()) {
        frames += 1;
        let level = match line.find(['├', '└']) {
            None => 1,
            Some(at) => 2 + line[..at].chars().count() as u64 / 5,
        };
        depth = depth.max(level);
    }
    // The tree holds each frame's own gas, children excluded, and yields the
    // root last.
    let spent: Vec<u64> = tree.iter().map(|frame| frame.spent).collect();
    let root = *spent.last().expect("a transaction has a root frame");
    let below_root: u64 = spent.iter().sum::<u64>() - root;
    (frames.saturating_sub(2), depth, below_root)
}

/// The size of the transaction the network carries for this call.
fn tx_bytes(
    sk: &AccountSecretKey,
    contract: ContractId,
    fn_name: &str,
    args: &[u8],
    deposit: u64,
) -> (u64, u64) {
    let tx = Transaction::moonlight(
        sk,
        None,
        0,
        deposit,
        RELAYER_GAS_LIMIT,
        LUX,
        1,
        CHAIN_ID,
        Some(ContractCall {
            contract,
            fn_name: String::from(fn_name),
            fn_args: args.to_vec(),
        }),
    )
    .expect("the transaction should build");
    let archived = rkyv::to_bytes::<_, 4096>(&tx)
        .expect("the transaction archives")
        .len();
    (tx.to_var_bytes().len() as u64, archived as u64)
}

fn measure_raw(
    net: &mut Net,
    sk: &AccountSecretKey,
    contract: ContractId,
    fn_name: &str,
    args: Vec<u8>,
    deposit: u64,
) -> Measured {
    let (network_bytes, archived_bytes) = tx_bytes(sk, contract, fn_name, &args, deposit);
    assert!(
        archived_bytes <= ARGBUF_LEN as u64,
        "{fn_name}: a {archived_bytes}-byte transaction doesn't fit the transfer contract's argument"
    );
    let before = net.memory();
    let receipt = net.call_raw(sk, contract, fn_name, args, deposit);
    let after = net.memory();
    let (calls, depth, call_gas) = call_tree_shape(&receipt.call_tree);
    let events: Vec<_> = receipt.events.iter().filter(|e| !e.reverted).collect();
    Measured {
        tx_gas: receipt.gas_spent,
        call_gas,
        calls,
        depth,
        events: events.len() as u64,
        event_bytes: events
            .iter()
            .map(|e| (e.topic.len() + e.data.len()) as u64)
            .sum(),
        tx_bytes: network_bytes,
        growth: CONTRACTS
            .iter()
            .zip(before.iter().zip(&after))
            .filter(|(_, (b, a))| a > b)
            .map(|((name, _), (b, a))| (*name, a - b))
            .collect(),
    }
}

fn measure<A>(
    net: &mut Net,
    sk: &AccountSecretKey,
    contract: ContractId,
    fn_name: &str,
    args: &A,
    deposit: u64,
) -> Measured
where
    A: rkyv::Serialize<AllocSerializer<4096>>,
{
    measure_raw(net, sk, contract, fn_name, archive(args), deposit)
}

/// The relayer's `process` of `encoded`, signed at the net's threshold.
fn measure_process(net: &mut Net, encoded: Vec<u8>) -> Measured {
    let metadata = net.metadata(&encoded);
    measure_raw(
        net,
        &RELAYER_SK,
        MAILBOX_ID,
        "process",
        archive(&(metadata, encoded)),
        0,
    )
}

/// A query's gas and reply size.
#[derive(Clone, Copy, Debug)]
struct Queried {
    gas: u64,
    reply_bytes: u64,
}

fn measure_query<A>(net: &mut Net, contract: ContractId, fn_name: &str, args: &A) -> Queried
where
    A: rkyv::Serialize<AllocSerializer<4096>>,
{
    let receipt = net
        .s
        .query_raw(contract, fn_name, archive(args), BLOCK_GAS_LIMIT)
        .unwrap_or_else(|e| panic!("query {fn_name} failed: {e}"));
    Queried {
        gas: receipt.gas_spent,
        reply_bytes: receipt.data.len() as u64,
    }
}

// =============================================================================
// Budgets
// =============================================================================

/// One measured row of the profile.
enum Row {
    Tx(Measured),
    Query(Queried),
}

/// Collects rows, prints them and checks them against `BUDGETS` and the
/// platform limits.
#[derive(Default)]
struct Profile {
    rows: Vec<(String, Row)>,
}

impl Profile {
    fn tx(&mut self, name: &str, measured: Measured) -> Measured {
        println!("cost|{name}|tx gas|{}", measured.tx_gas);
        println!("cost|{name}|call gas|{}", measured.call_gas);
        println!("cost|{name}|calls|{}", measured.calls);
        println!("cost|{name}|depth|{}", measured.depth);
        println!("cost|{name}|events|{}", measured.events);
        println!("cost|{name}|event bytes|{}", measured.event_bytes);
        println!("cost|{name}|tx bytes|{}", measured.tx_bytes);
        for (contract, bytes) in &measured.growth {
            println!("cost|{name}|{contract} memory growth|{bytes}");
        }
        self.rows
            .push((name.to_string(), Row::Tx(measured.clone())));
        measured
    }

    fn query(&mut self, name: &str, queried: Queried) -> Queried {
        println!("cost|{name}|query gas|{}", queried.gas);
        println!("cost|{name}|reply bytes|{}", queried.reply_bytes);
        self.rows.push((name.to_string(), Row::Query(queried)));
        queried
    }

    /// Fails with every budget and limit the rows exceed, and every row
    /// without a budget.
    fn check(&self) {
        let budgets: BTreeMap<&str, &[u64]> = BUDGETS.iter().map(|(n, b)| (*n, *b)).collect();
        let mut failures = Vec::new();
        for (name, row) in &self.rows {
            let Some(budget) = budgets.get(name.as_str()) else {
                failures.push(format!("{name}: no budget"));
                continue;
            };
            let measured: Vec<(&str, u64)> = match row {
                Row::Tx(m) => {
                    if m.depth > MAX_CALL_DEPTH {
                        failures.push(format!("{name}: call depth {} > {MAX_CALL_DEPTH}", m.depth));
                    }
                    vec![
                        ("tx gas", m.tx_gas),
                        ("call gas", m.call_gas),
                        ("calls", m.calls),
                        ("depth", m.depth),
                        ("events", m.events),
                        ("event bytes", m.event_bytes),
                        ("tx bytes", m.tx_bytes),
                    ]
                }
                Row::Query(q) => {
                    if q.gas > BLOCK_GAS_LIMIT {
                        failures.push(format!("{name}: query gas {} > {BLOCK_GAS_LIMIT}", q.gas));
                    }
                    if q.reply_bytes >= ARGBUF_LEN as u64 {
                        failures.push(format!("{name}: reply {} bytes >= 64 KiB", q.reply_bytes));
                    }
                    vec![("query gas", q.gas), ("reply bytes", q.reply_bytes)]
                }
            };
            if measured.len() != budget.len() {
                failures.push(format!(
                    "{name}: budget has {} values, row {}",
                    budget.len(),
                    measured.len()
                ));
                continue;
            }
            for ((metric, value), limit) in measured.iter().zip(budget.iter()) {
                if value > limit {
                    failures.push(format!("{name}: {metric} {value} > budget {limit}"));
                }
            }
        }
        assert!(failures.is_empty(), "over budget:\n{}", failures.join("\n"));
    }
}

/// Budgets: for a transaction row, tx gas, call gas, calls, depth, events,
/// event bytes and tx bytes; for a query, gas and reply bytes. Measured
/// values plus 5% for gas and 10% for bytes, rounded up; calls, depth and
/// events as measured. A change that costs more must raise its budget here
/// and its row in docs/costs.md.
#[rustfmt::skip]
const BUDGETS: &[(&str, &[u64])] = &[
    ("AggregationHook hook_type", &[87, 2]),
    ("AggregationHook hooks", &[977, 80]),
    ("AggregationHook mailbox", &[215, 36]),
    ("AggregationHook quote_dispatch", &[23_544, 9]),
    ("AggregationHook state_version", &[87, 5]),
    ("IGP beneficiary", &[215, 36]),
    ("IGP claim", &[1_960_385, 55_076, 1, 3, 2, 1_008, 275]),
    ("IGP claimable_fees", &[104, 9]),
    ("IGP domain_gas_config", &[269, 27]),
    ("IGP gas_payment_at", &[435, 71]),
    ("IGP gas_payment_count", &[88, 5]),
    ("IGP gas_payments, 256 across two chunks", &[108_315, 18_032]),
    ("IGP gas_payments, 256 from 0", &[108_210, 18_032]),
    ("IGP hook_type", &[87, 2]),
    ("IGP mailbox", &[215, 36]),
    ("IGP owner", &[870, 37]),
    ("IGP pay_for_gas", &[1_924_374, 12_033, 1, 3, 3, 1_069, 344]),
    ("IGP pay_for_gas, with a refund", &[1_940_547, 28_207, 2, 3, 4, 1_353, 344]),
    ("IGP quote_dispatch", &[1_514, 9]),
    ("IGP quote_gas_payment", &[959, 9]),
    ("IGP quote_pay_for_gas", &[950, 9]),
    ("IGP set_beneficiary", &[1_949_525, 39_304, 0, 2, 2, 775, 322]),
    ("IGP set_domain_gas_config", &[1_949_932, 39_330, 0, 2, 2, 782, 328]),
    ("IGP set_domain_gas_configs, 2 domains", &[1_955_993, 42_032, 0, 2, 3, 840, 373]),
    ("IGP state_version", &[87, 5]),
    ("IGP total_gas_payments", &[104, 9]),
    ("IGP transfer_ownership", &[1_950_004, 39_636, 0, 2, 2, 817, 325]),
    ("ISM module_type", &[87, 2]),
    ("ISM owner", &[870, 37]),
    ("ISM set_validators_and_threshold, 255 validators", &[2_629_463, 290_855, 0, 2, 2, 6_377, 5_924]),
    ("ISM set_validators_and_threshold, 6 validators", &[1_963_931, 44_764, 0, 2, 2, 899, 446]),
    ("ISM state_version", &[87, 5]),
    ("ISM threshold", &[88, 2]),
    ("ISM transfer_ownership", &[1_949_900, 39_532, 0, 2, 2, 817, 325]),
    ("ISM validators, 255 validators", &[168_342, 5_619]),
    ("ISM validators, 6 validators", &[4_694, 141]),
    ("ISM validators_and_threshold, 255 validators", &[168_402, 5_624]),
    ("ISM validators_and_threshold, 6 validators", &[4_754, 146]),
    ("ISM verify, 1 of 6 signatures", &[2_179_260, 253_194, 0, 2, 1, 723, 527]),
    ("ISM verify, 3 of 6 signatures", &[2_626_296, 689_498, 0, 2, 1, 723, 668]),
    ("ISM verify, 6 of 255 signatures", &[3_309_074, 1_355_874, 0, 2, 1, 723, 884]),
    ("ISM verify, 6 of 6 signatures", &[3_297_455, 1_344_193, 0, 2, 1, 723, 884]),
    ("Mailbox default_hook", &[215, 36]),
    ("Mailbox default_ism", &[215, 36]),
    ("Mailbox delivered", &[338, 2]),
    ("Mailbox dispatch, 64-byte body, free hook instead of the IGP", &[2_155_553, 234_981, 14, 6, 8, 1_444, 442]),
    ("Mailbox dispatch_default, 1 KiB body", &[3_159_088, 1_160_128, 16, 6, 10, 2_666, 1_462]),
    ("Mailbox dispatch_default, 64-byte body", &[2_174_094, 255_826, 16, 6, 10, 1_610, 406]),
    ("Mailbox dispatch_default, empty body", &[2_117_054, 204_148, 16, 6, 10, 1_539, 336]),
    ("Mailbox dispatch_default, largest body", &[65_470_116, 58_243_553, 16, 6, 10, 71_117, 69_913]),
    ("Mailbox dispatched_block_height", &[180, 9]),
    ("Mailbox dispatched_id", &[278, 36]),
    ("Mailbox fee_credit", &[302, 9]),
    ("Mailbox fund_dispatch, existing payer", &[1_920_076, 9_055, 1, 3, 3, 1_069, 328]),
    ("Mailbox fund_dispatch, new payer", &[1_920_239, 9_218, 1, 3, 3, 1_069, 328]),
    ("Mailbox latest_dispatched_id", &[215, 36]),
    ("Mailbox local_domain", &[88, 5]),
    ("Mailbox memory_bytes", &[107, 9]),
    ("Mailbox nonce", &[88, 5]),
    ("Mailbox owner", &[870, 37]),
    ("Mailbox process, recipient contract, 1 KiB body, 6 of 6 signatures", &[3_667_795, 1_629_069, 3, 3, 3, 852, 2_011]),
    ("Mailbox process, recipient contract, 16 KiB body, 6 of 6 signatures", &[8_392_512, 5_063_546, 3, 3, 3, 852, 18_907]),
    ("Mailbox process, recipient contract, empty body, 1 of 6 signatures", &[2_210_910, 285_397, 3, 3, 3, 852, 528]),
    ("Mailbox process, recipient contract, empty body, 3 of 6 signatures", &[2_666_260, 730_014, 3, 3, 3, 852, 669]),
    ("Mailbox process, recipient contract, empty body, 6 of 6 signatures", &[3_350_924, 1_398_214, 3, 3, 3, 852, 885]),
    ("Mailbox process, recipient contract, largest body, 6 of 6 signatures", &[22_662_250, 15_437_188, 3, 3, 3, 852, 69_921]),
    ("Mailbox process, recipient that is its own ISM", &[1_936_258, 21_586, 3, 3, 3, 852, 383]),
    ("Mailbox quote_dispatch_for_sender", &[50_964, 9]),
    ("Mailbox recipient_ism", &[1_014, 36]),
    ("Mailbox renounce_ownership", &[1_944_836, 38_181, 0, 2, 2, 779, 290]),
    ("Mailbox required_hook", &[215, 36]),
    ("Mailbox set_default_hook", &[1_949_763, 39_590, 0, 2, 2, 776, 323]),
    ("Mailbox set_default_ism", &[1_949_889, 39_606, 0, 2, 2, 775, 322]),
    ("Mailbox set_required_hook", &[1_949_861, 39_592, 0, 2, 2, 777, 324]),
    ("Mailbox state_version", &[87, 5]),
    ("Mailbox transfer_ownership", &[1_950_249, 39_882, 0, 2, 2, 817, 325]),
    ("Mailbox withdraw_dispatch_credit", &[61_631_148, 59_705_442, 1, 3, 3, 1_111, 525]),
    ("MerkleTreeHook count", &[88, 5]),
    ("MerkleTreeHook hook_type", &[87, 2]),
    ("MerkleTreeHook inserted_block_height", &[180, 9]),
    ("MerkleTreeHook latest_checkpoint", &[259_528, 40]),
    ("MerkleTreeHook mailbox", &[215, 36]),
    ("MerkleTreeHook message_id_at", &[278, 36]),
    ("MerkleTreeHook message_ids, 256 across two chunks", &[65_312, 9_020]),
    ("MerkleTreeHook message_ids, 256 from 0", &[65_207, 9_020]),
    ("MerkleTreeHook quote_dispatch", &[321, 9]),
    ("MerkleTreeHook root", &[259_502, 36]),
    ("MerkleTreeHook state_version", &[87, 5]),
    ("MerkleTreeHook tree", &[5_110, 1_131]),
    ("ProtocolFee beneficiary", &[215, 36]),
    ("ProtocolFee claim", &[1_958_939, 53_568, 1, 3, 2, 1_008, 275]),
    ("ProtocolFee claimable_fees", &[104, 9]),
    ("ProtocolFee collected_fees", &[104, 9]),
    ("ProtocolFee hook_type", &[87, 2]),
    ("ProtocolFee mailbox", &[215, 36]),
    ("ProtocolFee max_protocol_fee", &[104, 9]),
    ("ProtocolFee owner", &[870, 37]),
    ("ProtocolFee protocol_fee", &[104, 9]),
    ("ProtocolFee quote_dispatch", &[324, 9]),
    ("ProtocolFee set_beneficiary", &[1_948_083, 37_862, 0, 2, 2, 775, 322]),
    ("ProtocolFee set_protocol_fee", &[1_943_807, 36_305, 0, 2, 2, 750, 296]),
    ("ProtocolFee state_version", &[87, 5]),
    ("ProtocolFee transfer_ownership", &[1_948_562, 38_194, 0, 2, 2, 817, 325]),
    ("ValidatorAnnounce announce, new validator, 1 KiB location", &[2_364_814, 362_820, 0, 2, 2, 1_905, 1_520]),
    ("ValidatorAnnounce announce, new validator, 50-byte location", &[2_172_240, 252_696, 0, 2, 2, 835, 446]),
    ("ValidatorAnnounce announce, second location of a validator", &[2_173_069, 252_642, 0, 2, 2, 835, 450]),
    ("ValidatorAnnounce announced_validator_count", &[88, 5]),
    ("ValidatorAnnounce get_announced_storage_location_hashes, 1 location", &[2_631, 44]),
    ("ValidatorAnnounce get_announced_storage_location_hashes, 16 locations", &[6_712, 572]),
    ("ValidatorAnnounce get_announced_validators, 256", &[46_937, 5_641]),
    ("ValidatorAnnounce local_domain", &[88, 5]),
    ("ValidatorAnnounce mailbox", &[215, 36]),
    ("ValidatorAnnounce state_version", &[87, 5]),
    ("WarpDrc20 allowance", &[25_902, 9]),
    ("WarpDrc20 approve, a new spender", &[1_966_790, 42_825, 0, 2, 2, 1_179, 507]),
    ("WarpDrc20 approve, an existing allowance", &[1_966_522, 42_557, 0, 2, 2, 1_179, 507]),
    ("WarpDrc20 balance_of", &[17_971, 9]),
    ("WarpDrc20 claim_pending", &[1_968_356, 61_847, 0, 2, 4, 1_304, 284]),
    ("WarpDrc20 claim_pending_contract, by the red-team contract", &[1_931_272, 18_283, 1, 3, 3, 1_249, 366]),
    ("WarpDrc20 claim_pending_for", &[61_600_185, 59_676_035, 0, 2, 4, 1_304, 509]),
    ("WarpDrc20 decimals", &[88, 2]),
    ("WarpDrc20 enroll_remote_router, enrolled domain", &[1_949_868, 38_967, 0, 2, 2, 787, 332]),
    ("WarpDrc20 enroll_remote_router, new domain", &[1_949_850, 38_949, 0, 2, 2, 787, 332]),
    ("WarpDrc20 enrolled_router", &[922, 36]),
    ("WarpDrc20 hook", &[215, 36]),
    ("WarpDrc20 interchain_security_module", &[215, 36]),
    ("WarpDrc20 is_registered", &[726, 2]),
    ("WarpDrc20 mailbox", &[215, 36]),
    ("WarpDrc20 name", &[911, 27]),
    ("WarpDrc20 owner", &[870, 37]),
    ("WarpDrc20 pending_balance", &[280, 9]),
    ("WarpDrc20 pending_total", &[104, 9]),
    ("WarpDrc20 process, existing escrow, 1 of 6 signatures", &[2_232_164, 301_294, 3, 3, 4, 931, 599]),
    ("WarpDrc20 process, existing escrow, 3 of 6 signatures", &[2_689_306, 747_683, 3, 3, 4, 931, 740]),
    ("WarpDrc20 process, existing escrow, 6 of 6 signatures", &[3_375_742, 1_417_656, 3, 3, 4, 931, 955]),
    ("WarpDrc20 process, mint to a registered account, 1 of 6 signatures", &[2_253_810, 322_940, 3, 3, 5, 1_386, 599]),
    ("WarpDrc20 process, mint to a registered account, 3 of 6 signatures", &[2_711_200, 769_578, 3, 3, 5, 1_386, 740]),
    ("WarpDrc20 process, mint to a registered account, 6 of 6 signatures", &[3_395_440, 1_437_354, 3, 3, 5, 1_386, 955]),
    ("WarpDrc20 process, new escrow, 1 of 6 signatures", &[2_232_583, 301_713, 3, 3, 4, 931, 599]),
    ("WarpDrc20 process, new escrow, 3 of 6 signatures", &[2_689_379, 747_757, 3, 3, 4, 931, 740]),
    ("WarpDrc20 process, new escrow, 6 of 6 signatures", &[3_374_630, 1_416_544, 3, 3, 4, 931, 955]),
    ("WarpDrc20 quote_transfer_remote", &[58_893, 9]),
    ("WarpDrc20 register_account, new account", &[1_944_057, 37_459, 0, 2, 2, 778, 288]),
    ("WarpDrc20 register_account, registered account", &[1_944_093, 37_496, 0, 2, 2, 778, 288]),
    ("WarpDrc20 set_hook", &[1_948_943, 39_351, 0, 2, 2, 767, 314]),
    ("WarpDrc20 set_ism", &[1_948_628, 39_347, 0, 2, 2, 766, 313]),
    ("WarpDrc20 state_version", &[87, 5]),
    ("WarpDrc20 symbol", &[784, 9]),
    ("WarpDrc20 total_supply", &[104, 9]),
    ("WarpDrc20 transfer, to a holder", &[1_968_232, 43_956, 0, 2, 2, 1_179, 508]),
    ("WarpDrc20 transfer, to a new holder", &[1_967_907, 43_631, 0, 2, 2, 1_179, 508]),
    ("WarpDrc20 transfer_from", &[2_004_768, 63_955, 0, 2, 2, 1_179, 724]),
    ("WarpDrc20 transfer_ownership", &[1_950_254, 39_886, 0, 2, 2, 817, 325]),
    ("WarpDrc20 transfer_remote", &[2_277_606, 356_420, 27, 7, 15, 2_587, 445]),
    ("WarpDrc20 withdraw_dispatch_credit", &[61_634_142, 59_708_501, 2, 4, 3, 1_111, 525]),
    ("WarpDrc20Collateral claim_pending", &[1_972_209, 65_766, 5, 3, 4, 1_304, 284]),
    ("WarpDrc20Collateral claim_pending_contract, by the red-team contract", &[1_942_902, 29_913, 6, 4, 3, 1_249, 366]),
    ("WarpDrc20Collateral claim_pending_for", &[61_604_835, 59_680_686, 5, 3, 4, 1_304, 509]),
    ("WarpDrc20Collateral enroll_remote_router, enrolled domain", &[1_949_868, 38_967, 0, 2, 2, 787, 332]),
    ("WarpDrc20Collateral enroll_remote_router, new domain", &[1_949_850, 38_949, 0, 2, 2, 787, 332]),
    ("WarpDrc20Collateral enrolled_router", &[922, 36]),
    ("WarpDrc20Collateral hook", &[215, 36]),
    ("WarpDrc20Collateral interchain_security_module", &[215, 36]),
    ("WarpDrc20Collateral is_registered", &[726, 2]),
    ("WarpDrc20Collateral mailbox", &[215, 36]),
    ("WarpDrc20Collateral owner", &[870, 37]),
    ("WarpDrc20Collateral pending_balance", &[280, 9]),
    ("WarpDrc20Collateral pending_total", &[104, 9]),
    ("WarpDrc20Collateral process, existing escrow, 1 of 6 signatures", &[2_234_794, 303_924, 4, 4, 4, 931, 599]),
    ("WarpDrc20Collateral process, existing escrow, 3 of 6 signatures", &[2_691_128, 749_506, 4, 4, 4, 931, 740]),
    ("WarpDrc20Collateral process, existing escrow, 6 of 6 signatures", &[3_376_397, 1_418_311, 4, 4, 4, 931, 955]),
    ("WarpDrc20Collateral process, new escrow, 1 of 6 signatures", &[2_234_780, 303_909, 4, 4, 4, 931, 599]),
    ("WarpDrc20Collateral process, new escrow, 3 of 6 signatures", &[2_691_139, 749_517, 4, 4, 4, 931, 740]),
    ("WarpDrc20Collateral process, new escrow, 6 of 6 signatures", &[3_376_005, 1_417_918, 4, 4, 4, 931, 955]),
    ("WarpDrc20Collateral process, payout to a registered account, 1 of 6 signatures", &[2_292_535, 361_665, 9, 4, 5, 1_386, 599]),
    ("WarpDrc20Collateral process, payout to a registered account, 3 of 6 signatures", &[2_748_843, 807_221, 9, 4, 5, 1_386, 740]),
    ("WarpDrc20Collateral process, payout to a registered account, 6 of 6 signatures", &[3_433_707, 1_475_621, 9, 4, 5, 1_386, 955]),
    ("WarpDrc20Collateral quote_transfer_remote", &[58_981, 9]),
    ("WarpDrc20Collateral register_account, new account", &[1_944_123, 37_459, 0, 2, 2, 778, 288]),
    ("WarpDrc20Collateral register_account, registered account", &[1_944_159, 37_496, 0, 2, 2, 778, 288]),
    ("WarpDrc20Collateral set_hook", &[1_948_943, 39_351, 0, 2, 2, 767, 314]),
    ("WarpDrc20Collateral set_ism", &[1_948_628, 39_347, 0, 2, 2, 766, 313]),
    ("WarpDrc20Collateral state_version", &[87, 5]),
    ("WarpDrc20Collateral transfer_ownership", &[1_950_254, 39_886, 0, 2, 2, 817, 325]),
    ("WarpDrc20Collateral transfer_remote", &[2_301_340, 380_154, 30, 7, 15, 2_587, 445]),
    ("WarpDrc20Collateral withdraw_dispatch_credit", &[61_634_724, 59_709_084, 2, 4, 3, 1_111, 525]),
    ("WarpDrc20Collateral wrapped_token", &[215, 36]),
    ("WarpNative claim_pending", &[1_961_925, 55_416, 1, 3, 4, 1_133, 284]),
    ("WarpNative claim_pending_contract, by the red-team contract", &[1_937_087, 24_098, 3, 5, 3, 895, 366]),
    ("WarpNative claim_pending_for", &[61_589_585, 59_665_436, 1, 3, 4, 1_133, 509]),
    ("WarpNative enroll_remote_router, enrolled domain", &[1_949_868, 38_967, 0, 2, 2, 787, 332]),
    ("WarpNative enroll_remote_router, new domain", &[1_949_850, 38_949, 0, 2, 2, 787, 332]),
    ("WarpNative enrolled_router", &[922, 36]),
    ("WarpNative hook", &[215, 36]),
    ("WarpNative interchain_security_module", &[215, 36]),
    ("WarpNative is_registered", &[726, 2]),
    ("WarpNative mailbox", &[215, 36]),
    ("WarpNative owner", &[870, 37]),
    ("WarpNative pending_balance", &[280, 9]),
    ("WarpNative pending_total", &[104, 9]),
    ("WarpNative process, existing escrow, 1 of 6 signatures", &[2_234_081, 303_211, 4, 4, 4, 931, 599]),
    ("WarpNative process, existing escrow, 3 of 6 signatures", &[2_691_087, 749_465, 4, 4, 4, 931, 740]),
    ("WarpNative process, existing escrow, 6 of 6 signatures", &[3_377_057, 1_418_970, 4, 4, 4, 931, 955]),
    ("WarpNative process, new escrow, 1 of 6 signatures", &[2_234_201, 303_331, 4, 4, 4, 931, 599]),
    ("WarpNative process, new escrow, 3 of 6 signatures", &[2_691_215, 749_592, 4, 4, 4, 931, 740]),
    ("WarpNative process, new escrow, 6 of 6 signatures", &[3_376_457, 1_418_371, 4, 4, 4, 931, 955]),
    ("WarpNative process, payout to a registered account, 1 of 6 signatures", &[2_249_625, 318_693, 5, 4, 5, 1_216, 599]),
    ("WarpNative process, payout to a registered account, 3 of 6 signatures", &[2_707_971, 766_287, 5, 4, 5, 1_216, 740]),
    ("WarpNative process, payout to a registered account, 6 of 6 signatures", &[3_393_322, 1_435_174, 5, 4, 5, 1_216, 955]),
    ("WarpNative quote_transfer_remote", &[58_981, 9]),
    ("WarpNative register_account, new account", &[1_943_924, 37_459, 0, 2, 2, 778, 288]),
    ("WarpNative register_account, registered account", &[1_943_961, 37_496, 0, 2, 2, 778, 288]),
    ("WarpNative set_hook", &[1_948_943, 39_351, 0, 2, 2, 767, 314]),
    ("WarpNative set_ism", &[1_948_628, 39_347, 0, 2, 2, 766, 313]),
    ("WarpNative state_version", &[87, 5]),
    ("WarpNative transfer_ownership", &[1_950_254, 39_886, 0, 2, 2, 817, 325]),
    ("WarpNative transfer_remote", &[2_238_303, 317_179, 27, 7, 14, 2_131, 445]),
    ("WarpNative transfer_remote, first message on the Mailbox", &[2_242_940, 321_839, 27, 7, 14, 2_131, 445]),
    ("WarpNative withdraw_dispatch_credit", &[61_634_142, 59_708_501, 2, 4, 3, 1_111, 525]),
];

// =============================================================================
// Rows
// =============================================================================

const ROUTES: [ContractId; 3] = [NATIVE_ID, SYNTHETIC_ID, COLLATERAL_ID];

fn route_name(route: ContractId) -> &'static str {
    match route {
        NATIVE_ID => "WarpNative",
        SYNTHETIC_ID => "WarpDrc20",
        _ => "WarpDrc20Collateral",
    }
}

/// The deposit `transfer_remote` of `amount` takes: the dispatch fee, and
/// on the native route the amount too.
fn transfer_deposit(net: &mut Net, route: ContractId, amount: u64) -> u64 {
    let fee = net.quote(route, amount);
    if route == NATIVE_ID {
        amount + fee
    } else {
        fee
    }
}

/// The largest `len` in `0..=hi` for which `fits(len)` holds, `fits` being
/// monotone.
fn largest_fitting(hi: usize, fits: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0usize, hi);
    assert!(fits(lo));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// A call the attacker contract makes from its `run` batch.
type Action = (ContractId, String, Vec<u8>);

fn action<A>(target: ContractId, fn_name: &str, args: &A) -> Action
where
    A: rkyv::Serialize<AllocSerializer<4096>>,
{
    (target, String::from(fn_name), archive(args))
}

/// The attacker contract makes `actions` in one transaction signed by the
/// owner; any failed call fails the transaction.
fn run(net: &mut Net, actions: Vec<Action>) -> CallReceipt<Vec<u8>> {
    net.call_raw(&OWNER_SK, ATTACKER_ID, "run", archive(&(actions, true)), 0)
}

#[test]
fn user_flows_fit_their_budgets() {
    let mut p = Profile::default();

    // The first transfer on a fresh Mailbox starts the first chunk of every
    // log it writes to.
    let mut net = Net::deployed();
    let deposit = transfer_deposit(&mut net, NATIVE_ID, 1_000);
    p.tx(
        "WarpNative transfer_remote, first message on the Mailbox",
        measure(
            &mut net,
            &USER_SK,
            NATIVE_ID,
            "transfer_remote",
            &(REMOTE_DOMAIN, [0xEE; 32], 1_000u64),
            deposit,
        ),
    );

    let mut net = Net::new();
    for route in ROUTES {
        let deposit = transfer_deposit(&mut net, route, 1_000);
        p.tx(
            &format!("{} transfer_remote", route_name(route)),
            measure(
                &mut net,
                &USER_SK,
                route,
                "transfer_remote",
                &(REMOTE_DOMAIN, [0xEE; 32], 1_000u64),
                deposit,
            ),
        );
    }

    // An account dispatching itself, from prepaid credit.
    let quote: u64 = net.query(
        MAILBOX_ID,
        "quote_dispatch_for_sender",
        &(
            *USER_ID,
            REMOTE_DOMAIN,
            [0xD1u8; 32],
            Vec::<u8>::new(),
            Vec::<u8>::new(),
            NO_HOOK,
        ),
    );
    assert_eq!(
        quote,
        PROTOCOL_FEE + 25_000,
        "protocol fee plus 250,000 gas at 1 gwei and 1e-10"
    );
    let credit = 100 * quote;
    for label in ["new payer", "existing payer"] {
        p.tx(
            &format!("Mailbox fund_dispatch, {label}"),
            measure(
                &mut net,
                &USER_SK,
                MAILBOX_ID,
                "fund_dispatch",
                &(*USER_ID, credit),
                credit,
            ),
        );
    }
    let dispatch = |len: usize| (REMOTE_DOMAIN, [0xD1u8; 32], vec![0xB0u8; len]);
    let largest = largest_fitting(ARGBUF_LEN, |len| {
        let args = archive(&dispatch(len));
        tx_bytes(&USER_SK, MAILBOX_ID, "dispatch_default", &args, 0).1 <= ARGBUF_LEN as u64
    });
    println!("cost|Mailbox dispatch_default, largest body|body bytes|{largest}");
    for (label, len) in [
        ("empty body", 0usize),
        ("64-byte body", 64),
        ("1 KiB body", 1024),
        ("largest body", largest),
    ] {
        p.tx(
            &format!("Mailbox dispatch_default, {label}"),
            measure(
                &mut net,
                &USER_SK,
                MAILBOX_ID,
                "dispatch_default",
                &dispatch(len),
                0,
            ),
        );
    }
    p.tx(
        "Mailbox dispatch, 64-byte body, free hook instead of the IGP",
        measure(
            &mut net,
            &USER_SK,
            MAILBOX_ID,
            "dispatch",
            &(
                REMOTE_DOMAIN,
                [0xD1u8; 32],
                vec![0xB0u8; 64],
                Vec::<u8>::new(),
                TEST_MOCK_ID,
            ),
            0,
        ),
    );
    p.tx(
        "Mailbox withdraw_dispatch_credit",
        measure(
            &mut net,
            &USER_SK,
            MAILBOX_ID,
            "withdraw_dispatch_credit",
            &(*USER_PK, 1_000u64),
            0,
        ),
    );
    for route in ROUTES {
        net.call(
            &OWNER_SK,
            MAILBOX_ID,
            "fund_dispatch",
            &(route.to_bytes(), 10_000u64),
            10_000,
        );
        p.tx(
            &format!("{} withdraw_dispatch_credit", route_name(route)),
            measure(
                &mut net,
                &OWNER_SK,
                route,
                "withdraw_dispatch_credit",
                &(*OWNER_PK, 1_000u64),
                0,
            ),
        );
    }

    // The synthetic token as a DRC20.
    let (_, holder, _) = fresh_key(1);
    let holder = Account::moonlight(&holder);
    for label in ["a new holder", "a holder"] {
        p.tx(
            &format!("WarpDrc20 transfer, to {label}"),
            measure(
                &mut net,
                &USER_SK,
                SYNTHETIC_ID,
                "transfer",
                &TransferCall {
                    to: holder,
                    amount: 10,
                },
                0,
            ),
        );
    }
    for (label, amount) in [
        ("a new spender", 1_000u64),
        ("an existing allowance", 2_000),
    ] {
        p.tx(
            &format!("WarpDrc20 approve, {label}"),
            measure(
                &mut net,
                &USER_SK,
                SYNTHETIC_ID,
                "approve",
                &ApproveCall {
                    spender: Account::moonlight(&OWNER_PK),
                    amount,
                },
                0,
            ),
        );
    }
    p.tx(
        "WarpDrc20 transfer_from",
        measure(
            &mut net,
            &OWNER_SK,
            SYNTHETIC_ID,
            "transfer_from",
            &TransferFromCall {
                owner: Account::moonlight(&USER_PK),
                to: holder,
                amount: 10,
            },
            0,
        ),
    );

    // Registrations and claims, on every route.
    for (index, route) in (0u64..).zip(ROUTES) {
        let name = route_name(route);
        let (sk, pk, _) = fresh_key(10 + index);
        net.s.transfer(&OWNER_SK, &pk, dusk(1.0));
        for label in ["new account", "registered account"] {
            p.tx(
                &format!("{name} register_account, {label}"),
                measure(&mut net, &sk, route, "register_account", &(), 0),
            );
        }

        let (sk, pk, id) = fresh_key(20 + index);
        net.deliver(route, id, 1_000);
        net.s.transfer(&OWNER_SK, &pk, dusk(1.0));
        p.tx(
            &format!("{name} claim_pending"),
            measure(&mut net, &sk, route, "claim_pending", &(), 0),
        );

        let (_, pk, id) = fresh_key(30 + index);
        net.deliver(route, id, 1_000);
        p.tx(
            &format!("{name} claim_pending_for"),
            measure(&mut net, &RELAYER_SK, route, "claim_pending_for", &(pk,), 0),
        );

        net.deliver(route, ATTACKER_ID.to_bytes(), 1_000);
        let actions = vec![action(route, "claim_pending_contract", &())];
        p.tx(
            &format!("{name} claim_pending_contract, by the red-team contract"),
            measure(&mut net, &OWNER_SK, ATTACKER_ID, "run", &(actions, true), 0),
        );
    }

    // What apps quote before a transfer.
    for route in ROUTES {
        p.query(
            &format!("{} quote_transfer_remote", route_name(route)),
            measure_query(
                &mut net,
                route,
                "quote_transfer_remote",
                &(REMOTE_DOMAIN, [0xEE; 32], 1_000u64),
            ),
        );
    }
    p.query(
        "Mailbox quote_dispatch_for_sender",
        measure_query(
            &mut net,
            MAILBOX_ID,
            "quote_dispatch_for_sender",
            &(
                *USER_ID,
                REMOTE_DOMAIN,
                [0xD1u8; 32],
                vec![0u8; 64],
                Vec::<u8>::new(),
                NO_HOOK,
            ),
        ),
    );

    // Paying for more gas for a message whose dispatch paid for too little,
    // as its sender or anyone else can: the exact quote, and a deposit with
    // a margin the IGP sends back.
    let id: H256 = net.query(MAILBOX_ID, "latest_dispatched_id", &());
    let quote: u64 = net.query(IGP_ID, "quote_pay_for_gas", &(REMOTE_DOMAIN, 100_000u64));
    assert_eq!(
        quote, 10_000,
        "100,000 gas at 1 gwei and 1e-10, no overhead"
    );
    for (label, deposit) in [("", quote), (", with a refund", quote + 1_000)] {
        p.tx(
            &format!("IGP pay_for_gas{label}"),
            measure(
                &mut net,
                &USER_SK,
                IGP_ID,
                "pay_for_gas",
                &(id, REMOTE_DOMAIN, 100_000u64, deposit),
                deposit,
            ),
        );
    }
    p.check();
}

/// What `deploy/testnet.sh` assumes a relayed `process` costs at `threshold`
/// signatures, and the least relayer gas limit it accepts for it.
fn script_estimate(threshold: u8) -> (u64, u64) {
    let estimate = 1_950_000 + 220_000 * u64::from(threshold);
    (estimate, estimate * 3 / 2)
}

#[test]
fn relayed_process_fits_its_budgets_and_the_relayer_limit() {
    let mut net = Net::new();
    let mut p = Profile::default();
    let mut escrow_seed = 100;
    for threshold in [1u8, 3, 6] {
        net.set_threshold(threshold);
        for route in ROUTES {
            let name = route_name(route);
            let payout = if route == SYNTHETIC_ID {
                "mint"
            } else {
                "payout"
            };
            escrow_seed += 1;
            let (_, _, escrow) = fresh_key(escrow_seed);
            for (path, recipient) in [
                (format!("{payout} to a registered account"), *USER_ID),
                (String::from("new escrow"), escrow),
                (String::from("existing escrow"), escrow),
            ] {
                let encoded = net.inbound(route, recipient, 1_000);
                let row = format!("{name} process, {path}, {threshold} of 6 signatures");
                let measured = p.tx(&row, measure_process(&mut net, encoded));
                // The relayer's 5M limit, with the headroom deploy/testnet.sh
                // insists on, and the least limit the script accepts at this
                // threshold, cover every delivery to a route.
                let (estimate, least_limit) = script_estimate(threshold);
                println!("cost|{row}|script estimate|{estimate}");
                assert!(
                    measured.tx_gas * 3 / 2 <= RELAYER_GAS_LIMIT,
                    "{row}: {} gas leaves less than 1.5x headroom under {RELAYER_GAS_LIMIT}",
                    measured.tx_gas
                );
                assert!(
                    measured.tx_gas <= least_limit,
                    "{row}: {} gas is more than the {least_limit} limit deploy/testnet.sh accepts",
                    measured.tx_gas
                );
            }
        }
        // A recipient contract whose `handle` only counts, using the
        // Mailbox's default ISM.
        let encoded = net.message([0xAB; 32], ATTACKER_ID, &[]);
        p.tx(
            &format!(
                "Mailbox process, recipient contract, empty body, {threshold} of 6 signatures"
            ),
            measure_process(&mut net, encoded),
        );
    }

    // Body sizes, at 6 signatures. The largest is what fits a transaction.
    let metadata_len = signed_metadata(&[0u8; 77], &net.validators[..6]).len();
    let largest = largest_fitting(ARGBUF_LEN, |len| {
        let encoded = message::encode(
            VERSION,
            0,
            REMOTE_DOMAIN,
            [0xAB; 32],
            LOCAL_DOMAIN,
            ATTACKER_ID.to_bytes(),
            &vec![0xB0u8; len],
        );
        let args = archive(&(vec![0u8; metadata_len], encoded));
        tx_bytes(&RELAYER_SK, MAILBOX_ID, "process", &args, 0).1 <= ARGBUF_LEN as u64
    });
    println!("cost|Mailbox process, recipient contract, largest body, 6 of 6 signatures|body bytes|{largest}");
    for (label, len) in [
        ("1 KiB body", 1024usize),
        ("16 KiB body", 16 * 1024),
        ("largest body", largest),
    ] {
        let encoded = net.message([0xAB; 32], ATTACKER_ID, &vec![0xB0u8; len]);
        p.tx(
            &format!("Mailbox process, recipient contract, {label}, 6 of 6 signatures"),
            measure_process(&mut net, encoded),
        );
    }

    // The cheapest delivery anyone can make: to a recipient that names
    // itself as its ISM, with no metadata.
    net.call(
        &OWNER_SK,
        ATTACKER_ID,
        "set_returns",
        &(0u64, ATTACKER_ID),
        0,
    );
    let encoded = net.message([0xAB; 32], ATTACKER_ID, &[]);
    p.tx(
        "Mailbox process, recipient that is its own ISM",
        measure(
            &mut net,
            &USER_SK,
            MAILBOX_ID,
            "process",
            &(Vec::<u8>::new(), encoded),
            0,
        ),
    );
    p.check();
}

#[test]
fn admin_calls_and_announcements_fit_their_budgets() {
    let mut net = Net::new();
    let mut p = Profile::default();
    let addresses: Vec<EthAddress> = net.validators.iter().map(|(a, _)| *a).collect();

    // The ISM's own check, called directly, by signature count and set size.
    for threshold in [1u8, 3, 6] {
        net.set_threshold(threshold);
        let encoded = net.message([0xAB; 32], ATTACKER_ID, &[]);
        let metadata = net.metadata(&encoded);
        p.tx(
            &format!("ISM verify, {threshold} of 6 signatures"),
            measure(
                &mut net,
                &RELAYER_SK,
                ISM_ID,
                "verify",
                &(metadata, encoded),
                0,
            ),
        );
    }
    p.tx(
        "ISM set_validators_and_threshold, 6 validators",
        measure(
            &mut net,
            &OWNER_SK,
            ISM_ID,
            "set_validators_and_threshold",
            &(addresses.clone(), 1u8),
            0,
        ),
    );
    p.tx(
        "ISM transfer_ownership",
        measure(
            &mut net,
            &OWNER_SK,
            ISM_ID,
            "transfer_ownership",
            &(*OWNER_ID,),
            0,
        ),
    );
    // The most validators the ISM takes, signing at the end of the list.
    let many = validator_keys(255);
    let many_addresses: Vec<EthAddress> = many.iter().map(|(a, _)| *a).collect();
    deploy(
        &mut net.s,
        "hyperlane_dusk_ism_multisig",
        BIG_ISM_ID,
        Some(&(*OWNER_ID, vec![many_addresses[0]], 1u8)),
    );
    p.tx(
        "ISM set_validators_and_threshold, 255 validators",
        measure(
            &mut net,
            &OWNER_SK,
            BIG_ISM_ID,
            "set_validators_and_threshold",
            &(many_addresses, 6u8),
            0,
        ),
    );
    let encoded = net.message([0xAB; 32], ATTACKER_ID, &[]);
    let metadata = signed_metadata(&encoded, &many[249..]);
    p.tx(
        "ISM verify, 6 of 255 signatures",
        measure(
            &mut net,
            &RELAYER_SK,
            BIG_ISM_ID,
            "verify",
            &(metadata, encoded),
            0,
        ),
    );

    // Announcements.
    let location = String::from("s3://hyperlane-validator-signatures-dusk/eu-west-1");
    let (validator, signature) = announcement(0, &location);
    p.tx(
        &format!(
            "ValidatorAnnounce announce, new validator, {}-byte location",
            location.len()
        ),
        measure(
            &mut net,
            &RELAYER_SK,
            VALIDATOR_ANNOUNCE_ID,
            "announce",
            &(validator, location, signature),
            0,
        ),
    );
    let location = String::from("s3://hyperlane-validator-signatures-dusk-2/eu-west-1");
    let (validator, signature) = announcement(0, &location);
    p.tx(
        "ValidatorAnnounce announce, second location of a validator",
        measure(
            &mut net,
            &RELAYER_SK,
            VALIDATOR_ANNOUNCE_ID,
            "announce",
            &(validator, location, signature),
            0,
        ),
    );
    let mut location = String::from("s3://");
    location.extend(std::iter::repeat_n('x', 1024 - location.len()));
    let (validator, signature) = announcement(1, &location);
    p.tx(
        "ValidatorAnnounce announce, new validator, 1 KiB location",
        measure(
            &mut net,
            &RELAYER_SK,
            VALIDATOR_ANNOUNCE_ID,
            "announce",
            &(validator, location, signature),
            0,
        ),
    );

    // Fee contracts. Net::new paid both through the routes' transfers.
    let pricing = (REMOTE_DOMAIN, EVM_PRICING);
    p.tx(
        "IGP set_domain_gas_config",
        measure(
            &mut net,
            &OWNER_SK,
            IGP_ID,
            "set_domain_gas_config",
            &pricing,
            0,
        ),
    );
    p.tx(
        "IGP set_domain_gas_configs, 2 domains",
        measure(
            &mut net,
            &OWNER_SK,
            IGP_ID,
            "set_domain_gas_configs",
            &(vec![pricing, (1u32, EVM_PRICING)],),
            0,
        ),
    );
    for contract in [IGP_ID, PROTOCOL_FEE_ID] {
        let name = if contract == IGP_ID {
            "IGP"
        } else {
            "ProtocolFee"
        };
        p.tx(
            &format!("{name} claim"),
            measure(&mut net, &OWNER_SK, contract, "claim", &(), 0),
        );
        p.tx(
            &format!("{name} set_beneficiary"),
            measure(
                &mut net,
                &OWNER_SK,
                contract,
                "set_beneficiary",
                &(*OWNER_ID,),
                0,
            ),
        );
        p.tx(
            &format!("{name} transfer_ownership"),
            measure(
                &mut net,
                &OWNER_SK,
                contract,
                "transfer_ownership",
                &(*OWNER_ID,),
                0,
            ),
        );
    }
    p.tx(
        "ProtocolFee set_protocol_fee",
        measure(
            &mut net,
            &OWNER_SK,
            PROTOCOL_FEE_ID,
            "set_protocol_fee",
            &(PROTOCOL_FEE,),
            0,
        ),
    );

    // Routes.
    for route in ROUTES {
        let name = route_name(route);
        p.tx(
            &format!("{name} enroll_remote_router, enrolled domain"),
            measure(
                &mut net,
                &OWNER_SK,
                route,
                "enroll_remote_router",
                &(REMOTE_DOMAIN, [0xA9u8; 32]),
                0,
            ),
        );
        p.tx(
            &format!("{name} enroll_remote_router, new domain"),
            measure(
                &mut net,
                &OWNER_SK,
                route,
                "enroll_remote_router",
                &(1u32, [0xA9u8; 32]),
                0,
            ),
        );
        p.tx(
            &format!("{name} set_hook"),
            measure(&mut net, &OWNER_SK, route, "set_hook", &(NO_HOOK,), 0),
        );
        p.tx(
            &format!("{name} set_ism"),
            measure(&mut net, &OWNER_SK, route, "set_ism", &(NO_HOOK,), 0),
        );
        p.tx(
            &format!("{name} transfer_ownership"),
            measure(
                &mut net,
                &OWNER_SK,
                route,
                "transfer_ownership",
                &(*OWNER_ID,),
                0,
            ),
        );
    }

    // The Mailbox's owner calls, renouncing last.
    for (fn_name, contract) in [
        ("set_default_ism", ISM_ID),
        ("set_default_hook", IGP_ID),
        ("set_required_hook", AGGREGATION_HOOK_ID),
    ] {
        p.tx(
            &format!("Mailbox {fn_name}"),
            measure(&mut net, &OWNER_SK, MAILBOX_ID, fn_name, &(contract,), 0),
        );
    }
    p.tx(
        "Mailbox transfer_ownership",
        measure(
            &mut net,
            &OWNER_SK,
            MAILBOX_ID,
            "transfer_ownership",
            &(*OWNER_ID,),
            0,
        ),
    );
    p.tx(
        "Mailbox renounce_ownership",
        measure(
            &mut net,
            &OWNER_SK,
            MAILBOX_ID,
            "renounce_ownership",
            &(),
            0,
        ),
    );
    p.check();
}

/// Dispatches `count` messages through the deployed hooks from the attacker
/// contract, 256 to a transaction, from credit funded for it.
fn dispatch_from_contract(net: &mut Net, count: u32) {
    let credit = u64::from(count) * (PROTOCOL_FEE + 25_000);
    net.call(
        &OWNER_SK,
        MAILBOX_ID,
        "fund_dispatch",
        &(ATTACKER_ID.to_bytes(), credit),
        credit,
    );
    let mut sent = 0;
    while sent < count {
        let batch = (count - sent).min(256);
        let actions = (sent..sent + batch)
            .map(|i| {
                action(
                    MAILBOX_ID,
                    "dispatch",
                    &(
                        REMOTE_DOMAIN,
                        [0xD1u8; 32],
                        i.to_le_bytes().to_vec(),
                        Vec::<u8>::new(),
                        NO_HOOK,
                    ),
                )
            })
            .collect();
        run(net, actions);
        sent += batch;
    }
}

/// The queries agents and apps make, on a Mailbox with 2,304 dispatches
/// (past the first 2,048-ID chunk), 2,304 IGP payments (past two 1,024-record
/// chunks), 256 announced validators, a validator with all 16 locations, and
/// an ISM with 255 validators. Every reply must stay under the 64 KiB a
/// reply can take, and every query under the block gas limit.
#[test]
fn agent_queries_fit_their_budgets_and_reply_limits() {
    let mut net = Net::new();
    let mut p = Profile::default();
    dispatch_from_contract(&mut net, 2_304);
    let nonce: u32 = net.query(MAILBOX_ID, "nonce", &());
    let last = nonce - 1;

    // 256 validators, and one of them with 16 locations.
    let actions = (0..256u64)
        .map(|i| {
            let location = format!("s3://hyperlane-validator-{i:04}/eu-west-1");
            let (validator, signature) = announcement(1_000 + i, &location);
            action(
                VALIDATOR_ANNOUNCE_ID,
                "announce",
                &(validator, location, signature),
            )
        })
        .collect();
    run(&mut net, actions);
    let actions = (1..16u64)
        .map(|i| {
            let location = format!("s3://hyperlane-validator-0000/{i}");
            let (validator, signature) = announcement(1_000, &location);
            action(
                VALIDATOR_ANNOUNCE_ID,
                "announce",
                &(validator, location, signature),
            )
        })
        .collect();
    run(&mut net, actions);
    let (busy, _) = announcement(1_000, "");
    let (quiet, _) = announcement(1_001, "");
    let many = validator_keys(255);
    deploy(
        &mut net.s,
        "hyperlane_dusk_ism_multisig",
        BIG_ISM_ID,
        Some(&(*OWNER_ID, vec![many[0].0], 1u8)),
    );
    net.call(
        &OWNER_SK,
        BIG_ISM_ID,
        "set_validators_and_threshold",
        &(many.iter().map(|(a, _)| *a).collect::<Vec<_>>(), 6u8),
        0,
    );
    let sample = net.message([0xAB; 32], NATIVE_ID, &[0u8; 64]);
    let outbound = message::encode(
        VERSION,
        0,
        LOCAL_DOMAIN,
        NATIVE_ID.to_bytes(),
        REMOTE_DOMAIN,
        NATIVE_ROUTER,
        &[0u8; 64],
    );
    let hook_args = (Vec::<u8>::new(), outbound);
    let unit = ();

    macro_rules! q {
        ($row:expr, $contract:expr, $fn_name:expr, $args:expr) => {
            p.query($row, measure_query(&mut net, $contract, $fn_name, $args))
        };
    }
    q!("Mailbox nonce", MAILBOX_ID, "nonce", &unit);
    q!(
        "Mailbox latest_dispatched_id",
        MAILBOX_ID,
        "latest_dispatched_id",
        &unit
    );
    q!(
        "Mailbox dispatched_id",
        MAILBOX_ID,
        "dispatched_id",
        &(last,)
    );
    q!(
        "Mailbox dispatched_block_height",
        MAILBOX_ID,
        "dispatched_block_height",
        &(last,)
    );
    q!(
        "Mailbox delivered",
        MAILBOX_ID,
        "delivered",
        &(message::id(&sample),)
    );
    q!(
        "Mailbox recipient_ism",
        MAILBOX_ID,
        "recipient_ism",
        &(NATIVE_ID,)
    );
    q!("Mailbox default_ism", MAILBOX_ID, "default_ism", &unit);
    q!("Mailbox default_hook", MAILBOX_ID, "default_hook", &unit);
    q!("Mailbox required_hook", MAILBOX_ID, "required_hook", &unit);
    q!("Mailbox local_domain", MAILBOX_ID, "local_domain", &unit);
    q!("Mailbox owner", MAILBOX_ID, "owner", &unit);
    q!(
        "Mailbox fee_credit",
        MAILBOX_ID,
        "fee_credit",
        &(ATTACKER_ID.to_bytes(),)
    );
    q!("Mailbox memory_bytes", MAILBOX_ID, "memory_bytes", &unit);
    q!("Mailbox state_version", MAILBOX_ID, "state_version", &unit);

    q!("MerkleTreeHook count", MERKLE_TREE_HOOK_ID, "count", &unit);
    q!("MerkleTreeHook root", MERKLE_TREE_HOOK_ID, "root", &unit);
    q!(
        "MerkleTreeHook latest_checkpoint",
        MERKLE_TREE_HOOK_ID,
        "latest_checkpoint",
        &unit
    );
    q!("MerkleTreeHook tree", MERKLE_TREE_HOOK_ID, "tree", &unit);
    q!(
        "MerkleTreeHook message_id_at",
        MERKLE_TREE_HOOK_ID,
        "message_id_at",
        &(last,)
    );
    q!(
        "MerkleTreeHook message_ids, 256 from 0",
        MERKLE_TREE_HOOK_ID,
        "message_ids",
        &(0u32, 256u32)
    );
    q!(
        "MerkleTreeHook message_ids, 256 across two chunks",
        MERKLE_TREE_HOOK_ID,
        "message_ids",
        &(2_040u32, 256u32)
    );
    q!(
        "MerkleTreeHook inserted_block_height",
        MERKLE_TREE_HOOK_ID,
        "inserted_block_height",
        &(last,)
    );
    q!(
        "MerkleTreeHook quote_dispatch",
        MERKLE_TREE_HOOK_ID,
        "quote_dispatch",
        &hook_args
    );
    q!(
        "MerkleTreeHook hook_type",
        MERKLE_TREE_HOOK_ID,
        "hook_type",
        &unit
    );
    q!(
        "MerkleTreeHook mailbox",
        MERKLE_TREE_HOOK_ID,
        "mailbox",
        &unit
    );
    q!(
        "MerkleTreeHook state_version",
        MERKLE_TREE_HOOK_ID,
        "state_version",
        &unit
    );

    q!(
        "AggregationHook quote_dispatch",
        AGGREGATION_HOOK_ID,
        "quote_dispatch",
        &hook_args
    );
    q!("AggregationHook hooks", AGGREGATION_HOOK_ID, "hooks", &unit);
    q!(
        "AggregationHook hook_type",
        AGGREGATION_HOOK_ID,
        "hook_type",
        &unit
    );
    q!(
        "AggregationHook mailbox",
        AGGREGATION_HOOK_ID,
        "mailbox",
        &unit
    );
    q!(
        "AggregationHook state_version",
        AGGREGATION_HOOK_ID,
        "state_version",
        &unit
    );

    q!(
        "ProtocolFee quote_dispatch",
        PROTOCOL_FEE_ID,
        "quote_dispatch",
        &hook_args
    );
    for fn_name in [
        "protocol_fee",
        "max_protocol_fee",
        "collected_fees",
        "claimable_fees",
        "beneficiary",
        "owner",
        "mailbox",
        "hook_type",
        "state_version",
    ] {
        q!(
            &format!("ProtocolFee {fn_name}"),
            PROTOCOL_FEE_ID,
            fn_name,
            &unit
        );
    }

    q!("IGP quote_dispatch", IGP_ID, "quote_dispatch", &hook_args);
    q!(
        "IGP quote_gas_payment",
        IGP_ID,
        "quote_gas_payment",
        &(REMOTE_DOMAIN, 50_000u64)
    );
    q!(
        "IGP quote_pay_for_gas",
        IGP_ID,
        "quote_pay_for_gas",
        &(REMOTE_DOMAIN, 100_000u64)
    );
    q!(
        "IGP domain_gas_config",
        IGP_ID,
        "domain_gas_config",
        &(REMOTE_DOMAIN,)
    );
    q!("IGP gas_payment_at", IGP_ID, "gas_payment_at", &(last,));
    q!(
        "IGP gas_payments, 256 from 0",
        IGP_ID,
        "gas_payments",
        &(0u32, 256u32)
    );
    q!(
        "IGP gas_payments, 256 across two chunks",
        IGP_ID,
        "gas_payments",
        &(1_020u32, 256u32)
    );
    for fn_name in [
        "gas_payment_count",
        "total_gas_payments",
        "claimable_fees",
        "beneficiary",
        "owner",
        "mailbox",
        "hook_type",
        "state_version",
    ] {
        q!(&format!("IGP {fn_name}"), IGP_ID, fn_name, &unit);
    }

    for fn_name in ["validators_and_threshold", "validators"] {
        q!(
            &format!("ISM {fn_name}, 6 validators"),
            ISM_ID,
            fn_name,
            &unit
        );
        q!(
            &format!("ISM {fn_name}, 255 validators"),
            BIG_ISM_ID,
            fn_name,
            &unit
        );
    }
    for fn_name in ["threshold", "module_type", "owner", "state_version"] {
        q!(&format!("ISM {fn_name}"), ISM_ID, fn_name, &unit);
    }

    q!(
        "ValidatorAnnounce get_announced_validators, 256",
        VALIDATOR_ANNOUNCE_ID,
        "get_announced_validators",
        &(0u32, 256u32)
    );
    q!(
        "ValidatorAnnounce get_announced_storage_location_hashes, 16 locations",
        VALIDATOR_ANNOUNCE_ID,
        "get_announced_storage_location_hashes",
        &(busy,)
    );
    q!(
        "ValidatorAnnounce get_announced_storage_location_hashes, 1 location",
        VALIDATOR_ANNOUNCE_ID,
        "get_announced_storage_location_hashes",
        &(quiet,)
    );
    for fn_name in [
        "announced_validator_count",
        "local_domain",
        "mailbox",
        "state_version",
    ] {
        q!(
            &format!("ValidatorAnnounce {fn_name}"),
            VALIDATOR_ANNOUNCE_ID,
            fn_name,
            &unit
        );
    }

    for route in ROUTES {
        let name = route_name(route);
        q!(
            &format!("{name} is_registered"),
            route,
            "is_registered",
            &(*USER_ID,)
        );
        q!(
            &format!("{name} pending_balance"),
            route,
            "pending_balance",
            &(*OWNER_ID,)
        );
        q!(
            &format!("{name} enrolled_router"),
            route,
            "enrolled_router",
            &(REMOTE_DOMAIN,)
        );
        for fn_name in [
            "pending_total",
            "interchain_security_module",
            "hook",
            "mailbox",
            "owner",
            "state_version",
        ] {
            q!(&format!("{name} {fn_name}"), route, fn_name, &unit);
        }
    }
    for fn_name in ["name", "symbol", "decimals", "total_supply"] {
        q!(
            &format!("WarpDrc20 {fn_name}"),
            SYNTHETIC_ID,
            fn_name,
            &unit
        );
    }
    q!(
        "WarpDrc20 balance_of",
        SYNTHETIC_ID,
        "balance_of",
        &hyperlane_dusk_types::drc20::BalanceOf {
            account: Account::moonlight(&USER_PK)
        }
    );
    q!(
        "WarpDrc20 allowance",
        SYNTHETIC_ID,
        "allowance",
        &hyperlane_dusk_types::drc20::Allowance {
            owner: Account::moonlight(&USER_PK),
            spender: Account::Contract(COLLATERAL_ID)
        }
    );
    q!(
        "WarpDrc20Collateral wrapped_token",
        COLLATERAL_ID,
        "wrapped_token",
        &unit
    );
    p.check();
}

// =============================================================================
// Memory growth per entry
// =============================================================================

/// What a permissionless growth path costs per entry: the memory each
/// contract gained over `entries` of them, and the gas of one more entry,
/// marginal when a contract makes many in one transaction, or a whole
/// transaction when each needs its own.
struct Growth {
    path: String,
    entries: u32,
    gas: u64,
    batched: bool,
    grown: Vec<(&'static str, u64)>,
}

impl Growth {
    fn print(&self) {
        let metric = if self.batched {
            "marginal gas"
        } else {
            "tx gas"
        };
        println!("cost|{}|{metric}|{}", self.path, self.gas);
        for (contract, bytes) in self.grown.iter().filter(|(_, bytes)| *bytes > 0) {
            println!(
                "cost|{}|{contract} bytes per entry|{:.1}",
                self.path,
                *bytes as f64 / f64::from(self.entries)
            );
        }
    }
}

/// The memory each watched contract gained since `before`.
fn grown_since(net: &mut Net, before: &[u64]) -> Vec<(&'static str, u64)> {
    let after = net.memory();
    CONTRACTS
        .iter()
        .zip(before.iter().zip(&after))
        .map(|((name, _), (b, a))| (*name, a - b))
        .collect()
}

/// Makes `entries` calls `make(i)` from the attacker contract, `per_batch`
/// to a transaction, after a warm-up batch that starts any first chunk.
/// The marginal gas is the difference between a batch and a single call.
fn grow_batched(
    net: &mut Net,
    path: &str,
    entries: u32,
    per_batch: u32,
    mut make: impl FnMut(u32) -> Action,
) -> Growth {
    let single = run(net, vec![make(0)]).gas_spent;
    let batch = run(net, (1..=per_batch).map(&mut make).collect()).gas_spent;
    let gas = (batch - single) / u64::from(per_batch - 1);
    let before = net.memory();
    let mut next = per_batch + 1;
    let end = next + entries;
    while next < end {
        let count = per_batch.min(end - next);
        run(net, (next..next + count).map(&mut make).collect());
        next += count;
    }
    let growth = Growth {
        path: path.to_string(),
        entries,
        gas,
        batched: true,
        grown: grown_since(net, &before),
    };
    growth.print();
    growth
}

/// Runs `entries` transactions `make(net, i)`, each adding one entry and
/// returning its gas, after one warm-up transaction.
fn grow_per_tx(
    net: &mut Net,
    path: &str,
    entries: u32,
    mut make: impl FnMut(&mut Net, u32) -> u64,
) -> Growth {
    make(net, 0);
    let before = net.memory();
    let gas: u64 = (1..=entries).map(|i| make(net, i)).sum();
    let growth = Growth {
        path: path.to_string(),
        entries,
        gas: gas / u64::from(entries),
        batched: false,
        grown: grown_since(net, &before),
    };
    growth.print();
    growth
}

/// Dispatches and deliveries: the Mailbox, its hooks and the IGP.
fn messaging_growth(entries: u32) -> Vec<Growth> {
    let mut net = Net::new();
    let credit = u64::from(entries + 600) * (PROTOCOL_FEE + 25_000);
    net.call(
        &OWNER_SK,
        MAILBOX_ID,
        "fund_dispatch",
        &(ATTACKER_ID.to_bytes(), credit),
        credit,
    );
    let mut rows = vec![grow_batched(
        &mut net,
        "dispatch through the deployed hooks",
        entries,
        256,
        |i| {
            action(
                MAILBOX_ID,
                "dispatch",
                &(
                    REMOTE_DOMAIN,
                    [0xD1u8; 32],
                    i.to_le_bytes().to_vec(),
                    Vec::<u8>::new(),
                    NO_HOOK,
                ),
            )
        },
    )];
    net.call(
        &OWNER_SK,
        ATTACKER_ID,
        "set_returns",
        &(0u64, ATTACKER_ID),
        0,
    );
    rows.push(grow_batched(
        &mut net,
        "delivery to a recipient that is its own ISM",
        entries,
        256,
        |i| {
            let encoded = message::encode(
                VERSION,
                i,
                REMOTE_DOMAIN,
                [0xAB; 32],
                LOCAL_DOMAIN,
                ATTACKER_ID.to_bytes(),
                b"",
            );
            action(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
        },
    ));
    rows
}

/// Announcements and the synthetic token's balances and allowances.
fn registry_growth(entries: u32) -> Vec<Growth> {
    let mut net = Net::new();
    // The contract keeps the location's hash, so the shortest location
    // fills it for the least gas; the longest costs the most per entry.
    let mut rows = vec![grow_batched(
        &mut net,
        "announcement of a new validator, 1-byte location",
        entries,
        256,
        |i| {
            let location = String::from("x");
            let (validator, signature) = announcement(10_000_000 + u64::from(i), &location);
            action(
                VALIDATOR_ANNOUNCE_ID,
                "announce",
                &(validator, location, signature),
            )
        },
    )];
    rows.push(grow_batched(
        &mut net,
        "announcement of a new validator, 1 KiB location",
        entries,
        48,
        |i| {
            let mut location = format!("s3://{i:08}/");
            location.extend(std::iter::repeat_n('x', 1024 - location.len()));
            let (validator, signature) = announcement(10_000 + u64::from(i), &location);
            action(
                VALIDATOR_ANNOUNCE_ID,
                "announce",
                &(validator, location, signature),
            )
        },
    ));
    // The attacker contract holds synthetic tokens to give away.
    net.deliver(SYNTHETIC_ID, ATTACKER_ID.to_bytes(), 1_000_000_000);
    run(
        &mut net,
        vec![action(SYNTHETIC_ID, "claim_pending_contract", &())],
    );
    let new_account = |i: u32, seed: u8| {
        let mut key = [seed; 193];
        key[..4].copy_from_slice(&i.to_le_bytes());
        Account::Moonlight(key)
    };
    rows.push(grow_batched(
        &mut net,
        "WarpDrc20 transfer to a new holder",
        entries,
        128,
        |i| {
            action(
                SYNTHETIC_ID,
                "transfer",
                &TransferCall {
                    to: new_account(i, 0x61),
                    amount: 1,
                },
            )
        },
    ));
    rows.push(grow_batched(
        &mut net,
        "WarpDrc20 approval of a new spender",
        entries,
        128,
        |i| {
            action(
                SYNTHETIC_ID,
                "approve",
                &ApproveCall {
                    spender: new_account(i, 0x62),
                    amount: 1,
                },
            )
        },
    ));
    rows
}

/// Route escrow, route registrations and Mailbox credit, which take a
/// relayed delivery or a transaction per entry.
fn route_growth(batched_entries: u32, tx_entries: u32) -> Vec<Growth> {
    let mut net = Net::new();
    let mut rows = Vec::new();
    for route in ROUTES {
        // Batched deliveries need an ISM that takes no signatures.
        net.call(&OWNER_SK, route, "set_ism", &(TEST_MOCK_ID,), 0);
        let router = match route {
            NATIVE_ID => NATIVE_ROUTER,
            SYNTHETIC_ID => SYNTHETIC_ROUTER,
            _ => COLLATERAL_ROUTER,
        };
        rows.push(grow_batched(
            &mut net,
            &format!("{} escrow for a new recipient", route_name(route)),
            batched_entries,
            256,
            |i| {
                let mut recipient = [0x5E; 32];
                recipient[..4].copy_from_slice(&i.to_le_bytes());
                let encoded = message::encode(
                    VERSION,
                    1_000_000 + i,
                    REMOTE_DOMAIN,
                    router,
                    LOCAL_DOMAIN,
                    route.to_bytes(),
                    &token_message::encode(recipient, 1),
                );
                action(MAILBOX_ID, "process", &(Vec::<u8>::new(), encoded))
            },
        ));
    }
    rows.push(grow_per_tx(
        &mut net,
        "Mailbox credit for a new payer",
        tx_entries,
        |net, i| {
            let payer = message::keccak256(&i.to_le_bytes());
            net.call(&OWNER_SK, MAILBOX_ID, "fund_dispatch", &(payer, 1u64), 1)
                .gas_spent
        },
    ));
    // A top-up records a payment for any message ID, paid from its
    // transaction's deposit, so each takes a transaction. 1,000 gas costs
    // 100 LUX at the profile's pricing; the amount doesn't change the gas.
    let quote: u64 = net.query(IGP_ID, "quote_pay_for_gas", &(REMOTE_DOMAIN, 1_000u64));
    rows.push(grow_per_tx(&mut net, "IGP top-up", tx_entries, |net, i| {
        let id = message::keccak256(&i.to_le_bytes());
        net.call(
            &OWNER_SK,
            IGP_ID,
            "pay_for_gas",
            &(id, REMOTE_DOMAIN, 1_000u64, quote),
            quote,
        )
        .gas_spent
    }));
    for route in ROUTES {
        rows.push(grow_per_tx(
            &mut net,
            &format!("{} registration of a new account", route_name(route)),
            tx_entries,
            |net, i| {
                let (sk, pk, _) = fresh_key(0x1_0000 + u64::from(i));
                if route == NATIVE_ID {
                    net.s.transfer(&OWNER_SK, &pk, dusk(1.0));
                }
                net.call(&sk, route, "register_account", &(), 0).gas_spent
            },
        ));
    }
    rows
}

/// Bytes per entry and gas per entry, over 32,768 entries (4,096 for the
/// paths that take a transaction each), for docs/costs.md. Memory grows in
/// 64 KiB pages, so it takes that many entries to resolve a few bytes per
/// entry. Slow; run it in release mode:
///
///   cargo test --release -p hyperlane-dusk-integration-tests --test cost_profile \
///       growth -- --ignored --nocapture
const PRECISE_GROWTH_ENTRIES: (u32, u32) = (32_768, 4_096);
/// Per path and contract, the most bytes per entry: the measured value
/// plus 10%, rounded up. A contract a path doesn't grow needs no entry.
#[rustfmt::skip]
const PRECISE_GROWTH_BUDGETS: &[(&str, &str, f64)] = &[
    ("dispatch through the deployed hooks", "Mailbox", 36.0),
    ("dispatch through the deployed hooks", "MerkleTreeHook", 36.0),
    ("dispatch through the deployed hooks", "IGP", 71.0),
    ("delivery to a recipient that is its own ISM", "Mailbox", 29.0),
    ("announcement of a new validator, 1-byte location", "ValidatorAnnounce", 119.0),
    ("announcement of a new validator, 1 KiB location", "ValidatorAnnounce", 122.0),
    ("WarpDrc20 transfer to a new holder", "WarpDrc20", 69.0),
    ("WarpDrc20 approval of a new spender", "WarpDrc20", 66.0),
    ("WarpNative escrow for a new recipient", "Mailbox", 29.0),
    ("WarpNative escrow for a new recipient", "WarpNative", 82.0),
    ("WarpDrc20 escrow for a new recipient", "Mailbox", 29.0),
    ("WarpDrc20 escrow for a new recipient", "WarpDrc20", 82.0),
    ("WarpDrc20Collateral escrow for a new recipient", "Mailbox", 29.0),
    ("WarpDrc20Collateral escrow for a new recipient", "WarpDrc20Collateral", 82.0),
    ("Mailbox credit for a new payer", "Mailbox", 71.0),
    ("IGP top-up", "IGP", 71.0),
    ("WarpNative registration of a new account", "WarpNative", 370.0),
    ("WarpDrc20 registration of a new account", "WarpDrc20", 370.0),
    ("WarpDrc20Collateral registration of a new account", "WarpDrc20Collateral", 370.0),
];
/// Per path, the most gas per entry, marginal or per transaction: the
/// measured value plus 5%, rounded up.
#[rustfmt::skip]
const PRECISE_GAS_BUDGETS: &[(&str, u64)] = &[
    ("dispatch through the deployed hooks", 189_029),
    ("delivery to a recipient that is its own ISM", 36_885),
    ("announcement of a new validator, 1-byte location", 265_744),
    ("announcement of a new validator, 1 KiB location", 471_726),
    ("WarpDrc20 transfer to a new holder", 59_120),
    ("WarpDrc20 approval of a new spender", 55_696),
    ("WarpNative escrow for a new recipient", 60_782),
    ("WarpDrc20 escrow for a new recipient", 60_973),
    ("WarpDrc20Collateral escrow for a new recipient", 62_881),
    ("Mailbox credit for a new payer", 1_922_682),
    ("IGP top-up", 1_923_911),
    ("WarpNative registration of a new account", 1_947_220),
    ("WarpDrc20 registration of a new account", 1_947_344),
    ("WarpDrc20Collateral registration of a new account", 1_947_344),
];

#[test]
#[ignore = "slow: memory per entry over 32,768 entries; run with --release"]
fn growth_per_entry_fits_its_budget() {
    let (batched, per_tx) = PRECISE_GROWTH_ENTRIES;
    let mut rows = messaging_growth(batched);
    rows.extend(registry_growth(batched));
    rows.extend(route_growth(batched, per_tx));
    let mut failures = Vec::new();
    for row in &rows {
        match PRECISE_GAS_BUDGETS
            .iter()
            .find(|(path, _)| *path == row.path)
        {
            Some((_, limit)) if row.gas > *limit => {
                failures.push(format!("{}: {} gas per entry > {limit}", row.path, row.gas));
            }
            None => failures.push(format!("{}: no gas budget", row.path)),
            _ => {}
        }
        // The attacker contract only stands in for whoever makes the calls.
        for (contract, bytes) in row.grown.iter().filter(|(c, _)| *c != "attacker") {
            let per_entry = *bytes as f64 / f64::from(row.entries);
            let budget = PRECISE_GROWTH_BUDGETS
                .iter()
                .find(|(path, c, _)| *path == row.path && c == contract)
                .map(|(_, _, b)| *b);
            match budget {
                Some(limit) if per_entry > limit => failures.push(format!(
                    "{}: {contract} {per_entry:.1} bytes per entry > {limit}",
                    row.path
                )),
                None if *bytes > 0 => {
                    failures.push(format!("{}: {contract} has no budget", row.path));
                }
                _ => {}
            }
        }
    }
    assert!(failures.is_empty(), "over budget:\n{}", failures.join("\n"));
}
