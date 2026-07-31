//! Taking a seat, funding it, and trading.

use anyhow::Result;
use clob_book::{BaseLots, QuoteLots, Side, Ticks};
use clob_client::instruction::{self, Receipt};

use crate::rpc::Client;
use crate::store::MarketRecord;

/// Claims a seat and deposits funds in one transaction.
///
/// `claim_seat` is idempotent, so this is safe to repeat and does not need the market
/// read first to find out whether a seat already exists.
pub fn fund(client: &Client, cluster: &str, base_lots: u64, quote_lots: u64) -> Result<()> {
    let record = MarketRecord::load(cluster)?;
    let addresses = record.addresses()?;
    let (payer_base, payer_quote) = record.payer_token_accounts()?;
    let trader = client.payer_key();

    let signature = client.send(
        &[
            instruction::claim_seat(&addresses, &trader),
            instruction::deposit(
                &addresses,
                &trader,
                &payer_base,
                &payer_quote,
                BaseLots(base_lots),
                QuoteLots(quote_lots),
            ),
        ],
        &[],
    )?;

    println!("seat funded with {base_lots} base lots and {quote_lots} quote lots");
    println!("signature {signature}");
    Ok(())
}

/// Places one limit order.
pub fn place(
    client: &Client,
    cluster: &str,
    side: Side,
    price_in_ticks: u64,
    base_lots: u64,
    receipt: bool,
) -> Result<()> {
    let record = MarketRecord::load(cluster)?;
    let addresses = record.addresses()?;
    let packet = instruction::limit(side, Ticks(price_in_ticks), BaseLots(base_lots));

    let signature = client.send(
        &[instruction::place_order(
            &addresses,
            &client.payer_key(),
            &packet,
            if receipt { Receipt::On } else { Receipt::Off },
        )],
        &[],
    )?;

    println!("{side:?} {base_lots} lots at {price_in_ticks} ticks");
    println!("signature {signature}");
    Ok(())
}

/// Cancels every resting order on one side.
pub fn cancel_all(client: &Client, cluster: &str, side: Side, limit: u32) -> Result<()> {
    let record = MarketRecord::load(cluster)?;
    let addresses = record.addresses()?;

    let signature = client.send(
        &[instruction::cancel_all_orders(
            &addresses,
            &client.payer_key(),
            side,
            limit,
        )],
        &[],
    )?;

    println!("cancelled up to {limit} {side:?} orders");
    println!("signature {signature}");
    Ok(())
}
