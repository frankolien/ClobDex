//! The market: configuration, seats, and the book, in one castable value.

mod funds;
mod header;

use bytemuck::{Pod, Zeroable};
use clob_book::{BaseLots, FIFOOrderId, Invariant, LotConfig, OrderBook, QuoteLots, Side};

pub use header::MarketHeader;

use crate::error::{EngineError, Result};
use crate::fees::FeeSchedule;
use crate::trader::{SeatIndex, TraderKey, TraderTable};

/// A single spot market: lot geometry, fee schedule, seats, and both sides of the book.
///
/// Everything a market needs lives in one `repr(C)` value, so on-chain a market account
/// is a single cast away from being operable — no deserialization, and no second account
/// to keep consistent with the first.
///
/// The three capacities are separate const generics. Bid and ask depth is often
/// structurally lopsided, and seat count is unrelated to either; collapsing them into
/// one number would mean paying rent for the maximum of the three on all three.
#[repr(C)]
pub struct Market<const BIDS: usize, const ASKS: usize, const SEATS: usize> {
    header: MarketHeader,
    traders: TraderTable<SEATS>,
    book: OrderBook<BIDS, ASKS>,
}

impl<const BIDS: usize, const ASKS: usize, const SEATS: usize> Clone for Market<BIDS, ASKS, SEATS> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<const BIDS: usize, const ASKS: usize, const SEATS: usize> Copy for Market<BIDS, ASKS, SEATS> {}

// SAFETY: repr(C) over three Pod fields, each 8-aligned with a size that is a multiple
// of 8. ASSERT_NO_PADDING verifies the composition.
unsafe impl<const BIDS: usize, const ASKS: usize, const SEATS: usize> Zeroable
    for Market<BIDS, ASKS, SEATS>
{
}
unsafe impl<const BIDS: usize, const ASKS: usize, const SEATS: usize> Pod
    for Market<BIDS, ASKS, SEATS>
{
}

// ---------------------------------------------------------------------------------
// Construction and access
// ---------------------------------------------------------------------------------

impl<const BIDS: usize, const ASKS: usize, const SEATS: usize> Market<BIDS, ASKS, SEATS> {
    const ASSERT_NO_PADDING: () = assert!(
        core::mem::size_of::<Self>()
            == core::mem::size_of::<MarketHeader>()
                + TraderTable::<SEATS>::SIZE_IN_BYTES
                + OrderBook::<BIDS, ASKS>::SIZE_IN_BYTES,
        "Market has padding between its header, trader table and book"
    );

    /// Account space this market needs, in bytes.
    pub const SIZE_IN_BYTES: usize = core::mem::size_of::<Self>();

    /// An empty market.
    ///
    /// Beware the stack temporary at realistic capacities; prefer
    /// [`Market::new_boxed`] off-chain and a cast from account bytes on-chain.
    ///
    /// # Errors
    ///
    /// [`EngineError::InvalidLotConfig`] or [`EngineError::InvalidFeeRate`].
    pub fn new(lot_config: LotConfig, fees: FeeSchedule) -> Result<Self> {
        let () = Self::ASSERT_NO_PADDING;
        let mut market: Self = Zeroable::zeroed();
        market.header = MarketHeader::new(lot_config, fees)?;
        Ok(market)
    }

    /// An empty market on the heap.
    ///
    /// # Errors
    ///
    /// [`EngineError::InvalidLotConfig`] or [`EngineError::InvalidFeeRate`].
    #[cfg(feature = "std")]
    pub fn new_boxed(lot_config: LotConfig, fees: FeeSchedule) -> Result<std::boxed::Box<Self>> {
        let () = Self::ASSERT_NO_PADDING;
        let mut market: std::boxed::Box<Self> = bytemuck::zeroed_box();
        market.header = MarketHeader::new(lot_config, fees)?;
        Ok(market)
    }

    /// Configuration and running totals.
    #[inline(always)]
    pub const fn header(&self) -> &MarketHeader {
        &self.header
    }

    /// The book.
    #[inline(always)]
    pub const fn book(&self) -> &OrderBook<BIDS, ASKS> {
        &self.book
    }

    /// The seat table.
    #[inline(always)]
    pub const fn traders(&self) -> &TraderTable<SEATS> {
        &self.traders
    }

    /// Claims a seat for `key`, or returns the existing one.
    ///
    /// # Errors
    ///
    /// [`EngineError::SeatTableFull`].
    pub fn claim_seat(&mut self, key: TraderKey) -> Result<SeatIndex> {
        self.traders.claim_seat(key)
    }

    /// Releases an empty seat.
    ///
    /// # Errors
    ///
    /// [`EngineError::SeatNotFound`] or [`EngineError::SeatNotEmpty`].
    pub fn release_seat(&mut self, key: &TraderKey) -> Result<()> {
        self.traders.release_seat(key)
    }

    /// The seat index for `key`, or [`NO_SEAT`](crate::trader::NO_SEAT).
    #[inline]
    pub fn seat_index(&self, key: &TraderKey) -> SeatIndex {
        self.traders.index_of(key)
    }

