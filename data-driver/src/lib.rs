// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Data driver for Hyperlane Dusk contracts.
//
// Implements the `ConvertibleContract` trait from `dusk-data-driver` to
// convert between JSON and rkyv binary formats for Mailbox contract
// functions and events.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use dusk_bytes::Serializable;
use dusk_core::signatures::bls::PublicKey as AccountPublicKey;
use dusk_data_driver::{
    from_rkyv, json_to_rkyv, rkyv_to_json_u64, ConvertibleContract, Error, JsonValue,
};
use serde::Deserialize;

use hyperlane_dusk_types::drc20::{Account, Allowance, BalanceOf};
use hyperlane_dusk_types::events;
use hyperlane_dusk_types::{DomainGasConfig, EthAddress, GasPaymentRecord, MessageId, H256};

mod lossless_json;

fn rkyv_to_json<T>(bytes: &[u8]) -> Result<JsonValue, Error>
where
    T: serde::Serialize + rkyv::Archive,
    for<'a> T::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>
        + rkyv::Deserialize<T, rkyv::Infallible>,
{
    let value: T = from_rkyv(bytes)?;
    Ok(lossless_json::to_json(&value)?)
}

fn deserialize_u64<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Input {
        Integer(u64),
        Decimal(String),
    }
    match Input::deserialize(deserializer)? {
        Input::Integer(value) => Ok(value),
        Input::Decimal(value) => {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(serde::de::Error::custom("expected an unsigned decimal u64"));
            }
            value.parse().map_err(serde::de::Error::custom)
        }
    }
}

