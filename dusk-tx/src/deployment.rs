//! What `deploy-hyperlane` reads back once its contracts are deployed.
//!
//! Someone watching the mempool can deploy at this deployment's contract IDs
//! first, or front-run a contract's `init`, and so choose its owner and what
//! it is bound to (docs/testnet.md, Before mainnet). So once every contract
//! is deployed, `deploy-hyperlane` reads each one back: the owner Rusk
//! records for it, its `owner()`, each binding its `init` set and its
//! `state_version`, against what it sent. When any of them differs, or
//! can't be read, it reports none of the contracts as deployed.
//!
//! Some of what `init` set has no view that lists it. A route has none that
//! lists the routers it enrolled, and the IGP none that lists the domains it
//! prices. The read-back asks each route for its router on the local domain
//! and on every domain the IGP was priced for, which must have none, and the
//! IGP for the pricing it was sent. The canonical DRC20 a collateral route
//! can wrap has no view that names its admin: the read-back asks it whether
//! the deployer holds each role `init` gives the admin, and checks that the
//! deployer holds the whole supply `init` minted, so that nobody else holds
//! any of it.
//!
//! `deploy-token-route` reads back the canonical DRC20 and the collateral
//! route it deploys for an existing Mailbox ([`TokenRoute`]) the same way.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use dusk_core::abi::ContractId;
use hyperlane_dusk_types::{DomainGasConfig, EthAddress, H256};

use crate::rues::{rkyv_deserialize, rkyv_serialize, RuesClient};

/// The `state_version` each contract of this checkout reports, by its crate
/// under `contracts/`. `scripts/test-deploy-scripts.py` checks them against
/// the contracts' sources.
pub const STATE_VERSIONS: [(&str, u32); 12] = [
    ("test-mock", 1),
    ("test-recipient", 1),
    ("merkle-tree-hook", 2),
    ("ism-multisig", 1),
    ("protocol-fee", 1),
    ("aggregation-hook", 1),
    ("mailbox", 6),
    ("validator-announce", 3),
    ("igp", 4),
    ("warp-drc20", 8),
    ("warp-native", 5),
    ("warp-drc20-collateral", 7),
];

/// Module types the ISMs report (Hyperlane's `IsmType`).
const MESSAGE_ID_MULTISIG: u8 = 5;
const NULL_ISM: u8 = 6;

/// How often a read is tried before it counts as failed, and how long to
/// wait between tries: a node that is briefly unavailable shouldn't make a
/// finished deployment unusable.
const READ_ATTEMPTS: usize = 3;
const READ_RETRY_DELAY: Duration = Duration::from_secs(1);

/// What `deploy-hyperlane` sent: the contracts it deployed and what each
/// `init` was given.
pub struct Sent {
    /// The deployer's public key, which Rusk records as every contract's
    /// owner.
    pub owner_key: Vec<u8>,
    /// `keccak256` of it: the owner each `init` was given, and the fee
    /// contracts' beneficiary.
    pub owner: H256,
    /// The local domain.
    pub domain: u32,
    pub test_mock: ContractId,
    pub test_recipient: ContractId,
    pub merkle_tree_hook: ContractId,
    /// The MessageIdMultisig ISM, with its validators in the order sent
    /// (ascending) and its threshold; `None` when the Mailbox's default ISM
    /// is TestMock.
    pub ism: Option<(ContractId, Vec<EthAddress>, u8)>,
    /// The ProtocolFee, with its fee and its maximum fee.
    pub protocol_fee: (ContractId, u64, u64),
    pub aggregation_hook: ContractId,
    pub mailbox: ContractId,
    pub validator_announce: ContractId,
    /// The IGP, with the pricing it was given for each domain.
    pub igp: (ContractId, Vec<(u32, DomainGasConfig)>),
    /// The synthetic route, with its name, symbol and decimals.
    pub warp_drc20: Option<(ContractId, String, String, u8)>,
    pub warp_native: Option<ContractId>,
    /// The canonical DRC20 deployed for the collateral route to wrap.
    pub canonical_token: Option<CanonicalToken>,
    /// The collateral route, with the token it locks.
    pub warp_collateral: Option<(ContractId, ContractId)>,
}

/// A canonical DRC20 deployed for a collateral route to lock: the deployer
/// its admin, with every role, and the holder of its whole supply, and no
/// cap.
pub struct CanonicalToken {
    pub id: ContractId,
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    /// The `balance_of` argument that names the deployer.
    pub holder: Vec<u8>,
    /// What its `init` minted to the deployer.
    pub supply: u64,
    /// The `has_role` argument for each role the deployer must hold, and
    /// the role's name.
    pub roles: Vec<(&'static str, Vec<u8>)>,
}

/// What `deploy-token-route` deploys for an existing Mailbox: a canonical
/// DRC20, and a collateral route that locks it.
pub struct TokenRoute {
    /// The deployer's public key, which Rusk records as both contracts'
    /// owner.
    pub owner_key: Vec<u8>,
    /// `keccak256` of it: the route's owner.
    pub owner: H256,
    pub mailbox: ContractId,
    /// The domains the route must have no router for: the Mailbox's own,
    /// and the remote domains the command was given.
    pub router_domains: Vec<u32>,
    pub token: CanonicalToken,
    pub route: ContractId,
}

/// What a read-back reads: the contracts a command deployed, each owned by
/// the deployer, and the views each must answer.
pub trait Deployed {
    /// The deployer's public key, which Rusk records as every contract's
    /// owner.
    fn owner_key(&self) -> &[u8];
    /// Every contract deployed, by name.
    fn contracts(&self) -> Vec<(&'static str, ContractId)>;
    /// Every view to read, with the value an honest deployment returns.
    fn checks(&self) -> Vec<Check>;
    /// The command that deployed them, and what to do when they don't read
    /// back.
    fn command(&self) -> &'static str;
    fn remedy(&self) -> &'static str;
}

impl Deployed for Sent {
    fn owner_key(&self) -> &[u8] {
        &self.owner_key
    }

