// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Adapted from the dusk-forge test-bridge test_session.rs.
// Provides a TestSession that deploys transfer + stake contracts and funds
// public accounts, so that integration tests can deploy and call Hyperlane
// contracts through Moonlight transactions.

use dusk_core::abi::{
    ContractError, ContractId, Metadata, StandardBufSerializer, CONTRACT_ID_BYTES,
};
use dusk_core::signatures::bls::{PublicKey as AccountPublicKey, SecretKey as AccountSecretKey};
use dusk_core::stake::STAKE_CONTRACT;
use dusk_core::transfer::data::ContractCall;
use dusk_core::transfer::moonlight::AccountData;
use dusk_core::transfer::{Transaction, TRANSFER_CONTRACT};
use dusk_core::LUX;
use dusk_vm::host_queries::{set_hard_fork, HardFork};
use dusk_vm::{execute, CallReceipt, ContractData, Error as VMError, ExecutionConfig, Session, VM};
use rkyv::bytecheck::CheckBytes;
use rkyv::ser::serializers::{BufferScratch, BufferSerializer, CompositeSerializer};
use rkyv::ser::Serializer;
use rkyv::validation::validators::DefaultValidator;
use rkyv::{check_archived_root, Archive, Deserialize, Infallible, Serialize};

const ZERO_ADDRESS: ContractId = ContractId::from_bytes([0; CONTRACT_ID_BYTES]);
const GAS_LIMIT: u64 = 0x10_000_000;
const CHAIN_ID: u8 = 0x1;
const CONFIG: ExecutionConfig = ExecutionConfig {
    gas_per_deploy_byte: 0u64,
    gas_per_blob: 0u64,
    min_deploy_points: 0u64,
    min_deploy_gas_price: 0u64,
    with_public_sender: true,
    with_blob: true,
    disable_wasm64: false,
    disable_wasm32: false,
    disable_3rd_party: false,
    phoenix_refund_check: false,
};

/// VM Session with transfer + stake contracts deployed and funded accounts.
pub struct TestSession(pub Session);

impl TestSession {
    /// Deploy bytecode with maximum gas limit.
    pub fn deploy<'a, A, D>(
        &mut self,
        bytecode: &[u8],
        deploy_data: D,
    ) -> Result<ContractId, VMError>
    where
        A: 'a + for<'b> Serialize<StandardBufSerializer<'b>>,
        D: Into<ContractData<'a, A>>,
    {
        self.0.deploy(bytecode, deploy_data, u64::MAX)
    }

    /// Returns the current block-height.
    pub fn block_height(&self) -> u64 {
        rkyv_deserialize(self.0.meta(Metadata::BLOCK_HEIGHT).unwrap())
    }

    /// Sets a new block-height.
    #[allow(dead_code)]
    pub fn set_block_height(&mut self, block_height: u64) {
        let _ = self.0.set_meta(Metadata::BLOCK_HEIGHT, block_height);
    }

    /// Query the chain-id.
    fn chain_id(&self) -> u8 {
        rkyv_deserialize(self.0.meta(Metadata::CHAIN_ID).unwrap())
    }

    /// Query account data for a public key.
    pub fn account(&mut self, pk: &AccountPublicKey) -> Result<AccountData, VMError> {
        self.0
            .call(TRANSFER_CONTRACT, "account", pk, GAS_LIMIT)
            .map(|r| r.data)
    }

    /// Direct call (bypasses transfer contract, no gas, no auth).
    pub fn direct_call<A, R>(
        &mut self,
        contract: ContractId,
        fn_name: &str,
        fn_arg: &A,
    ) -> Result<CallReceipt<R>, ContractError>
    where
        A: for<'b> Serialize<StandardBufSerializer<'b>>,
        A::Archived: for<'b> CheckBytes<DefaultValidator<'b>>,
        R: Archive,
        R::Archived: Deserialize<R, Infallible> + for<'b> CheckBytes<DefaultValidator<'b>>,
    {
        self.0
            .call::<_, R>(contract, fn_name, fn_arg, u64::MAX)
            .map_err(|e| match e {
                VMError::Panic(panic_msg) => ContractError::Panic(panic_msg),
                VMError::OutOfGas => ContractError::OutOfGas,
                _ => ContractError::Panic(format!("{e}")),
            })
    }

    /// Call through the transfer contract (Moonlight tx, with auth).
    pub fn call_public<A, R>(
        &mut self,
        sender_sk: &AccountSecretKey,
        contract: ContractId,
        fn_name: &str,
        fn_arg: &A,
    ) -> Result<CallReceipt<R>, ContractError>
    where
        A: for<'b> Serialize<StandardBufSerializer<'b>>,
        A::Archived: for<'b> CheckBytes<DefaultValidator<'b>>,
        R: Archive,
        R::Archived: Deserialize<R, Infallible> + for<'b> CheckBytes<DefaultValidator<'b>>,
    {
        let contract_call = ContractCall {
            contract,
            fn_name: String::from(fn_name),
            fn_args: rkyv_serialize(fn_arg),
        };

        let moonlight_pk = AccountPublicKey::from(sender_sk);

        let AccountData { nonce, .. } = self
            .account(&moonlight_pk)
            .expect("Getting the account should succeed");

        let transaction = Transaction::moonlight(
            sender_sk,
            None,
            0,
            0,
            GAS_LIMIT,
            LUX,
            nonce + 1,
            CHAIN_ID,
            Some(contract_call),
        )
        .expect("Creating moonlight transaction should succeed");

        let _hard_fork = set_hard_fork(HardFork::Aegis);
        let receipt = execute(&mut self.0, &transaction, &CONFIG)
            .unwrap_or_else(|e| panic!("Unspendable transaction due to '{e}'"));

        match receipt.data {
            Ok(serialized) => Ok(CallReceipt {
                gas_limit: receipt.gas_limit,
                gas_spent: receipt.gas_spent,
                events: receipt.events,
                call_tree: receipt.call_tree,
                data: rkyv_deserialize(&serialized),
            }),
            Err(e) => Err(e),
        }
    }
}