#[derive(Deserialize)]
struct JsonU64(#[serde(deserialize_with = "deserialize_u64")] u64);

fn encode_quote_gas_input(json: &str) -> Result<Vec<u8>, Error> {
    let (domain, amount): (u32, JsonU64) = serde_json::from_str(json)?;
    rkyv::to_bytes::<_, 1024>(&(domain, amount.0))
        .map(|bytes| bytes.to_vec())
        .map_err(|error| Error::Rkyv(format!("cannot serialize quote input: {error}")))
}

fn encode_quote_transfer_input(json: &str) -> Result<Vec<u8>, Error> {
    let (domain, recipient, amount): (u32, H256, JsonU64) = serde_json::from_str(json)?;
    rkyv::to_bytes::<_, 1024>(&(domain, recipient, amount.0))
        .map(|bytes| bytes.to_vec())
        .map_err(|error| Error::Rkyv(format!("cannot serialize quote input: {error}")))
}

/// Data driver for Hyperlane Dusk contracts (Mailbox, hooks, warp routes).
///
/// Enables the Dusk block explorer and RUES tooling to decode contract
/// inputs, outputs, and events in a human-readable JSON format.
#[derive(Default)]
pub struct HyperlaneDataDriver;

#[derive(Deserialize)]
struct WithdrawalInputJson {
    recipient: Vec<u8>,
    #[serde(deserialize_with = "deserialize_u64")]
    amount: u64,
}

fn encode_withdrawal_input(json: &str) -> Result<Vec<u8>, Error> {
    let input: WithdrawalInputJson = serde_json::from_str(json)?;
    let recipient_bytes: [u8; 96] = input.recipient.try_into().map_err(|bytes: Vec<u8>| {
        Error::Json(format!(
            "withdrawal recipient must be 96 bytes, got {}",
            bytes.len()
        ))
    })?;
    let recipient = AccountPublicKey::from_bytes(&recipient_bytes)
        .map_err(|error| Error::Json(format!("invalid withdrawal recipient: {error:?}")))?;
    if !recipient.is_valid() {
        return Err(Error::Json(
            "withdrawal recipient is not a valid BLS public key".into(),
        ));
    }
    rkyv::to_bytes::<_, 1024>(&(recipient, input.amount))
        .map(|bytes| bytes.to_vec())
        .map_err(|error| Error::Rkyv(format!("cannot serialize withdrawal input: {error}")))
}

#[derive(Deserialize)]
struct AccountInputJson {
    account: Vec<u8>,
}

/// `claim_pending_for` takes one account public key.
fn encode_account_input(json: &str) -> Result<Vec<u8>, Error> {
    let input: AccountInputJson = serde_json::from_str(json)?;
    let account_bytes: [u8; 96] = input.account.try_into().map_err(|bytes: Vec<u8>| {
        Error::Json(format!("account must be 96 bytes, got {}", bytes.len()))
    })?;
    let account = AccountPublicKey::from_bytes(&account_bytes)
        .map_err(|error| Error::Json(format!("invalid account: {error:?}")))?;
    if !account.is_valid() {
        return Err(Error::Json("account is not a valid BLS public key".into()));
    }
    rkyv::to_bytes::<_, 1024>(&account)
        .map(|bytes| bytes.to_vec())
        .map_err(|error| Error::Rkyv(format!("cannot serialize account input: {error}")))
}

#[derive(Deserialize)]
struct PayForGasInputJson {
    message_id: MessageId,
    destination: u32,
    #[serde(deserialize_with = "deserialize_u64")]
    gas_amount: u64,
    #[serde(deserialize_with = "deserialize_u64")]
    deposit: u64,
}

/// The IGP's `pay_for_gas(message_id, destination, gas_amount, deposit)`,
/// from an object that names them. `deposit` must be the transaction's
/// deposit.
fn encode_pay_for_gas_input(json: &str) -> Result<Vec<u8>, Error> {
    let input: PayForGasInputJson = serde_json::from_str(json)?;
    rkyv::to_bytes::<_, 1024>(&(
        input.message_id,
        input.destination,
        input.gas_amount,
        input.deposit,
    ))
    .map(|bytes| bytes.to_vec())
    .map_err(|error| Error::Rkyv(format!("cannot serialize pay_for_gas input: {error}")))
}

fn decode_pay_for_gas_input(rkyv: &[u8]) -> Result<JsonValue, Error> {
    let (message_id, destination, gas_amount, deposit): (MessageId, u32, u64, u64) =
        from_rkyv(rkyv)?;
    Ok(serde_json::json!({
        "message_id": message_id.to_vec(),
        "destination": destination,
        "gas_amount": alloc::string::ToString::to_string(&gas_amount),
        "deposit": alloc::string::ToString::to_string(&deposit),
    }))
}

fn decode_account_input(rkyv: &[u8]) -> Result<JsonValue, Error> {
    let account: AccountPublicKey = from_rkyv(rkyv)?;
    Ok(serde_json::json!({ "account": account.to_bytes().to_vec() }))
}

fn decode_withdrawal_input(rkyv: &[u8]) -> Result<JsonValue, Error> {
    let (recipient, amount): (AccountPublicKey, u64) = from_rkyv(rkyv)?;
    Ok(serde_json::json!({
        "recipient": recipient.to_bytes().to_vec(),
        "amount": alloc::string::ToString::to_string(&amount),
    }))
}

impl ConvertibleContract for HyperlaneDataDriver {
    fn encode_input_fn(&self, fn_name: &str, json: &str) -> Result<Vec<u8>, Error> {
        match fn_name {
            // Mailbox queries (no args)
            "local_domain"
            | "nonce"
            | "latest_dispatched_id"
            | "default_ism"
            | "default_hook"
            | "required_hook"
            | "owner"
            | "claimable_fees"
            | "gas_payment_count"
            | "hooks"
            | "pending_total"
            | "state_version"
            | "validators_and_threshold"
            | "announced_validator_count"
            | "memory_bytes" => json_to_rkyv::<()>(json),
            // Mailbox queries with args
            "delivered" => json_to_rkyv::<(MessageId,)>(json),
            "dispatched_id"
            | "dispatched_block_height"
            | "message_id_at"
            | "inserted_block_height"
            | "gas_payment_at" => json_to_rkyv::<(u32,)>(json),
            "message_ids" | "gas_payments" | "get_announced_validators" => {
                json_to_rkyv::<(u32, u32)>(json)
            }
            "recipient_ism" | "fee_credit" => json_to_rkyv::<(H256,)>(json),
            "get_announced_storage_location_hashes" => json_to_rkyv::<(EthAddress,)>(json),
            "quote_dispatch_for_sender" => {
                json_to_rkyv::<(H256, u32, H256, Vec<u8>, Vec<u8>, H256)>(json)
            }
            "withdraw_dispatch_credit" => encode_withdrawal_input(json),
            "claim_pending_for" => encode_account_input(json),
            // Hook queries
            "hook_type" | "total_gas_payments" | "collected_fees" | "protocol_fee"
            | "max_protocol_fee" | "beneficiary" => json_to_rkyv::<()>(json),
            // Hook contracts use (metadata, message), while Mailbox uses
            // (destination, recipient, body, metadata, hook). The explorer
            // selects a driver by function name, so support both ABI shapes.
            "quote_dispatch" | "quote_dispatch_for_contract" => {
                json_to_rkyv::<(u32, H256, Vec<u8>, Vec<u8>, H256)>(json)
                    .or_else(|_| json_to_rkyv::<(Vec<u8>, Vec<u8>)>(json))
            }
            "quote_transfer_remote" => encode_quote_transfer_input(json),
            "quote_gas_payment" | "quote_pay_for_gas" => encode_quote_gas_input(json),
            "pay_for_gas" => encode_pay_for_gas_input(json),
            "domain_gas_config" => json_to_rkyv::<(u32,)>(json),
            // Warp route queries
            "mailbox"
            | "hook"
            | "interchain_security_module"
            | "total_supply"
            | "name"
            | "symbol"
            | "decimals"
            | "wrapped_token" => json_to_rkyv::<()>(json),
            "enrolled_router" => json_to_rkyv::<(u32,)>(json),
            "is_registered" | "pending_balance" => json_to_rkyv::<(H256,)>(json),
            "balance_of" => json_to_rkyv::<BalanceOf>(json),
            "allowance" => json_to_rkyv::<Allowance>(json),
            "allowance_count" => json_to_rkyv::<Account>(json),
            name => Err(Error::Unsupported(format!("fn_name {name}"))),
        }
    }

    fn decode_input_fn(&self, fn_name: &str, rkyv: &[u8]) -> Result<JsonValue, Error> {
        match fn_name {
            "local_domain"
            | "nonce"
            | "latest_dispatched_id"
            | "default_ism"
            | "default_hook"
            | "required_hook"
            | "owner"
            | "claimable_fees"
            | "gas_payment_count"
            | "hooks"
            | "pending_total"
            | "state_version"
            | "validators_and_threshold"
            | "announced_validator_count"
            | "memory_bytes"
            | "hook_type"
            | "total_gas_payments"
            | "collected_fees"
            | "protocol_fee"
            | "max_protocol_fee"
            | "beneficiary"
            | "mailbox"
            | "hook"
            | "interchain_security_module"
            | "total_supply"
            | "name"
            | "symbol"
            | "decimals"
            | "wrapped_token" => rkyv_to_json::<()>(rkyv),
            "delivered" => rkyv_to_json::<(MessageId,)>(rkyv),
            "dispatched_id"
            | "dispatched_block_height"
            | "message_id_at"
            | "inserted_block_height"
            | "gas_payment_at" => rkyv_to_json::<(u32,)>(rkyv),
            "message_ids" | "gas_payments" | "get_announced_validators" => {
                rkyv_to_json::<(u32, u32)>(rkyv)
            }
            "recipient_ism" | "fee_credit" => rkyv_to_json::<(H256,)>(rkyv),
            "get_announced_storage_location_hashes" => rkyv_to_json::<(EthAddress,)>(rkyv),
            "quote_dispatch_for_sender" => {
                rkyv_to_json::<(H256, u32, H256, Vec<u8>, Vec<u8>, H256)>(rkyv)
            }
            "withdraw_dispatch_credit" => decode_withdrawal_input(rkyv),
            "claim_pending_for" => decode_account_input(rkyv),
            "quote_dispatch" | "quote_dispatch_for_contract" => {
                rkyv_to_json::<(u32, H256, Vec<u8>, Vec<u8>, H256)>(rkyv)
                    .or_else(|_| rkyv_to_json::<(Vec<u8>, Vec<u8>)>(rkyv))
            }
            "quote_transfer_remote" => rkyv_to_json::<(u32, H256, u64)>(rkyv),
            "quote_gas_payment" | "quote_pay_for_gas" => rkyv_to_json::<(u32, u64)>(rkyv),
            "pay_for_gas" => decode_pay_for_gas_input(rkyv),
            "domain_gas_config" | "enrolled_router" => rkyv_to_json::<(u32,)>(rkyv),
            "is_registered" | "pending_balance" => rkyv_to_json::<(H256,)>(rkyv),
            "balance_of" => rkyv_to_json::<BalanceOf>(rkyv),
            "allowance" => rkyv_to_json::<Allowance>(rkyv),
            "allowance_count" => rkyv_to_json::<Account>(rkyv),
            name => Err(Error::Unsupported(format!("fn_name {name}"))),
        }
    }

    fn decode_output_fn(&self, fn_name: &str, rkyv: &[u8]) -> Result<JsonValue, Error> {
        match fn_name {
            // u32 outputs
            "local_domain"
            | "nonce"
            | "gas_payment_count"
            | "state_version"
            | "announced_validator_count"
            | "allowance_count" => rkyv_to_json::<u32>(rkyv),
            // u64 outputs
            "protocol_fee"
            | "max_protocol_fee"
            | "collected_fees"
            | "total_gas_payments"
            | "total_supply"
            | "pending_balance"
            | "balance_of"
            | "allowance"
            | "claimable_fees"
            | "fee_credit"
            | "dispatched_block_height"
            | "inserted_block_height"
            | "memory_bytes"
            | "pending_total" => rkyv_to_json_u64(rkyv),
            // u8 outputs
            "hook_type" | "decimals" => rkyv_to_json::<u8>(rkyv),
            // bool outputs
            "delivered" | "is_registered" => rkyv_to_json::<bool>(rkyv),
            // H256 outputs
            "latest_dispatched_id" | "dispatched_id" | "message_id_at" => {
                rkyv_to_json::<MessageId>(rkyv)
            }
            // ContractId (= H256) outputs
            "default_ism"
            | "default_hook"
            | "required_hook"
            | "beneficiary"
            | "mailbox"
            | "hook"
            | "interchain_security_module"
            | "recipient_ism"
            | "wrapped_token"
            | "enrolled_router" => rkyv_to_json::<H256>(rkyv),
            "owner" => rkyv_to_json::<Option<H256>>(rkyv),
            // Vec<H256> outputs
            "hooks" | "message_ids" | "get_announced_storage_location_hashes" => {
                rkyv_to_json::<Vec<H256>>(rkyv)
            }
            // String outputs
            "name" | "symbol" => rkyv_to_json::<String>(rkyv),
            // Struct outputs
            "domain_gas_config" => rkyv_to_json::<DomainGasConfig>(rkyv),
            "gas_payment_at" => rkyv_to_json::<GasPaymentRecord>(rkyv),
            "gas_payments" => rkyv_to_json::<Vec<GasPaymentRecord>>(rkyv),
            "validators_and_threshold" => rkyv_to_json::<(Vec<EthAddress>, u8)>(rkyv),
            "get_announced_validators" => rkyv_to_json::<Vec<EthAddress>>(rkyv),
            // u64 from quote_dispatch / quote_gas_payment
            "quote_dispatch"
            | "quote_dispatch_for_contract"
            | "quote_dispatch_for_sender"
            | "quote_gas_payment"
            | "quote_pay_for_gas"
            | "quote_transfer_remote" => rkyv_to_json_u64(rkyv),
            name => Err(Error::Unsupported(format!("fn_name {name}"))),
        }
    }

    fn decode_event(&self, event_name: &str, rkyv: &[u8]) -> Result<JsonValue, Error> {
        match event_name {
            events::Initialized::TOPIC => rkyv_to_json::<events::Initialized>(rkyv),
            events::OwnershipTransferred::TOPIC => {
                rkyv_to_json::<events::OwnershipTransferred>(rkyv)
            }
            events::OwnershipRenounced::TOPIC => rkyv_to_json::<events::OwnershipRenounced>(rkyv),
            events::AccountRegistered::TOPIC => rkyv_to_json::<events::AccountRegistered>(rkyv),
            events::RemoteRouterEnrolled::TOPIC => {
                rkyv_to_json::<events::RemoteRouterEnrolled>(rkyv)
            }
            events::HookSet::TOPIC => rkyv_to_json::<events::HookSet>(rkyv),
            events::IsmSet::TOPIC => rkyv_to_json::<events::IsmSet>(rkyv),
            events::BeneficiarySet::TOPIC => rkyv_to_json::<events::BeneficiarySet>(rkyv),
            events::ProtocolFeeSet::TOPIC => rkyv_to_json::<events::ProtocolFeeSet>(rkyv),
            events::DomainGasConfigSet::TOPIC => rkyv_to_json::<events::DomainGasConfigSet>(rkyv),
            events::ValidatorsAndThresholdSet::TOPIC => {
                rkyv_to_json::<events::ValidatorsAndThresholdSet>(rkyv)
            }
            events::PendingTransferClaimed::TOPIC => {
                rkyv_to_json::<events::PendingTransferClaimed>(rkyv)
            }
            events::Dispatch::TOPIC => rkyv_to_json::<events::Dispatch>(rkyv),
            events::DispatchId::TOPIC => rkyv_to_json::<events::DispatchId>(rkyv),
            events::DispatchFeeFunded::TOPIC => rkyv_to_json::<events::DispatchFeeFunded>(rkyv),
            events::DispatchFeeWithdrawn::TOPIC => {
                rkyv_to_json::<events::DispatchFeeWithdrawn>(rkyv)
            }
            events::DispatchFeePaid::TOPIC => rkyv_to_json::<events::DispatchFeePaid>(rkyv),
            events::Process::TOPIC => rkyv_to_json::<events::Process>(rkyv),
            events::ProcessId::TOPIC => rkyv_to_json::<events::ProcessId>(rkyv),
            events::DefaultIsmSet::TOPIC => rkyv_to_json::<events::DefaultIsmSet>(rkyv),
            events::DefaultHookSet::TOPIC => rkyv_to_json::<events::DefaultHookSet>(rkyv),
            events::RequiredHookSet::TOPIC => rkyv_to_json::<events::RequiredHookSet>(rkyv),
            events::InsertedIntoTree::TOPIC => rkyv_to_json::<events::InsertedIntoTree>(rkyv),
            events::ValidatorAnnouncement::TOPIC => {
                rkyv_to_json::<events::ValidatorAnnouncement>(rkyv)
            }
            events::ProtocolFeePaid::TOPIC => rkyv_to_json::<events::ProtocolFeePaid>(rkyv),
            events::GasPayment::TOPIC => rkyv_to_json::<events::GasPayment>(rkyv),
            events::Drc20Approval::TOPIC => rkyv_to_json::<events::Drc20Approval>(rkyv),
            events::Drc20Transfer::TOPIC => rkyv_to_json::<events::Drc20Transfer>(rkyv),
            events::SentTransferRemote::TOPIC => rkyv_to_json::<events::SentTransferRemote>(rkyv),
            events::ReceivedTransferRemote::TOPIC => {
                rkyv_to_json::<events::ReceivedTransferRemote>(rkyv)
            }
            event => Err(Error::Unsupported(format!("event {event}"))),
        }
    }

    fn get_schema(&self) -> String {
        String::new()
    }
}

#[cfg(all(target_family = "wasm", feature = "ffi"))]
dusk_data_driver::generate_wasm_entrypoint!(HyperlaneDataDriver);

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{ConvertibleContract, HyperlaneDataDriver};
    use dusk_bytes::Serializable;
    use dusk_core::signatures::bls::{
        PublicKey as AccountPublicKey, SecretKey as AccountSecretKey,
    };
    use dusk_data_driver::{json_to_rkyv, to_json};
    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::{EthAddress, GasPaymentRecord};
    use rand::SeedableRng;

    #[test]
    fn owner_output_decodes_optional_owner_state() {
        let value = Some([7u8; 32]);
        let json = to_json(value).unwrap().to_string();
        let encoded = json_to_rkyv::<Option<[u8; 32]>>(&json).unwrap();
        HyperlaneDataDriver
            .decode_output_fn("owner", &encoded)
            .expect("optional owner output should decode");

        let json = to_json(Option::<[u8; 32]>::None).unwrap().to_string();
        let encoded = json_to_rkyv::<Option<[u8; 32]>>(&json).unwrap();
        HyperlaneDataDriver
            .decode_output_fn("owner", &encoded)
            .expect("renounced owner output should decode");
    }

    #[test]
    fn operational_event_added_by_the_contracts_decodes() {
        let event = events::PendingTransferClaimed {
            recipient: [9u8; 32],
            amount: 42,
        };
        let json = to_json(event).unwrap().to_string();
        let encoded = json_to_rkyv::<events::PendingTransferClaimed>(&json).unwrap();
        HyperlaneDataDriver
            .decode_event(events::PendingTransferClaimed::TOPIC, &encoded)
            .expect("pending collateral claim event should decode");
    }

    #[test]
    fn drc20_contract_account_query_round_trips() {
        let json = r#"{"account":{"kind":"Contract","bytes":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1]}}"#;
        let encoded = HyperlaneDataDriver
            .encode_input_fn("balance_of", json)
            .expect("contract-account balance query should encode");
        HyperlaneDataDriver
            .decode_input_fn("balance_of", &encoded)
            .expect("contract-account balance query should decode");
    }

    /// WarpDrc20's `allowance_count` takes the owner itself and answers a
    /// `u32`; its `memory_bytes` answers as the Mailbox's does.
    #[test]
    fn warp_drc20_allowance_count_round_trips() {
        let owner = r#"{"kind":"Contract","bytes":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1]}"#;
        let encoded = HyperlaneDataDriver
            .encode_input_fn("allowance_count", owner)
            .expect("an allowance count query should encode");
        assert_eq!(
            HyperlaneDataDriver
                .decode_input_fn("allowance_count", &encoded)
                .expect("an allowance count query should decode")["kind"],
            "Contract"
        );
        let count = rkyv::to_bytes::<_, 16>(&64u32).unwrap();
        assert_eq!(
            HyperlaneDataDriver
                .decode_output_fn("allowance_count", &count)
                .expect("an allowance count should decode"),
            serde_json::json!(64)
        );
    }

    #[test]
    fn current_accounting_queries_round_trip() {
        let driver = HyperlaneDataDriver;

        for name in [
            "claimable_fees",
            "gas_payment_count",
            "hooks",
            "pending_total",
            "state_version",
            "validators_and_threshold",
        ] {
            let encoded = driver
                .encode_input_fn(name, "null")
                .expect("no-argument accounting query should encode");
            driver
                .decode_input_fn(name, &encoded)
                .expect("no-argument accounting query should decode");
        }

        let payer_json = to_json(([7u8; 32],)).unwrap().to_string();
        let encoded = driver
            .encode_input_fn("fee_credit", &payer_json)
            .expect("fee-credit payer should encode");
        driver
            .decode_input_fn("fee_credit", &encoded)
            .expect("fee-credit payer should decode");

        let index_json = to_json((3u32,)).unwrap().to_string();
        for name in [
            "dispatched_block_height",
            "inserted_block_height",
            "message_id_at",
            "gas_payment_at",
        ] {
            let encoded = driver
                .encode_input_fn(name, &index_json)
                .expect("indexed accounting query should encode");
            driver
                .decode_input_fn(name, &encoded)
                .expect("indexed accounting query should decode");
        }

        let page_json = to_json((3u32, 256u32)).unwrap().to_string();
        for name in ["message_ids", "gas_payments"] {
            let encoded = driver
                .encode_input_fn(name, &page_json)
                .expect("paged query should encode");
            driver
                .decode_input_fn(name, &encoded)
                .expect("paged query should decode");
        }

        let u64_json = to_json(42u64).unwrap().to_string();
        let u64_output = json_to_rkyv::<u64>(&u64_json).unwrap();
        for name in [
            "claimable_fees",
            "fee_credit",
            "dispatched_block_height",
            "inserted_block_height",
            "pending_total",
        ] {
            driver
                .decode_output_fn(name, &u64_output)
                .expect("u64 accounting output should decode");
        }

        let u32_json = to_json(1u32).unwrap().to_string();
        let u32_output = json_to_rkyv::<u32>(&u32_json).unwrap();
        driver
            .decode_output_fn("state_version", &u32_output)
            .expect("state-version output should decode");

        let h256_json = to_json([5u8; 32]).unwrap().to_string();
        let h256_output = json_to_rkyv::<[u8; 32]>(&h256_json).unwrap();
        driver
            .decode_output_fn("message_id_at", &h256_output)
            .expect("Merkle provenance output should decode");

        let record = GasPaymentRecord {
            message_id: [1u8; 32],
            destination: 7,
            gas_limit: 8,
            payment: 9,
            block_height: 10,
        };
        let record_json = to_json(record).unwrap().to_string();
        let record_output = json_to_rkyv::<GasPaymentRecord>(&record_json).unwrap();
        driver
            .decode_output_fn("gas_payment_at", &record_output)
            .expect("gas-payment record should decode");

        let records_json = to_json(vec![record]).unwrap().to_string();
        let records_output = json_to_rkyv::<Vec<GasPaymentRecord>>(&records_json).unwrap();
        driver
            .decode_output_fn("gas_payments", &records_output)
            .expect("gas-payment page should decode");

        let message_ids = vec![[2u8; 32], [3u8; 32]];
        let message_ids_json = to_json(message_ids).unwrap().to_string();
        let message_ids_output = json_to_rkyv::<Vec<[u8; 32]>>(&message_ids_json).unwrap();
        driver
            .decode_output_fn("message_ids", &message_ids_output)
            .expect("message-ID page should decode");

        let validator_config = (vec![EthAddress([4u8; 20])], 1u8);
        let validator_json = to_json(validator_config).unwrap().to_string();
        let validator_output = json_to_rkyv::<(Vec<EthAddress>, u8)>(&validator_json).unwrap();
        driver
            .decode_output_fn("validators_and_threshold", &validator_output)
            .expect("validator configuration should decode");
    }

    /// Queries the contracts no longer have: the Mailbox's processed log and
    /// delivery heights, the Merkle hook's per-insertion roots and the
    /// ValidatorAnnounce location strings.
    #[test]
    fn removed_storage_queries_are_unsupported() {
        let driver = HyperlaneDataDriver;
        for name in [
            "processed_count",
            "processed_at_index",
            "processed_block_height_at_index",
            "delivered_at",
            "root_at",
            "get_announced_storage_locations",
            "get_announced_storage_locations_for_validator",
        ] {
            assert!(driver.encode_input_fn(name, "[3]").is_err(), "{name}");
            assert!(driver.decode_input_fn(name, &[0; 4]).is_err(), "{name}");
            assert!(driver.decode_output_fn(name, &[0; 8]).is_err(), "{name}");
        }
    }

    #[test]
    fn quote_dispatch_for_sender_round_trips() {
        let driver = HyperlaneDataDriver;
        let json = to_json((
            [1u8; 32],
            1000u32,
            [2u8; 32],
            vec![3u8, 4],
            Vec::<u8>::new(),
            [0u8; 32],
        ))
        .unwrap()
        .to_string();
        let encoded = driver
            .encode_input_fn("quote_dispatch_for_sender", &json)
            .expect("quote input should encode");
        assert_eq!(
            encoded,
            rkyv::to_bytes::<_, 1024>(&(
                [1u8; 32],
                1000u32,
                [2u8; 32],
                vec![3u8, 4],
                Vec::<u8>::new(),
                [0u8; 32]
            ))
            .unwrap()
            .to_vec()
        );
        driver
            .decode_input_fn("quote_dispatch_for_sender", &encoded)
            .expect("quote input should decode");
        let quote = rkyv::to_bytes::<_, 1024>(&u64::MAX).unwrap();
        assert_eq!(
            driver
                .decode_output_fn("quote_dispatch_for_sender", &quote)
                .unwrap(),
            serde_json::json!(u64::MAX.to_string())
        );
    }

    #[test]
    fn quote_dispatch_supports_mailbox_and_hook_abis() {
        let driver = HyperlaneDataDriver;
        let mailbox_json = to_json((1000u32, [1u8; 32], vec![2u8, 3], vec![4u8, 5], [6u8; 32]))
            .unwrap()
            .to_string();
        let hook_json = to_json((vec![7u8, 8], vec![9u8, 10])).unwrap().to_string();

        for json in [mailbox_json, hook_json] {
            let encoded = driver
                .encode_input_fn("quote_dispatch", &json)
                .expect("supported quote_dispatch ABI should encode");
            driver
                .decode_input_fn("quote_dispatch", &encoded)
                .expect("supported quote_dispatch ABI should decode");
        }
    }

    #[test]
    fn validator_announce_discovery_queries_round_trip() {
        let driver = HyperlaneDataDriver;
        let validator = EthAddress([7u8; 20]);

        let page_json = to_json((0u32, 2u32)).unwrap().to_string();
        let page = driver
            .encode_input_fn("get_announced_validators", &page_json)
            .expect("validator registry query should encode");
        driver
            .decode_input_fn("get_announced_validators", &page)
            .expect("validator registry query should decode");

        let count = driver
            .encode_input_fn("announced_validator_count", "null")
            .expect("validator registry count should encode");
        driver
            .decode_input_fn("announced_validator_count", &count)
            .expect("validator registry count should decode");

        let one_json = to_json((validator,)).unwrap().to_string();
        let one = driver
            .encode_input_fn("get_announced_storage_location_hashes", &one_json)
            .expect("location hash query should encode");
        driver
            .decode_input_fn("get_announced_storage_location_hashes", &one)
            .expect("location hash query should decode");

        let validators =
            json_to_rkyv::<Vec<EthAddress>>(&to_json(vec![validator]).unwrap().to_string())
                .unwrap();
        driver
            .decode_output_fn("get_announced_validators", &validators)
            .expect("validator registry output should decode");

        let hashes =
            json_to_rkyv::<Vec<[u8; 32]>>(&to_json(vec![[6u8; 32]]).unwrap().to_string()).unwrap();
        driver
            .decode_output_fn("get_announced_storage_location_hashes", &hashes)
            .expect("location hash output should decode");
    }

    #[test]
    fn dispatch_fee_withdrawn_event_round_trips_through_the_driver() {
        let payer = [0x11u8; 32];
        let recipient = [0x22u8; 32];
        let json = format!(
            r#"{{"payer":{:?},"recipient":{:?},"amount":42}}"#,
            payer, recipient
        );
        let bytes = json_to_rkyv::<events::DispatchFeeWithdrawn>(&json)
            .expect("event JSON should serialize");

        let decoded = HyperlaneDataDriver
            .decode_event(events::DispatchFeeWithdrawn::TOPIC, &bytes)
            .expect("event bytes should decode");
        assert_eq!(decoded["amount"].as_str(), Some("42"));
        assert!(decoded["payer"]
            .as_array()
            .expect("payer should be an array")
            .iter()
            .all(|value| value.as_u64() == Some(0x11)));
        assert!(decoded["recipient"]
            .as_array()
            .expect("recipient should be an array")
            .iter()
            .all(|value| value.as_u64() == Some(0x22)));

        assert!(HyperlaneDataDriver
            .decode_event(events::DispatchFeeWithdrawn::TOPIC, &[0xff])
            .is_err());
    }

    #[test]
    fn dispatch_credit_withdrawal_input_round_trips_through_the_driver() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x57495448);
        let recipient = AccountPublicKey::from(&AccountSecretKey::random(&mut rng));
        let expected = serde_json::json!({
            "recipient": recipient.to_bytes().to_vec(),
            "amount": 42u64,
        });
        let json = expected.to_string();
        let bytes = HyperlaneDataDriver
            .encode_input_fn("withdraw_dispatch_credit", &json)
            .expect("withdrawal input should encode");
        let decoded = HyperlaneDataDriver
            .decode_input_fn("withdraw_dispatch_credit", &bytes)
            .expect("withdrawal input should decode");
        let mut expected = expected;
        expected["amount"] = serde_json::json!("42");
        assert_eq!(decoded, expected);
        assert_eq!(
            HyperlaneDataDriver
                .encode_input_fn("withdraw_dispatch_credit", &decoded.to_string())
                .unwrap(),
            bytes
        );
    }

    /// Routes no longer export `register_account` or `register_account_for`;
    /// only a claim of escrow registers a key, the claimant's own or someone
    /// else's.
    #[test]
    fn sponsored_claim_input_round_trips_and_registration_for_is_gone() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x434c_4149);
        let account = AccountPublicKey::from(&AccountSecretKey::random(&mut rng));
        let json = serde_json::json!({ "account": account.to_bytes().to_vec() });
        let bytes = HyperlaneDataDriver
            .encode_input_fn("claim_pending_for", &json.to_string())
            .expect("claim input should encode");
        assert_eq!(
            HyperlaneDataDriver
                .decode_input_fn("claim_pending_for", &bytes)
                .expect("claim input should decode"),
            json
        );

        assert!(HyperlaneDataDriver
            .encode_input_fn("register_account_for", &json.to_string())
            .is_err());
        assert!(HyperlaneDataDriver
            .decode_input_fn("register_account_for", &bytes)
            .is_err());
        assert!(HyperlaneDataDriver
            .encode_input_fn("register_account", "null")
            .is_err());
    }

    #[test]
    fn structured_u64_outputs_are_decimal_strings_at_every_magnitude() {
        use hyperlane_dusk_types::DomainGasConfig;
        for value in [0u64, 42, 9_007_199_254_740_993, u64::MAX] {
            let config = DomainGasConfig {
                gas_overhead: value,
                token_exchange_rate: value,
                gas_price: value,
            };
            let bytes = rkyv::to_bytes::<_, 1024>(&config).unwrap();
            let decoded = HyperlaneDataDriver
                .decode_output_fn("domain_gas_config", &bytes)
                .unwrap();
            for field in ["gas_overhead", "token_exchange_rate", "gas_price"] {
                assert_eq!(decoded[field], serde_json::json!(value.to_string()));
            }
            let record = GasPaymentRecord {
                message_id: [0x11; 32],
                destination: u32::MAX,
                gas_limit: value,
                payment: value,
                block_height: value,
            };
            let bytes = rkyv::to_bytes::<_, 1024>(&vec![record]).unwrap();
            let decoded = HyperlaneDataDriver
                .decode_output_fn("gas_payments", &bytes)
                .unwrap();
            let record = &decoded[0];
            assert_eq!(record["destination"].as_u64(), Some(u64::from(u32::MAX)));
            assert_eq!(
                record["message_id"],
                serde_json::json!([0x11; 32].as_slice())
            );
            for field in ["gas_limit", "payment", "block_height"] {
                assert_eq!(record[field], serde_json::json!(value.to_string()));
            }
            let event = events::GasPayment {
                message_id: [0x22; 32],
                gas_limit: value,
                payment: value,
            };
            let bytes = rkyv::to_bytes::<_, 1024>(&event).unwrap();
            let decoded = HyperlaneDataDriver
                .decode_event(events::GasPayment::TOPIC, &bytes)
                .unwrap();
            assert_eq!(decoded["gas_limit"], serde_json::json!(value.to_string()));
            assert_eq!(decoded["payment"], serde_json::json!(value.to_string()));
            assert_eq!(
                decoded["message_id"],
                serde_json::json!([0x22; 32].as_slice())
            );
        }
    }

    /// The IGP's `pay_for_gas` takes `(message_id, destination, gas_amount,
    /// deposit)`; the driver names them, takes the u64s as integers or
    /// decimal strings, and gives them back as decimal strings.
    #[test]
    fn pay_for_gas_input_round_trips_through_the_driver() {
        let driver = HyperlaneDataDriver;
        let id = [0x5Au8; 32];
        for value in [1u64, 42, 9_007_199_254_740_993, u64::MAX] {
            let expected = serde_json::json!({
                "message_id": id.as_slice(),
                "destination": 11_155_111,
                "gas_amount": value.to_string(),
                "deposit": value.to_string(),
            });
            let mut numbers = expected.clone();
            numbers["gas_amount"] = serde_json::json!(value);
            numbers["deposit"] = serde_json::json!(value);
            let encoded = driver
                .encode_input_fn("pay_for_gas", &expected.to_string())
                .expect("pay_for_gas input should encode");
            assert_eq!(
                encoded,
                rkyv::to_bytes::<_, 1024>(&(id, 11_155_111u32, value, value))
                    .unwrap()
                    .to_vec()
            );
            assert_eq!(
                driver
                    .encode_input_fn("pay_for_gas", &numbers.to_string())
                    .unwrap(),
                encoded
            );
            let decoded = driver.decode_input_fn("pay_for_gas", &encoded).unwrap();
            assert_eq!(decoded, expected);
            assert_eq!(
                driver
                    .encode_input_fn("pay_for_gas", &decoded.to_string())
                    .unwrap(),
                encoded
            );
        }
        let short_id = [1u8; 31];
        let valid = serde_json::json!({
            "message_id": id.as_slice(),
            "destination": 1,
            "gas_amount": "1",
            "deposit": "1",
        });
        for (field, bad) in [
            ("message_id", serde_json::json!(short_id.as_slice())),
            ("destination", serde_json::json!(-1)),
            ("gas_amount", serde_json::json!("-1")),
            ("deposit", serde_json::json!(1.5)),
            ("deposit", serde_json::json!(null)),
        ] {
            let mut input = valid.clone();
            input[field] = bad;
            assert!(
                driver
                    .encode_input_fn("pay_for_gas", &input.to_string())
                    .is_err(),
                "{input}"
            );
        }
        let quote = rkyv::to_bytes::<_, 1024>(&u64::MAX).unwrap();
        assert_eq!(
            driver
                .decode_output_fn("quote_pay_for_gas", &quote)
                .unwrap(),
            serde_json::json!(u64::MAX.to_string())
        );

        // The bytes the JavaScript bindings produce (ts-sdk/tests): rkyv lays
        // the tuple out in rustc's field order, the gas amount before the
        // destination, the same here as in the wasm32 contract.
        let pinned = |args: &[u8]| -> alloc::string::String {
            args.iter().map(|b| format!("{b:02x}")).collect()
        };
        let call = rkyv::to_bytes::<_, 1024>(&(
            [0x11u8; 32],
            0x0102_0304u32,
            0x0a0b_0c0d_0e0f_1011u64,
            0x2122_2324_2526_2728u64,
        ))
        .unwrap();
        assert_eq!(
            pinned(&call),
            "11".repeat(32) + "11100f0e0d0c0b0a" + "0403020100000000" + "2827262524232221"
        );
        let quote = rkyv::to_bytes::<_, 1024>(&(0x0102_0304u32, 0x0a0b_0c0d_0e0f_1011u64)).unwrap();
        assert_eq!(
            pinned(&quote),
            alloc::string::String::from("0403020100000000") + "11100f0e0d0c0b0a"
        );
    }

    #[test]
    fn quote_inputs_accept_decimal_strings_and_preserve_binary_round_trips() {
        for value in [0u64, 42, 9_007_199_254_740_993, u64::MAX] {
            let recipient = [0x33u8; 32];
            for (name, number, decimal, index) in [
                (
                    "quote_gas_payment",
                    serde_json::json!([4242, value]),
                    serde_json::json!([4242, value.to_string()]),
                    1,
                ),
                (
                    "quote_pay_for_gas",
                    serde_json::json!([4242, value]),
                    serde_json::json!([4242, value.to_string()]),
                    1,
                ),
                (
                    "quote_transfer_remote",
                    serde_json::json!([4242, recipient, value]),
                    serde_json::json!([4242, recipient, value.to_string()]),
                    2,
                ),
            ] {
                let legacy = HyperlaneDataDriver
                    .encode_input_fn(name, &number.to_string())
                    .unwrap();
                let encoded = HyperlaneDataDriver
                    .encode_input_fn(name, &decimal.to_string())
                    .unwrap();
                assert_eq!(legacy, encoded);
                let decoded = HyperlaneDataDriver.decode_input_fn(name, &encoded).unwrap();
                assert_eq!(decoded[index], serde_json::json!(value.to_string()));
                assert_eq!(decoded[0].as_u64(), Some(4242));
                assert_eq!(
                    HyperlaneDataDriver
                        .encode_input_fn(name, &decoded.to_string())
                        .unwrap(),
                    encoded
                );
            }
        }
        for bad in ["", "-1", "+1", "1.5", "1e3", "18446744073709551616"] {
            assert!(HyperlaneDataDriver
                .encode_input_fn(
                    "quote_gas_payment",
                    &serde_json::json!([4242, bad]).to_string()
                )
                .is_err());
        }
        for bad in [
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!(true),
            serde_json::json!(null),
        ] {
            assert!(HyperlaneDataDriver
                .encode_input_fn(
                    "quote_gas_payment",
                    &serde_json::json!([4242, bad]).to_string()
                )
                .is_err());
        }
    }
}
