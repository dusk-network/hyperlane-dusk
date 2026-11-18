//! RUES HTTP client for transaction submission and contract queries.

use dusk_core::signatures::bls::PublicKey as BlsPublicKey;
use dusk_core::transfer::moonlight::AccountData;
use rkyv::ser::serializers::AllocSerializer;
use rkyv::ser::Serializer;
use rkyv::validation::validators::DefaultValidator;
use rkyv::{check_archived_root, Archive, Deserialize, Infallible, Serialize};
use serde_json::Value;

const TRANSFER_CONTRACT: &str = "0100000000000000000000000000000000000000000000000000000000000000";
const MAX_TRANSACTION_STATUS_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_CONTRACT_QUERY_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_ERROR_RESPONSE_BYTES: usize = 64 * 1024;
/// A contract's owner as Rusk reports it: the bytes its deploy named, 96 for
/// a Moonlight account.
const MAX_CONTRACT_OWNER_RESPONSE_BYTES: usize = 64 * 1024;
/// How Rusk's preverify refuses a Moonlight transaction whose nonce is
/// ahead of the account's next one (`TxAcceptanceError::MissingIntermediateNonce`
/// in rusk-private's node/src/mempool.rs). Propagate queues such a
/// transaction instead.
const MISSING_INTERMEDIATE_NONCE: &str = "Missing intermediate nonce";
/// How long a request may take.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// The body of a Rusk node's HTTP 500 answer to a contract query that
/// fails in the VM. A query of a function the contract doesn't export gets
/// it, and so does every other query that fails there, one whose call
/// panics or whose contract doesn't exist among them: the node tells them
/// apart only in its log.
pub const FAILED_QUERY_BODY: &[u8] = br#"{"error":"Internal server error"}"#;
/// How many times [`RuesClient::optional_view`] asks for a view before it
/// takes it as missing, and how long it waits between.
const MISSING_VIEW_TRIES: usize = 2;
const MISSING_VIEW_PAUSE: std::time::Duration = std::time::Duration::from_secs(1);

/// A node's answer to a contract query that came back.
enum QueryReply {
    /// HTTP 2xx, with the archived value the method returned.
    Value(Vec<u8>),
    /// HTTP 500 with [`FAILED_QUERY_BODY`]: the query failed in the VM.
    Failed,
}

pub struct RuesClient {
    client: reqwest::Client,
    base_url: reqwest::Url,
}

/// What the ledger says about a transaction hash. `tx(hash)` only proves
/// inclusion: a block that isn't final yet can still be reverted, which
/// drops its transactions or puts them back in the mempool to run again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionStatus {
    NotFound,
    Executed(Inclusion),
    Failed { error: String, inclusion: Inclusion },
}

/// Where a transaction was included, and the gas it used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inclusion {
    pub block_height: u64,
    pub gas_spent: u64,
}

impl TransactionStatus {
    /// The block the transaction is in, if it is in one.
    pub fn inclusion(&self) -> Option<Inclusion> {
        match self {
            Self::NotFound => None,
            Self::Executed(inclusion) | Self::Failed { inclusion, .. } => Some(*inclusion),
        }
    }
}

/// The block at a height, as `block(height)` reports it: its hash and its
/// consensus label (Accepted, Attested, Confirmed or Final).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockStatus {
    pub hash: String,
    pub label: String,
}

impl BlockStatus {
    /// A final block can't be reverted.
    pub fn is_final(&self) -> bool {
        self.label == "Final"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionStatusQueryError {
    Retryable(String),
    Terminal(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResponseBodyError {
    Transport(String),
    Limit(String),
}

impl ResponseBodyError {
    fn into_transaction_status_error(self) -> TransactionStatusQueryError {
        match self {
            Self::Transport(error) => TransactionStatusQueryError::Retryable(error),
            Self::Limit(error) => TransactionStatusQueryError::Terminal(error),
        }
    }
}

impl core::fmt::Display for ResponseBodyError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Transport(error) | Self::Limit(error) => formatter.write_str(error),
        }
    }
}

impl core::fmt::Display for TransactionStatusQueryError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Retryable(error) | Self::Terminal(error) => formatter.write_str(error),
        }
    }
}