impl TestSession {
    /// Create a new test session with transfer + stake contracts and funded accounts.
    pub fn instantiate(public_pks: Vec<(&AccountPublicKey, u64)>) -> Self {
        let vm = VM::ephemeral().expect("Creating VM should succeed");
        let mut session = VM::genesis_session(&vm, 1);

        // Deploy transfer contract
        let transfer_contract = include_bytes!("genesis-contracts/transfer_contract.wasm");
        session
            .deploy(
                transfer_contract,
                ContractData::builder()
                    .owner(ZERO_ADDRESS.to_bytes())
                    .contract_id(TRANSFER_CONTRACT),
                GAS_LIMIT,
            )
            .expect("Deploying the transfer contract should succeed");

        // Deploy stake contract
        let stake_contract = include_bytes!("genesis-contracts/stake_contract.wasm");
        session
            .deploy(
                stake_contract,
                ContractData::builder()
                    .owner(ZERO_ADDRESS.to_bytes())
                    .contract_id(STAKE_CONTRACT),
                GAS_LIMIT,
            )
            .expect("Deploying the stake contract should succeed");

        // Fund public keys with DUSK
        for (&pk_to_fund, val) in &public_pks {
            session
                .call::<_, ()>(
                    TRANSFER_CONTRACT,
                    "add_account_balance",
                    &(pk_to_fund, *val),
                    GAS_LIMIT,
                )
                .expect("Add account balance should succeed");
        }

        let base = session.commit().expect("Committing should succeed");

        let mut session = TestSession(
            vm.session(base, CHAIN_ID, 1)
                .expect("Instantiating new session should succeed"),
        );

        for (pk, value) in public_pks {
            let account = session
                .account(pk)
                .expect("Getting the account should succeed");
            assert_eq!(
                account.balance, value,
                "The account should own the specified value"
            );
            assert_eq!(account.nonce, 0);
        }

        assert_eq!(
            session.chain_id(),
            CHAIN_ID,
            "the chain id should be as expected"
        );

        session
    }
}

/// Deserialize using rkyv.
pub fn rkyv_deserialize<R>(serialized: impl AsRef<[u8]>) -> R
where
    R: Archive,
    R::Archived: Deserialize<R, Infallible> + for<'b> CheckBytes<DefaultValidator<'b>>,
{
    let ta = check_archived_root::<R>(serialized.as_ref()).expect("Failed to deserialize data");
    ta.deserialize(&mut Infallible)
        .expect("Failed to deserialize using rkyv")
}

/// Serialize using rkyv.
pub fn rkyv_serialize<A>(fn_arg: &A) -> Vec<u8>
where
    A: for<'b> Serialize<StandardBufSerializer<'b>>,
    A::Archived: for<'b> CheckBytes<DefaultValidator<'b>>,
{
    const SCRATCH_SPACE: usize = 1024;
    const PAGE_SIZE: usize = 0x1000;

    let mut sbuf = [0u8; SCRATCH_SPACE];
    let scratch = BufferScratch::new(&mut sbuf);
    let mut buffer = [0u8; PAGE_SIZE];
    let ser = BufferSerializer::new(&mut buffer[..]);
    let mut ser = CompositeSerializer::new(ser, scratch, Infallible);

    ser.serialize_value(fn_arg)
        .expect("Failed to rkyv serialize fn_arg");
    let pos = ser.pos();

    buffer[..pos].to_vec()
}

/// Assert that a contract call panicked with a specific message.
#[allow(dead_code)]
pub fn assert_contract_panic<R>(
    call_result: Result<CallReceipt<R>, ContractError>,
    expected_panic: &str,
) where
    R: Archive,
    R::Archived: Deserialize<R, Infallible> + for<'b> CheckBytes<DefaultValidator<'b>>,
{
    let contract_err = match call_result {
        Ok(_) => panic!("Contract call shouldn't pass"),
        Err(error) => error,
    };

    if let ContractError::Panic(panic_msg) = contract_err {
        assert_eq!(panic_msg, expected_panic);
    } else {
        panic!("Expected contract panic, got error: {contract_err}");
    }
}
