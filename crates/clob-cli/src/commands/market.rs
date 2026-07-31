//! Creating a market from nothing, and reading one back.

use anyhow::{Context, Result};
use clob_book::{LotConfig, Side};
use clob_client::setup::{CreateMarketParams, TOKEN_ACCOUNT_LEN, create_market};
use clob_client::state::MarketState;
use clob_program::state::SizeClass;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_system_interface::instruction as system;

use crate::config::write_keypair;
use crate::rpc::Client;
use crate::spl::{self, MINT_LEN};
use crate::store::{MarketRecord, STORE_DIR};

/// A SOL/USDC-shaped market: 9-decimal base, 6-decimal quote.
///
/// One base lot is 0.001 of a base unit and one tick is 0.001 quote units, so a price of
/// 150_000 ticks reads as $150. The tick size divides the base lots per unit exactly,
/// which is the invariant that keeps every fill's quote value a whole number.
const BASE_DECIMALS: u8 = 9;
const QUOTE_DECIMALS: u8 = 6;

fn default_lot_config() -> Result<LotConfig> {
    LotConfig::new(1_000, 1_000, 1_000_000, 1).map_err(|e| anyhow::anyhow!("{e:?}"))
}

/// How much of each token to mint to the payer, in whole units.
const SUPPLY_IN_UNITS: u64 = 1_000_000;

/// Rent for a 19 KB market, two vaults, two mints and two token accounts, plus fees and
/// a margin. Checked before the first transaction rather than discovered before the last.
const MINIMUM_LAMPORTS: u64 = 200_000_000;

/// Creates two mints, funds the payer, and initialises a market over them.
///
/// Three transactions rather than one: mints and token accounts are independent, but the
/// six market-creation instructions must land together, or an allocated-but-uninitialised
/// market account would be left owned by the program with no authority able to finish it.
pub fn create(client: &Client, cluster: &str, taker_fee_bps: u64) -> Result<()> {
    let payer = client.payer_key();
    let lot_config = default_lot_config()?;

    // Three transactions, and the last one allocates a 19 KB account. Running out
    // between them leaves orphaned mints and rent that is awkward to reclaim.
    let balance = client.balance()?;
    println!("payer {payer} ({:.4} SOL)", balance as f64 / 1e9);
    if balance < MINIMUM_LAMPORTS {
        anyhow::bail!(
            "need at least {:.2} SOL to create a market",
            MINIMUM_LAMPORTS as f64 / 1e9
        );
    }

    let base_mint = Keypair::new();
    let quote_mint = Keypair::new();
    let mint_rent = client.rent(MINT_LEN as usize)?;

    println!("creating mints");
    client.send(
        &[
            system::create_account(&payer, &base_mint.pubkey(), mint_rent, MINT_LEN, &spl_program()),
            spl::initialize_mint(&base_mint.pubkey(), &payer, BASE_DECIMALS),
            system::create_account(&payer, &quote_mint.pubkey(), mint_rent, MINT_LEN, &spl_program()),
            spl::initialize_mint(&quote_mint.pubkey(), &payer, QUOTE_DECIMALS),
        ],
        &[&base_mint, &quote_mint],
    )?;

    let payer_base = Keypair::new();
    let payer_quote = Keypair::new();
    let token_rent = client.rent(TOKEN_ACCOUNT_LEN as usize)?;

    println!("funding the payer");
    client.send(
        &[
            system::create_account(&payer, &payer_base.pubkey(), token_rent, TOKEN_ACCOUNT_LEN, &spl_program()),
            spl::initialize_account(&payer_base.pubkey(), &base_mint.pubkey(), &payer),
            system::create_account(&payer, &payer_quote.pubkey(), token_rent, TOKEN_ACCOUNT_LEN, &spl_program()),
            spl::initialize_account(&payer_quote.pubkey(), &quote_mint.pubkey(), &payer),
            spl::mint_to(
                &base_mint.pubkey(),
                &payer_base.pubkey(),
                &payer,
                SUPPLY_IN_UNITS * 10u64.pow(BASE_DECIMALS as u32),
            ),
            spl::mint_to(
                &quote_mint.pubkey(),
                &payer_quote.pubkey(),
                &payer,
                SUPPLY_IN_UNITS * 10u64.pow(QUOTE_DECIMALS as u32),
            ),
        ],
        &[&payer_base, &payer_quote],
    )?;

    let market = Keypair::new();
    let base_vault = Keypair::new();
    let quote_vault = Keypair::new();

    let setup = create_market(&CreateMarketParams {
        program_id: client.program_id,
        payer,
        market: market.pubkey(),
        base_vault: base_vault.pubkey(),
        quote_vault: quote_vault.pubkey(),
        base_mint: base_mint.pubkey(),
        quote_mint: quote_mint.pubkey(),
        authority: payer,
        fee_recipient: payer_quote.pubkey(),
        size_class: SizeClass::Small,
        lot_config,
        taker_fee_bps,
        market_rent_lamports: client.rent(SizeClass::Small.account_len())?,
        vault_rent_lamports: token_rent,
    });

    println!("creating the market");
    let signature = client.send(&setup.instructions, &[&market, &base_vault, &quote_vault])?;

    // The mint authority is the payer, so no separate secret is created here. The mint
    // keypairs are only needed for their signature on allocation, which already
    // happened — but they are worth keeping to identify the tokens later.
    for (name, keypair) in [("base-mint", &base_mint), ("quote-mint", &quote_mint)] {
        write_keypair(
            &std::path::Path::new(STORE_DIR).join(format!("{cluster}-{name}.json")),
            keypair,
        )?;
    }

    let record = MarketRecord {
        program_id: client.program_id.to_string(),
        market: market.pubkey().to_string(),
        base_vault: base_vault.pubkey().to_string(),
        quote_vault: quote_vault.pubkey().to_string(),
        base_mint: base_mint.pubkey().to_string(),
        quote_mint: quote_mint.pubkey().to_string(),
        payer_base: payer_base.pubkey().to_string(),
        payer_quote: payer_quote.pubkey().to_string(),
    };
    let path = record.save(cluster)?;

    println!();
    println!("market    {}", record.market);
    println!("base      {} ({BASE_DECIMALS} decimals)", record.base_mint);
    println!("quote     {} ({QUOTE_DECIMALS} decimals)", record.quote_mint);
    println!("fee       {taker_fee_bps} bps");
    println!("signature {signature}");
    println!("saved to  {}", path.display());
    Ok(())
}

