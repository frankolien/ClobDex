//! Remembering what was created.
//!
//! `create-market` mints tokens and allocates accounts whose addresses are random; every
//! later command needs them. Rather than making the user paste six pubkeys per call,
//! they are written to `.clob/<cluster>.json` and read back by default.
//!
//! Addresses only. The mint authority keypair is written beside it as a normal Solana
//! keypair file, because that one is a secret and does not belong in a document people
//! copy around.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clob_client::instruction::MarketAddresses;
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

/// Directory holding created-market records. Gitignored.
pub const STORE_DIR: &str = ".clob";

/// A market this CLI created, and everything needed to use it again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketRecord {
    /// The deployed program.
    pub program_id: String,
    /// The market account.
    pub market: String,
    /// Vault holding base deposits.
    pub base_vault: String,
    /// Vault holding quote deposits.
    pub quote_vault: String,
    /// Base token mint.
    pub base_mint: String,
    /// Quote token mint.
    pub quote_mint: String,
    /// The payer's base token account.
    pub payer_base: String,
    /// The payer's quote token account.
    pub payer_quote: String,
}

impl MarketRecord {
    /// Path for a cluster's record, e.g. `.clob/devnet.json`.
    pub fn path(cluster: &str) -> PathBuf {
        Path::new(STORE_DIR).join(format!("{cluster}.json"))
    }

    /// Writes the record, creating the directory if needed.
    pub fn save(&self, cluster: &str) -> Result<PathBuf> {
        let path = Self::path(cluster);
        std::fs::create_dir_all(STORE_DIR)?;
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("cannot write {}", path.display()))?;
        Ok(path)
    }

    /// Reads a previously created market.
    pub fn load(cluster: &str) -> Result<Self> {
        let path = Self::path(cluster);
        let contents = std::fs::read_to_string(&path).with_context(|| {
            format!("no market recorded at {} — run `create-market` first", path.display())
        })?;
        serde_json::from_str(&contents).with_context(|| format!("{} is malformed", path.display()))
    }

    /// The addresses every instruction builder takes.
    pub fn addresses(&self) -> Result<MarketAddresses> {
        Ok(MarketAddresses::new(
            parse(&self.program_id, "program_id")?,
            parse(&self.market, "market")?,
            parse(&self.base_vault, "base_vault")?,
            parse(&self.quote_vault, "quote_vault")?,
        ))
    }

    /// The payer's two token accounts, in the order the builders take them.
    pub fn payer_token_accounts(&self) -> Result<(Pubkey, Pubkey)> {
        Ok((parse(&self.payer_base, "payer_base")?, parse(&self.payer_quote, "payer_quote")?))
    }
}

fn parse(value: &str, field: &str) -> Result<Pubkey> {
    value
        .parse()
        .with_context(|| format!("{field} is not a valid pubkey: {value}"))
}
