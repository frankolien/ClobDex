//! Compute measured on a validator, against a market that is actually running.
//!
//! The harness in `programs/clob/tests/compute.rs` executes the same binary and gives
//! exact numbers at exactly the book depth asked for. It is the better instrument. What
//! it cannot do is prove that those numbers are what a validator charges, or be pointed
//! at somebody else's program.
//!
//! This does both. Every sample is a `simulateTransaction` — no signature that spends
//! anything, no cooperation from the venue — so the same procedure works against any
//! market on the cluster. A compute figure is worth something only if the method behind
//! it can be run against whatever it is being compared to.
//!
//! # What it measures against
//!
//! Whatever is on the book right now. It does not place orders to make its own numbers
//! look a particular way: an instruction whose preconditions the current market does not
//! meet is reported as unmeasurable, with the reason. A benchmark that arranges its own
//! book is measuring the book it chose.

use anyhow::{Context, Result};
use clob_book::{BaseLots, FIFOOrderId, QuoteLots, Side, Ticks};
use clob_client::instruction::{self, MarketAddresses, Receipt};
use clob_client::state::MarketState;
use clob_engine::{PostOnlyRejection, TraderKey};
use clob_ops::record::MarketRecord;
use clob_ops::rpc::Client;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

/// One instruction's cost, or why it could not be priced.
struct Sample {
    /// Names the instruction and, where it matters, how much work it was given — a sweep
    /// is only a number worth reading next to the depth it crossed.
    what: String,
    accounts: usize,
    outcome: Result<u64, String>,
}

/// Prices every instruction the current market state allows.
pub fn run(client: &Client, cluster: &str, as_trader: Option<&str>) -> Result<()> {
    let record = MarketRecord::load(cluster)?;
    let addresses = record.addresses()?;
    let (token_base, token_quote) = record.token_accounts_for(as_trader)?;
    let trader = client.payer_key();

    let data = client
        .account_data(&addresses.market)?
        .context("the market account does not exist on this cluster")?;
    let state = MarketState::decode(&data).map_err(|error| anyhow::anyhow!("{error}"))?;
    let seat = state.seat_of(&TraderKey::new(trader.to_bytes()));

    println!("market {}", record.market);
    println!(
        "book   {} bid(s), {} ask(s)",
        state.bids.len(),
        state.asks.len()
    );
    match seat {
        Some(index) => println!("seat   {index}"),
        None => println!("seat   none — fund one with `clob fund` to price more of the table"),
    }
    println!();

    let mut samples = Vec::new();
    let context = Context_ {
        addresses,
        trader,
        token_base,
        token_quote,
        state: &state,
        seat,
    };

    samples.push(measure(client, "claim seat", instruction::claim_seat(&addresses, &trader)));
    samples.extend(funds(client, &context));
    samples.extend(quoting(client, &context));
    samples.extend(cancelling(client, &context));
    samples.push(measure(
        client,
        "collect fees",
        instruction::collect_fees(&addresses, &state.fee_recipient()),
    ));
    samples.extend(taking(client, &context));

    report(&samples);
    Ok(())
}

/// The addresses and market state every sample needs. Named for what it is rather than
/// threaded through six arguments.
struct Context_<'a> {
    addresses: MarketAddresses,
    trader: Pubkey,
    token_base: Pubkey,
    token_quote: Pubkey,
    state: &'a MarketState,
    seat: Option<u32>,
}

/// Moving money in and out.
fn funds(client: &Client, context: &Context_) -> Vec<Sample> {
    let Context_ { addresses, trader, token_base, token_quote, .. } = context;
    vec![
        measure(
            client,
            "deposit",
            instruction::deposit(
                addresses,
                trader,
                token_base,
                token_quote,
                BaseLots(1),
                QuoteLots(1),
            ),
        ),
        measure(
            client,
            "withdraw",
            instruction::withdraw(
                addresses,
                trader,
                token_base,
                token_quote,
                BaseLots(1),
                QuoteLots(1),
            ),
        ),
    ]
}

/// Resting a quote, one at a time and batched.
///
/// Prices are placed outside the current touch so the order rests rather than trades —
/// a post-only that crosses is rejected, and a rejection is not a measurement.
fn quoting(client: &Client, context: &Context_) -> Vec<Sample> {
    let Context_ { addresses, trader, state, .. } = context;
    let (bid, ask) = quote_prices(state);

    let one = instruction::post_only(Side::Bid, bid, BaseLots(1), PostOnlyRejection::Reject);
    let ladder: Vec<_> = (0..4)
        .map(|level| {
            instruction::post_only(
                Side::Bid,
                Ticks(bid.as_u64().saturating_sub(level)),
                BaseLots(1),
                PostOnlyRejection::Reject,
            )
        })
        .collect();

    vec![
        measure(
            client,
            "post-only, rests",
            instruction::place_order(addresses, trader, &one, Receipt::Off),
        ),
        measure(
            client,
            "post-only, with receipt",
            instruction::place_order(
                addresses,
                trader,
                &instruction::post_only(Side::Ask, ask, BaseLots(1), PostOnlyRejection::Reject),
                Receipt::On,
            ),
        ),
        measure(
            client,
            "batch: 0 cancels, 4 places",
            instruction::batch_update(addresses, trader, &[], &ladder),
        ),
    ]
}

