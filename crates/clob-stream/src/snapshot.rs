//! Fetching the current state of every market before streaming starts.
//!
//! Without this, the first update for a market only establishes a baseline and derives
//! nothing — so every restart silently loses the first transaction on every market. That
//! is correct (reporting the whole resting book as newly posted would be worse) but it
//! is still a hole, and one that grows with how often the process restarts.
//!
//! Two JSON-RPC methods are all this needs, so it speaks JSON-RPC directly rather than
//! pulling in the whole Agave client stack for one call — the same trade `clob-client`
//! makes by hand-encoding a token instruction instead of depending on `spl-token`.

use anyhow::{Context, Result, bail};
use base64::Engine;
use serde_json::{Value, json};
use solana_pubkey::Pubkey;

/// A market as it stood when the snapshot was taken.
pub struct Account {
    /// Which market.
    pub market: Pubkey,
    /// Its raw account data.
    pub data: Vec<u8>,
}

/// Every market owned by the program, and the slot the answer was true at.
pub struct Snapshot {
    /// The markets.
    pub accounts: Vec<Account>,
    /// Slot the RPC node answered from.
    ///
    /// Load-bearing: an account update older than this describes a state the snapshot
    /// already supersedes, and using it as a baseline would walk the book backwards.
    pub slot: u64,
}

/// A JSON-RPC endpoint.
pub struct Rpc {
    url: String,
    client: reqwest::Client,
}

impl Rpc {
    /// Points at an HTTP endpoint.
    pub fn new(url: String) -> Self {
        Self {
            url,
            client: reqwest::Client::new(),
        }
    }

    /// Reads it out of the environment.
    pub fn from_env() -> Result<Self> {
        Ok(Self::new(
            std::env::var("RPC_URL").context("RPC_URL is not set — see .env.example")?,
        ))
    }

    /// Every account owned by `program_id`, with the slot the answer is true at.
    ///
    /// `withContext` is what makes the slot available; without it the accounts arrive
    /// with no way to tell how stale they are.
    pub async fn program_accounts(&self, program_id: &Pubkey, finalized: bool) -> Result<Snapshot> {
        let commitment = if finalized { "finalized" } else { "confirmed" };
        let response = self
            .call(
                "getProgramAccounts",
                json!([
                    program_id.to_string(),
                    { "encoding": "base64", "withContext": true, "commitment": commitment }
                ]),
            )
            .await?;

        let slot = response["context"]["slot"]
            .as_u64()
            .context("the RPC response carried no slot")?;
        let entries = response["value"]
            .as_array()
            .context("the RPC response carried no accounts")?;

        let mut accounts = Vec::with_capacity(entries.len());
        for entry in entries {
            let market: Pubkey = entry["pubkey"]
                .as_str()
                .context("an account had no pubkey")?
                .parse()
                .context("an account pubkey did not parse")?;

            // [data, encoding] — the encoding is the one that was asked for.
            let encoded = entry["account"]["data"][0]
                .as_str()
                .context("an account had no data")?;
            let data = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .context("account data was not base64")?;

            accounts.push(Account { market, data });
        }

        Ok(Snapshot { accounts, slot })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let response: Value = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("{method} could not reach the RPC endpoint"))?
            .json()
            .await
            .with_context(|| format!("{method} returned something that was not JSON"))?;

        if let Some(error) = response.get("error") {
            bail!("{method} failed: {error}");
        }
        response
            .get("result")
            .cloned()
            .with_context(|| format!("{method} returned no result"))
    }
}