impl RuesClient {
    pub fn new(base_url: &str) -> Result<Self, String> {
        Self::with_timeout(base_url, REQUEST_TIMEOUT)
    }

    /// A client whose requests give up after `timeout`.
    fn with_timeout(base_url: &str, timeout: std::time::Duration) -> Result<Self, String> {
        let base_url = reqwest::Url::parse(base_url).map_err(|_| "Invalid RUES URL".to_owned())?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err("RUES URL must use HTTP or HTTPS".into());
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| format!("Failed to build RUES HTTP client: {}", e.without_url()))?;
        Ok(Self { client, base_url })
    }

    /// A client whose requests give up after `timeout`, for tests of a
    /// node that doesn't answer.
    #[cfg(test)]
    pub fn with_test_timeout(base_url: &str, timeout: std::time::Duration) -> Self {
        Self::with_timeout(base_url, timeout).unwrap()
    }

    fn endpoint(&self, path: &str) -> reqwest::Url {
        let mut url = self.base_url.clone();
        url.set_path(&format!("{}{path}", url.path().trim_end_matches('/')));
        url.set_fragment(None);
        url
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

    /// The view `method` of a contract that may not have it: its value, or
    /// `None` when the contract doesn't export it.
    ///
    /// A node answers a query of a function the contract doesn't export as
    /// it answers every query that fails in the VM ([`FAILED_QUERY_BODY`]),
    /// so that answer alone doesn't show the view is missing. It counts as
    /// missing only when it comes back every one of `MISSING_VIEW_TRIES`
    /// times, a second apart, and after each the contract answers `control`
    /// with an amount: `control` is a query of a view it has that returns a
    /// `u64`, with its argument archived. The node then runs the contract's
    /// code, and only this view fails. The caller must ask for a view that
    /// can't fail on a contract that has it, one that reads state with a
    /// well-formed argument.
    ///
    /// Any other answer is an error: another status or body, no answer in
    /// time, a value that doesn't decode as `O`, or a control that fails or
    /// answers anything but a `u64`, an empty reply among them.
    pub async fn optional_view<I, O>(
        &self,
        contract_id: &[u8; 32],
        method: &str,
        args: &I,
        control: (&str, &[u8]),
    ) -> Result<Option<O>, String>
    where
        I: Serialize<AllocSerializer<256>>,
        O: Archive,
        O::Archived: Deserialize<O, Infallible> + for<'b> rkyv::CheckBytes<DefaultValidator<'b>>,
    {
        let body = rkyv_serialize(args);
        let hex_id = hex::encode(contract_id);
        for attempt in 0..MISSING_VIEW_TRIES {
            if attempt > 0 && !cfg!(test) {
                tokio::time::sleep(MISSING_VIEW_PAUSE).await;
            }
            match self.contract_query_reply(&hex_id, method, &body).await? {
                QueryReply::Value(value) => {
                    return rkyv_deserialize(&value).map(Some).map_err(|error| {
                        format!("Query {hex_id}/{method} answered, but not with the expected value: {error}")
                    });
                }
                QueryReply::Failed => {
                    let (control_method, control_args) = control;
                    let amount = self
                        .raw_contract_query(&hex_id, control_method, control_args)
                        .await
                        .map_err(|error| {
                            format!(
                                "Query {hex_id}/{method} failed, and so did {control_method}, \
                                 which it has: {error}"
                            )
                        })?;
                    // An archived u64 is exactly its 8 bytes. rkyv reads a
                    // root from the end of its buffer, so a longer reply
                    // would decode its last 8 bytes; it isn't an amount.
                    let decoded = if amount.len() == core::mem::size_of::<u64>() {
                        rkyv_deserialize::<u64>(&amount)
                    } else {
                        Err(format!("{} bytes, not 8", amount.len()))
                    };
                    decoded.map_err(|error| {
                        format!(
                            "Query {hex_id}/{method} failed, and {control_method}, which it has, \
                             answered, but not with an amount: {error}"
                        )
                    })?;
                }
            }
        }
        Ok(None)
    }

    /// A contract query with an argument already archived, and its reply as
    /// the node sends it: the archived value the method returned.
    pub async fn contract_query_raw(
        &self,
        contract_id: &[u8; 32],
        method: &str,
        args: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.raw_contract_query(&hex::encode(contract_id), method, args)
            .await
    }

    /// Preverify then propagate a serialized transaction.
    ///
    /// With `queue_future_nonce`, preverify refusing the transaction only
    /// because its nonce is ahead of the account's next one doesn't stop it:
    /// propagate holds such a transaction until the transactions with the
    /// nonces before it arrive, and checks it then.
    pub async fn propagate_tx(
        &self,
        tx_bytes: &[u8],
        queue_future_nonce: bool,
    ) -> Result<(), String> {
        // Preverify first
        let url = self.endpoint("/on/transactions/preverify");
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/octet-stream")
            .body(tx_bytes.to_vec())
            .send()
            .await
            .map_err(|e| format!("Preverify failed before propagation: {}", e.without_url()))?;

        let status = response.status();
        if !status.is_success() {
            let body = read_response_body(response, MAX_ERROR_RESPONSE_BYTES, "preverify")
                .await
                .map_err(|error| format!("Preverify failed before propagation: {error}"))?;
            let body = String::from_utf8_lossy(&body);
            if !(queue_future_nonce && body.contains(MISSING_INTERMEDIATE_NONCE)) {
                return Err(format!(
                    "Preverify rejected before propagation ({status}): {body}"
                ));
            }
        }

        // Propagate
        let url = self.endpoint("/on/transactions/propagate");
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/octet-stream")
            .body(tx_bytes.to_vec())
            .send()
            .await
            .map_err(|e| format!("Propagation outcome unknown: {}", e.without_url()))?;

        let status = response.status();
        if !status.is_success() {
            let body = read_response_body(response, MAX_ERROR_RESPONSE_BYTES, "propagate")
                .await
                .map_err(|error| format!("Propagation outcome unknown: {error}"))?;
            return Err(propagation_status_error(status, &body));
        }
        Ok(())
    }

    /// Query the persisted execution result for an exact transaction hash.
    pub async fn query_transaction_status(
        &self,
        tx_id: &str,
    ) -> Result<TransactionStatus, TransactionStatusQueryError> {
        let url = self.endpoint("/graphql");
        let query = transaction_status_query(tx_id);
        let response = self
            .client
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({ "query": query }))
                    .expect("GraphQL request serialization should not fail"),
            )
            .send()
            .await
            .map_err(|e| {
                TransactionStatusQueryError::Retryable(format!("HTTP error: {}", e.without_url()))
            })?;

        let status = response.status();
        let body = read_response_body(
            response,
            MAX_TRANSACTION_STATUS_RESPONSE_BYTES,
            "transaction status",
        )
        .await
        .map_err(ResponseBodyError::into_transaction_status_error)?;
        if !status.is_success() {
            let error = format!(
                "Transaction status query failed ({status}): {}",
                String::from_utf8_lossy(&body)
            );
            if status.is_server_error() || status.as_u16() == 429 {
                return Err(TransactionStatusQueryError::Retryable(error));
            }
            return Err(TransactionStatusQueryError::Terminal(error));
        }

        parse_transaction_status_response(&body).map_err(TransactionStatusQueryError::Terminal)
    }

    /// The hash and label of the block the node has at `height`, or `None`
    /// if it has none there (yet, or after a revert).
    pub async fn query_block_status(
        &self,
        height: u64,
    ) -> Result<Option<BlockStatus>, TransactionStatusQueryError> {
        let url = self.endpoint("/graphql");
        let response = self
            .client
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({ "query": block_status_query(height) }))
                    .expect("GraphQL request serialization should not fail"),
            )
            .send()
            .await
            .map_err(|e| {
                TransactionStatusQueryError::Retryable(format!("HTTP error: {}", e.without_url()))
            })?;
        let status = response.status();
        let body = read_response_body(
            response,
            MAX_TRANSACTION_STATUS_RESPONSE_BYTES,
            "block status",
        )
        .await
        .map_err(ResponseBodyError::into_transaction_status_error)?;
        if !status.is_success() {
            let error = format!(
                "Block status query failed ({status}): {}",
                String::from_utf8_lossy(&body)
            );
            if status.is_server_error() || status.as_u16() == 429 {
                return Err(TransactionStatusQueryError::Retryable(error));
            }
            return Err(TransactionStatusQueryError::Terminal(error));
        }
        parse_block_status_response(&body)
    }

    /// Contract events emitted in the block at `height`, from the node's
    /// archive. Each item has `source` (contract ID hex), `topic`, `data`
    /// (hex) and `reverted`.
    pub async fn block_events(&self, height: u64) -> Result<Vec<Value>, String> {
        let url = self.endpoint("/graphql");
        let query = format!("query {{ contractEvents(height: {height}) {{ json }} }}");
        let response = self
            .client
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({ "query": query }))
                    .expect("GraphQL request serialization should not fail"),
            )
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e.without_url()))?;
        let status = response.status();
        let body = read_response_body(response, MAX_CONTRACT_QUERY_RESPONSE_BYTES, "block events")
            .await
            .map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!(
                "Block events query failed ({status}): {}",
                String::from_utf8_lossy(&body)
            ));
        }
        parse_block_events_response(&body)
    }

    /// Run a GraphQL query and return its `data`.
    async fn graphql_data(&self, query: &str, what: &str) -> Result<Value, String> {
        let url = self.endpoint("/graphql");
        let response = self
            .client
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({ "query": query }))
                    .expect("GraphQL request serialization should not fail"),
            )
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e.without_url()))?;
        let status = response.status();
        let body = read_response_body(response, MAX_TRANSACTION_STATUS_RESPONSE_BYTES, what)
            .await
            .map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!(
                "The {what} query failed ({status}): {}",
                String::from_utf8_lossy(&body)
            ));
        }
        let payload: Value =
            serde_json::from_slice(&body).map_err(|e| format!("Invalid {what} response: {e}"))?;
        if let Some(errors) = payload.get("errors") {
            return Err(format!("The {what} query failed: {errors}"));
        }
        payload
            .get("data")
            .cloned()
            .ok_or_else(|| format!("The {what} response has no data"))
    }

    /// The node's last block: its height and hash.
    pub async fn tip_block(&self) -> Result<(u64, String), String> {
        let data = self
            .graphql_data(
                "query { block(height: -1) { header { height hash } } }",
                "tip block",
            )
            .await?;
        let height = data.pointer("/block/header/height").and_then(Value::as_u64);
        let hash = data.pointer("/block/header/hash").and_then(Value::as_str);
        match (height, hash) {
            (Some(height), Some(hash)) => Ok((height, hash.to_owned())),
            _ => Err(format!(
                "The tip block response has no height and hash: {data}"
            )),
        }
    }

    /// Whether the node holds the transaction in its mempool.
    pub async fn in_mempool(&self, tx_id: &str) -> Result<bool, String> {
        let query = format!(r#"query {{ mempoolTx(hash: "{tx_id}") {{ id }} }}"#);
        let data = self.graphql_data(&query, "mempool transaction").await?;
        match data.get("mempoolTx") {
            Some(Value::Null) => Ok(false),
            Some(Value::Object(_)) => Ok(true),
            _ => Err(format!(
                "The mempool transaction response is unexpected: {data}"
            )),
        }
    }

    /// Returns whether a contract exists on-chain, using VM metadata.
    ///
    /// This does **not** invoke contract code.
    pub async fn contract_exists(&self, contract_hex: &str) -> Result<bool, String> {
        Ok(self.contract_owner(contract_hex).await?.is_some())
    }

    /// The owner Rusk recorded for a contract, the bytes the deploy that
    /// created it named, or `None` when no contract has the ID. Read from
    /// VM metadata; this does **not** invoke contract code.
    pub async fn contract_owner(&self, contract_hex: &str) -> Result<Option<Vec<u8>>, String> {
        // RUES requires a non-empty topic; "owner" is conventional here.
        let url = self.endpoint(&format!("/on/contract_owner:{contract_hex}/owner"));
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/octet-stream")
            .body(Vec::new())
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e.without_url()))?;

        let status = response.status();
        if status.is_success() {
            // A binary request gets the owner's raw bytes back.
            let owner = read_response_body(
                response,
                MAX_CONTRACT_OWNER_RESPONSE_BYTES,
                "contract owner",
            )
            .await
            .map_err(|error| error.to_string())?;
            return Ok(Some(owner));
        }

        let body = read_response_body(response, MAX_ERROR_RESPONSE_BYTES, "contract existence")
            .await
            .map_err(|error| error.to_string())?;
        let body = String::from_utf8_lossy(&body);

        // Rusk reports non-existent contracts with a descriptive error body.
        // The status code and exact wording have varied across versions.
        let body_lc = body.to_lowercase();
        if (status.as_u16() == 404 || status.as_u16() == 500)
            && (body.contains("ContractDoesNotExist")
                || body_lc.contains("contract does not exist")
                || body_lc.contains("contract owner not found"))
        {
            return Ok(None);
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
        match self
            .contract_query_reply(contract_hex, method, body)
            .await?
        {
            QueryReply::Value(value) => Ok(value),
            QueryReply::Failed => Err(format!(
                "Query {contract_hex}/{method} failed (500 Internal Server Error): {}",
                String::from_utf8_lossy(FAILED_QUERY_BODY)
            )),
        }
    }

    /// A contract query's answer: its value, or that it failed in the VM.
    /// Any other answer, or none, is an error.
    async fn contract_query_reply(
        &self,
        contract_hex: &str,
        method: &str,
        body: &[u8],
    ) -> Result<QueryReply, String> {
        let url = self.endpoint(&format!("/on/contracts:{contract_hex}/{method}"));
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/octet-stream")
            .body(body.to_vec())
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e.without_url()))?;

        let status = response.status();
        let max_bytes = if status.is_success() {
            MAX_CONTRACT_QUERY_RESPONSE_BYTES
        } else {
            MAX_ERROR_RESPONSE_BYTES
        };
        let response_body = read_response_body(response, max_bytes, "contract query")
            .await
            .map_err(|error| error.to_string())?;
        if status.is_success() {
            return Ok(QueryReply::Value(response_body));
        }
        if status == reqwest::StatusCode::INTERNAL_SERVER_ERROR
            && response_body == FAILED_QUERY_BODY
        {
            return Ok(QueryReply::Failed);
        }
        Err(format!(
            "Query {contract_hex}/{method} failed ({status}): {}",
            String::from_utf8_lossy(&response_body)
        ))
    }
}

