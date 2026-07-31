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

fn parse(value: &str, field: &str) -> Result<Pubkey> {
    value
        .parse()
        .with_context(|| format!("{field} is not a valid pubkey: {value}"))
}
