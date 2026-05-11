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

use dusk_data_driver::{
    json_to_rkyv, rkyv_to_json, rkyv_to_json_u64, ConvertibleContract, Error, JsonValue,
};

use hyperlane_dusk_types::events;
use hyperlane_dusk_types::{DomainGasConfig, H256, MessageId};

/// Data driver for Hyperlane Dusk contracts (Mailbox, hooks, warp routes).
///
/// Enables the Dusk block explorer and RUES tooling to decode contract
/// inputs, outputs, and events in a human-readable JSON format.
#[derive(Default)]
pub struct HyperlaneDataDriver;

impl ConvertibleContract for HyperlaneDataDriver {
    fn encode_input_fn(&self, fn_name: &str, json: &str) -> Result<Vec<u8>, Error> {
        match fn_name {
            // Mailbox queries (no args)
            "local_domain" | "nonce" | "latest_dispatched_id" | "default_ism"
            | "default_hook" | "required_hook" | "owner" | "processed_count" => {
                json_to_rkyv::<()>(json)
            }
            // Mailbox queries with args
            "delivered" | "delivered_at" => json_to_rkyv::<(MessageId,)>(json),
            "dispatched_message" | "processed_at_index" => json_to_rkyv::<(u32,)>(json),
            "recipient_ism" => json_to_rkyv::<(H256,)>(json),
            // Hook queries
            "hook_type" | "total_gas_payments" | "collected_fees" | "protocol_fee"
            | "max_protocol_fee" | "beneficiary" => json_to_rkyv::<()>(json),
            "quote_dispatch" => json_to_rkyv::<(Vec<u8>, Vec<u8>)>(json),
            "quote_gas_payment" => json_to_rkyv::<(u32, u64)>(json),
            "domain_gas_config" => json_to_rkyv::<(u32,)>(json),
            // Warp route queries
            "mailbox" | "hook" | "interchain_security_module" | "total_supply"
            | "name" | "symbol" | "decimals" | "wrapped_token" => json_to_rkyv::<()>(json),
            "enrolled_router" => json_to_rkyv::<(u32,)>(json),
            "is_registered" => json_to_rkyv::<(H256,)>(json),
            name => Err(Error::Unsupported(format!("fn_name {name}"))),
        }
    }

    fn decode_input_fn(&self, fn_name: &str, rkyv: &[u8]) -> Result<JsonValue, Error> {
        match fn_name {
            "local_domain" | "nonce" | "latest_dispatched_id" | "default_ism"
            | "default_hook" | "required_hook" | "owner" | "processed_count"
            | "hook_type" | "total_gas_payments" | "collected_fees" | "protocol_fee"
            | "max_protocol_fee" | "beneficiary" | "mailbox" | "hook"
            | "interchain_security_module" | "total_supply" | "name" | "symbol"
            | "decimals" | "wrapped_token" => rkyv_to_json::<()>(rkyv),
            "delivered" | "delivered_at" => rkyv_to_json::<(MessageId,)>(rkyv),
            "dispatched_message" | "processed_at_index" => rkyv_to_json::<(u32,)>(rkyv),
            "recipient_ism" => rkyv_to_json::<(H256,)>(rkyv),
            "quote_dispatch" => rkyv_to_json::<(Vec<u8>, Vec<u8>)>(rkyv),
            "quote_gas_payment" => rkyv_to_json::<(u32, u64)>(rkyv),
            "domain_gas_config" | "enrolled_router" => rkyv_to_json::<(u32,)>(rkyv),
            "is_registered" => rkyv_to_json::<(H256,)>(rkyv),
            name => Err(Error::Unsupported(format!("fn_name {name}"))),
        }
    }

    fn decode_output_fn(&self, fn_name: &str, rkyv: &[u8]) -> Result<JsonValue, Error> {
        match fn_name {
            // u32 outputs
            "local_domain" | "nonce" | "processed_count" => rkyv_to_json::<u32>(rkyv),
            // u64 outputs
            "delivered_at" | "protocol_fee" | "max_protocol_fee" | "collected_fees"
            | "total_gas_payments" | "total_supply" => rkyv_to_json_u64(rkyv),
            // u8 outputs
            "hook_type" | "decimals" => rkyv_to_json::<u8>(rkyv),
            // bool outputs
            "delivered" | "is_registered" => rkyv_to_json::<bool>(rkyv),
            // H256 outputs
            "latest_dispatched_id" | "processed_at_index" => rkyv_to_json::<MessageId>(rkyv),
            // ContractId (= H256) outputs
            "default_ism" | "default_hook" | "required_hook" | "owner"
            | "beneficiary" | "mailbox" | "hook" | "interchain_security_module"
            | "recipient_ism" | "wrapped_token" | "enrolled_router" => {
                rkyv_to_json::<H256>(rkyv)
            }
            // Vec<u8> outputs
            "dispatched_message" => rkyv_to_json::<Vec<u8>>(rkyv),
            // String outputs
            "name" | "symbol" => rkyv_to_json::<String>(rkyv),
            // Struct outputs
            "domain_gas_config" => rkyv_to_json::<DomainGasConfig>(rkyv),
            // u64 from quote_dispatch / quote_gas_payment
            "quote_dispatch" | "quote_gas_payment" => rkyv_to_json_u64(rkyv),
            name => Err(Error::Unsupported(format!("fn_name {name}"))),
        }
    }

    fn decode_event(&self, event_name: &str, rkyv: &[u8]) -> Result<JsonValue, Error> {
        match event_name {
            events::Dispatch::TOPIC => rkyv_to_json::<events::Dispatch>(rkyv),
            events::DispatchId::TOPIC => rkyv_to_json::<events::DispatchId>(rkyv),
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
            events::SentTransferRemote::TOPIC => {
                rkyv_to_json::<events::SentTransferRemote>(rkyv)
            }
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
