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
    from_rkyv, json_to_rkyv, rkyv_to_json, rkyv_to_json_u64, ConvertibleContract, Error, JsonValue,
};
use serde::Deserialize;

use hyperlane_dusk_types::drc20::{Allowance, BalanceOf};
use hyperlane_dusk_types::events;
use hyperlane_dusk_types::{DomainGasConfig, EthAddress, GasPaymentRecord, MessageId, H256};

/// Data driver for Hyperlane Dusk contracts (Mailbox, hooks, warp routes).
///
/// Enables the Dusk block explorer and RUES tooling to decode contract
/// inputs, outputs, and events in a human-readable JSON format.
#[derive(Default)]
pub struct HyperlaneDataDriver;

#[derive(Deserialize)]
struct WithdrawalInputJson {
    recipient: Vec<u8>,
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

fn decode_withdrawal_input(rkyv: &[u8]) -> Result<JsonValue, Error> {
    let (recipient, amount): (AccountPublicKey, u64) = from_rkyv(rkyv)?;
    Ok(serde_json::json!({
        "recipient": recipient.to_bytes().to_vec(),
        "amount": amount,
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
            | "processed_count"
            | "claimable_fees"
            | "gas_payment_count"
            | "hooks"
            | "pending_total"
            | "state_version"
            | "validators_and_threshold" => json_to_rkyv::<()>(json),
            // Mailbox queries with args
            "delivered" | "delivered_at" => json_to_rkyv::<(MessageId,)>(json),
            "dispatched_message"
            | "dispatched_block_height"
            | "processed_at_index"
            | "processed_block_height_at_index"
            | "message_id_at"
            | "inserted_block_height"
            | "root_at"
            | "gas_payment_at" => json_to_rkyv::<(u32,)>(json),
            "message_ids" | "gas_payments" => json_to_rkyv::<(u32, u32)>(json),
            "recipient_ism" | "fee_credit" => json_to_rkyv::<(H256,)>(json),
            "withdraw_dispatch_credit" => encode_withdrawal_input(json),
            // Hook queries
            "hook_type" | "total_gas_payments" | "collected_fees" | "protocol_fee"
            | "max_protocol_fee" | "beneficiary" => json_to_rkyv::<()>(json),
            // Hook contracts use (metadata, message), while Mailbox uses
            // (destination, recipient, body, metadata, hook). The explorer
            // selects a driver by function name, so support both ABI shapes.
            "quote_dispatch" => json_to_rkyv::<(u32, H256, Vec<u8>, Vec<u8>, H256)>(json)
                .or_else(|_| json_to_rkyv::<(Vec<u8>, Vec<u8>)>(json)),
            "quote_gas_payment" => json_to_rkyv::<(u32, u64)>(json),
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
            | "processed_count"
            | "claimable_fees"
            | "gas_payment_count"
            | "hooks"
            | "pending_total"
            | "state_version"
            | "validators_and_threshold"
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
            "delivered" | "delivered_at" => rkyv_to_json::<(MessageId,)>(rkyv),
            "dispatched_message"
            | "dispatched_block_height"
            | "processed_at_index"
            | "processed_block_height_at_index"
            | "message_id_at"
            | "inserted_block_height"
            | "root_at"
            | "gas_payment_at" => rkyv_to_json::<(u32,)>(rkyv),
            "message_ids" | "gas_payments" => rkyv_to_json::<(u32, u32)>(rkyv),
            "recipient_ism" | "fee_credit" => rkyv_to_json::<(H256,)>(rkyv),
            "withdraw_dispatch_credit" => decode_withdrawal_input(rkyv),
            "quote_dispatch" => rkyv_to_json::<(u32, H256, Vec<u8>, Vec<u8>, H256)>(rkyv)
                .or_else(|_| rkyv_to_json::<(Vec<u8>, Vec<u8>)>(rkyv)),
            "quote_gas_payment" => rkyv_to_json::<(u32, u64)>(rkyv),
            "domain_gas_config" | "enrolled_router" => rkyv_to_json::<(u32,)>(rkyv),
            "is_registered" | "pending_balance" => rkyv_to_json::<(H256,)>(rkyv),
            "balance_of" => rkyv_to_json::<BalanceOf>(rkyv),
            "allowance" => rkyv_to_json::<Allowance>(rkyv),
            name => Err(Error::Unsupported(format!("fn_name {name}"))),
        }
    }

    fn decode_output_fn(&self, fn_name: &str, rkyv: &[u8]) -> Result<JsonValue, Error> {
        match fn_name {
            // u32 outputs
            "local_domain" | "nonce" | "processed_count" | "gas_payment_count"
            | "state_version" => rkyv_to_json::<u32>(rkyv),
            // u64 outputs
            "delivered_at"
            | "protocol_fee"
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
            | "processed_block_height_at_index"
            | "pending_total" => rkyv_to_json_u64(rkyv),
            // u8 outputs
            "hook_type" | "decimals" => rkyv_to_json::<u8>(rkyv),
            // bool outputs
            "delivered" | "is_registered" => rkyv_to_json::<bool>(rkyv),
            // H256 outputs
            "latest_dispatched_id" | "processed_at_index" | "message_id_at" | "root_at" => {
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
            // Vec<u8> outputs
            "dispatched_message" => rkyv_to_json::<Vec<u8>>(rkyv),
            // Vec<H256> outputs
            "hooks" | "message_ids" => rkyv_to_json::<Vec<H256>>(rkyv),
            // String outputs
            "name" | "symbol" => rkyv_to_json::<String>(rkyv),
            // Struct outputs
            "domain_gas_config" => rkyv_to_json::<DomainGasConfig>(rkyv),
            "gas_payment_at" => rkyv_to_json::<GasPaymentRecord>(rkyv),
            "gas_payments" => rkyv_to_json::<Vec<GasPaymentRecord>>(rkyv),
            "validators_and_threshold" => rkyv_to_json::<(Vec<EthAddress>, u8)>(rkyv),
            // u64 from quote_dispatch / quote_gas_payment
            "quote_dispatch" | "quote_gas_payment" => rkyv_to_json_u64(rkyv),
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
        let json = r#"{"account":{"Contract":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1]}}"#;
        let encoded = HyperlaneDataDriver
            .encode_input_fn("balance_of", json)
            .expect("contract-account balance query should encode");
        HyperlaneDataDriver
            .decode_input_fn("balance_of", &encoded)
            .expect("contract-account balance query should decode");
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
            "processed_block_height_at_index",
            "root_at",
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
            "processed_block_height_at_index",
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
        for name in ["message_id_at", "root_at"] {
            driver
                .decode_output_fn(name, &h256_output)
                .expect("Merkle provenance output should decode");
        }

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
        assert_eq!(decoded["amount"].as_u64(), Some(42));
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
        assert_eq!(decoded, expected);
    }
}
