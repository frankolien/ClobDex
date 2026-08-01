//! Remembering what was created.
//!
//! `create-market` mints tokens and allocates accounts whose addresses are random; every
//! later command needs them. Rather than making the user paste six pubkeys per call,
//! they are written to `.clob/<cluster>.json` and read back by default.
//!
//! This file is the contract between the tool that creates a market and every tool that
//! trades on it, which is why it lives here rather than in whichever one happened to
//! write it first. Two copies of these field names would be two things to keep in step.
//!
//! Addresses only. The mint authority keypair is written beside it as a normal Solana
//! keypair file, because that one is a secret and does not belong in a document people
//! copy around.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clob_client::instruction::MarketAddresses;
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

/// Directory holding created-market records. Gitignored.
pub const RECORD_DIR: &str = ".clob";

/// A market `create-market` created, and everything needed to use it again.
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
    /// Extra wallets created by `new-trader`, by name.
    ///
    /// A market with one participant can only ever produce self-trades: the taker owns
    /// the liquidity it crosses, no value moves, and no fee is charged. A second wallet
    /// is what makes a real fill possible.
    #[serde(default)]
    pub traders: BTreeMap<String, TraderRecord>,
}

/// A wallet that can trade on this market, and its token accounts.
///
/// The keypair lives beside this file rather than in it — secrets do not belong in a
/// document people copy around.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraderRecord {
    /// The wallet.
    pub pubkey: String,
    /// Its base token account.
    pub base: String,
    /// Its quote token account.
    pub quote: String,
}

impl TraderRecord {
    /// Where this trader's keypair is kept.
    pub fn keypair_path(cluster: &str, name: &str) -> PathBuf {
        Path::new(RECORD_DIR).join(format!("{cluster}-trader-{name}.json"))
    }

    /// Its two token accounts, in the order the builders take them.
    pub fn token_accounts(&self) -> Result<(Pubkey, Pubkey)> {
        Ok((parse(&self.base, "base")?, parse(&self.quote, "quote")?))
    }
}

impl MarketRecord {
    /// Path for a cluster's record, e.g. `.clob/devnet.json`.
    pub fn path(cluster: &str) -> PathBuf {
        Path::new(RECORD_DIR).join(format!("{cluster}.json"))
    }

    /// Writes the record, creating the directory if needed.
    pub fn save(&self, cluster: &str) -> Result<PathBuf> {
        let path = Self::path(cluster);
        std::fs::create_dir_all(RECORD_DIR)?;
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

    /// Token accounts for a named trader, or the payer's when unnamed.
    pub fn token_accounts_for(&self, trader: Option<&str>) -> Result<(Pubkey, Pubkey)> {
        match trader {
            None => self.payer_token_accounts(),
            Some(name) => self
                .traders
                .get(name)
                .with_context(|| format!("no trader named {name} — run `new-trader {name}` first"))?
                .token_accounts(),
        }
    }
}

fn parse(value: &str, field: &str) -> Result<Pubkey> {
    value
        .parse()
        .with_context(|| format!("{field} is not a valid pubkey: {value}"))
}