fn propagation_status_error(status: reqwest::StatusCode, body: &[u8]) -> String {
    let detail = String::from_utf8_lossy(body);
    format!("Propagation outcome unknown ({status}): {detail}")
}

async fn read_response_body(
    mut response: reqwest::Response,
    max_bytes: usize,
    context: &str,
) -> Result<Vec<u8>, ResponseBodyError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(ResponseBodyError::Limit(format!(
            "{context} response exceeds {max_bytes} bytes"
        )));
    }

    let mut body = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or_default()
            .min(max_bytes as u64) as usize,
    );
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        ResponseBodyError::Transport(format!(
            "Failed to read {context} response: {}",
            e.without_url()
        ))
    })? {
        append_bounded_chunk(&mut body, &chunk, max_bytes, context)?;
    }
    Ok(body)
}

fn append_bounded_chunk(
    body: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: usize,
    context: &str,
) -> Result<(), ResponseBodyError> {
    if chunk.len() > max_bytes.saturating_sub(body.len()) {
        return Err(ResponseBodyError::Limit(format!(
            "{context} response exceeds {max_bytes} bytes"
        )));
    }
    body.extend_from_slice(chunk);
    Ok(())
}
fn parse_block_events_response(body: &[u8]) -> Result<Vec<Value>, String> {
    let payload: Value =
        serde_json::from_slice(body).map_err(|e| format!("Invalid block events response: {e}"))?;
    if let Some(errors) = payload.get("errors").filter(|e| !e.is_null()) {
        return Err(format!(
            "Block events query returned errors (the node needs the archive): {errors}"
        ));
    }
    let json = payload
        .pointer("/data/contractEvents/json")
        .ok_or_else(|| "Block events response is missing data.contractEvents.json".to_string())?;
    let events = json.get("events").unwrap_or(json);
    match events {
        Value::Array(items) => Ok(items.clone()),
        Value::Null => Ok(Vec::new()),
        _ => Err("Block events response is not a list".into()),
    }
}