/// Taking an order back off the book.
fn cancelling(client: &Client, context: &Context_) -> Vec<Sample> {
    let Context_ { addresses, trader, state, seat, .. } = context;
    let mut samples = Vec::new();

    let ours: Vec<FIFOOrderId> = match seat {
        None => Vec::new(),
        Some(index) => state
            .bids
            .iter()
            .chain(state.asks.iter())
            .filter(|order| order.trader_index == *index)
            .map(|order| order.id)
            .collect(),
    };

    samples.push(match ours.first() {
        Some(id) => measure(
            client,
            "cancel one",
            instruction::cancel_order(addresses, trader, id),
        ),
        None => unmeasurable("cancel one", 2, "nothing of ours is resting"),
    });

    samples.push(match ours.len() >= 4 {
        true => measure(
            client,
            "batch: 4 cancels, 4 places",
            instruction::batch_update(
                addresses,
                trader,
                &ours[..4],
                &(0..4)
                    .map(|level| {
                        let (bid, _) = quote_prices(state);
                        instruction::post_only(
                            Side::Bid,
                            Ticks(bid.as_u64().saturating_sub(level)),
                            BaseLots(1),
                            PostOnlyRejection::Reject,
                        )
                    })
                    .collect::<Vec<_>>(),
            ),
        ),
        false => unmeasurable(
            "batch: 4 cancels, 4 places",
            2,
            "fewer than four of our orders are resting",
        ),
    });

    samples.push(measure(
        client,
        "cancel all, up to 8",
        instruction::cancel_all_orders(addresses, trader, Side::Bid, 8),
    ));
    samples
}

/// Crossing the book, which is the number that decides whether an aggregator routes here.
///
/// Sized to the resting depth so the sweep is real: a taker that fills nothing costs
/// almost nothing and would flatter the table.
fn taking(client: &Client, context: &Context_) -> Vec<Sample> {
    let Context_ { addresses, trader, token_base, token_quote, state, .. } = context;

    let Some(best_ask) = state.best_ask() else {
        return vec![unmeasurable("swap, crossing", 8, "no asks are resting")];
    };
    let worst = state
        .asks
        .last()
        .map(|order| order.price_in_ticks())
        .unwrap_or(best_ask.price_in_ticks());

    let one_level = best_ask.num_base_lots;
    let everything: u64 = state.asks.iter().map(|order| order.num_base_lots.as_u64()).sum();
    let levels = state.level_two(Side::Ask, usize::MAX).len();

    vec![
        measure(
            client,
            "swap, 1 level",
            instruction::swap(
                addresses,
                trader,
                token_base,
                token_quote,
                Side::Bid,
                best_ask.price_in_ticks(),
                one_level,
                BaseLots::ZERO,
                64,
                Receipt::Off,
            ),
        ),
        measure(
            client,
            format!("swap, sweeping {levels} level(s)"),
            instruction::swap(
                addresses,
                trader,
                token_base,
                token_quote,
                Side::Bid,
                worst,
                BaseLots(everything),
                BaseLots::ZERO,
                64,
                Receipt::Off,
            ),
        ),
    ]
}

/// One tick outside the touch on each side, so a post-only rests instead of crossing.
fn quote_prices(state: &MarketState) -> (Ticks, Ticks) {
    let bid = state
        .best_bid()
        .map(|order| order.price_in_ticks().as_u64().saturating_sub(1))
        .unwrap_or(1);
    let ask = state
        .best_ask()
        .map(|order| order.price_in_ticks().as_u64().saturating_add(1))
        .unwrap_or(bid + 2);
    (Ticks(bid.max(1)), Ticks(ask))
}

fn measure(client: &Client, what: impl Into<String>, instruction: Instruction) -> Sample {
    let accounts = instruction.accounts.len();
    let outcome = match client.simulate(&[instruction]) {
        Err(error) => Err(format!("{error:#}")),
        // A reverted instruction reports the compute it burned before reverting, which is
        // not the number being asked for. Reporting it as a cost would be a lie of the
        // most convenient kind — reverts are cheap.
        Ok(simulation) => match (simulation.error, simulation.compute_units) {
            (Some(error), _) => Err(error),
            (None, Some(units)) => Ok(units),
            (None, None) => Err("the endpoint reported no compute".to_string()),
        },
    };
    Sample {
        what: what.into(),
        accounts,
        outcome,
    }
}

fn unmeasurable(what: impl Into<String>, accounts: usize, why: &str) -> Sample {
    Sample {
        what: what.into(),
        accounts,
        outcome: Err(why.to_string()),
    }
}

fn report(samples: &[Sample]) {
    println!("  instruction                          accounts       CU");
    println!("  ----------------------------------------------------------");
    for sample in samples {
        match &sample.outcome {
            Ok(units) => println!(
                "  {:<36} {:>5}   {:>6}",
                sample.what, sample.accounts, units
            ),
            Err(why) => println!("  {:<36} {:>5}        -   {why}", sample.what, sample.accounts),
        }
    }
    println!();
    println!("Simulated, not sent. Compute is the program's; a landed transaction also pays");
    println!("per-signature and per-account costs that no venue controls.");
}
