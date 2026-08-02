//! What the socket says the market is.
//!
//! A pure reducer over the four message kinds, with no terminal and no network in it, so
//! the parts that are hard to get right can be tested without either.
//!
//! Two of those kinds are the reason this is a state machine rather than a cache:
//!
//! - **`retract`** — fills already on screen did not happen, because the slot that produced
//!   them was abandoned. A viewer that ignores this shows volume that never traded.
//! - **`lagged`** — this subscriber fell behind and the server dropped messages rather than
//!   stalling ingest for everyone. Only a fresh snapshot fixes a book that is wrong by an
//!   unknown amount, so this asks for a reconnect.
//!
//! Neither fires while watching a quiet test market. Both fire in production.

use crate::wire;

/// How many fills to keep. Enough to fill any terminal, bounded so a busy market cannot
/// grow this without limit.
const TAPE: usize = 256;

/// One price level, parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub price_in_ticks: u64,
    pub base_lots: u64,
}

/// One fill, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    pub slot: u64,
    pub price_in_ticks: u64,
    pub base_lots: u64,
    pub taker_is_bid: bool,
    pub maker_seat: u32,
    pub taker_seat: Option<u32>,
}

/// What the viewer should do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Carry on.
    None,
    /// The subscriber fell behind. Reconnect for a fresh snapshot.
    Resubscribe,
}

/// The market as this client currently believes it to be.
#[derive(Clone, Debug, Default)]
pub struct Feed {
    /// Bids, best first.
    pub bids: Vec<Level>,
    /// Asks, best first.
    pub asks: Vec<Level>,
    /// Slot the book came from.
    pub slot: u64,
    /// Everything at or below this is rooted.
    pub finalized_through: u64,
    /// Recent fills, newest first.
    pub tape: Vec<Fill>,
    /// Whether a snapshot has arrived at all.
    pub ready: bool,
    /// Fills withdrawn because their slot was abandoned, for the whole session.
    pub retracted: u64,
    /// Messages the server dropped for this subscriber.
    pub missed: u64,
}

impl Feed {
    /// Applies one message.
    ///
    /// A malformed quantity drops that message rather than the connection: one unreadable
    /// frame should cost a frame, not the session.
    pub fn apply(&mut self, message: wire::Message) -> Action {
        match message {
            wire::Message::Snapshot {
                slot,
                finalized_through,
                bids,
                asks,
            } => {
                // Assigned, never merged. A snapshot is the whole truth as of its slot, and
                // combining it with what was already here would keep whatever was wrong.
                self.bids = levels(&bids);
                self.asks = levels(&asks);
                self.slot = slot;
                self.finalized_through = finalized_through;
                self.ready = true;
            }

            wire::Message::Update {
                slot,
                trades,
                bids,
                asks,
                finalized_through,
            } => {
                // The feed sends the whole top of book rather than a patch, so this
                // replaces rather than applies a diff. Patching would be a second
                // implementation of the book, and one that only goes wrong after some
                // unpredictable sequence is the hardest kind to notice.
                self.bids = levels(&bids);
                self.asks = levels(&asks);
                self.slot = slot;
                self.finalized_through = finalized_through;

                for trade in trades.iter().rev() {
                    if let Some(fill) = fill(trade) {
                        self.tape.insert(0, fill);
                    }
                }
                self.tape.truncate(TAPE);
            }

            wire::Message::Retract { slot, trades } => {
                // Removed from the tape, not marked. The slot was abandoned, so these did
                // not happen — leaving them visible with a label would still have them
                // counted by anyone reading the column.
                let before = self.tape.len();
                self.tape.retain(|fill| fill.slot != slot);
                let removed = (before - self.tape.len()) as u64;
                self.retracted += removed.max(trades as u64);
            }

            wire::Message::Lagged { missed } => {
                self.missed += missed;
                // The book is now wrong by an unknown amount and no later update carries
                // what was dropped. Only a new snapshot fixes it.
                self.ready = false;
                return Action::Resubscribe;
            }
        }
        Action::None
    }

    /// Best bid, if the side has liquidity.
    pub fn best_bid(&self) -> Option<u64> {
        self.bids.first().map(|level| level.price_in_ticks)
    }

    /// Best ask, if the side has liquidity.
    pub fn best_ask(&self) -> Option<u64> {
        self.asks.first().map(|level| level.price_in_ticks)
    }

    /// Ask minus bid, when both sides have liquidity.
    ///
    /// `None` on a one-sided book rather than a number computed from one side, which would
    /// be invented.
    pub fn spread(&self) -> Option<u64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask.saturating_sub(bid)),
            _ => None,
        }
    }

    /// Midpoint, when both sides have liquidity.
    pub fn mid(&self) -> Option<u64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid + ask) / 2),
            _ => None,
        }
    }

    /// Whether a fill's slot is rooted.
    ///
    /// Derived from the current watermark rather than read off the fill. The server stamps
    /// finality when it sends, so a fill that arrived provisional and has since rooted still
    /// says otherwise on the message it came in — trusting that leaves every print marked
    /// uncertain forever and the flag stops meaning anything.
    pub fn is_final(&self, fill: &Fill) -> bool {
        fill.slot <= self.finalized_through
    }
}

fn levels(wire: &[wire::Level]) -> Vec<Level> {
    wire.iter()
        .filter_map(|level| {
            Some(Level {
                price_in_ticks: level.price_in_ticks.trim().parse().ok()?,
                base_lots: level.base_lots.trim().parse().ok()?,
            })
        })
        .collect()
}

fn fill(trade: &wire::Trade) -> Option<Fill> {
    Some(Fill {
        slot: trade.slot,
        price_in_ticks: trade.price_in_ticks.trim().parse().ok()?,
        base_lots: trade.base_lots.trim().parse().ok()?,
        taker_is_bid: trade.taker_side == "bid",
        maker_seat: trade.maker_seat,
        taker_seat: trade.taker_seat,
    })
}