    /// The lot geometry, for callers converting between lots and atoms.
    #[inline(always)]
    pub const fn lot_config(&self) -> &LotConfig {
        &self.header.lot_config
    }
}

// ---------------------------------------------------------------------------------
// Internal helpers shared by the funds, orders and matching submodules
// ---------------------------------------------------------------------------------

impl<const BIDS: usize, const ASKS: usize, const SEATS: usize> Market<BIDS, ASKS, SEATS> {
    /// The quote value of `base_lots` at `price`.
    #[inline]
    fn quote_value(&self, price: clob_book::Ticks, base_lots: BaseLots) -> Result<QuoteLots> {
        self.header
            .lot_config
            .quote_lots_for(price, base_lots)
            .ok_or(EngineError::Overflow)
    }

    /// Releases the funds backing `base_lots` of a resting order, without touching the
    /// book. The side is read from the order id, so a caller cannot release against the
    /// wrong balance.
    fn unlock_backing(&mut self, id: &FIFOOrderId, seat: SeatIndex, base_lots: BaseLots) -> Result<()> {
        match id.side() {
            Side::Ask => self.traders.state_mut(seat)?.unlock_base(base_lots),
            Side::Bid => {
                let quote = self.quote_value(id.price_in_ticks, base_lots)?;
                self.traders.state_mut(seat)?.unlock_quote(quote)
            }
        }
    }

    /// The seat owning a resting order.
    fn owner_of(&self, id: &FIFOOrderId) -> Result<SeatIndex> {
        self.book
            .get(id)
            .map(|order| order.trader_index as SeatIndex)
            .ok_or(EngineError::OrderNotFound)
    }
}

// ---------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------

/// A market-level invariant that [`Market::check_conservation`] found violated.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConservationError {
    /// Seat base balances do not sum to `base_lots_deposited`.
    BaseNotConserved,
    /// Seat quote balances plus unclaimed fees do not sum to `quote_lots_deposited`.
    QuoteNotConserved,
    /// Resting order sizes do not match the base lots locked across all seats.
    LockedBaseMismatch,
    /// Resting bid values do not match the quote lots locked across all seats.
    LockedQuoteMismatch,
    /// A resting order points at a seat that does not exist.
    OrphanedOrder,
    /// The book's own structure is corrupt.
    Book(Invariant),
}

impl<const BIDS: usize, const ASKS: usize, const SEATS: usize> Market<BIDS, ASKS, SEATS> {
    /// Verifies that the market holds exactly what it thinks it holds.
    ///
    /// This is the invariant the whole engine exists to preserve, stated four ways:
    /// balances sum to the deposited totals, locked funds correspond exactly to resting
    /// orders, and every order has a live owner. If any of these can be broken by a
    /// sequence of operations, the market can be drained.
    ///
    /// O(seats + orders), so it belongs in tests and fuzz harnesses rather than on the
    /// hot path.
    ///
    /// # Errors
    ///
    /// The first [`ConservationError`] found.
    pub fn check_conservation(&self) -> core::result::Result<(), ConservationError> {
        self.book.check().map_err(ConservationError::Book)?;

        let mut base_total = BaseLots::ZERO;
        let mut quote_total = QuoteLots::ZERO;
        let mut base_locked = BaseLots::ZERO;
        let mut quote_locked = QuoteLots::ZERO;

        for (_, state) in self.traders.iter() {
            base_total = base_total.saturating_add(state.total_base_lots());
            quote_total = quote_total.saturating_add(state.total_quote_lots());
            base_locked = base_locked.saturating_add(state.base_lots_locked);
            quote_locked = quote_locked.saturating_add(state.quote_lots_locked);
        }

        if base_total != self.header.base_lots_deposited {
            return Err(ConservationError::BaseNotConserved);
        }
        let quote_accounted = quote_total.saturating_add(self.header.unclaimed_quote_lot_fees);
        if quote_accounted != self.header.quote_lots_deposited {
            return Err(ConservationError::QuoteNotConserved);
        }

        // Every locked lot must be backed by a resting order, and vice versa.
        let mut resting_base = BaseLots::ZERO;
        let mut resting_quote = QuoteLots::ZERO;

        for entry in self.book.asks().iter() {
            if self.traders.state(entry.value.trader_index as SeatIndex).is_err() {
                return Err(ConservationError::OrphanedOrder);
            }
            resting_base = resting_base.saturating_add(entry.value.num_base_lots);
        }
        for entry in self.book.bids().iter() {
            if self.traders.state(entry.value.trader_index as SeatIndex).is_err() {
                return Err(ConservationError::OrphanedOrder);
            }
            let value = self
                .quote_value(entry.key.price_in_ticks, entry.value.num_base_lots)
                .map_err(|_| ConservationError::LockedQuoteMismatch)?;
            resting_quote = resting_quote.saturating_add(value);
        }

        if resting_base != base_locked {
            return Err(ConservationError::LockedBaseMismatch);
        }
        if resting_quote != quote_locked {
            return Err(ConservationError::LockedQuoteMismatch);
        }

        Ok(())
    }
}