/// Prints a market's book and parameters.
pub fn show(client: &Client, cluster: &str, depth: usize) -> Result<()> {
    let record = MarketRecord::load(cluster)?;
    let addresses = record.addresses()?;

    let data = client
        .account_data(&addresses.market)?
        .context("the market account does not exist on this cluster")?;
    let state = MarketState::decode(&data).map_err(|e| anyhow::anyhow!("{e:?}"))?;

    println!("market {}", record.market);
    println!("fee    {} bps", state.fees().taker_fee_bps);
    // Written by the program on every fill. Zero after a crossing trade means the
    // liquidity was removed without value changing hands — a self-trade.
    println!("earned {} quote lots", state.header.collected_quote_lot_fees.as_u64());
    // Occupied seats, and how many of those hold nothing and could be evicted to make
    // room. An evictable seat is still a claimed seat, so it counts as occupied.
    println!(
        "seats  {} of {} claimed, {} evictable",
        state.traders.len(),
        state.seat_capacity(),
        state.evictable_seats().len()
    );
    println!();

    // Asks printed high-to-low above bids, so the spread sits in the middle the way a
    // trader expects to read it.
    let asks = state.level_two(Side::Ask, depth);
    for level in asks.iter().rev() {
        println!("  ask {:>12}  {:>12}", level.price_in_ticks.as_u64(), level.base_lots.as_u64());
    }
    match state.spread_in_ticks() {
        Some(spread) => println!("      ---- spread {spread} ----"),
        None => println!("      ---- one side empty ----"),
    }
    for level in state.level_two(Side::Bid, depth) {
        println!("  bid {:>12}  {:>12}", level.price_in_ticks.as_u64(), level.base_lots.as_u64());
    }
    Ok(())
}

fn spl_program() -> Pubkey {
    clob_client::instruction::TOKEN_PROGRAM_ID
}
