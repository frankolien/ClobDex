//! Public order entry points: place, cancel, reduce.

use clob_book::{BaseLots, FIFOOrderId, RestingOrder, Side, Ticks};

use super::Market;
use crate::error::{EngineError, Result};
use crate::fill::{FillObserver, MatchStop, OrderOutcome};
use crate::order::{OrderPacket, PostOnlyRejection};
use crate::trader::SeatIndex;

impl<const BIDS: usize, const ASKS: usize, const SEATS: usize> Market<BIDS, ASKS, SEATS> {
    /// Submits an order on behalf of `seat`.
    ///
    /// Crossing happens first and posting second, so a limit order that crosses is
    /// filled at the resting prices before its remainder joins the queue.
    ///
    /// # Errors do not roll back
    ///
    /// An `Err` from this method may leave the market mutated — specifically
    /// [`EngineError::MinimumFillNotMet`], which is only knowable after matching has
    /// already settled some fills. The engine has no undo log, and building one would
    /// mean buffering every fill for a rollback that a Solana program never needs.
    ///
    /// On-chain this is free and correct: a returned error aborts the instruction and
    /// the runtime discards every account write, which is exactly the all-or-nothing
    /// semantic fill-or-kill promises. **Any caller outside that model — a simulator, an
    /// off-chain matcher, a backtester — must discard the market on error rather than
    /// continue using it.** Value stays conserved either way; what breaks is the
    /// caller's expectation that a rejected order changed nothing.
    ///
    /// # Errors
    ///
    /// [`EngineError::ZeroSize`], [`EngineError::SeatNotFound`],
    /// [`EngineError::BookSideFull`], insufficient-funds errors when posting,
    /// [`EngineError::SelfTradeAborted`], [`EngineError::PostOnlyWouldCross`],
    /// [`EngineError::PostOnlyNoRoom`], or [`EngineError::MinimumFillNotMet`].
    pub fn place_order<O: FillObserver>(
        &mut self,
        seat: SeatIndex,
        packet: OrderPacket,
        observer: &mut O,
    ) -> Result<OrderOutcome> {
        if packet.num_base_lots().is_zero() {
            return Err(EngineError::ZeroSize);
        }
        // Fail before any mutation if the seat is bogus.
        self.traders.state(seat)?;

        match packet {
            OrderPacket::PostOnly {
                side,
                price_in_ticks,
                num_base_lots,
                rejection,
            } => self.place_post_only(seat, side, price_in_ticks, num_base_lots, rejection),

            OrderPacket::Limit {
                side,
                price_in_ticks,
                num_base_lots,
                self_trade_behavior,
                match_limit,
            } => {
                let cross = self.cross(
                    seat,
                    side,
                    Some(price_in_ticks),
                    num_base_lots,
                    self_trade_behavior,
                    match_limit,
                    observer,
                )?;

                let mut outcome = OrderOutcome {
                    order_id: None,
                    base_lots_filled: cross.base_lots_filled,
                    quote_lots_filled: cross.quote_lots_filled,
                    fee_in_quote_lots: cross.fee_in_quote_lots,
                    base_lots_posted: BaseLots::ZERO,
                    base_lots_self_traded: cross.base_lots_self_traded,
                    stop: cross.stop,
                };

                if !cross.remaining.is_zero() {
                    outcome.order_id =
                        Some(self.post(seat, side, price_in_ticks, cross.remaining)?);
                    outcome.base_lots_posted = cross.remaining;
                }
                Ok(outcome)
            }

            OrderPacket::ImmediateOrCancel {
                side,
                price_in_ticks,
                num_base_lots,
                min_base_lots_to_fill,
                self_trade_behavior,
                match_limit,
            } => {
                let cross = self.cross(
                    seat,
                    side,
                    price_in_ticks,
                    num_base_lots,
                    self_trade_behavior,
                    match_limit,
                    observer,
                )?;

                // Checked after matching so the error carries the real shortfall, but
                // returning Err discards every mutation: on-chain the whole instruction
                // reverts, which is exactly the all-or-nothing semantic asked for.
                if cross.base_lots_filled < min_base_lots_to_fill {
                    return Err(EngineError::MinimumFillNotMet);
                }

                Ok(OrderOutcome {
                    order_id: None,
                    base_lots_filled: cross.base_lots_filled,
                    quote_lots_filled: cross.quote_lots_filled,
                    fee_in_quote_lots: cross.fee_in_quote_lots,
                    base_lots_posted: BaseLots::ZERO,
                    base_lots_self_traded: cross.base_lots_self_traded,
                    stop: cross.stop,
                })
            }
        }
    }

    /// Posts without taking, applying the caller's cross policy.
    fn place_post_only(
        &mut self,
        seat: SeatIndex,
        side: Side,
        price_in_ticks: Ticks,
        num_base_lots: BaseLots,
        rejection: PostOnlyRejection,
    ) -> Result<OrderOutcome> {
        let price = match self.non_crossing_price(side, price_in_ticks) {
            Some(price) => price,
            None => return Err(EngineError::PostOnlyNoRoom),
        };

        if price != price_in_ticks && rejection == PostOnlyRejection::Reject {
            return Err(EngineError::PostOnlyWouldCross);
        }

        let order_id = self.post(seat, side, price, num_base_lots)?;
        Ok(OrderOutcome {
            order_id: Some(order_id),
            base_lots_posted: num_base_lots,
            ..OrderOutcome::empty(MatchStop::DidNotCross)
        })
    }

