//! Devnet operations for a ClobDex market.

mod config;

use anyhow::Result;
use solana_signer::Signer;

use crate::config::Config;

fn main() -> Result<()> {
    let config = Config::load(None)?;
    println!("program {}", config.program_id);
    println!("payer   {}", config.payer.pubkey());
    Ok(())
}
