//! Deriving the trade tape from two book snapshots.
//!
//! # Why not replay
//!
//! The obvious approach is to replay the transaction through [`clob_engine`], which is
//! pure and deterministic, and read off the fills it reports. That works for
//! `PlaceOrder` and falls apart for `Swap`, which deposits and withdraws computed
//! amounts around its match — replaying it means reimplementing the program's handler
//! in the indexer, and a second copy of that logic is exactly the drift this codebase
//! has been avoiding everywhere else.
//!
//! # What this does instead
//!
//! Diff the book. An order's price is part of its identity, and its size is in the
//! snapshot, so liquidity that disappeared between two states *is* the tape — prices
//! and sizes exactly, not estimated. No program logic is duplicated, and `Swap` needs
//! no special handling because a swap's fills look like any other taker's.
//!
//! The instructions are consulted for one thing only: which side the taker was on, so
//! removals can be told apart from cancels. That is a two-line read of the decoded
//! instruction, not a reimplementation of it.
//!
//! # Attribution, in order of precedence
//!
//! A single transaction can cancel and take at once, so removals are classified by:
//!
//! 1. an explicit `CancelOrder` or `ReduceOrder` naming the id → cancelled;
//! 2. a `CancelAllOrders` covering that side → cancelled;
//! 3. the side opposite a taker, owned by *someone else* → **a fill**;
//! 4. the side opposite a taker, owned by the taker → self-traded;
//! 5. anything left → cancelled.
//!
//! Rules 3 and 4 look at the same liquidity, and only the owner tells them apart. A
//! self-trade removes liquidity from the opposite side exactly like a fill does — that
//! is *why* it crosses — so side alone cannot distinguish them. Getting this wrong
//! would let anyone inflate a market's reported volume by crossing their own quotes for
//! free, which is why [`ObservedInstruction`] carries the submitting seat.

use std::collections::HashMap;

use clob_book::{BaseLots, FIFOOrderId, Side};
use clob_client::decode::ClobInstruction;
use clob_client::state::{BookOrder, MarketState};

use crate::tape::{BookDelta, Posted, Removal, RemovalReason, Trade};

/// An instruction as observed, with the seat that submitted it.
///
/// The seat is resolved by the caller from the transaction's account keys, because that
/// is where the keys are — the instruction data carries no trader. It is `None` for
/// instructions with no submitter, and for a trader whose seat could not be resolved,
/// in which case the derivation cannot tell a self-trade from a fill and says so by
/// treating the liquidity as traded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedInstruction {
    /// What was submitted.
    pub instruction: ClobInstruction,
    /// Seat index of the submitting trader.
    pub seat: Option<u32>,
}

impl ObservedInstruction {
    /// An instruction whose submitter is known.
    pub fn new(instruction: ClobInstruction, seat: u32) -> Self {
        Self {
            instruction,
            seat: Some(seat),
        }
    }

    /// An instruction with no submitter, or an unresolved one.
    pub fn anonymous(instruction: ClobInstruction) -> Self {
        Self {
            instruction,
            seat: None,
        }
    }
}

/// Derives what one transaction did, from the market before and after it.
///
/// `instructions` are this program's instructions from the transaction, in order.
/// Instructions addressed to other programs are irrelevant and can be left out.
pub fn derive(
    before: &MarketState,
    after: &MarketState,
    instructions: &[ObservedInstruction],
    slot: u64,
) -> BookDelta {
    let context = Context::new(instructions);
    let mut delta = BookDelta {
        fees_earned: after
            .header
            .collected_quote_lot_fees
            .saturating_sub(before.header.collected_quote_lot_fees),
        ..Default::default()
    };

    for side in [Side::Bid, Side::Ask] {
        diff_side(before, after, side, &context, slot, &mut delta);
    }

    // Best price first on each side, then oldest — the order a taker consumed them in,
    // which is the order a tape should read.
    delta.trades.sort_by_key(|t| {
        let price = t.price_in_ticks.as_u64();
        let ranked = match t.taker_side {
            // A bid taker walks asks upward; an ask taker walks bids downward.
            Side::Bid => price,
            Side::Ask => u64::MAX - price,
        };
        (ranked, t.maker_order_id.sequence_number())
    });
    delta
}

/// What the instructions say about how to read the diff.
struct Context {
    /// Each taker in the transaction: the side it was on, and its seat if known.
    takers: Vec<(Side, Option<u32>)>,
    cancelled_ids: Vec<FIFOOrderId>,
    cancel_all_sides: Vec<Side>,
}