    fn contracts(&self) -> Vec<(&'static str, ContractId)> {
        Sent::contracts(self)
    }

    fn checks(&self) -> Vec<Check> {
        checks(self)
    }

    fn command(&self) -> &'static str {
        "deploy-hyperlane"
    }

    fn remedy(&self) -> &'static str {
        "Deploy again with another deployer key."
    }
}

impl Deployed for TokenRoute {
    fn owner_key(&self) -> &[u8] {
        &self.owner_key
    }

    fn contracts(&self) -> Vec<(&'static str, ContractId)> {
        vec![("Token", self.token.id), ("WarpCollateral", self.route)]
    }

    fn checks(&self) -> Vec<Check> {
        const NONE: [u8; 32] = [0u8; 32];
        let route = self.route;
        let mut checks = token_checks("Token", &self.token);
        let views = [
            (
                "state_version",
                Value::U32(state_version("warp-drc20-collateral")),
            ),
            ("owner", Value::Owner(Some(self.owner))),
            ("mailbox", Value::Bytes32(self.mailbox.to_bytes())),
            ("wrapped_token", Value::Bytes32(self.token.id.to_bytes())),
            ("hook", Value::Bytes32(NONE)),
            ("interchain_security_module", Value::Bytes32(NONE)),
            ("pending_total", Value::U64(0)),
        ];
        checks.extend(views.into_iter().map(|(method, expected)| Check {
            name: "WarpCollateral",
            contract: route.to_bytes(),
            method,
            args: rkyv_serialize(&()),
            shown_args: String::new(),
            expected,
        }));
        for domain in &self.router_domains {
            checks.push(Check {
                name: "WarpCollateral",
                contract: route.to_bytes(),
                method: "enrolled_router",
                args: rkyv_serialize(domain),
                shown_args: format!("({domain})"),
                expected: Value::Bytes32(NONE),
            });
        }
        checks
    }

    fn command(&self) -> &'static str {
        "deploy-token-route"
    }

    fn remedy(&self) -> &'static str {
        "Deploy the route again at other deploy nonces."
    }
}

/// What a canonical DRC20 must answer: its name, symbol and decimals, its
/// whole supply held by the deployer, each of the admin's roles held by the
/// deployer, no cap, and not paused.
fn token_checks(name: &'static str, token: &CanonicalToken) -> Vec<Check> {
    let id = token.id.to_bytes();
    let check = |method, args: Vec<u8>, shown_args: &str, expected| Check {
        name,
        contract: id,
        method,
        args,
        shown_args: shown_args.to_string(),
        expected,
    };
    let none = || rkyv_serialize(&());
    let mut checks = vec![
        check("name", none(), "", Value::Text(token.name.clone())),
        check("symbol", none(), "", Value::Text(token.symbol.clone())),
        check("decimals", none(), "", Value::U8(token.decimals)),
        check("total_supply", none(), "", Value::U64(token.supply)),
        check(
            "balance_of",
            token.holder.clone(),
            "(the deployer)",
            Value::U64(token.supply),
        ),
        check("cap", none(), "", Value::U64(0)),
        check("paused", none(), "", Value::Bool(false)),
    ];
    for (role, args) in &token.roles {
        checks.push(check(
            "has_role",
            args.clone(),
            &format!("({role}, the deployer)"),
            Value::Bool(true),
        ));
    }
    checks
}

impl Sent {
    /// Every contract deployed, by name.
    pub fn contracts(&self) -> Vec<(&'static str, ContractId)> {
        let mut contracts = vec![
            ("TestMock", self.test_mock),
            ("TestRecipient", self.test_recipient),
            ("MerkleTreeHook", self.merkle_tree_hook),
        ];
        contracts.extend(self.ism.as_ref().map(|(id, ..)| ("IsmMultisig", *id)));
        contracts.extend([
            ("ProtocolFee", self.protocol_fee.0),
            ("AggregationHook", self.aggregation_hook),
            ("Mailbox", self.mailbox),
            ("ValidatorAnnounce", self.validator_announce),
            ("IGP", self.igp.0),
        ]);
        contracts.extend(self.warp_drc20.as_ref().map(|(id, ..)| ("WarpDrc20", *id)));
        contracts.extend(self.warp_native.map(|id| ("WarpNative", id)));
        contracts.extend(
            self.canonical_token
                .as_ref()
                .map(|token| ("CollateralToken", token.id)),
        );
        contracts.extend(self.warp_collateral.map(|(id, _)| ("WarpCollateral", id)));
        contracts
    }

