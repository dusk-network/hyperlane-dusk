//! RUES HTTP client for E2E testing.
//!
//! Mirrors the same serialization protocol as the hyperlane-dusk agent crate,
//! validating that rkyv serialization works against a live Rusk node.

use dusk_core::signatures::bls::PublicKey as BlsPublicKey;
use dusk_core::transfer::moonlight::AccountData;
use rkyv::ser::serializers::AllocSerializer;
use rkyv::ser::Serializer;
use rkyv::validation::validators::DefaultValidator;
use rkyv::{check_archived_root, Archive, Deserialize, Infallible, Serialize};

const TRANSFER_CONTRACT: &str = "0100000000000000000000000000000000000000000000000000000000000000";

pub struct RuesClient {
    client: reqwest::Client,
    base_url: String,
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

    /// Preverify a transaction (validates against current state).
    pub async fn preverify_tx(&self, tx_bytes: &[u8]) -> Result<(), String> {
        let url = format!("{}/on/transactions/preverify", self.base_url);
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(tx_bytes.to_vec())
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Preverify failed ({status}): {body}"));
        }
        Ok(())
    }

    /// Propagate a serialized transaction (preverify first).
    pub async fn propagate_tx(&self, tx_bytes: &[u8]) -> Result<(), String> {
        // Preverify first to catch errors early
        self.preverify_tx(tx_bytes).await?;

        let url = format!("{}/on/transactions/propagate", self.base_url);
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(tx_bytes.to_vec())
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Propagate failed ({status}): {body}"));
        }
        Ok(())
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
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Query {contract_hex}/{method} failed ({status}): {body}"
            ));
        }

        Ok(response.bytes().await.map_err(|e| format!("{e}"))?.to_vec())
    }
}

fn rkyv_serialize<T>(value: &T) -> Vec<u8>
where
    T: Serialize<AllocSerializer<256>>,
{
    let mut serializer = AllocSerializer::<256>::default();
    serializer
        .serialize_value(value)
        .expect("rkyv serialization should not fail");
    serializer.into_serializer().into_inner().to_vec()
}

fn rkyv_deserialize<T>(bytes: &[u8]) -> Result<T, String>
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