impl Context {
    fn new(instructions: &[ObservedInstruction]) -> Self {
        let mut context = Self {
            takers: Vec::new(),
            cancelled_ids: Vec::new(),
            cancel_all_sides: Vec::new(),
        };

        for observed in instructions {
            if let Some(side) = observed.instruction.taker_side() {
                context.takers.push((side, observed.seat));
            }
            match &observed.instruction {
                ClobInstruction::CancelOrder { order_id }
                | ClobInstruction::ReduceOrder { order_id, .. } => {
                    context.cancelled_ids.push(*order_id);
                }
                ClobInstruction::CancelAllOrders { side, .. } => {
                    context.cancel_all_sides.push(*side);
                }
                _ => {}
            }
        }
        context
    }

    /// How liquidity leaving `side` should be read, given who owned it.
    ///
    /// `None` when no taker in this transaction could have consumed it.
    fn consumed_by_taker(&self, side: Side, owner: u32) -> Option<TakerEffect> {
        self.takers
            .iter()
            .find(|(taker_side, _)| *taker_side == side.opposite())
            .map(|(_, seat)| {
                if *seat == Some(owner) {
                    TakerEffect::SelfTrade
                } else {
                    TakerEffect::Fill
                }
            })
    }

    fn explicitly_cancelled(&self, id: &FIFOOrderId) -> bool {
        self.cancelled_ids.contains(id)
    }

    fn cleared(&self, side: Side) -> bool {
        self.cancel_all_sides.contains(&side)
    }
}

/// What a taker did to liquidity it crossed.
#[derive(Copy, Clone, PartialEq, Eq)]
enum TakerEffect {
    /// Someone else's order: value changed hands.
    Fill,
    /// The taker's own order: removed under a self-trade policy, no value moved.
    SelfTrade,
}

fn index(orders: &[BookOrder]) -> HashMap<FIFOOrderId, &BookOrder> {
    orders.iter().map(|order| (order.id, order)).collect()
}

fn diff_side(
    before: &MarketState,
    after: &MarketState,
    side: Side,
    context: &Context,
    slot: u64,
    delta: &mut BookDelta,
) {
    let old = index(before.side(side));
    let new = index(after.side(side));

    for order in before.side(side) {
        let remaining = new
            .get(&order.id)
            .map(|o| o.num_base_lots)
            .unwrap_or(BaseLots::ZERO);
        let removed = order.num_base_lots.saturating_sub(remaining);
        if removed.is_zero() {
            continue;
        }

        // Precedence as documented on the module.
        if context.explicitly_cancelled(&order.id) || context.cleared(side) {
            delta.removals.push(cancelled(order, removed));
        } else {
            match context.consumed_by_taker(side, order.trader_index) {
                Some(TakerEffect::Fill) => {
                    delta.trades.push(trade(before, order, removed, side, slot));
                }
                Some(TakerEffect::SelfTrade) => delta.removals.push(Removal {
                    order_id: order.id,
                    seat: order.trader_index,
                    base_lots: removed,
                    reason: RemovalReason::SelfTraded,
                }),
                None => delta.removals.push(cancelled(order, removed)),
            }
        }
    }

    for order in after.side(side) {
        if !old.contains_key(&order.id) {
            delta.posted.push(Posted {
                order_id: order.id,
                seat: order.trader_index,
                base_lots: order.num_base_lots,
            });
        }
    }
}

fn cancelled(order: &BookOrder, base_lots: BaseLots) -> Removal {
    Removal {
        order_id: order.id,
        seat: order.trader_index,
        base_lots,
        reason: RemovalReason::Cancelled,
    }
}

fn trade(
    before: &MarketState,
    order: &BookOrder,
    base_lots: BaseLots,
    maker_side: Side,
    slot: u64,
) -> Trade {
    let price = order.price_in_ticks();
    Trade {
        slot,
        price_in_ticks: price,
        base_lots,
        // Saturating rather than erroring: a tape entry with a wrong value is worse than
        // one with a clamped value, and the fee reconciliation will flag it either way.
        quote_lots: before
            .lot_config()
            .quote_lots_for(price, base_lots)
            .unwrap_or(clob_book::QuoteLots::MAX),
        maker_order_id: order.id,
        maker_seat: order.trader_index,
        taker_side: maker_side.opposite(),
    }
}
