//! RUES HTTP client for transaction submission and contract queries.

use dusk_core::signatures::bls::PublicKey as BlsPublicKey;
use dusk_core::transfer::moonlight::AccountData;
use rkyv::ser::serializers::AllocSerializer;
use rkyv::ser::Serializer;
use rkyv::validation::validators::DefaultValidator;
use rkyv::{check_archived_root, Archive, Deserialize, Infallible, Serialize};
use serde::Deserialize as SerdeDeserialize;
use serde_json::Value;

const TRANSFER_CONTRACT: &str = "0100000000000000000000000000000000000000000000000000000000000000";
const MAX_TRANSACTION_STATUS_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_CONTRACT_QUERY_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_ERROR_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_SIMULATION_RESPONSE_BYTES: usize = 64 * 1024;

pub struct RuesClient {
    client: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionStatus {
    NotFound,
    Executed,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, SerdeDeserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SimulationResult {
    pub gas_spent: u64,
    pub error: Option<String>,
}

impl RuesClient {
    pub fn new(base_url: &str) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("Failed to build RUES HTTP client: {e}"))?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    /// Query chain_id from the transfer contract.
    pub async fn query_chain_id(&self) -> Result<u8, String> {
        let bytes = self
            .raw_contract_query(TRANSFER_CONTRACT, "chain_id", &[])
            .await?;
        if bytes.len() == 1 {
            Ok(bytes[0])
        } else {
            Err(format!(
                "Unexpected chain_id response: {} bytes",
                bytes.len()
            ))
        }
    }

    /// Query account data (nonce + balance) for a BLS public key.
    pub async fn query_account(&self, pk: &BlsPublicKey) -> Result<(u64, u64), String> {
        let body = rkyv_serialize(pk);
        let response = self
            .raw_contract_query(TRANSFER_CONTRACT, "account", &body)
            .await?;
        let account: AccountData = rkyv_deserialize(&response)?;
        Ok((account.nonce, account.balance))
    }

    /// Generic typed contract query using rkyv.
    pub async fn contract_query<I, O>(
        &self,
        contract_id: &[u8; 32],
        method: &str,
        args: &I,
    ) -> Result<O, String>
    where
        I: Serialize<AllocSerializer<256>>,
        O: Archive,
        O::Archived: Deserialize<O, Infallible> + for<'b> rkyv::CheckBytes<DefaultValidator<'b>>,
    {
        let body = rkyv_serialize(args);
        let hex_id = hex::encode(contract_id);
        let response = self.raw_contract_query(&hex_id, method, &body).await?;
        rkyv_deserialize(&response)
    }

    /// Preverify then propagate a serialized transaction.
    pub async fn propagate_tx(&self, tx_bytes: &[u8]) -> Result<(), String> {
        // Preverify first
        let url = format!("{}/on/transactions/preverify", self.base_url);
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(tx_bytes.to_vec())
            .send()
            .await
            .map_err(|e| format!("Preverify failed before propagation: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = read_response_body(response, MAX_ERROR_RESPONSE_BYTES, "preverify")
                .await
                .map_err(|error| format!("Preverify failed before propagation: {error}"))?;
            return Err(format!(
                "Preverify rejected before propagation ({status}): {}",
                String::from_utf8_lossy(&body)
            ));
        }

        // Propagate
        let url = format!("{}/on/transactions/propagate", self.base_url);
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(tx_bytes.to_vec())
            .send()
            .await
            .map_err(|e| format!("Propagation outcome unknown: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = read_response_body(response, MAX_ERROR_RESPONSE_BYTES, "propagate")
                .await
                .map_err(|error| format!("Propagation outcome unknown: {error}"))?;
            return Err(propagation_status_error(status, &body));
        }
        Ok(())
    }

    /// Execute a transaction against an ephemeral node session without
    /// propagating or committing it.
    pub async fn simulate_tx(&self, tx_bytes: &[u8]) -> Result<SimulationResult, String> {
        let url = format!("{}/on/transactions/simulate", self.base_url);
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(tx_bytes.to_vec())
            .send()
            .await
            .map_err(|error| format!("Simulation request failed: {error}"))?;
        let status = response.status();
        let body =
            read_response_body(response, MAX_SIMULATION_RESPONSE_BYTES, "simulation").await?;
        if !status.is_success() {
            return Err(format!(
                "Simulation request failed ({status}): {}",
                String::from_utf8_lossy(&body)
            ));
        }
        parse_simulation_response(&body)
    }

    /// Query the persisted execution result for an exact transaction hash.
    pub async fn query_transaction_status(&self, tx_id: &str) -> Result<TransactionStatus, String> {
        let url = format!("{}/graphql", self.base_url);
        let query = transaction_status_query(tx_id);
        let response = self
            .client
            .post(&url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({ "query": query }))
                    .expect("GraphQL request serialization should not fail"),
            )
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        let status = response.status();
        let body = read_response_body(
            response,
            MAX_TRANSACTION_STATUS_RESPONSE_BYTES,
            "transaction status",
        )
        .await?;
        if !status.is_success() {
            return Err(format!(
                "Transaction status query failed ({status}): {}",
                String::from_utf8_lossy(&body)
            ));
        }

        parse_transaction_status_response(&body)
    }

    /// Returns whether a contract exists on-chain, using VM metadata.
    ///
    /// This does **not** invoke contract code.
    pub async fn contract_exists(&self, contract_hex: &str) -> Result<bool, String> {
        // RUES requires a non-empty topic; "owner" is conventional here.
        let url = format!("{}/on/contract_owner:{}/owner", self.base_url, contract_hex);
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(Vec::new())
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        let status = response.status();
        if status.is_success() {
            return Ok(true);
        }

        let body =
            read_response_body(response, MAX_ERROR_RESPONSE_BYTES, "contract existence").await?;
        let body = String::from_utf8_lossy(&body);

        // Rusk reports non-existent contracts with a descriptive error body.
        // The status code and exact wording have varied across versions.
        let body_lc = body.to_lowercase();
        if (status.as_u16() == 404 || status.as_u16() == 500)
            && (body.contains("ContractDoesNotExist")
                || body_lc.contains("contract does not exist")
                || body_lc.contains("contract owner not found"))
        {
            return Ok(false);
        }

        Err(format!(
            "Contract existence query failed ({status}): {body}"
        ))
    }

    async fn raw_contract_query(
        &self,
        contract_hex: &str,
        method: &str,
        body: &[u8],
    ) -> Result<Vec<u8>, String> {
        let url = format!("{}/on/contracts:{}/{}", self.base_url, contract_hex, method);
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(body.to_vec())
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        let status = response.status();
        let max_bytes = if status.is_success() {
            MAX_CONTRACT_QUERY_RESPONSE_BYTES
        } else {
            MAX_ERROR_RESPONSE_BYTES
        };
        let response_body = read_response_body(response, max_bytes, "contract query").await?;
        if !status.is_success() {
            return Err(format!(
                "Query {contract_hex}/{method} failed ({status}): {}",
                String::from_utf8_lossy(&response_body)
            ));
        }

        Ok(response_body)
    }
}

fn propagation_status_error(status: reqwest::StatusCode, body: &[u8]) -> String {
    let detail = String::from_utf8_lossy(body);
    if status.is_client_error() {
        format!("Propagation rejected ({status}): {detail}")
    } else {
        format!("Propagation outcome unknown ({status}): {detail}")
    }
}

fn parse_simulation_response(body: &[u8]) -> Result<SimulationResult, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| format!("Invalid simulation response: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "Invalid simulation response: expected an object".to_string())?;
    if !object.contains_key("gas-spent") || !object.contains_key("error") {
        return Err("Invalid simulation response: missing gas-spent or error field".to_string());
    }
    serde_json::from_value(value).map_err(|error| format!("Invalid simulation response: {error}"))
}

async fn read_response_body(
    mut response: reqwest::Response,
    max_bytes: usize,
    context: &str,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(format!("{context} response exceeds {max_bytes} bytes"));
    }

    let mut body = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or_default()
            .min(max_bytes as u64) as usize,
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Failed to read {context} response: {e}"))?
    {
        append_bounded_chunk(&mut body, &chunk, max_bytes, context)?;
    }
    Ok(body)
}

fn append_bounded_chunk(
    body: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: usize,
    context: &str,
) -> Result<(), String> {
    if chunk.len() > max_bytes.saturating_sub(body.len()) {
        return Err(format!("{context} response exceeds {max_bytes} bytes"));
    }
    body.extend_from_slice(chunk);
    Ok(())
}
fn transaction_status_query(tx_id: &str) -> String {
    format!(r#"query {{ tx(hash: "{tx_id}") {{ err }} }}"#)
}

fn parse_transaction_status_response(body: &[u8]) -> Result<TransactionStatus, String> {
    let payload: Value = serde_json::from_slice(body)
        .map_err(|e| format!("Invalid transaction status response: {e}"))?;

    if let Some(errors) = payload.get("errors") {
        let contains_errors = match errors {
            Value::Null => false,
            Value::Array(items) => !items.is_empty(),
            _ => true,
        };
        if contains_errors {
            return Err(format!(
                "Transaction status query returned errors: {errors}"
            ));
        }
    }

    let transaction = payload
        .get("data")
        .and_then(|data| data.get("tx"))
        .ok_or_else(|| "Transaction status response is missing data.tx".to_string())?;
    if transaction.is_null() {
        return Ok(TransactionStatus::NotFound);
    }

    match transaction.get("err") {
        Some(Value::Null) => Ok(TransactionStatus::Executed),
        Some(Value::String(error)) => Ok(TransactionStatus::Failed(error.clone())),
        Some(other) => Err(format!(
            "Transaction status response has invalid data.tx.err: {other}"
        )),
        None => Err("Transaction status response is missing data.tx.err".into()),
    }
}

pub fn rkyv_serialize<T>(value: &T) -> Vec<u8>
where
    T: Serialize<AllocSerializer<256>>,
{
    let mut serializer = AllocSerializer::<256>::default();
    serializer
        .serialize_value(value)
        .expect("rkyv serialization should not fail");
    serializer.into_serializer().into_inner().to_vec()
}

pub fn rkyv_deserialize<T>(bytes: &[u8]) -> Result<T, String>
where
    T: Archive,
    T::Archived: Deserialize<T, Infallible> + for<'b> rkyv::CheckBytes<DefaultValidator<'b>>,
{
    let archived =
        check_archived_root::<T>(bytes).map_err(|e| format!("rkyv deserialization error: {e}"))?;
    archived
        .deserialize(&mut Infallible)
        .map_err(|e| format!("rkyv deserialize error: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::{
        append_bounded_chunk, parse_simulation_response, parse_transaction_status_response,
        propagation_status_error, transaction_status_query, TransactionStatus,
        MAX_TRANSACTION_STATUS_RESPONSE_BYTES,
    };

    #[test]
    fn simulation_response_requires_explicit_gas_and_error_fields() {
        let result = parse_simulation_response(br#"{"gas-spent":42,"error":null}"#).unwrap();
        assert_eq!(result.gas_spent, 42);
        assert_eq!(result.error, None);
        assert!(parse_simulation_response(br#"{"gas-spent":42}"#).is_err());
    }

    #[test]
    fn propagation_server_failures_remain_outcome_unknown() {
        assert!(
            propagation_status_error(reqwest::StatusCode::BAD_REQUEST, b"invalid")
                .contains("Propagation rejected")
        );
        assert!(propagation_status_error(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            b"lost reply"
        )
        .contains("Propagation outcome unknown"));
    }

    #[test]
    fn transaction_status_query_targets_the_exact_hash() {
        assert_eq!(
            transaction_status_query("aabbcc"),
            r#"query { tx(hash: "aabbcc") { err } }"#
        );
    }

    #[test]
    fn transaction_status_distinguishes_success_failure_and_pending() {
        assert_eq!(
            parse_transaction_status_response(br#"{"data":{"tx":{"err":null}}}"#).unwrap(),
            TransactionStatus::Executed
        );
        assert_eq!(
            parse_transaction_status_response(
                br#"{"data":{"tx":{"err":"Mailbox: insufficient fee credit"}}}"#,
            )
            .unwrap(),
            TransactionStatus::Failed("Mailbox: insufficient fee credit".into())
        );
        assert_eq!(
            parse_transaction_status_response(br#"{"data":{"tx":null}}"#).unwrap(),
            TransactionStatus::NotFound
        );
    }

    #[test]
    fn transaction_status_rejects_graphql_and_malformed_responses() {
        assert!(parse_transaction_status_response(
            br#"{"errors":[{"message":"archive unavailable"}],"data":{"tx":null}}"#
        )
        .unwrap_err()
        .contains("errors"));
        assert!(parse_transaction_status_response(br#"{"data":{}}"#).is_err());
        assert!(parse_transaction_status_response(br#"{"data":{"tx":{}}}"#).is_err());
        assert!(parse_transaction_status_response(br#"{"data":{"tx":{"err":7}}}"#).is_err());
        assert!(parse_transaction_status_response(b"not json").is_err());
    }

    #[test]
    fn transaction_status_body_is_bounded_without_content_length() {
        let mut body = Vec::new();
        append_bounded_chunk(
            &mut body,
            &vec![b'x'; MAX_TRANSACTION_STATUS_RESPONSE_BYTES],
            MAX_TRANSACTION_STATUS_RESPONSE_BYTES,
            "transaction status",
        )
        .unwrap();
        assert!(append_bounded_chunk(
            &mut body,
            b"x",
            MAX_TRANSACTION_STATUS_RESPONSE_BYTES,
            "transaction status",
        )
        .is_err());
    }
}
