//! One turn of the loop: read the market, decide, send.
//!
//! Everything decided here is decided by [`clob_mm`], which is pure. This module supplies
//! it with a market and does what it says — the split exists so that the interesting part
//! of a trading bot can be tested without a cluster.
//!
//! # Quotes are post-only, and rejected rather than slid
//!
//! Sliding would post at the best non-crossing tick instead of failing, which sounds like
//! the accommodating choice and is not. The bot would then be resting at a price it did
//! not choose, the next cycle would measure that as drift, and it would re-quote — into
//! the same slide. A maker that thrashes is worse than one that occasionally misses a
//! refresh and says so.
//!
//! A rejection is not fatal. The ladder cannot cross the book it was built from, but that
//! book is a slot or two old by the time the transaction lands, and somebody posting
//! inside the spread in between is ordinary. The cycle fails, the next one re-reads, and
//! the position is unchanged throughout.

use anyhow::{Context, Result};
use clob_book::Side;
use clob_client::instruction::{self, MarketAddresses};
use clob_client::state::MarketState;
use clob_engine::{OrderPacket, PostOnlyRejection, TraderKey};
use clob_mm::fair::{self, Fair};
use clob_mm::inventory::Inventory;
use clob_mm::ladder::{self, Quote};
use clob_mm::plan::{self, Plan, Resting};
use clob_mm::Params;
use clob_ops::Client;
use solana_pubkey::Pubkey;

/// How many orders per side a shutdown cancel is allowed to walk.
///
/// Generous rather than exact: the ladder is at most [`MAX_LEVELS`] deep, and a bot
/// shutting down would rather spend compute than leave a quote behind.
///
/// [`MAX_LEVELS`]: clob_mm::params::MAX_LEVELS
const SHUTDOWN_CANCEL_LIMIT: u32 = 64;

/// What one cycle did.
pub struct Report {
    /// The price the ladder was built around.
    pub fair: Fair,
    /// Ticks the ladder was shifted by inventory.
    pub skew_in_ticks: i64,
    /// The position, measured against target.
    pub inventory: Inventory,
    /// What the bot wants resting.
    pub desired: Vec<Quote>,
    /// What was already resting.
    pub resting: usize,
    /// What it decided to do.
    pub plan: Plan,
    /// The transaction, if one was sent.
    pub signature: Option<String>,
}

/// Everything a cycle needs that does not change between cycles.
pub struct Session {
    client: Client,
    addresses: MarketAddresses,
    trader: Pubkey,
    params: Params,
    dry_run: bool,
}

impl Session {
    /// Binds a strategy to a market and a signer.
    pub fn new(
        client: Client,
        addresses: MarketAddresses,
        params: Params,
        dry_run: bool,
    ) -> Self {
        Self {
            trader: client.payer_key(),
            client,
            addresses,
            params,
            dry_run,
        }
    }

    /// The wallet quoting.
    pub fn trader(&self) -> Pubkey {
        self.trader
    }

    /// Claims a seat, which the bot cannot quote without.
    ///
    /// Idempotent on chain, so it is sent unconditionally at startup rather than after a
    /// read that would only be able to answer for the slot it was taken at.
    pub fn claim_seat(&self) -> Result<String> {
        self.client
            .send(&[instruction::claim_seat(&self.addresses, &self.trader)], &[])
            .context("claiming a seat")
    }

    /// Reads the market, decides, and sends if there is anything to send.
    pub fn cycle(&self) -> Result<Report> {
        let state = self.read_market()?;
        let key = TraderKey::new(self.trader.to_bytes());
        let seat = state.seat_of(&key);
        // A seat that does not exist yet holds nothing, which is exactly what an empty
        // `TraderState` says. The ladder's budget then funds no levels, so a bot with no
        // seat quotes nothing rather than building orders it cannot back.
        let balances = state.trader(&key).copied().unwrap_or_default();

        let touch = fair::Touch::excluding(&state, seat);
        let fair = fair::price(&touch, self.params.reference_in_ticks);
        let inventory = Inventory::of(&balances, &self.params);
        let desired = ladder::build(
            &self.params,
            &fair,
            &inventory,
            &balances,
            state.lot_config(),
        );
        let resting = seat.map(|seat| Resting::owned_by(&state, seat)).unwrap_or_default();
        let plan = plan::decide(&resting, &desired, self.params.drift_tolerance_in_ticks);

        let signature = match &plan {
            Plan::Hold => None,
            Plan::Replace { cancels, places, .. } if !self.dry_run => {
                Some(self.send_refresh(cancels, places)?)
            }
            Plan::Replace { .. } => None,
        };

        Ok(Report {
            fair,
            skew_in_ticks: inventory.skew_in_ticks(&self.params),
            inventory,
            desired,
            resting: resting.len(),
            plan,
            signature,
        })
    }

    /// Cancels everything the bot has resting, on both sides.
    ///
    /// For shutdown. Quotes outlive the process that placed them, and an abandoned ladder
    /// is still executable at prices nothing is maintaining — the market gets to trade
    /// against a bot's last opinion for as long as it stays wrong.
    ///
    /// `cancel_all_orders` rather than a batch of IDs, because it does not need the book
    /// read first: shutting down is the worst time to depend on an RPC round trip.
    pub fn withdraw_quotes(&self) -> Result<String> {
        self.client
            .send(
                &[
                    instruction::cancel_all_orders(
                        &self.addresses,
                        &self.trader,
                        Side::Bid,
                        SHUTDOWN_CANCEL_LIMIT,
                    ),
                    instruction::cancel_all_orders(
                        &self.addresses,
                        &self.trader,
                        Side::Ask,
                        SHUTDOWN_CANCEL_LIMIT,
                    ),
                ],
                &[],
            )
            .context("cancelling the ladder")
    }

    fn read_market(&self) -> Result<MarketState> {
        let data = self
            .client
            .account_data(&self.addresses.market)?
            .context("the market account does not exist on this cluster")?;
        MarketState::decode(&data).map_err(|error| anyhow::anyhow!("{error}"))
    }

    fn send_refresh(
        &self,
        cancels: &[clob_book::FIFOOrderId],
        places: &[Quote],
    ) -> Result<String> {
        let packets: Vec<OrderPacket> = places.iter().map(post_only).collect();
        let instruction =
            instruction::batch_update(&self.addresses, &self.trader, cancels, &packets);
        self.client.send(&[instruction], &[]).context("refreshing the ladder")
    }
}

/// A quote as an order that will rest or be refused, never take.
fn post_only(quote: &Quote) -> OrderPacket {
    instruction::post_only(
        quote.side,
        quote.price_in_ticks,
        quote.base_lots,
        PostOnlyRejection::Reject,
    )
}