    /// The domains a route must have no router for: the local one and every
    /// one the IGP was priced for.
    fn router_domains(&self) -> Vec<u32> {
        let mut domains = vec![self.domain];
        for (domain, _) in &self.igp.1 {
            if !domains.contains(domain) {
                domains.push(*domain);
            }
        }
        domains
    }
}

/// A value a view returns, as the contract archives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A `ContractId` or another 32 bytes.
    Bytes32([u8; 32]),
    /// `owner()`: an account hash, or none.
    Owner(Option<H256>),
    U8(u8),
    U32(u32),
    U64(u64),
    Bool(bool),
    /// A list of contract IDs.
    Ids(Vec<[u8; 32]>),
    /// `validators_and_threshold()`.
    Validators(Vec<EthAddress>, u8),
    /// `domain_gas_config(domain)`.
    Pricing(DomainGasConfig),
    Text(String),
}

impl Value {
    /// A reply decoded as the type of `self`.
    fn decode_like(&self, bytes: &[u8]) -> Result<Self, String> {
        Ok(match self {
            Self::Bytes32(_) => Self::Bytes32(rkyv_deserialize(bytes)?),
            Self::Owner(_) => Self::Owner(rkyv_deserialize(bytes)?),
            Self::U8(_) => Self::U8(rkyv_deserialize(bytes)?),
            Self::U32(_) => Self::U32(rkyv_deserialize(bytes)?),
            Self::U64(_) => Self::U64(rkyv_deserialize(bytes)?),
            Self::Bool(_) => Self::Bool(rkyv_deserialize(bytes)?),
            Self::Ids(_) => Self::Ids(
                rkyv_deserialize::<Vec<ContractId>>(bytes)?
                    .into_iter()
                    .map(ContractId::to_bytes)
                    .collect(),
            ),
            Self::Validators(..) => {
                let (validators, threshold) = rkyv_deserialize::<(Vec<EthAddress>, u8)>(bytes)?;
                Self::Validators(validators, threshold)
            }
            Self::Pricing(_) => Self::Pricing(rkyv_deserialize(bytes)?),
            Self::Text(_) => Self::Text(rkyv_deserialize(bytes)?),
        })
    }