    /// The given price, or the nearest non-crossing one, or `None` if there is no room.
    ///
    /// Sliding one tick inside the touch is the smallest move that stops the order
    /// crossing, which keeps a slid quote as aggressive as it can legally be.
    fn non_crossing_price(&self, side: Side, price: Ticks) -> Option<Ticks> {
        match side {
            Side::Bid => match self.book.best_ask() {
                Some(ask) if price >= ask.key.price_in_ticks => {
                    ask.key.price_in_ticks.as_u64().checked_sub(1).and_then(|p| {
                        // A bid at zero ticks is worthless, so there is nowhere to slide.
                        (p > 0).then_some(Ticks(p))
                    })
                }
                _ => Some(price),
            },
            Side::Ask => match self.book.best_bid() {
                Some(bid) if price <= bid.key.price_in_ticks => bid
                    .key
                    .price_in_ticks
                    .as_u64()
                    .checked_add(1)
                    .map(Ticks),
                _ => Some(price),
            },
        }
    }

    /// Locks the backing funds and rests the order.
    ///
    /// If the book rejects the order the lock is reversed, so a full side leaves the
    /// seat exactly as it was.
    fn post(
        &mut self,
        seat: SeatIndex,
        side: Side,
        price: Ticks,
        size: BaseLots,
    ) -> Result<FIFOOrderId> {
        match side {
            Side::Ask => self.traders.state_mut(seat)?.lock_base(size)?,
            Side::Bid => {
                let cost = self.quote_value(price, size)?;
                self.traders.state_mut(seat)?.lock_quote(cost)?;
            }
        }

        let order = RestingOrder::new(seat as u64, size);
        match self.book.place(side, price, order) {
            Some(id) => Ok(id),
            None => {
                match side {
                    Side::Ask => self.traders.state_mut(seat)?.unlock_base(size)?,
                    Side::Bid => {
                        let cost = self.quote_value(price, size)?;
                        self.traders.state_mut(seat)?.unlock_quote(cost)?;
                    }
                }
                Err(EngineError::BookSideFull)
            }
        }
    }

    /// Cancels a resting order, returning the size removed and releasing its funds.
    ///
    /// # Errors
    ///
    /// [`EngineError::OrderNotFound`] or [`EngineError::NotOrderOwner`].
    pub fn cancel_order(&mut self, seat: SeatIndex, id: &FIFOOrderId) -> Result<BaseLots> {
        let owner = self.owner_of(id)?;
        if owner != seat {
            return Err(EngineError::NotOrderOwner);
        }

        let size = self
            .book
            .get(id)
            .ok_or(EngineError::OrderNotFound)?
            .num_base_lots;

        self.unlock_backing(id, seat, size)?;
        self.book.cancel(id);
        Ok(size)
    }

    /// Shrinks a resting order by up to `base_lots`, releasing the funds behind the
    /// removed size. Reducing by at least the resting size is exactly a cancel.
    ///
    /// Queue position is preserved, so a maker can trim size without going to the back.
    ///
    /// # Errors
    ///
    /// [`EngineError::OrderNotFound`] or [`EngineError::NotOrderOwner`].
    pub fn reduce_order(
        &mut self,
        seat: SeatIndex,
        id: &FIFOOrderId,
        base_lots: BaseLots,
    ) -> Result<BaseLots> {
        let owner = self.owner_of(id)?;
        if owner != seat {
            return Err(EngineError::NotOrderOwner);
        }

        let resting = self
            .book
            .get(id)
            .ok_or(EngineError::OrderNotFound)?
            .num_base_lots;
        let removed = resting.min(base_lots);

        self.unlock_backing(id, seat, removed)?;
        self.book.reduce(id, removed);
        Ok(removed)
    }

    /// Cancels up to `limit` of a seat's resting orders on `side`, best-priced first,
    /// and reports how many went.
    ///
    /// Bounded rather than unbounded because an unbounded cancel-all on a large book can
    /// exceed the compute budget and revert, leaving the maker unable to pull quotes at
    /// exactly the moment it most needs to. A bounded call always makes progress.
    pub fn cancel_orders_for_seat(
        &mut self,
        seat: SeatIndex,
        side: Side,
        limit: u32,
    ) -> Result<u32> {
        let mut cancelled = 0u32;

        while cancelled < limit {
            // Re-scan each time: cancelling mutates the tree, which invalidates any
            // iterator held across the call.
            let Some(id) = self
                .book
                .iter_side(side)
                .find(|entry| entry.value.trader_index as SeatIndex == seat)
                .map(|entry| entry.key)
            else {
                break;
            };
            self.cancel_order(seat, &id)?;
            cancelled += 1;
        }

        Ok(cancelled)
    }
}
