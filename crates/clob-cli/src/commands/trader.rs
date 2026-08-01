//! Adding a second wallet to a market.
//!
//! A market with one participant cannot produce a real trade. The taker owns every
//! resting order it crosses, so the program removes the liquidity under its self-trade
//! policy, no value moves, and no fee is charged — which is correct, and useless as
//! test data for anything that reads a tape.

use anyhow::{Result, bail};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_system_interface::instruction as system;

use crate::config::write_keypair;
use crate::rpc::Client;
use crate::spl;
use crate::store::{MarketRecord, TraderRecord};
use clob_client::instruction::TOKEN_PROGRAM_ID;
use clob_client::setup::TOKEN_ACCOUNT_LEN;

/// SOL transferred to a new trader, enough for a few hundred transactions.
const FUNDING_LAMPORTS: u64 = 100_000_000;

/// Creates a wallet, funds it with SOL and tokens, and records it.
///
/// The payer keeps the mint authority, so this mints directly rather than transferring
/// — one fewer moving part, and the balances start round.
pub fn create(client: &Client, cluster: &str, name: &str, units: u64) -> Result<()> {
    let mut record = MarketRecord::load(cluster)?;
    if record.traders.contains_key(name) {
        bail!("a trader named {name} already exists on {cluster}");
    }

    let payer = client.payer_key();
    let base_mint: Pubkey = record.base_mint.parse()?;
    let quote_mint: Pubkey = record.quote_mint.parse()?;

    let trader = Keypair::new();
    let base = Keypair::new();
    let quote = Keypair::new();
    let token_rent = client.rent(TOKEN_ACCOUNT_LEN as usize)?;

    client.send(
        &[
            system::transfer(&payer, &trader.pubkey(), FUNDING_LAMPORTS),
            system::create_account(&payer, &base.pubkey(), token_rent, TOKEN_ACCOUNT_LEN, &TOKEN_PROGRAM_ID),
            spl::initialize_account(&base.pubkey(), &base_mint, &trader.pubkey()),
            system::create_account(&payer, &quote.pubkey(), token_rent, TOKEN_ACCOUNT_LEN, &TOKEN_PROGRAM_ID),
            spl::initialize_account(&quote.pubkey(), &quote_mint, &trader.pubkey()),
            spl::mint_to(&base_mint, &base.pubkey(), &payer, units * 1_000_000_000),
            spl::mint_to(&quote_mint, &quote.pubkey(), &payer, units * 1_000_000),
        ],
        &[&base, &quote],
    )?;

    let path = TraderRecord::keypair_path(cluster, name);
    write_keypair(&path, &trader)?;

    record.traders.insert(
        name.to_string(),
        TraderRecord {
            pubkey: trader.pubkey().to_string(),
            base: base.pubkey().to_string(),
            quote: quote.pubkey().to_string(),
        },
    );
    record.save(cluster)?;

    println!("trader  {name}");
    println!("wallet  {}", trader.pubkey());
    println!("funded  {:.3} SOL, {units} base units, {units} quote units", FUNDING_LAMPORTS as f64 / 1e9);
    println!("keypair {}", path.display());
    Ok(())
}