    /// The reply a contract holding this value gives.
    #[cfg(test)]
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Bytes32(value) => rkyv_serialize(value),
            Self::Owner(value) => rkyv_serialize(value),
            Self::U8(value) => rkyv_serialize(value),
            Self::U32(value) => rkyv_serialize(value),
            Self::U64(value) => rkyv_serialize(value),
            Self::Bool(value) => rkyv_serialize(value),
            Self::Ids(ids) => rkyv_serialize(
                &ids.iter()
                    .map(|id| ContractId::from_bytes(*id))
                    .collect::<Vec<_>>(),
            ),
            Self::Validators(validators, threshold) => {
                rkyv_serialize(&(validators.clone(), *threshold))
            }
            Self::Pricing(value) => rkyv_serialize(value),
            Self::Text(value) => rkyv_serialize(value),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bytes32(value) => f.write_str(&hex::encode(value)),
            Self::Owner(Some(owner)) => write!(f, "the account with hash {}", hex::encode(owner)),
            Self::Owner(None) => f.write_str("none"),
            Self::U8(value) => write!(f, "{value}"),
            Self::U32(value) => write!(f, "{value}"),
            Self::U64(value) => write!(f, "{value}"),
            Self::Bool(value) => write!(f, "{value}"),
            Self::Ids(ids) => write!(
                f,
                "[{}]",
                ids.iter().map(hex::encode).collect::<Vec<_>>().join(", ")
            ),
            Self::Validators(validators, threshold) => write!(
                f,
                "{threshold} of [{}]",
                validators
                    .iter()
                    .map(|address| format!("0x{}", hex::encode(address.0)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Pricing(config) => write!(
                f,
                "gas overhead {}, token exchange rate {}, gas price {}",
                config.gas_overhead, config.token_exchange_rate, config.gas_price
            ),
            Self::Text(text) => write!(f, "{text:?}"),
        }
    }
}

/// One value to read back: what `method` of `contract` must return.
#[derive(Debug, Clone)]
pub struct Check {
    /// The contract's name, as `deploy-hyperlane` prints it.
    pub name: &'static str,
    pub contract: [u8; 32],
    pub method: &'static str,
    /// The query's argument, as the contract decodes it.
    pub args: Vec<u8>,
    /// The argument as it reads, for the report: `(11155111)`, or nothing.
    pub shown_args: String,
    pub expected: Value,
}

/// Every view the read-back reads, with the value an honest deployment of
/// `sent` returns. The owner Rusk records is read besides these.
pub fn checks(sent: &Sent) -> Vec<Check> {
    const NONE: [u8; 32] = [0u8; 32];
    let owner = Value::Owner(Some(sent.owner));
    let id = |contract: ContractId| Value::Bytes32(contract.to_bytes());
    let version = |name: &str| Value::U32(state_version(name));
    let (protocol_fee, fee, max_fee) = sent.protocol_fee;
    let (igp, pricing) = &sent.igp;
    let default_ism = sent.ism.as_ref().map_or(sent.test_mock, |(ism, ..)| *ism);

    // (contract name, contract, view, value), for views without an argument.
    let mut views: Vec<(&'static str, ContractId, &'static str, Value)> = vec![
        (
            "TestMock",
            sent.test_mock,
            "state_version",
            version("test-mock"),
        ),
        (
            "TestMock",
            sent.test_mock,
            "module_type",
            Value::U8(NULL_ISM),
        ),
        (
            "TestRecipient",
            sent.test_recipient,
            "state_version",
            version("test-recipient"),
        ),
        (
            "TestRecipient",
            sent.test_recipient,
            "interchain_security_module",
            Value::Bytes32(NONE),
        ),
        (
            "MerkleTreeHook",
            sent.merkle_tree_hook,
            "state_version",
            version("merkle-tree-hook"),
        ),
        (
            "MerkleTreeHook",
            sent.merkle_tree_hook,
            "mailbox",
            id(sent.aggregation_hook),
        ),
    ];
    if let Some((ism, validators, threshold)) = &sent.ism {
        views.extend([
            (
                "IsmMultisig",
                *ism,
                "state_version",
                version("ism-multisig"),
            ),
            (
                "IsmMultisig",
                *ism,
                "module_type",
                Value::U8(MESSAGE_ID_MULTISIG),
            ),
            ("IsmMultisig", *ism, "owner", owner.clone()),
            (
                "IsmMultisig",
                *ism,
                "validators_and_threshold",
                Value::Validators(validators.clone(), *threshold),
            ),
        ]);
    }
    views.extend([
        (
            "ProtocolFee",
            protocol_fee,
            "state_version",
            version("protocol-fee"),
        ),
        ("ProtocolFee", protocol_fee, "owner", owner.clone()),
        (
            "ProtocolFee",
            protocol_fee,
            "mailbox",
            id(sent.aggregation_hook),
        ),
        (
            "ProtocolFee",
            protocol_fee,
            "beneficiary",
            Value::Bytes32(sent.owner),
        ),
        ("ProtocolFee", protocol_fee, "protocol_fee", Value::U64(fee)),
        (
            "ProtocolFee",
            protocol_fee,
            "max_protocol_fee",
            Value::U64(max_fee),
        ),
        (
            "AggregationHook",
            sent.aggregation_hook,
            "state_version",
            version("aggregation-hook"),
        ),
        (
            "AggregationHook",
            sent.aggregation_hook,
            "mailbox",
            id(sent.mailbox),
        ),
        (
            "AggregationHook",
            sent.aggregation_hook,
            "hooks",
            Value::Ids(vec![
                sent.merkle_tree_hook.to_bytes(),
                protocol_fee.to_bytes(),
            ]),
        ),
        ("Mailbox", sent.mailbox, "state_version", version("mailbox")),
        ("Mailbox", sent.mailbox, "owner", owner.clone()),
        (
            "Mailbox",
            sent.mailbox,
            "local_domain",
            Value::U32(sent.domain),
        ),
        ("Mailbox", sent.mailbox, "default_ism", id(default_ism)),
        ("Mailbox", sent.mailbox, "default_hook", id(*igp)),
        (
            "Mailbox",
            sent.mailbox,
            "required_hook",
            id(sent.aggregation_hook),
        ),
        (
            "ValidatorAnnounce",
            sent.validator_announce,
            "state_version",
            version("validator-announce"),
        ),
        (
            "ValidatorAnnounce",
            sent.validator_announce,
            "mailbox",
            id(sent.mailbox),
        ),
        (
            "ValidatorAnnounce",
            sent.validator_announce,
            "local_domain",
            Value::U32(sent.domain),
        ),
        ("IGP", *igp, "state_version", version("igp")),
        ("IGP", *igp, "owner", owner.clone()),
        ("IGP", *igp, "mailbox", id(sent.mailbox)),
        ("IGP", *igp, "beneficiary", Value::Bytes32(sent.owner)),
    ]);

    // (route name, route, its crate), each read as the routes share.
    let mut routes: Vec<(&'static str, ContractId, &'static str)> = Vec::new();
    if let Some((route, name, symbol, decimals)) = &sent.warp_drc20 {
        routes.push(("WarpDrc20", *route, "warp-drc20"));
        views.extend([
            ("WarpDrc20", *route, "name", Value::Text(name.clone())),
            ("WarpDrc20", *route, "symbol", Value::Text(symbol.clone())),
            ("WarpDrc20", *route, "decimals", Value::U8(*decimals)),
        ]);
    }
    if let Some(route) = sent.warp_native {
        routes.push(("WarpNative", route, "warp-native"));
    }
    if let Some((route, token)) = sent.warp_collateral {
        routes.push(("WarpCollateral", route, "warp-drc20-collateral"));
        views.push(("WarpCollateral", route, "wrapped_token", id(token)));
    }
    for &(name, route, kind) in &routes {
        views.extend([
            (name, route, "state_version", version(kind)),
            (name, route, "owner", owner.clone()),
            (name, route, "mailbox", id(sent.mailbox)),
            (name, route, "hook", Value::Bytes32(NONE)),
            (
                name,
                route,
                "interchain_security_module",
                Value::Bytes32(NONE),
            ),
        ]);
    }

    let mut checks: Vec<Check> = views
        .into_iter()
        .map(|(name, contract, method, expected)| Check {
            name,
            contract: contract.to_bytes(),
            method,
            args: rkyv_serialize(&()),
            shown_args: String::new(),
            expected,
        })
        .collect();
    let with_domain = |name, contract: ContractId, method, domain: u32, expected| Check {
        name,
        contract: contract.to_bytes(),
        method,
        args: rkyv_serialize(&domain),
        shown_args: format!("({domain})"),
        expected,
    };
    for (domain, config) in pricing {
        checks.push(with_domain(
            "IGP",
            *igp,
            "domain_gas_config",
            *domain,
            Value::Pricing(*config),
        ));
    }
    if let Some(token) = &sent.canonical_token {
        checks.extend(token_checks("CollateralToken", token));
    }
    for &(name, route, _) in &routes {
        for domain in sent.router_domains() {
            checks.push(with_domain(
                name,
                route,
                "enrolled_router",
                domain,
                Value::Bytes32(NONE),
            ));
        }
    }
    checks
}

/// The `state_version` of the contract crate `name`.
fn state_version(name: &str) -> u32 {
    STATE_VERSIONS
        .iter()
        .find(|(crate_name, _)| *crate_name == name)
        .map(|(_, version)| *version)
        .expect("every deployed contract has a state version")
}

/// What the read-back asks the node.
pub trait Node {
    /// The reply of a contract query: the archived value the method
    /// returned.
    fn query(
        &self,
        contract: &[u8; 32],
        method: &str,
        args: &[u8],
    ) -> impl Future<Output = Result<Vec<u8>, String>>;