fn transaction_status_query(tx_id: &str) -> String {
    format!(r#"query {{ tx(hash: "{tx_id}") {{ err gasSpent blockHeight }} }}"#)
}

fn block_status_query(height: u64) -> String {
    format!("query {{ block(height: {height}) {{ header {{ hash }} status }} }}")
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

    let number = |field: &str| {
        transaction
            .get(field)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("Transaction status response has no numeric data.tx.{field}"))
    };
    let error = match transaction.get("err") {
        Some(Value::Null) => None,
        Some(Value::String(error)) => Some(error.clone()),
        Some(other) => {
            return Err(format!(
                "Transaction status response has invalid data.tx.err: {other}"
            ))
        }
        None => return Err("Transaction status response is missing data.tx.err".into()),
    };
    let inclusion = Inclusion {
        block_height: number("blockHeight")?,
        gas_spent: number("gasSpent")?,
    };
    Ok(match error {
        None => TransactionStatus::Executed(inclusion),
        Some(error) => TransactionStatus::Failed { error, inclusion },
    })
}

/// GraphQL errors count as retryable: the node resolves the block and its
/// label separately, and a block reverted in between fails the label.
fn parse_block_status_response(
    body: &[u8],
) -> Result<Option<BlockStatus>, TransactionStatusQueryError> {
    let terminal = TransactionStatusQueryError::Terminal;
    let payload: Value = serde_json::from_slice(body)
        .map_err(|e| terminal(format!("Invalid block status response: {e}")))?;
    if let Some(errors) = payload.get("errors") {
        let contains_errors = match errors {
            Value::Null => false,
            Value::Array(items) => !items.is_empty(),
            _ => true,
        };
        if contains_errors {
            return Err(TransactionStatusQueryError::Retryable(format!(
                "Block status query returned errors: {errors}"
            )));
        }
    }
    let block = payload
        .pointer("/data/block")
        .ok_or_else(|| terminal("Block status response is missing data.block".into()))?;
    if block.is_null() {
        return Ok(None);
    }
    let hash = block
        .pointer("/header/hash")
        .and_then(Value::as_str)
        .ok_or_else(|| terminal("Block status response has no data.block.header.hash".into()))?;
    // The label is serialized as {"Final": n}, or a bare string.
    let label = match block.get("status") {
        Some(Value::String(label)) => label.clone(),
        Some(Value::Object(label)) if label.len() == 1 => {
            label.keys().next().cloned().unwrap_or_default()
        }
        _ => {
            return Err(terminal(
                "Block status response has no valid data.block.status".into(),
            ))
        }
    };
    Ok(Some(BlockStatus {
        hash: hash.to_owned(),
        label,
    }))
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
    #[tokio::test]
    async fn request_routes_preserve_path_query_and_ignore_fragment() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        for suffix in [
            "/rpc",
            "/rpc/",
            "/rpc?token=fixture%2Fvalue",
            "/rpc#label",
            "/rpc/?token=fixture%2Fvalue#label",
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let worker = std::thread::spawn(move || {
                let mut paths = Vec::new();
                for body in [
                    vec![7],
                    br#"{"data":{"tx":{"err":null,"gasSpent":21,"blockHeight":9}}}"#.to_vec(),
                    br#"{"data":{"block":{"header":{"hash":"ab"},"status":{"Final":1}}}}"#.to_vec(),
                    vec![],
                    vec![],
                    vec![],
                ] {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                        .unwrap();
                    let mut reader = BufReader::new(&mut stream);
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    paths.push(first.split_whitespace().nth(1).unwrap().to_owned());
                    let mut length = 0;
                    loop {
                        let mut header = String::new();
                        assert_ne!(reader.read_line(&mut header).unwrap(), 0);
                        if header == "\r\n" {
                            break;
                        }
                        if let Some(value) =
                            header.to_ascii_lowercase().strip_prefix("content-length:")
                        {
                            length = value.trim().parse::<usize>().unwrap();
                        }
                    }
                    let mut input = vec![0; length];
                    reader.read_exact(&mut input).unwrap();
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(&body).unwrap();
                }
                paths
            });
            let client = super::RuesClient::new(&format!("http://{address}{suffix}")).unwrap();
            assert_eq!(client.query_chain_id().await.unwrap(), 7);
            assert_eq!(
                client
                    .query_transaction_status(&"ab".repeat(32))
                    .await
                    .unwrap(),
                super::TransactionStatus::Executed(super::Inclusion {
                    block_height: 9,
                    gas_spent: 21
                })
            );
            assert!(client
                .query_block_status(9)
                .await
                .unwrap()
                .unwrap()
                .is_final());
            assert!(client.contract_exists(&"02".repeat(32)).await.unwrap());
            client.propagate_tx(&[1], false).await.unwrap();
            let query = if suffix.contains('?') {
                "?token=fixture%2Fvalue"
            } else {
                ""
            };
            assert_eq!(worker.join().unwrap(), vec![
                format!("/rpc/on/contracts:0100000000000000000000000000000000000000000000000000000000000000/chain_id{query}"),
                format!("/rpc/graphql{query}"),
                format!("/rpc/graphql{query}"),
                format!("/rpc/on/contract_owner:{}/owner{query}", "02".repeat(32)),
                format!("/rpc/on/transactions/preverify{query}"),
                format!("/rpc/on/transactions/propagate{query}"),
            ], "base suffix: {suffix}");
        }
    }

    use super::{
        append_bounded_chunk, block_status_query, parse_block_status_response,
        parse_transaction_status_response, propagation_status_error, transaction_status_query,
        BlockStatus, Inclusion, ResponseBodyError, TransactionStatus, TransactionStatusQueryError,
        MAX_TRANSACTION_STATUS_RESPONSE_BYTES,
    };

    #[tokio::test]
    async fn transport_errors_do_not_expose_rpc_credentials_or_paths() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client = super::RuesClient::new(&format!(
            "http://rpc-user:rpc-password@{address}/rpc-path-sentinel?key=query-sentinel"
        ))
        .unwrap();
        let errors = [
            client.query_chain_id().await.unwrap_err(),
            client.contract_exists(&"02".repeat(32)).await.unwrap_err(),
            client.propagate_tx(&[0], false).await.unwrap_err(),
            client
                .query_transaction_status(&"ab".repeat(32))
                .await
                .unwrap_err()
                .to_string(),
            client.query_block_status(9).await.unwrap_err().to_string(),
        ];
        for error in errors {
            for secret in [
                "rpc-user",
                "rpc-password",
                "rpc-path-sentinel",
                "query-sentinel",
            ] {
                assert!(
                    !error.contains(secret),
                    "RPC diagnostic retained {secret}: {error}"
                );
            }
            assert!(!error.contains("http://"));
        }
    }

    #[test]
    fn every_propagation_non_success_remains_outcome_unknown() {
        for status in [
            reqwest::StatusCode::BAD_REQUEST,
            reqwest::StatusCode::REQUEST_TIMEOUT,
            reqwest::StatusCode::CONFLICT,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert!(propagation_status_error(status, b"lost reply")
                .contains("Propagation outcome unknown"));
        }
    }

    #[test]
    fn transaction_status_query_targets_the_exact_hash() {
        assert_eq!(
            transaction_status_query("aabbcc"),
            r#"query { tx(hash: "aabbcc") { err gasSpent blockHeight } }"#
        );
        assert_eq!(
            block_status_query(1234),
            "query { block(height: 1234) { header { hash } status } }"
        );
    }

    #[test]
    fn transaction_status_distinguishes_success_failure_and_pending() {
        let inclusion = Inclusion {
            block_height: 1234,
            gas_spent: 2_140_000,
        };
        assert_eq!(
            parse_transaction_status_response(
                br#"{"data":{"tx":{"err":null,"gasSpent":2140000,"blockHeight":1234}}}"#
            )
            .unwrap(),
            TransactionStatus::Executed(inclusion)
        );
        assert_eq!(
            parse_transaction_status_response(
                br#"{"data":{"tx":{"err":"Mailbox: insufficient fee credit","gasSpent":2140000,"blockHeight":1234}}}"#,
            )
            .unwrap(),
            TransactionStatus::Failed {
                error: "Mailbox: insufficient fee credit".into(),
                inclusion
            }
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
        // Inclusion without a block height can't be checked for finality.
        assert!(
            parse_transaction_status_response(br#"{"data":{"tx":{"err":null}}}"#)
                .unwrap_err()
                .contains("blockHeight")
        );
        assert!(parse_transaction_status_response(
            br#"{"data":{"tx":{"err":null,"gasSpent":1,"blockHeight":-1}}}"#
        )
        .is_err());
        assert!(parse_transaction_status_response(b"not json").is_err());
    }

    #[test]
    fn block_status_reads_the_label_rusk_serializes() {
        let block = |label: &str| {
            parse_block_status_response(
                format!(
                    r#"{{"data":{{"block":{{"header":{{"hash":"ab"}},"status":{{"{label}":2}}}}}}}}"#
                )
                .as_bytes(),
            )
            .unwrap()
            .unwrap()
        };
        assert_eq!(
            block("Final"),
            BlockStatus {
                hash: "ab".into(),
                label: "Final".into()
            }
        );
        assert!(block("Final").is_final());
        for label in ["Accepted", "Attested", "Confirmed"] {
            assert!(!block(label).is_final(), "{label}");
        }
        assert!(parse_block_status_response(
            br#"{"data":{"block":{"header":{"hash":"ab"},"status":"Final"}}}"#
        )
        .unwrap()
        .unwrap()
        .is_final());
        assert_eq!(
            parse_block_status_response(br#"{"data":{"block":null}}"#).unwrap(),
            None
        );
    }

    #[test]
    fn block_status_retries_graphql_errors_and_rejects_malformed_responses() {
        assert!(matches!(
            parse_block_status_response(
                br#"{"errors":[{"message":"Label not found"}],"data":{"block":null}}"#
            ),
            Err(TransactionStatusQueryError::Retryable(_))
        ));
        for body in [
            &b"not json"[..],
            br#"{"data":{}}"#,
            br#"{"data":{"block":{"status":{"Final":1}}}}"#,
            br#"{"data":{"block":{"header":{"hash":"ab"}}}}"#,
            br#"{"data":{"block":{"header":{"hash":"ab"},"status":{"Final":1,"Accepted":1}}}}"#,
        ] {
            assert!(
                matches!(
                    parse_block_status_response(body),
                    Err(TransactionStatusQueryError::Terminal(_))
                ),
                "{}",
                String::from_utf8_lossy(body)
            );
        }
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
        let error = append_bounded_chunk(
            &mut body,
            b"x",
            MAX_TRANSACTION_STATUS_RESPONSE_BYTES,
            "transaction status",
        )
        .unwrap_err();
        assert!(matches!(error, ResponseBodyError::Limit(_)));
        assert!(matches!(
            error.into_transaction_status_error(),
            super::TransactionStatusQueryError::Terminal(_)
        ));
    }
}