    /// The owner Rusk records for a contract, or `None` when no contract
    /// has the ID.
    fn contract_owner(
        &self,
        contract: &[u8; 32],
    ) -> impl Future<Output = Result<Option<Vec<u8>>, String>>;
}

impl Node for RuesClient {
    async fn query(
        &self,
        contract: &[u8; 32],
        method: &str,
        args: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.contract_query_raw(contract, method, args).await
    }

    async fn contract_owner(&self, contract: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        RuesClient::contract_owner(self, &hex::encode(contract)).await
    }
}

/// `read`, tried up to `READ_ATTEMPTS` times while it fails.
async fn with_retries<T, F, Fut>(mut read: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    let mut attempt = 1;
    loop {
        match read().await {
            Ok(value) => return Ok(value),
            Err(error) if attempt >= READ_ATTEMPTS => return Err(error),
            Err(_) => {
                attempt += 1;
                if !cfg!(test) {
                    tokio::time::sleep(READ_RETRY_DELAY).await;
                }
            }
        }
    }
}

/// Read back every contract of `sent` and compare it with what was sent.
/// Returns how many values were read, or every difference and every value
/// that couldn't be read.
pub async fn read_back(node: &impl Node, sent: &impl Deployed) -> Result<usize, String> {
    let mut problems = Vec::new();
    let mut read = 0;
    for (name, contract) in sent.contracts() {
        let id = contract.to_bytes();
        let label = format!("{name} {}", hex::encode(id));
        match with_retries(|| node.contract_owner(&id)).await {
            Ok(Some(owner)) if owner == sent.owner_key() => read += 1,
            Ok(Some(owner)) => problems.push(format!(
                "{label}: Rusk records its owner as {}, not the deployer {}",
                crate::describe_owner(&owner),
                bs58::encode(sent.owner_key()).into_string()
            )),
            Ok(None) => problems.push(format!("{label}: there is no contract at this ID")),
            Err(error) => problems.push(format!(
                "{label}: can't read the owner Rusk records for it: {error}"
            )),
        }
    }
    for check in sent.checks() {
        let label = format!("{} {}", check.name, hex::encode(check.contract));
        let view = format!("{}{}", check.method, check.shown_args);
        let found = with_retries(|| node.query(&check.contract, check.method, &check.args))
            .await
            .and_then(|reply| check.expected.decode_like(&reply));
        match found {
            Ok(found) if found == check.expected => read += 1,
            Ok(found) => problems.push(format!(
                "{label}: {view} is {found}, not {}",
                check.expected
            )),
            Err(error) => problems.push(format!("{label}: can't read {view}: {error}")),
        }
    }
    if problems.is_empty() {
        return Ok(read);
    }
    Err(format!(
        "The contracts were deployed, but they don't all hold what {} sent, so it reports \
         none of them as deployed:\n  - {}\nA deploy or a contract's init may have been \
         front-run: whoever deployed or initialized a contract first chose its owner and its \
         bindings. Don't use these contracts. {}",
        sent.command(),
        problems.join("\n  - "),
        sent.remedy()
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// A query: the contract, the method and the archived argument.
    type Query = ([u8; 32], String, Vec<u8>);

    /// A node that answers from what it was given: replies by query, and
    /// owners by contract.
    #[derive(Default, Clone)]
    pub struct FakeNode {
        pub replies: HashMap<Query, Result<Vec<u8>, String>>,
        pub owners: HashMap<[u8; 32], Result<Option<Vec<u8>>, String>>,
    }

    impl FakeNode {
        /// A node with an honest deployment of `sent`.
        pub fn honest(sent: &impl Deployed) -> Self {
            let mut node = Self::default();
            for (_, contract) in sent.contracts() {
                node.owners
                    .insert(contract.to_bytes(), Ok(Some(sent.owner_key().to_vec())));
            }
            for check in sent.checks() {
                node.answer(&check, Ok(check.expected.encode()));
            }
            node
        }

        pub fn answer(&mut self, check: &Check, reply: Result<Vec<u8>, String>) {
            self.replies.insert(
                (check.contract, check.method.to_string(), check.args.clone()),
                reply,
            );
        }
    }

    impl Node for FakeNode {
        async fn query(
            &self,
            contract: &[u8; 32],
            method: &str,
            args: &[u8],
        ) -> Result<Vec<u8>, String> {
            self.replies
                .get(&(*contract, method.to_string(), args.to_vec()))
                .cloned()
                .unwrap_or_else(|| Err(format!("Query {method} failed (500): no such function")))
        }

        async fn contract_owner(&self, contract: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
            self.owners.get(contract).cloned().unwrap_or(Ok(None))
        }
    }

    fn contract(byte: u8) -> ContractId {
        ContractId::from_bytes([byte; 32])
    }

    fn pricing(gas_price: u64) -> DomainGasConfig {
        DomainGasConfig {
            gas_overhead: 110_000,
            token_exchange_rate: 375_000,
            gas_price,
        }
    }

    /// What `deploy/testnet.sh` deploys: a MessageIdMultisig ISM of two
    /// validators and the native route.
    pub fn testnet() -> Sent {
        Sent {
            owner_key: vec![7; 96],
            owner: [0xd0; 32],
            domain: 1_146_442_572,
            test_mock: contract(0xc1),
            test_recipient: contract(0xc2),
            merkle_tree_hook: contract(0xa4),
            ism: Some((
                contract(0xb1),
                vec![EthAddress([0x11; 20]), EthAddress([0x22; 20])],
                2,
            )),
            protocol_fee: (contract(0xa5), 1_000_000, 100_000_000),
            aggregation_hook: contract(0xa6),
            mailbox: contract(0xa1),
            validator_announce: contract(0xa3),
            igp: (contract(0xa2), vec![(11_155_111, pricing(1_000_000_000))]),
            warp_drc20: None,
            warp_native: Some(contract(0xe1)),
            canonical_token: None,
            warp_collateral: None,
        }
    }

    /// A canonical DRC20 at `id`, with the deployer's arguments stood in by
    /// bytes of their own.
    fn canonical_token(id: ContractId, symbol: &str, decimals: u8, supply: u64) -> CanonicalToken {
        CanonicalToken {
            id,
            name: format!("{symbol} token"),
            symbol: symbol.to_string(),
            decimals,
            holder: vec![7; 12],
            supply,
            roles: vec![
                ("admin", vec![8; 12]),
                ("minter", vec![9; 12]),
                ("pauser", vec![10; 12]),
            ],
        }
    }

    /// What `demo/deploy.sh` deploys with TestMock: every route, the
    /// collateral one locking the canonical DRC20 deployed with it.
    fn demo() -> Sent {
        Sent {
            ism: None,
            igp: (
                contract(0xa2),
                vec![(31_337, pricing(1)), (31_338, pricing(2))],
            ),
            warp_drc20: Some((contract(0xe2), "Wrapped Ether".into(), "WETH".into(), 18)),
            canonical_token: Some(canonical_token(
                contract(0xe4),
                "CDRC20",
                18,
                5_000_000_000_000_000_000,
            )),
            warp_collateral: Some((contract(0xe3), contract(0xe4))),
            ..testnet()
        }
    }

    /// What `deploy/testnet.sh` deploys for a token route: EURd and its
    /// collateral route, bound to the testnet deployment's Mailbox.
    fn token_route() -> TokenRoute {
        let testnet = testnet();
        TokenRoute {
            owner_key: testnet.owner_key,
            owner: testnet.owner,
            mailbox: testnet.mailbox,
            router_domains: vec![1_146_442_572, 11_155_111],
            token: canonical_token(contract(0xf1), "EURd", 6, 1_000_000_000_000),
            route: contract(0xf2),
        }
    }

    /// A different value of the same type.
    fn other(value: &Value) -> Value {
        match value {
            Value::Bytes32(bytes) => Value::Bytes32({
                let mut other = *bytes;
                other[31] ^= 1;
                other
            }),
            Value::Owner(Some(owner)) => Value::Owner(Some({
                let mut other = *owner;
                other[0] ^= 1;
                other
            })),
            Value::Owner(None) => Value::Owner(Some([1; 32])),
            Value::U8(value) => Value::U8(value.wrapping_add(1)),
            Value::U32(value) => Value::U32(value.wrapping_add(1)),
            Value::U64(value) => Value::U64(value.wrapping_add(1)),
            Value::Bool(value) => Value::Bool(!value),
            Value::Ids(ids) => Value::Ids(ids.iter().rev().cloned().collect()),
            Value::Validators(validators, threshold) => {
                Value::Validators(validators.clone(), threshold.wrapping_sub(1))
            }
            Value::Pricing(config) => Value::Pricing(DomainGasConfig {
                gas_price: config.gas_price + 1,
                ..*config
            }),
            Value::Text(text) => Value::Text(format!("{text}!")),
        }
    }

    #[tokio::test]
    async fn an_honest_deployment_reads_back_whole() {
        for sent in [testnet(), demo()] {
            let checks = checks(&sent);
            let read = read_back(&FakeNode::honest(&sent), &sent).await.unwrap();
            assert_eq!(read, sent.contracts().len() + checks.len());
        }
        // Every contract is read: its state version, and each owner and
        // binding its init set.
        let views: Vec<String> = checks(&testnet())
            .iter()
            .map(|check| format!("{} {}{}", check.name, check.method, check.shown_args))
            .collect();
        for expected in [
            "TestMock state_version",
            "TestRecipient interchain_security_module",
            "MerkleTreeHook mailbox",
            "IsmMultisig owner",
            "IsmMultisig validators_and_threshold",
            "ProtocolFee beneficiary",
            "ProtocolFee max_protocol_fee",
            "AggregationHook hooks",
            "Mailbox owner",
            "Mailbox default_ism",
            "Mailbox default_hook",
            "Mailbox required_hook",
            "ValidatorAnnounce mailbox",
            "ValidatorAnnounce local_domain",
            "IGP owner",
            "IGP beneficiary",
            "IGP domain_gas_config(11155111)",
            "WarpNative owner",
            "WarpNative mailbox",
            "WarpNative hook",
            "WarpNative interchain_security_module",
            "WarpNative enrolled_router(1146442572)",
            "WarpNative enrolled_router(11155111)",
        ] {
            assert!(
                views.contains(&expected.to_string()),
                "{expected}: {views:?}"
            );
        }
        let demo = checks(&demo());
        assert!(demo.iter().any(|check| check.name == "CollateralToken"
            && check.method == "balance_of"
            && check.expected == Value::U64(5_000_000_000_000_000_000)));
        // The canonical DRC20: everything init set, and the deployer holds
        // each of the admin's roles.
        for (method, shown_args, expected) in [
            ("name", "", Value::Text("CDRC20 token".into())),
            ("symbol", "", Value::Text("CDRC20".into())),
            ("decimals", "", Value::U8(18)),
            ("total_supply", "", Value::U64(5_000_000_000_000_000_000)),
            ("cap", "", Value::U64(0)),
            ("paused", "", Value::Bool(false)),
            ("has_role", "(admin, the deployer)", Value::Bool(true)),
            ("has_role", "(minter, the deployer)", Value::Bool(true)),
            ("has_role", "(pauser, the deployer)", Value::Bool(true)),
        ] {
            assert!(
                demo.iter().any(|check| check.name == "CollateralToken"
                    && check.method == method
                    && check.shown_args == shown_args
                    && check.expected == expected),
                "{method}{shown_args}"
            );
        }
        assert!(demo.iter().any(|check| check.name == "WarpCollateral"
            && check.method == "wrapped_token"
            && check.expected == Value::Bytes32([0xe4; 32])));
        for route in ["WarpDrc20", "WarpNative", "WarpCollateral"] {
            for domain in [1_146_442_572, 31_337, 31_338] {
                assert!(demo
                    .iter()
                    .any(|check| check.name == route && check.shown_args == format!("({domain})")));
            }
        }
    }

    /// A token route reads back whole: the token's settings, supply and
    /// roles, and the route's owner and bindings, with no router for the
    /// Mailbox's domain or the remote ones.
    #[tokio::test]
    async fn an_honest_token_route_reads_back_whole() {
        let sent = token_route();
        let checks = sent.checks();
        let read = read_back(&FakeNode::honest(&sent), &sent).await.unwrap();
        assert_eq!(read, 2 + checks.len());
        let views: Vec<String> = checks
            .iter()
            .map(|check| format!("{} {}{}", check.name, check.method, check.shown_args))
            .collect();
        for expected in [
            "Token name",
            "Token symbol",
            "Token decimals",
            "Token total_supply",
            "Token balance_of(the deployer)",
            "Token cap",
            "Token paused",
            "Token has_role(admin, the deployer)",
            "Token has_role(minter, the deployer)",
            "Token has_role(pauser, the deployer)",
            "WarpCollateral state_version",
            "WarpCollateral owner",
            "WarpCollateral mailbox",
            "WarpCollateral wrapped_token",
            "WarpCollateral hook",
            "WarpCollateral interchain_security_module",
            "WarpCollateral pending_total",
            "WarpCollateral enrolled_router(1146442572)",
            "WarpCollateral enrolled_router(11155111)",
        ] {
            assert!(
                views.contains(&expected.to_string()),
                "{expected}: {views:?}"
            );
        }
        assert!(checks
            .iter()
            .any(|check| check.method == "wrapped_token"
                && check.expected == Value::Bytes32([0xf1; 32])));
        assert!(
            checks
                .iter()
                .any(|check| check.method == "mailbox"
                    && check.expected == Value::Bytes32([0xa1; 32]))
        );
    }

    /// A token route whose token someone else initialized, keeping the
    /// settings but making itself the admin, or minting some of the supply
    /// to itself, is caught, and so is a route bound to another Mailbox or
    /// that came with a router; each value counts, and so does each owner
    /// Rusk records.
    #[tokio::test]
    async fn a_token_route_must_read_back_as_sent() {
        let sent = token_route();
        for check in sent.checks() {
            let mut node = FakeNode::honest(&sent);
            node.answer(&check, Ok(other(&check.expected).encode()));
            let error = read_back(&node, &sent).await.unwrap_err();
            let view = format!(
                "{} {}: {}{} is {}, not {}",
                check.name,
                hex::encode(check.contract),
                check.method,
                check.shown_args,
                other(&check.expected),
                check.expected
            );
            assert!(error.contains(&view), "{view}: {error}");
            assert!(error.contains("what deploy-token-route sent"), "{error}");
            assert!(error.contains("at other deploy nonces"), "{error}");
            assert_eq!(error.matches("\n  - ").count(), 1, "{error}");
        }
        for (name, contract) in sent.contracts() {
            let mut node = FakeNode::honest(&sent);
            node.owners.insert(contract.to_bytes(), Ok(None));
            let error = read_back(&node, &sent).await.unwrap_err();
            assert!(
                error.contains(&format!(
                    "{name} {}: there is no contract at this ID",
                    hex::encode(contract.to_bytes())
                )),
                "{error}"
            );
        }
    }

    /// Each value read back counts: one that differs from what was sent, or
    /// can't be read or decoded, stops the deployment and is named.
    #[tokio::test]
    async fn every_value_must_read_back_as_sent() {
        for sent in [testnet(), demo()] {
            for check in checks(&sent) {
                let view = format!(
                    "{} {}: {}{} is ",
                    check.name,
                    hex::encode(check.contract),
                    check.method,
                    check.shown_args
                );
                let unreadable = format!(
                    "{} {}: can't read {}{}",
                    check.name,
                    hex::encode(check.contract),
                    check.method,
                    check.shown_args
                );
                // An empty reply is too short for any of the types.
                for (reply, expected) in [
                    (Ok(other(&check.expected).encode()), view.clone()),
                    (
                        Err("Query failed (500): busy".to_string()),
                        unreadable.clone(),
                    ),
                    (Ok(Vec::new()), unreadable.clone()),
                ] {
                    let mut node = FakeNode::honest(&sent);
                    node.answer(&check, reply);
                    let error = read_back(&node, &sent).await.unwrap_err();
                    assert!(error.contains(&expected), "{expected}: {error}");
                    assert!(
                        error.contains("reports none of them as deployed"),
                        "{error}"
                    );
                    assert_eq!(error.matches("\n  - ").count(), 1, "{error}");
                }
            }
        }
    }

    /// Rusk must record the deployer as every contract's owner.
    #[tokio::test]
    async fn every_contract_must_be_the_deployers() {
        let sent = demo();
        for (name, contract) in sent.contracts() {
            let label = format!("{name} {}", hex::encode(contract.to_bytes()));
            for (owner, expected) in [
                (Ok(Some(vec![8; 96])), "Rusk records its owner as"),
                (Ok(None), "there is no contract at this ID"),
                (Err("busy".to_string()), "can't read the owner Rusk records"),
            ] {
                let mut node = FakeNode::honest(&sent);
                node.owners.insert(contract.to_bytes(), owner);
                let error = read_back(&node, &sent).await.unwrap_err();
                assert!(
                    error.contains(&format!("{label}: {expected}")),
                    "{label}: {error}"
                );
            }
        }
    }

    /// A front-run init that named someone else as a contract's owner, with
    /// every other argument as sent, is caught, and so is one that kept the
    /// owner and enrolled a router.
    #[tokio::test]
    async fn a_front_run_init_is_caught() {
        let sent = testnet();
        let attacker = [0xee; 32];
        let mut node = FakeNode::honest(&sent);
        for check in checks(&sent) {
            if check.name == "Mailbox" && check.method == "owner" {
                node.answer(&check, Ok(Value::Owner(Some(attacker)).encode()));
            }
            if check.name == "WarpNative" && check.shown_args == "(11155111)" {
                node.answer(&check, Ok(Value::Bytes32(attacker).encode()));
            }
        }
        let error = read_back(&node, &sent).await.unwrap_err();
        assert!(
            error.contains(&format!(
                "Mailbox {}: owner is the account with hash {}, not the account with hash {}",
                hex::encode([0xa1; 32]),
                hex::encode(attacker),
                hex::encode([0xd0; 32])
            )),
            "{error}"
        );
        assert!(
            error.contains(&format!(
                "WarpNative {}: enrolled_router(11155111) is {}, not {}",
                hex::encode([0xe1; 32]),
                hex::encode(attacker),
                hex::encode([0u8; 32])
            )),
            "{error}"
        );
    }

    /// A node briefly unavailable doesn't fail the read-back: each read is
    /// tried again.
    #[tokio::test]
    async fn a_read_is_tried_again() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let value = with_retries(|| {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if call + 1 < READ_ATTEMPTS {
                    Err("busy".to_string())
                } else {
                    Ok(call)
                }
            }
        })
        .await;
        assert_eq!(value, Ok(READ_ATTEMPTS - 1));
        calls.store(0, Ordering::SeqCst);
        let failed: Result<(), String> = with_retries(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err("down".to_string()) }
        })
        .await;
        assert_eq!(failed, Err("down".to_string()));
        assert_eq!(calls.load(Ordering::SeqCst), READ_ATTEMPTS);
    }
}
